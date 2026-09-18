//! Rattrapage des films suivis et manquants **par identifiant TMDB**, chez un seul indexer (C411), via Prowlarr.
//!
//! La recherche de Radarr reste la voie normale : elle interroge déjà par identifiant et rattache une
//! release française (« Souvenirs de Marnie ») au bon film. Cette tâche ne prend que ce qui manque encore
//! `missing_hours` après l'ajout (release absente au moment de la demande, indexeur bloqué ce jour-là…) :
//! requête `{TmdbId}` → releases portant cet identifiant → `parse` Radarr (qualité) → mêmes garde-fous
//! que les séries (français, qualité du profil ≤ 1080p, sources) → `release/push`, ou qBittorrent avec
//! l'étiquette `homelab:movie=<id>` si Radarr refuse pour une raison d'identification ou d'indexeur.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::series_search::{
    acceptable, allowed_qualities, due, lang_rank, send_release, size_ok, tmdb_matches, Target,
    Throttle,
};
use super::{Report, Task};
use crate::clients::{ArrClient, ProwlarrClient};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, SeasonSearchRecord};

pub struct MovieSearch;

/// Film à rattraper : suivi, sans fichier, sorti, ajouté depuis assez longtemps, hors file d'attente.
pub fn wanted(movie: &Value, queued: &HashSet<i64>, now_secs: i64, missing_hours: i64) -> bool {
    let id = movie.get("id").and_then(Value::as_i64).unwrap_or(0);
    let added = movie
        .get("added")
        .and_then(Value::as_str)
        .and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok())
        .map(|d| d.timestamp())
        .unwrap_or(now_secs);
    movie.get("monitored").and_then(Value::as_bool) == Some(true)
        && movie.get("hasFile").and_then(Value::as_bool) != Some(true)
        && movie.get("isAvailable").and_then(Value::as_bool) == Some(true)
        && movie.get("tmdbId").and_then(Value::as_i64).unwrap_or(0) > 0
        && !queued.contains(&id)
        && now_secs - added >= missing_hours * 3600
}

/// Meilleure release d'un film : identifiant TMDB, français, qualité acceptable ; tri langue, résolution,
/// sources. Le codec ne compte pas (voir `series_search::choose`). `items` : (résultat Prowlarr,
/// `parsedMovieInfo` Radarr).
pub fn best_movie_release<'a>(
    items: &'a [(Value, Value)],
    tmdb_id: i64,
    allowed: &HashSet<i64>,
    max_gb: f64,
    allow_vo: bool,
) -> Option<(&'a Value, Value)> {
    items
        .iter()
        .filter_map(|(r, info)| {
            let title = r.get("title").and_then(Value::as_str)?;
            r.get("downloadUrl").and_then(Value::as_str)?;
            if !tmdb_matches(r, tmdb_id) {
                return None;
            }
            let lang = lang_rank(title);
            let quality = info.get("quality").cloned().unwrap_or(Value::Null);
            let res = quality
                .pointer("/quality/resolution")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let seeders = r.get("seeders").and_then(Value::as_i64).unwrap_or(0);
            let size = r
                .get("size")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0);
            if !size_ok(size, 1, max_gb) || (lang == 0 && !allow_vo) {
                return None;
            }
            let release = json!({
                "title": title,
                "downloadUrl": r.get("downloadUrl"),
                "publishDate": r.get("publishDate"),
                "quality": quality,
                // sert à retrouver un torrent déjà présent dans qBittorrent (titre re-demandé)
                "infoHash": r.get("infoHash"),
            });
            acceptable(&release, res, seeders, allowed).then_some((
                r,
                release,
                // le codec ne départage rien : voir series_search::choose (mesures du 2026-09-18)
                (lang, res, seeders >= 2, seeders),
            ))
        })
        .max_by_key(|(_, _, rank)| *rank)
        .map(|(r, release, _)| (r, release))
}

