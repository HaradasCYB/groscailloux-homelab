//! Prend les releases qu'un Sonarr rejette uniquement pour « Unknown Series » : titres traduits
//! (« New York Police Judiciaire » pour *Law & Order*) que le parseur ne relie pas à la fiche.
//!
//! Pour quelques saisons suivies avec des épisodes manquants (les plus récentes d'abord) :
//! recherche interactive, releases de l'indexer configuré (C411) dont le seul rejet est
//! « Unknown Series », puis garde-fous : titre de la release **identique** (après normalisation)
//! au titre français, d'origine, Sonarr ou alternatif de la série — une série voisine
//! (« New York Unité Spéciale ») ne passe pas —, bonne saison, épisodes manquants, qualité
//! autorisée par le profil et ≤ 1080p, marqueur de langue française (VOSTFR en dernier). Le
//! meilleur candidat est envoyé par grab forcé (série et épisodes fournis) ; `id_match_import`
//! débloquera l'import. Une saison sans candidat n'est recherchée qu'après `retry_after_hours`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;
use crate::matching::{normalize, parsed_series};
use crate::state::{now, SeasonSearchRecord};

pub struct UnknownSeriesGrab;

/// Une release candidate, réduite à ce qui sert au choix.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub release: Value,
    pub title: String,
    pub season: i64,
    pub episodes: Vec<i64>,
    pub full_season: bool,
    pub lang_rank: u8,
    pub resolution: i64,
    pub h264: bool,
    pub seeders: i64,
}

/// Rang de langue d'après le titre : VF > MULTi > FRENCH > VOSTFR ; `None` = pas de français.
pub fn lang_rank(title: &str) -> Option<u8> {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(&x.as_str()));
    if has(&["VFF", "TRUEFRENCH", "VFQ", "VFI", "VF2", "VFB"]) {
        Some(4)
    } else if has(&["MULTI"]) {
        Some(3)
    } else if has(&["FRENCH"]) {
        Some(2)
    } else if has(&["VOSTFR", "SUBFRENCH"]) {
        Some(1)
    } else {
        None
    }
}

/// Le titre de série extrait de la release correspond-il exactement à l'un des noms connus ?
pub fn title_matches(release_series_title: &str, names: &[String]) -> bool {
    let t = normalize(release_series_title);
    !t.is_empty() && names.iter().any(|n| normalize(n) == t)
}

fn is_h264(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    ["x264", "h264", "h.264", "avc"]
        .iter()
        .any(|k| t.contains(k))
}

/// Construit un candidat depuis une release de l'indexer voulu, rejetée uniquement « Unknown Series ».
pub fn candidate(release: &Value, indexer: &str) -> Option<Candidate> {
    let ix = release.get("indexer").and_then(Value::as_str).unwrap_or("");
    if !ix
        .to_ascii_lowercase()
        .starts_with(&indexer.to_ascii_lowercase())
    {
        return None;
    }
    let rej: Vec<&str> = release
        .get("rejections")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if rej != ["Unknown Series"] {
        return None;
    }
    let title = release.get("title").and_then(Value::as_str)?.to_string();
    Some(Candidate {
        lang_rank: lang_rank(&title)?,
        season: release.get("seasonNumber").and_then(Value::as_i64)?,
        episodes: release
            .get("episodeNumbers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        full_season: release
            .get("fullSeason")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        resolution: release
            .pointer("/quality/quality/resolution")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        h264: is_h264(&title),
        seeders: release.get("seeders").and_then(Value::as_i64).unwrap_or(0),
        release: release.clone(),
        title,
    })
}

/// Ids de qualité autorisés par un profil Sonarr (groupes compris).
pub fn allowed_qualities(profile: &Value) -> HashSet<i64> {
    let mut out = HashSet::new();
    for it in profile
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let allowed = it.get("allowed").and_then(Value::as_bool).unwrap_or(false);
        if let Some(id) = it.pointer("/quality/id").and_then(Value::as_i64) {
            if allowed {
                out.insert(id);
            }
        }
        for sub in it
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if allowed {
                if let Some(id) = sub.pointer("/quality/id").and_then(Value::as_i64) {
                    out.insert(id);
                }
            }
        }
    }
    out
}

