//! Comptes « premium » : premium = compte Jellyfin actif, non-premium = compte suspendu
//! (`Policy.IsDisabled`, fonction native : connexion et jetons refusés, rien n'est supprimé).
//! La politique Jellyfin est la seule source de vérité ; l'état ne garde que les permissions
//! Jellyseerr à restaurer (une session Jellyseerr ouverte survit à la suspension, on lui retire
//! donc le droit de demander). Les comptes protégés (`accounts.protected`) ne sont jamais
//! suspendus ni supprimés ; les autres, admin compris, sont gérés depuis la page « Comptes ».

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::context::TaskContext;
use crate::state::{now, AccountRecord};

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub premium: bool,
    pub admin: bool,
    /// Listé dans `accounts.protected` : ni interrupteur ni suppression.
    pub protected: bool,
    pub max_streams: u32,
    /// Dernière activité Jellyfin (ISO 8601), si le compte a déjà servi.
    pub last_activity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Activated,
    Suspended,
    Unchanged,
    CapReached { premium: usize, max: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub name: String,
    /// Compte Jellyseerr supprimé aussi (absent si jamais importé).
    pub jellyseerr: bool,
}

fn policy_flag(u: &Value, key: &str) -> bool {
    u.pointer(&format!("/Policy/{key}"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn is_admin(u: &Value) -> bool {
    policy_flag(u, "IsAdministrator")
}

pub fn is_premium(u: &Value) -> bool {
    !policy_flag(u, "IsDisabled")
}

pub fn is_protected(u: &Value, protected: &[String]) -> bool {
    u.get("Name")
        .and_then(Value::as_str)
        .map(|n| protected.iter().any(|p| p.eq_ignore_ascii_case(n)))
        .unwrap_or(false)
}

/// Comptes premium comptés dans le plafond : non protégés et actifs.
pub fn premium_count(users: &[Value], protected: &[String]) -> usize {
    users
        .iter()
        .filter(|u| !is_protected(u, protected) && is_premium(u))
        .count()
}

pub fn can_activate(premium: usize, max: usize) -> bool {
    premium < max
}

/// Politique actuelle avec seulement l'accès et le nombre de lectures simultanées modifiés :
/// bibliothèques, droits et fournisseur d'authentification restent ceux du compte.
pub fn with_access(policy: &Value, premium: bool, max_streams: u32) -> Value {
    let mut p = policy.clone();
    if let Some(o) = p.as_object_mut() {
        o.insert("IsDisabled".into(), Value::Bool(!premium));
        o.insert("MaxActiveSessions".into(), Value::from(max_streams));
    }
    p
}

/// Tous les comptes, par nom.
pub fn accounts(users: &[Value], protected: &[String]) -> Vec<Account> {
    let mut out: Vec<Account> = users
        .iter()
        .filter_map(|u| {
            Some(Account {
                id: u.get("Id")?.as_str()?.to_string(),
                name: u.get("Name")?.as_str()?.to_string(),
                premium: is_premium(u),
                admin: is_admin(u),
                protected: is_protected(u, protected),
                max_streams: u
                    .pointer("/Policy/MaxActiveSessions")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                last_activity: u
                    .get("LastActivityDate")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

pub async fn list(ctx: &TaskContext) -> Result<Vec<Account>> {
    Ok(accounts(
        &ctx.jellyfin.users().await?,
        &ctx.cfg.accounts.protected,
    ))
}

/// Compte par id Jellyfin ou par nom (insensible à la casse).
pub async fn resolve(ctx: &TaskContext, who: &str) -> Result<Account> {
    let wanted = who.to_lowercase();
    list(ctx)
        .await?
        .into_iter()
        .find(|a| a.id == who || a.name.to_lowercase() == wanted)
        .with_context(|| format!("compte introuvable : {who}"))
}

/// Active ou suspend un compte. Sérialisé avec l'onboarding (plafond).
pub async fn set_premium(ctx: &TaskContext, user_id: &str, on: bool) -> Result<Outcome> {
    let _guard = ctx.onboard_lock.lock().await;
    set_premium_locked(ctx, user_id, on).await
}

/// Comme [`set_premium`], pour un appelant qui tient déjà `onboard_lock` (l'onboarding).
pub async fn set_premium_locked(ctx: &TaskContext, user_id: &str, on: bool) -> Result<Outcome> {
    let cfg = &ctx.cfg.accounts;
    let users = ctx.jellyfin.users().await?;
    let user = users
        .iter()
        .find(|u| u.get("Id").and_then(Value::as_str) == Some(user_id))
        .with_context(|| format!("compte Jellyfin introuvable : {user_id}"))?;
    if is_protected(user, &cfg.protected) {
        bail!("compte protégé : jamais suspendu par cette page");
    }
    let name = user.get("Name").and_then(Value::as_str).unwrap_or("?");
    if is_premium(user) == on {
        return Ok(Outcome::Unchanged);
    }
    if on {
        let premium = premium_count(&users, &cfg.protected);
        if !can_activate(premium, cfg.max_premium) {
            return Ok(Outcome::CapReached {
                premium,
                max: cfg.max_premium,
            });
        }
    }
    let outcome = if on {
        Outcome::Activated
    } else {
        Outcome::Suspended
    };
    if ctx.dry_run {
        info!(task = "accounts", user = %name, ?outcome, "dry-run: premium not changed");
        return Ok(outcome);
    }
    let policy = user.get("Policy").context("Users sans Policy")?;
    ctx.jellyfin
        .set_policy(user_id, &with_access(policy, on, cfg.max_streams_per_user))
        .await?;
    info!(task = "accounts", user = %name, ?outcome, "premium changed");
    if !on {
        match ctx.jellyfin.sessions_of(user_id).await {
            Ok(sessions) => {
                for s in sessions
                    .iter()
                    .filter(|s| s.get("NowPlayingItem").is_some())
                {
                    if let Some(sid) = s.get("Id").and_then(Value::as_str) {
                        if let Err(e) = ctx.jellyfin.stop_playback(sid).await {
                            warn!(task = "accounts", user = %name, error = %e, "stop playback failed");
                        }
                    }
                }
            }
            Err(e) => warn!(task = "accounts", user = %name, error = %e, "sessions unreadable"),
        }
    }
    if let Err(e) = sync_jellyseerr(ctx, user_id, on).await {
        warn!(task = "accounts", user = %name, error = %e, "jellyseerr permissions not synced");
    }
    Ok(outcome)
}

/// Suspension : permissions Jellyseerr → 0 (sauvegardées). Activation : restauration.
async fn sync_jellyseerr(ctx: &TaskContext, user_id: &str, on: bool) -> Result<()> {
    let js_users = ctx.jellyseerr.users(1000).await?;
    let Some(js) = js_users
        .iter()
        .find(|u| u.get("jellyfinUserId").and_then(Value::as_str) == Some(user_id))
    else {
        return Ok(()); // pas (encore) importé dans Jellyseerr
    };
    let js_id = js
        .get("id")
        .and_then(Value::as_i64)
        .context("user sans id")?;
    let perms = js.get("permissions").and_then(Value::as_i64).unwrap_or(0);
    if on {
        let saved = ctx
            .state
            .read(|s| s.accounts.get(user_id).map(|r| r.jellyseerr_permissions))
            .await;
        // sans sauvegarde (suspendu via homelabctl pendant que le daemon réécrivait l'état, ou à
        // la main) : permissions par défaut de Jellyseerr plutôt qu'un compte actif sans droits
        let restore = match saved {
            Some(p) => Some(p),
            None if perms == 0 => Some(ctx.jellyseerr.default_permissions().await?),
            None => None,
        };
        if let Some(p) = restore {
            ctx.jellyseerr.set_permissions(js_id, p).await?;
        }
        ctx.state
            .update(|s| {
                s.accounts.remove(user_id);
            })
            .await?;
    } else if perms != 0 {
        // enregistré avant la remise à 0 : un échec ensuite ne perd pas les droits d'origine
        ctx.state
            .update(|s| {
                s.accounts.insert(
                    user_id.to_string(),
                    AccountRecord {
                        at: now(),
                        jellyseerr_id: js_id,
                        jellyseerr_permissions: perms,
                    },
                );
            })
            .await?;
        ctx.jellyseerr.set_permissions(js_id, 0).await?;
    }
    Ok(())
}

/// Supprime un compte : Jellyfin puis Jellyseerr (ses demandes partent avec). Jamais un compte
/// protégé. Sérialisé avec l'onboarding.
pub async fn delete(ctx: &TaskContext, user_id: &str) -> Result<Deleted> {
    let _guard = ctx.onboard_lock.lock().await;
    let users = ctx.jellyfin.users().await?;
    let user = users
        .iter()
        .find(|u| u.get("Id").and_then(Value::as_str) == Some(user_id))
        .with_context(|| format!("compte Jellyfin introuvable : {user_id}"))?;
    if is_protected(user, &ctx.cfg.accounts.protected) {
        bail!("compte protégé : jamais supprimé par cette page");
    }
    let name = user
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let js_id = ctx
        .jellyseerr
        .users(1000)
        .await?
        .iter()
        .find(|u| u.get("jellyfinUserId").and_then(Value::as_str) == Some(user_id))
        .and_then(|u| u.get("id").and_then(Value::as_i64));
    if ctx.dry_run {
        info!(task = "accounts", user = %name, ?js_id, "dry-run: account not deleted");
        return Ok(Deleted {
            name,
            jellyseerr: js_id.is_some(),
        });
    }
    ctx.jellyfin.delete_user(user_id).await?;
    info!(task = "accounts", user = %name, "jellyfin account deleted");
    let mut jellyseerr = false;
    if let Some(id) = js_id {
        match ctx.jellyseerr.delete_user(id).await {
            Ok(()) => {
                jellyseerr = true;
                info!(task = "accounts", user = %name, jellyseerr_id = id, "jellyseerr account deleted");
            }
            Err(e) => {
                warn!(task = "accounts", user = %name, error = %e, "jellyseerr account not deleted")
            }
        }
    }
    ctx.state
        .update(|s| {
            s.accounts.remove(user_id);
        })
        .await?;
    Ok(Deleted { name, jellyseerr })
}

/// Applique `max_streams_per_user` aux comptes non protégés qui ne l'ont pas encore.
/// Renvoie les noms des comptes modifiés (ou qui le seraient en dry-run).
pub async fn apply_stream_limit(ctx: &TaskContext) -> Result<Vec<String>> {
    let max = ctx.cfg.accounts.max_streams_per_user;
    let mut changed = Vec::new();
    for u in ctx.jellyfin.users().await? {
        if is_protected(&u, &ctx.cfg.accounts.protected) {
            continue;
        }
        let current = u
            .pointer("/Policy/MaxActiveSessions")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if current == u64::from(max) {
            continue;
        }
        let (Some(id), Some(name), Some(policy)) = (
            u.get("Id").and_then(Value::as_str),
            u.get("Name").and_then(Value::as_str),
            u.get("Policy"),
        ) else {
            continue;
        };
        if !ctx.dry_run {
            ctx.jellyfin
                .set_policy(id, &with_access(policy, is_premium(&u), max))
                .await?;
            info!(task = "accounts", user = %name, max, "stream limit applied");
        }
        changed.push(name.to_string());
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(name: &str, admin: bool, disabled: bool) -> Value {
        json!({
            "Id": format!("id-{name}"),
            "Name": name,
            "LastActivityDate": "2026-09-14T10:00:00Z",
            "Policy": {
                "IsAdministrator": admin,
                "IsDisabled": disabled,
                "MaxActiveSessions": 0,
                "EnabledFolders": ["films", "series"],
                "EnableAllFolders": false
            }
        })
    }

    #[test]
    fn with_access_only_touches_access_and_streams() {
        let u = user("bob", false, false);
        let p = with_access(&u["Policy"], false, 2);
        assert_eq!(p["IsDisabled"], json!(true));
        assert_eq!(p["MaxActiveSessions"], json!(2));
        assert_eq!(p["EnabledFolders"], json!(["films", "series"]));
        assert_eq!(p["EnableAllFolders"], json!(false));
        assert_eq!(p["IsAdministrator"], json!(false));
        let back = with_access(&p, true, 2);
        assert_eq!(back["IsDisabled"], json!(false));
    }

    fn prot() -> Vec<String> {
        vec!["Haradas".into(), "LeGrosCailloux".into()]
    }

    #[test]
    fn cap_counts_active_unprotected_accounts_admins_included() {
        let mut users = vec![
            user("haradas", true, false), // protégé (casse ignorée) : hors plafond
            user("Paul", true, false),   // admin non protégé : compté
            user("off", false, true),
        ];
        for i in 0..23 {
            users.push(user(&format!("u{i}"), false, false));
        }
        assert_eq!(premium_count(&users, &prot()), 24);
        assert!(can_activate(premium_count(&users, &prot()), 25));
        users.push(user("u23", false, false));
        assert_eq!(premium_count(&users, &prot()), 25);
        assert!(!can_activate(premium_count(&users, &prot()), 25));
    }

    #[test]
    fn accounts_list_everyone_with_flags_sorted_by_name() {
        let users = vec![
            user("Zoe", false, true),
            user("LeGrosCailloux", true, false),
            user("Paul", true, false),
            user("alice", false, false),
        ];
        let a = accounts(&users, &prot());
        assert_eq!(
            a.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["alice", "LeGrosCailloux", "Paul", "Zoe"]
        );
        assert!(a[0].premium && !a[0].admin && !a[0].protected);
        assert!(a[1].admin && a[1].protected);
        assert!(a[2].admin && !a[2].protected);
        assert!(!a[3].premium);
        assert_eq!(a[0].last_activity.as_deref(), Some("2026-09-14T10:00:00Z"));
    }
}
