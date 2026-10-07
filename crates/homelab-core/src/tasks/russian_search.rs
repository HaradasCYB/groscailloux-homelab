//! Voie russe : recherche RuTracker par **titre original** (2026-09-27).
//!
//! Sonarr et Radarr cherchent par leur titre (anglais) : RuTracker range les œuvres sous le titre russe et ne répond
//! rien (« The Interns » → 0, « Интерны » → 18 releases). Pour chaque fiche de la voie russe (dossier russe de la
//! seedbox, voir `anime_library::russian_route`) à qui il manque des fichiers, cette tâche :
//! 1. lit le titre original TMDB (Jellyseerr) et interroge RuTracker par le Jackett de la seedbox ;
//! 2. film : release de la bonne année, ≤ 1080p ; série : release dont la plage d'épisodes (`E1-120`,
//!    `S4E61-268 of 278`, numérotation **absolue** des séries russes) couvre le plus d'épisodes manquants ;
//! 3. ajoute le `.torrent` au qBittorrent de la seedbox (étiquette `homelab:russe`) et, pour une série, ne garde que
//!    les fichiers des épisodes manquants (numéro lu dans le nom : `Интерны_060.avi`) ;
//! 4. une fois complet, l'importe (`ManualImport` copy = lien physique) : épisode par `SxxEyy` si l'Arr le lit, sinon
//!    par numéro absolu ; puis demande l'analyse complète de Jellyfin.
//!
//! Au plus `max_per_run` fiches par passage, une recherche par fiche toutes les `retry_hours`. Rien sur C411.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::anime_library::{request_library_scan, russian_route};
use super::series_search::{audio_rank, codec_rank};
use super::{Report, Task};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, RussianTitleRecord};

pub struct RussianSearch;

pub const TAG: &str = "homelab:russe";

/// Une release Torznab.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub title: String,
    pub link: String,
    pub size: i64,
    pub seeders: i64,
    pub infohash: Option<String>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Releases d'une réponse Torznab (Jackett).
pub fn parse_items(xml: &str) -> Vec<Release> {
    let item = Regex::new(r"(?s)<item>(.*?)</item>").expect("regex");
    let tag = |name: &str| Regex::new(&format!(r"(?s)<{name}>(.*?)</{name}>")).expect("regex");
    let (t, l, z) = (tag("title"), tag("link"), tag("size"));
    let attr =
        |name: &str| Regex::new(&format!(r#"name="{name}"\s+value="([^"]*)""#)).expect("regex");
    let (seeders, hash) = (attr("seeders"), attr("infohash"));
    item.captures_iter(xml)
        .filter_map(|c| {
            let body = c.get(1)?.as_str();
            Some(Release {
                title: unescape(t.captures(body)?.get(1)?.as_str().trim()),
                link: unescape(l.captures(body)?.get(1)?.as_str().trim()),
                size: z
                    .captures(body)
                    .and_then(|m| m[1].trim().parse().ok())
                    .unwrap_or(0),
                seeders: seeders
                    .captures(body)
                    .and_then(|m| m[1].parse().ok())
                    .unwrap_or(0),
                infohash: hash.captures(body).map(|m| m[1].to_ascii_lowercase()),
            })
        })
        .collect()
}

/// Résolution annoncée (0 = inconnue, souvent un DVDRip/SATRip en définition standard).
pub fn resolution(title: &str) -> i64 {
    let t = title.to_ascii_lowercase();
    if t.contains("2160p") || t.contains("4k") || t.contains("uhd") {
        2160
    } else if t.contains("1080") {
        1080
    } else if t.contains("720p") {
        720
    } else {
        0
    }
}

/// Plage d'épisodes annoncée par RuTracker, numérotation absolue : `E1-120 of 120`, `S4E61-268 of 278`,
/// `S6E1-20 20 RUS (101-120)` (le numéro absolu est alors entre parenthèses).
pub fn episode_range(title: &str) -> Option<(u32, u32)> {
    let paren = Regex::new(r"\((\d{1,4})-(\d{1,4})\)").expect("regex");
    let se = Regex::new(r"(?i)E(\d{1,4})-(\d{1,4})").expect("regex");
    let single = Regex::new(r"(?i)(?:^|[^A-Z])E(\d{1,4})(?:\D|$)").expect("regex");
    // « S6E1-20 … (101-120) » : la saison russe renumérote, les parenthèses donnent l'absolu
    if let (Some(_), Some(p)) = (se.captures(title), paren.captures(title)) {
        let (a, b) = (p[1].parse().ok()?, p[2].parse().ok()?);
        if a <= b {
            return Some((a, b));
        }
    }
    if let Some(c) = se.captures(title) {
        let (a, b): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        return (a <= b).then_some((a, b));
    }
    single
        .captures(title)
        .and_then(|c| c[1].parse().ok())
        .map(|n| (n, n))
}

/// Épisode lu dans un nom de fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEp {
    /// `S02E02` : saison et épisode.
    Se(u32, u32),
    /// Numéro absolu (`Интерны_060.avi`, `Kukhnya - 12.mkv`).
    Abs(u32),
}

/// Épisode d'un fichier : `SxxEyy` s'il y en a un, sinon le dernier nombre de 1 à 4 chiffres du nom, en ignorant
/// résolution (`1080p`), codec (`x264`, `H.265`, `10bit`), audio (`5.1`) et années.
pub fn file_episode(name: &str) -> Option<FileEp> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let se = Regex::new(r"(?i)S(\d{1,2})[ ._-]?E(\d{1,4})").expect("regex");
    if let Some(c) = se.captures(stem) {
        return Some(FileEp::Se(c[1].parse().ok()?, c[2].parse().ok()?));
    }
    let noise = Regex::new(r"(?i)\d{3,4}p|[xh][ .]?26[45]|\d{1,2}bits?|\d\.\d|\b(19|20)\d{2}\b")
        .expect("regex");
    let clean = noise.replace_all(stem, " ");
    let re = Regex::new(r"(\d{1,4})").expect("regex");
    re.captures_iter(&clean)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .last()
        .map(FileEp::Abs)
}

