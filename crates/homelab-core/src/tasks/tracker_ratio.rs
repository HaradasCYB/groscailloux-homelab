//! Share limits par tracker (ex `qbit-tracker-ratio-policy.sh`, toutes les 30 min).
//!   privé prioritaire (c411)      → illimité
//!   privé secondaire / publics    → ratio 2.0, 14 j
//!   défaut                        → ratio 1.0, 7 j

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::{Config, TrackerRatio as Cfg};
use crate::context::TaskContext;

pub struct TrackerRatio;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub tier: &'static str,
    pub ratio: f64,
    pub time_min: i64,
}

pub fn policy_for(cfg: &Cfg, tracker: &str) -> Policy {
    let hit = |list: &[String]| list.iter().any(|p| tracker.contains(p.as_str()));
    if hit(&cfg.unlimited) {
        Policy {
            tier: "c411",
            ratio: -1.0,
            time_min: -1,
        }
    } else if hit(&cfg.secondary) {
        Policy {
            tier: "ygg",
            ratio: cfg.public_ratio,
            time_min: cfg.public_time_min,
        }
    } else if hit(&cfg.public) {
        Policy {
            tier: "public",
            ratio: cfg.public_ratio,
            time_min: cfg.public_time_min,
        }
    } else {
        Policy {
            tier: "default",
            ratio: cfg.default_ratio,
            time_min: cfg.default_time_min,
        }
    }
}

fn same_ratio(a: f64, b: f64) -> bool {
    (a * 100.0).round() == (b * 100.0).round()
}

#[async_trait]
impl Task for TrackerRatio {
    fn name(&self) -> &'static str {
        "tracker_ratio"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.tracker_ratio.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.tracker_ratio;
        let _guard = ctx.qbit_lock.lock().await;
        ctx.qbit.version().await?;
        let torrents = ctx.qbit.torrents().await?;
        let mut changes = 0u32;
        for t in &torrents {
            let p = policy_for(cfg, &t.tracker);
            if same_ratio(t.ratio_limit, p.ratio) && t.seeding_time_limit == p.time_min {
                continue;
            }
            let name: String = t.name.chars().take(60).collect();
            if ctx.dry_run {
                info!(task = "tracker_ratio", tier = p.tier, ratio = p.ratio, time = p.time_min, hash = %t.hash, %name, "dry-run: would apply");
                changes += 1;
                continue;
            }
            match ctx
                .qbit
                .set_share_limits(&t.hash, p.ratio, p.time_min)
                .await
            {
                Ok(()) => {
                    changes += 1;
                    info!(task = "tracker_ratio", tier = p.tier, ratio = p.ratio, time = p.time_min, hash = %t.hash, %name, "applied");
                }
                Err(e) => {
                    warn!(task = "tracker_ratio", hash = %t.hash, %name, error = %e, "set_limits_failed")
                }
            }
        }
        Ok(Report::new(
            format!("changes={changes} total_torrents={}", torrents.len()),
            changes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        let cfg = Cfg::default();
        assert_eq!(policy_for(&cfg, "https://c411.org/announce").tier, "c411");
        assert_eq!(policy_for(&cfg, "https://c411.org/announce").ratio, -1.0);
        assert_eq!(policy_for(&cfg, "http://yggleak.xyz/a").tier, "ygg");
        assert_eq!(
            policy_for(&cfg, "udp://tracker.opentrackr.org:1337").tier,
            "public"
        );
        assert_eq!(
            policy_for(&cfg, "udp://tracker.opentrackr.org:1337").time_min,
            20160
        );
        let d = policy_for(&cfg, "https://unknown.example/announce");
        assert_eq!((d.tier, d.ratio, d.time_min), ("default", 1.0, 10080));
        assert_eq!(policy_for(&cfg, "").tier, "default");
    }

    #[test]
    fn ratio_comparison_is_two_decimals() {
        assert!(same_ratio(2.0, 2.004));
        assert!(!same_ratio(1.0, 2.0));
        assert!(same_ratio(-1.0, -1.0));
    }
}