/// Meilleur candidat pour une saison : pack si `want_pack`, sinon épisodes tous manquants.
pub fn choose<'a>(
    cands: &'a [Candidate],
    season: i64,
    want_pack: bool,
    missing: &HashSet<i64>,
    allowed: &HashSet<i64>,
) -> Option<&'a Candidate> {
    let ok = |c: &&Candidate| {
        c.season == season
            && c.seeders > 0
            && c.resolution <= 1080
            && c.release
                .pointer("/quality/quality/id")
                .and_then(Value::as_i64)
                .map(|q| allowed.is_empty() || allowed.contains(&q))
                .unwrap_or(false)
            && if want_pack {
                c.full_season
            } else {
                !c.full_season
                    && !c.episodes.is_empty()
                    && c.episodes.iter().all(|e| missing.contains(e))
            }
    };
    cands
        .iter()
        .filter(ok)
        .max_by_key(|c| (c.lang_rank, c.resolution, c.h264, c.seeders))
}

/// Faut-il (re)chercher cette saison ?
pub fn due(
    rec: Option<&SeasonSearchRecord>,
    now: i64,
    retry_h: i64,
    grabbed_h: i64,
    error_h: i64,
) -> bool {
    match rec {
        None => true,
        Some(r) => {
            // une erreur (indexer indisponible, recherche trop longue) n'est pas une absence de candidat :
            // on réessaie vite, sinon la saison reste bloquée trois jours pour un incident passager
            let wait = match r.outcome.as_str() {
                "grabbed" => grabbed_h,
                "error" => error_h,
                _ => retry_h,
            };
            now - r.at >= wait * 3600
        }
    }
}

struct SeasonTodo {
    series_id: i64,
    season: i64,
    latest_air: String,
    missing_numbers: HashSet<i64>,
}

async fn names_for(ctx: &TaskContext, series: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(t) = series.get("title").and_then(Value::as_str) {
        names.push(t.to_string());
    }
    for a in series
        .get("alternateTitles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(t) = a.get("title").and_then(Value::as_str) {
            names.push(t.to_string());
        }
    }
    if let Some(tmdb) = series
        .get("tmdbId")
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
    {
        match ctx.jellyseerr.tv_details(tmdb).await {
            Ok(tv) => {
                for k in ["name", "originalName"] {
                    if let Some(t) = tv.get(k).and_then(Value::as_str) {
                        names.push(t.to_string());
                    }
                }
            }
            Err(e) => {
                warn!(task = "unknown_series_grab", tmdb, error = %e, "jellyseerr title lookup failed")
            }
        }
    }
    names
}

/// Recherche de secours : Sonarr n'interroge l'indexer qu'avec ses propres titres, donc une série dont les
/// releases portent un titre traduit (« L'attaque des Titans » pour *Attack on Titan*) ne remonte jamais.
/// On interroge alors l'indexer **en texte libre** par Prowlarr, avec les noms connus de la série, et on
/// pousse la release retenue à Sonarr, qui la rattache lui-même (son parseur, lui, reconnaît le titre).
async fn prowlarr_candidates(
    ctx: &TaskContext,
    arr: &ArrClient,
    names: &[String],
    todo: &SeasonTodo,
) -> Result<Vec<Candidate>> {
    let Some(prow) = &ctx.prowlarr else {
        return Ok(Vec::new());
    };
    let cfg = &ctx.cfg.tasks.unknown_series_grab;
    let Some(indexer_id) = prow.indexer_id(&cfg.indexer).await? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for name in names.iter().take(cfg.prowlarr_queries) {
        for r in prow.search(name, indexer_id, 100).await? {
            let (Some(title), Some(url)) = (
                r.get("title").and_then(Value::as_str),
                r.get("downloadUrl").and_then(Value::as_str),
            ) else {
                continue;
            };
            if !seen.insert(title.to_string()) {
                continue;
            }
            let Some(lang) = lang_rank(title) else {
                continue;
            };
            let parse = arr.parse(title).await?;
            let Some(p) = parsed_series(&parse) else {
                continue;
            };
            if !title_matches(&p.title, names) {
                continue;
            }
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if info.get("seasonNumber").and_then(Value::as_i64) != Some(todo.season) {
                continue;
            }
            out.push(Candidate {
                title: title.to_string(),
                season: todo.season,
                episodes: info
                    .get("episodeNumbers")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default(),
                full_season: info
                    .get("fullSeason")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                lang_rank: lang,
                resolution: info
                    .pointer("/quality/quality/resolution")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                h264: !title.to_ascii_uppercase().contains("265")
                    && !title.to_ascii_uppercase().contains("HEVC"),
                seeders: r.get("seeders").and_then(Value::as_i64).unwrap_or(0),
                release: serde_json::json!({
                    "source": "prowlarr",
                    "title": title,
                    "downloadUrl": url,
                    "publishDate": r.get("publishDate"),
                    "quality": info.get("quality"),
                }),
            });
        }
    }
    Ok(out)
}

