//! Nettoyage après une suppression dans Jellyfin (toutes les 5 min, VPS et seedbox).
//!
//! Jellyfin n'efface que le dossier ou le fichier. Sans suite, Radarr/Sonarr re-téléchargeraient le
//! titre (toujours surveillé) et le torrent garderait les données (lien physique). Un fichier connu
//! d'un Arr est tenu pour supprimé seulement si trois signaux concordent :
//! 1. il n'existe plus sur le disque (NotFound, pas une erreur d'E/S) ;
//! 2. Jellyfin n'a plus d'élément à ce chemin ;
//! 3. il était déjà absent au passage précédent (`confirm_after_secs`).
//!
//! Garde-fous : fichiers importés depuis moins de `min_file_age_mins` ignorés, montage seedbox
//! vérifié et cache rclone rafraîchi avant de conclure, abandon si trop de titres manquent d'un coup,
//! `max_titles_per_run` titres au plus. Nettoyage :
//! - film : fiche Radarr supprimée (`deleteFiles`), média Jellyseerr libéré ;
//! - série entière : idem côté Sonarr ;
//! - saison entière : épisodes non surveillés, saison notée pour `monitor_sync` ;
//! - épisodes : non surveillés ;
//! - torrents d'origine retirés avec leurs fichiers s'ils ne servent plus à rien d'autre ; C411 (ou
//!   tracker inconnu) seulement une fois `c411_min_ratio` ou `c411_min_seed_days` atteint.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::Torrent;
use crate::config::{Config, DeletionCleanup as Cfg};
use crate::context::{Side, TaskContext};
use crate::state::{now, PendingTorrent};

pub struct DeletionCleanup;

/// (racine vue par l'Arr, même dossier sur l'hôte, même dossier vu par Jellyfin).
pub type PathMap = (String, PathBuf, String);

/// Chemin Arr → (chemin sur l'hôte, chemin Jellyfin).
pub fn map_path(maps: &[PathMap], arr_path: &str) -> Option<(PathBuf, String)> {
    maps.iter().find_map(|(arr, host, jf)| {
        let rest = arr_path.strip_prefix(arr.trim_end_matches('/'))?;
        (rest.is_empty() || rest.starts_with('/')).then(|| {
            (
                PathBuf::from(format!("{}{rest}", host.display())),
                format!("{}{rest}", jf.trim_end_matches('/')),
            )
        })
    })
}

/// Fichier importé depuis au moins `min_age_mins` (date ISO 8601 de l'Arr).
pub fn old_enough(date_added: Option<&str>, now: i64, min_age_mins: i64) -> bool {
    date_added
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| now - d.timestamp() >= min_age_mins * 60)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentAction {
    DeleteNow,
    Wait,
}

/// C411 (ou tracker inconnu, par prudence) : seuil de seed ; autres trackers : tout de suite.
pub fn torrent_action(t: &Torrent, unlimited: &[String], cfg: &Cfg) -> TorrentAction {
    let tracker = t.tracker.to_lowercase();
    let private = tracker.is_empty()
        || unlimited
            .iter()
            .any(|u| tracker.contains(&u.to_lowercase()));
    if !private
        || t.ratio >= cfg.c411_min_ratio
        || t.seeding_time >= cfg.c411_min_seed_days * 86_400
    {
        TorrentAction::DeleteNow
    } else {
        TorrentAction::Wait
    }
}

/// Le torrent `name` est-il la source de ce fichier ? (`originalFilePath` ou `sceneName` de l'Arr)
pub fn torrent_matches(
    torrent_name: &str,
    original_path: Option<&str>,
    scene: Option<&str>,
) -> bool {
    let name = torrent_name.trim();
    if name.len() < 8 {
        return false; // trop court pour être un nom de release
    }
    if scene.map(|s| s.trim() == name).unwrap_or(false) {
        return true;
    }
    original_path
        .map(|p| {
            p.split('/')
                .any(|seg| seg == name || seg.rsplit_once('.').map(|(stem, _)| stem) == Some(name))
        })
        .unwrap_or(false)
}

/// Hashes (minuscules) cités par l'historique d'un titre. Défensif : un évènement d'un autre titre
/// (`key` ≠ `id`) ou d'un autre épisode (séries) n'est jamais retenu.
pub fn hashes_from_history(
    records: &[Value],
    key: &str,
    id: i64,
    episodes: Option<&BTreeSet<i64>>,
) -> BTreeSet<String> {
    records
        .iter()
        .filter(|r| r.get(key).and_then(Value::as_i64) == Some(id))
        .filter(|r| match episodes {
            Some(eps) => r
                .get("episodeId")
                .and_then(Value::as_i64)
                .map(|e| eps.contains(&e))
                .unwrap_or(false),
            None => true,
        })
        .filter_map(|r| r.get("downloadId").and_then(Value::as_str))
        .map(str::to_lowercase)
        .collect()
}

/// Ce qu'une suppression de fichiers d'épisodes représente pour la série.
#[derive(Debug, Default, PartialEq)]
pub struct SeriesPlan {
    pub whole: bool,
    /// Saisons dont tous les fichiers ont disparu.
    pub seasons: Vec<i64>,
    /// Épisodes dont le fichier a disparu (à ne plus surveiller).
    pub episodes: Vec<i64>,
}

pub fn plan_series(missing: &BTreeSet<i64>, episodes: &[Value], total_files: usize) -> SeriesPlan {
    let mut by_season: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let mut affected = Vec::new();
    for e in episodes {
        let file = e.get("episodeFileId").and_then(Value::as_i64).unwrap_or(0);
        if file <= 0 {
            continue;
        }
        let season = e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(-1);
        by_season.entry(season).or_default().insert(file);
        if missing.contains(&file) {
            if let Some(id) = e.get("id").and_then(Value::as_i64) {
                affected.push(id);
            }
        }
    }
    SeriesPlan {
        whole: total_files > 0 && missing.len() >= total_files,
        seasons: by_season
            .into_iter()
            .filter(|(_, files)| !files.is_empty() && files.is_subset(missing))
            .map(|(s, _)| s)
            .collect(),
        episodes: affected,
    }
}

