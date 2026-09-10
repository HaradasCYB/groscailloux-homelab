use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::{json, Value};

use super::{check, json};
use crate::secret::Secret;

#[derive(Clone)]
pub struct JellyseerrClient {
    base: Url,
    key: Secret,
    http: Client,
}

impl JellyseerrClient {
    pub fn new(base: &str, key: Secret, http: Client) -> Result<Self> {
        Ok(Self {
            base: Url::parse(base).context("URL Jellyseerr invalide")?,
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

    pub async fn status(&self) -> Result<Value> {
        let resp = self.req(Method::GET, "api/v1/status").send().await?;
        json(resp, "jellyseerr status").await
    }

    pub async fn users(&self, take: u32) -> Result<Vec<Value>> {
        let resp = self
            .req(Method::GET, "api/v1/user")
            .query(&[("take", take.to_string())])
            .send()
            .await?;
        let v = json(resp, "jellyseerr user").await?;
        Ok(v.get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub async fn delete_user(&self, id: i64) -> Result<()> {
        let resp = self
            .req(Method::DELETE, &format!("api/v1/user/{id}"))
            .send()
            .await?;
        check(resp, "jellyseerr DELETE user").await.map(|_| ())
    }

    pub async fn import_from_jellyfin(&self, jellyfin_ids: &[&str]) -> Result<Value> {
        let body = json!({ "jellyfinUserIds": jellyfin_ids });
        let resp = self
            .req(Method::POST, "api/v1/user/import-from-jellyfin")
            .json(&body)
            .send()
            .await?;
        json(resp, "jellyseerr import-from-jellyfin").await
    }

    pub async fn set_main_settings(&self, id: i64, email: &str, username: &str) -> Result<()> {
        let body = json!({ "email": email, "username": username });
        let resp = self
            .req(Method::POST, &format!("api/v1/user/{id}/settings/main"))
            .json(&body)
            .send()
            .await?;
        check(resp, "jellyseerr settings/main").await.map(|_| ())
    }

    /// Toutes les requêtes (paginé par 100), tous statuts confondus.
    pub async fn all_requests(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut skip = 0u32;
        loop {
            let resp = self
                .req(Method::GET, "api/v1/request")
                .query(&[
                    ("take", "100"),
                    ("skip", &skip.to_string()),
                    ("filter", "all"),
                ])
                .send()
                .await?;
            let v = json(resp, "jellyseerr request").await?;
            let page = v
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let n = page.len() as u32;
            out.extend(page);
            let total = v
                .pointer("/pageInfo/results")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            skip += n;
            if n == 0 || skip >= total {
                break;
            }
        }
        Ok(out)
    }
}
