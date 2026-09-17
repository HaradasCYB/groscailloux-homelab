use std::time::Duration;

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

    /// Compte correspondant au jeton de session d'un membre (`/Users/Me`), `None` si Jellyfin refuse
    /// le jeton (expiré, déconnecté, compte suspendu). Le jeton n'est ni journalisé ni conservé.
    pub async fn user_from_token(&self, token: &str) -> Result<Option<Value>> {
        let url = self.base.join("Users/Me").expect("chemin API valide");
        let resp = self
            .http
            .get(url)
            .header("X-Emby-Token", token)
            .send()
            .await?;
        if matches!(resp.status().as_u16(), 401 | 403) {
            return Ok(None);
        }
        json(resp, "jellyfin Users/Me").await.map(Some)
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

    /// Chemins de tous les films et épisodes de la bibliothèque.
    /// Séries et films de la médiathèque, avec leur chemin et leurs identifiants de référence.
    pub async fn titles_with_ids(&self) -> Result<Vec<Value>> {
        let resp = self
            .req(Method::GET, "Items")
            .query(&[
                ("Recursive", "true"),
                ("IncludeItemTypes", "Series,Movie"),
                ("Fields", "Path,ProviderIds"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ])
            .send()
            .await?;
        let v = json(resp, "jellyfin Items").await?;
        Ok(v.get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Fiches proposées par les fournisseurs de métadonnées pour un élément (`Series` ou `Movie`).
    pub async fn remote_search(&self, kind: &str, item_id: &str, ids: Value) -> Result<Vec<Value>> {
        let resp = self
            .req(Method::POST, &format!("Items/RemoteSearch/{kind}"))
            .json(&json!({ "ItemId": item_id, "SearchInfo": { "ProviderIds": ids } }))
            .timeout(Duration::from_secs(300))
            .send()
            .await?;
        let v = json(resp, "jellyfin RemoteSearch").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Applique une fiche puis relance les métadonnées. L'application peut dépasser le délai côté client
    /// alors que Jellyfin la termine : on ne traite pas ce cas comme un échec (le contrôle suivant tranche).
    pub async fn apply_remote(&self, item_id: &str, candidate: &Value) -> Result<()> {
        let sent = self
            .req(
                Method::POST,
                &format!("Items/RemoteSearch/Apply/{item_id}?ReplaceAllImages=true"),
            )
            .json(candidate)
            .timeout(Duration::from_secs(420))
            .send()
            .await;
        if let Err(e) = sent {
            if !e.is_timeout() {
                return Err(e.into());
            }
        }
        let resp = self
            .req(
                Method::POST,
                &format!("Items/{item_id}/Refresh?Recursive=true&MetadataRefreshMode=FullRefresh&ImageRefreshMode=FullRefresh&ReplaceAllMetadata=true"),
            )
            .send()
            .await?;
        check(resp, "jellyfin Items/Refresh").await.map(|_| ())
    }

    pub async fn item_paths(&self) -> Result<std::collections::HashSet<String>> {
        let resp = self
            .req(Method::GET, "Items")
            .query(&[
                ("Recursive", "true"),
                ("IncludeItemTypes", "Movie,Episode"),
                ("Fields", "Path"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ])
            .send()
            .await?;
        let v = json(resp, "jellyfin Items").await?;
        Ok(v.get("Items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("Path").and_then(Value::as_str))
            .map(str::to_string)
            .collect())
    }

    /// Requête SQL en lecture sur la base de Playback Reporting (plugin) : (colonnes, lignes).
    pub async fn playback_query(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let resp = self
            .req(Method::POST, "user_usage_stats/submit_custom_query")
            .json(&json!({ "CustomQueryString": sql, "ReplaceUserId": false }))
            .send()
            .await?;
        let v = json(resp, "jellyfin playback query").await?;
        let cols = v
            .get("colums")
            .or_else(|| v.get("columns"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let rows = v
            .get("results")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_array)
                    .map(|r| {
                        r.iter()
                            .map(|c| {
                                c.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| c.to_string())
                            })
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((cols, rows))
    }

    /// Éléments par id (type, série parente), ceux qui n'existent plus sont absents.
    pub async fn items_by_ids(&self, ids: &[String]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(80) {
            let resp = self
                .req(Method::GET, "Items")
                .query(&[
                    ("Ids", chunk.join(",").as_str()),
                    ("Fields", "SeriesId"),
                    ("EnableImages", "false"),
                ])
                .send()
                .await?;
            let v = json(resp, "jellyfin Items?Ids").await?;
            out.extend(
                v.get("Items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        Ok(out)
    }

    /// Collection (BoxSet) par nom exact, vue par `user_id`.
    pub async fn find_collection(&self, user_id: &str, name: &str) -> Result<Option<String>> {
        let resp = self
            .req(Method::GET, &format!("Users/{user_id}/Items"))
            .query(&[("Recursive", "true"), ("IncludeItemTypes", "BoxSet")])
            .send()
            .await?;
        let v = json(resp, "jellyfin BoxSets").await?;
        Ok(v.get("Items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|i| i.get("Name").and_then(Value::as_str) == Some(name))
            .and_then(|i| i.get("Id").and_then(Value::as_str))
            .map(str::to_string))
    }

    pub async fn collection_children(&self, user_id: &str, id: &str) -> Result<Vec<String>> {
        let resp = self
            .req(Method::GET, &format!("Users/{user_id}/Items"))
            .query(&[("ParentId", id)])
            .send()
            .await?;
        let v = json(resp, "jellyfin collection items").await?;
        Ok(v.get("Items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("Id").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    pub async fn create_collection(&self, name: &str, ids: &[String]) -> Result<String> {
        let resp = self
            .req(Method::POST, "Collections")
            .query(&[("Name", name), ("Ids", ids.join(",").as_str())])
            .send()
            .await?;
        let v = json(resp, "jellyfin POST Collections").await?;
        v.get("Id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("Collections sans Id")
    }

    pub async fn collection_edit(&self, id: &str, ids: &[String], add: bool) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let method = if add { Method::POST } else { Method::DELETE };
        let resp = self
            .req(method, &format!("Collections/{id}/Items"))
            .query(&[("Ids", ids.join(",").as_str())])
            .send()
            .await?;
        check(resp, "jellyfin Collections/{id}/Items")
            .await
            .map(|_| ())
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let resp = self
            .req(Method::DELETE, &format!("Users/{id}"))
            .send()
            .await?;
        check(resp, "jellyfin DELETE Users/{id}").await.map(|_| ())
    }

    pub async fn user(&self, id: &str) -> Result<Value> {
        let resp = self.req(Method::GET, &format!("Users/{id}")).send().await?;
        json(resp, "jellyfin Users/{id}").await
    }

    /// Sessions d'un utilisateur (appareils connectés, avec ou sans lecture).
    pub async fn sessions_of(&self, user_id: &str) -> Result<Vec<Value>> {
        let resp = self.req(Method::GET, "Sessions").send().await?;
        let v = json(resp, "jellyfin Sessions").await?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s.get("UserId").and_then(Value::as_str) == Some(user_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Toutes les sessions connues de Jellyfin.
    pub async fn sessions(&self) -> Result<Vec<Value>> {
        let resp = self.req(Method::GET, "Sessions").send().await?;
        Ok(json(resp, "jellyfin Sessions")
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Message affiché à l'écran d'une session (`timeout_ms` : durée d'affichage).
    pub async fn send_message(
        &self,
        session_id: &str,
        header: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<()> {
        let resp = self
            .req(Method::POST, &format!("Sessions/{session_id}/Message"))
            .json(&json!({ "Header": header, "Text": text, "TimeoutMs": timeout_ms }))
            .send()
            .await?;
        check(resp, "jellyfin Sessions/{id}/Message")
            .await
            .map(|_| ())
    }

    pub async fn stop_playback(&self, session_id: &str) -> Result<()> {
        let resp = self
            .req(Method::POST, &format!("Sessions/{session_id}/Playing/Stop"))
            .send()
            .await?;
        check(resp, "jellyfin Sessions/{id}/Playing/Stop")
            .await
            .map(|_| ())
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

    /// Ordre des bibliothèques dans le menu d'un compte (`OrderedViews`) ; sans lui, Jellyfin les trie par nom
    /// et « Anime » passerait avant « Films ».
    pub async fn set_view_order(&self, user_id: &str, libraries: &[String]) -> Result<()> {
        let resp = self
            .req(Method::GET, &format!("Users/{user_id}"))
            .send()
            .await?;
        let user = json(resp, "jellyfin Users/{id}").await?;
        let mut cfg = user
            .get("Configuration")
            .cloned()
            .context("compte Jellyfin sans Configuration")?;
        cfg["OrderedViews"] = json!(libraries);
        let resp = self
            .req(Method::POST, &format!("Users/{user_id}/Configuration"))
            .json(&cfg)
            .send()
            .await?;
        check(resp, "jellyfin Users/{id}/Configuration")
            .await
            .map(|_| ())
    }

    /// Signale des fichiers nouveaux : Jellyfin ne scanne que leurs dossiers, pas la bibliothèque.
    pub async fn media_updated(&self, paths: &[String]) -> Result<()> {
        let updates: Vec<Value> = paths
            .iter()
            .map(|p| json!({ "Path": p, "UpdateType": "Created" }))
            .collect();
        let resp = self
            .req(Method::POST, "Library/Media/Updated")
            .json(&json!({ "Updates": updates }))
            .send()
            .await?;
        check(resp, "jellyfin Library/Media/Updated")
            .await
            .map(|_| ())
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

    /// Chemins des fichiers en cours de lecture (sessions actives depuis 5 min).
    pub async fn playing_paths(&self) -> Result<Vec<String>> {
        let resp = self
            .req(Method::GET, "Sessions")
            .query(&[("activeWithinSeconds", "300")])
            .send()
            .await?;
        let v = json(resp, "jellyfin Sessions").await?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.pointer("/NowPlayingItem/Path").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Politique utilisateur non-admin limitée aux bibliothèques données (ids Jellyfin), avec au plus
/// `max_streams` appareils connectés (`MaxActiveSessions`, 0 = illimité ; ne limite que les nouvelles connexions).
pub fn non_admin_policy(libraries: &[String], max_streams: u32) -> Value {
    json!({
        "IsAdministrator": false,
        // absent de la liste publique de l'écran de connexion (chacun tape son nom)
        "IsHidden": true,
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
        "EnabledFolders": libraries,
        "EnabledChannels": [],
        "EnabledDevices": [],
        "BlockedTags": [],
        "BlockedChannels": [],
        "BlockedMediaFolders": [],
        "AccessSchedules": [],
        "LoginAttemptsBeforeLockout": 5,
        "MaxActiveSessions": max_streams,
        "AuthenticationProviderId": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
        "PasswordResetProviderId": "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
        "SyncPlayAccess": "CreateAndJoinGroups"
    })
}
