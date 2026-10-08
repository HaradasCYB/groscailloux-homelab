//! UI d'onboarding (remplace le conteneur Flask `onboarder`) + endpoints de santé.
//! `POST /onboard` exige l'en-tête `X-Onboard-Token` (`HOMELABD_ONBOARD_TOKEN`), posé par la session
//! d'administration (`admin_auth`) ou par la CLI. `/accounts` (page « Comptes ») exige ce même jeton : sans
//! `HOMELABD_ONBOARD_TOKEN`, tout reste fermé. Adresse du client : `client_addr::Client`, posé par la couche.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Json, Router};
use homelab_core::accounts::{self, Outcome};
use homelab_core::config::Donation;
use homelab_core::mail;
use homelab_core::subscription_ops;
use homelab_core::subscriptions::{self, Status as SubStatus};
use homelab_core::tasks::onboard::{self, OnboardRequest};
use homelab_core::welcome;
use homelab_core::{Secret, TaskContext};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::admin_auth::token_matches;
use crate::client_addr::Client;
use crate::{accounts_page, guide, status_page};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const BIENVENUE_HTML: &str = include_str!("../assets/bienvenue.html");
const INSCRIPTION_HTML: &str = include_str!("../assets/inscription.html");
const PREMIUM_HTML: &str = include_str!("../assets/premium.html");
const PREMIUM_ACTIVATE_HTML: &str = include_str!("../assets/premium-activate.html");
const PREMIUM_MERCI_HTML: &str = include_str!("../assets/premium-merci.html");

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
    let chat = crate::chat_api::router(ctx.clone())?;
    let search = crate::search_page::router(ctx.clone());
    let subs = crate::subs_api::router(ctx.clone());
    let auth = crate::admin_auth::AdminAuth::new(ctx.clone());
    let login = Router::new()
        .route(
            "/connexion",
            get(crate::admin_auth::login_get).post(crate::admin_auth::login_post),
        )
        .with_state(auth.clone());
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
        .route("/guide", get(guide_html))
        .route("/guide/icon.png", get(guide_icon))
        .route("/bienvenue/renouveler", post(welcome_renew))
        .route("/premiers-pas", get(first_steps_page))
        .route("/bienvenue/{token}", get(welcome_get).post(welcome_post))
        .route("/inscription", get(signup_get).post(signup_post))
        .route("/accounts/link", post(accounts_link))
        .route("/admin/link", post(admin_link))
        .route("/admin/mail-test", post(admin_mail_test))
        .route("/admin/run", post(admin_run))
        .route("/admin/accounts", post(admin_accounts))
        .route("/admin/vo-native", post(admin_vo_native))
        .route("/don", get(don_redirect))
        .route("/premium", get(premium))
        .route("/premium/merci", get(premium_merci))
        .route("/accounts/subs", post(accounts_subs))
        .route(
            "/premium/activate",
            get(premium_activate).post(premium_activate_post),
        )
        .route("/accounts", get(accounts_html))
        .route("/accounts/premium", post(accounts_toggle))
        .route(
            "/accounts/delete",
            get(accounts_delete_confirm).post(accounts_delete),
        )
        .with_state(state)
        .merge(chat)
        .merge(search)
        .merge(subs)
        .merge(login)
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::admin_auth::layer,
        ));
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

/// Guide des nouveaux membres (public, lien du mail de bienvenue).
async fn guide_html(State(st): State<AppState>) -> Response {
    (
        [(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=3600"),
        )],
        Html(guide::page(&st.ctx.secrets)),
    )
        .into_response()
}

async fn guide_icon() -> Response {
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("image/png"),
            ),
            (
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=86400"),
            ),
        ],
        guide::ICON_PNG,
    )
        .into_response()
}

/// Garde le lien court historique `/don` : redirection 301 (permanente) vers `/premium`.
async fn don_redirect() -> Response {
    (
        StatusCode::MOVED_PERMANENTLY,
        [(
            axum::http::header::LOCATION,
            axum::http::HeaderValue::from_static("/premium"),
        )],
        Html("<a href=\"/premium\">Premium</a>".to_string()),
    )
        .into_response()
}

/// Page d'abonnement Premium (publique) : 404 tant que PayPal n'est pas configuré.
/// Identifiants du bouton PayPal : l'application REST (`PAYPAL_*`, sandbox ou live) quand elle est
/// configurée, sinon les identifiants historiques de la page de don.
fn premium_ids(st: &AppState, test: bool) -> Option<(String, String, bool)> {
    // Tant que l'application REST est en sandbox, la page publique garde le bouton Live historique
    // (`DONATION_*`) : seul `?test=1` montre le bouton sandbox.
    if let Some(p) = &st.ctx.secrets.paypal {
        if !p.sandbox || test || st.ctx.secrets.donation.is_none() {
            return Some((p.client_id.clone(), p.plan_id.clone(), p.sandbox));
        }
    }
    st.ctx
        .secrets
        .donation
        .as_ref()
        .map(|d| (d.paypal_client_id.clone(), d.paypal_plan_id.clone(), false))
}

/// `deja` : le compte prérempli a déjà un abonnement PayPal qui se renouvelle tout seul — message à la
/// place du bouton (un second abonnement = un second prélèvement chaque mois).
fn premium_page(
    client_id: &str,
    plan_id: &str,
    sandbox: bool,
    compte: &str,
    price: &str,
    deja: bool,
) -> String {
    let host = if sandbox {
        "https://www.sandbox.paypal.com"
    } else {
        "https://www.paypal.com"
    };
    PREMIUM_HTML
        .replace("{{CLIENT_ID}}", client_id)
        .replace("{{PLAN_ID}}", plan_id)
        .replace("{{SDK_HOST}}", host)
        .replace(
            "{{SUBSCRIBE_URL}}",
            &format!("{host}/webapps/billing/plans/subscribe?plan_id={plan_id}"),
        )
        .replace("{{COMPTE}}", &page_esc(compte))
        .replace("{{PRICE}}", &page_esc(price))
        .replace("{{DEJA}}", if deja { "1" } else { "0" })
}

/// « Déjà abonné » sur `/premium` ? Seulement pour un lien émis par homelabd (`k` =
/// [`subscriptions::premium_link_key`] du compte) : la page est publique, et sans cette clé n'importe
/// qui saurait si un pseudo existe et paie par PayPal. La fiche n'est lue (`fiche`) qu'avec une clé
/// valide.
fn premium_deja(
    secret: Option<&str>,
    compte: &str,
    k: Option<&str>,
    fiche: impl FnOnce(&str) -> Option<subscriptions::Subscriber>,
) -> bool {
    let (Some(secret), Some(k)) = (secret, k) else {
        return false;
    };
    !compte.is_empty()
        && subscriptions::premium_link_ok(secret, compte, k)
        && fiche(compte).is_some_and(|s| subscriptions::auto_renews(&s))
}

