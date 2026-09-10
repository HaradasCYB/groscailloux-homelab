//! UI d'onboarding (remplace le conteneur Flask `onboarder`) + endpoints de santé.
//! Si `HOMELABD_ONBOARD_TOKEN` est défini, `POST /onboard` exige l'en-tête
//! `X-Onboard-Token` ; la page le lit depuis `?token=` et le renvoie.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use homelab_core::tasks::onboard::{self, OnboardRequest};
use homelab_core::{Secret, TaskContext};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

const INDEX_HTML: &str = include_str!("../assets/index.html");

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
        .route("/onboard", post(onboard_handler))
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

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn status(State(st): State<AppState>) -> Json<Value> {
    let runs = st
        .ctx
        .state
        .read(|s| serde_json::to_value(&s.task_runs).unwrap_or(Value::Null))
        .await;
    Json(json!({ "dry_run": st.ctx.dry_run, "tasks": runs }))
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
                    "Jellyfin Id {} · Jellyseerr id {} · mail {}",
                    r.jellyfin_id,
                    r.jellyseerr_id,
                    if r.mail_sent {
                        "envoyé"
                    } else {
                        "NON envoyé (transmettre les identifiants à la main)"
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
