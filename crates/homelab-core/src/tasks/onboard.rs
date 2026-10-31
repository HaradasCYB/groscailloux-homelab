//! Onboarding unifié (ex `homelab-onboard-user.sh` + `jellyfin-create-user.sh`) :
//! compte Jellyfin (source de vérité du mot de passe) → import Jellyseerr →
//! email Jellyseerr → mail de bienvenue. Sérialisé par `onboard_lock` : le poller
//! et l'UI web ne peuvent plus se marcher dessus. Le mot de passe n'est jamais loggé.
//! Si `accounts.new_accounts_premium = false`, le compte est ensuite suspendu (non-premium) :
//! l'admin l'active depuis la page « Comptes » (voir `accounts`).

use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use regex::Regex;
use serde_json::Value;
use tracing::{info, warn};

use crate::accounts::{self, Outcome};
use crate::clients::jellyfin::non_admin_policy;
use crate::context::TaskContext;
use crate::secret::Secret;
use crate::subscription_ops::{self, TrialBlock, TrialOutcome};
use crate::vo_native;
use crate::welcome::{self, WelcomeNotice};

/// D'où vient la demande : formulaire admin / CLI, page publique d'inscription, ou « Add User » Jellyseerr repris
/// par `user_poller`. Une inscription publique démarre l'essai gratuit (`[subscriptions] trial_days`) puis prévient
/// l'admin par mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Admin,
    SelfSignup,
    Poller,
}

#[derive(Debug, Clone)]
pub struct OnboardRequest {
    pub username: String,
    pub email: String,
    /// Mot de passe imposé par l'admin ; sinon un aléa jamais communiqué, que le membre remplace via le lien.
    pub password: Option<Secret>,
    pub source: Source,
    /// Code de parrainage saisi à l'inscription publique (ignoré pour les autres sources).
    pub referral: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OnboardResult {
    pub username: String,
    pub email: String,
    /// Seulement si l'admin l'a imposé ; sinon le membre le définit sur la page de bienvenue.
    pub password: Option<Secret>,
    pub jellyfin_id: String,
    pub jellyseerr_id: i64,
    pub jellyfin_url: String,
    pub jellyseerr_url: String,
    /// Mail de bienvenue (lien) parti.
    pub mail_sent: bool,
    /// URL du lien de bienvenue (à transmettre à la main si le mail n'est pas parti ; jamais journalisée).
    pub link_url: Option<String>,
    pub link_expires_at: Option<i64>,
    /// Compte actif dès la création ; `false` = suspendu, en attente d'activation.
    pub premium: bool,
    /// Essai de l'inscription publique, décidé avant les mails ; `None` pour les autres sources (et en dry-run).
    pub trial: Option<TrialOutcome>,
    pub dry_run: bool,
}

pub fn valid_email(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$").unwrap())
        .is_match(s)
}

pub fn valid_username(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-zA-Z0-9_-]{2,32}$").unwrap())
        .is_match(s)
}

pub fn generate_password() -> Secret {
    let s: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    Secret::new(s)
}

