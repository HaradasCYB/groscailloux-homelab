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
    acceptable, allowed_qualities, audio_rank, codec_rank, due, lang_rank, send_release, size_ok,
    tmdb_matches, Target, Throttle,
};
use super::{Report, Task};
use crate::clients::{ArrClient, ProwlarrClient};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, SeasonSearchRecord};

pub struct MovieSearch;

/// Requêtes pour un tracker public français (World-torrent) : il ne trouve que le titre **français**, et la
/// ponctuation le perd (« Les Gardiens de la Galaxie Vol. 2 » → 0, « Les Gardiens de la Galaxie 2017 » → 5).
/// Titre nettoyé + année, puis début du titre (avant « : », « - », « Vol ») + année.
pub fn fallback_queries(fr_title: &str, year: i64) -> Vec<String> {
    let clean = |t: &str| {
        t.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '\'' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let y = if year > 0 {
        format!(" {year}")
    } else {
        String::new()
    };
    let mut out = vec![format!("{}{y}", clean(fr_title))];
    let lower = fr_title.to_lowercase();
    let cut = [":", " - ", " vol", " –"]
        .iter()
        .filter_map(|m| lower.find(m))
        .min();
    if let Some(i) = cut {
        let head = clean(&fr_title[..i]);
        if !head.is_empty() {
            out.push(format!("{head}{y}"));
        }
    }
    out.dedup();
    out
}

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

/// Film français en attente de sa sortie en VOD : `(date de sortie en salle, date VOD estimée)`, ou `None`.
///
/// Sans date numérique, Radarr croit un film disponible 90 jours après la salle ; en France, la chronologie des
/// médias place la VOD **4 mois** après la salle, et C411 n'a rien avant (*Le Vertige*, 2026-09-25 : 0 release à
/// 107 jours). Seuls les films en langue originale française, sans date numérique ni physique connue, et sortis en
/// salle depuis moins de `min_days` jours sont concernés : les films étrangers ont presque toujours une date
/// numérique (américaine), et Radarr gère déjà ceux-là (`isAvailable`).
pub fn awaiting_vod(
    movie: &Value,
    now: chrono::DateTime<chrono::Utc>,
    min_days: i64,
) -> Option<(chrono::NaiveDate, chrono::NaiveDate)> {
    let french = movie
        .pointer("/originalLanguage/name")
        .and_then(Value::as_str)
        == Some("French");
    let dated = ["digitalRelease", "physicalRelease"].iter().any(|k| {
        movie
            .get(*k)
            .and_then(Value::as_str)
            .is_some_and(|d| !d.is_empty())
    });
    if !french || dated {
        return None;
    }
    let cinema = movie
        .get("inCinemas")
        .and_then(Value::as_str)
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())?
        .with_timezone(&chrono::Utc);
    let days = (now - cinema).num_days();
    if days < 0 || days >= min_days {
        return None;
    }
    let c = cinema.date_naive();
    Some((c, c.checked_add_months(chrono::Months::new(4))?))
}

/// Date à laquelle Radarr tient un film pour sorti (`minimumAvailability = released`) : la plus proche des dates
/// numérique et physique, sinon la salle + 90 jours ; `None` sans aucune date.
pub fn release_date(movie: &Value) -> Option<chrono::NaiveDate> {
    let day = |k: &str| {
        let d = movie.get(k).and_then(Value::as_str)?;
        chrono::NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok()
    };
    match (day("digitalRelease"), day("physicalRelease")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => day("inCinemas")?.checked_add_days(chrono::Days::new(90)),
    }
}

/// Délai avant de rechercher de nouveau un film resté sans release : `release_retry_hours` à moins de
/// `release_window_days` jours de sa date de sortie, `retry_after_hours` sinon.
pub fn retry_hours(
    cfg: &crate::config::MovieSearch,
    release: Option<chrono::NaiveDate>,
    today: chrono::NaiveDate,
) -> i64 {
    match release {
        Some(d) if (today - d).num_days().abs() <= cfg.release_window_days => {
            cfg.release_retry_hours
        }
        _ => cfg.retry_after_hours,
    }
}

