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

    /// Épisodes manquants suivis (toutes les pages), du plus récent au plus ancien.
    pub async fn wanted_missing(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 1..=20 {
            let p = page.to_string();
            let v = self
                .get(
                    "api/v3/wanted/missing",
                    &[
                        ("page", p.as_str()),
                        ("pageSize", "500"),
                        ("monitored", "true"),
                        ("sortKey", "airDateUtc"),
                        ("sortDirection", "descending"),
                    ],
                )
                .await?;
            let recs = v
                .get("records")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = v.get("totalRecords").and_then(Value::as_i64).unwrap_or(0);
            out.extend(recs);
            if out.len() as i64 >= total {
                break;
            }
        }
        Ok(out)
    }

    /// Recherche interactive d'un seul épisode. Pour un anime, Sonarr interroge l'indexer épisode par
    /// épisode : une recherche de saison entière dépasse le délai du proxy de la seedbox (504 après 300 s).
    pub async fn releases_for_episode(&self, episode_id: i64) -> Result<Vec<Value>> {
        let id = episode_id.to_string();
        let resp = self
            .req(Method::GET, "api/v3/release")
            .query(&[("episodeId", id.as_str())])
            // ~200 s en pratique (tous les indexers interactifs sont interrogés) ; le proxy de la
            // seedbox coupe à 300 s, on reste juste en dessous.
            .timeout(std::time::Duration::from_secs(280))
            .send()
            .await?;
        let v = json(resp, &format!("{} GET release (épisode)", self.name)).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Recherche interactive d'une saison (tous les indexers en recherche interactive).
    pub async fn releases(&self, series_id: i64, season: i64) -> Result<Vec<Value>> {
        let (s, n) = (series_id.to_string(), season.to_string());
        let resp = self
            .req(Method::GET, "api/v3/release")
            .query(&[("seriesId", s.as_str()), ("seasonNumber", n.as_str())])
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await?;
        let v = json(resp, &format!("{} GET release", self.name)).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Envoie une release au client en forçant son rattachement (série et épisodes fournis).
    pub async fn grab_override(
        &self,
        release: &Value,
        series_id: i64,
        episode_ids: &[i64],
        download_client_id: i64,
    ) -> Result<Value> {
        let body = json!({
            "guid": release.get("guid"),
            "indexerId": release.get("indexerId"),
            "shouldOverride": true,
            "seriesId": series_id,
            "episodeIds": episode_ids,
            "quality": release.get("quality"),
            "languages": release.get("languages").cloned().unwrap_or_else(|| json!([])),
            "downloadClientId": download_client_id,
        });
        self.post("api/v3/release", &body).await
    }

    /// Pousse une release trouvée ailleurs (Prowlarr, titre traduit) : Sonarr la parse, la rattache à la
    /// série et la confie à son client de téléchargement. Renvoie la décision (`approved`, `rejections`).
    pub async fn push_release(
        &self,
        title: &str,
        download_url: &str,
        publish_date: Option<&str>,
        indexer: &str,
    ) -> Result<Value> {
        let body = json!({
            "title": title,
            "downloadUrl": download_url,
            "protocol": "torrent",
            "publishDate": publish_date.unwrap_or("2000-01-01T00:00:00Z"),
            "indexer": indexer,
        });
        self.post("api/v3/release/push", &body).await
    }

    pub async fn quality_profile(&self, id: i64) -> Result<Value> {
        self.get(&format!("api/v3/qualityprofile/{id}"), &[]).await
    }

    /// `DELETE` avec paramètres (fiche film / série). Réponse ignorée.
    pub async fn delete(&self, path: &str, query: &[(&str, &str)]) -> Result<()> {
        let resp = self.req(Method::DELETE, path).query(query).send().await?;
        check(resp, &format!("{} DELETE {path}", self.name))
            .await
            .map(|_| ())
    }

    /// Films avec leur `movieFile` (Radarr).
    pub async fn movies(&self) -> Result<Vec<Value>> {
        let v = self.get("api/v3/movie", &[]).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Fichiers d'épisodes d'une série (Sonarr).
    pub async fn episode_files(&self, series_id: i64) -> Result<Vec<Value>> {
        let v = self
            .get(
                "api/v3/episodefile",
                &[("seriesId", &series_id.to_string())],
            )
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Historique d'un film (`history/movie?movieId=`) ou d'une série (`history/series?seriesId=`).
    /// Attention : `GET history?movieId=` n'existe pas (filtre ignoré ⇒ tout l'historique).
    pub async fn title_history(&self, id: i64) -> Result<Vec<Value>> {
        let (path, key) = if self.is_radarr() {
            ("api/v3/history/movie", "movieId")
        } else {
            ("api/v3/history/series", "seriesId")
        };
        let v = self.get(path, &[(key, &id.to_string())]).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Historique filtré par `downloadId` (filtre supporté par Radarr et Sonarr), 250 évènements au plus.
    pub async fn history_where(&self, key: &str, value: &str) -> Result<Vec<Value>> {
        let v = self
            .get(
                "api/v3/history",
                &[(key, value), ("pageSize", "250"), ("sortKey", "date")],
            )
            .await?;
        Ok(v.get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Épisodes suivis ou non (`PUT episode/monitor`).
    pub async fn set_episodes_monitored(&self, ids: &[i64], monitored: bool) -> Result<()> {
        self.put(
            "api/v3/episode/monitor",
            &json!({ "episodeIds": ids, "monitored": monitored }),
        )
        .await
        .map(|_| ())
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
