//! Dégradation gracieuse du seed quand le disque sature
//! (ex `qbit-disk-pressure-handler.sh`, toutes les 15 min).
//!   < hard_pct       : rien
//!   hard..crit       : supprime (fichiers compris) les torrents déjà arrêtés
//!                      (`stoppedUP`) les plus anciens, max N par passage
//!   ≥ crit_pct       : log critique, aucune action automatique
//! Un torrent en seed actif n'est jamais touché ; les hardlinks dans /media survivent.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{error, info, warn};

use super::{Report, Task};
use crate::clients::Torrent;
use crate::config::Config;
use crate::context::TaskContext;
use crate::disk;

pub struct DiskPressure;

#[derive(Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Hard,
    Critical,
}

pub fn level(pct: u8, hard: u8, crit: u8) -> Level {
    if pct >= crit {
        Level::Critical
    } else if pct >= hard {
        Level::Hard
    } else {
        Level::Ok
    }
}

/// Torrents arrêtés après seed complet, du plus ancien au plus récent.
pub fn candidates(torrents: &[Torrent], max: usize) -> Vec<&Torrent> {
    let mut v: Vec<&Torrent> = torrents.iter().filter(|t| t.state == "stoppedUP").collect();
    v.sort_by_key(|t| t.completion_on);
    v.truncate(max);
    v
}

#[async_trait]
impl Task for DiskPressure {
    fn name(&self) -> &'static str {
        "disk_pressure"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.disk_pressure.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.disk_pressure;
        let pct = disk::usage_percent(&ctx.cfg.paths.base)?;
        match level(pct, cfg.hard_pct, cfg.crit_pct) {
            Level::Ok => {
                info!(task = "disk_pressure", use_pct = pct, "ok");
                return Ok(Report::new(format!("use_pct={pct} no_action"), 0));
            }
            Level::Critical => {
                error!(
                    task = "disk_pressure",
                    use_pct = pct,
                    "critical: manual cleanup needed, no auto action"
                );
                return Ok(Report::new(format!("use_pct={pct} CRITICAL"), 0));
            }
            Level::Hard => {}
        }
        let _guard = ctx.qbit_lock.lock().await;
        ctx.qbit.version().await?;
        let torrents = ctx.qbit.torrents().await?;
        let picked = candidates(&torrents, cfg.max_actions_per_run);
        if picked.is_empty() {
            warn!(
                task = "disk_pressure",
                use_pct = pct,
                "hard: no stopped torrents to delete"
            );
            return Ok(Report::new(format!("use_pct={pct} hard no_candidates"), 0));
        }
        let hashes: Vec<String> = picked.iter().map(|t| t.hash.clone()).collect();
        if ctx.dry_run {
            for t in &picked {
                info!(task = "disk_pressure", hash = %t.hash, size = t.size, completed = t.completion_on, name = %t.name, "dry-run: would delete with files");
            }
            return Ok(Report::new(
                format!("use_pct={pct} dry_run would_delete={}", picked.len()),
                picked.len() as u32,
            ));
        }
        ctx.qbit.delete(&hashes, true).await?;
        for t in &picked {
            info!(task = "disk_pressure", hash = %t.hash, size = t.size, completed = t.completion_on, name = %t.name, "deleted");
        }
        Ok(Report::new(
            format!("use_pct={pct} deleted={}", picked.len()),
            picked.len() as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(level(80, 95, 98), Level::Ok);
        assert_eq!(level(95, 95, 98), Level::Hard);
        assert_eq!(level(97, 95, 98), Level::Hard);
        assert_eq!(level(98, 95, 98), Level::Critical);
    }

    #[test]
    fn oldest_stopped_first_and_capped() {
        let mk = |h: &str, state: &str, done: i64| Torrent {
            hash: h.into(),
            name: h.into(),
            state: state.into(),
            tracker: String::new(),
            ratio_limit: -2.0,
            seeding_time_limit: -2,
            completion_on: done,
            size: 1,
        };
        let ts = vec![
            mk("a", "stoppedUP", 30),
            mk("b", "uploading", 1),
            mk("c", "stoppedUP", 10),
            mk("d", "stoppedUP", 20),
        ];
        let got: Vec<&str> = candidates(&ts, 2).iter().map(|t| t.hash.as_str()).collect();
        assert_eq!(got, vec!["c", "d"]);
    }
}
