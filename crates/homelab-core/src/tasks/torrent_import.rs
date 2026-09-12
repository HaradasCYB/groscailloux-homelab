//! Importe dans Radarr/Sonarr les torrents ajoutés à la main dans qBittorrent (VPS et seedbox),
//! pour qu'ils apparaissent dans Jellyfin comme le reste.
//!
//! Pour chaque torrent terminé pas encore jugé : ignoré s'il est suivi par un Arr (historique du
//! téléchargement), s'il n'a pas de vidéo, ou si ses vidéos sont déjà hardlinkées en bibliothèque
//! (VPS) ; sinon parse → recherche → choix de la fiche (`matching`), refus si la fiche a déjà des
//! fichiers sur l'autre machine (doublon dans Jellyfin), ajout de la fiche **non surveillée** si
//! besoin, puis `ManualImport` en `importMode: copy` (= hardlink) des seuls fichiers sans rejet.
//!
//! Jamais `importMode: auto` : pour un téléchargement que l'Arr n'a pas demandé, `auto` déplace
//! le fichier et casse le seed. Aucune modification des torrents (catégorie, chemin).
//! Jellyfin : le LibraryMonitor voit les imports du VPS ; `seedbox_refresh` ceux de la seedbox.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::id_match_import::select_files;
use super::{Report, Task};
use crate::classify::{classify, is_video, MediaKind};
use crate::clients::{ArrClient, Torrent, TorrentFile};
use crate::config::Config;
use crate::context::{Side, TaskContext};
use crate::matching::{parsed_movie, parsed_series, pick_movie, pick_series};
use crate::state::{now, TorrentImportRecord};

pub struct TorrentImport;

/// Résultat de l'examen d'un torrent.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub outcome: &'static str,
    pub detail: String,
    pub files: u32,
    /// L'examen a coûté des appels lourds (parse, recherche, import) : compte dans le budget.
    pub costly: bool,
}

impl Outcome {
    fn cheap(outcome: &'static str, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: detail.into(),
            files: 0,
            costly: false,
        }
    }
    fn costly(outcome: &'static str, detail: impl Into<String>) -> Self {
        Self {
            costly: true,
            ..Self::cheap(outcome, detail)
        }
    }
}

pub fn state_key(side: &str, hash: &str) -> String {
    format!("{side}:{}", hash.to_ascii_lowercase())
}

/// Terminé, chemin connu, et pas encore jugé définitivement.
pub fn is_candidate(t: &Torrent, record: Option<&TorrentImportRecord>) -> bool {
    t.progress >= 1.0 && !t.content_path.is_empty() && record.map(|r| !r.is_final()).unwrap_or(true)
}

/// Vidéos du torrent, sans les extraits (`sample`).
pub fn video_files(files: &[TorrentFile]) -> Vec<&TorrentFile> {
    files
        .iter()
        .filter(|f| {
            let base = f.name.rsplit('/').next().unwrap_or(&f.name);
            is_video(base)
                && !f
                    .name
                    .to_ascii_lowercase()
                    .split(['/', '.', '-', '_', ' '])
                    .any(|w| w == "sample")
        })
        .collect()
}

/// Libellés à soumettre au parse : le nom du torrent, puis le premier fichier vidéo (ordre alphabétique).
pub fn parse_labels(name: &str, videos: &[&TorrentFile]) -> Vec<String> {
    let mut out = vec![name.to_string()];
    let first = videos
        .iter()
        .map(|f| f.name.rsplit('/').next().unwrap_or(&f.name))
        .min();
    if let Some(f) = first {
        if f != name {
            out.push(f.to_string());
        }
    }
    out
}

/// Rejet qui ne tient qu'à l'identification (on fournit nous-mêmes la fiche et les épisodes).
pub fn is_identification_rejection(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "unknown movie",
        "unknown series",
        "unable to parse",
        "unable to identify",
        "was matched to",
        "grab history",
    ]
    .iter()
    .any(|k| r.contains(k))
}