pub async fn run(ctx: &TaskContext, req: OnboardRequest) -> Result<OnboardResult> {
    if !valid_username(&req.username) {
        bail!("username invalide (2-32 caractères, lettres/chiffres/_-)");
    }
    if !valid_email(&req.email) {
        bail!("email invalide");
    }
    let email = req.email.to_lowercase();
    let imposed = req.password.clone();
    let password = req.password.unwrap_or_else(generate_password);
    let _guard = ctx.onboard_lock.lock().await;
    let s = &ctx.secrets;

    ctx.jellyfin
        .system_info()
        .await
        .context("Jellyfin injoignable ou clé invalide")?;
    ctx.jellyseerr
        .status()
        .await
        .context("Jellyseerr injoignable ou clé invalide")?;

    if let Some(u) = ctx.jellyfin.find_user(&req.username).await? {
        bail!(
            "user Jellyfin '{}' existe déjà (Id={})",
            req.username,
            u.get("Id").and_then(Value::as_str).unwrap_or("?")
        );
    }
    let js_users = ctx.jellyseerr.users(1000).await?;
    if let Some(u) = js_users.iter().find(|u| {
        u.get("email")
            .and_then(Value::as_str)
            .map(|e| e.to_lowercase() == email)
            .unwrap_or(false)
    }) {
        bail!(
            "email '{}' déjà utilisé côté Jellyseerr (id={})",
            email,
            u.get("id").and_then(Value::as_i64).unwrap_or(0)
        );
    }

    let mut result = OnboardResult {
        username: req.username.clone(),
        email: email.clone(),
        password: imposed,
        jellyfin_id: String::new(),
        jellyseerr_id: 0,
        jellyfin_url: s.jellyfin_public_url.clone(),
        jellyseerr_url: s.jellyseerr_public_url.clone(),
        mail_sent: false,
        link_url: None,
        link_expires_at: None,
        premium: ctx.cfg.accounts.new_accounts_premium,
        trial: None,
        dry_run: ctx.dry_run,
    };
    if ctx.dry_run {
        info!(task = "onboard", username = %req.username, email = %email, "dry-run: pre-checks OK, would create user");
        return Ok(result);
    }

    let jf_id = ctx.jellyfin.create_user(&req.username, &password).await?;
    info!(task = "onboard", username = %req.username, jellyfin_id = %jf_id, "jellyfin user created");
    let libraries: Vec<String> = [s.jellyfin_lib_films.clone(), s.jellyfin_lib_series.clone()]
        .into_iter()
        .chain(s.jellyfin_lib_extra.iter().cloned())
        .collect();
    if let Err(e) = ctx
        .jellyfin
        .set_policy(
            &jf_id,
            &non_admin_policy(&libraries, ctx.cfg.accounts.max_devices_per_user),
        )
        .await
    {
        warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "policy not applied, fix in Jellyfin dashboard");
    }
    let latest_excludes = ctx
        .jellyfin
        .library_ids_of_type("boxsets")
        .await
        .unwrap_or_default();
    if let Err(e) = ctx
        .jellyfin
        .set_view_order(&jf_id, &libraries, &latest_excludes)
        .await
    {
        warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "library order not applied");
    }
    let skip = &ctx.cfg.accounts;
    if let Err(e) = ctx
        .jellyfin
        .set_skip_lengths(&jf_id, skip.skip_forward_ms, skip.skip_back_ms)
        .await
    {
        warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "skip lengths not applied");
    }
    // langue d'un compte neuf : `fr` avec les réglages d'origine (`fre` + `Smart`) ; si l'onboarding est réglé sur la
    // VO, mêmes règles que Mon compte (« Langue d'origine » quand `vo_native = true` : un compte neuf n'a rien lu, la
    // garde `vo_native_guard` le ramène sur `vo_audio_language` dès sa première session ou son premier appareil sur un
    // client hors liste)
    let mode = vo_native::onboarding_mode(skip);
    let mut prefs = vo_native::prefs_for(mode, skip, false);
    let mut native = false;
    if mode == vo_native::Mode::Vo {
        let (audio, outcome) = vo_native::vo_audio(skip, &[]);
        native = outcome == vo_native::Outcome::Native;
        prefs.audio = audio;
    }
    match ctx
        .jellyfin
        .set_language_prefs(
            &jf_id,
            &prefs.audio,
            &prefs.subtitles,
            &prefs.subtitle_mode,
            false,
        )
        .await
    {
        Ok(()) => {
            if native {
                let rec = crate::state::VoNativeRecord {
                    at: crate::state::now(),
                    name: req.username.clone(),
                    source: "onboarding".into(),
                    outcome: vo_native::Outcome::Native.as_str().into(),
                    old: String::new(),
                    new: prefs.audio.clone(),
                    clients: Vec::new(),
                };
                if let Err(e) = vo_native::record(&ctx.state, &jf_id, rec).await {
                    warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "VO « Langue d'origine » non notée dans l'état");
                }
            }
        }
        Err(e) => {
            warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "language preferences not applied");
        }
    }
    result.jellyfin_id = jf_id.clone();

    let js_id = match import_into_jellyseerr(ctx, &jf_id).await {
        Ok(id) => id,
        Err(e) => {
            // compte Jellyfin créé, mais ni Jellyseerr, ni essai, ni lien : une inscription publique ne doit pas
            // rester à moitié faite sans que l'admin le sache (le membre qui réessaie bute sur « existe déjà »)
            if req.source == Source::SelfSignup {
                let (subject, body) = half_signup_mail(&req.username, &format!("{e:#}"));
                let sent =
                    crate::alerts::admin(ctx, crate::alerts::Level::Warn, &subject, &body).await;
                if crate::alerts::retry_later(sent, crate::alerts::configured(ctx)) {
                    warn!(task = "onboard", username = %req.username, "inscription ratée NON signalée à l'admin");
                }
            }
            return Err(e);
        }
    };
    result.jellyseerr_id = js_id;
    info!(task = "onboard", username = %req.username, jellyseerr_id = js_id, "imported into jellyseerr");

    if let Err(e) = ctx
        .jellyseerr
        .set_main_settings(js_id, &email, &req.username)
        .await
    {
        warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "set email failed, fix manually");
    }

    // droits de l'import (defaultPermissions) + validation automatique : posés avant la suspension,
    // qui les sauvegarde pour l'activation
    let bits = accounts::granted_bits(&ctx.cfg.accounts);
    if bits != 0 {
        let perms = ctx
            .jellyseerr
            .users(1000)
            .await
            .ok()
            .and_then(|us| {
                us.iter()
                    .find(|u| u.get("id").and_then(Value::as_i64) == Some(js_id))
                    .and_then(|u| u.get("permissions").and_then(Value::as_i64))
            })
            .unwrap_or(0);
        let wanted = accounts::request_permissions(perms, bits);
        if wanted != perms {
            match ctx.jellyseerr.set_permissions(js_id, wanted).await {
                Ok(()) => {
                    info!(task = "onboard", username = %req.username, permissions = wanted, "jellyseerr auto-approve set")
                }
                Err(e) => {
                    warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "auto-approve not set, fix in Jellyseerr")
                }
            }
        }
    }

    // suspendu après l'import : Jellyseerr doit trouver le compte, et ses droits sont sauvegardés
    if !ctx.cfg.accounts.new_accounts_premium {
        match accounts::set_premium_locked(ctx, &jf_id, false).await {
            Ok(Outcome::Suspended) => {
                info!(task = "onboard", username = %req.username, "account created suspended (not premium)")
            }
            Ok(o) => {
                result.premium = true;
                warn!(task = "onboard", username = %req.username, outcome = ?o, "account not suspended")
            }
            Err(e) => {
                result.premium = true;
                warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "suspend failed, account is active")
            }
        }
    }

    // inscription publique : l'essai est décidé (et le compte activé) AVANT les mails, qui disent ce qui s'est
    // vraiment passé (1.30.5 : le membre lisait « sera activé par l'administrateur » et l'admin « à activer »
    // alors que l'essai venait d'activer le compte)
    if req.source == Source::SelfSignup {
        let referral = req
            .referral
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        let trial = subscription_ops::start_trial(ctx, &jf_id, &req.username, referral).await;
        if trial.started() {
            result.premium = true;
        } else if let TrialOutcome::NotStarted {
            block: TrialBlock::Failed(_),
            ..
        } = &trial
        {
            // l'activation a pu partir avant l'erreur : les mails disent l'état réel du compte
            match accounts::account_is_premium(ctx, &jf_id).await {
                Ok(p) => result.premium = p,
                Err(e) => {
                    warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "état du compte non relu après l'essai raté")
                }
            }
        }
        result.trial = Some(trial);
    }

    // lien de bienvenue : le membre définit son mot de passe sur la page, rien de sensible dans le mail
    let notice = welcome_notice(result.trial.as_ref(), result.premium);
    match welcome::send_with(
        ctx,
        &jf_id,
        &req.username,
        &email,
        welcome::KIND_WELCOME,
        notice,
    )
    .await
    {
        Ok(sent) => {
            result.mail_sent = sent.mail_sent;
            result.link_expires_at = Some(sent.expires_at);
            result.link_url = Some(sent.url);
            if sent.mail_sent {
                info!(task = "onboard", username = %req.username, "welcome link mailed");
            } else {
                warn!(task = "onboard", username = %req.username, "welcome link NOT mailed, hand the link over manually");
            }
        }
        Err(e) => {
            warn!(task = "onboard", username = %req.username, error = format!("{e:#}"), "welcome link failed")
        }
    }
    if let Some(trial) = &result.trial {
        notify_admin_signup(ctx, &req.username, trial, result.premium, result.mail_sent).await;
    }
    Ok(result)
}

