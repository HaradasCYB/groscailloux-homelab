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
use crate::state::{now, WatchAlert};

/// Début de l'objet des alertes « tâche en échec répété » (scheduler) : sert aussi à les compter dans
/// `state.alerts.recent` pour le plafond `[alerts] fail_alerts_max`.
pub const STREAK_SUBJECT: &str = "Tâche en échec répété";

/// Niveau, pour la couleur Discord.
#[derive(Debug, Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Au moins un canal d'alerte est configuré : mail admin complet (SMTP + `CHAT_ADMIN_EMAIL`) ou Discord admin
/// activé avec son webhook. Sans canal, une alerte non livrée ne se réessaie pas : rien ne changera au passage suivant.
pub fn configured(ctx: &TaskContext) -> bool {
    let mail = ctx.secrets.smtp.is_some() && ctx.secrets.chat_admin_email.is_some();
    let discord =
        ctx.cfg.discord.admin_alerts && discord::webhook_for(ctx, Channel::Admin).is_some();
    mail || discord
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
                Err(e) => tracing::warn!(
                    task = "alerts",
                    error = format!("{e:#}"),
                    "mail admin en échec"
                ),
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
    trace(ctx, subject, mailed, posted, configured(ctx)).await;
    (mailed, posted)
}

/// Au moins un canal a pris l'alerte : l'appelant qui note « déjà signalé » avant d'envoyer peut s'en servir pour
/// réessayer au passage suivant quand rien n'est parti.
pub fn delivered(sent: (bool, bool)) -> bool {
    sent.0 || sent.1
}

/// Faut-il réessayer cette alerte plus tard ? Oui si rien n'est parti **alors qu'un canal est configuré** (panne
/// d'envoi, probablement passagère). Sans canal configuré, réessayer à chaque passage ne ferait qu'écrire un
/// avertissement et une ligne « non livrée » de plus à chaque fois.
pub fn retry_later(sent: (bool, bool), configured: bool) -> bool {
    !delivered(sent) && configured
}

/// Délai avant de répéter l'alerte d'une surveillance quotidienne pour le **même** défaut (2026-10-08). Un peu
/// moins de 24 h : le passage planifié du lendemain (intervalle de 24 h) passe toujours, un redémarrage de homelabd
/// dans la journée jamais.
pub const WATCH_REPEAT_SECS: i64 = 20 * 3600;

/// Faut-il envoyer l'alerte d'une surveillance quotidienne ? `last` : dernière alerte partie pour cette tâche.
/// Oui s'il n'y en a pas, si le défaut n'est plus le même (`key` différente), ou si elle date d'au moins
/// [`WATCH_REPEAT_SECS`]. Une date dans le futur (horloge reculée) ne retient pas l'alerte.
pub fn watch_due(last: Option<&WatchAlert>, key: &str, ts: i64) -> bool {
    !last.is_some_and(|w| w.key == key && (0..WATCH_REPEAT_SECS).contains(&(ts - w.at)))
}

/// Empreinte courte et stable d'un défaut décrit par plusieurs lignes (SHA-256, 16 premiers caractères hex) : deux
/// passages qui relèvent les mêmes lignes ont la même, un défaut différent une autre.
pub fn fingerprint(parts: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]); // « ab|c » ≠ « a|bc »
    }
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Alerte d'une surveillance quotidienne (`cert_watch`, `backup_watch`, `diun_watch`) **avec mémoire** : ces tâches
/// tournent à chaque démarrage de homelabd et réalertaient le même défaut à chaque redémarrage. `task` : nom de la
/// tâche ; `key` : identité du défaut (voir [`watch_due`]). Rend `false` si l'alerte a été retenue (même défaut déjà
/// signalé il y a moins de [`WATCH_REPEAT_SECS`]), `true` si elle a été envoyée — ou tentée : rien n'est noté quand un
/// canal configuré n'a pas abouti (`retry_later`), la tâche réessaiera à son passage suivant.
pub async fn watch(
    ctx: &TaskContext,
    task: &str,
    key: &str,
    level: Level,
    subject: &str,
    body: &str,
) -> bool {
    let ts = now();
    let last = ctx.state.read(|s| s.watch_alerts.get(task).cloned()).await;
    if !watch_due(last.as_ref(), key, ts) {
        tracing::info!(
            task,
            "défaut déjà signalé il y a moins de 20 h : pas de nouvelle alerte"
        );
        return false;
    }
    let sent = admin(ctx, level, subject, body).await;
    if !retry_later(sent, configured(ctx)) && !ctx.dry_run {
        let (task, key) = (task.to_string(), key.to_string());
        if let Err(e) = ctx
            .state
            .update(|s| {
                s.watch_alerts.insert(task, WatchAlert { at: ts, key });
            })
            .await
        {
            tracing::warn!(
                task = "alerts",
                error = format!("{e:#}"),
                "mémoire de l'alerte non enregistrée"
            );
        }
    }
    true
}

