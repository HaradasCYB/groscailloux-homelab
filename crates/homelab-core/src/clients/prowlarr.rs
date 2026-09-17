use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::Value;

use super::json;
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
        let resp = self.req(Method::GET, "api/v1/indexer").send().await?;
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

    /// Recherche en texte libre sur un indexer. Les résultats portent `title`, `downloadUrl`,
    /// `publishDate`, `size` et `seeders`.
    pub async fn search(&self, query: &str, indexer_id: i64, limit: u32) -> Result<Vec<Value>> {
        let (id, lim) = (indexer_id.to_string(), limit.to_string());
        let resp = self
            .req(Method::GET, "api/v1/search")
            .query(&[
                ("query", query),
                ("indexerIds", id.as_str()),
                ("categories", "5000"),
                ("type", "search"),
                ("limit", lim.as_str()),
            ])
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?;
        let v = json(resp, "prowlarr search").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }
}