/// Numéro absolu d'un fichier (via la table (saison, épisode) → absolu de l'Arr pour un `SxxEyy`).
pub fn file_number(name: &str, se_to_abs: &HashMap<(u32, u32), u32>) -> Option<u32> {
    match file_episode(name)? {
        FileEp::Abs(n) => Some(n),
        FileEp::Se(s, e) => se_to_abs.get(&(s, e)).copied(),
    }
}

fn is_video(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [".mkv", ".avi", ".mp4", ".m4v", ".ts"]
        .iter()
        .any(|e| n.ends_with(e))
}

/// Clé de choix commune : ≤ 1080p (1080 > 720 > inconnue), sources ≥ 2, codec, audio, sources.
fn quality_key(r: &Release) -> (i64, bool, u8, u8, i64) {
    let res = resolution(&r.title);
    (
        if res > 1080 { -1 } else { res },
        r.seeders >= 2,
        codec_rank(&r.title),
        audio_rank(&r.title),
        r.seeders,
    )
}

/// Film : bonne année (± 1), jamais 4K, sous le plafond, assez de sources.
pub fn pick_movie(
    releases: &[Release],
    year: i64,
    min_seeders: i64,
    max_gb: f64,
) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| r.seeders >= min_seeders && resolution(&r.title) <= 1080)
        .filter(|r| r.size <= 0 || (r.size as f64 / 1_073_741_824.0) <= max_gb)
        .filter(|r| year <= 0 || (year - 1..=year + 1).any(|y| r.title.contains(&y.to_string())))
        .max_by_key(|r| quality_key(r))
}

/// Série : la release qui couvre le plus d'épisodes manquants (numéros absolus), puis la meilleure qualité, puis la
/// plus petite plage (moins de fichiers à trier). Taille jugée par épisode sur toute la plage annoncée.
pub fn pick_series<'a>(
    releases: &'a [Release],
    missing: &BTreeSet<u32>,
    min_seeders: i64,
    max_gb_per_episode: f64,
) -> Option<(&'a Release, usize)> {
    releases
        .iter()
        .filter(|r| r.seeders >= min_seeders && resolution(&r.title) <= 1080)
        .filter_map(|r| {
            let (a, b) = episode_range(&r.title)?;
            let n = (b - a + 1) as f64;
            if r.size > 0 && (r.size as f64 / 1_073_741_824.0) / n > max_gb_per_episode {
                return None;
            }
            let covered = missing.iter().filter(|e| (a..=b).contains(e)).count();
            (covered > 0).then_some((r, covered, b - a))
        })
        .max_by_key(|(r, covered, span)| (*covered, quality_key(r), std::cmp::Reverse(*span)))
        .map(|(r, c, _)| (r, c))
}

