//! Alertes pour l'admin : un seul point d'entrée, qui envoie par **mail** (`CHAT_ADMIN_EMAIL`, repli
//! `GUIDE_CONTACT_EMAIL`) **et** sur le salon Discord admin quand il est configuré. Jamais bloquant :
//! les échecs sont journalisés. Appelé par `hls_loop_watch`, `indexer_unblock`, `stack_health`, l'onboarding
//! (inscription à activer, mail de bienvenue non parti), le tchat (récapitulatif), le scheduler (tâche en
//! échec répété), `cert_watch`, `backup_watch` et les seuils de capacité.
//!
//! Chaque envoi laisse une trace (2026-10-07) : une ligne `alerte envoyée` dans le journal (objet et canaux,
//! jamais l'adresse, l'URL du webhook ni le corps) et une entrée dans `state.alerts` (affichée sur `/status.html`
//! et par `homelabctl status`). Avant, seuls les échecs d'envoi laissaient une ligne, et le journal ne remontait
//! qu'à trois jours : impossible de dire si l'alerte indexeur du 01/10 était partie.
use crate::context::TaskContext;
use crate::discord::{self, Channel, Embed};
use crate::mail;
use crate::state::now;

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
    let mail_configured = ctx.secrets.smtp.is_some() && ctx.secrets.chat_admin_email.is_some();
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
    let discord_configured =
        ctx.cfg.discord.admin_alerts && discord::webhook_for(ctx, Channel::Admin).is_some();
    if ctx.cfg.discord.admin_alerts {
        let embed = match level {
            Level::Info => Embed::info(subject, body),
            Level::Warn => Embed::warn(subject, body),
            Level::Error => Embed::error(subject, body),
        };
        posted = discord::notify(ctx, Channel::Admin, embed).await;
    }
    trace(
        ctx,
        subject,
        mailed,
        posted,
        mail_configured || discord_configured,
    )
    .await;
    (mailed, posted)
}

/// Au moins un canal a pris l'alerte : l'appelant qui note « déjà signalé » avant d'envoyer peut s'en servir pour
/// réessayer au passage suivant quand rien n'est parti.
pub fn delivered(sent: (bool, bool)) -> bool {
    sent.0 || sent.1
}

/// Journal et état d'un envoi. Rien en dry-run (rien n'est parti) ; l'objet seul est écrit, jamais le corps
/// (il porte parfois un lien de bienvenue).
async fn trace(ctx: &TaskContext, subject: &str, mailed: bool, posted: bool, configured: bool) {
    if ctx.dry_run {
        return;
    }
    if mailed || posted {
        tracing::info!(task = "alerts", subject, mailed, posted, "alerte envoyée");
    } else if configured {
        tracing::warn!(
            task = "alerts",
            subject,
            "alerte NON livrée : aucun canal n'a abouti"
        );
    } else {
        tracing::warn!(
            task = "alerts",
            subject,
            "alerte sans canal : ni mail admin (SMTP + CHAT_ADMIN_EMAIL) ni Discord admin configurés"
        );
    }
    let at = now();
    let subject = subject.to_string();
    if let Err(e) = ctx
        .state
        .update(|s| s.alerts.record(at, &subject, mailed, posted))
        .await
    {
        tracing::warn!(task = "alerts", error = %e, "trace de l'alerte non enregistrée");
    }
}

/// Ce qu'un relevé de capacité (disque, quota) demande à ce passage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crossing {
    Nothing,
    /// Le seuil vient d'être franchi (ou l'alerte précédente n'est pas partie) : alerter.
    Alert,
    /// Repassé nettement sous le seuil : la prochaine montée alertera de nouveau.
    Rearm,
}

/// Une alerte par franchissement. `alerted` : alerte déjà partie pour la montée en cours. Le réarmement attend
/// `rearm_margin` points sous le seuil (84 % puis 86 % ne doivent pas faire deux messages).
pub fn crossing(pct: u8, threshold: u8, rearm_margin: u8, alerted: bool) -> Crossing {
    if threshold == 0 {
        return Crossing::Nothing; // 0 = seuil désactivé
    }
    if pct >= threshold && !alerted {
        Crossing::Alert
    } else if alerted && pct < threshold.saturating_sub(rearm_margin) {
        Crossing::Rearm
    } else {
        Crossing::Nothing
    }
}

/// Alerte de capacité : `key` (`disque_vps`, `quota_seedbox`) sert de mémoire dans `state.capacity_alerts`.
/// `detail` complète le message (ce qui occupe la place, ce qu'il reste à faire).
pub async fn capacity(
    ctx: &TaskContext,
    key: &str,
    label: &str,
    pct: u8,
    threshold: u8,
    detail: &str,
) {
    let margin = ctx.cfg.alerts.capacity_rearm_pts;
    let alerted = ctx
        .state
        .read(|s| s.capacity_alerts.contains_key(key))
        .await;
    match crossing(pct, threshold, margin, alerted) {
        Crossing::Nothing => {}
        Crossing::Rearm => {
            tracing::info!(
                task = "alerts",
                key,
                pct,
                threshold,
                "capacité repassée sous le seuil"
            );
            if !ctx.dry_run {
                let _ = ctx.state.update(|s| s.capacity_alerts.remove(key)).await;
            }
        }
        Crossing::Alert => {
            let subject = format!("{label} : {pct} % (seuil {threshold} %)");
            let body = format!(
                "{label} atteint {pct} % et franchit le seuil d'alerte de {threshold} %. {detail}\n\n\
                 Une seule alerte par franchissement : le prochain message viendra si la valeur \
                 repasse sous {} % puis remonte.",
                threshold.saturating_sub(margin)
            );
            let sent = admin(ctx, Level::Warn, &subject, &body).await;
            // rien n'est parti : on réessaiera au passage suivant plutôt que de croire l'admin prévenu
            if delivered(sent) && !ctx.dry_run {
                let at = now();
                let _ = ctx
                    .state
                    .update(|s| s.capacity_alerts.insert(key.to_string(), at))
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_alert_per_crossing() {
        // sous le seuil : rien
        assert_eq!(crossing(70, 85, 3, false), Crossing::Nothing);
        assert_eq!(crossing(84, 85, 3, false), Crossing::Nothing);
        // franchissement : une alerte
        assert_eq!(crossing(85, 85, 3, false), Crossing::Alert);
        assert_eq!(crossing(97, 85, 3, false), Crossing::Alert);
        // déjà signalé : silence tant que ça dure, même en oscillant juste sous le seuil
        assert_eq!(crossing(90, 85, 3, true), Crossing::Nothing);
        assert_eq!(crossing(84, 85, 3, true), Crossing::Nothing);
        assert_eq!(crossing(82, 85, 3, true), Crossing::Nothing);
        // nettement redescendu : réarmé
        assert_eq!(crossing(81, 85, 3, true), Crossing::Rearm);
        // puis une nouvelle montée alerte de nouveau
        assert_eq!(crossing(86, 85, 3, false), Crossing::Alert);
    }

    #[test]
    fn a_zero_threshold_disables_the_check() {
        assert_eq!(crossing(100, 0, 3, false), Crossing::Nothing);
        assert_eq!(crossing(0, 0, 3, true), Crossing::Nothing);
    }

    #[test]
    fn rearm_margin_cannot_underflow() {
        assert_eq!(crossing(0, 2, 5, true), Crossing::Nothing);
    }

    #[test]
    fn delivered_means_at_least_one_channel() {
        assert!(delivered((true, false)));
        assert!(delivered((false, true)));
        assert!(!delivered((false, false)));
    }
}
