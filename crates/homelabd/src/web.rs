//! UI d'onboarding (remplace le conteneur Flask `onboarder`) + endpoints de santé.
//! Si `HOMELABD_ONBOARD_TOKEN` est défini, `POST /onboard` exige l'en-tête
//! `X-Onboard-Token` ; la page le lit depuis `?token=` et le renvoie.
//! `/accounts` (page « Comptes ») exige ce même jeton, et refuse tout s'il n'est pas défini.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use homelab_core::accounts::{self, Outcome};
use homelab_core::config::Donation;
use homelab_core::tasks::onboard::{self, OnboardRequest};
use homelab_core::{Secret, TaskContext};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::{accounts_page, status_page};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const DON_HTML: &str = include_str!("../assets/don.html");

#[derive(Clone)]
struct AppState {
    ctx: Arc<TaskContext>,
    last_request: Arc<Mutex<HashMap<String, Instant>>>,
}

#[derive(Deserialize)]
struct OnboardBody {
    username: String,
    email: String,
    #[serde(default)]
    password: Option<String>,
}

pub async fn serve(ctx: Arc<TaskContext>) -> Result<()> {
    let listen = ctx.cfg.web.listen.clone();
    let state = AppState {
        ctx,
        last_request: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route("/status", get(status))
        .route("/status.html", get(status_html))
        .route("/onboard", post(onboard_handler))
        .route("/don", get(don))
        .route("/accounts", get(accounts_html))
        .route("/accounts/premium", post(accounts_toggle))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    info!(%listen, "web server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Page de don (publique, sans lien avec le reste) : 404 tant que PayPal n'est pas configuré.
fn don_page(d: &Donation) -> String {
    DON_HTML
        .replace("{{CLIENT_ID}}", &d.paypal_client_id)
        .replace("{{PLAN_ID}}", &d.paypal_plan_id)
}

async fn don(State(st): State<AppState>) -> Response {
    match &st.ctx.secrets.donation {
        Some(d) => (
            [(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=3600"),
            )],
            Html(don_page(d)),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

/// `/status` et `/status.html` exposent l'activité interne : si `HOMELABD_STATUS_TOKEN` est
/// défini, ils exigent `?token=` (le sous-domaine d'onboarding est public).
fn status_allowed(st: &AppState, q: &HashMap<String, String>) -> bool {
    match &st.ctx.secrets.status_token {
        Some(t) => q.get("token").map(|g| g == t.expose()).unwrap_or(false),
        None => true,
    }
}

async fn status(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !status_allowed(&st, &q) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "token manquant ou invalide" })),
        );
    }
    let runs = st
        .ctx
        .state
        .read(|s| serde_json::to_value(&s.task_runs).unwrap_or(Value::Null))
        .await;
    (
        StatusCode::OK,
        Json(json!({ "dry_run": st.ctx.dry_run, "tasks": runs })),
    )
}

async fn status_html(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !status_allowed(&st, &q) {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<p>token manquant ou invalide</p>".to_string()),
        );
    }
    let runs = st.ctx.state.read(|s| s.task_runs.clone()).await;
    let cfg = st.ctx.cfg.clone();
    if cfg.seedbox.enabled {
        // le cache de répertoires rclone (1 h) masquerait le quota réécrit toutes les 15 min
        let rc = format!(
            "{}/vfs/refresh",
            cfg.seedbox.rclone_rc.trim_end_matches('/')
        );
        let _ = st
            .ctx
            .http
            .post(&rc)
            .json(&json!({ "dir": ".homelab" }))
            .timeout(Duration::from_secs(2))
            .send()
            .await;
    }
    // lecture via le montage rclone : bornée, la seedbox peut être injoignable
    let probe = tokio::task::spawn_blocking(move || {
        let disk = homelab_core::disk::usage_percent(&cfg.paths.base).ok();
        if !cfg.seedbox.enabled {
            return (disk, None, None);
        }
        let mp = &cfg.seedbox.mount_point;
        let mounted = std::fs::read_dir(mp)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        let quota = std::fs::read_to_string(mp.join(".homelab/quota.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<status_page::SeedboxQuota>(&s).ok());
        (disk, quota, Some(mounted))
    });
    let (disk, quota, mounted) = tokio::time::timeout(Duration::from_secs(3), probe)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or((None, None, Some(false)));
    let tasks = homelab_core::tasks::names();
    let html = status_page::render(&status_page::PageData {
        now: homelab_core::state::now(),
        runs: &runs,
        tasks: &tasks,
        vps_disk_pct: disk,
        seedbox: quota,
        mount_ok: mounted,
    });
    (StatusCode::OK, Html(html))
}

#[derive(Deserialize)]
struct ToggleForm {
    token: String,
    user_id: String,
    on: String,
}

/// Jeton d'onboarding obligatoire : sans `HOMELABD_ONBOARD_TOKEN`, la page reste fermée.
fn accounts_allowed(st: &AppState, given: Option<&str>) -> bool {
    match (&st.ctx.secrets.onboard_token, given) {
        (Some(t), Some(g)) => !g.is_empty() && g == t.expose(),
        _ => false,
    }
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html("<p>Jeton manquant ou invalide : ouvrir la page avec ?token=…</p>".to_string()),
    )
        .into_response()
}

async fn accounts_html(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let token = q.get("token").map(String::as_str);
    if !accounts_allowed(&st, token) {
        return denied();
    }
    let list = match accounts::list(&st.ctx).await {
        Ok(l) => l,
        Err(e) => {
            warn!(task = "accounts", error = %e, "accounts page: jellyfin unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                Html("<p>Jellyfin injoignable, réessayer dans un instant.</p>".to_string()),
            )
                .into_response();
        }
    };
    let msg = q
        .get("msg")
        .map(|m| (m.as_str(), q.get("who").map(String::as_str).unwrap_or("")));
    let html = accounts_page::render(&accounts_page::PageData {
        now: homelab_core::state::now(),
        accounts: &list,
        max_premium: st.ctx.cfg.accounts.max_premium,
        max_streams: st.ctx.cfg.accounts.max_streams_per_user,
        token: token.unwrap_or(""),
        msg,
    });
    let mut resp = Html(html).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// Bascule premium puis redirection (POST → GET) avec le résultat en paramètre.
async fn accounts_toggle(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<ToggleForm>,
) -> Response {
    if !accounts_allowed(&st, Some(&f.token)) {
        return denied();
    }
    let on = f.on == "1";
    let ip = client_ip(&headers, addr);
    let who = accounts::list(&st.ctx)
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|a| a.id == f.user_id).map(|a| a.name))
        .unwrap_or_default();
    let code = match accounts::set_premium(&st.ctx, &f.user_id, on).await {
        Ok(Outcome::Activated) => "activated",
        Ok(Outcome::Suspended) => "suspended",
        Ok(Outcome::Unchanged) => "unchanged",
        Ok(Outcome::CapReached { .. }) => "cap",
        Err(e) => {
            warn!(task = "accounts", %ip, user = %who, error = %e, "premium toggle failed");
            "error"
        }
    };
    info!(task = "accounts", %ip, user = %who, on, result = code, "premium toggle via web");
    let url = format!(
        "/accounts?token={}&msg={code}&who={}",
        urlencode(&f.token),
        urlencode(&who)
    );
    Redirect::to(&url).into_response()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn client_ip(headers: &HeaderMap, addr: SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| addr.ip().to_string())
}