/// (saison, épisode) → numéro absolu, d'après `absolute_numbers`.
pub fn se_to_abs(episodes: &[Value], abs: &HashMap<i64, u32>) -> HashMap<(u32, u32), u32> {
    episodes
        .iter()
        .filter_map(|e| {
            let id = e.get("id").and_then(Value::as_i64)?;
            Some((
                (
                    e.get("seasonNumber").and_then(Value::as_i64)? as u32,
                    e.get("episodeNumber").and_then(Value::as_i64)? as u32,
                ),
                *abs.get(&id)?,
            ))
        })
        .collect()
}

/// Numéro absolu d'un épisode Sonarr : `absoluteEpisodeNumber`, sinon son rang parmi les épisodes hors spéciaux.
pub fn absolute_numbers(episodes: &[Value]) -> HashMap<i64, u32> {
    let mut regular: Vec<&Value> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0) > 0)
        .collect();
    regular.sort_by_key(|e| {
        (
            e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0),
            e.get("episodeNumber").and_then(Value::as_i64).unwrap_or(0),
        )
    });
    regular
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let id = e.get("id").and_then(Value::as_i64)?;
            let abs = e
                .get("absoluteEpisodeNumber")
                .and_then(Value::as_i64)
                .map(|n| n as u32)
                .unwrap_or(i as u32 + 1);
            Some((id, abs))
        })
        .collect()
}

pub fn due(rec: Option<&RussianTitleRecord>, now: i64, retry_hours: i64) -> bool {
    match rec {
        None => true,
        Some(r) if r.outcome == "grabbed" => false,
        // une erreur (Jackett, qBittorrent) est retentée au bout d'une heure
        Some(r) if r.outcome == "error" => now - r.at >= 3600,
        Some(r) => now - r.at >= retry_hours * 3600,
    }
}

async fn jackett_search(ctx: &TaskContext, kind: &str, query: &str) -> Result<Vec<Release>> {
    let cfg = &ctx.cfg.tasks.russian_search;
    let key = ctx
        .secrets
        .seedbox_jackett_api_key
        .as_ref()
        .context("SEEDBOX_JACKETT_API_KEY absent")?;
    let url = format!(
        "{}/api/v2.0/indexers/{}/results/torznab/api",
        cfg.jackett_url.trim_end_matches('/'),
        cfg.indexer
    );
    let (t, cat) = if kind == "tv" {
        ("tvsearch", "5000")
    } else {
        ("movie", "2000")
    };
    let resp = ctx
        .http
        .get(&url)
        .query(&[
            ("apikey", key.expose()),
            ("t", t),
            ("q", query),
            ("cat", cat),
        ])
        .timeout(Duration::from_secs(150))
        .send()
        .await
        // l'URL porte la clé Jackett (`apikey=`) : retirée de l'erreur, journalisée et notée dans l'état
        .map_err(reqwest::Error::without_url)
        .context("Jackett injoignable")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Jackett {status}");
    }
    Ok(parse_items(&body))
}

async fn record(ctx: &TaskContext, key: &str, rec: RussianTitleRecord) -> Result<()> {
    if ctx.dry_run {
        return Ok(());
    }
    let k = key.to_string();
    ctx.state
        .update(|s| {
            s.russian_title.insert(k, rec);
        })
        .await
}

