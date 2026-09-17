//! Fait apparaître dans Jellyfin (VPS) ce que les Radarr/Sonarr de la seedbox viennent
//! d'importer. Le montage rclone ne voit pas les nouveaux fichiers avant l'expiration de son
//! cache de répertoires et Jellyfin ne surveille pas un montage réseau : on lit l'historique
//! d'imports des Arrs seedbox depuis le dernier id traité, on invalide les dossiers concernés
//! dans rclone (`vfs/refresh`) puis on signale les fichiers à Jellyfin (`Library/Media/Updated`).
//! Montage absent ⇒ rien n'avance (le curseur reste en place pour la passe suivante).

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Map, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::{Config, Seedbox};
use crate::context::TaskContext;

pub struct SeedboxRefresh;

/// Chemin d'import côté seedbox → (dossier relatif pour rclone, chemin vu par Jellyfin).
pub fn map_path(sb: &Seedbox, imported: &str) -> Option<(String, String)> {
    let root = sb.media_root.trim_end_matches('/');
    let rel = imported.strip_prefix(root)?.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let dir = Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let jf = format!("{}/{}", sb.jellyfin_root.trim_end_matches('/'), rel);
    Some((dir, jf))
}

/// Nouveaux imports (id > curseur), du plus ancien au plus récent.
pub fn new_imports(records: &[Value], cursor: i64) -> Vec<(i64, String)> {
    let mut out: Vec<(i64, String)> = records
        .iter()
        .filter_map(|r| {
            let id = r.get("id").and_then(Value::as_i64)?;
            let path = r.pointer("/data/importedPath").and_then(Value::as_str)?;
            (id > cursor).then(|| (id, path.to_string()))
        })
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

/// Paramètres `vfs/refresh` : le dossier et chacun de ses parents (un nouveau dossier de
/// film n'est visible qu'une fois le parent `Movies` relu), en **récursif** : sans cela, le dossier
/// d'une série tout juste créée est listé vide et Jellyfin enregistre une série sans épisode
/// (Game of Thrones, le 2026-09-17).
pub fn refresh_params(dirs: &[String]) -> Value {
    let mut all: Vec<String> = Vec::new();
    for d in dirs {
        let mut p = Path::new(d);
        loop {
            let s = p.to_string_lossy().to_string();
            if !s.is_empty() && !all.contains(&s) {
                all.push(s);
            }
            match p.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => p = parent,
                _ => break,
            }
        }
    }
    all.sort_by_key(|s| s.matches('/').count());
    let mut m = Map::new();
    m.insert("recursive".to_string(), Value::String("true".into()));
    for (i, d) in all.iter().enumerate() {
        let key = if i == 0 {
            "dir".to_string()
        } else {
            format!("dir{}", i + 1)
        };
        m.insert(key, Value::String(d.clone()));
    }
    Value::Object(m)
}

#[async_trait]
impl Task for SeedboxRefresh {
    fn name(&self) -> &'static str {
        "seedbox_refresh"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        if cfg.seedbox.enabled {
            Duration::from_secs(cfg.tasks.seedbox_refresh.interval_secs)
        } else {
            Duration::ZERO
        }
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let sb = &ctx.cfg.seedbox;
        if !sb.enabled {
            return Ok(Report::new("seedbox disabled", 0));
        }
        let arrs: Vec<_> = ctx
            .seedbox_radarr
            .iter()
            .chain(ctx.seedbox_sonarr.iter())
            .collect();
        let mut pending: Vec<(String, i64, Vec<(i64, String)>)> = Vec::new();
        for arr in arrs {
            let records = arr.recent_imports(100).await?;
            let cursor = ctx
                .state
                .read(|s| s.seedbox_history.get(arr.name).copied())
                .await;
            let Some(cursor) = cursor else {
                // première passe : on part de l'état actuel, sans rejouer l'historique
                let max = records
                    .iter()
                    .filter_map(|r| r.get("id").and_then(Value::as_i64))
                    .max()
                    .unwrap_or(0);
                if !ctx.dry_run {
                    ctx.state
                        .update(|s| s.seedbox_history.insert(arr.name.to_string(), max))
                        .await?;
                }
                info!(
                    task = "seedbox_refresh",
                    service = arr.name,
                    cursor = max,
                    "cursor initialized"
                );
                continue;
            };
            let news = new_imports(&records, cursor);
            if !news.is_empty() {
                pending.push((arr.name.to_string(), cursor, news));
            }
        }
        if pending.is_empty() {
            return Ok(Report::new("no new imports", 0));
        }

