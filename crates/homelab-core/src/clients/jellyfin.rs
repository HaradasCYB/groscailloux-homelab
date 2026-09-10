use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::{json, Value};

use super::{check, json};
use crate::secret::Secret;

#[derive(Clone)]
pub struct JellyfinClient {
    base: Url,
    key: Secret,
    http: Client,
}

impl JellyfinClient {
    pub fn new(base: &str, key: Secret, http: Client) -> Result<Self> {
        Ok(Self {
            base: Url::parse(base).context("URL Jellyfin invalide")?,
            key,
            http,
        })
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let url = self.base.join(path).expect("chemin API valide");
        self.http
            .request(method, url)
            .header("X-Emby-Token", self.key.expose())
    }

    pub async fn system_info(&self) -> Result<Value> {
        let resp = self.req(Method::GET, "System/Info").send().await?;
        json(resp, "jellyfin System/Info").await
    }

    pub async fn users(&self) -> Result<Vec<Value>> {
        let resp = self.req(Method::GET, "Users").send().await?;
        Ok(json(resp, "jellyfin Users")
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Recherche insensible à la casse, comme l'ancien `ascii_downcase` de jq.
    pub async fn find_user(&self, name: &str) -> Result<Option<Value>> {
        let wanted = name.to_lowercase();
        Ok(self.users().await?.into_iter().find(|u| {
            u.get("Name")
                .and_then(Value::as_str)
                .map(|n| n.to_lowercase() == wanted)
                .unwrap_or(false)
        }))
    }

    pub async fn create_user(&self, name: &str, password: &Secret) -> Result<String> {
        let body = json!({ "Name": name, "Password": password.expose() });
        let resp = self
            .req(Method::POST, "Users/New")
            .json(&body)
            .send()
            .await?;
        let v = json(resp, "jellyfin Users/New").await?;
        v.get("Id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("Users/New sans Id")
    }

    pub async fn set_policy(&self, user_id: &str, policy: &Value) -> Result<()> {
        let resp = self
            .req(Method::POST, &format!("Users/{user_id}/Policy"))
            .json(policy)
            .send()
            .await?;
        check(resp, "jellyfin Users/{id}/Policy").await.map(|_| ())
    }

    /// Nombre de sessions avec lecture en cours (utile avant un redémarrage).
    pub async fn active_playbacks(&self) -> Result<usize> {
        let resp = self
            .req(Method::GET, "Sessions")
            .query(&[("activeWithinSeconds", "120")])
            .send()
            .await?;
        let v = json(resp, "jellyfin Sessions").await?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s.get("NowPlayingItem").is_some())
                    .count()
            })
            .unwrap_or(0))
    }
}

/// Politique utilisateur non-admin limitée aux deux bibliothèques Films/Séries.
pub fn non_admin_policy(lib_films: &str, lib_series: &str) -> Value {
    json!({
        "IsAdministrator": false,
        "IsHidden": false,
        "IsDisabled": false,
        "EnableUserPreferenceAccess": true,
        "EnableRemoteAccess": true,
        "EnableMediaPlayback": true,
        "EnableAudioPlaybackTranscoding": true,
        "EnableVideoPlaybackTranscoding": true,
        "EnablePlaybackRemuxing": true,
        "EnableLiveTvAccess": false,
        "EnableLiveTvManagement": false,
        "EnableContentDeletion": false,
        "EnableContentDownloading": false,
        "EnableSyncTranscoding": true,
        "EnableSubtitleManagement": false,
        "EnableAllDevices": true,
        "EnableAllChannels": false,
        "EnableAllFolders": false,
        "EnabledFolders": [lib_films, lib_series],
        "EnabledChannels": [],
        "EnabledDevices": [],
        "BlockedTags": [],
        "BlockedChannels": [],
        "BlockedMediaFolders": [],
        "AccessSchedules": [],
        "LoginAttemptsBeforeLockout": 5,
        "MaxActiveSessions": 0,
        "AuthenticationProviderId": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
        "PasswordResetProviderId": "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
        "SyncPlayAccess": "CreateAndJoinGroups"
    })
}