async fn process_season(
    ctx: &TaskContext,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    download_client: i64,
) -> Result<(String, String)> {
    let cfg = &ctx.cfg.tasks.unknown_series_grab;
    let title = series.get("title").and_then(Value::as_str).unwrap_or("?");
    // Anime : Sonarr interroge l'indexer épisode par épisode. Une recherche de saison entière dépasse le
    // délai du proxy de la seedbox (504 au bout de 300 s, saisons 2 et 3 d'Attack on Titan jamais cherchées
    // le 2026-09-16) : on avance par petits paquets d'épisodes, les résultats sont les mêmes (packs compris).
    let anime = series.get("seriesType").and_then(Value::as_str) == Some("anime");
    let releases = if anime {
        let eps = arr.episodes(todo.series_id).await?;
        let mut ids: Vec<i64> = eps
            .iter()
            .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(todo.season))
            .filter(|e| {
                e.get("episodeNumber")
                    .and_then(Value::as_i64)
                    .is_some_and(|n| todo.missing_numbers.contains(&n))
            })
            .filter_map(|e| e.get("id").and_then(Value::as_i64))
            .collect();
        ids.sort_unstable();
        ids.truncate(cfg.anime_episodes_per_run);
        let mut out: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for id in ids {
            for r in arr.releases_for_episode(id).await? {
                let guid = r
                    .get("guid")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if guid.is_empty() || seen.insert(guid) {
                    out.push(r);
                }
            }
        }
        out
    } else {
        arr.releases(todo.series_id, todo.season).await?
    };
    let names = names_for(ctx, series).await;
    let mut cands = Vec::new();
    for r in &releases {
        let Some(c) = candidate(r, &cfg.indexer) else {
            continue;
        };
        let parse = arr.parse(&c.title).await?;
        match parsed_series(&parse) {
            Some(p) if title_matches(&p.title, &names) => cands.push(c),
            Some(p) => {
                info!(task = "unknown_series_grab", series = title, release = %c.title, parsed = %p.title, "title does not match the series, ignored")
            }
            None => {}
        }
    }
    if cands.is_empty() {
        match prowlarr_candidates(ctx, arr, &names, todo).await {
            Ok(extra) if !extra.is_empty() => {
                info!(
                    task = "unknown_series_grab",
                    series = title,
                    season = todo.season,
                    candidates = extra.len(),
                    "candidats trouvés par titre traduit (Prowlarr)"
                );
                cands = extra;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(task = "unknown_series_grab", series = title, error = %e, "recherche Prowlarr en échec")
            }
        }
    }
    let episodes = arr.episodes(todo.series_id).await?;
    let season_eps: Vec<&Value> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(todo.season))
        .collect();
    let want_pack = todo.missing_numbers.len() * 2 >= season_eps.len().max(1);
    let profile_id = series
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let allowed = allowed_qualities(&arr.quality_profile(profile_id).await?);
    let chosen = choose(
        &cands,
        todo.season,
        want_pack,
        &todo.missing_numbers,
        &allowed,
    )
    .or_else(|| {
        // pas de pack : épisodes seuls ; pas d'épisode : pack
        choose(
            &cands,
            todo.season,
            !want_pack,
            &todo.missing_numbers,
            &allowed,
        )
    });
    let Some(c) = chosen else {
        return Ok((
            "none".into(),
            format!(
                "{} candidat(s) C411 « Unknown Series » retenu(s), aucun acceptable",
                cands.len()
            ),
        ));
    };
    let ids: Vec<i64> = season_eps
        .iter()
        .filter(|e| {
            c.full_season
                || e.get("episodeNumber")
                    .and_then(Value::as_i64)
                    .map(|n| c.episodes.contains(&n))
                    .unwrap_or(false)
        })
        .filter_map(|e| e.get("id").and_then(Value::as_i64))
        .collect();
    if ids.is_empty() {
        return Ok((
            "none".into(),
            format!("{} : épisodes introuvables", c.title),
        ));
    }
    if ctx.dry_run {
        info!(task = "unknown_series_grab", service = arr.name, series = title, season = todo.season, release = %c.title, episodes = ids.len(), "dry-run: would grab");
        return Ok(("dry_run".into(), c.title.clone()));
    }
    // release trouvée par Prowlarr : Sonarr ne l'a pas en mémoire, on la lui pousse (il la rattache seul).
    if c.release.get("source").and_then(Value::as_str) == Some("prowlarr") {
        let url = c
            .release
            .get("downloadUrl")
            .and_then(Value::as_str)
            .context("release sans lien de téléchargement")?;
        let decision = arr
            .push_release(
                &c.title,
                url,
                c.release.get("publishDate").and_then(Value::as_str),
                &cfg.indexer,
            )
            .await
            .with_context(|| format!("push {}", c.title))?;
        let rejected: Vec<String> = decision
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(std::slice::from_ref(&decision))
            .iter()
            .filter(|d| d.get("approved").and_then(Value::as_bool) != Some(true))
            .flat_map(|d| {
                d.get("rejections")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|r| r.as_str().map(str::to_string))
            .collect();
        if !rejected.is_empty() {
            return Ok((
                "none".into(),
                format!("{} refusé : {}", c.title, rejected.join(" ; ")),
            ));
        }
        info!(task = "unknown_series_grab", service = arr.name, series = title, season = todo.season, release = %c.title, "poussé à Sonarr (titre traduit)");
        return Ok(("grabbed".into(), c.title.clone()));
    }
    arr.grab_override(&c.release, todo.series_id, &ids, download_client)
        .await
        .with_context(|| format!("grab {}", c.title))?;
    info!(task = "unknown_series_grab", service = arr.name, series = title, season = todo.season, release = %c.title, episodes = ids.len(), "grabbed");
    Ok(("grabbed".into(), c.title.clone()))
}

