//! Clients HTTP minimalistes, un par service. Les payloads sont des
//! `serde_json::Value` là où l'ancien bash utilisait `jq`, et des structs
//! typées là où l'on filtre/décide.

mod arr;
pub mod jellyfin;
mod jellyseerr;
mod qbit;

pub use arr::{ArrClient, QueueItem};
pub use jellyfin::JellyfinClient;
pub use jellyseerr::JellyseerrClient;
pub use qbit::{QbitClient, Torrent};

use anyhow::{bail, Context, Result};
use reqwest::Response;

/// Convertit une réponse en erreur lisible (code + début du body) si non-2xx.
pub(crate) async fn check(resp: Response, what: &str) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let snippet: String = body.chars().take(200).collect();
    bail!("{what} → HTTP {status} {snippet}");
}

pub(crate) async fn json(resp: Response, what: &str) -> Result<serde_json::Value> {
    let resp = check(resp, what).await?;
    resp.json()
        .await
        .with_context(|| format!("{what} : JSON invalide"))
}