/// Radarr ne tient un film pour sorti (`isAvailable`, RSS, `release/push`) qu'à sa date ; `availabilityDelay`
/// négatif avance ce moment. Réglé ici pour que `homelab.toml` reste la source de la valeur.
async fn ensure_availability_delay(ctx: &TaskContext, arr: &ArrClient, days: i64) -> Result<()> {
    let mut cfg = arr.get("api/v3/config/indexer", &[]).await?;
    let before = cfg.get("availabilityDelay").and_then(Value::as_i64);
    if before == Some(-days) {
        return Ok(());
    }
    if ctx.dry_run {
        info!(
            task = "movie_search",
            service = arr.name,
            ?before,
            after = -days,
            "dry-run: would set availabilityDelay"
        );
        return Ok(());
    }
    let id = cfg.get("id").and_then(Value::as_i64).unwrap_or(1);
    cfg["availabilityDelay"] = json!(-days);
    arr.put(&format!("api/v3/config/indexer/{id}"), &cfg)
        .await?;
    info!(
        task = "movie_search",
        service = arr.name,
        ?before,
        after = -days,
        "availabilityDelay réglé"
    );
    Ok(())
}

/// Résultat d'un passage qui attend une version française (voir `fresh_without_french`) : refait au bout de
/// `release_retry_hours`.
pub const VO_WAIT: &str = "vo_wait";

/// Release sans audio français (VOSTFR, VO) publiée il y a moins de `hours` heures : la VF suit souvent de peu
/// (*Insidious*, 2026-10-05 : VOSTFR 48 min avant la VFF) et le profil ne remplace jamais un fichier pris.
pub fn fresh_without_french(r: &Value, now_secs: i64, hours: i64) -> bool {
    let Some(title) = r.get("title").and_then(Value::as_str) else {
        return false;
    };
    lang_rank(title) < 2
        && r.get("publishDate")
            .and_then(Value::as_str)
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .is_some_and(|d| now_secs - d.timestamp() < hours * 3600)
}

/// Profil de délai par défaut de Radarr (sans étiquette) corrigé, ou `None` s'il est déjà bon : son RSS prendrait
/// sinon la première release venue, VOSTFR comprise. Une release torrent attend `vo_wait_hours`, sauf au-dessus de
/// `vo_wait_bypass_score` (MULTi, VFF…) ; « torrent » doit être le protocole préféré, sans quoi Radarr ignore le
/// passe-droit au score.
pub fn delay_profile_fix(profile: &Value, cfg: &crate::config::MovieSearch) -> Option<Value> {
    let want = [
        ("torrentDelay", json!(cfg.vo_wait_hours * 60)),
        ("preferredProtocol", json!("torrent")),
        ("bypassIfHighestQuality", json!(false)),
        ("bypassIfAboveCustomFormatScore", json!(true)),
        ("minimumCustomFormatScore", json!(cfg.vo_wait_bypass_score)),
    ];
    if want.iter().all(|(k, v)| profile.get(*k) == Some(v)) {
        return None;
    }
    let mut fixed = profile.clone();
    for (k, v) in want {
        fixed[k] = v;
    }
    Some(fixed)
}