async fn plan_seasons(ctx: &TaskContext, arr: &ArrClient) -> Result<Vec<SeasonTodo>> {
    let cfg = &ctx.cfg.tasks.unknown_series_grab;
    let now_iso = chrono::Utc::now().to_rfc3339();
    let mut by: BTreeMap<(i64, i64), SeasonTodo> = BTreeMap::new();
    for e in arr.wanted_missing().await? {
        let (Some(sid), Some(season), Some(num)) = (
            e.get("seriesId").and_then(Value::as_i64),
            e.get("seasonNumber").and_then(Value::as_i64),
            e.get("episodeNumber").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let air = e
            .get("airDateUtc")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if season == 0 || air.is_empty() || air > now_iso {
            continue;
        }
        let t = by.entry((sid, season)).or_insert(SeasonTodo {
            series_id: sid,
            season,
            latest_air: String::new(),
            missing_numbers: HashSet::new(),
        });
        t.missing_numbers.insert(num);
        if air > t.latest_air {
            t.latest_air = air;
        }
    }
    let queued: HashSet<(i64, i64)> = arr
        .queue_records()
        .await?
        .iter()
        .filter_map(|r| {
            Some((
                r.get("seriesId").and_then(Value::as_i64)?,
                r.get("seasonNumber")
                    .and_then(Value::as_i64)
                    .or_else(|| r.pointer("/episode/seasonNumber").and_then(Value::as_i64))?,
            ))
        })
        .collect();
    let records = ctx.state.read(|s| s.unknown_series.clone()).await;
    let t = now();
    let mut todo: Vec<SeasonTodo> = by
        .into_values()
        .filter(|s| !queued.contains(&(s.series_id, s.season)))
        .filter(|s| {
            due(
                records.get(&key(arr, s.series_id, s.season)),
                t,
                cfg.retry_after_hours,
                cfg.grabbed_retry_hours,
                cfg.error_retry_hours,
            )
        })
        .collect();
    todo.sort_by(|a, b| b.latest_air.cmp(&a.latest_air));
    Ok(todo)
}

fn key(arr: &ArrClient, series: i64, season: i64) -> String {
    format!("{}:{series}:{season}", arr.name)
}

#[async_trait]
impl Task for UnknownSeriesGrab {
    fn name(&self) -> &'static str {
        "unknown_series_grab"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.unknown_series_grab.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let budget = ctx.cfg.tasks.unknown_series_grab.max_searches_per_run;
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        // saisons de tous les Sonarr, puis un seul ordre global : nouvelles demandes d'abord
        struct Ctx<'a> {
            arr: &'a ArrClient,
            series: HashMap<i64, Value>,
            dc: i64,
        }
        let mut arrs: Vec<Ctx> = Vec::new();
        let mut all: Vec<(usize, SeasonTodo)> = Vec::new();
        for arr in ctx.all_sonarr() {
            let prepared = async {
                let todo = plan_seasons(ctx, arr).await?;
                let series: HashMap<i64, Value> = arr
                    .series()
                    .await?
                    .into_iter()
                    .filter_map(|s| Some((s.get("id").and_then(Value::as_i64)?, s)))
                    .collect();
                let dc = arr
                    .download_clients()
                    .await?
                    .into_iter()
                    .find(|d| d.get("enable").and_then(Value::as_bool).unwrap_or(false))
                    .and_then(|d| d.get("id").and_then(Value::as_i64))
                    .context("aucun client de téléchargement actif")?;
                anyhow::Ok((todo, series, dc))
            }
            .await;
            match prepared {
                Ok((todo, series, dc)) => {
                    let i = arrs.len();
                    all.extend(todo.into_iter().map(|t| (i, t)));
                    arrs.push(Ctx { arr, series, dc });
                }
                Err(e) => {
                    warn!(task = "unknown_series_grab", service = arr.name, error = %e, "planning failed")
                }
            }
        }
        // saisons déjà présentes sur l'autre machine : jamais prises ici (doublon Jellyfin)
        let files: Vec<BTreeMap<i64, std::collections::BTreeSet<i64>>> = arrs
            .iter()
            .map(|c| {
                let list: Vec<Value> = c.series.values().cloned().collect();
                super::monitor_sync::seasons_with_files(&list)
            })
            .collect();
        let before = all.len();
        all.retain(|(i, t)| {
            let tvdb = arrs[*i]
                .series
                .get(&t.series_id)
                .and_then(|s| s.get("tvdbId").and_then(Value::as_i64))
                .unwrap_or(0);
            !files.iter().enumerate().any(|(j, f)| {
                j != *i && f.get(&tvdb).map(|s| s.contains(&t.season)).unwrap_or(false)
            })
        });
        if before > all.len() {
            counts.insert("dup_other_side".into(), (before - all.len()) as u32);
        }
        let entries: Vec<(String, String, bool)> = all
            .iter()
            .map(|(i, t)| {
                let ser = arrs[*i].series.get(&t.series_id);
                let added = ser
                    .and_then(|s| s.get("added").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                let anime =
                    ser.and_then(|s| s.get("seriesType").and_then(Value::as_str)) == Some("anime");
                (added, t.latest_air.clone(), anime)
            })
            .collect();
        let chosen = pick_order(&entries, budget);
        counts.insert("pending".into(), (all.len() - chosen.len()) as u32);
        // le planificateur coupe une tâche après 10 min ; une recherche de saison dure ~1 min
        let deadline = std::time::Instant::now() + Duration::from_secs(480);
        for idx in chosen {
            if std::time::Instant::now() > deadline {
                *counts.entry("pending".into()).or_default() += 1;
                continue;
            }
            let (i, s) = &all[idx];
            let c = &arrs[*i];
            let Some(ser) = c.series.get(&s.series_id) else {
                continue;
            };
            let (outcome, detail) = match process_season(ctx, c.arr, ser, s, c.dc).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(task = "unknown_series_grab", service = c.arr.name, series_id = s.series_id, season = s.season, error = %e, "season failed");
                    ("error".into(), format!("{e:#}").chars().take(200).collect())
                }
            };
            let sname = ser.get("title").and_then(Value::as_str).unwrap_or("?");
            info!(task = "unknown_series_grab", service = c.arr.name, series = sname, season = s.season, %outcome, %detail, "season searched");
            *counts.entry(outcome.clone()).or_default() += 1;
            if !ctx.dry_run {
                let rec = SeasonSearchRecord {
                    at: now(),
                    outcome,
                    detail,
                };
                let k = key(c.arr, s.series_id, s.season);
                ctx.state
                    .update(|st| st.unknown_series.insert(k, rec))
                    .await?;
            }
        }
        let grabbed = counts.get("grabbed").copied().unwrap_or(0);
        let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Ok(Report::new(summary.join(" "), grabbed))
    }
}

