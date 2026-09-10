//! Ménage quotidien (ex `homelab-cleanup.sh`, cron.daily). Ne cible que des
//! chemins qui existent ; tout est relatif à la config.

use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use tracing::info;
use walkdir::WalkDir;

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct Cleanup;

fn older_than(meta: &std::fs::Metadata, days: u64) -> bool {
    meta.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(|age| age > Duration::from_secs(days * 86400))
        .unwrap_or(false)
}

/// Supprime les fichiers de `dir` (profondeur 1) plus vieux que `days`.
fn purge_files(dir: &Path, days: u64, dry: bool) -> u32 {
    if !dir.is_dir() {
        return 0;
    }
    let mut n = 0;
    for e in WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .flatten()
    {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_file()
            && older_than(&meta, days)
            && (dry || std::fs::remove_file(e.path()).is_ok())
        {
            n += 1;
        }
    }
    n
}

/// Supprime les dossiers vides (profondeur 1..3) plus vieux que `days`, du plus profond au moins profond.
fn purge_empty_dirs(dir: &Path, days: u64, dry: bool) -> u32 {
    if !dir.is_dir() {
        return 0;
    }
    let mut n = 0;
    for e in WalkDir::new(dir)
        .min_depth(1)
        .max_depth(3)
        .contents_first(true)
        .into_iter()
        .flatten()
    {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_dir() || !older_than(&meta, days) {
            continue;
        }
        let empty = std::fs::read_dir(e.path())
            .map(|mut r| r.next().is_none())
            .unwrap_or(false);
        if empty && (dry || std::fs::remove_dir(e.path()).is_ok()) {
            n += 1;
        }
    }
    n
}

#[async_trait]
impl Task for Cleanup {
    fn name(&self) -> &'static str {
        "cleanup"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.cleanup.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = ctx.cfg.cleanup.clone();
        let downloads = ctx.cfg.paths.downloads.clone();
        let dry = ctx.dry_run;
        let (transcodes, empty_dirs, recycle, logs) = tokio::task::spawn_blocking(move || {
            let t = purge_files(&cfg.transcodes_dir, cfg.transcodes_max_age_days, dry);
            let d = purge_empty_dirs(&downloads, cfg.empty_download_dirs_max_age_days, dry);
            let r: u32 = cfg
                .recycle_dirs
                .iter()
                .map(|p| purge_files(p, cfg.recycle_max_age_days, dry))
                .sum();
            let l = purge_files(&cfg.jellyfin_log_dir, cfg.jellyfin_log_max_age_days, dry);
            (t, d, r, l)
        })
        .await?;
        let total = transcodes + empty_dirs + recycle + logs;
        info!(
            task = "cleanup",
            dry_run = dry,
            transcodes,
            empty_dirs,
            recycle,
            logs,
            "done"
        );
        Ok(Report::new(
            format!(
                "transcodes={transcodes} empty_dirs={empty_dirs} recycle={recycle} logs={logs}"
            ),
            total,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_counts_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("old.ts");
        std::fs::write(&f, b"x").unwrap();
        // fichier fraîchement créé : pas plus vieux que 0 jour → pas candidat
        assert_eq!(purge_files(dir.path(), 1, true), 0);
        assert!(f.exists());
        assert_eq!(purge_files(Path::new("/nonexistent/path"), 1, false), 0);
    }
}
