//! Réconciliation PayPal (quotidienne) : pour chaque fiche rattachée à un abonnement, relit son
//! statut et sa facturation chez PayPal et rejoue un paiement manqué (webhook perdu, daemon
//! arrêté). Un paiement n'est appliqué que s'il est constaté (dernier paiement PayPal plus récent que
//! celui déjà appliqué, noté sur la fiche) ; facturation due sans paiement = alerte admin, rien n'est
//! prolongé ; annulation, suspension ou expiration = notée, l'accès va jusqu'à l'échéance payée.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::subscription_ops;

pub struct SubscriptionReconcile;

#[async_trait]
impl Task for SubscriptionReconcile {
    fn name(&self) -> &'static str {
        "subscription_reconcile"
    }

    fn label(&self) -> &'static str {
        "Contrôle PayPal"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.subscription_reconcile.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        if !ctx.cfg.subscriptions.enabled {
            return Ok(Report::new("désactivé", 0));
        }
        let (summary, actions) = subscription_ops::reconcile(ctx).await?;
        Ok(Report::new(summary, actions))
    }
}