/// La surveillance `task` ne relève plus rien : oublie sa dernière alerte, pour qu'une rechute alerte normalement.
/// N'écrit rien quand il n'y avait rien à oublier.
pub async fn watch_clear(ctx: &TaskContext, task: &str) {
    if ctx.dry_run || !ctx.state.read(|s| s.watch_alerts.contains_key(task)).await {
        return;
    }
    let _ = ctx.state.update(|s| s.watch_alerts.remove(task)).await;
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
        tracing::warn!(
            task = "alerts",
            error = format!("{e:#}"),
            "trace de l'alerte non enregistrée"
        );
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
            // rien n'est parti alors qu'un canal existe : on réessaiera au passage suivant plutôt que de croire
            // l'admin prévenu. Sans canal configuré (2026-10-08) on marque quand même : rien ne changera au passage
            // suivant, et une écriture d'état + un avertissement à chaque passage n'avanceraient à rien.
            if !retry_later(sent, configured(ctx)) && !ctx.dry_run {
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
    fn an_undelivered_alert_is_retried_only_when_a_channel_exists() {
        // canal configuré mais en échec : on réessaiera
        assert!(retry_later((false, false), true));
        // aucun canal : inutile de réessayer à chaque passage
        assert!(!retry_later((false, false), false));
        // partie par au moins un canal : terminé
        assert!(!retry_later((true, false), true));
        assert!(!retry_later((false, true), true));
    }

    fn watch_at(at: i64, key: &str) -> WatchAlert {
        WatchAlert {
            at,
            key: key.into(),
        }
    }

    #[test]
    fn a_daily_watch_does_not_repeat_the_same_defect_after_a_restart() {
        let h = 3600;
        let last = watch_at(1_000_000, "stale");
        // aucune alerte connue : on alerte
        assert!(watch_due(None, "stale", 1_000_000));
        // redémarrage de homelabd 5 min, 2 h, 19 h 59 après l'alerte : silence
        assert!(!watch_due(Some(&last), "stale", 1_000_000 + 300));
        assert!(!watch_due(Some(&last), "stale", 1_000_000 + 2 * h));
        assert!(!watch_due(Some(&last), "stale", 1_000_000 + 20 * h - 1));
        // 20 h écoulées (passage planifié du lendemain, intervalle de 24 h) : on reprévient
        assert!(watch_due(Some(&last), "stale", 1_000_000 + 20 * h));
        assert!(watch_due(Some(&last), "stale", 1_000_000 + 24 * h));
    }

    #[test]
    fn a_different_defect_alerts_at_once() {
        let last = watch_at(1_000_000, "stale");
        assert!(watch_due(Some(&last), "missing", 1_000_060));
        // même défaut mais dont l'empreinte a changé (autres lignes en erreur)
        assert!(watch_due(
            Some(&watch_at(1_000_000, &fingerprint(&["a".into()]))),
            &fingerprint(&["a".into(), "b".into()]),
            1_000_060
        ));
    }

    #[test]
    fn a_clock_set_back_does_not_mute_the_watch_for_ever() {
        // dernière alerte « dans le futur » : elle ne retient pas l'alerte
        let last = watch_at(2_000_000, "stale");
        assert!(watch_due(Some(&last), "stale", 1_000_000));
    }

    #[test]
    fn fingerprints_are_stable_and_distinguish_the_lines() {
        let a = fingerprint(&["ligne 4 : clé en double".into(), "ligne 9 : x".into()]);
        assert_eq!(a.len(), 16);
        assert_eq!(
            a,
            fingerprint(&["ligne 4 : clé en double".into(), "ligne 9 : x".into()])
        );
        assert_ne!(a, fingerprint(&["ligne 4 : clé en double".into()]));
        // la frontière entre deux lignes compte
        assert_ne!(
            fingerprint(&["ab".into(), "c".into()]),
            fingerprint(&["a".into(), "bc".into()])
        );
    }

    #[test]
    fn delivered_means_at_least_one_channel() {
        assert!(delivered((true, false)));
        assert!(delivered((false, true)));
        assert!(!delivered((false, false)));
    }
}
