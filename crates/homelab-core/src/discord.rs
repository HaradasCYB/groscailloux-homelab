//! Discord par **webhooks** (aucun bot) : un salon `#jellyfin` pour les membres (nouveautés, demandes, annonces)
//! et un salon privé pour l'admin (alertes). Les URL de webhook sont des secrets (`DISCORD_WEBHOOK_MEMBERS`,
//! `DISCORD_WEBHOOK_ADMIN` dans `.env`) : jamais journalisées (`mask`), jamais dans le dépôt. Les Arrs et
//! Jellyseerr postent directement (connexions configurées par `apply`) ; homelabd poste ses propres messages ici.
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::context::TaskContext;

pub const USERNAME: &str = "Groscailloux";
pub const COLOR_INFO: u32 = 0x5865F2;
pub const COLOR_OK: u32 = 0x10B981;
pub const COLOR_WARN: u32 = 0xF59E0B;
pub const COLOR_ERROR: u32 = 0xEF4444;
pub const NAME_MEMBERS: &str = "Discord membres";
pub const NAME_ADMIN: &str = "Discord admin";

/// Quel salon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Members,
    Admin,
}

/// Un message Discord (un embed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    pub title: String,
    pub description: String,
    pub url: Option<String>,
    pub color: u32,
    pub fields: Vec<(String, String)>,
}

impl Embed {
    pub fn new(title: impl Into<String>, description: impl Into<String>, color: u32) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            url: None,
            color,
            fields: Vec::new(),
        }
    }
    pub fn info(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(title, description, COLOR_INFO)
    }
    pub fn warn(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(title, description, COLOR_WARN)
    }
    pub fn error(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(title, description, COLOR_ERROR)
    }
    pub fn ok(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(title, description, COLOR_OK)
    }
    pub fn link(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((name.into(), value.into()));
        self
    }
}

