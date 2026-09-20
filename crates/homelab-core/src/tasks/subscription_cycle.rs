//! Cycle des abonnés (`[subscriptions]`) : rappels J-N, passage en grâce à l'échéance, suspension
//! à la fin de la grâce, fiches créées pour les comptes qui n'en ont pas. Décisions pures dans
//! `subscriptions::decide`, application dans `subscription_ops::run_cycle`. `cycle_dry_run = true`
//! : tout est annoncé (journal + Discord admin), rien n'est appliqué ni envoyé aux membres.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::subscription_ops;

pub struct SubscriptionCycle;

#[async_trait]
impl Task for SubscriptionCycle {
    fn name(&self) -> &'static str {
        "subscription_cycle"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.subscription_cycle.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        if !ctx.cfg.subscriptions.enabled {
            return Ok(Report::new("désactivé", 0));
        }
        let (summary, actions) = subscription_ops::run_cycle(ctx).await?;
        Ok(Report::new(summary, actions))
    }
}