/// Un fichier de titre, tel que l'Arr le connaît.
#[derive(Debug, Clone)]
struct FileRef {
    id: i64,
    key: String,
    host: PathBuf,
    jellyfin: String,
    original: Option<String>,
    scene: Option<String>,
}

#[derive(Debug, Clone)]
enum Kind {
    Movie,
    Series { tvdb: i64, total_files: usize },
}

#[derive(Debug, Clone)]
struct Title {
    kind: Kind,
    id: i64,
    tmdb: i64,
    name: String,
    files: Vec<FileRef>,
}

/// Fiche déplacée par `anime_library` depuis moins de `MOVE_GRACE_SECS`.
fn recently_moved(moves: &BTreeMap<String, i64>, side: &str, t: &Title, now: i64) -> bool {
    let kind = match t.kind {
        Kind::Movie => "movie",
        Kind::Series { .. } => "series",
    };
    moves
        .get(&format!("{side}:{kind}:{}", t.id))
        .is_some_and(|at| now - at < super::anime_library::MOVE_GRACE_SECS)
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn side_maps(ctx: &TaskContext, side: &str) -> Vec<PathMap> {
    if side == "vps" {
        ctx.cfg
            .tasks
            .deletion_cleanup
            .vps_paths
            .iter()
            .map(|[a, h, j]| (a.clone(), PathBuf::from(h), j.clone()))
            .collect()
    } else {
        let sb = &ctx.cfg.seedbox;
        vec![(
            sb.media_root.clone(),
            sb.mount_point.clone(),
            sb.jellyfin_root.clone(),
        )]
    }
}

/// Fichiers absents (NotFound seulement), vérifiés hors du runtime et bornés dans le temps.
async fn missing_among(paths: Vec<(String, PathBuf)>) -> Option<BTreeSet<String>> {
    let job = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter(|(_, p)| {
                matches!(std::fs::metadata(p), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            })
            .map(|(k, _)| k)
            .collect::<BTreeSet<_>>()
    });
    tokio::time::timeout(Duration::from_secs(180), job)
        .await
        .ok()
        .and_then(|r| r.ok())
}

/// Montage seedbox utilisable : quota écrit par la seedbox lisible et dossier des films listable.
async fn seedbox_mount_ok(ctx: &TaskContext) -> bool {
    let mp = ctx.cfg.seedbox.mount_point.clone();
    let job = tokio::task::spawn_blocking(move || {
        mp.join(".homelab/quota.json").is_file()
            && std::fs::read_dir(mp.join("Movies"))
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
    });
    tokio::time::timeout(Duration::from_secs(20), job)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false)
}

/// Invalide le cache de répertoires rclone pour les dossiers parents d'un fichier (relatifs au montage).
pub(crate) async fn rclone_refresh(ctx: &TaskContext, host: &std::path::Path) {
    let mp = &ctx.cfg.seedbox.mount_point;
    let Ok(rel) = host.strip_prefix(mp) else {
        return;
    };
    let rc = format!(
        "{}/vfs/refresh",
        ctx.cfg.seedbox.rclone_rc.trim_end_matches('/')
    );
    let mut dir = PathBuf::new();
    let parents: Vec<_> = rel
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .collect();
    for c in parents {
        dir.push(c);
        let _ = ctx
            .http
            .post(&rc)
            .json(&json!({ "dir": dir.to_string_lossy() }))
            .timeout(Duration::from_secs(20))
            .send()
            .await;
    }
}

/// Fiches média Jellyseerr **en attente (2) ou en cours (3)** sans demande : l'admin a retiré la demande,
/// le titre n'est plus voulu. Renvoie (`movie`/`tv`, identifiant TMDB, id du média).
///
/// L'état compte : un scan Jellyfin crée une fiche média pour **tout** ce qui est déjà dans la
/// bibliothèque (237 médias pour 117 demandes le 2026-09-17). Ces fiches-là sont « disponible » (5) ou
/// « partiel » (4) et ne doivent jamais être touchées, sous peine d'effacer la médiathèque.
pub fn dropped_media(media: &[Value], requests: &[Value]) -> Vec<(String, i64, i64)> {
    let kept: BTreeSet<i64> = requests
        .iter()
        .filter_map(|r| r.pointer("/media/id").and_then(Value::as_i64))
        .collect();
    media
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_i64)?;
            let tmdb = m.get("tmdbId").and_then(Value::as_i64).filter(|t| *t > 0)?;
            let kind = m.get("mediaType").and_then(Value::as_str)?;
            let status = m.get("status").and_then(Value::as_i64).unwrap_or(0);
            (!kept.contains(&id) && matches!(status, 2 | 3)).then(|| (kind.to_string(), tmdb, id))
        })
        .collect()
}

/// Supprime une fiche dont la demande a été retirée : fiche Arr **et fichiers**, torrents devenus inutiles,
/// puis la fiche média Jellyseerr (le titre redevient demandable).
async fn drop_title(
    ctx: &TaskContext,
    side: &Side<'_>,
    t: &Title,
    media_id: i64,
) -> Result<String> {
    let torrents = side.qbit.torrents().await.unwrap_or_default();
    let hashes = source_hashes(side, t, None, &torrents).await;
    let (arr, path, params) = match t.kind {
        Kind::Movie => (
            side.radarr,
            format!("api/v3/movie/{}", t.id),
            [("deleteFiles", "true"), ("addImportExclusion", "false")],
        ),
        Kind::Series { .. } => (
            side.sonarr,
            format!("api/v3/series/{}", t.id),
            [("deleteFiles", "true"), ("addImportListExclusion", "false")],
        ),
    };
    if ctx.dry_run {
        info!(task = "deletion_cleanup", side = side.name, title = %t.name, files = t.files.len(), "dry-run: would drop (request removed)");
        return Ok(format!("{} « {} » (demande retirée)", side.name, t.name));
    }
    arr.delete(&path, &params).await?;
    info!(task = "deletion_cleanup", side = side.name, title = %t.name, files = t.files.len(), "dropped: request removed in jellyseerr");
    let mut notes = Vec::new();
    for h in &hashes {
        if let Some(tor) = torrents.iter().find(|x| x.hash.eq_ignore_ascii_case(h)) {
            notes.push(handle_torrent(ctx, side, tor).await);
        }
    }
    if let Err(e) = ctx.jellyseerr.delete_media(media_id).await {
        warn!(task = "deletion_cleanup", title = %t.name, error = %e, "jellyseerr media not released");
    }
    Ok(format!(
        "{} « {} » (demande retirée{})",
        side.name,
        t.name,
        if notes.is_empty() {
            String::new()
        } else {
            format!(", {}", notes.join(" "))
        }
    ))
}

