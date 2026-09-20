//! Abonnés : webhook PayPal (`/paypal/webhook`), rattachement depuis la page `/premium`
//! (`/premium/lier`), espace membre « Mon compte » (`/compte/*`, servi sous `/gc-compte/` sur
//! l'adresse de Jellyfin comme le tchat : identité = jeton de session Jellyfin vérifié par
//! `/Users/Me`, cache mémoire 5 min, jamais journalisé) et actions admin de `/accounts/subs`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use homelab_core::chat;
use homelab_core::clients::paypal::{event_facts, EventFacts, WebhookHeaders};
use homelab_core::state::now;
use homelab_core::subscription_ops as ops;
use homelab_core::subscriptions::{self as subs, Status};
use homelab_core::welcome;
use homelab_core::TaskContext;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

const APP_JS: &str = include_str!("../assets/compte/app.js");
const AUTH_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct SubsState {
    pub ctx: Arc<TaskContext>,
    auth: Arc<Mutex<HashMap<String, (Me, Instant)>>>,
    last: Arc<Mutex<HashMap<String, Instant>>>,
}

#[derive(Clone, Debug)]
struct Me {
    id: String,
    name: String,
    disabled: bool,
}

type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "error": msg })))
}

pub fn router(ctx: Arc<TaskContext>) -> Router {
    let st = SubsState {
        ctx,
        auth: Arc::new(Mutex::new(HashMap::new())),
        last: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/paypal/webhook", post(webhook))
        .route("/premium/lier", post(link_from_page))
        .route("/compte/app.js", get(app_js))
        .route("/compte/api/me", get(me))
        .route("/compte/api/devices/{id}", delete(logout_device))
        .route("/compte/api/password", post(password_link))
        .with_state(st)
}

fn etag() -> &'static str {
    static ETAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ETAG.get_or_init(|| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in APP_JS.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("\"{h:016x}\"")
    })
}

async fn app_js(headers: HeaderMap) -> Response {
    let tag = etag();
    let cache = [
        (header::ETAG, HeaderValue::from_str(tag).expect("etag")),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
    ];
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|e| e.trim() == tag))
        .unwrap_or(false)
    {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/javascript; charset=utf-8"),
        )],
        cache,
        APP_JS,
    )
        .into_response()
}