async fn premium(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some((cid, plan, sandbox)) = premium_ids(&st, q.contains_key("test")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let compte = q
        .get("compte")
        .map(|c| c.trim())
        .filter(|c| onboard::valid_username(c))
        .unwrap_or("");
    // fiche locale seulement (aucun appel à PayPal depuis une page publique), et seulement pour un lien
    // émis par homelabd (clé `k`)
    let deja = st.ctx.cfg.subscriptions.enabled
        && premium_deja(
            st.ctx.secrets.onboard_token.as_ref().map(|t| t.expose()),
            compte,
            q.get("k").map(String::as_str),
            |c| st.ctx.subs.by_username(c).ok().flatten(),
        );
    let mut resp = Html(premium_page(
        &cid,
        &plan,
        sandbox,
        compte,
        &st.ctx.cfg.subscriptions.price_text,
        deja,
    ))
    .into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// GET /premium/merci : après un rattachement réussi.
async fn premium_merci(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let compte = q.get("compte").map(String::as_str).unwrap_or("");
    let jusqu = q
        .get("jusqu")
        .filter(|j| !j.is_empty())
        .map(|j| format!(" jusqu'au {}", page_esc(j)))
        .unwrap_or_default();
    let html = PREMIUM_MERCI_HTML
        .replace("{{COMPTE}}", &page_esc(compte))
        .replace("{{JUSQU}}", &jusqu)
        .replace(
            "{{JELLYFIN_URL}}",
            &page_esc(&st.ctx.secrets.jellyfin_public_url),
        );
    let mut resp = Html(html).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

#[derive(Deserialize)]
struct SubsForm {
    token: String,
    user_id: String,
    /// `active`, `offered`, `exempt`, `suspended`, `unknown`, `trial` ou `extend`.
    action: String,
    #[serde(default)]
    days: Option<i64>,
}

/// POST /accounts/subs : décision admin sur une fiche (statut, prolongation), journalisée.
async fn accounts_subs(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<SubsForm>,
) -> Response {
    if !accounts_allowed(&st, Some(&f.token)) {
        return denied();
    }
    let ip = client.ip;
    let who = st
        .ctx
        .subs
        .get(&f.user_id)
        .ok()
        .flatten()
        .map(|s| s.username)
        .unwrap_or_default();
    let days = f.days.filter(|d| (1..=730).contains(d));
    let mut until = String::new();
    let code = if f.action == "extend" {
        match subscription_ops::admin_extend(&st.ctx, &f.user_id, days.unwrap_or(30), "admin").await
        {
            Ok(t) => {
                until = subscription_ops::date_text(t);
                "sub_extended"
            }
            Err(e) => {
                warn!(task = "subs", %ip, user = %who, error = format!("{e:#}"), "extend failed");
                "error"
            }
        }
    } else if let Some(status) = SubStatus::parse(f.action.trim_end_matches("_days")) {
        // « offered » = sans limite (N ignoré) ; « offered_days » = offert pour N jours
        let days = if f.action == "offered" { None } else { days };
        match subscription_ops::admin_set(&st.ctx, &f.user_id, status, days, "admin").await {
            Ok(s) => {
                until = s
                    .expires_at
                    .map(subscription_ops::date_text)
                    .unwrap_or_default();
                "sub_set"
            }
            Err(e) => {
                warn!(task = "subs", %ip, user = %who, error = format!("{e:#}"), "status change failed");
                "error"
            }
        }
    } else {
        "error"
    };
    info!(task = "subs", %ip, user = %who, action = %f.action, days = ?days, %until, result = code, "subscription action via web");
    Redirect::to(&format!(
        "/accounts?msg={code}&who={}&until={}",
        urlencode(&who),
        urlencode(&until)
    ))
    .into_response()
}

/// Page « activation Premium » : le visiteur y indique le nom exact de son compte après paiement.
async fn premium_activate() -> Response {
    let mut resp = Html(PREMIUM_ACTIVATE_HTML).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

#[derive(Deserialize)]
struct ActivateForm {
    name: String,
}

/// Demande d'activation Premium : nom exact + IP → mail admin (rate-limit 30 s/IP partagé avec
/// l'onboarding). POST → GET avec `?ok=1` / `?err=1` pour tenir hors du panneau de succès.
async fn premium_activate_post(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<ActivateForm>,
) -> Response {
    let ip = client.ip;
    let limit = Duration::from_secs(st.ctx.cfg.web.rate_limit_secs);
    {
        let mut map = st.last_request.lock().await;
        let now = Instant::now();
        map.retain(|_, t| now.duration_since(*t) < limit);
        if map.contains_key(&ip) {
            return Redirect::to("/premium/activate?err=1").into_response();
        }
        map.insert(ip.clone(), now);
    }
    let name = f.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Redirect::to("/premium/activate?err=1").into_response();
    }
    let to = st.ctx.secrets.donation_notify_email.clone();
    if !to.is_empty() {
        if let Some(smtp) = &st.ctx.secrets.smtp {
            let body = activation_mail_body(
                st.ctx.secrets.donation.as_ref(),
                st.ctx.secrets.onboard_token.as_ref(),
                st.ctx.secrets.onboard_public_url.as_deref(),
                &name,
                &ip,
            );
            match mail::send_plain(
                smtp,
                "Admin Premium",
                &to,
                &format!("Demande d'activation Premium — {name}"),
                &body,
            )
            .await
            {
                Ok(()) => {
                    info!(task = "premium", %ip, name = %name, %to, "activation demand emailed")
                }
                Err(e) => {
                    warn!(task = "premium", %ip, name = %name, error = format!("{e:#}"), "activation demand mail failed")
                }
            }
        } else {
            warn!(
                task = "premium",
                "SMTP non configuré : demande d'activation non envoyée"
            );
        }
    }
    info!(task = "premium", %ip, name = %name, "premium activation requested via web");
    Redirect::to("/premium/activate?ok=1").into_response()
}

/// Corps du mail d'activation : horodatage lisible + epoch, IP, plan PayPal et lien admin vers la page Comptes, sur
/// l'hôte d'onboarding (`ONBOARD_PUBLIC_URL`, seul hôte qui sert les pages d'administration ; aucun domaine dans le
/// dépôt public, aucun jeton dans le lien : la page passe par la session admin). Sans jeton d'onboarding, la page
/// Comptes est fermée : pas de lien.
fn activation_mail_body(
    donation: Option<&Donation>,
    onboard_token: Option<&Secret>,
    onboard_public_url: Option<&str>,
    name: &str,
    ip: &str,
) -> String {
    let now = chrono::Local::now();
    let plan = donation
        .map(|d| format!("Plan PayPal {} · 3,50 €/mois", d.paypal_plan_id))
        .unwrap_or_else(|| "Plan PayPal non configuré côté serveur".to_string());
    let activation = match (onboard_token, onboard_public_url) {
        (Some(_), Some(base)) => format!(
            "\n\nActiver le compte depuis la page Comptes :\n{}/accounts\n",
            base.trim_end_matches('/')
        ),
        (Some(_), None) => {
            "\n\nActiver le compte depuis la page Comptes (/accounts de l'adresse d'onboarding).\n"
                .to_string()
        }
        (None, _) => String::new(),
    };
    format!(
        "Demande d'activation du compte Premium Homeflix GrosCailloux.\n\n\
Nom saisi : {name}\n\
IP : {ip}\n\
Date : {date}\n\
Epoch : {epoch}\n\
{plan}\n\
\nVérifier le paiement PayPal puis activer le compte si le nom correspond.{activation}",
        date = now.format("%Y-%m-%d %H:%M:%S %z"),
        epoch = now.timestamp(),
    )
}

/// Santé du daemon, lue par le chien de garde (`scripts/homelabd-watchdog.sh`) : 200 tant que l'ordonnanceur tourne,
/// 503 quand plus aucune boucle de tâche n'a fait de tour depuis `scheduler::stale_after` (20 min au moins). Avant,
/// la réponse était toujours « ok » : un ordonnanceur figé ne déclenchait aucune relance.
async fn health() -> (StatusCode, Json<Value>) {
    let h = crate::scheduler::health();
    let code = if h.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": if h.ok { "ok" } else { "scheduler_stale" },
            "version": crate::VERSION,
            "scheduler_idle_secs": h.age_secs,
        })),
    )
}