/// Corps JSON attendu par un webhook Discord ; limites de l'API respectées (titre 256, description 4096,
/// 25 champs de 1024). `mention` = identifiant de rôle à mentionner (en tête du message), facultatif.
pub fn payload(embed: &Embed, mention: Option<&str>) -> Value {
    let mut e = json!({
        "title": cut(&embed.title, 256),
        "description": cut(&embed.description, 4096),
        "color": embed.color,
        "fields": embed.fields.iter().take(25).map(|(n, v)| json!({ "name": cut(n, 256), "value": cut(v, 1024), "inline": false })).collect::<Vec<_>>(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(u) = &embed.url {
        e["url"] = json!(u);
    }
    let mut body = json!({ "username": USERNAME, "embeds": [e] });
    if let Some(role) = mention.filter(|r| !r.is_empty()) {
        body["content"] = json!(format!("<@&{role}>"));
        body["allowed_mentions"] = json!({ "roles": [role] });
    }
    body
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// URL sans le jeton, pour les journaux : `https://discord.com/api/webhooks/<id>/…`.
pub fn mask(url: &str) -> String {
    match url.find("/webhooks/") {
        Some(i) => {
            let rest = &url[i + "/webhooks/".len()..];
            let id = rest.split('/').next().unwrap_or("");
            format!("{}/webhooks/{id}/…", &url[..i])
        }
        None => "<webhook>".to_string(),
    }
}

pub fn looks_like_webhook(url: &str) -> bool {
    (url.starts_with("https://discord.com/api/webhooks/")
        || url.starts_with("https://discordapp.com/api/webhooks/"))
        && url.matches('/').count() >= 6
}

/// Poste un embed sur un webhook ; une reprise après un 429 (`retry_after`).
pub async fn post(http: &reqwest::Client, webhook: &str, body: &Value) -> Result<()> {
    for attempt in 0..2 {
        let resp = http
            .post(webhook)
            .timeout(Duration::from_secs(15))
            .json(body)
            .send()
            .await
            .with_context(|| format!("discord {}", mask(webhook)))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status.as_u16() == 429 && attempt == 0 {
            let wait = resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("retry_after").and_then(Value::as_f64))
                .unwrap_or(2.0)
                .clamp(0.5, 10.0);
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            continue;
        }
        let text = resp.text().await.unwrap_or_default();
        bail!(
            "discord {} → {status} {}",
            mask(webhook),
            text.chars().take(200).collect::<String>()
        );
    }
    bail!("discord {} → 429 persistant", mask(webhook))
}

/// URL du webhook d'un salon (admin se rabat sur membres si absent).
pub fn webhook_for(ctx: &TaskContext, channel: Channel) -> Option<&str> {
    let s = &ctx.secrets;
    match channel {
        Channel::Members => s.discord_webhook_members.as_deref(),
        Channel::Admin => s
            .discord_webhook_admin
            .as_deref()
            .or(s.discord_webhook_members.as_deref()),
    }
}

/// Envoie un embed sur le salon voulu ; sans webhook configuré ou en dry-run, ne fait rien (Ok).
/// Les erreurs sont journalisées, jamais bloquantes pour l'appelant.
pub async fn notify(ctx: &TaskContext, channel: Channel, embed: Embed) -> bool {
    let Some(url) = webhook_for(ctx, channel) else {
        return false;
    };
    if ctx.dry_run {
        tracing::info!(task = "discord", ?channel, title = %embed.title, "dry-run: not posted");
        return false;
    }
    let mention = match channel {
        Channel::Members => None, // les mentions de rôle restent réservées aux nouveautés (Arrs/Jellyseerr)
        Channel::Admin => None,
    };
    match post(&ctx.http, url, &payload(&embed, mention)).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(task = "discord", ?channel, error = %e, "post failed");
            false
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Configuration des Arrs et de Jellyseerr (par API, idempotente) : `homelabctl discord apply|test|remove`.
// ---------------------------------------------------------------------------------------------

/// Types de notification Jellyseerr postés sur le salon des membres : nouvelle demande (2), validée
/// automatiquement (128), refusée (64), disponible (8).
pub const JELLYSEERR_TYPES: i64 = 2 | 8 | 64 | 128;

/// Corps d'une connexion Discord d'un Arr. `kind` : "sonarr" ou "radarr" ; `channel` décide des
/// événements : membres = imports (Sonarr : un message par téléchargement via `onImportComplete`, jamais
/// par épisode) ; admin = santé.
pub fn arr_notification(
    kind: &str,
    channel: Channel,
    webhook: &str,
    existing_fields: &[Value],
) -> Value {
    let name = match channel {
        Channel::Members => NAME_MEMBERS,
        Channel::Admin => NAME_ADMIN,
    };
    let members = channel == Channel::Members;
    let mut fields: Vec<Value> = existing_fields
        .iter()
        .filter(|f| {
            !matches!(
                f.get("name").and_then(Value::as_str),
                Some("webHookUrl") | Some("username") | Some("avatar") | Some("author")
            )
        })
        .map(|f| json!({ "name": f["name"], "value": f.get("value").cloned().unwrap_or(Value::Null) }))
        .collect();
    fields.push(json!({ "name": "webHookUrl", "value": webhook }));
    fields.push(json!({ "name": "username", "value": USERNAME }));
    fields.push(json!({ "name": "author", "value": USERNAME }));
    let mut body = json!({
        "name": name,
        "implementation": "Discord",
        "configContract": "DiscordSettings",
        "tags": [],
        "fields": fields,
        "onGrab": false,
        "onRename": false,
        "onApplicationUpdate": false,
        "onManualInteractionRequired": !members,
        "onHealthIssue": !members,
        "onHealthRestored": !members,
        "includeHealthWarnings": false,
    });
    if kind == "sonarr" {
        body["onDownload"] = json!(false);
        body["onImportComplete"] = json!(members);
        body["onUpgrade"] = json!(members);
        body["onSeriesAdd"] = json!(false);
        body["onSeriesDelete"] = json!(false);
        body["onEpisodeFileDelete"] = json!(false);
        body["onEpisodeFileDeleteForUpgrade"] = json!(false);
    } else {
        body["onDownload"] = json!(members);
        body["onUpgrade"] = json!(members);
        body["onMovieAdded"] = json!(false);
        body["onMovieDelete"] = json!(false);
        body["onMovieFileDelete"] = json!(false);
        body["onMovieFileDeleteForUpgrade"] = json!(false);
    }
    body
}

/// Corps de l'agent Discord de Jellyseerr.
pub fn jellyseerr_agent(webhook: Option<&str>, role: Option<&str>) -> Value {
    json!({
        "enabled": webhook.is_some(),
        "types": JELLYSEERR_TYPES,
        "options": {
            "botUsername": USERNAME,
            "botAvatarUrl": "",
            "webhookUrl": webhook.unwrap_or(""),
            "webhookRoleId": role.unwrap_or(""),
            "enableMentions": role.is_some(),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// `homelabctl discord apply|test|remove`
// ---------------------------------------------------------------------------------------------

/// Résultat d'une application : lignes lisibles pour la CLI.
pub struct Applied {
    pub lines: Vec<String>,
}

fn kind_of(arr: &crate::clients::ArrClient) -> &'static str {
    if arr.name.starts_with("sonarr") {
        "sonarr"
    } else {
        "radarr"
    }
}

/// Crée ou met à jour les connexions « Discord membres » / « Discord admin » des 4 Arrs et l'agent Discord
/// de Jellyseerr, d'après `.env`. Idempotent ; sauvegarde JSON de l'existant dans `backups/`.
pub async fn apply(ctx: &TaskContext) -> Result<Applied> {
    let members = ctx.secrets.discord_webhook_members.as_deref();
    let admin = webhook_for(ctx, Channel::Admin);
    let mut lines = Vec::new();
    if members.is_none() && admin.is_none() {
        bail!("aucun webhook dans .env (DISCORD_WEBHOOK_MEMBERS / DISCORD_WEBHOOK_ADMIN)");
    }
    for url in [members, admin].into_iter().flatten() {
        if !looks_like_webhook(url) {
            bail!(
                "webhook invalide ({}) : attendu https://discord.com/api/webhooks/<id>/<jeton>",
                mask(url)
            );
        }
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let bdir = ctx
        .cfg
        .paths
        .base
        .join("backups")
        .join(format!("discord-{stamp}"));
    for arr in ctx.all_arrs() {
        let existing = arr.get("api/v3/notification", &[]).await?;
        let list = existing.as_array().cloned().unwrap_or_default();
        if !ctx.dry_run {
            std::fs::create_dir_all(&bdir)?;
            std::fs::write(
                bdir.join(format!("{}-notifications.json", arr.name)),
                serde_json::to_vec_pretty(&existing)?,
            )?;
        }
        let schema_fields: Vec<Value> = arr
            .get("api/v3/notification/schema", &[])
            .await?
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|s| s.get("implementation").and_then(Value::as_str) == Some("Discord"))
            })
            .and_then(|s| s.get("fields").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        for (channel, url) in [(Channel::Members, members), (Channel::Admin, admin)] {
            let name = match channel {
                Channel::Members => NAME_MEMBERS,
                Channel::Admin => NAME_ADMIN,
            };
            let current = list
                .iter()
                .find(|n| n.get("name").and_then(Value::as_str) == Some(name));
            let Some(url) = url else {
                if let Some(c) = current {
                    let id = c.get("id").and_then(Value::as_i64).unwrap_or(0);
                    if !ctx.dry_run {
                        arr.delete(&format!("api/v3/notification/{id}"), &[])
                            .await?;
                    }
                    lines.push(format!("{} : « {name} » retiré (pas de webhook)", arr.name));
                }
                continue;
            };
            let base_fields = current
                .and_then(|c| c.get("fields").and_then(Value::as_array).cloned())
                .unwrap_or(schema_fields.clone());
            let mut body = arr_notification(kind_of(arr), channel, url, &base_fields);
            match current {
                Some(c) => {
                    let id = c.get("id").and_then(Value::as_i64).unwrap_or(0);
                    body["id"] = json!(id);
                    if !ctx.dry_run {
                        arr.put(&format!("api/v3/notification/{id}"), &body).await?;
                    }
                    lines.push(format!("{} : « {name} » mis à jour", arr.name));
                }
                None => {
                    if !ctx.dry_run {
                        arr.post("api/v3/notification", &body).await?;
                    }
                    lines.push(format!("{} : « {name} » créé", arr.name));
                }
            }
        }
    }
    // Jellyseerr
    let before = ctx
        .jellyseerr
        .get_json("api/v1/settings/notifications/discord")
        .await?;
    if !ctx.dry_run {
        std::fs::create_dir_all(&bdir)?;
        let p = bdir.join("jellyseerr-discord.json");
        std::fs::write(&p, serde_json::to_vec_pretty(&before)?)?;
        let _ = std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    }
    let agent = jellyseerr_agent(members, ctx.secrets.discord_role_members.as_deref());
    if !ctx.dry_run {
        ctx.jellyseerr
            .post_json("api/v1/settings/notifications/discord", &agent)
            .await?;
    }
    lines.push(format!(
        "jellyseerr : agent Discord {}",
        if members.is_some() {
            "activé (demandes)"
        } else {
            "désactivé"
        }
    ));
    if !ctx.dry_run {
        lines.push(format!("sauvegardes : {}", bdir.display()));
    }
    Ok(Applied { lines })
}

/// Retire les connexions Discord des Arrs et désactive l'agent Jellyseerr (sauvegardes avant).
pub async fn remove(ctx: &TaskContext) -> Result<Applied> {
    let mut lines = Vec::new();
    for arr in ctx.all_arrs() {
        let list = arr.get("api/v3/notification", &[]).await?;
        for n in list.as_array().cloned().unwrap_or_default() {
            let name = n.get("name").and_then(Value::as_str).unwrap_or("");
            if name == NAME_MEMBERS || name == NAME_ADMIN {
                let id = n.get("id").and_then(Value::as_i64).unwrap_or(0);
                if !ctx.dry_run {
                    arr.delete(&format!("api/v3/notification/{id}"), &[])
                        .await?;
                }
                lines.push(format!("{} : « {name} » retiré", arr.name));
            }
        }
    }
    if !ctx.dry_run {
        ctx.jellyseerr
            .post_json(
                "api/v1/settings/notifications/discord",
                &jellyseerr_agent(None, None),
            )
            .await?;
    }
    lines.push("jellyseerr : agent Discord désactivé".into());
    Ok(Applied { lines })
}

/// Un message d'essai sur chaque salon configuré, plus le test intégré de Jellyseerr.
pub async fn test(ctx: &TaskContext) -> Result<Applied> {
    let mut lines = Vec::new();
    for (channel, label) in [(Channel::Members, "membres"), (Channel::Admin, "admin")] {
        let Some(url) = webhook_for(ctx, channel) else {
            lines.push(format!("salon {label} : pas de webhook"));
            continue;
        };
        if channel == Channel::Admin && Some(url) == ctx.secrets.discord_webhook_members.as_deref()
        {
            lines.push("salon admin : même webhook que les membres (repli)".into());
            continue;
        }
        let e = Embed::ok(
            "Test Groscailloux",
            format!(
                "Ce salon recevra les messages « {label} ». Si tu lis ceci, la connexion marche."
            ),
        );
        if ctx.dry_run {
            lines.push(format!("salon {label} : dry-run, rien posté"));
            continue;
        }
        post(&ctx.http, url, &payload(&e, None)).await?;
        lines.push(format!(
            "salon {label} : message d'essai posté ({})",
            mask(url)
        ));
    }
    if let Some(members) = ctx.secrets.discord_webhook_members.as_deref() {
        if !ctx.dry_run {
            let agent =
                jellyseerr_agent(Some(members), ctx.secrets.discord_role_members.as_deref());
            ctx.jellyseerr
                .post_json("api/v1/settings/notifications/discord/test", &agent)
                .await?;
            lines.push("jellyseerr : notification d'essai envoyée".into());
        }
    }
    Ok(Applied { lines })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_hides_the_token() {
        let m = mask("https://discord.com/api/webhooks/123456/abcDEF-secret");
        assert_eq!(m, "https://discord.com/api/webhooks/123456/…");
        assert_eq!(mask("nope"), "<webhook>");
        assert!(looks_like_webhook(
            "https://discord.com/api/webhooks/123456/abcDEF"
        ));
        assert!(!looks_like_webhook("https://example.com/hook"));
    }

    #[test]
    fn payload_respects_limits_and_mentions() {
        let e = Embed::warn("t".repeat(300), "d".repeat(5000)).field("a", "b");
        let p = payload(&e, Some("42"));
        assert_eq!(p["username"], USERNAME);
        assert_eq!(
            p["embeds"][0]["title"].as_str().unwrap().chars().count(),
            256
        );
        assert_eq!(
            p["embeds"][0]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            4096
        );
        assert_eq!(p["embeds"][0]["color"], COLOR_WARN);
        assert_eq!(p["content"], "<@&42>");
        assert_eq!(p["allowed_mentions"]["roles"][0], "42");
        let q = payload(&Embed::info("x", "y"), None);
        assert!(q.get("content").is_none());
    }

    #[test]
    fn arr_notification_members_vs_admin() {
        let m = arr_notification(
            "sonarr",
            Channel::Members,
            "https://discord.com/api/webhooks/1/x",
            &[],
        );
        assert_eq!(m["name"], NAME_MEMBERS);
        assert_eq!(m["onImportComplete"], true);
        assert_eq!(m["onDownload"], false, "jamais un message par épisode");
        assert_eq!(m["onHealthIssue"], false);
        let a = arr_notification(
            "radarr",
            Channel::Admin,
            "https://discord.com/api/webhooks/1/x",
            &[
                json!({"name":"grabFields","value":[0,1]}),
                json!({"name":"webHookUrl","value":"old"}),
            ],
        );
        assert_eq!(a["name"], NAME_ADMIN);
        assert_eq!(a["onDownload"], false);
        assert_eq!(a["onHealthIssue"], true);
        let names: Vec<&str> = a["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.iter().filter(|n| **n == "webHookUrl").count(), 1);
        assert!(names.contains(&"grabFields"));
    }

    #[test]
    fn jellyseerr_agent_disabled_without_webhook() {
        let off = jellyseerr_agent(None, None);
        assert_eq!(off["enabled"], false);
        let on = jellyseerr_agent(Some("https://discord.com/api/webhooks/1/x"), Some("7"));
        assert_eq!(on["enabled"], true);
        assert_eq!(on["types"], JELLYSEERR_TYPES);
        assert_eq!(on["options"]["enableMentions"], true);
    }
}