async fn process_movie(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    movie: &Value,
    throttle: &mut Throttle,
) -> Result<(String, String)> {
    let cfg = &ctx.cfg.tasks.movie_search;
    let id = movie.get("id").and_then(Value::as_i64).unwrap_or(0);
    let tmdb = movie.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let title = movie.get("title").and_then(Value::as_str).unwrap_or("?");
    if !throttle.take().await {
        return Ok(("pending".into(), String::new()));
    }
    let Some(found) = crate::indexer::search_tmdb(ctx, prow, tmdb, None, false).await? else {
        return Ok(("pending".into(), String::new()));
    };
    let mut items = Vec::new();
    for r in found {
        if !tmdb_matches(&r, tmdb) {
            continue;
        }
        let Some(t) = r.get("title").and_then(Value::as_str) else {
            continue;
        };
        let parse = arr.parse(t).await?;
        let info = parse.get("parsedMovieInfo").cloned().unwrap_or_default();
        items.push((r, info));
    }
    let profile_id = movie
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let allowed = allowed_qualities(&arr.quality_profile(profile_id).await?);
    let ix = &ctx.cfg.indexers;
    let Some((_, release)) = best_movie_release(
        &items,
        tmdb,
        &allowed,
        ix.max_gb_per_movie,
        ix.allow_no_french,
    ) else {
        return Ok((
            "none".into(),
            format!(
                "{} release(s) {}, aucune acceptable",
                items.len(),
                cfg.indexer
            ),
        ));
    };
    let rtitle = release
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    if ctx.dry_run {
        info!(task = "movie_search", service = arr.name, movie = title, release = %rtitle, "dry-run: would send");
        return Ok(("dry_run".into(), rtitle));
    }
    send_release(
        ctx,
        prow,
        arr,
        &rtitle,
        &release,
        &cfg.indexer,
        Target::Movie { movie_id: id },
    )
    .await
}