/// `/status` et `/status.html` exposent l'activité interne : si `HOMELABD_STATUS_TOKEN` est
/// défini, ils exigent le jeton, réinjecté en interne par la session (`admin_auth`).
fn status_allowed(st: &AppState, q: &HashMap<String, String>) -> bool {
    match &st.ctx.secrets.status_token {
        Some(t) => token_matches(Some(t), q.get("token").map(String::as_str)),
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
    let imports = st.ctx.state.read(|s| s.torrent_import.clone()).await;
    let seasons = st.ctx.state.read(|s| s.unknown_series.clone()).await;
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
    let alerts = st.ctx.state.read(|s| s.alerts.clone()).await;
    let catalogue = st.ctx.state.read(|s| s.catalogue.clone()).await;
    let html = status_page::render(&status_page::PageData {
        now: homelab_core::state::now(),
        runs: &runs,
        tasks: &tasks,
        vps_disk_pct: disk,
        seedbox: quota,
        mount_ok: mounted,
        stuck_torrents: &status_page::unmatched(&imports, homelab_core::state::now(), 15),
        blocked_seasons: &status_page::blocked_seasons(&seasons, homelab_core::state::now(), 15),
        canary: canary_text(&st.ctx).await,
        catalogue: catalogue.as_ref(),
        alerts: &alerts,
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
    token_matches(st.ctx.secrets.onboard_token.as_ref(), given)
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html("<p>Session expirée : <a href=\"/connexion\">se reconnecter</a>.</p>".to_string()),
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
            warn!(
                task = "accounts",
                error = format!("{e:#}"),
                "accounts page: jellyfin unreachable"
            );
            return (
                StatusCode::BAD_GATEWAY,
                Html("<p>Jellyfin injoignable, réessayer dans un instant.</p>".to_string()),
            )
                .into_response();
        }
    };
    let who_until = match (q.get("who"), q.get("until")) {
        (Some(w), Some(u)) if !u.is_empty() => format!("{w}|{u}"),
        (Some(w), _) => w.clone(),
        _ => String::new(),
    };
    let msg = q.get("msg").map(|m| (m.as_str(), who_until.as_str()));
    let now = homelab_core::state::now();
    let all_links: Vec<homelab_core::state::WelcomeLink> = st
        .ctx
        .state
        .read(|s| s.welcome_links.values().cloned().collect())
        .await;
    let links: HashMap<String, String> = list
        .iter()
        .map(|a| (a.id.clone(), link_status_text(&all_links, &a.id, now)))
        .collect();
    let mut subs: HashMap<String, accounts_page::SubInfo> = HashMap::new();
    if st.ctx.cfg.subscriptions.enabled {
        if let Err(e) = subscription_ops::ensure_fiches(&st.ctx).await {
            warn!(
                task = "subs",
                error = format!("{e:#}"),
                "fiches non synchronisées"
            );
        }
        for s in st.ctx.subs.list().unwrap_or_default() {
            subs.insert(
                s.user_id.clone(),
                accounts_page::SubInfo {
                    status: s.status.as_str().to_string(),
                    label: s.status.label().to_string(),
                    // hors essai et hors PayPal, échéance passée : l'admin gère à la main (2026-10-08)
                    hand_due: subscriptions::hand_due(&s, now),
                    expires: s
                        .expires_at
                        .map(subscription_ops::date_text)
                        .unwrap_or_default(),
                    // abonnement arrêté chez PayPal (annulé, suspendu, expiré) : visible ici
                    source: match s.paypal_status.as_deref() {
                        Some(ps)
                            if s.paypal_sub_id.is_some() && ps != subscriptions::PAYPAL_ACTIVE =>
                        {
                            format!(
                                "{} · PayPal {}",
                                s.source,
                                subscriptions::paypal_status_label(ps)
                            )
                        }
                        _ => s.source.clone(),
                    },
                },
            );
        }
    }
    let html = accounts_page::render(&accounts_page::PageData {
        now,
        accounts: &list,
        max_premium: st.ctx.cfg.accounts.max_premium,
        max_playbacks: st.ctx.cfg.accounts.max_playbacks_per_user,
        token: token.unwrap_or(""),
        msg,
        links: &links,
        subs: &subs,
    });
    let mut resp = Html(html).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// Ligne « Canari de lecture » de `/status.html` : (ok, texte).
async fn canary_text(ctx: &TaskContext) -> Option<(bool, String)> {
    let c = ctx.state.read(|s| s.canary.clone()).await;
    let ok = c.last_ok?;
    let ago = status_page::ago(homelab_core::state::now(), c.last_run);
    Some((
        ok,
        if ok {
            format!("OK {ago} ({})", c.last_detail)
        } else {
            format!(
                "ÉCHEC il y a {ago}, {} de suite — {}",
                c.failures, c.last_detail
            )
        },
    ))
}

/// Bascule premium puis redirection (POST → GET) avec le résultat en paramètre.
async fn accounts_toggle(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<ToggleForm>,
) -> Response {
    if !accounts_allowed(&st, Some(&f.token)) {
        return denied();
    }
    let on = f.on == "1";
    let ip = client.ip;
    let who = accounts::list(&st.ctx)
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|a| a.id == f.user_id).map(|a| a.name))
        .unwrap_or_default();
    let code = match accounts::set_premium(&st.ctx, &f.user_id, on).await {
        Ok(Outcome::Activated) => {
            // le membre l'attendait : mail « ton compte est actif » (bouton vers Jellyfin ou la page de bienvenue)
            let ctx = st.ctx.clone();
            let (uid, name) = (f.user_id.clone(), who.clone());
            tokio::spawn(async move {
                match accounts::email_of(&ctx, &uid).await {
                    Ok(Some(email)) => {
                        if let Err(e) =
                            welcome::send(&ctx, &uid, &name, &email, welcome::KIND_ACTIVATED, true)
                                .await
                        {
                            warn!(task = "accounts", user = %name, error = format!("{e:#}"), "activation mail failed");
                        }
                    }
                    Ok(None) => {
                        warn!(task = "accounts", user = %name, "no e-mail known, activation mail skipped")
                    }
                    Err(e) => {
                        warn!(task = "accounts", user = %name, error = format!("{e:#}"), "activation mail: jellyseerr lookup failed")
                    }
                }
            });
            "activated"
        }
        Ok(Outcome::Suspended) => "suspended",
        Ok(Outcome::Unchanged) => "unchanged",
        Ok(Outcome::CapReached { .. }) => "cap",
        Err(e) => {
            warn!(task = "accounts", %ip, user = %who, error = format!("{e:#}"), "premium toggle failed");
            "error"
        }
    };
    info!(task = "accounts", %ip, user = %who, on, result = code, "premium toggle via web");
    let url = format!("/accounts?msg={code}&who={}", urlencode(&who));
    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
struct DeleteForm {
    token: String,
    user_id: String,
}

fn back_to_list(_token: &str, code: &str, who: &str) -> Response {
    Redirect::to(&format!("/accounts?msg={code}&who={}", urlencode(who))).into_response()
}

/// Page de confirmation (GET) : rien n'est supprimé ici.
async fn accounts_delete_confirm(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let token = q.get("token").map(String::as_str);
    if !accounts_allowed(&st, token) {
        return denied();
    }
    let token = token.unwrap_or("");
    let user_id = q.get("user_id").map(String::as_str).unwrap_or("");
    match accounts::list(&st.ctx).await {
        Ok(list) => match list.into_iter().find(|a| a.id == user_id) {
            Some(a) if a.protected => back_to_list(token, "protected", &a.name),
            Some(a) => Html(accounts_page::render_confirm(&a, token)).into_response(),
            None => back_to_list(token, "error", user_id),
        },
        Err(e) => {
            warn!(
                task = "accounts",
                error = format!("{e:#}"),
                "delete confirm: jellyfin unreachable"
            );
            back_to_list(token, "error", user_id)
        }
    }
}

async fn accounts_delete(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<DeleteForm>,
) -> Response {
    if !accounts_allowed(&st, Some(&f.token)) {
        return denied();
    }
    let ip = client.ip;
    let who = accounts::list(&st.ctx)
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|a| a.id == f.user_id).map(|a| a.name))
        .unwrap_or_else(|| f.user_id.clone());
    match accounts::delete(&st.ctx, &f.user_id).await {
        Ok(d) => {
            info!(task = "accounts", %ip, user = %d.name, jellyseerr = d.jellyseerr, "account deleted via web");
            back_to_list(&f.token, "deleted", &d.name)
        }
        Err(e) => {
            warn!(task = "accounts", %ip, user_id = %f.user_id, error = format!("{e:#}"), "account deletion failed");
            let code = if e.to_string().contains("protégé") {
                "protected"
            } else {
                "error"
            };
            back_to_list(&f.token, code, &who)
        }
    }
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