fn fail(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "success": false, "error": msg.into() })))
}

async fn onboard_handler(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<OnboardBody>,
) -> impl IntoResponse {
    if let Some(expected) = &st.ctx.secrets.onboard_token {
        let given = headers
            .get("x-onboard-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if given != expected.expose() {
            return fail(
                StatusCode::UNAUTHORIZED,
                "token manquant ou invalide (ouvrir la page avec ?token=…)",
            );
        }
    }
    let username = body.username.trim().to_string();
    let email = body.email.trim().to_lowercase();
    if !onboard::valid_username(&username) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Username invalide (2-32 chars, lettres/chiffres/_-)",
        );
    }
    if !onboard::valid_email(&email) {
        return fail(StatusCode::BAD_REQUEST, "Email invalide");
    }
    let ip = client_ip(&headers, addr);
    let limit = Duration::from_secs(st.ctx.cfg.web.rate_limit_secs);
    {
        let mut map = st.last_request.lock().await;
        let now = Instant::now();
        map.retain(|_, t| now.duration_since(*t) < limit);
        if map.contains_key(&ip) {
            return fail(
                StatusCode::TOO_MANY_REQUESTS,
                format!("Rate limit ({}s entre requêtes)", limit.as_secs()),
            );
        }
        map.insert(ip.clone(), now);
    }
    info!(task = "onboard", %ip, %username, %email, "web onboarding requested");
    let req = OnboardRequest {
        username,
        email,
        password: body.password.filter(|p| !p.is_empty()).map(Secret::new),
    };
    match onboard::run(&st.ctx, req).await {
        Ok(r) => {
            let log = if r.dry_run {
                "DRY-RUN : pré-contrôles OK, aucun compte créé".to_string()
            } else {
                format!(
                    "Jellyfin Id {} · Jellyseerr id {} · mail {} · {}",
                    r.jellyfin_id,
                    r.jellyseerr_id,
                    if r.mail_sent {
                        "envoyé"
                    } else {
                        "NON envoyé (transmettre les identifiants à la main)"
                    },
                    if r.premium {
                        "compte premium"
                    } else {
                        "compte suspendu : l'activer depuis /accounts"
                    }
                )
            };
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "username": r.username,
                    "email": r.email,
                    "password": r.password.expose(),
                    "jellyfin_id": r.jellyfin_id,
                    "jellyseerr_id": r.jellyseerr_id,
                    "jellyfin_url": r.jellyfin_url,
                    "jellyseerr_url": r.jellyseerr_url,
                    "mail_sent": r.mail_sent,
                    "premium": r.premium,
                    "dry_run": r.dry_run,
                    "log": log,
                })),
            )
        }
        Err(e) => {
            warn!(task = "onboard", %ip, error = %e, "web onboarding failed");
            fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn don_page_fills_every_placeholder() {
        let html = don_page(&Donation {
            paypal_client_id: "CID-1".into(),
            paypal_plan_id: "P-9".into(),
        });
        assert!(!html.contains("{{"));
        assert!(html.contains("client-id=CID-1&"));
        assert!(html.contains("plan_id: 'P-9'"));
        assert!(html.contains("subscribe?plan_id=P-9"));
        assert!(html.contains("aucun service"));
        // un élément id="paypal" masquerait window.paypal et ferait planter le SDK
        assert!(!html.contains(r#"id="paypal""#));
    }
}