/// Import d'un torrent complet : fichiers vidéo choisis par chemin exact, épisode par `SxxEyy` lu par l'Arr ou par
/// numéro absolu, seulement les épisodes sans fichier.
async fn import(
    ctx: &TaskContext,
    arr: &ArrClient,
    movie: bool,
    arr_id: i64,
    hash: &str,
    content_path: &str,
    incomplete: &HashSet<String>,
) -> Result<usize> {
    let cands = arr
        .get(
            "api/v3/manualimport",
            &[("folder", content_path), ("filterExistingFiles", "false")],
        )
        .await?;
    let cands: Vec<Value> = cands
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| {
            c.get("path").and_then(Value::as_str).is_some_and(|p| {
                p.starts_with(content_path) && is_video(p) && !incomplete.contains(p)
            })
        })
        .collect();
    let base = |c: &Value| {
        json!({
            "path": c.get("path"),
            "quality": c.get("quality"),
            "languages": c.get("languages").cloned().unwrap_or_else(|| json!([])),
            "releaseGroup": c.get("releaseGroup"),
            "indexerFlags": c.get("indexerFlags").and_then(Value::as_i64).unwrap_or(0),
            "downloadId": hash.to_ascii_uppercase(),
        })
    };
    let mut files = Vec::new();
    if movie {
        if let Some(c) = cands
            .iter()
            .max_by_key(|c| c.get("size").and_then(Value::as_i64).unwrap_or(0))
        {
            let mut f = base(c);
            f["movieId"] = json!(arr_id);
            files.push(f);
        }
    } else {
        let eps = arr.episodes(arr_id).await?;
        let abs = absolute_numbers(&eps);
        let se_abs = se_to_abs(&eps, &abs);
        let by_abs: HashMap<u32, i64> = abs.iter().map(|(id, n)| (*n, *id)).collect();
        let without_file: HashSet<i64> = eps
            .iter()
            .filter(|e| e.get("hasFile").and_then(Value::as_bool) != Some(true))
            .filter_map(|e| e.get("id").and_then(Value::as_i64))
            .collect();
        let mut taken: HashSet<i64> = HashSet::new();
        for c in &cands {
            let path = c.get("path").and_then(Value::as_str).unwrap_or("");
            let mut ids: Vec<i64> =
                if c.pointer("/series/id").and_then(Value::as_i64) == Some(arr_id) {
                    c.get("episodes")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|e| e.get("id").and_then(Value::as_i64))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
            if ids.is_empty() {
                ids = file_number(path, &se_abs)
                    .and_then(|n| by_abs.get(&n).copied())
                    .into_iter()
                    .collect();
            }
            if ids.is_empty()
                || !ids
                    .iter()
                    .all(|i| without_file.contains(i) && !taken.contains(i))
            {
                continue;
            }
            taken.extend(ids.iter().copied());
            let mut f = base(c);
            f["seriesId"] = json!(arr_id);
            f["episodeIds"] = json!(ids);
            files.push(f);
        }
    }
    let n = files.len();
    if n > 0 && !ctx.dry_run {
        arr.command(json!({ "name": "ManualImport", "files": files, "importMode": "copy" }))
            .await?;
    }
    Ok(n)
}

