//! Remplace les téléchargements bloqués (ex `qbit-stuck-handler.sh`, toutes les 5 min).
//! Un item de queue Sonarr/Radarr dont `errorMessage` matche `pattern` est suivi ;
//! passé `stall_secs`, il est retiré (client + blocklist) et Sonarr/Radarr relance
//! une recherche. Max `max_actions_per_run` suppressions par passage. Jamais de
//! purge globale : chaque DELETE vise un id de queue précis.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::{ArrClient, QueueItem};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, StuckEntry};

pub struct StuckHandler;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Track,
    StillStuck { age: i64 },
    Deferred { age: i64 },
    Replace { age: i64 },
}

pub fn decide(
    first_seen: Option<i64>,
    now: i64,
    stall_secs: i64,
    actions: usize,
    max: usize,
) -> Decision {
    match first_seen {
        None => Decision::Track,
        Some(fs) => {
            let age = now - fs;
            if age < stall_secs {
                Decision::StillStuck { age }
            } else if actions >= max {
                Decision::Deferred { age }
            } else {
                Decision::Replace { age }
            }
        }
    }
}

pub fn is_stuck(re: &Regex, item: &QueueItem) -> bool {
    item.error_message
        .as_deref()
        .map(|m| re.is_match(m))
        .unwrap_or(false)
}

async fn process(ctx: &TaskContext, arr: &ArrClient, re: &Regex, actions: &mut u32) -> Result<()> {
    let cfg = &ctx.cfg.tasks.stuck_handler;
    let queue = match arr.queue().await {
        Ok(q) => q,
        Err(e) => {
            warn!(task = "stuck_handler", service = arr.name, error = %e, "queue_unreachable");
            return Ok(());
        }
    };
    let ts = now();
    let mut seen = BTreeSet::new();
    for item in queue.iter().filter(|i| is_stuck(re, i)) {
        let Some(did) = item
            .download_id
            .as_deref()
            .filter(|d| !d.is_empty() && *d != "null")
        else {
            continue;
        };
        let key = format!("{}:{}", arr.name, did);
        seen.insert(key.clone());
        let first_seen = ctx
            .state
            .read(|s| s.stuck.get(&key).map(|e| e.first_seen))
            .await;
        match decide(
            first_seen,
            ts,
            cfg.stall_secs,
            *actions as usize,
            cfg.max_actions_per_run,
        ) {
            Decision::Track => {
                ctx.state
                    .update(|s| {
                        s.stuck.insert(
                            key.clone(),
                            StuckEntry {
                                service: arr.name.into(),
                                download_id: did.into(),
                                first_seen: ts,
                                title: item.title.clone(),
                            },
                        );
                    })
                    .await?;
                info!(task = "stuck_handler", service = arr.name, hash = did, title = %item.title, error = item.error_message.as_deref().unwrap_or(""), "tracking");
            }
            Decision::StillStuck { age } => {
                info!(task = "stuck_handler", service = arr.name, hash = did, age, title = %item.title, "still_stuck");
            }
            Decision::Deferred { age } => {
                info!(task = "stuck_handler", service = arr.name, hash = did, age, title = %item.title, "deferred: rate_limit");
            }
            Decision::Replace { age } => {
                if ctx.dry_run {
                    info!(task = "stuck_handler", service = arr.name, queue_id = item.id, hash = did, age, title = %item.title, "dry-run: would replace");
                    *actions += 1;
                    continue;
                }
                match arr.remove_queue_item(item.id).await {
                    Ok(()) => {
                        *actions += 1;
                        ctx.state.update(|s| s.stuck.remove(&key)).await?;
                        info!(task = "stuck_handler", service = arr.name, queue_id = item.id, hash = did, age, title = %item.title, "replaced");
                    }
                    Err(e) => {
                        warn!(task = "stuck_handler", service = arr.name, queue_id = item.id, hash = did, error = %e, "delete_failed")
                    }
                }
            }
        }
    }
    // Items disparus de la queue (importés, retirés à la main…)
    let cleared: Vec<(String, String)> = ctx
        .state
        .read(|s| {
            s.stuck
                .iter()
                .filter(|(k, e)| e.service == arr.name && !seen.contains(*k))
                .map(|(k, e)| (k.clone(), e.title.clone()))
                .collect()
        })
        .await;
    if !cleared.is_empty() {
        ctx.state
            .update(|s| {
                for (k, _) in &cleared {
                    s.stuck.remove(k);
                }
            })
            .await?;
        for (k, title) in cleared {
            info!(task = "stuck_handler", service = arr.name, key = %k, %title, "cleared");
        }
    }
    Ok(())
}

#[async_trait]
impl Task for StuckHandler {
    fn name(&self) -> &'static str {
        "stuck_handler"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.stuck_handler.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let re = Regex::new(&format!("(?i){}", ctx.cfg.tasks.stuck_handler.pattern))?;
        let _guard = ctx.qbit_lock.lock().await;
        let mut actions = 0u32;
        process(ctx, &ctx.sonarr, &re, &mut actions).await?;
        process(ctx, &ctx.radarr, &re, &mut actions).await?;
        let tracked = ctx.state.read(|s| s.stuck.len()).await;
        Ok(Report::new(
            format!("actions={actions} tracked={tracked}"),
            actions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_table() {
        assert_eq!(decide(None, 100, 10, 0, 5), Decision::Track);
        assert_eq!(
            decide(Some(95), 100, 10, 0, 5),
            Decision::StillStuck { age: 5 }
        );
        assert_eq!(
            decide(Some(50), 100, 10, 5, 5),
            Decision::Deferred { age: 50 }
        );
        assert_eq!(
            decide(Some(50), 100, 10, 4, 5),
            Decision::Replace { age: 50 }
        );
    }

    #[test]
    fn stuck_pattern_is_case_insensitive() {
        let re = Regex::new("(?i)stalled|metadata|no connections").unwrap();
        let mk = |m: Option<&str>| QueueItem {
            id: 1,
            download_id: Some("h".into()),
            title: "t".into(),
            error_message: m.map(str::to_string),
        };
        assert!(is_stuck(
            &re,
            &mk(Some("The download is Stalled with no connections"))
        ));
        assert!(is_stuck(&re, &mk(Some("Downloading metadata"))));
        assert!(!is_stuck(
            &re,
            &mk(Some("No files found are eligible for import"))
        ));
        assert!(!is_stuck(&re, &mk(None)));
    }
}