/// Import du compte Jellyfin dans Jellyseerr : identifiant Jellyseerr.
async fn import_into_jellyseerr(ctx: &TaskContext, jf_id: &str) -> Result<i64> {
    let imported = ctx.jellyseerr.import_from_jellyfin(&[jf_id]).await?;
    let mut js_id = find_js_id(&imported, jf_id);
    if js_id.is_none() {
        js_id = find_js_id(&Value::Array(ctx.jellyseerr.users(1000).await?), jf_id);
    }
    js_id.context("user Jellyseerr introuvable après import")
}

/// Mail de l'admin quand une inscription publique s'arrête après la création du compte Jellyfin (fonction pure).
pub fn half_signup_mail(username: &str, error: &str) -> (String, String) {
    (
        format!("Inscription ratée à moitié : {username}"),
        format!(
            "L'inscription de {username} depuis la page publique s'est arrêtée après la création du compte Jellyfin : {error}.\n\nNi compte Jellyseerr, ni essai, ni lien de bienvenue. Le membre a vu « réessaie » mais son pseudo est désormais pris : supprimer le compte Jellyfin à moitié créé (/accounts) pour qu'il puisse recommencer, ou finir l'import à la main.\n"
        ),
    )
}

/// Encadré du mail de bienvenue : essai démarré → « ton essai de N jours a commencé » ; sinon compte actif → rien,
/// compte suspendu → « sera activé par l'administrateur ».
pub fn welcome_notice(trial: Option<&TrialOutcome>, premium: bool) -> WelcomeNotice {
    match trial {
        Some(TrialOutcome::Started { days, .. }) => WelcomeNotice::Trial { days: *days },
        _ => welcome::notice_for(premium),
    }
}