#[async_trait]
impl Task for RussianSearch {
    fn name(&self) -> &'static str {
        "russian_search"
    }

    fn label(&self) -> &'static str {
        "Recherche russe (RuTracker)"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.russian_search.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.russian_search;
        let lib = &ctx.cfg.tasks.anime_library;
        if !lib.russian {
            return Ok(Report::new("voie russe désactivée", 0));
        }
        let Some(qbit) = ctx.seedbox_qbit.as_ref() else {
            return Ok(Report::new("pas de qBittorrent seedbox", 0));
        };
        // seedbox injoignable (qBittorrent ou applis arrêtés) : rien à faire ce passage, pas une erreur de la
        // tâche — 102 erreurs comptées pour l'arrêt des applis du 30/09 au 02/10 ; `seedbox_health` alerte déjà
        let Some(torrents) =
            super::side_or_skip("russian_search", "seedbox", qbit.torrents().await)
        else {
            return Ok(Report::new("seedbox injoignable", 0));
        };
        let records = ctx.state.read(|s| s.russian_title.clone()).await;
        let t = now();
        let mut notes: Vec<String> = Vec::new();
        let mut actions = 0u32;

        // 1. imports des torrents pris et terminés
        for (key, rec) in records.iter().filter(|(_, r)| r.outcome == "grabbed") {
            let Some(hash) = rec.hash.as_deref() else {
                continue;
            };
            let (movie, arr_id) = match key.split(':').collect::<Vec<_>>()[..] {
                [_, kind, id] => (kind == "movie", id.parse::<i64>().unwrap_or(0)),
                _ => continue,
            };
            let arr = if movie {
                ctx.seedbox_radarr.as_ref()
            } else {
                ctx.seedbox_sonarr.as_ref()
            };
            let Some(arr) = arr else { continue };
            let Some(tor) = torrents.iter().find(|x| x.hash.eq_ignore_ascii_case(hash)) else {
                record(
                    ctx,
                    key,
                    RussianTitleRecord {
                        at: t,
                        outcome: "error".into(),
                        detail: "torrent disparu".into(),
                        hash: None,
                    },
                )
                .await?;
                continue;
            };
            if tor.progress < 1.0 {
                continue;
            }
            // fichiers désélectionnés ou incomplets : jamais importés (Кухня, 2026-09-27)
            let incomplete = match qbit.files(hash).await {
                Ok(f) => super::torrent_import::incomplete_paths(&f, &tor.save_path),
                Err(e) => {
                    warn!(task = "russian_search", key = %key, error = format!("{e:#}"), "file list unreadable: import postponed");
                    continue;
                }
            };
            match import(
                ctx,
                arr,
                movie,
                arr_id,
                hash,
                &tor.content_path,
                &incomplete,
            )
            .await
            {
                Ok(n) => {
                    // pas de relance du partage : l'hébergeur (Ultra.cc, `~/.config/.stop_pub`) arrête toutes les 5 min
                    // les torrents non « privés », ce qu'est RuTracker (2026-09-27) ; ne pas lutter contre sa règle
                    info!(task = "russian_search", key = %key, files = n, "imported");
                    notes.push(format!("importé {n} fichier(s)"));
                    actions += n as u32;
                    request_library_scan();
                    record(
                        ctx,
                        key,
                        RussianTitleRecord {
                            at: t,
                            outcome: "imported".into(),
                            detail: format!("{n} fichier(s) : {}", rec.detail),
                            hash: rec.hash.clone(),
                        },
                    )
                    .await?;
                }
                Err(e) => {
                    warn!(task = "russian_search", key = %key, error = format!("{e:#}"), "import failed")
                }
            }
        }

        // 2. recherches par titre original
        let mut left = cfg.max_per_run;
        for (movie, arr) in [
            (false, ctx.seedbox_sonarr.as_ref()),
            (true, ctx.seedbox_radarr.as_ref()),
        ] {
            let Some(arr) = arr else { continue };
            let read = if movie {
                arr.movies().await
            } else {
                arr.series().await
            };
            let Some(list) = super::side_or_skip("russian_search", arr.name, read) else {
                notes.push(format!("{} injoignable", arr.name));
                continue;
            };
            for item in list.iter().filter(|i| russian_route(i, lib)) {
                if left == 0 {
                    break;
                }
                let Some(id) = item.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                let kind = if movie { "movie" } else { "series" };
                let key = format!("seedbox:{kind}:{id}");
                if item.get("monitored").and_then(Value::as_bool) != Some(true)
                    || !due(records.get(&key), t, cfg.retry_hours)
                {
                    continue;
                }
                let tmdb = item.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
                if tmdb <= 0 {
                    continue;
                }
                // manquants : film sans fichier ; série : épisodes suivis, diffusés, sans fichier (numéros absolus)
                let mut missing: BTreeSet<u32> = BTreeSet::new();
                let mut se_abs: HashMap<(u32, u32), u32> = HashMap::new();
                if movie {
                    if item.get("hasFile").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                } else {
                    // l'Arr vient de répondre à la liste et plus à ça : on laisse ce côté pour ce passage
                    let Some(eps) =
                        super::side_or_skip("russian_search", arr.name, arr.episodes(id).await)
                    else {
                        notes.push(format!("{} injoignable", arr.name));
                        break;
                    };
                    let abs = absolute_numbers(&eps);
                    se_abs = se_to_abs(&eps, &abs);
                    let today = chrono::Utc::now().to_rfc3339();
                    for e in &eps {
                        let aired = e
                            .get("airDateUtc")
                            .and_then(Value::as_str)
                            .is_some_and(|d| d <= today.as_str());
                        if e.get("monitored").and_then(Value::as_bool) == Some(true)
                            && e.get("hasFile").and_then(Value::as_bool) != Some(true)
                            && aired
                        {
                            if let Some(n) = e
                                .get("id")
                                .and_then(Value::as_i64)
                                .and_then(|i| abs.get(&i))
                            {
                                missing.insert(*n);
                            }
                        }
                    }
                    if missing.is_empty() {
                        continue;
                    }
                }
                left -= 1;
                let details = if movie {
                    ctx.jellyseerr.movie_details(tmdb).await
                } else {
                    ctx.jellyseerr.tv_details(tmdb).await
                }?;
                let original = details
                    .get("originalTitle")
                    .or_else(|| details.get("originalName"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let title = item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let year = item.get("year").and_then(Value::as_i64).unwrap_or(0);
                if original.is_empty() {
                    continue;
                }
                let releases = match jackett_search(
                    ctx,
                    if movie { "movie" } else { "tv" },
                    &original,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(task = "russian_search", title = %title, error = format!("{e:#}"), "search failed");
                        record(
                            ctx,
                            &key,
                            RussianTitleRecord {
                                at: t - (cfg.retry_hours - 1) * 3600,
                                outcome: "error".into(),
                                detail: format!("{e:#}"),
                                hash: None,
                            },
                        )
                        .await?;
                        continue;
                    }
                };
                let chosen = if movie {
                    pick_movie(&releases, year, cfg.min_seeders, cfg.max_gb_per_movie)
                        .map(|r| (r, 1usize))
                } else {
                    pick_series(&releases, &missing, cfg.min_seeders, cfg.max_gb_per_episode)
                };
                let Some((rel, covered)) = chosen else {
                    info!(task = "russian_search", title = %title, original = %original, results = releases.len(), "nothing suitable");
                    notes.push(format!("{title} : rien ({} résultats)", releases.len()));
                    record(
                        ctx,
                        &key,
                        RussianTitleRecord {
                            at: t,
                            outcome: "none".into(),
                            detail: format!(
                                "« {original} » : {} résultat(s), aucun retenu",
                                releases.len()
                            ),
                            hash: None,
                        },
                    )
                    .await?;
                    continue;
                };
                if ctx.dry_run {
                    info!(task = "russian_search", title = %title, release = %rel.title, covered, "dry-run: would grab");
                    notes.push(format!(
                        "{title} : prendrait « {} » ({covered} ép.)",
                        rel.title
                    ));
                    continue;
                }
                // .torrent par Jackett, ajouté au qBittorrent de la seedbox
                let before: HashSet<String> = torrents
                    .iter()
                    .map(|x| x.hash.to_ascii_lowercase())
                    .collect();
                let already = rel.infohash.as_ref().is_some_and(|h| before.contains(h));
                let hash = if already {
                    // déjà dans qBittorrent (ajouté à la main, ou redemandé) : on le reprend tel quel
                    rel.infohash.clone()
                } else {
                    // le lien porte la clé Jackett : jamais journalisé
                    let bytes = ctx
                        .http
                        .get(&rel.link)
                        .timeout(Duration::from_secs(120))
                        .send()
                        .await?
                        .bytes()
                        .await?
                        .to_vec();
                    // déjà présent sans hash annoncé : retrouvé par le nom de son dossier racine
                    let root = crate::torrent_file::files(&bytes).ok().and_then(|e| {
                        e.first()
                            .and_then(|f| f.path.split('/').next().map(str::to_string))
                    });
                    let present = root.as_deref().and_then(|r| {
                        torrents
                            .iter()
                            .find(|x| x.name == r)
                            .map(|x| x.hash.to_ascii_lowercase())
                    });
                    if let Some(h) = present {
                        Some(h)
                    } else if let Err(e) = qbit.add_torrent_with(bytes, "", TAG, true).await {
                        warn!(task = "russian_search", title = %title, error = format!("{e:#}"), "add failed");
                        record(
                            ctx,
                            &key,
                            RussianTitleRecord {
                                at: t,
                                outcome: "error".into(),
                                detail: format!("{e:#}"),
                                hash: None,
                            },
                        )
                        .await?;
                        continue;
                    } else {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        match &rel.infohash {
                            Some(h) => Some(h.clone()),
                            None => qbit
                                .torrents()
                                .await?
                                .into_iter()
                                .map(|x| x.hash.to_ascii_lowercase())
                                .find(|h| !before.contains(h)),
                        }
                    }
                };
                let Some(hash) = hash else {
                    record(
                        ctx,
                        &key,
                        RussianTitleRecord {
                            at: t,
                            outcome: "error".into(),
                            detail: "torrent ajouté mais introuvable".into(),
                            hash: None,
                        },
                    )
                    .await?;
                    continue;
                };
                // série : seuls les fichiers des épisodes manquants (la liste peut mettre quelques secondes à venir)
                if !movie {
                    let mut files = Vec::new();
                    for _ in 0..8 {
                        match qbit.files(&hash).await {
                            Ok(f) if !f.is_empty() => {
                                files = f;
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(task = "russian_search", title = %title, error = format!("{e:#}"), "file list unreadable")
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    let skip: Vec<usize> = files
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| {
                            !is_video(&f.name)
                                || file_number(&f.name, &se_abs)
                                    .is_none_or(|n| !missing.contains(&n))
                        })
                        .map(|(i, _)| i)
                        .collect();
                    if files.is_empty() {
                        warn!(task = "russian_search", title = %title, "file list empty: whole torrent kept");
                    } else if skip.len() == files.len() {
                        // aucun fichier reconnu : on garde tout plutôt que rien
                        warn!(task = "russian_search", title = %title, files = files.len(), "no file matched a missing episode: whole torrent kept");
                    } else if let Err(e) = qbit.set_file_priority(&hash, &skip, 0).await {
                        warn!(task = "russian_search", title = %title, error = format!("{e:#}"), "file priorities not set: whole torrent kept");
                    } else {
                        info!(task = "russian_search", title = %title, kept = files.len() - skip.len(), total = files.len(), "only missing episodes kept");
                    }
                }
                // ajouté arrêté : démarré seulement maintenant, fichiers inutiles déjà désélectionnés
                if let Err(e) = qbit.start(&hash, false).await {
                    warn!(task = "russian_search", title = %title, error = format!("{e:#}"), "torrent not started");
                }
                info!(task = "russian_search", title = %title, release = %rel.title, covered, "grabbed");
                notes.push(format!("{title} : « {} » ({covered} ép.)", rel.title));
                actions += 1;
                record(
                    ctx,
                    &key,
                    RussianTitleRecord {
                        at: t,
                        outcome: "grabbed".into(),
                        detail: rel.title.clone(),
                        hash: Some(hash),
                    },
                )
                .await?;
            }
        }
        let summary = if notes.is_empty() {
            "rien à faire".to_string()
        } else {
            notes.join(" · ")
        };
        Ok(Report::new(summary, actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(title: &str, seeders: i64, gb: f64) -> Release {
        Release {
            title: title.into(),
            link: "http://j/dl".into(),
            size: (gb * 1_073_741_824.0) as i64,
            seeders,
            infohash: None,
        }
    }

    #[test]
    fn reads_torznab_items() {
        let xml = r#"<rss><channel><title>x</title><item><title>E1-120 of 120 RUS [2010-2012, DVDRemux]</title>
<link>https://j/dl/rutracker/?jackett_apikey=K&amp;file=a</link><size>95246578483</size>
<torznab:attr name="seeders" value="5" /><torznab:attr name="infohash" value="ABCDEF" /></item></channel></rss>"#;
        let r = parse_items(xml);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].seeders, 5);
        assert_eq!(r[0].link, "https://j/dl/rutracker/?jackett_apikey=K&file=a");
        assert_eq!(r[0].infohash.as_deref(), Some("abcdef"));
    }

    #[test]
    fn russian_episode_ranges() {
        assert_eq!(
            episode_range("E1-120 of 120 RUS [2010-2012, DVDRemux]"),
            Some((1, 120))
        );
        assert_eq!(
            episode_range("S4E61-268 of 278 + 2 + RUS [2011-2016, WEBRip 1080p]"),
            Some((61, 268))
        );
        assert_eq!(
            episode_range("S6E1-20 20 RUS (101-120) [2011-2012, 2x DVD9]"),
            Some((101, 120))
        );
        assert_eq!(
            episode_range("S1E1-120 of 280 RUS [2010-2012, DVDRip]"),
            Some((1, 120))
        );
        assert_eq!(
            episode_range("Le grand frère DVO + AVO [1982, WEB-DL 1080p]"),
            None
        );
    }

    #[test]
    fn file_numbers() {
        let none = HashMap::new();
        assert_eq!(
            file_number("Интерны DVDRip/03 сезон/Интерны_060.avi", &none),
            Some(60)
        );
        assert_eq!(file_number("Show/Kukhnya - 12.mkv", &none), Some(12));
        assert_eq!(file_number("Film 2010.mkv", &none), None);
        // la résolution, le codec et l'audio ne sont pas des numéros d'épisode
        assert_eq!(
            file_number("Kuhnya.057.WEBRip.1080p.x264.AAC.5.1.mkv", &none),
            Some(57)
        );
        // SxxEyy : converti en absolu par la table de l'Arr
        let map: HashMap<(u32, u32), u32> = [((2, 2), 22)].into_iter().collect();
        assert_eq!(
            file_number("Kukhnya.S02E02.WEBRip.1080p.mkv", &map),
            Some(22)
        );
        assert_eq!(
            file_episode("Kukhnya.S02E02.1080p.mkv"),
            Some(FileEp::Se(2, 2))
        );
    }

    #[test]
    fn series_pick_covers_most_missing_then_quality() {
        let missing: BTreeSet<u32> = (1..=60).collect();
        let r = vec![
            rel("E1-120 of 120 RUS [2010-2012, DVDRemux]", 5, 88.7),
            rel("S4E61-268 of 278 RUS [2011-2016, WEBRip 1080p]", 14, 168.6),
            rel("S1E1-120 of 280 RUS [2010-2012, DVDRip]", 155, 35.2),
            rel("S14E1-20 of 20 RUS [2016, SATRip]", 3, 6.5),
        ];
        let (p, covered) = pick_series(&r, &missing, 2, 3.0).unwrap();
        assert_eq!(covered, 60);
        assert!(p.title.contains("DVDRip"), "{}", p.title);
        // rien ne couvre les épisodes manquants : rien
        let later: BTreeSet<u32> = [500].into_iter().collect();
        assert!(pick_series(&r, &later, 2, 3.0).is_none());
    }

    #[test]
    fn movie_pick_needs_the_year_and_no_4k() {
        let r = vec![
            rel("Brat RUS [1997, BDRip 1080p]", 40, 8.0),
            rel("Brat RUS [1997, UHD 2160p]", 90, 30.0),
            rel("Brat 2 RUS [2000, BDRip 1080p]", 60, 8.0),
        ];
        let p = pick_movie(&r, 1997, 2, 20.0).unwrap();
        assert!(p.title.contains("1997, BDRip 1080p"));
        assert!(pick_movie(&r, 1970, 2, 20.0).is_none());
    }

    #[test]
    fn absolute_numbering_falls_back_to_order() {
        let eps = vec![
            json!({"id": 1, "seasonNumber": 0, "episodeNumber": 1}),
            json!({"id": 2, "seasonNumber": 1, "episodeNumber": 1}),
            json!({"id": 3, "seasonNumber": 1, "episodeNumber": 2}),
            json!({"id": 4, "seasonNumber": 2, "episodeNumber": 1, "absoluteEpisodeNumber": 61}),
        ];
        let a = absolute_numbers(&eps);
        assert_eq!(a.get(&2), Some(&1));
        assert_eq!(a.get(&3), Some(&2));
        assert_eq!(a.get(&4), Some(&61));
        assert!(!a.contains_key(&1));
    }

    #[test]
    fn grabbed_waits_for_import() {
        let r = |o: &str, at| RussianTitleRecord {
            at,
            outcome: o.into(),
            detail: String::new(),
            hash: None,
        };
        assert!(due(None, 100_000, 24));
        assert!(!due(Some(&r("grabbed", 0)), 100_000, 24));
        assert!(!due(Some(&r("none", 100_000 - 3600)), 100_000, 24));
        assert!(due(Some(&r("none", 100_000 - 24 * 3600)), 100_000, 24));
        assert!(due(Some(&r("error", 100_000 - 3600)), 100_000, 24));
    }
}
