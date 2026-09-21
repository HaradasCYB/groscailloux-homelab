//! Fait voir à Jellyfin les sous-titres que le Bazarr de la seedbox vient d'écrire à côté des vidéos
//! (`<vidéo>.fr.srt`, extraits des pistes incrustées ou téléchargés). Sans ça, Jellyfin extrait lui-même
//! une piste incrustée en relisant tout le fichier par le lien seedbox (111 s pour 1,6 Go, mesuré le
//! 2026-09-21) et le lecteur abandonne avant : « pas de sous-titres ». Un fichier annexe n'est vu que par un
//! **FullRefresh** de l'item (ni `Library/Media/Updated`, ni Refresh « Default », vérifié) : on lit
//! l'historique Bazarr depuis le dernier horodatage traité, on invalide les dossiers dans rclone, on retrouve
//! la fiche Jellyfin par chemin et on la rafraîchit, jamais pendant une lecture.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::seedbox_refresh::{map_path, refresh_params};
use super::{Report, Task};
use crate::clients::HistoryRow;
use crate::config::Config;
use crate::context::TaskContext;

pub struct SubtitleSync;

const SUB_EXTS: [&str; 5] = ["srt", "ass", "ssa", "vtt", "sub"];
const FLAGS: [&str; 6] = ["hi", "sdh", "cc", "forced", "default", "foreign"];

/// Chemin d'un sous-titre externe → chemin de la vidéo **sans extension** :
/// `X.fr.srt`, `X.fr.hi.srt`, `X.fre.forced.ass` → `X`. `None` si ce n'est pas un fichier de sous-titres.
pub fn video_stem(subtitle_path: &str) -> Option<String> {
    let p = Path::new(subtitle_path);
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    if !SUB_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let mut stem = p.with_extension("").to_string_lossy().to_string();
    // drapeaux puis langue (2 ou 3 lettres), dans l'ordre inverse d'écriture
    for _ in 0..3 {
        let Some((base, last)) = stem.rsplit_once('.') else {
            break;
        };
        let l = last.to_ascii_lowercase();
        // un drapeau (« hi », « cc »…) avant une langue : on continue ; une langue : on s'arrête là
        if FLAGS.contains(&l.as_str()) {
            stem = base.to_string();
        } else if (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic()) {
            stem = base.to_string();
            break;
        } else {
            break;
        }
    }
    Some(stem)
}

/// Lignes à traiter : sous-titre écrit (`action == 1`), pas avant le curseur, du plus ancien au plus récent.
pub fn pending(rows: &[HistoryRow], cursor: i64) -> Vec<HistoryRow> {
    let mut out: Vec<HistoryRow> = rows
        .iter()
        .filter(|r| r.action == 1 && !r.subtitles_path.is_empty() && r.at >= cursor)
        .cloned()
        .collect();
    out.sort_by_key(|r| r.at);
    out
}