async fn auth(st: &SubsState, headers: &HeaderMap) -> ApiResult<Me> {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    let token = chat::token_from_headers(h("x-emby-token"), h("authorization"))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "non connecté"))?;
    let cached = {
        let mut cache = st.auth.lock().await;
        if cache.len() > 500 {
            cache.retain(|_, (_, t)| t.elapsed() < AUTH_TTL);
        }
        cache
            .get(&token)
            .filter(|(_, t)| t.elapsed() < AUTH_TTL)
            .map(|(u, _)| u.clone())
    };
    if let Some(u) = cached {
        return Ok(u);
    }
    let me = st
        .ctx
        .jellyfin
        .user_from_token(&token)
        .await
        .map_err(|e| {
            warn!(task = "subs", error = %e, "jellyfin Users/Me failed");
            err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
        })?
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "session Jellyfin invalide"))?;
    let (Some(id), Some(name)) = (
        me.get("Id").and_then(Value::as_str),
        me.get("Name").and_then(Value::as_str),
    ) else {
        return Err(err(StatusCode::UNAUTHORIZED, "session Jellyfin invalide"));
    };
    let u = Me {
        id: chat::normalize_id(id),
        name: name.to_string(),
        disabled: me
            .pointer("/Policy/IsDisabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    st.auth
        .lock()
        .await
        .insert(token, (u.clone(), Instant::now()));
    Ok(u)
}

fn day_text(t: i64) -> String {
    ops::date_text(t)
}

/// GET /compte/api/me : tout ce que la page « Mon compte » affiche.
async fn me(State(st): State<SubsState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let cfg = &st.ctx.cfg.subscriptions;
    let t = now();
    let ctx = st.ctx.clone();
    let (uid, uname) = (u.id.clone(), u.name.clone());
    let fiche = tokio::task::spawn_blocking(move || -> anyhow::Result<subs::Subscriber> {
        ctx.subs
            .ensure(&uid, &uname, Status::Unknown, None, "import", now())
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne"))?
    .map_err(|e| {
        warn!(task = "subs", error = %e, "fiche illisible");
        err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne")
    })?;
    let exempt = st
        .ctx
        .cfg
        .accounts
        .protected
        .iter()
        .chain(cfg.exempt.iter())
        .any(|n| n.eq_ignore_ascii_case(&u.name));
    let status = if exempt { Status::Exempt } else { fiche.status };
    let days_left = fiche
        .expires_at
        .map(|e| ((e - t) as f64 / subs::DAY as f64).ceil() as i64);
    let history: Vec<Value> = st
        .ctx
        .subs
        .history(&u.id, 12)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.kind != "paypal_event")
        .map(|e| json!({ "at": e.at, "date": day_text(e.at), "kind": e.kind, "detail": e.detail }))
        .collect();
    let sessions = st.ctx.jellyfin.sessions_of(&u.id).await.unwrap_or_default();
    let devices: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "device_id": s.get("DeviceId").and_then(Value::as_str).unwrap_or(""),
                "name": s.get("DeviceName").and_then(Value::as_str).unwrap_or("?"),
                "client": s.get("Client").and_then(Value::as_str).unwrap_or(""),
                "last": s.get("LastActivityDate").and_then(Value::as_str).unwrap_or(""),
                "playing": s.get("NowPlayingItem").and_then(|i| i.get("Name")).and_then(Value::as_str),
            })
        })
        .collect();
    Ok(Json(json!({
        "name": u.name,
        "status": status.as_str(),
        "status_label": status.label(),
        "expires_at": fiche.expires_at,
        "expires_text": fiche.expires_at.map(day_text),
        "days_left": days_left,
        "source": fiche.source,
        "paypal": fiche.paypal_sub_id.is_some(),
        "price_text": cfg.price_text,
        "pay_url": ops::pay_url(&st.ctx, &u.name),
        "can_pay": st.ctx.paypal.is_some() || st.ctx.secrets.donation.is_some(),
        "referral_code": fiche.referral_code,
        "referral_days": cfg.referral_days,
        "trial_days": cfg.trial_days,
        "grace_days": cfg.grace_days,
        "disabled": u.disabled,
        "history": history,
        "devices": devices,
        "signup_url": st.ctx.secrets.premium_public_url.as_ref().map(|b| format!("{b}/inscription")),
    })))
}