/// Ordre de traitement : séries ajoutées le plus récemment d'abord (une nouvelle demande passe
/// devant l'arriéré), puis épisodes diffusés le plus récemment ; au plus une saison d'anime par
/// passage (recherche épisode par épisode, coûteuse pour la limite d'API de C411).
/// `entries` : (date d'ajout de la série, dernière diffusion manquante, anime) ; renvoie des indices.
pub fn pick_order(entries: &[(String, String, bool)], budget: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..entries.len()).collect();
    idx.sort_by(|a, b| {
        let (ea, eb) = (&entries[*a], &entries[*b]);
        eb.0.cmp(&ea.0).then_with(|| eb.1.cmp(&ea.1))
    });
    let mut out = Vec::new();
    let mut anime = false;
    for i in idx {
        if out.len() >= budget {
            break;
        }
        if entries[i].2 {
            if anime {
                continue;
            }
            anime = true;
        }
        out.push(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[allow(clippy::too_many_arguments)]
    fn rel(
        title: &str,
        season: i64,
        full: bool,
        eps: &[i64],
        res: i64,
        qid: i64,
        seeders: i64,
        rej: &[&str],
    ) -> Value {
        json!({"title": title, "indexer": "C411", "seasonNumber": season, "fullSeason": full, "episodeNumbers": eps,
               "quality": {"quality": {"id": qid, "resolution": res}}, "seeders": seeders, "rejections": rej,
               "guid": title, "indexerId": 2})
    }

    #[test]
    fn language_ranks() {
        assert_eq!(
            lang_rank("New.York.Police.Judiciaire.S14.VFF.720p.HDTV"),
            Some(4)
        );
        assert_eq!(lang_rank("Show.S01.MULTi.1080p"), Some(3));
        assert_eq!(lang_rank("Show.S01.MULTi.VFF.1080p"), Some(4));
        assert_eq!(lang_rank("Show.S01.FRENCH.720p"), Some(2));
        assert_eq!(lang_rank("Show.S01E02.VOSTFR.1080p"), Some(1));
        assert_eq!(lang_rank("Law.And.Order.S14E07.720p.WEB"), None);
    }

    #[test]
    fn titles_must_match_a_known_name_exactly() {
        let names = vec![
            "Law & Order".to_string(),
            "New York Police Judiciaire".to_string(),
            "law and order".to_string(),
        ];
        assert!(title_matches("New York Police Judiciaire", &names));
        assert!(!title_matches("New York Unite Speciale", &names));
        assert!(!title_matches(
            "Law and Order Toronto Criminal Intent",
            &names
        ));
        assert!(title_matches("Law and Order", &names));
        assert!(!title_matches("", &names));
    }

    #[test]
    fn only_single_unknown_series_rejection_from_the_indexer() {
        assert!(candidate(
            &rel(
                "X.S14.VFF.720p",
                14,
                true,
                &[],
                720,
                4,
                5,
                &["Unknown Series"]
            ),
            "C411"
        )
        .is_some());
        assert!(candidate(
            &rel(
                "X.S14.VFF.720p",
                14,
                true,
                &[],
                720,
                4,
                5,
                &["Unknown Series", "Sample"]
            ),
            "C411"
        )
        .is_none());
        assert!(candidate(
            &rel("X.S14.VFF.720p", 14, true, &[], 720, 4, 5, &[]),
            "C411"
        )
        .is_none());
        let mut other = rel(
            "X.S14.VFF.720p",
            14,
            true,
            &[],
            720,
            4,
            5,
            &["Unknown Series"],
        );
        other["indexer"] = json!("1337x");
        assert!(candidate(&other, "C411").is_none());
        assert!(candidate(
            &rel(
                "X.S14.720p.WEB",
                14,
                true,
                &[],
                720,
                4,
                5,
                &["Unknown Series"]
            ),
            "C411"
        )
        .is_none());
    }

    #[test]
    fn chooses_best_pack_within_limits() {
        let u = ["Unknown Series"];
        let rels = [
            rel("A.S14.VFF.720p.HDTV.x264", 14, true, &[], 720, 4, 11, &u),
            rel("A.S14.MULTi.1080p.WEB.x265", 14, true, &[], 1080, 9, 30, &u),
            rel("A.S14.VFF.1080p.WEB.x264", 14, true, &[], 1080, 9, 3, &u),
            rel("A.S14.VFF.2160p.WEB", 14, true, &[], 2160, 18, 50, &u),
            rel("A.S14.VFF.1080p.dead", 14, true, &[], 1080, 9, 0, &u),
            rel("A.S15.VFF.1080p", 15, true, &[], 1080, 9, 9, &u),
        ];
        let cands: Vec<Candidate> = rels.iter().filter_map(|r| candidate(r, "C411")).collect();
        let allowed: HashSet<i64> = [4, 9].into_iter().collect();
        let missing: HashSet<i64> = (1..=24).collect();
        let c = choose(&cands, 14, true, &missing, &allowed).unwrap();
        assert_eq!(c.title, "A.S14.VFF.1080p.WEB.x264");
        let only720: HashSet<i64> = [4].into_iter().collect();
        assert_eq!(
            choose(&cands, 14, true, &missing, &only720).unwrap().title,
            "A.S14.VFF.720p.HDTV.x264"
        );
    }

    #[test]
    fn single_episodes_must_be_missing() {
        let u = ["Unknown Series"];
        let rels = [
            rel("A.S02E03.VFF.1080p", 2, false, &[3], 1080, 9, 4, &u),
            rel("A.S02E04.VFF.1080p", 2, false, &[4], 1080, 9, 4, &u),
        ];
        let cands: Vec<Candidate> = rels.iter().filter_map(|r| candidate(r, "C411")).collect();
        let missing: HashSet<i64> = [4].into_iter().collect();
        assert_eq!(
            choose(&cands, 2, false, &missing, &HashSet::new())
                .unwrap()
                .title,
            "A.S02E04.VFF.1080p"
        );
    }

    #[test]
    fn new_requests_first_and_one_anime_per_run() {
        let e = |added: &str, air: &str, anime: bool| (added.to_string(), air.to_string(), anime);
        let entries = vec![
            e("2026-05-01", "2026-09-10", false), // arriéré récent
            e("2026-09-13", "2008-10-01", false), // Mentalist S1, demandé aujourd'hui
            e("2026-09-13", "2015-02-01", false), // Mentalist S7
            e("2026-09-12", "2026-09-01", true),  // anime A
            e("2026-09-12", "2026-08-01", true),  // anime A, autre saison
        ];
        assert_eq!(pick_order(&entries, 3), vec![2, 1, 3]);
        assert_eq!(pick_order(&entries, 10), vec![2, 1, 3, 0]);
        assert!(pick_order(&entries, 0).is_empty());
    }

    #[test]
    fn retry_windows() {
        let r = |outcome: &str, at: i64| SeasonSearchRecord {
            at,
            outcome: outcome.into(),
            detail: String::new(),
        };
        assert!(due(None, 1000, 72, 168, 1));
        assert!(!due(Some(&r("none", 0)), 71 * 3600, 72, 168, 1));
        assert!(due(Some(&r("none", 0)), 72 * 3600, 72, 168, 1));
        assert!(!due(Some(&r("grabbed", 0)), 100 * 3600, 72, 168, 1));
        assert!(due(Some(&r("grabbed", 0)), 168 * 3600, 72, 168, 1));
        // une erreur ne bloque pas la saison trois jours : nouvelle tentative dans l'heure
        assert!(!due(Some(&r("error", 0)), 1800, 72, 168, 1));
        assert!(due(Some(&r("error", 0)), 3600, 72, 168, 1));
    }

    #[test]
    fn allowed_qualities_include_groups() {
        let p = json!({"items": [
            {"allowed": true, "quality": {"id": 4}},
            {"allowed": false, "quality": {"id": 18}},
            {"allowed": true, "name": "WEB 1080p", "items": [{"quality": {"id": 3}}, {"quality": {"id": 15}}]}
        ]});
        let a = allowed_qualities(&p);
        assert!(a.contains(&4) && a.contains(&3) && a.contains(&15) && !a.contains(&18));
    }
}