        let first_dir = sb.mount_point.join(
            pending[0].2[0]
                .1
                .strip_prefix(sb.media_root.trim_end_matches('/'))
                .unwrap_or("")
                .trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or(""),
        );
        if !first_dir.is_dir() {
            warn!(task = "seedbox_refresh", mount = %sb.mount_point.display(), "seedbox mount unavailable, retry next run");
            return Ok(Report::new("mount unavailable", 0));
        }

        let mut dirs = Vec::new();
        let mut jf_paths = Vec::new();
        for (_, _, news) in &pending {
            for (_, path) in news {
                if let Some((dir, jf)) = map_path(sb, path) {
                    dirs.push(dir);
                    jf_paths.push(jf);
                }
            }
        }
        dirs.sort();
        dirs.dedup();
        if ctx.dry_run {
            info!(
                task = "seedbox_refresh",
                ?dirs,
                files = jf_paths.len(),
                "dry-run: would refresh rclone + jellyfin"
            );
            return Ok(Report::new(
                format!("dry_run files={}", jf_paths.len()),
                jf_paths.len() as u32,
            ));
        }
        let rc = format!("{}/vfs/refresh", sb.rclone_rc.trim_end_matches('/'));
        let resp = ctx
            .http
            .post(&rc)
            .json(&refresh_params(&dirs))
            .send()
            .await
            .context("rclone rc injoignable")?;
        if !resp.status().is_success() {
            warn!(task = "seedbox_refresh", status = %resp.status(), "rclone vfs/refresh failed");
        }
        ctx.jellyfin.media_updated(&jf_paths).await?;
        for (name, _, news) in &pending {
            let last = news.last().map(|(id, _)| *id).unwrap_or(0);
            ctx.state
                .update(|s| s.seedbox_history.insert(name.clone(), last))
                .await?;
        }
        info!(
            task = "seedbox_refresh",
            files = jf_paths.len(),
            ?dirs,
            "jellyfin notified"
        );
        Ok(Report::new(
            format!("files={} dirs={}", jf_paths.len(), dirs.len()),
            jf_paths.len() as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sb() -> Seedbox {
        Seedbox {
            enabled: true,
            media_root: "/home/kakaouette/media".into(),
            jellyfin_root: "/seedbox/media".into(),
            ..Seedbox::default()
        }
    }

    #[test]
    fn maps_seedbox_paths_to_rclone_and_jellyfin() {
        let (dir, jf) = map_path(
            &sb(),
            "/home/kakaouette/media/Movies/Valmont (1989)/Valmont (1989).mkv",
        )
        .unwrap();
        assert_eq!(dir, "Movies/Valmont (1989)");
        assert_eq!(
            jf,
            "/seedbox/media/Movies/Valmont (1989)/Valmont (1989).mkv"
        );
        assert!(map_path(&sb(), "/elsewhere/file.mkv").is_none());
        assert!(map_path(&sb(), "/home/kakaouette/media").is_none());
    }

    #[test]
    fn cursor_filters_and_orders_imports() {
        let recs = vec![
            json!({"id": 12, "data": {"importedPath": "/m/b.mkv"}}),
            json!({"id": 10, "data": {"importedPath": "/m/a.mkv"}}),
            json!({"id": 9, "data": {"importedPath": "/m/old.mkv"}}),
            json!({"id": 13, "data": {}}),
        ];
        let got = new_imports(&recs, 9);
        assert_eq!(
            got,
            vec![(10, "/m/a.mkv".to_string()), (12, "/m/b.mkv".to_string())]
        );
    }

    #[test]
    fn refresh_params_include_parents_first() {
        let p = refresh_params(&[
            "TV Shows/Watson/Season 2".to_string(),
            "Movies/Valmont (1989)".to_string(),
        ]);
        let dirs: Vec<&str> = p
            .as_object()
            .unwrap()
            .values()
            .filter_map(Value::as_str)
            .collect();
        assert!(dirs.contains(&"Movies"));
        assert!(dirs.contains(&"TV Shows"));
        assert!(dirs.contains(&"TV Shows/Watson"));
        assert!(dirs.contains(&"TV Shows/Watson/Season 2"));
        assert_eq!(p["dir"].as_str().unwrap().matches('/').count(), 0);
    }
}
