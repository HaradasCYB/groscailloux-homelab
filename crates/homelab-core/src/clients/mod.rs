//! Clients HTTP minimalistes, un par service. Les payloads sont des
//! `serde_json::Value` là où l'ancien bash utilisait `jq`, et des structs
//! typées là où l'on filtre/décide.

mod arr;
pub mod bazarr;
pub mod jellyfin;
mod jellyseerr;
pub mod paypal;
mod prowlarr;
mod qbit;

pub use arr::{ArrClient, QueueItem};
pub use bazarr::{BazarrClient, HistoryRow};
pub use jellyfin::JellyfinClient;
pub use jellyseerr::JellyseerrClient;
pub use paypal::PayPalClient;
pub use prowlarr::ProwlarrClient;
pub use qbit::{QbitClient, Torrent, TorrentFile, ETA_UNKNOWN};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::{Method, RequestBuilder, Response};

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

/// Une coupure de transport vaut-elle une nouvelle tentative ? Oui pour une connexion fermée ou refusée
/// (`is_request`, `is_connect`), **jamais** pour un délai dépassé : la requête a pu être traitée, et un client
/// qui attend déjà 30 s ne doit pas attendre deux fois.
fn worth_retrying(request: bool, connect: bool, timeout: bool) -> bool {
    (request || connect) && !timeout
}

/// Chaîne de causes d'une erreur reqwest (`reqwest::Error` n'a pas de `{:#}`), sans l'URL : elle peut porter
/// des paramètres (clé, jeton) et le chemin est journalisé à part.
fn cause_chain(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut out = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

/// Envoi d'une requête avec **une** nouvelle tentative pour les `GET` (idempotents).
///
/// Le pool de connexions de reqwest garde une connexion inactive 90 s ; un serveur qui ferme la sienne plus tôt
/// (Node : 5 s, nginx : 65–75 s) laisse partir la requête suivante sur une connexion morte :
/// « connection closed before message completed » (Jellyseerr, 07/10, une erreur de tâche sans cause réelle).
/// Une seconde tentative repart sur une connexion neuve. Seuls les `GET` sont rejoués (un `POST` a pu agir) et
/// jamais sur un délai dépassé. À ne pas employer pour un `GET` qui a un effet : recherche qui consomme un quota
/// (Prowlarr, Arr `release`), lecture d'une playlist qui démarre une conversion.
#[async_trait]
pub(crate) trait SendRetry {
    async fn send_retry(self) -> reqwest::Result<Response>;
}

#[async_trait]
impl SendRetry for RequestBuilder {
    async fn send_retry(self) -> reqwest::Result<Response> {
        let (client, req) = self.build_split();
        let req = req?;
        let again = (req.method() == Method::GET)
            .then(|| req.try_clone())
            .flatten();
        let path = req.url().path().to_string();
        match client.execute(req).await {
            Err(e) if worth_retrying(e.is_request(), e.is_connect(), e.is_timeout()) => {
                let Some(again) = again else {
                    return Err(e);
                };
                tracing::info!(path = %path, cause = %cause_chain(e), "GET coupé : nouvelle tentative (une seule)");
                client.execute(again).await
            }
            r => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn retries_a_cut_connection_but_never_a_timeout() {
        assert!(worth_retrying(true, false, false), "connexion fermée");
        assert!(worth_retrying(false, true, false), "connexion refusée");
        assert!(!worth_retrying(true, false, true), "délai dépassé");
        assert!(!worth_retrying(true, true, true), "connexion trop lente");
        assert!(
            !worth_retrying(false, false, false),
            "ni requête ni connexion"
        );
    }

    /// Serveur de test : les `cut` premières connexions sont fermées sans réponse, les suivantes répondent « ok ».
    /// Si `hang`, la connexion reste ouverte sans répondre (pour provoquer un délai dépassé).
    async fn server(cut: usize, hang: bool) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/ping", listener.local_addr().unwrap());
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    if hang {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    } else if n >= cut {
                        let _ = sock
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                            )
                            .await;
                    }
                    // sinon : la connexion est simplement fermée (drop), sans réponse
                });
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn a_get_cut_once_is_replayed_once() {
        let (url, seen) = server(1, false).await;
        let http = reqwest::Client::new();
        let resp = http.get(&url).send_retry().await.expect("2e tentative");
        assert_eq!(resp.text().await.unwrap(), "ok");
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_get_cut_twice_fails_after_a_single_replay() {
        let (url, seen) = server(5, false).await;
        let http = reqwest::Client::new();
        assert!(http.get(&url).send_retry().await.is_err());
        assert_eq!(
            seen.load(Ordering::SeqCst),
            2,
            "une seule nouvelle tentative"
        );
    }

    #[tokio::test]
    async fn a_post_is_never_replayed() {
        let (url, seen) = server(1, false).await;
        let http = reqwest::Client::new();
        assert!(http.post(&url).body("x").send_retry().await.is_err());
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_timeout_is_not_replayed() {
        let (url, seen) = server(0, true).await;
        let http = reqwest::Client::new();
        let err = http
            .get(&url)
            .timeout(Duration::from_millis(300))
            .send_retry()
            .await
            .unwrap_err();
        assert!(err.is_timeout());
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }
}