/// L'item Jellyfin liste-t-il déjà un sous-titre **externe** dans cette langue (code 2 lettres de Bazarr) ?
pub fn has_external(item: &Value, lang2: &str) -> bool {
    let wanted: Vec<&str> = match lang2 {
        "fr" => vec!["fre", "fra", "fr"],
        "en" => vec!["eng", "en"],
        other => vec![other],
    };
    item.get("MediaStreams")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter().any(|s| {
                s.get("Type").and_then(Value::as_str) == Some("Subtitle")
                    && s.get("IsExternal")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    && s.get("Language")
                        .and_then(Value::as_str)
                        .map(|l| wanted.iter().any(|w| w.eq_ignore_ascii_case(l)))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Index des items Jellyfin (épisodes et films) par chemin de vidéo sans extension.
pub fn index_by_stem(items: &[Value]) -> HashMap<String, &Value> {
    items
        .iter()
        .filter_map(|i| {
            let p = i.get("Path").and_then(Value::as_str)?;
            let stem = Path::new(p)
                .with_extension("")
                .to_string_lossy()
                .to_string();
            Some((stem, i))
        })
        .collect()
}

#[async_trait]
impl Task for SubtitleSync {
    fn name(&self) -> &'static str {
        "subtitle_sync"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.subtitle_sync.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let sb = &ctx.cfg.seedbox;
        let Some(bazarr) = ctx.bazarr.as_ref() else {
            return Ok(Report::new("bazarr not configured", 0));
        };
        if !sb.mount_point.is_dir() {
            warn!(task = "subtitle_sync", mount = %sb.mount_point.display(), "seedbox mount unavailable, retry next run");
            return Ok(Report::new("mount unavailable", 0));
        }
        let max = ctx.cfg.tasks.subtitle_sync.max_per_run.max(1);
        // historique : assez profond pour le rattrapage initial (curseur 0), petit en régime normal
        let mut todo: Vec<(&'static str, HistoryRow)> = Vec::new();
        for (list, rows) in [
            ("episodes", bazarr.history_episodes(3000).await?),
            ("movies", bazarr.history_movies(1000).await?),
        ] {
            let cursor = ctx
                .state
                .read(|s| s.bazarr_history.get(list).copied())
                .await
                .unwrap_or(0);
            for r in pending(&rows, cursor) {
                todo.push((list, r));
            }
        }
        if todo.is_empty() {
            return Ok(Report::new("nothing new", 0));
        }
        todo.sort_by_key(|(_, r)| r.at);
        let batch: Vec<(&'static str, HistoryRow)> = todo.into_iter().take(max).collect();

        // dossiers à relire dans rclone, chemins Jellyfin attendus
        let mut dirs: Vec<String> = Vec::new();
        let mut wanted: Vec<(&'static str, HistoryRow, String)> = Vec::new();
        for (list, r) in batch {
            let Some((dir, jf)) = map_path(sb, &r.subtitles_path) else {
                warn!(task = "subtitle_sync", path = %r.subtitles_path, "chemin hors media_root, ignoré");
                continue;
            };
            let Some(stem) = video_stem(&jf) else {
                continue;
            };
            dirs.push(dir);
            wanted.push((list, r, stem));
        }
        dirs.sort();
        dirs.dedup();
        if wanted.is_empty() {
            return Ok(Report::new("nothing mappable", 0));
        }

        let items = ctx
            .jellyfin
            .items(&[
                ("Recursive", "true"),
                ("IncludeItemTypes", "Episode,Movie"),
                ("Fields", "Path,MediaStreams"),
                ("EnableImages", "false"),
            ])
            .await
            .context("jellyfin Items")?;
        let by_stem = index_by_stem(&items);
        let playing: HashSet<String> = ctx
            .jellyfin
            .sessions()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                s.pointer("/NowPlayingItem/Id")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();

        if ctx.dry_run {
            let n = wanted
                .iter()
                .filter(|(_, r, stem)| {
                    by_stem
                        .get(stem)
                        .map(|i| !has_external(i, &r.language))
                        .unwrap_or(false)
                })
                .count();
            info!(
                task = "subtitle_sync",
                rows = wanted.len(),
                to_refresh = n,
                ?dirs,
                "dry-run"
            );
            return Ok(Report::new(
                format!("dry_run rows={} refresh={n}", wanted.len()),
                n as u32,
            ));
        }

        let rc = format!("{}/vfs/refresh", sb.rclone_rc.trim_end_matches('/'));
        match ctx.http.post(&rc).json(&refresh_params(&dirs)).send().await {
            Ok(resp) if !resp.status().is_success() => {
                warn!(task = "subtitle_sync", status = %resp.status(), "rclone vfs/refresh failed")
            }
            Err(e) => warn!(task = "subtitle_sync", error = %e, "rclone rc injoignable"),
            _ => {}
        }

        let (mut refreshed, mut already, mut unknown, mut skipped, mut failed) =
            (0u32, 0u32, 0u32, 0u32, 0u32);
        let mut last_at: HashMap<&'static str, i64> = HashMap::new();
        for (list, r, stem) in &wanted {
            let Some(item) = by_stem.get(stem) else {
                unknown += 1;
                last_at.insert(list, r.at);
                continue;
            };
            let id = item.get("Id").and_then(Value::as_str).unwrap_or("");
            if has_external(item, &r.language) {
                already += 1;
                last_at.insert(list, r.at);
                continue;
            }
            if playing.contains(id) {
                // on reviendra dessus : le curseur ne dépasse pas cette ligne
                skipped += 1;
                break;
            }
            match ctx.jellyfin.refresh_streams(id).await {
                Ok(()) => {
                    refreshed += 1;
                    last_at.insert(list, r.at);
                }
                Err(e) => {
                    warn!(task = "subtitle_sync", item = id, error = %e, "refresh failed");
                    failed += 1;
                    break;
                }
            }
        }
        for (list, at) in &last_at {
            let (list, at) = (list.to_string(), *at);
            ctx.state
                .update(|s| {
                    let e = s.bazarr_history.entry(list).or_insert(0);
                    if at > *e {
                        *e = at;
                    }
                })
                .await?;
        }
        info!(
            task = "subtitle_sync",
            refreshed,
            already,
            unknown,
            skipped,
            failed,
            dirs = dirs.len(),
            "done"
        );
        Ok(Report::new(
            format!("refreshed={refreshed} already={already} unknown={unknown} playing={skipped} failed={failed}"),
            refreshed,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(path: &str, at: i64, action: i64) -> HistoryRow {
        HistoryRow {
            subtitles_path: path.into(),
            at,
            action,
            provider: "embeddedsubtitles".into(),
            language: "fr".into(),
        }
    }

    #[test]
    fn stems_drop_language_and_flags() {
        assert_eq!(
            video_stem("/m/A/S1/A - S01E01.fr.srt").as_deref(),
            Some("/m/A/S1/A - S01E01")
        );
        assert_eq!(video_stem("/m/A/A.fr.hi.srt").as_deref(), Some("/m/A/A"));
        assert_eq!(
            video_stem("/m/A/A.fre.forced.ass").as_deref(),
            Some("/m/A/A")
        );
        assert_eq!(video_stem("/m/A/A.srt").as_deref(), Some("/m/A/A"));
        // un point dans le titre n'est pas une langue
        assert_eq!(
            video_stem("/m/Mr. Robot/Mr. Robot - S01E01.fr.srt").as_deref(),
            Some("/m/Mr. Robot/Mr. Robot - S01E01")
        );
        assert_eq!(
            video_stem("/m/Dr.Who/ep.2024.fr.srt").as_deref(),
            Some("/m/Dr.Who/ep.2024")
        );
        assert_eq!(video_stem("/m/A/A.mkv"), None);
    }

    #[test]
    fn pending_filters_and_orders() {
        let rows = vec![
            row("/m/b.fr.srt", 300, 1),
            row("/m/a.fr.srt", 100, 1),
            row("", 400, 1),
            row("/m/c.fr.srt", 500, 2),
            row("/m/d.fr.srt", 50, 1),
        ];
        let p = pending(&rows, 100);
        assert_eq!(
            p.iter()
                .map(|r| r.subtitles_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/m/a.fr.srt", "/m/b.fr.srt"]
        );
    }

    #[test]
    fn external_detection_matches_codes() {
        let item = json!({"MediaStreams": [
            {"Type": "Subtitle", "Language": "fra", "IsExternal": false},
            {"Type": "Subtitle", "Language": "fre", "IsExternal": true}
        ]});
        assert!(has_external(&item, "fr"));
        assert!(!has_external(&item, "en"));
        let only_embedded = json!({"MediaStreams": [{"Type": "Subtitle", "Language": "fra"}]});
        assert!(!has_external(&only_embedded, "fr"));
        let items = vec![json!({"Id": "1", "Path": "/seedbox/media/A/A - S01E01.mkv"})];
        let idx = index_by_stem(&items);
        assert!(idx.contains_key("/seedbox/media/A/A - S01E01"));
    }
}