/// Texte affiché dans la colonne « Lien de bienvenue » de /accounts.
fn link_status_text(links: &[homelab_core::state::WelcomeLink], user_id: &str, now: i64) -> String {
    match welcome::status_for(links, user_id, now) {
        welcome::LinkStatus::None => "aucun".to_string(),
        welcome::LinkStatus::Pending { expires_at } => {
            format!(
                "envoyé, expire dans {} min",
                ((expires_at - now).max(0) + 59) / 60
            )
        }
        welcome::LinkStatus::Used { .. } => "mot de passe défini".to_string(),
        welcome::LinkStatus::Expired { .. } => "expiré".to_string(),
    }
}

/// Une requête par `rate_limit_secs` et par IP pour les pages publiques (même table que l'onboarding).
async fn rate_limited(st: &AppState, ip: &str) -> bool {
    let limit = Duration::from_secs(st.ctx.cfg.web.rate_limit_secs);
    let mut map = st.last_request.lock().await;
    let now = Instant::now();
    map.retain(|_, t| now.duration_since(*t) < limit);
    if map.contains_key(ip) {
        return true;
    }
    map.insert(ip.to_string(), now);
    false
}

fn page_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn public_page(tpl: &str, heading: &str, body: &str, status: StatusCode) -> Response {
    let html = tpl
        .replace("{{heading}}", &page_esc(heading))
        .replace("{{body}}", body);
    let mut resp = (status, Html(html)).into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// Adresse de Jellyfin sans le schéma (ce que le membre tape dans une appli).
fn jellyfin_host(s: &homelab_core::config::Secrets) -> String {
    s.jellyfin_public_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Premiers pas d'un membre (2026-09-25) : l'écran et l'appli à installer, la langue, la première demande, la
/// bulle « Aide et annonces », le guide. Servi après le choix du mot de passe (`access_block`) et tel quel sur
/// `/premiers-pas`.
/// Blocs dépliables plutôt qu'onglets : aucun JavaScript nécessaire (téléviseurs, WebView).
fn first_steps(s: &homelab_core::config::Secrets) -> String {
    let host = page_esc(&jellyfin_host(s));
    let jf = page_esc(&s.jellyfin_public_url);
    let js = page_esc(&s.jellyseerr_public_url);
    let guide = s
        .onboard_public_url
        .as_deref()
        .map(|u| format!("{}/guide", page_esc(u.trim_end_matches('/'))))
        .unwrap_or_else(|| "/guide".into());
    let discord = s
        .guide_contact_discord
        .as_deref()
        .map(|d| format!(" Sur Discord, écris à <b>{}</b>.", page_esc(d)))
        .unwrap_or_default();
    let quick = r#"<p class="qc"><b>Connexion sans clavier :</b> sur la télé, choisis <b>Quick Connect</b>. Un code s'affiche ; sur ton téléphone déjà connecté, touche ton profil (en haut à droite) → <b>Quick Connect</b> → saisis le code.</p>"#;
    format!(
        r#"<section class="steps" aria-label="Premiers pas">
<div class="step"><div class="n">1</div><div class="sc"><h2>Ton écran</h2>
<p class="dim">Adresse du serveur, à saisir une seule fois dans l'appli :</p>
<div class="addr"><code id="gc-host">{host}</code><button type="button" class="copy" data-copy="{host}">Copier</button></div>
<details><summary>Télé Android, Google TV, Fire TV, Shield</summary><p>Installe <b>Jellyfin</b> depuis le Play Store ou l'Appstore Amazon, puis saisis l'adresse.</p>{quick}</details>
<details><summary>Télé LG</summary><p>Installe <b>Jellyfin</b> depuis le LG Content Store, puis saisis l'adresse.</p>{quick}</details>
<details><summary>Télé Samsung</summary><p>Cherche <b>Jellyfin</b> dans le store de la télé. S'il n'y est pas, le plus simple est une clé Fire TV ou Google TV, ou de diffuser depuis ton téléphone.</p></details>
<details><summary>iPhone, iPad</summary><p>Installe <b>Jellyfin Mobile</b> (App Store), saisis l'adresse, puis ton identifiant et ton mot de passe. Pour la télé : bouton AirPlay du lecteur.</p></details>
<details><summary>Téléphone Android</summary><p>Installe <b>Jellyfin</b> depuis le <b>Play Store</b> (la version F-Droid n'a pas Chromecast), saisis l'adresse, puis tes identifiants.</p></details>
<details><summary>Ordinateur</summary><p>Ouvre simplement <a href="{jf}">{host}</a> dans ton navigateur, ou installe <b>Jellyfin Desktop</b> (Windows, Mac, Linux) depuis jellyfin.org.</p></details>
</div></div>
<div class="step"><div class="n">2</div><div class="sc"><h2>Ta langue</h2><p>En haut de Groscailloux, bouton <b>Mon compte</b> → <b>Langue de lecture</b> : la VF quand elle existe, ou <b>toujours en VO</b> avec sous-titres français (les animés en japonais). Tu y règles aussi la taille des sous-titres.</p></div></div>
<div class="step"><div class="n">3</div><div class="sc"><h2>Ta première demande</h2><p>Il manque un film ou une série ? Onglet <b>Découvrir</b> dans Groscailloux, ou le <a href="{js}">site des demandes</a>. Tu suis chaque demande dans l'onglet <b>Demandes</b>. Un film encore au cinéma arrive à sa sortie en VOD, environ 4 mois après la salle.</p></div></div>
<div class="step"><div class="n">4</div><div class="sc"><h2>Une question ?</h2><p><b>Aide et annonces</b> est dans Groscailloux, bulle en haut à droite (sur ordinateur et téléphone) : les nouvelles du serveur, l'entraide entre membres et un message privé à l'admin.{discord}</p></div></div>
<div class="step"><div class="n">5</div><div class="sc"><h2>Le guide complet</h2><p>Tout le reste, écran par écran : <a href="{guide}">le guide Groscailloux</a>.</p></div></div>
</section>"#
    )
}

/// Boutons vers les services + premiers pas, après la définition du mot de passe.
fn access_block(st: &AppState, username: &str, premium: bool) -> String {
    let s = &st.ctx.secrets;
    let pending = if premium {
        String::new()
    } else {
        r#"<div class="notice warn">Ton compte est en attente d'activation par l'administrateur : tu recevras un mail dès qu'il sera ouvert. D'ici là, la connexion sera refusée.</div>"#.to_string()
    };
    format!(
        r#"{pending}<p class="dim">Ton identifiant (le même partout) :</p><div class="who">{user}</div>
<div class="links"><a class="btn" href="{jf}">Regarder : ouvrir Groscailloux</a><a class="btn ghost" href="{js}">Demander un film ou une série</a></div>
{steps}"#,
        user = page_esc(username),
        jf = page_esc(&s.jellyfin_public_url),
        js = page_esc(&s.jellyseerr_public_url),
        steps = first_steps(s),
    )
}

/// `GET /premiers-pas` : les mêmes premiers pas, sans compte (lien du guide, de Mon compte et des mails).
async fn first_steps_page(State(st): State<AppState>) -> Response {
    let s = &st.ctx.secrets;
    let body = format!(
        r#"<p>Quatre minutes pour tout installer : ton écran, ta langue, ta première demande.</p>
<div class="links"><a class="btn" href="{jf}">Regarder : ouvrir Groscailloux</a></div>
{steps}"#,
        jf = page_esc(&s.jellyfin_public_url),
        steps = first_steps(s),
    );
    public_page(BIENVENUE_HTML, "Premiers pas", &body, StatusCode::OK)
}

fn password_form(username: &str, token: &str, demo: bool) -> String {
    format!(
        r#"<form id="pw-form" method="post" action="/bienvenue/{token}" data-username="{user}" autocomplete="off">
<div class="field"><label for="pw">Choisis ton mot de passe</label><input id="pw" name="pw" type="password" minlength="8" maxlength="128" required autocomplete="new-password" autofocus{dis}></div>
<div class="field"><label for="pw2">Confirme-le</label><input id="pw2" name="pw2" type="password" minlength="8" maxlength="128" required autocomplete="new-password"{dis}></div>
<p class="hint">8 caractères minimum, sans ton pseudo dedans. Il servira pour regarder et pour demander des titres.</p>
<div id="pw-err" class="notice err" hidden></div>
<button id="pw-btn" class="btn" type="submit" disabled>Enregistrer mon mot de passe</button>
</form>"#,
        token = page_esc(token),
        user = page_esc(username),
        dis = if demo { " disabled" } else { "" },
    )
}

fn expired_page() -> Response {
    public_page(
        BIENVENUE_HTML,
        "Ce lien n'est plus valable",
        r#"<p>Un lien de bienvenue ne sert qu'une fois et expire au bout d'une heure. Indique l'adresse e-mail de ton compte : si elle est connue, tu recevras un nouveau lien pour définir (ou changer) ton mot de passe.</p>
<form method="post" action="/bienvenue/renouveler"><div class="field"><label for="email">Ton adresse e-mail</label><input id="email" name="email" type="email" required autocomplete="email"></div><button class="btn" type="submit">Recevoir un nouveau lien</button></form>
<p class="foot">Rien reçu après quelques minutes ? Regarde dans les courriers indésirables, ou écris à l'administrateur.</p>"#,
        StatusCode::NOT_FOUND,
    )
}

/// GET /bienvenue/{token} : page où le membre définit son mot de passe.
async fn welcome_get(State(st): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(link) = welcome::lookup(&st.ctx, &token).await else {
        return expired_page();
    };
    let demo = link.kind == welcome::KIND_DEMO;
    let premium = if demo {
        false
    } else {
        accounts::list(&st.ctx)
            .await
            .ok()
            .and_then(|l| {
                l.into_iter()
                    .find(|a| a.id == link.user_id)
                    .map(|a| a.premium)
            })
            .unwrap_or(false)
    };
    let note = if demo {
        r#"<div class="notice ok">Ceci est un exemple : c'est la page qu'un nouveau membre voit en cliquant sur le bouton du mail. Rien n'est enregistré ici.</div>"#.to_string()
    } else if link.kind == welcome::KIND_ACTIVATED {
        r#"<div class="notice ok">Ton compte est actif. Choisis ton mot de passe pour te connecter.</div>"#.to_string()
    } else if premium {
        String::new()
    } else {
        r#"<div class="notice warn">Ton compte sera activé par l'administrateur sous peu : tu recevras un mail à ce moment-là. Tu peux déjà choisir ton mot de passe.</div>"#.to_string()
    };
    let body = format!(
        r#"<p>Ton compte <b>{user}</b> est prêt. Il ne reste qu'à choisir ton mot de passe.</p>{note}{form}"#,
        user = page_esc(&link.username),
        form = password_form(&link.username, &token, demo),
    );
    public_page(
        BIENVENUE_HTML,
        &format!("Bienvenue, {} !", link.username),
        &body,
        StatusCode::OK,
    )
}

#[derive(Deserialize)]
struct PwForm {
    pw: String,
    #[serde(default)]
    pw2: String,
}

fn password_ok(pw: &str, pw2: &str, username: &str) -> Result<(), &'static str> {
    if pw != pw2 {
        return Err("Les deux mots de passe ne sont pas identiques.");
    }
    let n = pw.chars().count();
    if n < 8 {
        return Err("Au moins 8 caractères.");
    }
    if n > 128 {
        return Err("128 caractères maximum.");
    }
    if pw.to_lowercase().contains(&username.to_lowercase()) {
        return Err("Le mot de passe ne doit pas contenir ton pseudo.");
    }
    Ok(())
}

/// POST /bienvenue/{token} : enregistre le mot de passe, consomme le lien, affiche les accès.
async fn welcome_post(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Form(f): Form<PwForm>,
) -> Response {
    let Some(link) = welcome::lookup(&st.ctx, &token).await else {
        return expired_page();
    };
    if let Err(msg) = password_ok(&f.pw, &f.pw2, &link.username) {
        let body = format!(
            r#"<div class="notice err">{msg}</div>{form}"#,
            form = password_form(&link.username, &token, false)
        );
        return public_page(
            BIENVENUE_HTML,
            &format!("Bienvenue, {} !", link.username),
            &body,
            StatusCode::BAD_REQUEST,
        );
    }
    if link.kind == welcome::KIND_DEMO {
        let body = format!(
            r#"<div class="notice ok">Exemple : ici le mot de passe serait enregistré, puis le membre verrait ses accès.</div>{}"#,
            access_block(&st, &link.username, true)
        );
        return public_page(BIENVENUE_HTML, "C'est prêt !", &body, StatusCode::OK);
    }
    if st.ctx.dry_run {
        return public_page(
            BIENVENUE_HTML,
            "Dry-run",
            "<p>Aucune écriture en dry-run.</p>",
            StatusCode::OK,
        );
    }
    if let Err(e) = st
        .ctx
        .jellyfin
        .set_password(&link.user_id, &Secret::new(f.pw))
        .await
    {
        warn!(task = "onboard", user = %link.username, error = format!("{e:#}"), "set password failed");
        let body = format!(
            r#"<div class="notice err">Impossible d'enregistrer le mot de passe pour l'instant. Réessaie dans un moment, ou écris à l'administrateur.</div>{}"#,
            password_form(&link.username, &token, false)
        );
        return public_page(
            BIENVENUE_HTML,
            &format!("Bienvenue, {} !", link.username),
            &body,
            StatusCode::BAD_GATEWAY,
        );
    }
    if let Err(e) = welcome::consume(&st.ctx, &token).await {
        warn!(task = "onboard", user = %link.username, error = format!("{e:#}"), "link consume failed");
    }
    info!(task = "onboard", user = %link.username, "password set via welcome link");
    let premium = accounts::list(&st.ctx)
        .await
        .ok()
        .and_then(|l| {
            l.into_iter()
                .find(|a| a.id == link.user_id)
                .map(|a| a.premium)
        })
        .unwrap_or(false);
    let body = format!(
        r#"<div class="notice ok">Mot de passe enregistré.</div>{}"#,
        access_block(&st, &link.username, premium)
    );
    public_page(
        BIENVENUE_HTML,
        &format!("C'est prêt, {} !", link.username),
        &body,
        StatusCode::OK,
    )
}

#[derive(Deserialize)]
struct RenewForm {
    email: String,
}

/// POST /bienvenue/renouveler : réponse identique que l'adresse soit connue ou non.
async fn welcome_renew(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<RenewForm>,
) -> Response {
    let ip = client.ip;
    let neutral = || {
        public_page(
            BIENVENUE_HTML,
            "Vérifie ta boîte mail",
            r#"<p>Si cette adresse correspond à un compte, un nouveau lien vient de partir. Il est valable une heure.</p><p class="foot">Rien reçu ? Regarde dans les courriers indésirables. Sinon, écris à l'administrateur.</p>"#,
            StatusCode::OK,
        )
    };
    if rate_limited(&st, &ip).await {
        return neutral();
    }
    let email = f.email.trim().to_lowercase();
    if !onboard::valid_email(&email) {
        return neutral();
    }
    let per_hour = st.ctx.cfg.onboard.renew_per_hour;
    let key = welcome::hash(&email);
    let now = homelab_core::state::now();
    let allowed = st
        .ctx
        .state
        .update(|s| welcome::renew_allowed(s.link_renewals.entry(key).or_default(), now, per_hour))
        .await
        .unwrap_or(false);
    if !allowed {
        return neutral();
    }
    match accounts::account_by_email(&st.ctx, &email).await {
        Ok(Some((uid, name))) => {
            let premium = accounts::list(&st.ctx)
                .await
                .ok()
                .and_then(|l| l.into_iter().find(|a| a.id == uid).map(|a| a.premium))
                .unwrap_or(false);
            match welcome::send(&st.ctx, &uid, &name, &email, welcome::KIND_WELCOME, premium).await
            {
                Ok(_) => info!(task = "onboard", user = %name, "welcome link renewed"),
                Err(e) => {
                    warn!(task = "onboard", user = %name, error = format!("{e:#}"), "welcome link renew failed")
                }
            }
        }
        Ok(None) => info!(task = "onboard", %ip, "link renewal for unknown address"),
        Err(e) => warn!(
            task = "onboard",
            error = format!("{e:#}"),
            "link renewal lookup failed"
        ),
    }
    neutral()
}

fn signup_form(username: &str, email: &str, err: Option<&str>) -> String {
    signup_form_p(username, email, "", err)
}

fn signup_form_p(username: &str, email: &str, parrain: &str, err: Option<&str>) -> String {
    format!(
        r#"{err}<p>Indique le pseudo que tu veux et ton adresse e-mail : tu recevras un lien pour choisir ton mot de passe. Chaque compte est ensuite validé par l'administrateur.</p>
<form method="post" action="/inscription" autocomplete="off">
<div class="field"><label for="username">Pseudo</label><input id="username" name="username" type="text" required pattern="[a-zA-Z0-9_-]{{2,32}}" maxlength="32" value="{u}" autocomplete="username"></div>
<div class="field"><label for="email">Adresse e-mail</label><input id="email" name="email" type="email" required maxlength="120" value="{e}" autocomplete="email"></div>
<div class="field"><label for="parrain">Code de parrainage <span class="opt">(facultatif)</span></label><input id="parrain" name="parrain" type="text" maxlength="8" value="{p}" autocomplete="off" autocapitalize="characters" spellcheck="false" pattern="[A-Za-z0-9]{{8}}"></div>
<div class="hp" aria-hidden="true"><label for="website">Site web</label><input id="website" name="website" type="text" tabindex="-1" autocomplete="off"></div>
<label class="check"><input type="checkbox" name="accept" value="1" required> Je sais que mon compte sera validé par l'administrateur avant de pouvoir regarder.</label>
<button class="btn" type="submit">Créer mon compte</button>
</form>
<p class="foot">Groscailloux est un service privé entre amis : l'administrateur peut refuser un compte qu'il ne connaît pas.</p>"#,
        err = err
            .map(|m| format!(r#"<div class="notice err">{}</div>"#, page_esc(m)))
            .unwrap_or_default(),
        u = page_esc(username),
        e = page_esc(email),
        p = page_esc(parrain),
    )
}

/// GET /inscription : page publique de création de compte.
async fn signup_get(State(st): State<AppState>) -> Response {
    if !st.ctx.cfg.onboard.public_signup {
        return StatusCode::NOT_FOUND.into_response();
    }
    public_page(
        INSCRIPTION_HTML,
        "Créer ton compte",
        &signup_form("", "", None),
        StatusCode::OK,
    )
}

#[derive(Deserialize)]
struct SignupForm {
    username: String,
    email: String,
    #[serde(default)]
    accept: String,
    #[serde(default)]
    website: String,
    #[serde(default)]
    parrain: String,
}

fn signup_done() -> Response {
    public_page(
        INSCRIPTION_HTML,
        "Vérifie ta boîte mail",
        r#"<div class="notice ok">Un lien vient de partir pour choisir ton mot de passe (valable une heure).</div><p>L'administrateur activera ton compte ensuite : tu recevras un second mail à ce moment-là.</p><p class="foot">Rien reçu ? Regarde dans les courriers indésirables.</p>"#,
        StatusCode::OK,
    )
}

/// POST /inscription : crée le compte (suspendu), envoie le lien, prévient l'admin.
async fn signup_post(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<SignupForm>,
) -> Response {
    if !st.ctx.cfg.onboard.public_signup {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ip = client.ip;
    let username = f.username.trim().to_string();
    let email = f.email.trim().to_lowercase();
    if !f.website.trim().is_empty() {
        // champ leurre rempli : un robot ; réponse neutre, rien de créé
        info!(task = "onboard", %ip, "signup honeypot hit");
        return signup_done();
    }
    if !onboard::valid_username(&username) {
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(
                &username,
                &email,
                Some("Pseudo invalide : 2 à 32 caractères, lettres, chiffres, tiret ou souligné."),
            ),
            StatusCode::BAD_REQUEST,
        );
    }
    if !onboard::valid_email(&email) {
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(&username, &email, Some("Adresse e-mail invalide.")),
            StatusCode::BAD_REQUEST,
        );
    }
    if f.accept != "1" {
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(&username, &email, Some("Coche la case pour continuer.")),
            StatusCode::BAD_REQUEST,
        );
    }
    if rate_limited(&st, &ip).await {
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(
                &username,
                &email,
                Some("Doucement : réessaie dans 30 secondes."),
            ),
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    // adresse déjà connue : réponse neutre (pas d'énumération), rien de créé
    match accounts::account_by_email(&st.ctx, &email).await {
        Ok(Some(_)) => {
            info!(task = "onboard", %ip, "signup with an address already in use");
            return signup_done();
        }
        Ok(None) => {}
        Err(e) => {
            warn!(
                task = "onboard",
                error = format!("{e:#}"),
                "signup: jellyseerr unreachable"
            );
            return public_page(
                INSCRIPTION_HTML,
                "Créer ton compte",
                &signup_form(
                    &username,
                    &email,
                    Some("Service momentanément indisponible, réessaie dans un instant."),
                ),
                StatusCode::BAD_GATEWAY,
            );
        }
    }
    if let Ok(Some(_)) = st.ctx.jellyfin.find_user(&username).await {
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(
                &username,
                &email,
                Some("Ce pseudo est déjà pris, choisis-en un autre."),
            ),
            StatusCode::CONFLICT,
        );
    }
    let max = st.ctx.cfg.onboard.max_signups_per_day;
    let now = homelab_core::state::now();
    let allowed = st
        .ctx
        .state
        .update(|s| welcome::signup_allowed(&mut s.signups, now, max))
        .await
        .unwrap_or(false);
    if !allowed {
        warn!(task = "onboard", %ip, "signup daily cap reached");
        return public_page(
            INSCRIPTION_HTML,
            "Créer ton compte",
            &signup_form(
                &username,
                &email,
                Some("Trop d'inscriptions aujourd'hui : réessaie demain."),
            ),
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    info!(task = "onboard", %ip, %username, "public signup");
    let req = OnboardRequest {
        username: username.clone(),
        email: email.clone(),
        password: None,
        source: onboard::Source::SelfSignup,
    };
    let parrain = f.parrain.trim().to_uppercase();
    match onboard::run(&st.ctx, req).await {
        Ok(r) => {
            // essai gratuit (`[subscriptions] trial_days`) : fiche « essai » + compte activé tout de suite
            if st.ctx.cfg.subscriptions.enabled {
                let referral = (!parrain.is_empty()).then_some(parrain.as_str());
                match subscription_ops::start_trial(&st.ctx, &r.jellyfin_id, &username, referral)
                    .await
                {
                    Ok(true) => info!(task = "subs", %username, "trial started at signup"),
                    Ok(false) => {}
                    Err(e) => {
                        warn!(task = "subs", %username, error = format!("{e:#}"), "trial not started")
                    }
                }
            }
            signup_done()
        }
        Err(e) => {
            warn!(task = "onboard", %ip, error = format!("{e:#}"), "public signup failed");
            public_page(INSCRIPTION_HTML, "Créer ton compte", &signup_form(&username, &email, Some("La création a échoué, réessaie dans un instant ou écris à l'administrateur.")), StatusCode::BAD_GATEWAY)
        }
    }
}

#[derive(Deserialize)]
struct LinkForm {
    token: String,
    user_id: String,
}

fn header_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-onboard-token").and_then(|v| v.to_str().ok())
}

#[derive(Deserialize)]
struct AdminLinkBody {
    user_id: String,
}

/// POST /admin/link (jeton en en-tête, utilisé par `homelabctl accounts link`) : les liens vivent dans l'état du
/// daemon, la CLI ne peut pas en émettre elle-même.
async fn admin_link(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<AdminLinkBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    let acc = match accounts::list(&st.ctx).await {
        Ok(l) => l
            .into_iter()
            .find(|a| a.id == b.user_id || a.name == b.user_id),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let Some(acc) = acc else {
        return fail(StatusCode::NOT_FOUND, "compte introuvable").into_response();
    };
    let email = match accounts::email_of(&st.ctx, &acc.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "aucune adresse e-mail connue pour ce compte",
            )
            .into_response()
        }
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    match welcome::send(
        &st.ctx,
        &acc.id,
        &acc.name,
        &email,
        welcome::KIND_WELCOME,
        acc.premium,
    )
    .await
    {
        Ok(sent) => {
            info!(task = "accounts", user = %acc.name, mail_sent = sent.mail_sent, "welcome link via CLI");
            Json(json!({ "success": true, "username": acc.name, "mail_sent": sent.mail_sent, "url": sent.url, "expires_at": sent.expires_at })).into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct MailTestBody {
    to: String,
}

/// POST /admin/mail-test (jeton en en-tête) : mail de bienvenue d'exemple, lien de démonstration.
#[derive(Deserialize)]
struct RunBody {
    task: String,
}

/// `POST /admin/run` (`homelabctl run <tâche>`) : un passage lancé DANS le daemon, seul propriétaire du fichier
/// d'état (la CLI l'écrasait avant le 2026-09-25). Répond quand le passage est fini (10 min au plus).
async fn admin_run(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<RunBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    use crate::scheduler::RunNow;
    info!(task = %b.task, "manual run requested via CLI");
    match crate::scheduler::run_now(st.ctx.clone(), &b.task).await {
        RunNow::Unknown => fail(
            StatusCode::NOT_FOUND,
            format!("tâche inconnue : {}", b.task),
        )
        .into_response(),
        RunNow::Busy => fail(
            StatusCode::CONFLICT,
            "tâche déjà en cours, réessayer à la fin du passage",
        )
        .into_response(),
        RunNow::Panicked => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "la tâche a paniqué (voir journalctl -u homelabd)",
        )
        .into_response(),
        RunNow::Done { ok, summary } => {
            Json(json!({ "success": true, "ok": ok, "summary": summary })).into_response()
        }
    }
}

#[derive(Deserialize)]
struct AdminAccountBody {
    action: String,
    user_id: String,
}

/// `POST /admin/accounts` (`homelabctl accounts on|off|delete`) : ces actions notent dans l'état les droits
/// Jellyseerr à restaurer ; elles passent donc par le daemon.
async fn admin_accounts(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<AdminAccountBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    match b.action.as_str() {
        "on" | "off" => match accounts::set_premium(&st.ctx, &b.user_id, b.action == "on").await {
            Ok(o) => {
                let (outcome, extra) = match o {
                    Outcome::Activated => ("activated", json!({})),
                    Outcome::Suspended => ("suspended", json!({})),
                    Outcome::Unchanged => ("unchanged", json!({})),
                    Outcome::CapReached { premium, max } => {
                        ("cap_reached", json!({ "premium": premium, "max": max }))
                    }
                };
                info!(task = "accounts", user_id = %b.user_id, outcome, "premium toggle via CLI");
                Json(json!({ "success": true, "outcome": outcome, "detail": extra }))
                    .into_response()
            }
            Err(e) => fail(StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        },
        "delete" => match accounts::delete(&st.ctx, &b.user_id).await {
            Ok(d) => {
                info!(task = "accounts", user = %d.name, jellyseerr = d.jellyseerr, "account deleted via CLI");
                Json(json!({ "success": true, "name": d.name, "jellyseerr": d.jellyseerr }))
                    .into_response()
            }
            Err(e) => fail(StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        },
        other => fail(
            StatusCode::BAD_REQUEST,
            format!("action inconnue : {other}"),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
struct VoNativeBody {
    direction: String,
}

/// `POST /admin/vo-native` (`homelabctl accounts vo-native|vo-classic`) : migration des comptes en mode VO vers « Langue
/// d'origine » ou retour à `vo_audio_language`. Dans le daemon : il écrit l'état (chaque changement y est noté) et la
/// sauvegarde d'avant ; la CLI fait elle-même le plan à blanc (`--dry-run`), sans rien écrire.
async fn admin_vo_native(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<VoNativeBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    let Some(dir) = homelab_core::vo_native::Direction::parse(&b.direction) else {
        return fail(
            StatusCode::BAD_REQUEST,
            format!("sens inconnu : {} (native | classic)", b.direction),
        )
        .into_response();
    };
    info!(
        task = "vo_native",
        direction = dir.as_str(),
        "migration requested via CLI"
    );
    let ctx = &st.ctx;
    if dir == homelab_core::vo_native::Direction::Native && !ctx.cfg.accounts.vo_native {
        return fail(
            StatusCode::CONFLICT,
            "[accounts] vo_native = false : passer l'interrupteur à true dans homelab.toml et redémarrer homelabd d'abord",
        )
        .into_response();
    }
    match homelab_core::vo_native::migrate(
        &ctx.jellyfin,
        &ctx.cfg.accounts,
        &ctx.cfg.paths.backups,
        &ctx.state,
        dir,
        ctx.dry_run,
    )
    .await
    {
        Ok(m) => Json(json!({ "success": true, "migration": m })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

async fn admin_mail_test(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<MailTestBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    let to = b.to.trim().to_lowercase();
    if !onboard::valid_email(&to) {
        return fail(StatusCode::BAD_REQUEST, "adresse invalide").into_response();
    }
    match welcome::send(&st.ctx, "demo", "Exemple", &to, welcome::KIND_DEMO, true).await {
        Ok(sent) => {
            info!(
                task = "onboard",
                mail_sent = sent.mail_sent,
                "demo welcome mail"
            );
            Json(json!({ "success": true, "mail_sent": sent.mail_sent, "url": sent.url }))
                .into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// POST /accounts/link : renvoie un lien de bienvenue (définir ou changer son mot de passe).
async fn accounts_link(State(st): State<AppState>, Form(f): Form<LinkForm>) -> Response {
    if !accounts_allowed(&st, Some(&f.token)) {
        return denied();
    }
    let acc = accounts::list(&st.ctx)
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|a| a.id == f.user_id));
    let Some(acc) = acc else {
        return back_to_list(&f.token, "error", "");
    };
    let code = match accounts::email_of(&st.ctx, &acc.id).await {
        Ok(Some(email)) => match welcome::send(
            &st.ctx,
            &acc.id,
            &acc.name,
            &email,
            welcome::KIND_WELCOME,
            acc.premium,
        )
        .await
        {
            Ok(sent) if sent.mail_sent => "link_sent",
            Ok(_) => "link_failed",
            Err(e) => {
                warn!(task = "accounts", user = %acc.name, error = format!("{e:#}"), "link resend failed");
                "link_failed"
            }
        },
        _ => "link_failed",
    };
    info!(task = "accounts", user = %acc.name, result = code, "welcome link resend via web");
    back_to_list(&f.token, code, &acc.name)
}

fn fail(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "success": false, "error": msg.into() })))
}

async fn onboard_handler(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    headers: HeaderMap,
    Json(body): Json<OnboardBody>,
) -> impl IntoResponse {
    // jeton posé par la session d'administration (`admin_auth`) ou par la CLI ; sans jeton configuré, fermé
    if !token_matches(
        st.ctx.secrets.onboard_token.as_ref(),
        header_token(&headers),
    ) {
        return fail(
            StatusCode::UNAUTHORIZED,
            "jeton manquant ou invalide : se reconnecter par /connexion",
        );
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
    let ip = client.ip;
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
        source: onboard::Source::Admin,
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
                        "lien envoyé"
                    } else {
                        "lien NON envoyé (transmettre le lien à la main)"
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
                    "password": r.password.as_ref().map(|p| p.expose().to_string()),
                    "link_url": r.link_url,
                    "link_expires_at": r.link_expires_at,
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
            warn!(task = "onboard", %ip, error = format!("{e:#}"), "web onboarding failed");
            fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premium_page_fills_every_placeholder() {
        let html = premium_page("CID-1", "P-9", false, "jo<hn", "3,50 € / mois", false);
        assert!(html.contains("client-id=CID-1"));
        assert!(html.contains("plan_id: 'P-9'"));
        assert!(html.contains("jo&lt;hn"));
        assert!(html.contains(r#"<main data-deja="0">"#));
        assert!(!html.contains("{{"));
    }

    #[test]
    fn premium_page_hides_the_button_for_an_account_already_subscribed() {
        let html = premium_page("CID-1", "P-9", false, "john", "3,50 € / mois", true);
        assert!(html.contains(r#"<main data-deja="1">"#));
        assert!(html.contains("a déjà un abonnement PayPal actif"));
        // le script s'arrête avant d'afficher le bouton PayPal
        let stop = html
            .find("getAttribute('data-deja') === '1') return;")
            .unwrap();
        assert!(stop < html.find("paypal.Buttons(").unwrap());
        assert!(!html.contains("{{"));
    }

    #[test]
    fn premium_deja_only_for_a_link_signed_by_homelabd() {
        // fiche qui se renouvelle toute seule par PayPal
        let auto = || subscriptions::Subscriber {
            user_id: "id1".into(),
            username: "john".into(),
            status: SubStatus::Active,
            starts_at: 0,
            expires_at: Some(2_000_000_000),
            source: "paypal".into(),
            paypal_sub_id: Some("I-ABCDEFGHIJ12".into()),
            paypal_email: None,
            referral_code: "ABCDEFGH".into(),
            referred_by: None,
            referral_credited: false,
            note: String::new(),
            reminded: 0,
            updated_at: 0,
            paypal_status: Some(subscriptions::PAYPAL_ACTIVE.into()),
            paypal_paid_at: None,
            due_noted: None,
        };
        let k = subscriptions::premium_link_key("secret", "john");
        assert!(premium_deja(Some("secret"), "john", Some(&k), |_| Some(
            auto()
        )));
        // sans clé, clé fausse ou d'un autre compte, ou sans secret : page normale, et la fiche n'est
        // même pas lue
        let unread =
            |_: &str| -> Option<subscriptions::Subscriber> { panic!("fiche lue sans clé valide") };
        assert!(!premium_deja(Some("secret"), "john", None, unread));
        assert!(!premium_deja(
            Some("secret"),
            "john",
            Some("0123456789abcdef"),
            unread
        ));
        assert!(!premium_deja(Some("secret"), "jane", Some(&k), unread));
        assert!(!premium_deja(None, "john", Some(&k), unread));
        assert!(!premium_deja(Some("secret"), "", Some(&k), unread));
        // clé valide mais abonnement arrêté chez PayPal, ou pas de fiche : page normale
        let mut stopped = auto();
        stopped.paypal_status = Some("CANCELLED".into());
        assert!(!premium_deja(Some("secret"), "john", Some(&k), |_| Some(
            stopped
        )));
        assert!(!premium_deja(Some("secret"), "john", Some(&k), |_| None));
    }

    #[test]
    fn activate_page_has_subscription_form() {
        let html = PREMIUM_ACTIVATE_HTML;
        assert!(html.contains(r#"<form method="post" action="/premium/activate">"#));
        assert!(html.contains(r#"name="name""#));
        assert!(html.contains(r#"type="submit""#));
        assert!(html.contains("/premium"));
    }

    #[test]
    fn activation_mail_contains_contact_and_plan() {
        let body = activation_mail_body(
            Some(&Donation {
                paypal_client_id: "CID-1".into(),
                paypal_plan_id: "P-9".into(),
            }),
            None,
            None,
            "john.doe",
            "1.2.3.4",
        );
        assert!(body.contains("john.doe"));
        assert!(body.contains("1.2.3.4"));
        assert!(body.contains("P-9"));
        assert!(body.contains("Epoch"));
        // sans jeton d'onboarding, la page Comptes est fermée : aucun lien
        assert!(!body.contains("/accounts"));
    }

    #[test]
    fn activation_mail_links_to_the_onboarding_host_only() {
        let tok = Secret::new("t".repeat(64));
        let body = activation_mail_body(
            None,
            Some(&tok),
            Some("https://onboarder.example.org/"),
            "john.doe",
            "1.2.3.4",
        );
        assert!(body.contains("\nhttps://onboarder.example.org/accounts\n"));
        // jamais le jeton dans le mail
        assert!(!body.contains(&"t".repeat(64)));
        assert!(!body.contains("token="));
        // adresse d'onboarding absente : la page est nommée, aucun domaine écrit en dur
        let body = activation_mail_body(None, Some(&tok), None, "john.doe", "1.2.3.4");
        assert!(body.contains("/accounts de l'adresse d'onboarding"));
        assert!(!body.contains("https://"));
    }
}