/// Fiches de cette machine correspondant à des demandes retirées, avec tous leurs fichiers.
async fn dropped_titles(
    ctx: &TaskContext,
    side: &Side<'_>,
    dropped: &[(String, i64, i64)],
) -> Result<Vec<(Title, i64)>> {
    let maps = side_maps(ctx, side.name);
    let mut out = Vec::new();
    for (kind, tmdb, media_id) in dropped {
        if kind == "movie" {
            let Some(m) = side
                .radarr
                .movies()
                .await?
                .into_iter()
                .find(|m| m.get("tmdbId").and_then(Value::as_i64) == Some(*tmdb))
            else {
                continue;
            };
            let file = m.get("movieFile");
            let files = file
                .and_then(|f| str_of(f, "path"))
                .and_then(|path| {
                    map_path(&maps, &path).map(|(host, jf)| (f_id(file), path, host, jf))
                })
                .map(|(id, path, host, jellyfin)| {
                    vec![FileRef {
                        id,
                        key: format!("{}:{path}", side.name),
                        host,
                        jellyfin,
                        original: file.and_then(|f| str_of(f, "originalFilePath")),
                        scene: file.and_then(|f| str_of(f, "sceneName")),
                    }]
                })
                .unwrap_or_default();
            out.push((
                Title {
                    kind: Kind::Movie,
                    id: m.get("id").and_then(Value::as_i64).unwrap_or(0),
                    tmdb: *tmdb,
                    name: str_of(&m, "title").unwrap_or_default(),
                    files,
                },
                *media_id,
            ));
        } else {
            let Some(sr) = side
                .sonarr
                .series()
                .await?
                .into_iter()
                .find(|s| s.get("tmdbId").and_then(Value::as_i64) == Some(*tmdb))
            else {
                continue;
            };
            let id = sr.get("id").and_then(Value::as_i64).unwrap_or(0);
            let files: Vec<FileRef> = side
                .sonarr
                .episode_files(id)
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|f| {
                    let path = str_of(f, "path")?;
                    let (host, jellyfin) = map_path(&maps, &path)?;
                    Some(FileRef {
                        id: f.get("id").and_then(Value::as_i64)?,
                        key: format!("{}:{path}", side.name),
                        host,
                        jellyfin,
                        original: str_of(f, "originalFilePath"),
                        scene: str_of(f, "sceneName"),
                    })
                })
                .collect();
            out.push((
                Title {
                    kind: Kind::Series {
                        tvdb: sr.get("tvdbId").and_then(Value::as_i64).unwrap_or(0),
                        total_files: files.len(),
                    },
                    id,
                    tmdb: *tmdb,
                    name: str_of(&sr, "title").unwrap_or_default(),
                    files,
                },
                *media_id,
            ));
        }
    }
    Ok(out)
}

fn f_id(file: Option<&Value>) -> i64 {
    file.and_then(|f| f.get("id").and_then(Value::as_i64))
        .unwrap_or(0)
}

/// Titres de cette machine dont au moins un fichier a disparu du disque.
async fn titles_with_missing_files(
    ctx: &TaskContext,
    side: &Side<'_>,
    now: i64,
) -> Result<Option<Vec<Title>>> {
    let cfg = &ctx.cfg.tasks.deletion_cleanup;
    let maps = side_maps(ctx, side.name);
    let mut titles: Vec<Title> = Vec::new();
    for m in side.radarr.movies().await? {
        let Some(f) = m.get("movieFile") else {
            continue;
        };
        let (Some(id), Some(path)) = (m.get("id").and_then(Value::as_i64), str_of(f, "path"))
        else {
            continue;
        };
        if !old_enough(
            f.get("dateAdded").and_then(Value::as_str),
            now,
            cfg.min_file_age_mins,
        ) {
            continue;
        }
        let Some((host, jellyfin)) = map_path(&maps, &path) else {
            continue;
        };
        titles.push(Title {
            kind: Kind::Movie,
            id,
            tmdb: m.get("tmdbId").and_then(Value::as_i64).unwrap_or(0),
            name: str_of(&m, "title").unwrap_or_default(),
            files: vec![FileRef {
                id: f.get("id").and_then(Value::as_i64).unwrap_or(0),
                key: format!("{}:{path}", side.name),
                host,
                jellyfin,
                original: str_of(f, "originalFilePath"),
                scene: str_of(f, "sceneName"),
            }],
        });
    }
    for s in side.sonarr.series().await? {
        let count = s
            .pointer("/statistics/episodeFileCount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(id) = s.get("id").and_then(Value::as_i64) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        // une nouvelle tentative : le Sonarr de la seedbox passe par un proxy HTTPS parfois lent
        let all = match side.sonarr.episode_files(id).await {
            Ok(v) => v,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                side.sonarr.episode_files(id).await?
            }
        };
        let files: Vec<FileRef> = all
            .iter()
            .filter(|f| {
                old_enough(
                    f.get("dateAdded").and_then(Value::as_str),
                    now,
                    cfg.min_file_age_mins,
                )
            })
            .filter_map(|f| {
                let path = str_of(f, "path")?;
                let (host, jellyfin) = map_path(&maps, &path)?;
                Some(FileRef {
                    id: f.get("id").and_then(Value::as_i64)?,
                    key: format!("{}:{path}", side.name),
                    host,
                    jellyfin,
                    original: str_of(f, "originalFilePath"),
                    scene: str_of(f, "sceneName"),
                })
            })
            .collect();
        titles.push(Title {
            kind: Kind::Series {
                tvdb: s.get("tvdbId").and_then(Value::as_i64).unwrap_or(0),
                total_files: all.len(),
            },
            id,
            tmdb: s.get("tmdbId").and_then(Value::as_i64).unwrap_or(0),
            name: str_of(&s, "title").unwrap_or_default(),
            files,
        });
    }
    let probe: Vec<(String, PathBuf)> = titles
        .iter()
        .flat_map(|t| t.files.iter().map(|f| (f.key.clone(), f.host.clone())))
        .collect();
    let Some(mut missing) = missing_among(probe).await else {
        warn!(
            task = "deletion_cleanup",
            side = side.name,
            "disk check timed out"
        );
        return Ok(None);
    };
    if side.name == "seedbox" && !missing.is_empty() {
        // le cache rclone peut ignorer un fichier fraîchement importé : on rafraîchit, puis on revérifie
        let recheck: Vec<(String, PathBuf)> = titles
            .iter()
            .flat_map(|t| t.files.iter())
            .filter(|f| missing.contains(&f.key))
            .map(|f| (f.key.clone(), f.host.clone()))
            .collect();
        for (_, p) in &recheck {
            rclone_refresh(ctx, p).await;
        }
        missing = missing_among(recheck).await.unwrap_or_default();
    }
    for t in titles.iter_mut() {
        t.files.retain(|f| missing.contains(&f.key));
    }
    titles.retain(|t| !t.files.is_empty());
    Ok(Some(titles))
}