#[async_trait]
impl Task for MovieSearch {
    fn name(&self) -> &'static str {
        "movie_search"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.movie_search.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.movie_search;
        let Some(prow) = &ctx.prowlarr else {
            return Ok(Report::new("prowlarr non configuré (PROWLARR_API_KEY)", 0));
        };
        // une machine hors de `[downloads] auto_sides` ne prend plus rien de neuf
        let radarrs: Vec<&ArrClient> = std::iter::once(&ctx.radarr)
            .chain(ctx.seedbox_radarr.as_ref())
            .filter(|a| ctx.cfg.downloads.may_grab(a.name))
            .collect();
        let records = ctx.state.read(|s| s.movie_search.clone()).await;
        let t = now();
        let mut todo: Vec<(&ArrClient, Value)> = Vec::new();
        for arr in radarrs {
            let prepared = async {
                let queued: HashSet<i64> = arr
                    .queue_records()
                    .await?
                    .iter()
                    .filter_map(|r| r.get("movieId").and_then(Value::as_i64))
                    .collect();
                anyhow::Ok((arr.movies().await?, queued))
            }
            .await;
            match prepared {
                Ok((movies, queued)) => {
                    for m in movies {
                        let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
                        if wanted(&m, &queued, t, cfg.missing_hours)
                            && due(
                                records.get(&format!("{}:{id}", arr.name)),
                                t,
                                cfg.retry_after_hours,
                                cfg.retry_after_hours,
                                cfg.error_retry_hours,
                                15,
                            )
                        {
                            todo.push((arr, m));
                        }
                    }
                }
                Err(e) => {
                    warn!(task = "movie_search", service = arr.name, error = %e, "planning failed")
                }
            }
        }
        // ajoutés le plus récemment d'abord
        todo.sort_by(|a, b| {
            let d = |m: &Value| {
                m.get("added")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            d(&b.1).cmp(&d(&a.1))
        });
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut throttle = Throttle::new(cfg.max_per_run, cfg.query_gap_secs);
        for (arr, movie) in &todo {
            let id = movie.get("id").and_then(Value::as_i64).unwrap_or(0);
            let (outcome, detail) = match process_movie(ctx, prow, arr, movie, &mut throttle).await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(task = "movie_search", service = arr.name, movie_id = id, error = %e, "movie failed");
                    ("error".into(), format!("{e:#}").chars().take(200).collect())
                }
            };
            *counts.entry(outcome.clone()).or_default() += 1;
            if outcome == "pending" {
                continue;
            }
            let title = movie.get("title").and_then(Value::as_str).unwrap_or("?");
            info!(task = "movie_search", service = arr.name, movie = title, %outcome, %detail, "movie searched");
            if !ctx.dry_run {
                let rec = SeasonSearchRecord {
                    at: now(),
                    outcome,
                    detail,
                    title: title.to_string(),
                    uncovered: Vec::new(),
                };
                let k = format!("{}:{id}", arr.name);
                ctx.state
                    .update(|st| st.movie_search.insert(k, rec))
                    .await?;
            }
        }
        let grabbed = counts.get("grabbed").copied().unwrap_or(0);
        let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let summary = if summary.is_empty() {
            "rien à rattraper".to_string()
        } else {
            summary.join(" ")
        };
        Ok(Report::new(summary, grabbed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(id: i64, monitored: bool, has_file: bool, added: &str) -> Value {
        json!({"id": id, "tmdbId": 100 + id, "monitored": monitored, "hasFile": has_file,
               "isAvailable": true, "added": added})
    }

    #[test]
    fn wanted_movies_are_missing_long_enough() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-17T12:00:00Z")
            .unwrap()
            .timestamp();
        let q: HashSet<i64> = [3].into_iter().collect();
        assert!(wanted(
            &movie(1, true, false, "2026-09-15T12:00:00Z"),
            &q,
            now,
            24
        ));
        assert!(!wanted(
            &movie(2, true, false, "2026-09-17T06:00:00Z"),
            &q,
            now,
            24
        )); // trop récent
        assert!(!wanted(
            &movie(3, true, false, "2026-09-10T12:00:00Z"),
            &q,
            now,
            24
        )); // en file
        assert!(!wanted(
            &movie(4, false, false, "2026-09-10T12:00:00Z"),
            &q,
            now,
            24
        )); // non suivi
        assert!(!wanted(
            &movie(5, true, true, "2026-09-10T12:00:00Z"),
            &q,
            now,
            24
        )); // déjà là
    }

    #[test]
    fn best_release_by_id_language_and_quality() {
        let r = |t: &str, tmdb: i64, seeders: i64| json!({"title": t, "tmdbId": tmdb, "seeders": seeders, "downloadUrl": "http://p/dl"});
        let q = |id: i64, res: i64| json!({"quality": {"quality": {"id": id, "resolution": res}}});
        let items = vec![
            (
                r(
                    "Souvenirs.De.Marnie.2014.MULTI.VFF.1080p.BluRay.x264",
                    83389,
                    20,
                ),
                q(7, 1080),
            ),
            (
                r("Souvenirs.De.Marnie.2014.VOSTFR.1080p.x264", 83389, 90),
                q(7, 1080),
            ),
            (
                r("Souvenirs.De.Marnie.2014.MULTI.VFF.2160p", 83389, 50),
                q(19, 2160),
            ),
            (r("Autre.Film.2014.MULTI.VFF.1080p", 12345, 99), q(7, 1080)),
        ];
        let allowed: HashSet<i64> = [7].into_iter().collect();
        let (_, rel) = best_movie_release(&items, 83389, &allowed, 25.0, true).unwrap();
        assert_eq!(
            rel["title"],
            "Souvenirs.De.Marnie.2014.MULTI.VFF.1080p.BluRay.x264"
        );
        assert!(best_movie_release(&items, 99999, &allowed, 25.0, true).is_none());
    }
}
