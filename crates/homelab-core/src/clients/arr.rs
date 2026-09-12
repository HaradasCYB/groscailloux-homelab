use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{check, json};
use crate::secret::Secret;

/// Sonarr et Radarr partagent l'API v3 ; `name` sert aux logs et à l'état.
#[derive(Clone)]
pub struct ArrClient {
    pub name: &'static str,
    base: Url,
    key: Secret,
    http: Client,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: i64,
    #[serde(default)]
    pub download_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub error_message: Option<String>,
}

impl ArrClient {
    pub fn new(name: &'static str, base: &str, key: Secret, http: Client) -> Result<Self> {
        let base = Url::parse(base).with_context(|| format!("URL {name} invalide"))?;
        Ok(Self {
            name,
            base,
            key,
            http,
        })
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let url = self.base.join(path).expect("chemin API valide");
        self.http
            .request(method, url)
            .header("X-Api-Key", self.key.expose())
    }

    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let resp = self.req(Method::GET, path).query(query).send().await?;
        json(resp, &format!("{} GET {path}", self.name)).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self.req(Method::POST, path).json(body).send().await?;
        json(resp, &format!("{} POST {path}", self.name)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self.req(Method::PUT, path).json(body).send().await?;
        json(resp, &format!("{} PUT {path}", self.name)).await
    }

    pub async fn ping(&self) -> Result<()> {
        let resp = self.req(Method::GET, "ping").send().await?;
        check(resp, &format!("{} ping", self.name))
            .await
            .map(|_| ())
    }

    pub async fn queue(&self) -> Result<Vec<QueueItem>> {
        let v = self.get("api/v3/queue", &[("pageSize", "200")]).await?;
        let records = v.get("records").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(records).context("queue : records invalides")
    }

    /// Retire l'item de la queue ET du client de téléchargement, blocklist la release.
    pub async fn remove_queue_item(&self, id: i64) -> Result<()> {
        let resp = self
            .req(Method::DELETE, &format!("api/v3/queue/{id}"))
            .query(&[
                ("removeFromClient", "true"),
                ("blocklist", "true"),
                ("skipRedownload", "false"),
                ("changeCategory", "false"),
            ])
            .send()
            .await?;
        check(resp, &format!("{} DELETE queue/{id}", self.name))
            .await
            .map(|_| ())
    }

    pub async fn command(&self, body: Value) -> Result<Value> {
        self.post("api/v3/command", &body).await
    }