/// Hashes (minuscules) des torrents d'où viennent ces fichiers. Séries : seulement les évènements
/// des épisodes concernés (`episodes`).
async fn source_hashes(
    side: &Side<'_>,
    t: &Title,
    episodes: Option<&BTreeSet<i64>>,
    torrents: &[Torrent],
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let (arr, key) = match t.kind {
        Kind::Movie => (side.radarr, "movieId"),
        Kind::Series { .. } => (side.sonarr, "seriesId"),
    };
    match arr.title_history(t.id).await {
        Ok(records) => out.extend(hashes_from_history(&records, key, t.id, episodes)),
        Err(e) => {
            warn!(task = "deletion_cleanup", title = %t.name, error = %e, "history unreadable")
        }
    }
    for tor in torrents {
        if t.files
            .iter()
            .any(|f| torrent_matches(&tor.name, f.original.as_deref(), f.scene.as_deref()))
        {
            out.insert(tor.hash.to_lowercase());
        }
    }
    out
}

/// Le torrent sert-il encore à un fichier de la bibliothèque (pack) ?
async fn still_used(
    side: &Side<'_>,
    t: &Title,
    hash: &str,
    exclude_episodes: &BTreeSet<i64>,
) -> bool {
    // VPS : un fichier du torrent encore lié ailleurs (lien physique vers la bibliothèque)
    if let Some((container, host_dir)) = &side.local_downloads {
        if let Ok(files) = side.qbit.files(hash).await {
            let torrent = side
                .qbit
                .torrents()
                .await
                .ok()
                .and_then(|l| l.into_iter().find(|x| x.hash.eq_ignore_ascii_case(hash)));
            if let Some(tor) = torrent {
                let linked = files.iter().any(|f| {
                    let p = format!("{}/{}", tor.save_path.trim_end_matches('/'), f.name);
                    let Some(rest) = p.strip_prefix(container.trim_end_matches('/')) else {
                        return false;
                    };
                    use std::os::unix::fs::MetadataExt;
                    std::fs::metadata(format!("{}{rest}", host_dir.display()))
                        .map(|m| m.nlink() > 1)
                        .unwrap_or(false)
                });
                if linked {
                    return true;
                }
            }
        }
    }
    let arr = match t.kind {
        Kind::Movie => side.radarr,
        Kind::Series { .. } => side.sonarr,
    };
    let Ok(records) = arr.history_where("downloadId", &hash.to_uppercase()).await else {
        return true; // doute : on garde
    };
    for r in records {
        let same = r
            .get("downloadId")
            .and_then(Value::as_str)
            .map(|d| d.eq_ignore_ascii_case(hash))
            .unwrap_or(false);
        if !same || r.get("eventType").and_then(Value::as_str) != Some("downloadFolderImported") {
            continue;
        }
        match t.kind {
            Kind::Movie => {
                let Some(mid) = r.get("movieId").and_then(Value::as_i64) else {
                    continue;
                };
                if mid == t.id {
                    continue;
                }
                if let Ok(m) = arr.get(&format!("api/v3/movie/{mid}"), &[]).await {
                    if m.get("hasFile").and_then(Value::as_bool).unwrap_or(false) {
                        return true;
                    }
                }
            }
            Kind::Series { .. } => {
                let Some(eid) = r.get("episodeId").and_then(Value::as_i64) else {
                    continue;
                };
                if exclude_episodes.contains(&eid) {
                    continue;
                }
                if let Ok(e) = arr.get(&format!("api/v3/episode/{eid}"), &[]).await {
                    if e.get("hasFile").and_then(Value::as_bool).unwrap_or(false) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Retire le torrent maintenant ou le met en attente (C411). Renvoie une étiquette pour le résumé.
async fn handle_torrent(ctx: &TaskContext, side: &Side<'_>, tor: &Torrent) -> &'static str {
    let dc = &ctx.cfg.tasks.deletion_cleanup;
    let unlimited = &ctx.cfg.tasks.tracker_ratio.unlimited;
    let key = format!("{}:{}", side.name, tor.hash.to_lowercase());
    match torrent_action(tor, unlimited, dc) {
        TorrentAction::DeleteNow => {
            if ctx.dry_run {
                info!(task = "deletion_cleanup", side = side.name, torrent = %tor.name, "dry-run: would delete torrent and files");
                return "torrent_deleted";
            }
            let _lock = ctx.qbit_lock.lock().await;
            match side
                .qbit
                .delete(std::slice::from_ref(&tor.hash), true)
                .await
            {
                Ok(()) => {
                    info!(task = "deletion_cleanup", side = side.name, torrent = %tor.name, ratio = tor.ratio, "torrent deleted with files");
                    let _ = ctx
                        .state
                        .update(|s| {
                            s.deletions.pending_torrents.remove(&key);
                        })
                        .await;
                    "torrent_deleted"
                }
                Err(e) => {
                    warn!(task = "deletion_cleanup", side = side.name, torrent = %tor.name, error = %e, "torrent delete failed");
                    "torrent_error"
                }
            }
        }
        TorrentAction::Wait => {
            if !ctx.dry_run {
                let _ = ctx
                    .state
                    .update(|s| {
                        s.deletions
                            .pending_torrents
                            .entry(key)
                            .or_insert(PendingTorrent {
                                at: now(),
                                name: tor.name.clone(),
                            });
                    })
                    .await;
            }
            info!(task = "deletion_cleanup", side = side.name, torrent = %tor.name, ratio = tor.ratio, seeding_days = tor.seeding_time / 86_400, "private tracker: torrent kept until seed threshold");
            "torrent_pending"
        }
    }
}

/// Le titre a-t-il encore des fichiers sur une autre machine ? (Jellyseerr n'est libéré que sinon)
async fn present_elsewhere(ctx: &TaskContext, here: &str, t: &Title) -> bool {
    for other in ctx.sides().into_iter().filter(|s| s.name != here) {
        let found = match t.kind {
            Kind::Movie => other.radarr.movies().await.map(|l| {
                l.iter().any(|m| {
                    m.get("tmdbId").and_then(Value::as_i64) == Some(t.tmdb)
                        && m.get("hasFile").and_then(Value::as_bool).unwrap_or(false)
                })
            }),
            Kind::Series { tvdb, .. } => other.sonarr.series().await.map(|l| {
                l.iter().any(|s| {
                    s.get("tvdbId").and_then(Value::as_i64) == Some(tvdb)
                        && s.pointer("/statistics/episodeFileCount")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                            > 0
                })
            }),
        };
        if found.unwrap_or(true) {
            return true; // en cas d'erreur : on considère présent (on ne libère pas)
        }
    }
    false
}

async fn release_jellyseerr(ctx: &TaskContext, side: &Side<'_>, t: &Title) {
    if t.tmdb <= 0 || present_elsewhere(ctx, side.name, t).await {
        return;
    }
    let kind = match t.kind {
        Kind::Movie => "movie",
        Kind::Series { .. } => "tv",
    };
    match ctx.jellyseerr.media_id(kind, t.tmdb).await {
        Ok(Some(mid)) => {
            if ctx.dry_run {
                info!(task = "deletion_cleanup", title = %t.name, "dry-run: would release jellyseerr media");
            } else if let Err(e) = ctx.jellyseerr.delete_media(mid).await {
                warn!(task = "deletion_cleanup", title = %t.name, error = %e, "jellyseerr media not released");
            } else {
                info!(task = "deletion_cleanup", title = %t.name, "jellyseerr media released");
            }
        }
        Ok(None) => {}
        Err(e) => {
            warn!(task = "deletion_cleanup", title = %t.name, error = %e, "jellyseerr lookup failed")
        }
    }
}

/// Nettoie un titre confirmé ; renvoie un résumé court.
async fn clean_title(ctx: &TaskContext, side: &Side<'_>, t: &Title) -> Result<String> {
    let torrents = side.qbit.torrents().await.unwrap_or_default();
    let dry = ctx.dry_run;
    let mut exclude_episodes = BTreeSet::new();
    let hashes: BTreeSet<String>;
    let what = match t.kind {
        Kind::Movie => {
            // relevé avant la suppression : l'historique de la fiche part avec elle
            hashes = source_hashes(side, t, None, &torrents).await;
            if dry {
                info!(task = "deletion_cleanup", side = side.name, title = %t.name, "dry-run: would delete radarr movie");
            } else {
                side.radarr
                    .delete(
                        &format!("api/v3/movie/{}", t.id),
                        &[("deleteFiles", "true"), ("addImportExclusion", "false")],
                    )
                    .await?;
                info!(task = "deletion_cleanup", side = side.name, title = %t.name, "radarr movie deleted");
            }
            release_jellyseerr(ctx, side, t).await;
            "film".to_string()
        }
        Kind::Series { tvdb, total_files } => {
            let episodes = side.sonarr.episodes(t.id).await?;
            let missing: BTreeSet<i64> = t.files.iter().map(|f| f.id).collect();
            let plan = plan_series(&missing, &episodes, total_files);
            exclude_episodes.extend(plan.episodes.iter().copied());
            hashes = source_hashes(side, t, Some(&exclude_episodes), &torrents).await;
            if plan.whole {
                if dry {
                    info!(task = "deletion_cleanup", side = side.name, title = %t.name, "dry-run: would delete sonarr series");
                } else {
                    side.sonarr
                        .delete(
                            &format!("api/v3/series/{}", t.id),
                            &[("deleteFiles", "true"), ("addImportListExclusion", "false")],
                        )
                        .await?;
                    info!(task = "deletion_cleanup", side = side.name, title = %t.name, "sonarr series deleted");
                }
                release_jellyseerr(ctx, side, t).await;
                "série".to_string()
            } else {
                if dry {
                    info!(task = "deletion_cleanup", side = side.name, title = %t.name, seasons = ?plan.seasons, episodes = plan.episodes.len(), "dry-run: would unmonitor");
                } else {
                    if !plan.episodes.is_empty() {
                        side.sonarr
                            .set_episodes_monitored(&plan.episodes, false)
                            .await?;
                    }
                    if !plan.seasons.is_empty() {
                        let mut series = side
                            .sonarr
                            .get(&format!("api/v3/series/{}", t.id), &[])
                            .await?;
                        if let Some(list) = series.get_mut("seasons").and_then(Value::as_array_mut)
                        {
                            for s in list.iter_mut() {
                                let n = s.get("seasonNumber").and_then(Value::as_i64).unwrap_or(-1);
                                if plan.seasons.contains(&n) {
                                    s["monitored"] = Value::Bool(false);
                                }
                            }
                        }
                        side.sonarr.put_series(t.id, &series).await?;
                        let at = now();
                        ctx.state
                            .update(|s| {
                                for n in &plan.seasons {
                                    s.deletions
                                        .seasons
                                        .insert(format!("{}:{tvdb}:{n}", side.name), at);
                                }
                            })
                            .await?;
                    }
                    info!(task = "deletion_cleanup", side = side.name, title = %t.name, seasons = ?plan.seasons, episodes = plan.episodes.len(), "episodes unmonitored");
                }
                if plan.seasons.is_empty() {
                    format!("{} épisode(s)", plan.episodes.len())
                } else {
                    format!("saison(s) {:?}", plan.seasons)
                }
            }
        }
    };
    let mut torrent_notes = Vec::new();
    for h in &hashes {
        let Some(tor) = torrents.iter().find(|x| x.hash.eq_ignore_ascii_case(h)) else {
            continue;
        };
        if still_used(side, t, h, &exclude_episodes).await {
            info!(task = "deletion_cleanup", side = side.name, torrent = %tor.name, "torrent still used by other library files: kept");
            torrent_notes.push("torrent_kept");
            continue;
        }
        torrent_notes.push(handle_torrent(ctx, side, tor).await);
    }
    Ok(format!(
        "{} « {} » ({}{})",
        side.name,
        t.name,
        what,
        if torrent_notes.is_empty() {
            String::new()
        } else {
            format!(", {}", torrent_notes.join(","))
        }
    ))
}

/// Torrents C411 en attente : retirés quand le seuil de seed est atteint.
async fn process_pending(ctx: &TaskContext, side: &Side<'_>) -> u32 {
    let prefix = format!("{}:", side.name);
    let pending: Vec<String> = ctx
        .state
        .read(|s| {
            s.deletions
                .pending_torrents
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect()
        })
        .await;
    if pending.is_empty() {
        return 0;
    }
    let Ok(torrents) = side.qbit.torrents().await else {
        return 0;
    };
    let mut done = 0;
    for key in pending {
        let hash = &key[prefix.len()..];
        match torrents.iter().find(|t| t.hash.eq_ignore_ascii_case(hash)) {
            None => {
                if !ctx.dry_run {
                    let _ = ctx
                        .state
                        .update(|s| {
                            s.deletions.pending_torrents.remove(&key);
                        })
                        .await;
                }
            }
            Some(t) => {
                let unlimited = &ctx.cfg.tasks.tracker_ratio.unlimited;
                if torrent_action(t, unlimited, &ctx.cfg.tasks.deletion_cleanup)
                    == TorrentAction::DeleteNow
                    && handle_torrent(ctx, side, t).await == "torrent_deleted"
                {
                    done += 1;
                }
            }
        }
    }
    done
}

#[async_trait]
impl Task for DeletionCleanup {
    fn name(&self) -> &'static str {
        "deletion_cleanup"
    }

    fn label(&self) -> &'static str {
        "Suppressions Jellyfin"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.deletion_cleanup.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.deletion_cleanup;
        let now = now();
        let mut summary = Vec::new();
        let mut actions = 0u32;
        let mut jellyfin_paths: Option<HashSet<String>> = None;
        for side in ctx.sides() {
            actions += process_pending(ctx, &side).await;
            if side.name == "seedbox" && !seedbox_mount_ok(ctx).await {
                summary.push("seedbox=montage_indisponible".to_string());
                continue;
            }
            // une machine injoignable n'empêche pas de traiter l'autre (et on ne conclut rien sur elle)
            let titles = match titles_with_missing_files(ctx, &side, now).await {
                Ok(Some(t)) => t,
                Ok(None) => {
                    summary.push(format!("{}=verif_disque_trop_longue", side.name));
                    continue;
                }
                Err(e) => {
                    warn!(task = "deletion_cleanup", side = side.name, error = %e, "arr unreachable: side skipped this run");
                    summary.push(format!("{}=arr_injoignable", side.name));
                    continue;
                }
            };
            // fiches rangées par anime_library il y a peu : un déplacement n'est pas une suppression
            let moves = ctx.state.read(|s| s.anime_moves.clone()).await;
            let titles: Vec<Title> = titles
                .into_iter()
                .filter(|t| !recently_moved(&moves, side.name, t, now))
                .collect();
            let prefix = format!("{}:", side.name);
            let keys: BTreeSet<String> = titles
                .iter()
                .flat_map(|t| t.files.iter().map(|f| f.key.clone()))
                .collect();
            if titles.len() > cfg.abort_if_missing_titles_over {
                warn!(
                    task = "deletion_cleanup",
                    side = side.name,
                    titles = titles.len(),
                    "too many titles missing at once: nothing done (disk or mount issue?)"
                );
                summary.push(format!(
                    "{}=abandon({} titres manquants)",
                    side.name,
                    titles.len()
                ));
                continue;
            }
            // première absence constatée ; les fichiers réapparus sont oubliés
            let first_seen: BTreeMap<String, i64> = if ctx.dry_run {
                ctx.state.read(|s| s.deletions.first_seen.clone()).await
            } else {
                ctx.state
                    .update(|s| {
                        s.deletions
                            .first_seen
                            .retain(|k, _| !k.starts_with(&prefix) || keys.contains(k));
                        for k in &keys {
                            s.deletions.first_seen.entry(k.clone()).or_insert(now);
                        }
                        s.deletions.first_seen.clone()
                    })
                    .await?
            };
            let confirmed: Vec<&Title> = titles
                .iter()
                .filter(|t| {
                    ctx.dry_run
                        || t.files.iter().all(|f| {
                            first_seen
                                .get(&f.key)
                                .map(|at| now - at >= cfg.confirm_after_secs)
                                .unwrap_or(false)
                        })
                })
                .collect();
            if confirmed.is_empty() {
                summary.push(format!("{}=0 ({} en observation)", side.name, titles.len()));
                continue;
            }
            if jellyfin_paths.is_none() {
                jellyfin_paths = Some(ctx.jellyfin.item_paths().await?);
            }
            let jf = jellyfin_paths.as_ref().expect("chargé juste avant");
            let mut done = Vec::new();
            for t in confirmed
                .into_iter()
                .filter(|t| t.files.iter().all(|f| !jf.contains(&f.jellyfin)))
                .take(cfg.max_titles_per_run)
            {
                match clean_title(ctx, &side, t).await {
                    Ok(s) => {
                        actions += 1;
                        done.push(s);
                        if !ctx.dry_run {
                            let ks: Vec<String> = t.files.iter().map(|f| f.key.clone()).collect();
                            let _ = ctx
                                .state
                                .update(|s| {
                                    for k in &ks {
                                        s.deletions.first_seen.remove(k);
                                    }
                                })
                                .await;
                        }
                    }
                    Err(e) => {
                        warn!(task = "deletion_cleanup", side = side.name, title = %t.name, error = %e, "cleanup failed");
                        done.push(format!("{} « {} » (erreur)", side.name, t.name));
                    }
                }
            }
            if done.is_empty() {
                summary.push(format!("{}=0 (encore dans Jellyfin)", side.name));
            } else {
                summary.push(done.join(" ; "));
            }
        }
        // demandes retirées dans Jellyseerr : la fiche Arr et ses fichiers partent avec elles
        match dropped_now(ctx).await {
            Err(e) => {
                warn!(task = "deletion_cleanup", error = %e, "jellyseerr unreachable: dropped requests skipped");
                summary.push("demandes=jellyseerr_injoignable".into());
            }
            Ok(dropped) if dropped.is_empty() => {}
            Ok(dropped) => {
                if dropped.len() > cfg.abort_if_missing_titles_over {
                    warn!(
                        task = "deletion_cleanup",
                        titles = dropped.len(),
                        "too many dropped requests at once: nothing done"
                    );
                    summary.push(format!("demandes=abandon({} d'un coup)", dropped.len()));
                } else {
                    let playing = ctx.jellyfin.playing_paths().await.unwrap_or_default();
                    let mut done = Vec::new();
                    for side in ctx.sides() {
                        let titles = match dropped_titles(ctx, &side, &dropped).await {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(task = "deletion_cleanup", side = side.name, error = %e, "arr unreachable: dropped requests skipped");
                                continue;
                            }
                        };
                        for (t, media_id) in titles.into_iter().take(cfg.max_titles_per_run) {
                            if t.files
                                .iter()
                                .any(|f| playing.iter().any(|p| p.starts_with(&f.jellyfin)))
                            {
                                info!(task = "deletion_cleanup", side = side.name, title = %t.name, "playing: kept for now");
                                continue;
                            }
                            match drop_title(ctx, &side, &t, media_id).await {
                                Ok(s) => {
                                    actions += 1;
                                    done.push(s);
                                }
                                Err(e) => {
                                    warn!(task = "deletion_cleanup", side = side.name, title = %t.name, error = %e, "drop failed");
                                    done.push(format!("{} « {} » (erreur)", side.name, t.name));
                                }
                            }
                        }
                    }
                    if !done.is_empty() {
                        summary.push(done.join(" ; "));
                    }
                }
            }
        }
        Ok(Report::new(summary.join(" | "), actions))
    }
}

/// Demandes retirées dans Jellyseerr, telles qu'elles se présentent maintenant.
async fn dropped_now(ctx: &TaskContext) -> Result<Vec<(String, i64, i64)>> {
    let media = ctx.jellyseerr.all_media().await?;
    let requests = ctx.jellyseerr.all_requests().await?;
    Ok(dropped_media(&media, &requests))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> Vec<PathMap> {
        vec![
            ("/movies".into(), "/h/movies".into(), "/media/movies".into()),
            (
                "/home/u/media".into(),
                "/mnt/sb".into(),
                "/seedbox/media".into(),
            ),
        ]
    }

    fn tor(tracker: &str, ratio: f64, days: i64) -> Torrent {
        serde_json::from_value(json!({
            "hash": "abc", "name": "Some.Movie.2020.1080p", "tracker": tracker,
            "ratio": ratio, "seeding_time": days * 86_400
        }))
        .unwrap()
    }

    #[test]
    fn media_without_request_is_dropped() {
        let media = vec![
            json!({"id": 1, "tmdbId": 100, "mediaType": "tv", "status": 3}),
            json!({"id": 2, "tmdbId": 200, "mediaType": "movie", "status": 3}),
            json!({"id": 3, "tmdbId": 0, "mediaType": "movie", "status": 2}),
            json!({"id": 4, "mediaType": "tv", "status": 2}),
            // fiches créées par le scan de la bibliothèque : jamais touchées
            json!({"id": 5, "tmdbId": 300, "mediaType": "movie", "status": 5}),
            json!({"id": 6, "tmdbId": 400, "mediaType": "tv", "status": 4}),
        ];
        let requests = vec![
            json!({"id": 9, "media": {"id": 2}}),
            json!({"id": 10, "media": {}}),
        ];
        let out = dropped_media(&media, &requests);
        assert_eq!(
            out,
            vec![("tv".to_string(), 100, 1)],
            "seul le média sans demande et avec TMDB"
        );
        assert!(dropped_media(
            &media,
            &[json!({"media": {"id": 1}}), json!({"media": {"id": 2}})]
        )
        .is_empty());
    }

    #[test]
    fn paths_map_to_host_and_jellyfin() {
        let (h, j) = map_path(&maps(), "/movies/X (2020)/x.mkv").unwrap();
        assert_eq!(h, PathBuf::from("/h/movies/X (2020)/x.mkv"));
        assert_eq!(j, "/media/movies/X (2020)/x.mkv");
        let (h, j) = map_path(&maps(), "/home/u/media/Movies/Y/y.mkv").unwrap();
        assert_eq!(h, PathBuf::from("/mnt/sb/Movies/Y/y.mkv"));
        assert_eq!(j, "/seedbox/media/Movies/Y/y.mkv");
        assert!(map_path(&maps(), "/moviesX/a.mkv").is_none());
        assert!(map_path(&maps(), "/tv/a.mkv").is_none());
    }

    #[test]
    fn titles_moved_by_anime_library_are_left_alone() {
        let t = |kind: Kind, id: i64| Title {
            kind,
            id,
            tmdb: 1,
            name: "x".into(),
            files: vec![],
        };
        let now = 1_000_000;
        let moves: BTreeMap<String, i64> = [
            ("seedbox:series:7".to_string(), now - 60),
            ("vps:movie:3".to_string(), now - 7 * 3600),
        ]
        .into_iter()
        .collect();
        let series = |id| {
            t(
                Kind::Series {
                    tvdb: 1,
                    total_files: 1,
                },
                id,
            )
        };
        assert!(recently_moved(&moves, "seedbox", &series(7), now));
        assert!(!recently_moved(&moves, "vps", &series(7), now));
        assert!(!recently_moved(&moves, "seedbox", &t(Kind::Movie, 7), now));
        // déplacé il y a plus de 6 h : suivi normal
        assert!(!recently_moved(&moves, "vps", &t(Kind::Movie, 3), now));
    }

    #[test]
    fn recent_imports_are_ignored() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-14T12:00:00Z")
            .unwrap()
            .timestamp();
        assert!(old_enough(Some("2026-09-14T10:00:00Z"), now, 60));
        assert!(!old_enough(Some("2026-09-14T11:30:00Z"), now, 60));
        assert!(!old_enough(None, now, 60));
    }

    #[test]
    fn c411_torrents_wait_for_the_seed_threshold() {
        let cfg = Cfg::default();
        let unl = vec!["c411.org".to_string()];
        assert_eq!(
            torrent_action(&tor("https://c411.org/ann", 0.4, 2), &unl, &cfg),
            TorrentAction::Wait
        );
        assert_eq!(
            torrent_action(&tor("https://c411.org/ann", 1.0, 0), &unl, &cfg),
            TorrentAction::DeleteNow
        );
        assert_eq!(
            torrent_action(&tor("https://c411.org/ann", 0.1, 7), &unl, &cfg),
            TorrentAction::DeleteNow
        );
        // tracker inconnu : prudence, même règle que C411
        assert_eq!(
            torrent_action(&tor("", 0.0, 0), &unl, &cfg),
            TorrentAction::Wait
        );
        // tracker public : tout de suite
        assert_eq!(
            torrent_action(&tor("udp://open.tracker/ann", 0.0, 0), &unl, &cfg),
            TorrentAction::DeleteNow
        );
    }

    #[test]
    fn torrent_source_matching() {
        let n = "Avatar.Aang.2026.MULTi.1080p.WEB.x264-GL0P";
        assert!(torrent_matches(
            n,
            Some("downloads/Avatar.Aang.2026.MULTi.1080p.WEB.x264-GL0P.mkv"),
            None
        ));
        assert!(torrent_matches(
            n,
            Some("radarr/Avatar.Aang.2026.MULTi.1080p.WEB.x264-GL0P/a.mkv"),
            None
        ));
        assert!(torrent_matches(n, None, Some(n)));
        assert!(!torrent_matches(
            "Other.Release.2026.1080p",
            Some("downloads/Avatar.mkv"),
            Some(n)
        ));
        assert!(
            !torrent_matches("a.mkv", Some("downloads/a.mkv"), None),
            "noms trop courts ignorés"
        );
    }

    #[test]
    fn history_never_yields_other_titles_torrents() {
        let recs = vec![
            json!({"movieId": 70, "downloadId": "AAA111"}),
            json!({"movieId": 12, "downloadId": "BBB222"}), // autre film : ignoré
            json!({"movieId": 70}),                         // sans torrent
        ];
        assert_eq!(
            hashes_from_history(&recs, "movieId", 70, None),
            BTreeSet::from(["aaa111".to_string()])
        );
        let recs = vec![
            json!({"seriesId": 5, "episodeId": 1, "downloadId": "S1PACK"}),
            json!({"seriesId": 5, "episodeId": 9, "downloadId": "S2PACK"}), // autre épisode : ignoré
            json!({"seriesId": 6, "episodeId": 1, "downloadId": "OTHER"}),  // autre série : ignoré
        ];
        let eps = BTreeSet::from([1]);
        assert_eq!(
            hashes_from_history(&recs, "seriesId", 5, Some(&eps)),
            BTreeSet::from(["s1pack".to_string()])
        );
    }

    fn ep(id: i64, season: i64, file: i64) -> Value {
        json!({"id": id, "seasonNumber": season, "episodeFileId": file})
    }

    #[test]
    fn series_plan_whole_season_or_episodes() {
        let eps = vec![
            ep(1, 1, 11),
            ep(2, 1, 12),
            ep(3, 2, 21),
            ep(4, 2, 22),
            ep(5, 2, 0),
        ];
        // saison 1 entière
        let p = plan_series(&BTreeSet::from([11, 12]), &eps, 4);
        assert!(!p.whole);
        assert_eq!(p.seasons, vec![1]);
        assert_eq!(p.episodes, vec![1, 2]);
        // un épisode isolé
        let p = plan_series(&BTreeSet::from([21]), &eps, 4);
        assert!(p.seasons.is_empty());
        assert_eq!(p.episodes, vec![3]);
        // toute la série
        let p = plan_series(&BTreeSet::from([11, 12, 21, 22]), &eps, 4);
        assert!(p.whole);
        assert_eq!(p.seasons, vec![1, 2]);
    }
}
