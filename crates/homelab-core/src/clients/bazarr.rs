//! Bazarr de la seedbox : lecture de son historique (sous-titres téléchargés ou extraits), pour que
//! `subtitle_sync` fasse relire les fiches Jellyfin concernées. Clé `X-API-KEY`, jamais journalisée.

use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::Value;

use super::json;
use crate::secret::Secret;

#[derive(Clone)]
pub struct BazarrClient {
    base: Url,
    key: Secret,
    http: Client,
}

/// Ligne d'historique Bazarr utile à `subtitle_sync`.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    /// Chemin du fichier de sous-titres **côté seedbox** (vide pour une suppression).
    pub subtitles_path: String,
    /// `parsed_timestamp` (« MM/DD/YY HH:MM:SS », heure locale de Bazarr) en secondes.
    pub at: i64,
    /// 1 = téléchargé / extrait ; autres valeurs : amélioré, supprimé, échec…
    pub action: i64,
    pub provider: String,
    pub language: String,
}

/// « 09/21/26 14:26:31 » → secondes (heure locale de Bazarr prise telle quelle : seules les comparaisons
/// entre lignes comptent).
pub fn parse_timestamp(s: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%y %H:%M:%S")
        .ok()
        .map(|d| d.and_utc().timestamp())
}

pub fn row_from_json(v: &Value) -> Option<HistoryRow> {
    Some(HistoryRow {
        subtitles_path: v
            .get("subtitles_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        at: parse_timestamp(v.get("parsed_timestamp").and_then(Value::as_str)?)?,
        action: v.get("action").and_then(Value::as_i64).unwrap_or(0),
        provider: v
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        language: v
            .pointer("/language/code2")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

impl BazarrClient {
    pub fn new(base: &str, key: Secret, http: Client) -> Result<Self> {
        Ok(Self {
            base: Url::parse(base).context("URL Bazarr invalide")?,
            key,
            http,
        })
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let url = self.base.join(path).expect("chemin API valide");
        self.http
            .request(method, url)
            .header("X-API-KEY", self.key.expose())
    }

    async fn history(&self, list: &str, length: usize) -> Result<Vec<HistoryRow>> {
        let resp = self
            .req(Method::GET, &format!("api/{list}/history"))
            .query(&[("start", "0"), ("length", &length.to_string())])
            .send()
            .await?;
        let v = json(resp, "bazarr history").await?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(row_from_json).collect())
            .unwrap_or_default())
    }

    /// Historique des épisodes, du plus récent au plus ancien.
    pub async fn history_episodes(&self, length: usize) -> Result<Vec<HistoryRow>> {
        self.history("episodes", length).await
    }

    /// Historique des films, du plus récent au plus ancien.
    pub async fn history_movies(&self, length: usize) -> Result<Vec<HistoryRow>> {
        self.history("movies", length).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timestamps_and_rows() {
        assert_eq!(parse_timestamp("09/21/26 14:26:31"), Some(1_790_000_791));
        assert!(parse_timestamp("48 seconds ago").is_none());
        let r = row_from_json(&json!({
            "subtitles_path": "/home/x/media/Anime/S/Season 1/S - S01E01.fr.srt",
            "parsed_timestamp": "09/21/26 14:26:31", "action": 1, "provider": "embeddedsubtitles",
            "language": {"code2": "fr", "code3": "fra"}
        }))
        .unwrap();
        assert_eq!(r.action, 1);
        assert_eq!(r.language, "fr");
        assert!(r.subtitles_path.ends_with(".fr.srt"));
        assert!(row_from_json(&json!({"action": 1})).is_none());
    }
}