    pub async fn series(&self) -> Result<Vec<Value>> {
        let v = self.get("api/v3/series", &[]).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub async fn put_series(&self, id: i64, series: &Value) -> Result<Value> {
        self.put(&format!("api/v3/series/{id}"), series).await
    }

    /// Sonarr parcourt tout le dossier : plusieurs dizaines de secondes sur un gros /downloads.
    pub async fn manual_import(&self, folder: &str) -> Result<Vec<Value>> {
        let resp = self
            .req(Method::GET, "api/v3/manualimport")
            .query(&[("folder", folder), ("filterExistingFiles", "true")])
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await?;
        let v = json(resp, &format!("{} GET manualimport", self.name)).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub async fn parse(&self, title: &str) -> Result<Value> {
        self.get("api/v3/parse", &[("title", title)]).await
    }

    /// `kind` = "movie" ou "series".
    pub async fn lookup(&self, kind: &str, term: &str) -> Result<Vec<Value>> {
        let v = self
            .get(&format!("api/v3/{kind}/lookup"), &[("term", term)])
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub async fn exists(&self, kind: &str, id_param: &str, id: i64) -> Result<bool> {
        let v = self
            .get(&format!("api/v3/{kind}"), &[(id_param, &id.to_string())])
            .await?;
        Ok(v.as_array().map(|a| !a.is_empty()).unwrap_or(false))
    }

    pub async fn add(&self, kind: &str, body: &Value) -> Result<Value> {
        self.post(&format!("api/v3/{kind}"), body).await
    }

    pub async fn download_clients(&self) -> Result<Vec<Value>> {
        let v = self.get("api/v3/downloadclient", &[]).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub async fn put_download_client(&self, id: i64, client: &Value) -> Result<()> {
        let resp = self
            .req(Method::PUT, &format!("api/v3/downloadclient/{id}"))
            .query(&[("forceSave", "true")])
            .json(client)
            .send()
            .await?;
        check(resp, &format!("{} PUT downloadclient/{id}", self.name))
            .await
            .map(|_| ())
    }

    /// Queue brute (tous les champs), pour les tâches qui lisent statusMessages/trackedDownloadState.
    pub async fn queue_records(&self) -> Result<Vec<Value>> {
        let v = self.get("api/v3/queue", &[("pageSize", "200")]).await?;
        Ok(v.get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Aperçu d'import manuel d'un téléchargement, rattaché à une fiche (`movieId` / `seriesId`).
    pub async fn manual_import_download(
        &self,
        download_id: &str,
        id_param: &str,
        id: i64,
    ) -> Result<Vec<Value>> {
        let id = id.to_string();
        let resp = self
            .req(Method::GET, "api/v3/manualimport")
            .query(&[
                ("downloadId", download_id),
                (id_param, id.as_str()),
                ("filterExistingFiles", "false"),
            ])
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await?;
        let v = json(resp, &format!("{} GET manualimport", self.name)).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub fn is_radarr(&self) -> bool {
        self.name.starts_with("radarr")
    }

    /// Imports réussis (`downloadFolderImported`, eventType 3), du plus récent au plus ancien.
    pub async fn recent_imports(&self, page_size: u32) -> Result<Vec<Value>> {
        let size = page_size.to_string();
        let v = self
            .get(
                "api/v3/history",
                &[
                    ("page", "1"),
                    ("pageSize", &size),
                    ("sortKey", "id"),
                    ("sortDirection", "descending"),
                    ("eventType", "3"),
                ],
            )
            .await?;
        Ok(v.get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Nombre d'événements d'historique (grab, import…) liés à ce téléchargement.
    /// Les Arrs stockent l'id qBittorrent en majuscules et le filtre y est sensible.
    pub async fn history_count_for_download(&self, hash: &str) -> Result<i64> {
        let id = hash.to_ascii_uppercase();
        let v = self
            .get(
                "api/v3/history",
                &[("downloadId", id.as_str()), ("pageSize", "1")],
            )
            .await?;
        Ok(v.get("totalRecords").and_then(Value::as_i64).unwrap_or(0))
    }

    /// Aperçu d'import manuel d'un fichier ou dossier quelconque, rattaché à une fiche.
    /// Sans `downloadId` : pour un téléchargement que l'Arr ne suit pas, il renverrait une liste vide.
    pub async fn manual_import_folder(
        &self,
        folder: &str,
        id_param: &str,
        id: i64,
    ) -> Result<Vec<Value>> {
        let id = id.to_string();
        let resp = self
            .req(Method::GET, "api/v3/manualimport")
            .query(&[
                ("folder", folder),
                (id_param, id.as_str()),
                ("filterExistingFiles", "false"),
            ])
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await?;
        let v = json(resp, &format!("{} GET manualimport", self.name)).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Fiche existante par identifiant externe (`movie`/`tmdbId` ou `series`/`tvdbId`).
    pub async fn find_by(&self, kind: &str, id_param: &str, id: i64) -> Result<Option<Value>> {
        let v = self
            .get(&format!("api/v3/{kind}"), &[(id_param, &id.to_string())])
            .await?;
        Ok(v.as_array().and_then(|a| a.first().cloned()))
    }

    /// Épisodes d'une série (id, saison, numéro, numéro absolu).
    pub async fn episodes(&self, series_id: i64) -> Result<Vec<Value>> {
        let v = self
            .get("api/v3/episode", &[("seriesId", &series_id.to_string())])
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Épisodes connus d'une série (vide juste après l'ajout, le temps du refresh).
    pub async fn episode_count(&self, series_id: i64) -> Result<usize> {
        let v = self
            .get("api/v3/episode", &[("seriesId", &series_id.to_string())])
            .await?;
        Ok(v.as_array().map(Vec::len).unwrap_or(0))
    }

    pub fn scan_command(&self, path: &str) -> Value {
        let name = if self.is_radarr() {
            "DownloadedMoviesScan"
        } else {
            "DownloadedEpisodesScan"
        };
        json!({ "name": name, "path": path, "importMode": "auto" })
    }
}
