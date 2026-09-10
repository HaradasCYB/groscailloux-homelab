//! Pont entre les dépôts directs (pyLoad, drop manuel) et Sonarr/Radarr
//! (ex `auto-import.sh`). Le daemon surveille `paths.downloads` ; pour chaque
//! vidéo : parse → lookup → ajout si absent → scan d'import. Les archives sont
//! extraites (unzip/unrar/7z) puis supprimées, et le dossier extrait est scanné.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::process::Command;
use tracing::{info, warn};

use crate::classify::{classify, file_kind, is_video, ArchiveKind, FileKind, MediaKind};
use crate::clients::ArrClient;
use crate::context::TaskContext;

/// Point d'entrée du watcher : `name` est relatif à `paths.downloads`.
pub async fn handle_new_entry(ctx: &TaskContext, name: &str) -> Result<()> {
    match file_kind(name) {
        FileKind::Video => {
            tokio::time::sleep(Duration::from_secs(ctx.cfg.auto_import.settle_secs)).await;
            let host = ctx.cfg.paths.downloads.join(name);
            if !host.is_file() {
                info!(task = "auto_import", file = name, "vanished");
                return Ok(());
            }
            info!(task = "auto_import", file = name, "detected video");
            let container = format!("{}/{}", ctx.cfg.paths.downloads_in_container, name);
            classify_and_scan(ctx, name, &container).await
        }
        FileKind::Archive(kind) => handle_archive(ctx, name, kind).await,
        FileKind::Ignore => Ok(()),
    }
}