/// DELETE /compte/api/devices/{id} : déconnecte un de SES appareils (propriétaire vérifié sur la
/// liste réelle : `GET /Devices?userId=` ignore son filtre).
async fn logout_device(
    State(st): State<SubsState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let devices = st.ctx.jellyfin.devices().await.map_err(|e| {
        warn!(task = "subs", error = %e, "devices unreadable");
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    let mine = devices.iter().any(|d| {
        d.get("Id").and_then(Value::as_str) == Some(id.as_str())
            && d.get("LastUserId")
                .and_then(Value::as_str)
                .map(chat::normalize_id)
                .as_deref()
                == Some(u.id.as_str())
    });
    if !mine {
        return Err(err(StatusCode::FORBIDDEN, "cet appareil n'est pas à toi"));
    }
    st.ctx.jellyfin.delete_device(&id).await.map_err(|e| {
        warn!(task = "subs", error = %e, "delete device failed");
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    info!(task = "subs", user = %u.name, "device logged out from Mon compte");
    Ok(Json(json!({ "ok": true })))
}

/// POST /compte/api/password : lien de changement de mot de passe (page /bienvenue/<jeton>, 1 h).
async fn password_link(State(st): State<SubsState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let email = homelab_core::accounts::email_of(&st.ctx, &u.id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let token = welcome::issue(&st.ctx, &u.id, &u.name, &email, welcome::KIND_ACTIVATED)
        .await
        .map_err(|e| {
            warn!(task = "subs", error = %e, "password link failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne")
        })?;
    let base = st
        .ctx
        .secrets
        .premium_public_url
        .clone()
        .or_else(|| st.ctx.secrets.onboard_public_url.clone())
        .unwrap_or_default();
    Ok(Json(
        json!({ "url": welcome::url(&base, &token), "ttl_mins": st.ctx.cfg.onboard.link_ttl_mins }),
    ))
}

fn webhook_headers(h: &HeaderMap) -> WebhookHeaders {
    let g = |n: &str| {
        h.get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    WebhookHeaders {
        auth_algo: g("paypal-auth-algo"),
        cert_url: g("paypal-cert-url"),
        transmission_id: g("paypal-transmission-id"),
        transmission_sig: g("paypal-transmission-sig"),
        transmission_time: g("paypal-transmission-time"),
    }
}

/// POST /paypal/webhook : signature vérifiée auprès de PayPal, événement traité une seule fois.
/// 2xx = pris en compte (PayPal ne réessaie pas) ; 5xx = réessai plus tard.
async fn webhook(State(st): State<SubsState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pp) = st.ctx.paypal.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if body.len() > 256 * 1024 {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let raw = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let event: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let wh = webhook_headers(&headers);
    if !wh.complete() {
        warn!(task = "subs", "webhook sans en-têtes de signature");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match pp.verify_webhook(&wh, raw).await {
        Ok(true) => {}
        Ok(false) => {
            warn!(task = "subs", "webhook PayPal : signature refusée");
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(e) => {
            warn!(task = "subs", error = %e, "webhook PayPal : vérification impossible");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }
    let facts = event_facts(&event);
    if facts.event_id.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let summary = format!(
        "{} sub={} custom={} statut={}",
        facts.event_type,
        facts.sub_id.as_deref().unwrap_or("-"),
        facts.custom_id.as_deref().unwrap_or("-"),
        facts.status.as_deref().unwrap_or("-")
    );
    match st.ctx.subs.record_paypal_event(
        &facts.event_id,
        &facts.event_type,
        facts.sub_id.as_deref(),
        &summary,
        now(),
    ) {
        Ok(true) => {}
        Ok(false) => {
            info!(task = "subs", event = %facts.event_id, "webhook déjà traité");
            return StatusCode::OK.into_response();
        }
        Err(e) => {
            warn!(task = "subs", error = %e, "webhook : base illisible");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    info!(task = "subs", %summary, "webhook PayPal");
    let r: anyhow::Result<()> = match facts.event_type.as_str() {
        "BILLING.SUBSCRIPTION.ACTIVATED"
        | "BILLING.SUBSCRIPTION.RE-ACTIVATED"
        | "PAYMENT.SALE.COMPLETED" => {
            if facts.sub_id.is_none() {
                Ok(()) // paiement ponctuel (don), pas un abonnement
            } else {
                complete_facts(&st, facts).await
            }
        }
        _ => ops::on_paypal_stop(&st.ctx, &facts).await,
    };
    match r {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            warn!(task = "subs", error = %e, "webhook : traitement en échec");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Un webhook de vente ne porte ni `custom_id` ni prochaine facturation : on complète en relisant
/// l'abonnement chez PayPal, puis on applique le paiement.
async fn complete_facts(st: &SubsState, mut facts: EventFacts) -> anyhow::Result<()> {
    if let (Some(pp), Some(sid)) = (st.ctx.paypal.as_ref(), facts.sub_id.clone()) {
        if facts.custom_id.is_none() || facts.next_billing.is_none() {
            if let Ok(v) = pp.subscription(&sid).await {
                let f2 = event_facts(
                    &json!({ "id": facts.event_id, "event_type": "BILLING.SUBSCRIPTION.ACTIVATED", "resource": v }),
                );
                if facts.custom_id.is_none() {
                    facts.custom_id = f2.custom_id;
                }
                if facts.next_billing.is_none() {
                    facts.next_billing = f2.next_billing;
                }
                if facts.email.is_none() {
                    facts.email = f2.email;
                }
            }
        }
    }
    ops::on_payment(&st.ctx, &facts, "webhook")
        .await
        .map(|_| ())
}

#[derive(Deserialize)]
pub struct LinkForm {
    subscription_id: String,
    #[serde(default)]
    compte: String,
}

/// POST /premium/lier : juste après l'approbation PayPal sur la page `/premium` (ou depuis
/// `/premium/activate` avec un identifiant d'abonnement). Vérifie l'abonnement chez PayPal, rattache,
/// active tout de suite sans attendre le webhook (qui sera alors ignoré comme doublon par date).
async fn link_from_page(
    State(st): State<SubsState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<LinkForm>,
) -> Response {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| addr.ip().to_string());
    {
        let mut m = st.last.lock().await;
        let t = Instant::now();
        m.retain(|_, at| t.duration_since(*at) < Duration::from_secs(10));
        if m.contains_key(&ip) {
            return Redirect::to("/premium/activate?err=rate").into_response();
        }
        m.insert(ip.clone(), t);
    }
    let sid = f.subscription_id.trim().to_string();
    let compte = f.compte.trim().to_string();
    if !subs::valid_paypal_sub_id(&sid) {
        return Redirect::to("/premium/activate?err=id").into_response();
    }
    let Some(pp) = st.ctx.paypal.as_ref() else {
        return Redirect::to("/premium/activate?err=nopaypal").into_response();
    };
    let v = match pp.subscription(&sid).await {
        Ok(v) => v,
        Err(e) => {
            warn!(task = "subs", %ip, error = %e, "abonnement PayPal illisible");
            return Redirect::to("/premium/activate?err=paypal").into_response();
        }
    };
    let mut facts = event_facts(
        &json!({ "id": format!("link:{sid}"), "event_type": "BILLING.SUBSCRIPTION.ACTIVATED", "resource": v }),
    );
    let status = facts.status.clone().unwrap_or_default();
    if status != "ACTIVE" {
        info!(task = "subs", %ip, %sid, %status, "abonnement pas encore actif");
        return Redirect::to(&format!(
            "/premium/activate?err=status&s={}",
            urlenc(&status)
        ))
        .into_response();
    }
    if !compte.is_empty() {
        match &facts.custom_id {
            Some(c) if !c.eq_ignore_ascii_case(&compte) => {
                warn!(task = "subs", %ip, %sid, "compte saisi différent du custom_id");
                return Redirect::to("/premium/activate?err=compte").into_response();
            }
            _ => facts.custom_id = Some(compte.clone()),
        }
    }
    if facts.custom_id.is_none() {
        return Redirect::to(&format!(
            "/premium/activate?err=nocompte&sub={}",
            urlenc(&sid)
        ))
        .into_response();
    }
    if !st
        .ctx
        .subs
        .record_paypal_event(&facts.event_id, "LINK", Some(&sid), "page /premium", now())
        .unwrap_or(false)
    {
        // déjà rattaché depuis la page : on renvoie simplement vers le merci
        if let Ok(Some(s)) = st.ctx.subs.by_paypal_sub(&sid) {
            return Redirect::to(&format!("/premium/merci?compte={}", urlenc(&s.username)))
                .into_response();
        }
    }
    match ops::on_payment(&st.ctx, &facts, "premium-page").await {
        Ok(Some(s)) => {
            info!(task = "subs", %ip, user = %s.username, %sid, "abonnement rattaché depuis /premium");
            Redirect::to(&format!(
                "/premium/merci?compte={}&jusqu={}",
                urlenc(&s.username),
                s.expires_at.map(day_text).unwrap_or_default()
            ))
            .into_response()
        }
        Ok(None) => Redirect::to(&format!(
            "/premium/activate?err=inconnu&sub={}",
            urlenc(&sid)
        ))
        .into_response(),
        Err(e) => {
            warn!(task = "subs", %ip, error = %e, "rattachement en échec");
            Redirect::to("/premium/activate?err=interne").into_response()
        }
    }
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
