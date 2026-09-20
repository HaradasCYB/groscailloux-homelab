//! Alertes pour l'admin : un seul point d'entrée, qui envoie par **mail** (`CHAT_ADMIN_EMAIL`, repli
//! `GUIDE_CONTACT_EMAIL`) **et** sur le salon Discord admin quand il est configuré. Jamais bloquant :
//! les échecs sont journalisés. Appelé par `hls_loop_watch`, `indexer_unblock`, `stack_health`, l'onboarding
//! (inscription à activer, mail de bienvenue non parti) et le tchat (récapitulatif).
use crate::context::TaskContext;
use crate::discord::{self, Channel, Embed};
use crate::mail;

/// Niveau, pour la couleur Discord.
#[derive(Debug, Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Envoie `subject`/`body` par mail à l'admin et l'embed correspondant sur Discord (si activé dans
/// `[discord] admin_alerts`). Renvoie (mail envoyé, discord envoyé).
pub async fn admin(ctx: &TaskContext, level: Level, subject: &str, body: &str) -> (bool, bool) {
    let mut mailed = false;
    if let (Some(smtp), Some(to)) = (&ctx.secrets.smtp, &ctx.secrets.chat_admin_email) {
        if ctx.dry_run {
            tracing::info!(task = "alerts", subject, "dry-run: admin mail not sent");
        } else {
            match mail::send_plain(smtp, "Admin Groscailloux", to, subject, body).await {
                Ok(()) => mailed = true,
                Err(e) => tracing::warn!(task = "alerts", error = %e, "mail admin en échec"),
            }
        }
    }
    let mut posted = false;
    if ctx.cfg.discord.admin_alerts {
        let embed = match level {
            Level::Info => Embed::info(subject, body),
            Level::Warn => Embed::warn(subject, body),
            Level::Error => Embed::error(subject, body),
        };
        posted = discord::notify(ctx, Channel::Admin, embed).await;
    }
    (mailed, posted)
}