/// Épisodes d'une série désignés par le `parse` d'un nom de fichier : saison + numéros, sinon
/// numéros absolus (anime).
pub fn map_episodes(parse: &Value, episodes: &[Value]) -> Vec<i64> {
    let Some(pei) = parse.get("parsedEpisodeInfo") else {
        return vec![];
    };
    let nums = |k: &str| -> Vec<i64> {
        pei.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    let season = pei.get("seasonNumber").and_then(Value::as_i64);
    let (eps, abs) = (nums("episodeNumbers"), nums("absoluteEpisodeNumbers"));
    let field = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64);
    let pick = |f: &dyn Fn(&Value) -> bool| -> Vec<i64> {
        episodes
            .iter()
            .filter(|e| f(e))
            .filter_map(|e| field(e, "id"))
            .collect()
    };
    if let (Some(s), false) = (season, eps.is_empty()) {
        let ids = pick(&|e| {
            field(e, "seasonNumber") == Some(s)
                && field(e, "episodeNumber")
                    .map(|n| eps.contains(&n))
                    .unwrap_or(false)
        });
        if !ids.is_empty() {
            return ids;
        }
    }
    if !abs.is_empty() {
        return pick(&|e| {
            field(e, "absoluteEpisodeNumber")
                .map(|n| abs.contains(&n))
                .unwrap_or(false)
        });
    }
    vec![]
}

/// Fichiers importables pour une fiche encore vide : aperçu sans id, fiche et épisodes fournis
/// par nous ; les rejets d'identification sont ignorés, les autres (sample…) excluent le fichier.
pub fn fresh_files(
    candidates: &[Value],
    movie: bool,
    id: i64,
    download_id: &str,
    episodes_by_path: &HashMap<String, Vec<i64>>,
) -> (Vec<Value>, Vec<String>) {
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    for c in candidates {
        let path = c.get("path").and_then(Value::as_str).unwrap_or("");
        let rel = c
            .get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_string();
        let blocking: Vec<&str> = c
            .get("rejections")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(|x| x.get("reason").and_then(Value::as_str))
                    .filter(|r| !is_identification_rejection(r))
                    .collect()
            })
            .unwrap_or_default();
        if !blocking.is_empty() {
            skipped.push(format!("{rel}: {}", blocking.join(", ")));
            continue;
        }
        let mut f = json!({
            "path": path,
            "quality": c.get("quality"),
            "languages": c.get("languages").cloned().unwrap_or_else(|| json!([])),
            "releaseGroup": c.get("releaseGroup"),
            "indexerFlags": c.get("indexerFlags").and_then(Value::as_i64).unwrap_or(0),
            "downloadId": download_id,
        });
        if movie {
            f["movieId"] = json!(id);
        } else {
            let eps = episodes_by_path.get(path).cloned().unwrap_or_default();
            if eps.is_empty() {
                skipped.push(format!("{rel}: épisode non identifié"));
                continue;
            }
            f["releaseType"] = json!(if eps.len() > 1 {
                "multiEpisode"
            } else {
                "singleEpisode"
            });
            f["seriesId"] = json!(id);
            f["episodeIds"] = json!(eps);
        }
        files.push(f);
    }
    (files, skipped)
}

/// La fiche de l'autre machine a-t-elle déjà des fichiers ?
pub fn has_files(movie: bool, item: &Value) -> bool {
    if movie {
        item.get("hasFile")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    } else {
        item.pointer("/statistics/episodeFileCount")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
    }
}

