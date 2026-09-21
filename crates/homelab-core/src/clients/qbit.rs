use std::sync::Arc;

use anyhow::{bail, Context, Result};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::Deserialize;
use tokio::sync::Mutex;

use super::{check, json};
use crate::secret::Secret;

/// qBittorrent WebUI API v2.
///
/// Sans identifiants (qBittorrent du VPS) : l'IP du daemon est whitelistée côté qBit.
/// Avec identifiants (qBittorrent de la seedbox, derrière le proxy HTTPS de l'hébergeur) :
/// session par cookie `SID`, ouverte à la première requête et rouverte sur 403.
#[derive(Clone)]
pub struct QbitClient {
    base: Url,
    http: Client,
    login: Option<(String, Secret)>,
    sid: Arc<Mutex<Option<String>>>,
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
    #[serde(default)]
    pub progress: f64,
    /// Fichier (torrent mono-fichier) ou dossier racine du torrent, chemin vu par qBittorrent.
    #[serde(default)]
    pub content_path: String,
    /// Dossier d'enregistrement ; les `name` de `torrents/files` lui sont relatifs.
    #[serde(default)]
    pub save_path: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub ratio: f64,
    /// Secondes passées en seed.
    #[serde(default)]
    pub seeding_time: i64,
    /// Étiquettes séparées par des virgules (`homelab:series=50:season=4` posé par series_search).
    #[serde(default)]
    pub tags: String,
    /// Secondes restantes estimées par qBittorrent ; 8640000 = inconnu.
    #[serde(default = "eta_unknown")]
    pub eta: i64,
    #[serde(default)]
    pub dlspeed: i64,
}

/// Valeur que qBittorrent renvoie quand il n'a pas d'estimation.
pub const ETA_UNKNOWN: i64 = 8_640_000;

fn eta_unknown() -> i64 {
    ETA_UNKNOWN
}