/// Pose le profil de délai par défaut de Radarr (`delay_profile_fix`).
async fn ensure_delay_profile(
    ctx: &TaskContext,
    arr: &ArrClient,
    cfg: &crate::config::MovieSearch,
) -> Result<()> {
    let profiles = arr.get("api/v3/delayprofile", &[]).await?;
    let Some(default) = profiles.as_array().and_then(|a| {
        a.iter().find(|p| {
            p.get("tags")
                .and_then(Value::as_array)
                .is_some_and(|t| t.is_empty())
        })
    }) else {
        anyhow::bail!("profil de délai par défaut introuvable");
    };
    let Some(fixed) = delay_profile_fix(default, cfg) else {
        return Ok(());
    };
    if ctx.dry_run {
        info!(
            task = "movie_search",
            service = arr.name,
            "dry-run: would set the delay profile"
        );
        return Ok(());
    }
    let id = default.get("id").and_then(Value::as_i64).unwrap_or(1);
    arr.put(&format!("api/v3/delayprofile/{id}"), &fixed)
        .await?;
    info!(
        task = "movie_search",
        service = arr.name,
        torrent_delay_mins = cfg.vo_wait_hours * 60,
        bypass_score = cfg.vo_wait_bypass_score,
        "profil de délai réglé"
    );
    Ok(())
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
                // x265 > x264 > AV1 après la langue et le partage : voir series_search::codec_rank
                (
                    lang,
                    res,
                    seeders >= 2,
                    codec_rank(title),
                    audio_rank(title),
                    seeders,
                ),
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
    // C411 par identifiant ; s'il est en panne, secours public en texte libre (titre + année)
    let (found, source) = match crate::indexer::search_tmdb(ctx, prow, tmdb, None, false).await {
        Ok(Some(f)) => (f, cfg.indexer.clone()),
        Ok(None) => return Ok(("pending".into(), String::new())),
        Err(e) if crate::indexer::is_outage(&e) => {
            let year = movie.get("year").and_then(Value::as_i64).unwrap_or(0);
            // les trackers publics français cherchent par titre FRANÇAIS (TMDB via Jellyseerr)
            let fr = ctx
                .jellyseerr
                .movie_details(tmdb)
                .await
                .ok()
                .and_then(|d| d.get("title").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| title.to_string());
            let mut got = None;
            for q in fallback_queries(&fr, year) {
                match crate::indexer::search_fallback(ctx, prow, &q, "2000").await? {
                    Some((name, f)) => {
                        info!(task = "movie_search", movie = title, fallback = %name, query = %q, results = f.len(), "C411 en panne : recherche de secours");
                        let empty = f.is_empty();
                        got = Some((f, name));
                        if !empty {
                            break;
                        }
                    }
                    None => return Err(e),
                }
            }
            match got {
                Some(g) => g,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let fallback = source != cfg.indexer;
    let mut items = Vec::new();
    for mut r in found.into_iter().take(40) {
        if !fallback && !tmdb_matches(&r, tmdb) {
            continue;
        }
        let Some(t) = r.get("title").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let parse = arr.parse(&t).await?;
        if fallback {
            // pas d'identifiant TMDB chez un tracker public : c'est l'Arr qui dit si c'est bien CE film
            if parse.pointer("/movie/id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            r["tmdbId"] = json!(tmdb);
        }
        let info = parse.get("parsedMovieInfo").cloned().unwrap_or_default();
        items.push((r, info));
    }
    let profile_id = movie
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let allowed = allowed_qualities(&arr.quality_profile(profile_id).await?);
    let ix = &ctx.cfg.indexers;
    // une VOSTFR ou une VO toute fraîche attend `vo_wait_hours` qu'une version française la rattrape
    let t = now();
    let (fresh, ready): (Vec<_>, Vec<_>) = items
        .iter()
        .cloned()
        .partition(|(r, _)| fresh_without_french(r, t, cfg.vo_wait_hours));
    let best = best_movie_release(
        &ready,
        tmdb,
        &allowed,
        ix.max_gb_per_movie,
        ix.allow_no_french,
    );
    let best_french = best.as_ref().is_some_and(|(_, rel)| {
        rel.get("title")
            .and_then(Value::as_str)
            .is_some_and(|x| lang_rank(x) >= 2)
    });
    if !best_french
        && best_movie_release(&fresh, tmdb, &allowed, ix.max_gb_per_movie, true).is_some()
    {
        return Ok((
            VO_WAIT.into(),
            format!(
                "{} release(s) sans audio français de moins de {} h : on attend une version française",
                fresh.len(),
                cfg.vo_wait_hours
            ),
        ));
    }
    let Some((_, release)) = best else {
        // secours sans rien d'acceptable : refait dans `fallback_retry_hours`, ou dès que C411 répond
        if fallback {
            return Ok((
                "fallback_none".into(),
                format!(
                    "C411 en panne ; secours {source} : {} release(s), aucune acceptable",
                    items.len()
                ),
            ));
        }
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
        &source,
        Target::Movie { movie_id: id },
    )
    .await
}

#[async_trait]
impl Task for MovieSearch {
    fn name(&self) -> &'static str {
        "movie_search"
    }

    fn label(&self) -> &'static str {
        "Rattrapage des films (TMDB)"
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
        let c411_up = if records.values().any(|r| r.outcome == "fallback_none") {
            crate::indexer::c411_up(ctx, prow).await
        } else {
            true
        };
        let t = now();
        let mut todo: Vec<(&ArrClient, Value)> = Vec::new();
        let mut vod_wait = 0u32;
        let mut watched = 0u32;
        let now_dt = chrono::Utc::now();
        let today = now_dt.date_naive();
        for arr in radarrs {
            // avant la liste des films : leur `isAvailable` en dépend
            if let Err(e) = ensure_availability_delay(ctx, arr, cfg.release_window_days).await {
                warn!(
                    task = "movie_search",
                    service = arr.name,
                    error = format!("{e:#}"),
                    "availabilityDelay not checked"
                );
            }
            if let Err(e) = ensure_delay_profile(ctx, arr, cfg).await {
                warn!(
                    task = "movie_search",
                    service = arr.name,
                    error = format!("{e:#}"),
                    "delay profile not checked"
                );
            }
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
                        if !wanted(&m, &queued, t, cfg.missing_hours)
                            || super::anime_library::russian_route(&m, &ctx.cfg.tasks.anime_library)
                        {
                            continue;
                        }
                        if awaiting_vod(&m, now_dt, cfg.min_days_after_cinema).is_some() {
                            vod_wait += 1;
                            continue;
                        }
                        let rec = records.get(&format!("{}:{id}", arr.name));
                        let mut retry = retry_hours(cfg, release_date(&m), today);
                        if retry != cfg.retry_after_hours {
                            watched += 1;
                        }
                        if rec.is_some_and(|r| r.outcome == VO_WAIT) {
                            retry = cfg.release_retry_hours;
                        }
                        let go = super::series_search::fallback_due(
                            rec,
                            t,
                            ctx.cfg.indexers.fallback_retry_hours,
                            c411_up,
                        )
                        .unwrap_or_else(|| {
                            due(
                                rec,
                                t,
                                retry,
                                cfg.retry_after_hours,
                                cfg.error_retry_hours,
                                15,
                            )
                        });
                        if go {
                            todo.push((arr, m));
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        task = "movie_search",
                        service = arr.name,
                        error = format!("{e:#}"),
                        "planning failed"
                    )
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
                    warn!(
                        task = "movie_search",
                        service = arr.name,
                        movie_id = id,
                        error = format!("{e:#}"),
                        "movie failed"
                    );
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
        let mut summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        if vod_wait > 0 {
            summary.push(format!("{vod_wait} en attente de la VOD"));
        }
        if watched > 0 {
            summary.push(format!(
                "{watched} autour de leur sortie (toutes les {} h)",
                cfg.release_retry_hours
            ));
        }
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

    #[test]
    fn fallback_queries_use_the_french_title_without_punctuation() {
        assert_eq!(
            fallback_queries("Les Gardiens de la Galaxie Vol. 2", 2017),
            vec![
                "Les Gardiens de la Galaxie Vol 2 2017",
                "Les Gardiens de la Galaxie 2017"
            ]
        );
        assert_eq!(
            fallback_queries("Kaamelott : Premier Volet", 2021),
            vec!["Kaamelott Premier Volet 2021", "Kaamelott 2021"]
        );
        assert_eq!(fallback_queries("Matrix", 1999), vec!["Matrix 1999"]);
        assert_eq!(fallback_queries("L'Été dernier", 0), vec!["L'Été dernier"]);
    }
    use super::*;

    fn fr_movie(lang: &str, cinema: &str, digital: Option<&str>) -> Value {
        let mut m = json!({"originalLanguage": {"name": lang}, "inCinemas": cinema});
        if let Some(d) = digital {
            m["digitalRelease"] = Value::String(d.into());
        }
        m
    }

    #[test]
    fn french_films_wait_for_the_vod_window() {
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // Le Vertige : salle le 10/06, 107 jours plus tard, pas de date numérique
        let m = fr_movie("French", "2026-06-10T00:00:00Z", None);
        assert_eq!(
            awaiting_vod(&m, at("2026-09-25T10:00:00Z"), 110),
            Some((d("2026-06-10"), d("2026-10-10")))
        );
        assert_eq!(
            awaiting_vod(&m, at("2026-10-03T10:00:00Z"), 110),
            None,
            "115 jours : on cherche"
        );
        assert_eq!(
            awaiting_vod(
                &fr_movie("English", "2026-06-10T00:00:00Z", None),
                at("2026-09-25T10:00:00Z"),
                110
            ),
            None
        );
        assert_eq!(
            awaiting_vod(
                &fr_movie(
                    "French",
                    "2026-06-10T00:00:00Z",
                    Some("2026-10-12T00:00:00Z")
                ),
                at("2026-09-25T10:00:00Z"),
                110
            ),
            None,
            "date numérique connue : Radarr s'en charge"
        );
        assert_eq!(
            awaiting_vod(
                &json!({"originalLanguage": {"name": "French"}}),
                at("2026-09-25T10:00:00Z"),
                110
            ),
            None
        );
    }

    #[test]
    fn release_date_follows_radarr() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // Spider-Man: Brand New Day : numérique avant physique
        let m = json!({"inCinemas": "2026-07-29T00:00:00Z", "digitalRelease": "2026-10-06T00:00:00Z",
                       "physicalRelease": "2026-12-02T00:00:00Z"});
        assert_eq!(release_date(&m), Some(d("2026-10-06")));
        let m = json!({"physicalRelease": "2026-11-17T00:00:00Z"});
        assert_eq!(release_date(&m), Some(d("2026-11-17")));
        // sans date numérique ni physique : salle + 90 jours, comme Radarr
        let m = json!({"inCinemas": "2026-09-30T00:00:00Z"});
        assert_eq!(release_date(&m), Some(d("2026-12-29")));
        assert_eq!(release_date(&json!({"digitalRelease": ""})), None);
    }

    #[test]
    fn films_are_watched_around_their_release() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let cfg = crate::config::MovieSearch::default();
        let verity = Some(d("2026-10-27"));
        assert_eq!(
            retry_hours(&cfg, verity, d("2026-10-24")),
            72,
            "3 jours avant"
        );
        assert_eq!(retry_hours(&cfg, verity, d("2026-10-25")), 2, "48 h avant");
        assert_eq!(retry_hours(&cfg, verity, d("2026-10-27")), 2);
        assert_eq!(
            retry_hours(&cfg, verity, d("2026-10-29")),
            2,
            "2 jours après"
        );
        assert_eq!(retry_hours(&cfg, verity, d("2026-10-30")), 72);
        assert_eq!(retry_hours(&cfg, None, d("2026-10-27")), 72);
    }

    #[test]
    fn a_fresh_release_without_french_audio_waits() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T14:00:00Z")
            .unwrap()
            .timestamp();
        let r = |t: &str, at: &str| json!({"title": t, "publishDate": at});
        // Insidious : VOSTFR publiée à 13:12, VFF à 14:00
        let vost = r(
            "Insidious.L.Invasion.du.Lointain.2026.VOSTFR.1080p.WEB.EAC3.5.1.H264-K",
            "2026-10-05T13:12:00Z",
        );
        assert!(fresh_without_french(&vost, now, 3));
        assert!(
            !fresh_without_french(&vost, now + 3 * 3600, 3),
            "3 h plus tard : prise si rien d'autre"
        );
        let vo = r("Insidious.2026.1080p.WEB.H264-X", "2026-10-05T13:50:00Z");
        assert!(fresh_without_french(&vo, now, 3));
        let vff = r(
            "Insidious.Out.Of.The.Further.2026.MULTI.VFF.1080p.WEB.AC3.5.1.H265-Slay3R",
            "2026-10-05T14:00:00Z",
        );
        assert!(!fresh_without_french(&vff, now, 3));
        let french = r(
            "Film.2026.TRUEFRENCH.1080p.WEB.H264-X",
            "2026-10-05T13:59:00Z",
        );
        assert!(!fresh_without_french(&french, now, 3));
        // sans date de publication : jamais retenue
        assert!(!fresh_without_french(
            &json!({"title": "Film.2026.VOSTFR.1080p"}),
            now,
            3
        ));
    }

    #[test]
    fn the_radarr_delay_profile_is_fixed_once() {
        let cfg = crate::config::MovieSearch::default();
        let current = json!({"id": 1, "enableUsenet": true, "enableTorrent": true, "preferredProtocol": "usenet",
            "usenetDelay": 0, "torrentDelay": 0, "bypassIfHighestQuality": true,
            "bypassIfAboveCustomFormatScore": false, "minimumCustomFormatScore": 0, "order": 2147483647, "tags": []});
        let fixed = delay_profile_fix(&current, &cfg).expect("à corriger");
        assert_eq!(fixed["torrentDelay"], 180);
        assert_eq!(fixed["preferredProtocol"], "torrent");
        assert_eq!(fixed["bypassIfHighestQuality"], false);
        assert_eq!(fixed["bypassIfAboveCustomFormatScore"], true);
        assert_eq!(fixed["minimumCustomFormatScore"], 2500);
        assert_eq!(fixed["usenetDelay"], 0, "le reste ne change pas");
        assert_eq!(fixed["tags"], json!([]));
        assert_eq!(
            delay_profile_fix(&fixed, &cfg),
            None,
            "déjà bon : rien à écrire"
        );
    }

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
        // 2026-09-26 : à langue égale, le x265 passe devant le x264 même moins partagé, l'AV1 en dernier
        let mut items = items;
        items.push((
            r(
                "Souvenirs.De.Marnie.2014.MULTI.VFF.1080p.WEB.AV1",
                83389,
                60,
            ),
            q(7, 1080),
        ));
        items.push((
            r(
                "Souvenirs.De.Marnie.2014.MULTI.VFF.1080p.BluRay.x265",
                83389,
                4,
            ),
            q(7, 1080),
        ));
        let (_, rel) = best_movie_release(&items, 83389, &allowed, 25.0, true).unwrap();
        assert_eq!(
            rel["title"],
            "Souvenirs.De.Marnie.2014.MULTI.VFF.1080p.BluRay.x265"
        );
    }
}