/// Corps d'ajout d'une fiche, non surveillée et sans recherche.
pub fn add_body(movie: bool, hit: &Value, root: &str, profile: i64) -> Value {
    let mut body = hit.clone();
    let overrides = if movie {
        json!({
            "qualityProfileId": profile, "rootFolderPath": root, "monitored": false,
            "minimumAvailability": "released",
            "addOptions": {"searchForMovie": false, "monitor": "none"}
        })
    } else {
        json!({
            "qualityProfileId": profile, "rootFolderPath": root, "monitored": false,
            "seasonFolder": true,
            "addOptions": {"monitor": "none", "searchForMissingEpisodes": false,
                           "searchForCutoffUnmetEpisodes": false}
        })
    };
    if let (Some(b), Some(o)) = (body.as_object_mut(), overrides.as_object()) {
        b.remove("id");
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
    body
}

/// Après une erreur : nouvel essai tant que `max` n'est pas atteint.
pub fn after_error(prev_attempts: u32, max: u32) -> (&'static str, u32) {
    let attempts = prev_attempts + 1;
    (if attempts >= max { "error" } else { "retry" }, attempts)
}

/// Toutes les vidéos ont déjà un autre lien (importées par hardlink) : rien à faire.
fn all_linked(side: &Side<'_>, t: &Torrent, videos: &[&TorrentFile]) -> bool {
    let Some((container, host)) = &side.local_downloads else {
        return false;
    };
    !videos.is_empty()
        && videos.iter().all(|f| {
            host_path(container, host, &t.save_path, &f.name)
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.nlink() > 1)
                .unwrap_or(false)
        })
}