/// Jour et mois (heure locale du VPS) d'une fin d'essai.
fn day_month(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%d/%m").to_string())
        .unwrap_or_else(|| "?".into())
}

/// Objet et corps du mail de l'admin pour une inscription publique (fonction pure). `accounts` = lien de la page
/// Comptes ; `premium` = compte actif à l'envoi ; `mail_sent` = le lien de bienvenue est bien parti.
pub fn signup_admin_mail(
    username: &str,
    trial: &TrialOutcome,
    premium: bool,
    mail_sent: bool,
    accounts: &str,
) -> (String, String) {
    let head =
        format!("Un nouveau compte vient d'être créé depuis la page d'inscription : {username}.");
    let link = if mail_sent {
        "Le membre a reçu son lien pour définir son mot de passe."
    } else {
        "Mail de bienvenue NON parti : le membre n'a aucun lien. Renvoyer le lien depuis /accounts (« Renvoyer le lien », colonne « Lien de bienvenue »)."
    };
    let sponsor = trial.sponsor().line();
    match trial {
        TrialOutcome::Started { days, ends_at, .. } => {
            let todo = if mail_sent {
                format!("Rien à faire : le compte est actif. {link}")
            } else {
                format!("Le compte est actif. {link}")
            };
            (
                format!("Nouveau membre en essai : {username}"),
                format!(
                    "{head}\n\nEssai gratuit de {days} jours, jusqu'au {end}. {sponsor}\n\n{todo}\n\nLa fin de l'essai est suivie par le cycle des abonnements ({accounts}).\n",
                    end = day_month(*ends_at),
                ),
            )
        }
        TrialOutcome::NotStarted { block, .. } => {
            let why = block.reason();
            let fiche = match block {
                TrialBlock::CapReached { .. } | TrialBlock::NoTrialDays => {
                    " Sa fiche d'abonné est « suspendue » : choisir son statut (essai, offert, abonné) en l'activant."
                }
                _ => "",
            };
            if premium {
                (
                    format!("Nouveau compte : {username}"),
                    format!(
                        "{head}\n\nEssai impossible : {why}. {sponsor}\n\nLe compte est pourtant actif : à qualifier sur {accounts}\n\n{link}\n"
                    ),
                )
            } else {
                let next = if mail_sent {
                    " ; il recevra un second mail à l'activation"
                } else {
                    ""
                };
                (
                    format!("Nouveau compte à activer : {username}"),
                    format!(
                        "{head}\n\nEssai impossible : {why}. {sponsor}\n\nIl est suspendu tant que tu ne l'actives pas : {accounts}{fiche}\n\n{link_}{next}.\n",
                        link_ = link.trim_end_matches('.'),
                    ),
                )
            }
        }
    }
}

/// Niveau de l'alerte : avertissement si l'admin a quelque chose à rattraper (erreur, lien non parti).
pub fn signup_admin_level(trial: &TrialOutcome, mail_sent: bool) -> crate::alerts::Level {
    match trial {
        TrialOutcome::NotStarted {
            block: TrialBlock::Failed(_),
            ..
        } => crate::alerts::Level::Warn,
        _ if !mail_sent => crate::alerts::Level::Warn,
        _ => crate::alerts::Level::Info,
    }
}

