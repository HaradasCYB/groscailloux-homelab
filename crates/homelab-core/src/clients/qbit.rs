use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

use super::{check, json};

/// qBittorrent WebUI API v2. Pas d'auth : l'IP du daemon est whitelistée côté qBit.
#[derive(Clone)]
pub struct QbitClient {
    base: Url,
    http: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Torrent {
    pub hash: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub tracker: String,
    #[serde(default = "minus_two")]
    pub ratio_limit: f64,
    #[serde(default = "minus_two_i")]
    pub seeding_time_limit: i64,
    #[serde(default)]
    pub completion_on: i64,
    #[serde(default)]
    pub size: i64,
}

fn minus_two() -> f64 {
    -2.0
}
fn minus_two_i() -> i64 {
    -2
}

impl QbitClient {
    pub fn new(base: &str, http: Client) -> Result<Self> {
        Ok(Self {
            base: Url::parse(base).context("URL qBittorrent invalide")?,
            http,
        })
    }

    fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("chemin API valide")
    }

    pub async fn version(&self) -> Result<String> {
        let resp = self.http.get(self.url("api/v2/app/version")).send().await?;
        Ok(check(resp, "qbit version").await?.text().await?)
    }

    pub async fn torrents(&self) -> Result<Vec<Torrent>> {
        let resp = self
            .http
            .get(self.url("api/v2/torrents/info"))
            .send()
            .await?;
        let v = json(resp, "qbit torrents/info").await?;
        serde_json::from_value(v).context("torrents/info : payload inattendu")
    }

    pub async fn set_share_limits(
        &self,
        hash: &str,
        ratio: f64,
        seeding_time_min: i64,
    ) -> Result<()> {
        let resp = self
            .http
            .post(self.url("api/v2/torrents/setShareLimits"))
            .form(&[
                ("hashes", hash.to_string()),
                ("ratioLimit", format_ratio(ratio)),
                ("seedingTimeLimit", seeding_time_min.to_string()),
                ("inactiveSeedingTimeLimit", "-1".to_string()),
            ])
            .send()
            .await?;
        check(resp, "qbit setShareLimits").await.map(|_| ())
    }

    pub async fn delete(&self, hashes: &[String], delete_files: bool) -> Result<()> {
        let resp = self
            .http
            .post(self.url("api/v2/torrents/delete"))
            .form(&[
                ("hashes", hashes.join("|")),
                ("deleteFiles", delete_files.to_string()),
            ])
            .send()
            .await?;
        check(resp, "qbit torrents/delete").await.map(|_| ())
    }
}

/// `-1` reste `-1`, sinon deux décimales (ce que qBit renvoie dans `ratio_limit`).
pub fn format_ratio(r: f64) -> String {
    if r < 0.0 {
        "-1".into()
    } else {
        format!("{r:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_formatting() {
        assert_eq!(format_ratio(-1.0), "-1");
        assert_eq!(format_ratio(2.0), "2.00");
        assert_eq!(format_ratio(1.0), "1.00");
    }
}