/// Chemin hôte d'un fichier de torrent : `save_path` vu par qBit → dossier de l'hôte.
fn host_path(
    container_root: &str,
    host_root: &Path,
    save_path: &str,
    name: &str,
) -> Option<std::path::PathBuf> {
    let rel = save_path
        .trim_end_matches('/')
        .strip_prefix(container_root.trim_end_matches('/'))?;
    if !rel.is_empty() && !rel.starts_with('/') {
        return None; // « /downloads2 » n'est pas sous « /downloads »
    }
    Some(host_root.join(rel.trim_start_matches('/')).join(name))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn examine(
    ctx: &TaskContext,
    side: &Side<'_>,
    others: &[&Side<'_>],
    t: &Torrent,
) -> Result<Outcome> {
    let hash = t.hash.to_ascii_uppercase();
    let managed = side.radarr.history_count_for_download(&hash).await?
        + side.sonarr.history_count_for_download(&hash).await?;
    if managed > 0 {
        return Ok(Outcome::cheap("arr_managed", ""));
    }
    let files = side.qbit.files(&t.hash).await?;
    let videos = video_files(&files);
    if videos.is_empty() {
        return Ok(Outcome::cheap("no_video", ""));
    }
    if all_linked(side, t, &videos) {
        return Ok(Outcome::cheap("already_linked", ""));
    }

    // série si le nom ou l'un des fichiers porte un marqueur d'épisode ou de saison
    let movie = classify(&t.name) == MediaKind::Movie
        && videos.iter().all(|f| classify(&f.name) == MediaKind::Movie);
    let (arr, kind, ext_param, id_param, root) = if movie {
        (side.radarr, "movie", "tmdbId", "movieId", &side.radarr_root)
    } else {
        (
            side.sonarr,
            "series",
            "tvdbId",
            "seriesId",
            &side.sonarr_root,
        )
    };
    // nom du torrent, puis celui du premier fichier vidéo (« Star Wars Rebels (2014) » ne se
    // parse pas, « Star.Wars.Rebels.S01E01.mkv » si)
    let mut parsed = None;
    for label in parse_labels(&t.name, &videos) {
        let parse = arr.parse(&label).await?;
        parsed = if movie {
            parsed_movie(&parse)
        } else {
            parsed_series(&parse)
        };
        if parsed.is_some() {
            break;
        }
    }
    let Some(parsed) = parsed else {
        return Ok(Outcome::costly(
            "no_match",
            "titre non reconnu par le parse",
        ));
    };
    let term = if movie && parsed.year > 0 {
        format!("{} {}", parsed.title, parsed.year)
    } else {
        parsed.title.clone()
    };
    let hits = arr.lookup(kind, &term).await?;
    let picked = if movie {
        pick_movie(&hits, &parsed)
    } else {
        pick_series(&hits, &parsed)
    };
    let Some(m) = picked else {
        return Ok(Outcome::costly(
            "no_match",
            format!("aucune fiche pour « {term} »"),
        ));
    };
    let ext_id = m.hit.get(ext_param).and_then(Value::as_i64).unwrap_or(0);
    let title = format!(
        "{} ({})",
        m.hit.get("title").and_then(Value::as_str).unwrap_or("?"),
        m.hit.get("year").and_then(Value::as_i64).unwrap_or(0)
    );
    if m.fuzzy {
        info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, "approximate match (first result, same first word)");
    }

    for other in others {
        let other_arr: &ArrClient = if movie { other.radarr } else { other.sonarr };
        if let Some(item) = other_arr.find_by(kind, ext_param, ext_id).await? {
            if has_files(movie, &item) {
                return Ok(Outcome::costly(
                    "dup_other_side",
                    format!("{title} a déjà des fichiers sur {}", other.name),
                ));
            }
        }
    }

    let existing = arr.find_by(kind, ext_param, ext_id).await?;
    if ctx.dry_run {
        let what = if existing.is_some() {
            "import"
        } else {
            "add unmonitored + import"
        };
        info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, fuzzy = m.fuzzy, "dry-run: would {what}");
        return Ok(Outcome::costly("dry_run", format!("{what} → {title}")));
    }
    // dossier de la fiche absent tant qu'elle n'a aucun fichier : `manualimport` avec l'id y
    // cherche les fichiers existants et répond 500 ; on passe alors par l'import sans id
    let fresh = existing
        .as_ref()
        .map(|i| !has_files(movie, i))
        .unwrap_or(true);
    let id = match existing {
        Some(item) => item
            .get("id")
            .and_then(Value::as_i64)
            .context("fiche sans id")?,
        None => {
            let added = arr
                .add(
                    kind,
                    &add_body(movie, &m.hit, root, side.quality_profile_id),
                )
                .await?;
            let id = added
                .get("id")
                .and_then(Value::as_i64)
                .context("ajout sans id")?;
            info!(task = "torrent_import", side = side.name, %title, id, "added unmonitored");
            if !movie
                && !wait_episodes(arr, id, ctx.cfg.tasks.torrent_import.series_ready_secs).await?
            {
                return Ok(Outcome::costly(
                    "retry",
                    format!("{title} : épisodes pas encore chargés"),
                ));
            }
            id
        }
    };

    let (files, skipped) = if fresh {
        let candidates = arr.manual_import(&t.content_path).await?;
        let mut by_path: HashMap<String, Vec<i64>> = HashMap::new();
        if !movie {
            let episodes = arr.episodes(id).await?;
            for c in &candidates {
                let Some(path) = c.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let base = path.rsplit('/').next().unwrap_or(path);
                let parse = arr.parse(base).await?;
                by_path.insert(path.to_string(), map_episodes(&parse, &episodes));
            }
        }
        fresh_files(&candidates, movie, id, &hash, &by_path)
    } else {
        let candidates = arr
            .manual_import_folder(&t.content_path, id_param, id)
            .await?;
        select_files(&candidates, movie, id, &hash)
    };
    for s in &skipped {
        info!(task = "torrent_import", side = side.name, torrent = %t.name, file = %s, "file skipped");
    }
    if files.is_empty() {
        let why = skipped
            .first()
            .cloned()
            .unwrap_or_else(|| "aucun fichier proposé".into());
        return Ok(Outcome::costly(
            "nothing_importable",
            format!("{title} : {}", truncate(&why, 160)),
        ));
    }
    let n = files.len() as u32;
    let cmd = arr
        .command(json!({ "name": "ManualImport", "files": files, "importMode": "copy" }))
        .await?;
    let cmd_id = cmd.get("id").and_then(Value::as_i64).unwrap_or(0);
    info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, files = n, cmd_id, "manual import (hardlink) triggered");
    Ok(Outcome {
        outcome: "imported",
        detail: format!("{title} : {n} fichier(s)"),
        files: n,
        costly: true,
    })
}

