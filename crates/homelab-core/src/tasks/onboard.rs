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
use crate::welcome;

/// D'où vient la demande : formulaire admin / CLI, page publique d'inscription, ou « Add User » Jellyseerr repris
/// par `user_poller`. Une inscription publique prévient l'admin par mail.
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
        warn!(task = "onboard", username = %req.username, error = %e, "policy not applied, fix in Jellyfin dashboard");
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
        warn!(task = "onboard", username = %req.username, error = %e, "library order not applied");
    }
    let skip = &ctx.cfg.accounts;
    if let Err(e) = ctx
        .jellyfin
        .set_skip_lengths(&jf_id, skip.skip_forward_ms, skip.skip_back_ms)
        .await
    {
        warn!(task = "onboard", username = %req.username, error = %e, "skip lengths not applied");
    }
    if let Err(e) = ctx
        .jellyfin
        .set_language_prefs(
            &jf_id,
            &skip.audio_language,
            &skip.subtitle_language,
            &skip.subtitle_mode,
            false,
        )
        .await
    {
        warn!(task = "onboard", username = %req.username, error = %e, "language preferences not applied");
    }
    result.jellyfin_id = jf_id.clone();

    let imported = ctx.jellyseerr.import_from_jellyfin(&[&jf_id]).await?;
    let mut js_id = find_js_id(&imported, &jf_id);
    if js_id.is_none() {
        js_id = find_js_id(&Value::Array(ctx.jellyseerr.users(1000).await?), &jf_id);
    }
    let js_id = js_id.context("user Jellyseerr introuvable après import")?;
    result.jellyseerr_id = js_id;
    info!(task = "onboard", username = %req.username, jellyseerr_id = js_id, "imported into jellyseerr");

    if let Err(e) = ctx
        .jellyseerr
        .set_main_settings(js_id, &email, &req.username)
        .await
    {
        warn!(task = "onboard", username = %req.username, error = %e, "set email failed, fix manually");
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
                    warn!(task = "onboard", username = %req.username, error = %e, "auto-approve not set, fix in Jellyseerr")
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
                warn!(task = "onboard", username = %req.username, error = %e, "suspend failed, account is active")
            }
        }
    }

    // lien de bienvenue : le membre définit son mot de passe sur la page, rien de sensible dans le mail
    match welcome::send(
        ctx,
        &jf_id,
        &req.username,
        &email,
        welcome::KIND_WELCOME,
        result.premium,
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
            warn!(task = "onboard", username = %req.username, error = %e, "welcome link failed")
        }
    }
    if req.source == Source::SelfSignup {
        notify_admin_signup(ctx, &req.username).await;
    }
    Ok(result)
}

/// Inscription publique : l'admin doit activer le compte depuis /accounts.
async fn notify_admin_signup(ctx: &TaskContext, username: &str) {
    let s = &ctx.secrets;
    let accounts = s
        .onboard_public_url
        .as_deref()
        .map(|u| format!("{}/accounts", u.trim_end_matches('/')))
        .unwrap_or_else(|| "la page Comptes (Homarr)".to_string());
    let body = format!(
        "Un nouveau compte vient d'être créé depuis la page d'inscription : {username}.\n\nIl est suspendu tant que tu ne l'actives pas : {accounts}\n\nLe membre a reçu son lien pour définir son mot de passe ; il recevra un second mail à l'activation.\n"
    );
    crate::alerts::admin(
        ctx,
        crate::alerts::Level::Info,
        &format!("Nouveau compte à activer : {username}"),
        &body,
    )
    .await;
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