/// Inscription publique : l'admin est prévenu, avec l'issue de l'essai (rien à faire, ou compte à activer).
async fn notify_admin_signup(
    ctx: &TaskContext,
    username: &str,
    trial: &TrialOutcome,
    premium: bool,
    mail_sent: bool,
) {
    let s = &ctx.secrets;
    let accounts = s
        .onboard_public_url
        .as_deref()
        .map(|u| format!("{}/accounts", u.trim_end_matches('/')))
        .unwrap_or_else(|| "la page Comptes (Homarr)".to_string());
    let (subject, body) = signup_admin_mail(username, trial, premium, mail_sent, &accounts);
    let level = signup_admin_level(trial, mail_sent);
    let sent = crate::alerts::admin(ctx, level, &subject, &body).await;
    if crate::alerts::retry_later(sent, crate::alerts::configured(ctx)) {
        // ni mail ni Discord : l'inscription reste visible sur /accounts, mais personne n'est prévenu
        warn!(task = "onboard", username = %username, subject = %subject, "inscription NON signalée à l'admin (mail et Discord en échec)");
    }
}

fn find_js_id(v: &Value, jellyfin_id: &str) -> Option<i64> {
    let list: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    list.into_iter()
        .find(|u| u.get("jellyfinUserId").and_then(Value::as_str) == Some(jellyfin_id))
        .and_then(|u| u.get("id").and_then(Value::as_i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validators() {
        assert!(valid_email("a.b+c@ex-ample.io"));
        assert!(!valid_email("nope@"));
        assert!(valid_username("hippo_42"));
        assert!(!valid_username("a"));
        assert!(!valid_username("with space"));
    }

    #[test]
    fn password_is_16_alnum() {
        let p = generate_password();
        assert_eq!(p.expose().len(), 16);
        assert!(p.expose().chars().all(|c| c.is_ascii_alphanumeric()));
    }

    /// Les cas d'une inscription publique : mail de l'admin (objet, contenu, niveau), encadré du mail du membre,
    /// texte de la page qui suit. La décision elle-même : `subscription_ops::tests::trial_decision_*`.
    #[test]
    fn signup_messages_follow_the_trial_decided_first() {
        use crate::alerts::Level;
        use crate::subscription_ops::Sponsor;
        const LINK: &str = "https://x.test/bienvenue/Xq3vN8pL2rT6yW0aZ4cE7gJ1kM5oR9sU2wY6bD8fH0j";
        const ACCOUNTS: &str = "https://x.test/accounts";
        let ends_at = 1_792_670_400; // 2026-10-22 12:00 UTC
        let started = |sponsor| TrialOutcome::Started {
            days: 7,
            ends_at,
            sponsor,
        };
        let not_started = |block, sponsor| TrialOutcome::NotStarted { block, sponsor };
        let member_text = |trial: &TrialOutcome, premium: bool| {
            let notice = welcome_notice(Some(trial), premium);
            welcome::render_with(welcome::KIND_WELCOME, "léa", LINK, 60, notice).text
        };
        // essai démarré : (parrain, phrase attendue)
        for (sponsor, part) in [
            (Sponsor::Known("bob".into()), "Parrainé par bob."),
            (Sponsor::None, "Sans parrain."),
            (
                Sponsor::Unknown,
                "Code de parrainage saisi inconnu, ignoré.",
            ),
        ] {
            let trial = started(sponsor);
            let (subject, body) = signup_admin_mail("léa", &trial, true, true, ACCOUNTS);
            assert_eq!(subject, "Nouveau membre en essai : léa");
            assert!(body.contains(&format!(
                "Essai gratuit de 7 jours, jusqu'au {}.",
                day_month(ends_at)
            )));
            assert!(body.contains("Rien à faire"), "{body}");
            assert!(body.contains("a reçu son lien"), "{body}");
            assert!(!body.contains("suspendu") && !body.contains("à activer"));
            assert!(body.contains(part), "{part} : {body}");
            assert!(matches!(signup_admin_level(&trial, true), Level::Info));
            assert_eq!(
                welcome_notice(Some(&trial), true),
                WelcomeNotice::Trial { days: 7 }
            );
            let text = member_text(&trial, true);
            assert!(text.contains("Ton essai de 7 jours a commencé"));
            assert!(!text.contains("activé par l'administrateur"));
            assert_eq!(trial.started_days(), Some(7));
        }
        // essai démarré mais lien de bienvenue NON parti : jamais « Rien à faire », avertissement
        let trial = started(Sponsor::None);
        let (subject, body) = signup_admin_mail("léa", &trial, true, false, ACCOUNTS);
        assert_eq!(subject, "Nouveau membre en essai : léa");
        assert!(!body.contains("Rien à faire") && !body.contains("a reçu son lien"));
        assert!(body.contains("Mail de bienvenue NON parti") && body.contains("Renvoyer le lien"));
        assert!(matches!(signup_admin_level(&trial, false), Level::Warn));
        // essai impossible : compte laissé suspendu, l'admin l'active ; le parrain est quand même dit
        for (block, why, fiche) in [
            (
                TrialBlock::CapReached {
                    premium: 25,
                    max: 25,
                },
                "Essai impossible : plafond premium atteint (25/25).",
                true,
            ),
            (
                TrialBlock::NoTrialDays,
                "Essai impossible : aucun essai configuré ([subscriptions] trial_days = 0).",
                true,
            ),
            (
                TrialBlock::Disabled,
                "Essai impossible : abonnements désactivés",
                false,
            ),
        ] {
            let trial = not_started(block, Sponsor::Known("bob".into()));
            let (subject, body) = signup_admin_mail("léa", &trial, false, true, ACCOUNTS);
            assert_eq!(subject, "Nouveau compte à activer : léa");
            assert!(body.contains(why), "{body}");
            assert!(body.contains("Parrainé par bob."), "{body}");
            assert!(body.contains(&format!(
                "suspendu tant que tu ne l'actives pas : {ACCOUNTS}"
            )));
            assert_eq!(
                body.contains("Sa fiche d'abonné est « suspendue »"),
                fiche,
                "{body}"
            );
            assert!(body.contains("il recevra un second mail à l'activation"));
            assert!(!body.contains("Rien à faire"));
            assert!(matches!(signup_admin_level(&trial, true), Level::Info));
            assert_eq!(
                welcome_notice(Some(&trial), false),
                WelcomeNotice::Activation
            );
            let text = member_text(&trial, false);
            assert!(text.contains("activé par l'administrateur"));
            assert!(!text.contains("essai"));
            assert_eq!(trial.started_days(), None);
        }
        // plafond atteint et lien non parti
        let trial = not_started(
            TrialBlock::CapReached {
                premium: 25,
                max: 25,
            },
            Sponsor::Unknown,
        );
        let (_, body) = signup_admin_mail("léa", &trial, false, false, ACCOUNTS);
        assert!(body.contains("Code de parrainage saisi inconnu, ignoré."));
        assert!(body.contains("Mail de bienvenue NON parti") && !body.contains("second mail"));
        assert!(matches!(signup_admin_level(&trial, false), Level::Warn));
        // erreur, compte relu actif (activation partie avant l'erreur) : ni « à activer » ni encadré « activation »
        let trial = not_started(TrialBlock::Failed("base verrouillée".into()), Sponsor::None);
        let (subject, body) = signup_admin_mail("léa", &trial, true, true, ACCOUNTS);
        assert_eq!(subject, "Nouveau compte : léa");
        assert!(
            body.contains("Essai impossible : erreur (base verrouillée)") && body.contains("actif")
        );
        assert!(matches!(signup_admin_level(&trial, true), Level::Warn));
        assert_eq!(welcome_notice(Some(&trial), true), WelcomeNotice::None);
        // erreur, compte relu suspendu : « à activer »
        let (subject, _) = signup_admin_mail("léa", &trial, false, true, ACCOUNTS);
        assert_eq!(subject, "Nouveau compte à activer : léa");
        // onboarding admin, CLI et poller : pas d'essai, encadré selon le compte
        assert_eq!(welcome_notice(None, false), WelcomeNotice::Activation);
        assert_eq!(welcome_notice(None, true), WelcomeNotice::None);
    }

    /// Inscription arrêtée après la création du compte Jellyfin : l'admin sait quoi rattraper.
    #[test]
    fn half_signup_tells_the_admin() {
        let (subject, body) = half_signup_mail("léa", "Jellyseerr injoignable");
        assert_eq!(subject, "Inscription ratée à moitié : léa");
        assert!(
            body.contains("Jellyseerr injoignable")
                && body.contains("supprimer le compte Jellyfin")
        );
    }

    #[test]
    fn js_id_from_array_or_object() {
        assert_eq!(
            find_js_id(&json!([{"id":3,"jellyfinUserId":"x"}]), "x"),
            Some(3)
        );
        assert_eq!(
            find_js_id(&json!({"id":4,"jellyfinUserId":"x"}), "x"),
            Some(4)
        );
        assert_eq!(find_js_id(&json!([]), "x"), None);
    }
}
