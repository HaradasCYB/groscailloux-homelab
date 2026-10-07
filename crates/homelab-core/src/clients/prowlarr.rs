use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::Value;

use super::{json, SendRetry};
use crate::secret::Secret;

/// Prowlarr sert à une seule chose ici : chercher chez un indexer **en texte libre**. Sonarr ne sait
/// interroger que par série et saison, avec ses propres titres — une série dont les releases portent un
/// titre traduit (« L'attaque des Titans » pour *Attack on Titan*) n'est alors jamais trouvée.
#[derive(Clone)]
pub struct ProwlarrClient {
    base: Url,
    key: Secret,
    http: Client,
}

impl ProwlarrClient {
    pub fn new(base: &str, key: Secret, http: Client) -> Result<Self> {
        Ok(Self {
            base: Url::parse(base).context("URL Prowlarr invalide")?,
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

    /// Indexers configurés (id, nom).
    pub async fn indexers(&self) -> Result<Vec<Value>> {
        let resp = self.req(Method::GET, "api/v1/indexer").send_retry().await?;
        let v = json(resp, "prowlarr indexer").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Id du premier indexer dont le nom commence par `name` (insensible à la casse).
    pub async fn indexer_id(&self, name: &str) -> Result<Option<i64>> {
        let want = name.to_ascii_lowercase();
        Ok(self.indexers().await?.into_iter().find_map(|i| {
            let n = i.get("name").and_then(Value::as_str)?.to_ascii_lowercase();
            n.starts_with(&want)
                .then(|| i.get("id").and_then(Value::as_i64))
                .flatten()
        }))
    }

    /// Recherche **par identifiant TMDB** : une série et une saison (`tvsearch`), ou un film (`movie`). C411
    /// renvoie alors les releases de cette œuvre quel que soit leur nom (japonais, anglais, français) et
    /// porte l'identifiant TMDB de chacune (`tmdbId`). L'identifiant IMDb, lui, est ignoré par C411.
    pub async fn search_by_tmdb(
        &self,
        tmdb_id: i64,
        season: Option<i64>,
        indexer_id: i64,
    ) -> Result<Vec<Value>> {
        match season {
            Some(n) => {
                let q = format!("{{TmdbId:{tmdb_id}}}{{Season:{n}}}");
                self.tmdb_search(&q, "tvsearch", "5000", indexer_id).await
            }
            None => {
                let q = format!("{{TmdbId:{tmdb_id}}}");
                self.tmdb_search(&q, "movie", "2000", indexer_id).await
            }
        }
    }

    /// Toutes les releases d'une **série** portant cet identifiant, toutes saisons confondues (`{TmdbId}` sans
    /// `{Season}`, recherche TV). C'est la seule requête qui renvoie une **intégrale** : la recherche par saison
    /// ne la voit pas (Space Dandy, 2026-10-03 : 0 par saison, l'intégrale MULTi 1080p ici).
    pub async fn search_series_by_tmdb(&self, tmdb_id: i64, indexer_id: i64) -> Result<Vec<Value>> {
        let q = format!("{{TmdbId:{tmdb_id}}}");
        self.tmdb_search(&q, "tvsearch", "5000", indexer_id).await
    }

    async fn tmdb_search(
        &self,
        query: &str,
        kind: &str,
        cat: &str,
        indexer_id: i64,
    ) -> Result<Vec<Value>> {
        let id = indexer_id.to_string();
        let resp = self
            .req(Method::GET, "api/v1/search")
            .query(&[
                ("query", query),
                ("indexerIds", id.as_str()),
                ("categories", cat),
                ("type", kind),
                ("limit", "100"),
            ])
            .timeout(std::time::Duration::from_secs(120))
            // une recherche consomme le quota de l'indexer : jamais rejouée
            .send()
            .await?;
        let v = json(resp, "prowlarr search (tmdb)").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Vrai si Prowlarr a noté un échec récent de cet indexer (panne, 503, délai dépassé). Prowlarr répond alors
    /// à une recherche par une liste **vide**, sans erreur : sans ce contrôle, une panne de C411 passait pour
    /// « aucune release » et la recherche n'était refaite que 24 h plus tard (3 films d'un membre, 30/09).
    pub async fn indexer_failing(&self, indexer_id: i64) -> Result<bool> {
        let resp = self
            .req(Method::GET, "api/v1/indexerstatus")
            .send_retry()
            .await?;
        let v = json(resp, "prowlarr indexerstatus").await?;
        Ok(failing_in(&v, indexer_id, chrono::Utc::now()))
    }

    /// Sonde directe de l'indexer : sa page `caps` Torznab (sans clé, aucun quota). Le 30/09, C411 en maintenance
    /// renvoyait une page HTML « Incident en cours » avec un code 200 : Prowlarr échouait en silence (liste vide,
    /// rien dans `indexerstatus`). Vrai = réponse Torznab valide.
    pub async fn indexer_reachable(&self, indexer_id: i64) -> Result<bool> {
        let resp = self
            .req(Method::GET, &format!("api/v1/indexer/{indexer_id}"))
            .send_retry()
            .await?;
        let def = json(resp, "prowlarr indexer").await?;
        let field = |n: &str| {
            def.get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|f| f.get("name").and_then(Value::as_str) == Some(n))
                .and_then(|f| f.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let Some(base) = field("baseUrl") else {
            return Ok(true); // indexer sans URL lisible : on ne conclut rien
        };
        let path = field("apiPath").unwrap_or_else(|| "/api".into());
        let url = format!("{}{}?t=caps", base.trim_end_matches('/'), path);
        let Ok(r) = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .send_retry()
            .await
        else {
            return Ok(false);
        };
        let ok = r.status().is_success();
        let body = r.text().await.unwrap_or_default();
        Ok(ok && looks_like_caps(&body))
    }

    /// Contenu d'un `.torrent` à partir du lien de téléchargement renvoyé par une recherche (il passe par
    /// Prowlarr, qui porte la clé de l'indexer).
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(url)
            .header("X-Api-Key", self.key.expose())
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            // le lien porte la clé de Prowlarr et celui de l'indexer (`apikey=`, `link=`) : jamais dans l'erreur
            .map_err(reqwest::Error::without_url)
            .context("prowlarr download")?;
        let resp = super::check(resp, "prowlarr download").await?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Lien magnet d'une release qui n'a pas de `.torrent` (Nyaa) : Prowlarr répond par une redirection vers
    /// `magnet:…`, que le client HTTP ne sait pas suivre.
    pub async fn resolve_magnet(&self, url: &str) -> Result<String> {
        if url.starts_with("magnet:") {
            return Ok(url.to_string());
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        let resp = client
            .get(url)
            .header("X-Api-Key", self.key.expose())
            .send()
            .await
            .map_err(reqwest::Error::without_url) // même lien à clé que `download`
            .context("prowlarr magnet")?;
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        anyhow::ensure!(
            location.starts_with("magnet:"),
            "Prowlarr n'a pas renvoyé de lien magnet (HTTP {})",
            resp.status()
        );
        Ok(location.to_string())
    }

    /// Recherche en texte libre sur un indexer. Les résultats portent `title`, `downloadUrl`,
    /// `publishDate`, `size` et `seeders`.
    pub async fn search(&self, query: &str, indexer_id: i64, limit: u32) -> Result<Vec<Value>> {
        self.search_in(query, indexer_id, "5000", limit).await
    }

    /// Recherche en texte libre dans une catégorie (`2000` films, `5000` séries).
    pub async fn search_in(
        &self,
        query: &str,
        indexer_id: i64,
        categories: &str,
        limit: u32,
    ) -> Result<Vec<Value>> {
        let (id, lim) = (indexer_id.to_string(), limit.to_string());
        let resp = self
            .req(Method::GET, "api/v1/search")
            .query(&[
                ("query", query),
                ("indexerIds", id.as_str()),
                ("categories", categories),
                ("type", "search"),
                ("limit", lim.as_str()),
            ])
            .timeout(std::time::Duration::from_secs(120))
            // une recherche consomme le quota de l'indexer : jamais rejouée
            .send()
            .await?;
        let v = json(resp, "prowlarr search").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }
}

/// Réponse `t=caps` Torznab valide (et non une page HTML de maintenance).
pub fn looks_like_caps(body: &str) -> bool {
    let b = body.trim_start();
    b.contains("<caps") && !b.to_ascii_lowercase().starts_with("<!doctype html")
}

/// `indexerstatus` de Prowlarr : l'indexer est en échec s'il est mis de côté (`disabledTill` à venir) ou s'il a
/// échoué dans les 10 dernières minutes (`mostRecentFailure`).
pub fn failing_in(status: &Value, indexer_id: i64, now: chrono::DateTime<chrono::Utc>) -> bool {
    let at = |e: &Value, k: &str| {
        e.get(k)
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
    };
    status.as_array().into_iter().flatten().any(|e| {
        e.get("indexerId").and_then(Value::as_i64) == Some(indexer_id)
            && (at(e, "disabledTill").is_some_and(|d| d > now)
                || at(e, "mostRecentFailure")
                    .is_some_and(|d| now - d < chrono::Duration::minutes(10)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indexer_failure_is_detected_from_status() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-30T12:58:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // la panne du 30/09 : mis de côté jusqu'à 14:59:21 heure de Paris
        let st = json!([{ "indexerId": 1, "disabledTill": "2026-09-30T12:59:21Z", "mostRecentFailure": "2026-09-30T12:58:21Z" }]);
        assert!(failing_in(&st, 1, now));
        assert!(
            !failing_in(&st, 2, now),
            "un autre indexer n'est pas concerné"
        );
        let old = json!([{ "indexerId": 1, "mostRecentFailure": "2026-09-30T10:00:00Z" }]);
        assert!(!failing_in(&old, 1, now), "un échec ancien ne compte plus");
        let recent = json!([{ "indexerId": 1, "mostRecentFailure": "2026-09-30T12:55:00Z" }]);
        assert!(failing_in(&recent, 1, now));
        assert!(!failing_in(&json!([]), 1, now));
    }

    #[test]
    fn caps_probe_rejects_maintenance_page() {
        assert!(looks_like_caps(
            "<?xml version=\"1.0\"?><caps><server title=\"C411\"/></caps>"
        ));
        assert!(!looks_like_caps(
            "<!DOCTYPE html><html><head><title>Incident en cours - C411</title>"
        ));
        assert!(!looks_like_caps(""));
    }
}