async fn wait_episodes(arr: &ArrClient, series_id: i64, max_secs: u64) -> Result<bool> {
    let mut waited = 0;
    loop {
        if arr.episode_count(series_id).await? > 0 {
            return Ok(true);
        }
        if waited >= max_secs {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += 5;
    }
}

async fn process_side(
    ctx: &TaskContext,
    side: &Side<'_>,
    others: &[&Side<'_>],
    budget: &mut usize,
    counts: &mut BTreeMap<&'static str, u32>,
) -> Result<u32> {
    let max_attempts = ctx.cfg.tasks.torrent_import.max_attempts;
    let torrents = side.qbit.torrents().await?;
    let records = ctx.state.read(|s| s.torrent_import.clone()).await;

    if !ctx.dry_run {
        let prefix = format!("{}:", side.name);
        let live: std::collections::HashSet<String> = torrents
            .iter()
            .map(|t| state_key(side.name, &t.hash))
            .collect();
        ctx.state
            .update(|s| {
                s.torrent_import
                    .retain(|k, _| !k.starts_with(&prefix) || live.contains(k))
            })
            .await?;
    }

    let mut cands: Vec<&Torrent> = torrents
        .iter()
        .filter(|t| is_candidate(t, records.get(&state_key(side.name, &t.hash))))
        .collect();
    cands.sort_by_key(|t| Reverse(t.completion_on));

    let mut imported = 0u32;
    for t in cands {
        if *budget == 0 {
            *counts.entry("pending").or_default() += 1;
            continue;
        }
        let key = state_key(side.name, &t.hash);
        let prev = records.get(&key).map(|r| r.attempts).unwrap_or(0);
        let (outcome, detail, attempts, costly) = match examine(ctx, side, others, t).await {
            Ok(o) => {
                imported += o.files;
                let attempts = if o.outcome == "retry" { prev + 1 } else { prev };
                (o.outcome, o.detail, attempts, o.costly)
            }
            Err(e) => {
                let (outcome, attempts) = after_error(prev, max_attempts);
                warn!(task = "torrent_import", side = side.name, torrent = %t.name, error = %e, attempts, "examine failed");
                (outcome, truncate(&format!("{e:#}"), 200), attempts, true)
            }
        };
        // un « retry » qui a épuisé ses essais devient définitif
        let outcome = if outcome == "retry" && attempts >= max_attempts {
            "error"
        } else {
            outcome
        };
        if costly {
            *budget -= 1;
        }
        *counts.entry(outcome).or_default() += 1;
        if !matches!(outcome, "arr_managed" | "already_linked" | "no_video") {
            info!(task = "torrent_import", side = side.name, torrent = %t.name, outcome, %detail, "decision");
        }
        if ctx.dry_run || outcome == "dry_run" {
            continue;
        }
        let rec = TorrentImportRecord {
            at: now(),
            name: t.name.clone(),
            outcome: outcome.to_string(),
            detail,
            attempts,
        };
        ctx.state
            .update(|s| s.torrent_import.insert(key, rec))
            .await?;
    }
    Ok(imported)
}

#[async_trait]
impl Task for TorrentImport {
    fn name(&self) -> &'static str {
        "torrent_import"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.torrent_import.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let mut budget = ctx.cfg.tasks.torrent_import.max_per_run;
        let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        let mut files = 0u32;
        let sides = ctx.sides();
        for (i, side) in sides.iter().enumerate() {
            let others: Vec<&Side<'_>> = sides
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, s)| s)
                .collect();
            match process_side(ctx, side, &others, &mut budget, &mut counts).await {
                Ok(n) => files += n,
                Err(e) => {
                    *counts.entry("side_error").or_default() += 1;
                    warn!(task = "torrent_import", side = side.name, error = %e, "side skipped")
                }
            }
        }
        let summary = if counts.is_empty() {
            "nothing new".to_string()
        } else {
            let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("files={files} {}", parts.join(" "))
        };
        Ok(Report::new(summary, files))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(progress: f64, content: &str) -> Torrent {
        serde_json::from_value(
            json!({"hash": "ABC", "name": "x", "progress": progress, "content_path": content,
                                      "save_path": "/downloads"}),
        )
        .unwrap()
    }

    fn rec(outcome: &str) -> TorrentImportRecord {
        TorrentImportRecord {
            at: 0,
            name: "x".into(),
            outcome: outcome.into(),
            detail: String::new(),
            attempts: 1,
        }
    }

    #[test]
    fn candidates_are_complete_and_not_yet_final() {
        assert!(is_candidate(&torrent(1.0, "/downloads/x"), None));
        assert!(!is_candidate(&torrent(0.99, "/downloads/x"), None));
        assert!(!is_candidate(&torrent(1.0, ""), None));
        assert!(!is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("imported"))
        ));
        assert!(!is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("no_match"))
        ));
        assert!(is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("retry"))
        ));
    }

    #[test]
    fn keeps_videos_and_drops_samples() {
        let files: Vec<TorrentFile> = serde_json::from_value(json!([
            {"name": "Show.S01/Show.S01E01.mkv", "size": 1},
            {"name": "Show.S01/Sample/show-sample.mkv", "size": 1},
            {"name": "Show.S01/show.sample.mkv", "size": 1},
            {"name": "Show.S01/Show.nfo", "size": 1},
            {"name": "Movie.2003.mp4", "size": 1},
            {"name": "Samples.Of.Life.2020.mkv", "size": 1}
        ]))
        .unwrap();
        let names: Vec<&str> = video_files(&files)
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Show.S01/Show.S01E01.mkv",
                "Movie.2003.mp4",
                "Samples.Of.Life.2020.mkv"
            ]
        );
    }

    #[test]
    fn parse_falls_back_to_the_first_video_name() {
        let files: Vec<TorrentFile> = serde_json::from_value(json!([
            {"name": "Star Wars Rebels (2014)/Star.Wars.Rebels.S01E02.mkv"},
            {"name": "Star Wars Rebels (2014)/Star.Wars.Rebels.S01E01.mkv"}
        ]))
        .unwrap();
        let videos: Vec<&TorrentFile> = files.iter().collect();
        assert_eq!(
            parse_labels("Star Wars Rebels (2014)", &videos),
            vec!["Star Wars Rebels (2014)", "Star.Wars.Rebels.S01E01.mkv"]
        );
        let single: Vec<TorrentFile> =
            serde_json::from_value(json!([{"name": "Fusion (2003).mkv"}])).unwrap();
        let v: Vec<&TorrentFile> = single.iter().collect();
        assert_eq!(
            parse_labels("Fusion (2003).mkv", &v),
            vec!["Fusion (2003).mkv"]
        );
    }

    #[test]
    fn maps_parsed_episodes_to_ids() {
        let eps = vec![
            json!({"id": 10, "seasonNumber": 1, "episodeNumber": 7, "absoluteEpisodeNumber": 7}),
            json!({"id": 11, "seasonNumber": 1, "episodeNumber": 8, "absoluteEpisodeNumber": 8}),
            json!({"id": 20, "seasonNumber": 2, "episodeNumber": 1, "absoluteEpisodeNumber": 13}),
        ];
        let p = json!({"parsedEpisodeInfo": {"seasonNumber": 1, "episodeNumbers": [7, 8], "absoluteEpisodeNumbers": []}});
        assert_eq!(map_episodes(&p, &eps), vec![10, 11]);
        let anime = json!({"parsedEpisodeInfo": {"seasonNumber": 0, "episodeNumbers": [], "absoluteEpisodeNumbers": [13]}});
        assert_eq!(map_episodes(&anime, &eps), vec![20]);
        assert!(map_episodes(&json!({}), &eps).is_empty());
        let wrong = json!({"parsedEpisodeInfo": {"seasonNumber": 5, "episodeNumbers": [1]}});
        assert!(map_episodes(&wrong, &eps).is_empty());
    }

    #[test]
    fn fresh_items_ignore_identification_rejections_only() {
        let cands = vec![
            json!({"path": "/d/a.mkv", "rejections": [{"reason": "Unknown Movie"}], "quality": {}}),
            json!({"path": "/d/s.mkv", "rejections": [{"reason": "Sample"}, {"reason": "Unknown Movie"}]}),
        ];
        let (files, skipped) = fresh_files(&cands, true, 68, "H", &HashMap::new());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["movieId"], 68);
        assert_eq!(skipped, vec!["/d/s.mkv: Sample".to_string()]);

        let series = vec![
            json!({"path": "/d/e1.mkv", "rejections": [{"reason": "Unknown Series"}]}),
            json!({"path": "/d/e2.mkv", "rejections": []}),
        ];
        let mut by = HashMap::new();
        by.insert("/d/e1.mkv".to_string(), vec![10, 11]);
        let (files, skipped) = fresh_files(&series, false, 5, "H", &by);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["episodeIds"], json!([10, 11]));
        assert_eq!(files[0]["releaseType"], "multiEpisode");
        assert_eq!(skipped.len(), 1);
        assert!(is_identification_rejection(
            "Found matching series via grab history, but release was matched to series by ID"
        ));
        assert!(!is_identification_rejection(
            "Not an upgrade for existing episode file(s)"
        ));
    }

    #[test]
    fn other_side_duplicate_needs_files() {
        assert!(has_files(true, &json!({"hasFile": true})));
        assert!(!has_files(true, &json!({"hasFile": false})));
        assert!(has_files(
            false,
            &json!({"statistics": {"episodeFileCount": 3}})
        ));
        assert!(!has_files(
            false,
            &json!({"statistics": {"episodeFileCount": 0}})
        ));
    }

    #[test]
    fn added_items_are_unmonitored_without_search() {
        let hit = json!({"id": 0, "title": "The Core", "tmdbId": 9341, "monitored": true});
        let b = add_body(true, &hit, "/movies", 6);
        assert_eq!(b["monitored"], false);
        assert_eq!(b["addOptions"]["searchForMovie"], false);
        assert_eq!(b["rootFolderPath"], "/movies");
        assert_eq!(b["qualityProfileId"], 6);
        assert_eq!(b["tmdbId"], 9341);
        assert!(b.get("id").is_none());
        let s = add_body(
            false,
            &json!({"title": "Daybreak (2019)", "tvdbId": 1}),
            "/tv",
            7,
        );
        assert_eq!(s["monitored"], false);
        assert_eq!(s["addOptions"]["monitor"], "none");
        assert_eq!(s["addOptions"]["searchForMissingEpisodes"], false);
    }

    #[test]
    fn errors_retry_then_give_up() {
        assert_eq!(after_error(0, 3), ("retry", 1));
        assert_eq!(after_error(1, 3), ("retry", 2));
        assert_eq!(after_error(2, 3), ("error", 3));
    }

    #[test]
    fn state_keys_are_side_scoped_and_lowercase() {
        assert_eq!(state_key("seedbox", "ABCdef"), "seedbox:abcdef");
    }

    #[test]
    fn maps_torrent_files_to_host_paths() {
        let p = host_path(
            "/downloads",
            Path::new("/opt/homelab/library/downloads"),
            "/downloads/",
            "Show.S01/E01.mkv",
        );
        assert_eq!(
            p.unwrap(),
            Path::new("/opt/homelab/library/downloads/Show.S01/E01.mkv")
        );
        assert!(host_path("/downloads", Path::new("/x"), "/elsewhere", "a.mkv").is_none());
        assert!(host_path("/downloads", Path::new("/x"), "/downloads2", "a.mkv").is_none());
    }
}
