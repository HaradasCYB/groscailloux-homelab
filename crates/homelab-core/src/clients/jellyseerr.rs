use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::{json, Value};

use super::{check, json, SendRetry};
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

    /// GET brut (réglages) ; les réponses de `settings/*` peuvent contenir des secrets : ne jamais les afficher.
    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let resp = self.req(Method::GET, path).send_retry().await?;
        json(resp, &format!("jellyseerr GET {path}")).await
    }

    /// POST brut (réglages, tests).
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<()> {
        let resp = self.req(Method::POST, path).json(body).send_retry().await?;
        check(resp, &format!("jellyseerr POST {path}"))
            .await
            .map(|_| ())
    }

    /// Fiche série TMDB vue par Jellyseerr, titres en français (`name`) et d'origine (`originalName`).
    pub async fn tv_details(&self, tmdb_id: i64) -> Result<Value> {
        let resp = self
            .req(Method::GET, &format!("api/v1/tv/{tmdb_id}"))
            .query(&[("language", "fr")])
            .send_retry()
            .await?;
        json(resp, "jellyseerr tv").await
    }

    pub async fn movie_details(&self, tmdb_id: i64) -> Result<Value> {
        let resp = self
            .req(Method::GET, &format!("api/v1/movie/{tmdb_id}"))
            .query(&[("language", "fr")])
            .send_retry()
            .await?;
        json(resp, "jellyseerr movie").await
    }

    /// Langue d'origine TMDB d'un film (`kind = "movie"`) ou d'une série (`"tv"`), code ISO 639-1 tel que TMDB le
    /// donne (`ja`, `en`, `fr`…), `None` si TMDB n'en a pas. Même source que `/compte/api/original` (mode VO de
    /// Mon compte) et que la tâche `original_language` (2026-10-08).
    pub async fn original_language(&self, kind: &str, tmdb_id: i64) -> Result<Option<String>> {
        let d = if kind == "tv" {
            self.tv_details(tmdb_id).await?
        } else {
            self.movie_details(tmdb_id).await?
        };
        Ok(Some(original_language_of(&d)).filter(|l| !l.is_empty()))
    }

    pub async fn status(&self) -> Result<Value> {
        let resp = self.req(Method::GET, "api/v1/status").send_retry().await?;
        json(resp, "jellyseerr status").await
    }

    pub async fn users(&self, take: u32) -> Result<Vec<Value>> {
        let resp = self
            .req(Method::GET, "api/v1/user")
            .query(&[("take", take.to_string())])
            .send_retry()
            .await?;
        let v = json(resp, "jellyseerr user").await?;
        Ok(v.get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Permissions données par Jellyseerr aux nouveaux comptes (`settings/main`).
    pub async fn default_permissions(&self) -> Result<i64> {
        let resp = self
            .req(Method::GET, "api/v1/settings/main")
            .send_retry()
            .await?;
        json(resp, "jellyseerr settings/main")
            .await?
            .get("defaultPermissions")
            .and_then(Value::as_i64)
            .context("settings/main sans defaultPermissions")
    }

    /// Permissions Jellyseerr (masque de bits : 2 = admin, 32 = demander…).
    pub async fn set_permissions(&self, id: i64, permissions: i64) -> Result<()> {
        let resp = self
            .req(
                Method::POST,
                &format!("api/v1/user/{id}/settings/permissions"),
            )
            .json(&json!({ "permissions": permissions }))
            .send_retry()
            .await?;
        check(resp, "jellyseerr settings/permissions")
            .await
            .map(|_| ())
    }

    /// Id du média Jellyseerr d'un film / d'une série (`kind` = `movie` ou `tv`), s'il existe.
    pub async fn media_id(&self, kind: &str, tmdb_id: i64) -> Result<Option<i64>> {
        let resp = self
            .req(Method::GET, &format!("api/v1/{kind}/{tmdb_id}"))
            .send_retry()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(json(resp, "jellyseerr media")
            .await?
            .pointer("/mediaInfo/id")
            .and_then(Value::as_i64))
    }

    /// Supprime le média (et ses demandes) : le titre redevient demandable.
    pub async fn delete_media(&self, id: i64) -> Result<()> {
        let resp = self
            .req(Method::DELETE, &format!("api/v1/media/{id}"))
            .send_retry()
            .await?;
        check(resp, "jellyseerr DELETE media").await.map(|_| ())
    }

    pub async fn delete_user(&self, id: i64) -> Result<()> {
        let resp = self
            .req(Method::DELETE, &format!("api/v1/user/{id}"))
            .send_retry()
            .await?;
        check(resp, "jellyseerr DELETE user").await.map(|_| ())
    }

    pub async fn import_from_jellyfin(&self, jellyfin_ids: &[&str]) -> Result<Value> {
        let body = json!({ "jellyfinUserIds": jellyfin_ids });
        let resp = self
            .req(Method::POST, "api/v1/user/import-from-jellyfin")
            .json(&body)
            .send_retry()
            .await?;
        json(resp, "jellyseerr import-from-jellyfin").await
    }

    pub async fn set_main_settings(&self, id: i64, email: &str, username: &str) -> Result<()> {
        let body = json!({ "email": email, "username": username });
        let resp = self
            .req(Method::POST, &format!("api/v1/user/{id}/settings/main"))
            .json(&body)
            .send_retry()
            .await?;
        check(resp, "jellyseerr settings/main").await.map(|_| ())
    }

    /// Toutes les fiches média (paginé par 100) : un média sans demande = demande retirée par l'admin.
    pub async fn all_media(&self) -> Result<Vec<Value>> {
        Ok(self.all_media_counted().await?.0)
    }

    /// Comme [`Self::all_media`], avec le total annoncé par Jellyseerr (`pageInfo.results` de la dernière page lue) :
    /// une liste qui en compte moins est incomplète. Chaque fiche porte ses saisons (`seasons[].status`).
    pub async fn all_media_counted(&self) -> Result<(Vec<Value>, u64)> {
        let mut out = Vec::new();
        let mut skip = 0u32;
        let announced = loop {
            let resp = self
                .req(Method::GET, "api/v1/media")
                .query(&[
                    ("take", "100"),
                    ("skip", &skip.to_string()),
                    ("filter", "all"),
                ])
                .send_retry()
                .await?;
            let v = json(resp, "jellyseerr media").await?;
            let page = v
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let n = page.len() as u32;
            out.extend(page);
            let announced = v
                .pointer("/pageInfo/results")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            skip += n;
            if n == 0 || u64::from(skip) >= announced {
                break announced;
            }
        };
        Ok((out, announced))
    }

    /// Une demande par son id ; `None` si Jellyseerr répond 404 (demande retirée). Sert à confirmer qu'une demande
    /// absente de la liste paginée n'existe vraiment plus (une suppression pendant la lecture fait glisser les pages).
    pub async fn request_by_id(&self, id: i64) -> Result<Option<Value>> {
        let resp = self
            .req(Method::GET, &format!("api/v1/request/{id}"))
            .send_retry()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(json(resp, "jellyseerr request/{id}").await?))
    }

    /// Toutes les requêtes (paginé par 100), tous statuts confondus.
    pub async fn all_requests(&self) -> Result<Vec<Value>> {
        Ok(self.all_requests_counted().await?.0)
    }

    /// Comme [`Self::all_requests`], avec le total annoncé par Jellyseerr (`pageInfo.results` de la dernière page
    /// lue) : une liste qui en compte moins (demande ajoutée ou retirée pendant la lecture) est incomplète.
    pub async fn all_requests_counted(&self) -> Result<(Vec<Value>, u64)> {
        let mut out = Vec::new();
        let mut skip = 0u32;
        let announced = loop {
            let resp = self
                .req(Method::GET, "api/v1/request")
                .query(&[
                    ("take", "100"),
                    ("skip", &skip.to_string()),
                    ("filter", "all"),
                ])
                .send_retry()
                .await?;
            let v = json(resp, "jellyseerr request").await?;
            let page = v
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let n = page.len() as u32;
            out.extend(page);
            let announced = v
                .pointer("/pageInfo/results")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            skip += n;
            if n == 0 || u64::from(skip) >= announced {
                break announced;
            }
        };
        Ok((out, announced))
    }

    /// Types de notifications par mail choisis par un compte Jellyseerr (`notificationTypes.email`, masque de bits :
    /// 8 = « disponible ») ; `None` quand le compte n'a jamais rien choisi (Jellyseerr envoie alors tout). La réponse
    /// porte d'autres réglages du compte (identifiants Discord, Telegram…) : seul ce champ en est lu, rien n'est
    /// journalisé.
    pub async fn email_notification_types(&self, user_id: i64) -> Result<Option<i64>> {
        let resp = self
            .req(
                Method::GET,
                &format!("api/v1/user/{user_id}/settings/notifications"),
            )
            .send_retry()
            .await?;
        Ok(json(resp, "jellyseerr user/{id}/settings/notifications")
            .await?
            .pointer("/notificationTypes/email")
            .and_then(Value::as_i64))
    }
}

/// Champ `originalLanguage` d'une fiche TMDB vue par Jellyseerr (film ou série), vide s'il manque.
pub fn original_language_of(details: &Value) -> String {
    details
        .get("originalLanguage")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_language_is_read_from_tmdb_details() {
        assert_eq!(
            original_language_of(&json!({"originalLanguage": "ja"})),
            "ja"
        );
        assert_eq!(
            original_language_of(&json!({"originalLanguage": " en "})),
            "en"
        );
        assert_eq!(original_language_of(&json!({"title": "x"})), "");
        assert_eq!(original_language_of(&json!({"originalLanguage": null})), "");
    }

    /// Une demande retirée (404) n'est pas une erreur ; une panne, si.
    #[tokio::test]
    async fn request_by_id_tells_gone_from_failing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/request/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/request/8"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/request/9"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let c = JellyseerrClient::new(
            &format!("{}/", server.uri()),
            Secret::new("k"),
            reqwest::Client::new(),
        )
        .unwrap();
        assert_eq!(c.request_by_id(7).await.unwrap().unwrap()["id"], 7);
        assert!(c.request_by_id(8).await.unwrap().is_none());
        assert!(c.request_by_id(9).await.is_err());
    }
}