#[derive(Debug, Clone, Deserialize)]
pub struct TorrentFile {
    /// Chemin relatif au dossier d'enregistrement du torrent.
    pub name: String,
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
            login: None,
            sid: Arc::new(Mutex::new(None)),
        })
    }

    /// Client authentifié (WebUI protégée par mot de passe). `base` doit finir par `/`.
    pub fn with_login(base: &str, user: &str, password: Secret, http: Client) -> Result<Self> {
        let mut c = Self::new(base, http)?;
        c.login = Some((user.to_string(), password));
        Ok(c)
    }

    fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("chemin API valide")
    }

    async fn authenticate(&self) -> Result<String> {
        let (user, password) = self
            .login
            .as_ref()
            .context("client qBit sans identifiants")?;
        let resp = self
            .http
            .post(self.url("api/v2/auth/login"))
            .form(&[("username", user.as_str()), ("password", password.expose())])
            .send()
            .await?;
        let resp = check(resp, "qbit auth/login").await?;
        let cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(sid_cookie);
        let body = resp.text().await.unwrap_or_default();
        match cookie {
            Some(c) => Ok(c),
            None => bail!("qbit auth/login refusé ({})", body.trim()),
        }
    }

    /// Envoie la requête construite par `build`, avec la session si le client en a une.
    async fn send(
        &self,
        build: impl Fn(RequestBuilder) -> RequestBuilder,
        method: Method,
        path: &str,
    ) -> Result<Response> {
        if self.login.is_none() {
            return Ok(build(self.http.request(method, self.url(path)))
                .send()
                .await?);
        }
        for attempt in 0..2 {
            let cookie = {
                let mut sid = self.sid.lock().await;
                if sid.is_none() || attempt == 1 {
                    *sid = Some(self.authenticate().await?);
                }
                sid.clone().unwrap_or_default()
            };
            let resp = build(self.http.request(method.clone(), self.url(path)))
                .header(reqwest::header::COOKIE, cookie)
                .send()
                .await?;
            if resp.status() != StatusCode::FORBIDDEN || attempt == 1 {
                return Ok(resp);
            }
        }
        unreachable!("deux tentatives au plus")
    }

    pub async fn version(&self) -> Result<String> {
        let resp = self.send(|r| r, Method::GET, "api/v2/app/version").await?;
        Ok(check(resp, "qbit version").await?.text().await?)
    }

    pub async fn torrents(&self) -> Result<Vec<Torrent>> {
        let resp = self
            .send(|r| r, Method::GET, "api/v2/torrents/info")
            .await?;
        let v = json(resp, "qbit torrents/info").await?;
        serde_json::from_value(v).context("torrents/info : payload inattendu")
    }

    pub async fn files(&self, hash: &str) -> Result<Vec<TorrentFile>> {
        let resp = self
            .send(
                |r| r.query(&[("hash", hash)]),
                Method::GET,
                "api/v2/torrents/files",
            )
            .await?;
        let v = json(resp, "qbit torrents/files").await?;
        serde_json::from_value(v).context("torrents/files : payload inattendu")
    }

    pub async fn set_share_limits(
        &self,
        hash: &str,
        ratio: f64,
        seeding_time_min: i64,
    ) -> Result<()> {
        let form = [
            ("hashes", hash.to_string()),
            ("ratioLimit", format_ratio(ratio)),
            ("seedingTimeLimit", seeding_time_min.to_string()),
            ("inactiveSeedingTimeLimit", "-1".to_string()),
        ];
        let resp = self
            .send(
                |r| r.form(&form),
                Method::POST,
                "api/v2/torrents/setShareLimits",
            )
            .await?;
        check(resp, "qbit setShareLimits").await.map(|_| ())
    }

    /// Ajoute un `.torrent` (contenu brut), démarré, avec des étiquettes. `save_path` vide = dossier par défaut.
    /// Ajout par lien (magnet), avec étiquettes.
    pub async fn add_url(&self, url: &str, tags: &str) -> Result<()> {
        let (url, tags) = (url.to_string(), tags.to_string());
        let resp = self
            .send(
                move |r| {
                    r.form(&[
                        ("urls", url.as_str()),
                        ("tags", tags.as_str()),
                        ("paused", "false"),
                        ("stopped", "false"),
                    ])
                },
                Method::POST,
                "api/v2/torrents/add",
            )
            .await?;
        let resp = check(resp, "qbit torrents/add").await?;
        let body = resp.text().await.unwrap_or_default();
        if body.trim().eq_ignore_ascii_case("fails.") {
            bail!("qBittorrent a refusé le lien (déjà présent ou invalide)");
        }
        Ok(())
    }

    pub async fn add_torrent(&self, torrent: Vec<u8>, save_path: &str, tags: &str) -> Result<()> {
        let (save, tags) = (save_path.to_string(), tags.to_string());
        let resp = self
            .send(
                move |r| {
                    let part = reqwest::multipart::Part::bytes(torrent.clone())
                        .file_name("release.torrent")
                        .mime_str("application/x-bittorrent")
                        .expect("type MIME valide");
                    let mut form = reqwest::multipart::Form::new()
                        .part("torrents", part)
                        .text("tags", tags.clone())
                        .text("paused", "false")
                        .text("stopped", "false");
                    if !save.is_empty() {
                        form = form.text("savepath", save.clone());
                    }
                    r.multipart(form)
                },
                Method::POST,
                "api/v2/torrents/add",
            )
            .await?;
        let resp = check(resp, "qbit torrents/add").await?;
        let body = resp.text().await.unwrap_or_default();
        if body.trim().eq_ignore_ascii_case("fails.") {
            bail!("qBittorrent a refusé le torrent (déjà présent ou invalide)");
        }
        Ok(())
    }

    /// Remplace les étiquettes d'un torrent déjà présent (les anciennes `homelab:` d'abord retirées).
    /// Sert quand un titre supprimé est redemandé : le torrent est encore là, complet, et il suffit de
    /// le rattacher à la nouvelle fiche pour que `torrent_import` l'importe sans rien retélécharger.
    pub async fn retag(&self, hash: &str, old_tags: &str, new_tag: &str) -> Result<()> {
        let stale: Vec<&str> = old_tags
            .split(',')
            .map(str::trim)
            .filter(|t| t.starts_with("homelab:") && *t != new_tag)
            .collect();
        if !stale.is_empty() {
            let form = [
                ("hashes".to_string(), hash.to_string()),
                ("tags".to_string(), stale.join(",")),
            ];
            let resp = self
                .send(
                    |r| r.form(&form),
                    Method::POST,
                    "api/v2/torrents/removeTags",
                )
                .await?;
            check(resp, "qbit torrents/removeTags").await?;
        }
        let form = [
            ("hashes".to_string(), hash.to_string()),
            ("tags".to_string(), new_tag.to_string()),
        ];
        let resp = self
            .send(|r| r.form(&form), Method::POST, "api/v2/torrents/addTags")
            .await?;
        check(resp, "qbit torrents/addTags").await.map(|_| ())
    }

    pub async fn delete(&self, hashes: &[String], delete_files: bool) -> Result<()> {
        let form = [
            ("hashes", hashes.join("|")),
            ("deleteFiles", delete_files.to_string()),
        ];
        let resp = self
            .send(|r| r.form(&form), Method::POST, "api/v2/torrents/delete")
            .await?;
        check(resp, "qbit torrents/delete").await.map(|_| ())
    }
}

/// `SID=abc; HttpOnly; path=/` → `SID=abc`.
fn sid_cookie(header: &str) -> Option<String> {
    let pair = header.split(';').next()?.trim();
    let value = pair.strip_prefix("SID=")?;
    (!value.is_empty()).then(|| pair.to_string())
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

    #[test]
    fn extracts_sid_cookie() {
        assert_eq!(
            sid_cookie("SID=AbC123; HttpOnly; path=/").as_deref(),
            Some("SID=AbC123")
        );
        assert_eq!(sid_cookie("SID=; path=/"), None);
        assert_eq!(sid_cookie("QBT_SID_8080=x; path=/"), None);
    }
}
