//! Onboarding unifié (ex `homelab-onboard-user.sh` + `jellyfin-create-user.sh`) :
//! compte Jellyfin (source de vérité du mot de passe) → import Jellyseerr →
//! email Jellyseerr → mail de bienvenue. Sérialisé par `onboard_lock` : le poller
//! et l'UI web ne peuvent plus se marcher dessus. Le mot de passe n'est jamais loggé.

use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use regex::Regex;
use serde_json::Value;
use tracing::{info, warn};

use crate::clients::jellyfin::non_admin_policy;
use crate::context::TaskContext;
use crate::mail;
use crate::secret::Secret;

#[derive(Debug, Clone)]
pub struct OnboardRequest {
    pub username: String,
    pub email: String,
    pub password: Option<Secret>,
}

#[derive(Debug, Clone)]
pub struct OnboardResult {
    pub username: String,
    pub email: String,
    pub password: Secret,
    pub jellyfin_id: String,
    pub jellyseerr_id: i64,
    pub jellyfin_url: String,
    pub jellyseerr_url: String,
    pub mail_sent: bool,
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

pub fn welcome_mail(
    username: &str,
    password: &str,
    from: &str,
    jellyfin_url: &str,
    jellyseerr_url: &str,
) -> String {
    format!(
        "Salut {username},

Si tu trouves ce mail dans tes spams / courrier indesirable / promotions,
merci de le marquer comme \"Pas indesirable\" et d'ajouter {from}
a tes contacts — sinon les futurs mails pourraient ne pas arriver.

Tu as peut-etre recu un premier mail \"Reset password\" automatique de
Jellyseerr juste avant celui-ci : IGNORE-LE. Ce sont les identifiants
ci-dessous qu'il faut utiliser.

Voici tes acces :

🎬 Streaming (regarder films/series) :
   {jellyfin_url}

🎯 Requetes (demander de nouveaux contenus) :
   {jellyseerr_url}

Identifiants (les memes sur les deux services) :
   Username : {username}
   Password : {password}

⚠️  Important — ton compte parent est Jellyfin.
   En cas de changement de mot de passe, la procedure se fait UNIQUEMENT
   sur Jellyfin (Profil → Mot de passe). Le changement sera automatiquement
   actif sur Jellyseerr egalement.

Sur Jellyseerr, connecte-toi via l'onglet \"Use your Jellyfin account\".

—
Jellyseerr Groscailloux
"
    )
}

pub async fn run(ctx: &TaskContext, req: OnboardRequest) -> Result<OnboardResult> {
    if !valid_username(&req.username) {
        bail!("username invalide (2-32 caractères, lettres/chiffres/_-)");
    }
    if !valid_email(&req.email) {
        bail!("email invalide");
    }
    let email = req.email.to_lowercase();
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
        password: password.clone(),
        jellyfin_id: String::new(),
        jellyseerr_id: 0,
        jellyfin_url: s.jellyfin_public_url.clone(),
        jellyseerr_url: s.jellyseerr_public_url.clone(),
        mail_sent: false,
        dry_run: ctx.dry_run,
    };
    if ctx.dry_run {
        info!(task = "onboard", username = %req.username, email = %email, "dry-run: pre-checks OK, would create user");
        return Ok(result);
    }

    let jf_id = ctx.jellyfin.create_user(&req.username, &password).await?;
    info!(task = "onboard", username = %req.username, jellyfin_id = %jf_id, "jellyfin user created");
    if let Err(e) = ctx
        .jellyfin
        .set_policy(
            &jf_id,
            &non_admin_policy(&s.jellyfin_lib_films, &s.jellyfin_lib_series),
        )
        .await
    {
        warn!(task = "onboard", username = %req.username, error = %e, "policy not applied, fix in Jellyfin dashboard");
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

    if let Some(smtp) = &s.smtp {
        let body = welcome_mail(
            &req.username,
            password.expose(),
            &smtp.from,
            &s.jellyfin_public_url,
            &s.jellyseerr_public_url,
        );
        match mail::send_plain(
            smtp,
            &req.username,
            &email,
            "Bienvenue sur Groscailloux — tes identifiants",
            &body,
        )
        .await
        {
            Ok(()) => {
                result.mail_sent = true;
                info!(task = "onboard", email = %email, "welcome mail sent");
            }
            Err(e) => {
                warn!(task = "onboard", email = %email, error = %e, "mail failed, hand over credentials manually")
            }
        }
    } else {
        warn!(task = "onboard", "SMTP non configuré : aucun mail envoyé");
    }
    Ok(result)
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