pub async fn classify_and_scan(ctx: &TaskContext, label: &str, scan_path: &str) -> Result<()> {
    match classify(label) {
        MediaKind::Series => {
            info!(task = "auto_import", label, "→ series");
            if let Err(e) = ensure_series(ctx, label).await {
                warn!(task = "auto_import", label, error = %e, "ensure_series failed");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            scan(ctx, &ctx.sonarr, scan_path).await
        }
        MediaKind::Movie => {
            info!(task = "auto_import", label, "→ movie");
            if let Err(e) = ensure_movie(ctx, label).await {
                warn!(task = "auto_import", label, error = %e, "ensure_movie failed");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            scan(ctx, &ctx.radarr, scan_path).await
        }
    }
}

async fn scan(ctx: &TaskContext, arr: &ArrClient, path: &str) -> Result<()> {
    if ctx.dry_run {
        info!(
            task = "auto_import",
            service = arr.name,
            path,
            "dry-run: would trigger scan"
        );
        return Ok(());
    }
    let resp = arr.command(arr.scan_command(path)).await?;
    let cmd_id = resp.get("id").and_then(Value::as_i64).unwrap_or(0);
    info!(
        task = "auto_import",
        service = arr.name,
        path,
        cmd_id,
        "scan triggered"
    );
    Ok(())
}

async fn ensure_movie(ctx: &TaskContext, filename: &str) -> Result<()> {
    let parsed = ctx.radarr.parse(filename).await?;
    let Some(title) = parsed
        .pointer("/parsedMovieInfo/movieTitles/0")
        .and_then(Value::as_str)
    else {
        info!(task = "auto_import", filename, "parse: no movie title");
        return Ok(());
    };
    let year = parsed
        .pointer("/parsedMovieInfo/year")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let term = if year > 0 {
        format!("{title} {year}")
    } else {
        title.to_string()
    };
    let found = ctx.radarr.lookup("movie", &term).await?;
    let Some(hit) = found.first() else {
        info!(task = "auto_import", term, "lookup: no_match");
        return Ok(());
    };
    let tmdb = hit.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    if tmdb == 0 {
        info!(task = "auto_import", term, "lookup: no tmdbId");
        return Ok(());
    }
    let found_title = hit.get("title").and_then(Value::as_str).unwrap_or(title);
    if ctx.radarr.exists("movie", "tmdbId", tmdb).await? {
        info!(
            task = "auto_import",
            tmdb,
            title = found_title,
            "already_in_library"
        );
        return Ok(());
    }
    let body = json!({
        "tmdbId": tmdb,
        "title": found_title,
        "year": hit.get("year"),
        "qualityProfileId": ctx.secrets.quality_profile_id,
        "rootFolderPath": ctx.cfg.auto_import.radarr_root,
        "monitored": true,
        "minimumAvailability": "released",
        "addOptions": {"searchForMovie": false, "monitor": "movieOnly"}
    });
    if ctx.dry_run {
        info!(
            task = "auto_import",
            tmdb,
            title = found_title,
            "dry-run: would add movie"
        );
        return Ok(());
    }
    let added = ctx.radarr.add("movie", &body).await?;
    let radarr_id = added.get("id").and_then(Value::as_i64).unwrap_or(0);
    info!(
        task = "auto_import",
        tmdb,
        title = found_title,
        radarr_id,
        "auto_added"
    );
    Ok(())
}

async fn ensure_series(ctx: &TaskContext, filename: &str) -> Result<()> {
    let parsed = ctx.sonarr.parse(filename).await?;
    let Some(title) = parsed
        .pointer("/parsedEpisodeInfo/seriesTitle")
        .and_then(Value::as_str)
    else {
        info!(task = "auto_import", filename, "parse: no series title");
        return Ok(());
    };
    let found = ctx.sonarr.lookup("series", title).await?;
    let Some(hit) = found.first() else {
        info!(task = "auto_import", title, "lookup: no_match");
        return Ok(());
    };
    let tvdb = hit.get("tvdbId").and_then(Value::as_i64).unwrap_or(0);
    if tvdb == 0 {
        info!(task = "auto_import", title, "lookup: no tvdbId");
        return Ok(());
    }
    let found_title = hit.get("title").and_then(Value::as_str).unwrap_or(title);
    if ctx.sonarr.exists("series", "tvdbId", tvdb).await? {
        info!(
            task = "auto_import",
            tvdb,
            title = found_title,
            "already_in_library"
        );
        return Ok(());
    }
    let body = json!({
        "tvdbId": tvdb,
        "title": found_title,
        "year": hit.get("year"),
        "qualityProfileId": ctx.secrets.quality_profile_id,
        "rootFolderPath": ctx.cfg.auto_import.sonarr_root,
        "monitored": true,
        "seasonFolder": true,
        "addOptions": {"searchForMissingEpisodes": false, "searchForCutoffUnmetEpisodes": false, "monitor": "all"}
    });
    if ctx.dry_run {
        info!(
            task = "auto_import",
            tvdb,
            title = found_title,
            "dry-run: would add series"
        );
        return Ok(());
    }
    let added = ctx.sonarr.add("series", &body).await?;
    let sonarr_id = added.get("id").and_then(Value::as_i64).unwrap_or(0);
    info!(
        task = "auto_import",
        tvdb,
        title = found_title,
        sonarr_id,
        "auto_added"
    );
    Ok(())
}

async fn handle_archive(ctx: &TaskContext, name: &str, kind: ArchiveKind) -> Result<()> {
    let cfg = &ctx.cfg.auto_import;
    let host = ctx.cfg.paths.downloads.join(name);
    info!(
        task = "auto_import",
        file = name,
        "archive detected, waiting for size stability"
    );
    let size = wait_stable(&host, cfg.archive_stable_checks, cfg.archive_max_wait_secs).await?;
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string();
    let dest = ctx.cfg.paths.downloads.join(&stem);
    if ctx.dry_run {
        info!(task = "auto_import", file = name, size, dest = %dest.display(), "dry-run: would extract, delete archive and scan");
        return Ok(());
    }
    std::fs::create_dir_all(&dest)?;
    extract(kind, &host, &dest).await?;
    info!(task = "auto_import", file = name, dest = %dest.display(), "extracted");
    if std::fs::remove_file(&host).is_ok() {
        info!(task = "auto_import", file = name, "archive removed");
    }
    let Some(first_video) = first_video(&dest) else {
        info!(task = "auto_import", dest = %dest.display(), "no video in extracted dir");
        return Ok(());
    };
    let label = first_video
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let scan_path = format!("{}/{}", ctx.cfg.paths.downloads_in_container, stem);
    classify_and_scan(ctx, &label, &scan_path).await
}

async fn wait_stable(path: &Path, checks: u32, max_secs: u64) -> Result<u64> {
    let (mut prev, mut stable, mut waited) = (u64::MAX, 0u32, 0u64);
    while waited < max_secs {
        let Ok(meta) = std::fs::metadata(path) else {
            bail!("vanished during wait")
        };
        let cur = meta.len();
        if cur > 0 && cur == prev {
            stable += 1;
            if stable >= checks {
                return Ok(cur);
            }
        } else {
            stable = 0;
        }
        prev = cur;
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += 5;
    }
    Ok(prev)
}

async fn extract(kind: ArchiveKind, archive: &Path, dest: &Path) -> Result<()> {
    let a = archive.to_string_lossy().to_string();
    let d = dest.to_string_lossy().to_string();
    let attempts: Vec<(&str, Vec<String>)> = match kind {
        ArchiveKind::Zip => vec![(
            "unzip",
            vec!["-q".into(), "-o".into(), a.clone(), "-d".into(), d.clone()],
        )],
        ArchiveKind::Rar => vec![
            (
                "unrar",
                vec![
                    "x".into(),
                    "-o+".into(),
                    "-y".into(),
                    a.clone(),
                    format!("{d}/"),
                ],
            ),
            (
                "7z",
                vec!["x".into(), "-y".into(), format!("-o{d}"), a.clone()],
            ),
        ],
    };
    let mut last = None;
    for (prog, args) in attempts {
        match Command::new(prog).args(&args).output().await {
            Ok(o) if o.status.success() => return Ok(()),
            Ok(o) => {
                last = Some(format!(
                    "{prog}: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => last = Some(format!("{prog}: {e}")),
        }
    }
    bail!("extraction échouée : {}", last.unwrap_or_default())
}

fn first_video(dir: &Path) -> Option<PathBuf> {
    walkdir::WalkDir::new(dir)
        .max_depth(3)
        .sort_by_file_name()
        .into_iter()
        .flatten()
        .find(|e| e.file_type().is_file() && e.file_name().to_str().map(is_video).unwrap_or(false))
        .map(|e| e.into_path())
}

pub fn ensure_watch_dir(p: &Path) -> Result<()> {
    if !p.is_dir() {
        bail!("dossier surveillé introuvable : {}", p.display());
    }
    std::fs::read_dir(p).with_context(|| format!("lecture de {}", p.display()))?;
    Ok(())
}
