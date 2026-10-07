//! Session d'administration par cookie : le jeton (`HOMELABD_ONBOARD_TOKEN`, ou `HOMELABD_STATUS_TOKEN` pour
//! `/status*` seulement) ne voyage plus dans les adresses. Jusqu'au 2026-09-23, `/accounts`, `/recherche` et
//! `/status.html` s'ouvraient avec `?token=…` : le jeton était écrit en clair dans les journaux du proxy à chaque
//! chargement, près de 5 000 fois.
//!
//! - `/connexion` (formulaire) ou un ancien lien `?token=` ouvre la session : cookie `gc_admin` signé
//!   (HMAC-SHA256 du jeton, sans état : il survit aux redémarrages, et changer le jeton ferme toutes les sessions),
//!   puis redirection vers l'adresse **sans** jeton.
//! - Avec la session, la couche réinjecte le jeton dans la requête **en interne** (requête, ou en-tête
//!   `X-Onboard-Token` pour `POST /onboard`) : les pages et leurs formulaires n'ont pas changé.
//! - Le jeton ne sort jamais dans une page (2026-10-07) : la couche le remplace dans le HTML par un **jeton de
//!   formulaire** dérivé (`form_token`, HMAC), et fait l'inverse dans un POST **avec session**. Les champs cachés
//!   de `/accounts` et `/recherche` le montraient en clair : un script injecté, ou n'importe quel appareil de la
//!   maison, repartait avec un jeton valable partout. Le jeton de formulaire, lui, ne vaut rien sans session.
//! - Échecs limités par adresse (`MAX_FAILS` par `FAIL_WINDOW`) et au total.
//! - IP de la maison (`HOMELABD_ADMIN_TRUSTED_IPS`) : session admin d'office, sans formulaire, et cookie posé au
//!   passage (si l'IP de la box change, le navigateur reste connecté). Demandé le 2026-09-23 : la connexion gênait
//!   la gestion depuis Homarr (iframe État comprise). Cette IP n'est lue que dans un `X-Forwarded-For` posé par NPM
//!   (`client_addr`) : jamais celui d'une connexion locale ou d'un autre conteneur (2026-10-07).
//! - Portes (2026-10-07) : les chemins d'administration (`is_admin_path`) répondent 404 sur un autre hôte que
//!   celui de `ONBOARD_PUBLIC_URL` ou une adresse locale — l'hôte premium (NPM 20) envoie tout à homelabd sans
//!   l'auth HTTP « admin-outils » de l'hôte d'onboarding. Les routes de la CLI (`/admin/*`) ne répondent qu'à un
//!   appel local (`homelabctl` → `127.0.0.1:8766`). Un POST sans session compte ses échecs comme `/connexion`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Extension;
use homelab_core::html::esc;
use homelab_core::{Secret, TaskContext};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::client_addr::{Client, ProxyTrust};

pub const COOKIE: &str = "gc_admin";
const SESSION_SECS: u64 = 365 * 24 * 3600;
const MAX_FAILS: usize = 10;
const MAX_FAILS_TOTAL: usize = 100;
const FAIL_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Jeton d'onboarding : tout (comptes, recherche, création, état).
    Admin,
    /// Jeton d'état : `/status` et `/status.html` seulement.
    Status,
}

impl Scope {
    fn tag(self) -> &'static str {
        match self {
            Scope::Admin => "a",
            Scope::Status => "s",
        }
    }
    fn covers(self, need: Scope) -> bool {
        self == Scope::Admin || need == Scope::Status
    }
}

/// Ce qu'une adresse exige ; `None` = publique (ou protégée autrement, comme les routes CLI par en-tête).
pub fn required(path: &str) -> Option<Scope> {
    match path {
        "/status" | "/status.html" => Some(Scope::Status),
        "/" | "/onboard" | "/accounts" | "/recherche" => Some(Scope::Admin),
        p if p.starts_with("/accounts/") || p.starts_with("/recherche/") => Some(Scope::Admin),
        _ => None,
    }
}

/// Chemins d'administration : servis seulement sur l'hôte d'onboarding ou une adresse locale. Les pages
/// publiques (`/premium`, `/inscription`, `/bienvenue`, `/guide`, `/paypal/webhook`, `/chat`, `/compte`…) restent
/// servies partout.
pub fn is_admin_path(path: &str) -> bool {
    required(path).is_some()
        || path == "/connexion"
        || path == "/admin"
        || path.starts_with("/admin/")
}

/// Nom d'hôte sans port, en minuscules, sans point final ni crochets (`[::1]:8766` → `::1`).
fn bare_host(raw: &str) -> String {
    let h = raw.trim().to_ascii_lowercase();
    let h = match h.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or("").to_string(),
        None => match h.rsplit_once(':') {
            Some((name, port))
                if !name.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                name.to_string()
            }
            _ => h,
        },
    };
    h.trim_end_matches('.').to_string()
}

/// Hôte d'une adresse publique (`https://hôte[:port]/…`, `ONBOARD_PUBLIC_URL`).
pub fn url_host(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = bare_host(authority.rsplit('@').next().unwrap_or(authority));
    (!h.is_empty()).then_some(h)
}

/// `localhost` ou une adresse IP locale ou privée : la CLI (`127.0.0.1:8766`), un conteneur qui vise la passerelle
/// (`172.18.0.1:8766`). Depuis Internet, un tel hôte tombe sur le serveur par défaut de NPM, jamais sur homelabd.
fn is_local_host(h: &str) -> bool {
    if h == "localhost" {
        return true;
    }
    match h.parse::<IpAddr>().map(|ip| ip.to_canonical()) {
        Ok(IpAddr::V4(a)) => a.is_loopback() || a.is_private(),
        Ok(IpAddr::V6(a)) => a.is_loopback() || a.is_unique_local(),
        Err(_) => false,
    }
}

/// L'en-tête `Host` permet-il un chemin d'administration ? `onboard` : hôte de `ONBOARD_PUBLIC_URL` ; absent, pas
/// de filtre (comportement d'origine, plutôt que d'enfermer l'admin dehors).
pub fn host_allowed(host: Option<&str>, onboard: Option<&str>) -> bool {
    let Some(onboard) = onboard else {
        return true;
    };
    match host.map(bare_host) {
        Some(h) if !h.is_empty() => h == onboard || is_local_host(&h),
        _ => false,
    }
}

/// Jeton attendu, donné et égal, comparé à temps constant. Aucun jeton configuré = refus.
pub fn token_matches(expected: Option<&Secret>, given: Option<&str>) -> bool {
    match (expected, given) {
        (Some(t), Some(g)) => !g.is_empty() && ct_eq(g.as_bytes(), t.expose().as_bytes()),
        _ => false,
    }
}

/// Plafond d'échecs atteint ? Le plafond global ne bloque pas un appel local (la CLI) : un balayage depuis
/// Internet ne doit pas fermer `homelabctl` à l'admin.
fn over_limit(mine: usize, total: usize, local: bool) -> bool {
    mine >= MAX_FAILS || (!local && total >= MAX_FAILS_TOTAL)
}

#[derive(Clone)]
pub struct AdminAuth {
    onboard_token: Option<Secret>,
    status_token: Option<Secret>,
    /// IP de la maison (`HOMELABD_ADMIN_TRUSTED_IPS`).
    home_ips: Arc<Vec<String>>,
    fails: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    proxy: Arc<ProxyTrust>,
    /// Hôte de `ONBOARD_PUBLIC_URL`, seul hôte public des pages d'administration.
    onboard_host: Option<String>,
}

impl AdminAuth {
    pub fn new(ctx: Arc<TaskContext>) -> Self {
        let onboard_host = ctx.secrets.onboard_public_url.as_deref().and_then(url_host);
        if onboard_host.is_none() {
            warn!(
                task = "admin",
                "ONBOARD_PUBLIC_URL absente : pages d'administration servies sur tous les hôtes"
            );
        }
        Self::from_parts(
            ctx.secrets.onboard_token.clone(),
            ctx.secrets.status_token.clone(),
            ctx.secrets.admin_trusted_ips.clone(),
            onboard_host,
            ProxyTrust::new(&ctx.cfg.web),
        )
    }

    fn from_parts(
        onboard_token: Option<Secret>,
        status_token: Option<Secret>,
        home_ips: Vec<String>,
        onboard_host: Option<String>,
        proxy: ProxyTrust,
    ) -> Self {
        Self {
            onboard_token,
            status_token,
            home_ips: Arc::new(home_ips),
            fails: Arc::new(Mutex::new(HashMap::new())),
            proxy: Arc::new(proxy),
            onboard_host,
        }
    }

    fn token(&self, scope: Scope) -> Option<&Secret> {
        match scope {
            Scope::Admin => self.onboard_token.as_ref(),
            Scope::Status => self.status_token.as_ref(),
        }
    }

    /// Jetons configurés, à masquer dans les pages. Un jeton de moins de 16 caractères n'est pas masqué : il
    /// abîmerait le texte des pages (et ne protège rien).
    fn tokens(&self) -> Vec<&Secret> {
        [self.onboard_token.as_ref(), self.status_token.as_ref()]
            .into_iter()
            .flatten()
            .filter(|t| t.expose().len() >= 16)
            .collect()
    }

    /// Le jeton donné, reconnu (comparaison à temps constant).
    fn scope_of(&self, given: &str) -> Option<Scope> {
        [Scope::Admin, Scope::Status].into_iter().find(|&s| {
            self.token(s).is_some_and(|t| {
                !given.is_empty() && ct_eq(given.as_bytes(), t.expose().as_bytes())
            })
        })
    }

    fn cookie_value(&self, scope: Scope, exp: u64) -> Option<String> {
        let t = self.token(scope)?;
        Some(format!(
            "{}.{exp}.{}",
            scope.tag(),
            sign(t.expose(), scope, exp)
        ))
    }

    fn verify(&self, value: &str, now: u64) -> Option<Scope> {
        check_cookie(value, now, |s| {
            self.token(s).map(|t| t.expose().to_string())
        })
    }

    fn set_cookie(&self, scope: Scope) -> Option<HeaderValue> {
        let v = self.cookie_value(scope, now() + SESSION_SECS)?;
        HeaderValue::from_str(&format!(
            "{COOKIE}={v}; Path=/; Max-Age={SESSION_SECS}; HttpOnly; Secure; SameSite=Lax"
        ))
        .ok()
    }

    /// Trop d'échecs (cette adresse ou au total) : on ne regarde même plus le jeton.
    async fn blocked(&self, c: &Client) -> bool {
        let mut f = self.fails.lock().await;
        let cutoff = Instant::now() - FAIL_WINDOW;
        f.retain(|_, v| {
            v.retain(|t| *t > cutoff);
            !v.is_empty()
        });
        let total: usize = f.values().map(Vec::len).sum();
        over_limit(f.get(&c.ip).map(Vec::len).unwrap_or(0), total, c.local)
    }

    async fn fail(&self, ip: &str, path: &str) {
        self.fails
            .lock()
            .await
            .entry(ip.to_string())
            .or_default()
            .push(Instant::now());
        warn!(task = "admin", ip, path, "jeton admin incorrect");
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104), clé = le jeton.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Jeton de formulaire : dérivé du jeton, seul à apparaître dans les pages. Sans session, il ne vaut rien
/// (`unmask_form` ne le remplace que pour une requête authentifiée) ; il change avec le jeton.
fn form_token(token: &str) -> String {
    hex(&hmac_sha256(token.as_bytes(), b"gc-admin-form-v1"))
}

/// Toutes les formes du jeton dans une page (brute, échappée HTML, encodée URL) deviennent le jeton de
/// formulaire. `None` : rien à changer.
fn mask_token(page: &str, token: &str, form: &str) -> Option<String> {
    let mut out = page.to_string();
    let mut changed = false;
    for v in [token.to_string(), esc(token), encode(token)] {
        if !v.is_empty() && out.contains(&v) {
            out = out.replace(&v, form);
            changed = true;
        }
    }
    changed.then_some(out)
}

/// Corps d'un formulaire (`application/x-www-form-urlencoded`) : `token=<jeton de formulaire>` redevient le
/// jeton attendu par la page. `None` : rien à changer.
fn unmask_form(body: &str, token: &str, form: &str) -> Option<String> {
    let mut changed = false;
    let pairs: Vec<String> = body
        .split('&')
        .map(|kv| match kv.strip_prefix("token=") {
            Some(v) if ct_eq(decode(v).as_bytes(), form.as_bytes()) => {
                changed = true;
                format!("token={}", encode(token))
            }
            _ => kv.to_string(),
        })
        .collect();
    changed.then(|| pairs.join("&"))
}

/// Formulaire d'une page admin : jamais plus gros que ça.
const FORM_LIMIT: usize = 1 << 20;
/// Page admin (la recherche peut lister quelques centaines de releases).
const PAGE_LIMIT: usize = 16 << 20;

/// POST avec session : remplace le jeton de formulaire par le vrai jeton dans le corps.
async fn unmask_request(req: Request, token: &str) -> Result<Request, StatusCode> {
    let is_form = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
    if req.method() != Method::POST || !is_form {
        return Ok(req);
    }
    let (mut parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, FORM_LIMIT)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let new = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|b| unmask_form(b, token, &form_token(token)));
    let bytes = match new {
        Some(b) => {
            parts
                .headers
                .insert(header::CONTENT_LENGTH, HeaderValue::from(b.len()));
            axum::body::Bytes::from(b)
        }
        None => bytes,
    };
    Ok(Request::from_parts(parts, Body::from(bytes)))
}

/// Page HTML d'administration : le jeton n'en sort jamais (voir `form_token`).
async fn mask_response(resp: Response, tokens: &[&Secret]) -> Response {
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if !is_html || tokens.is_empty() {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, PAGE_LIMIT).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut page) = String::from_utf8(bytes.to_vec()) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let mut changed = false;
    for t in tokens {
        if let Some(p) = mask_token(&page, t.expose(), &form_token(t.expose())) {
            page = p;
            changed = true;
        }
    }
    if !changed {
        return Response::from_parts(parts, Body::from(bytes));
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(page))
}

fn sign(token: &str, scope: Scope, exp: u64) -> String {
    hex(&hmac_sha256(
        token.as_bytes(),
        format!("gc-admin-v1|{}|{exp}", scope.tag()).as_bytes(),
    ))
}

/// Cookie `<portée>.<expiration>.<hmac>` valide pour le jeton actuel de cette portée ?
fn check_cookie(value: &str, now: u64, token: impl Fn(Scope) -> Option<String>) -> Option<Scope> {
    let mut it = value.splitn(3, '.');
    let scope = match it.next()? {
        "a" => Scope::Admin,
        "s" => Scope::Status,
        _ => return None,
    };
    let exp: u64 = it.next()?.parse().ok()?;
    let mac = it.next()?;
    if exp < now {
        return None;
    }
    let t = token(scope)?;
    ct_eq(mac.as_bytes(), sign(&t, scope, exp).as_bytes()).then_some(scope)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Le client est-il l'admin à la maison ? Seulement par l'adresse que NPM a vue (`X-Forwarded-For` d'un proxy de
/// confiance) : jamais une connexion locale ni un autre conteneur, même si leur adresse est listée.
fn at_home(c: &Client, list: &[String]) -> bool {
    c.via_proxy && list.iter().any(|t| t == &c.ip)
}

fn cookie_of(req: &Request) -> Option<String> {
    req.headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sépare `token=` du reste de la requête (le reste est gardé tel quel, encodage compris).
fn split_token(query: &str) -> (Option<String>, String) {
    let mut tok = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            if let Some(v) = kv.strip_prefix("token=") {
                tok = Some(decode(v));
                false
            } else {
                !kv.is_empty()
            }
        })
        .collect();
    (tok, rest.join("&"))
}

fn with_query(path: &str, query: &str) -> String {
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `next` sûr : un chemin local, jamais `//hôte` ni une adresse complète.
fn safe_next(next: &str) -> String {
    if next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.contains("token=")
    {
        next.to_string()
    } else {
        "/accounts".to_string()
    }
}

fn redirect(to: &str, cookie: Option<HeaderValue>) -> Response {
    let mut r = (StatusCode::SEE_OTHER, [(header::LOCATION, to.to_string())]).into_response();
    if let Some(c) = cookie {
        r.headers_mut().insert(header::SET_COOKIE, c);
    }
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

pub fn login_page(next: &str, msg: &str, status: StatusCode) -> Response {
    let flash = if msg.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="err">{}</p>"#, esc(msg))
    };
    let html = format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex"><title>Connexion admin</title>
<style>:root{{color-scheme:light dark;--bg:#f6f7f9;--fg:#15171a;--card:#fff;--line:#d6d9de;--acc:#2f6fdf;--err:#b3261e}}
@media (prefers-color-scheme:dark){{:root{{--bg:#111316;--fg:#e8eaed;--card:#1b1e22;--line:#33373d;--acc:#7aa7ff;--err:#ff8a80}}}}
body{{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;padding:16px;box-sizing:border-box}}
form{{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:24px;width:100%;max-width:360px;box-sizing:border-box}}
h1{{font-size:1.2rem;margin:0 0 16px}}label{{display:block;font-size:.9rem;margin-bottom:6px}}
input{{width:100%;box-sizing:border-box;padding:10px;border:1px solid var(--line);border-radius:8px;background:transparent;color:inherit;font:inherit}}
button{{margin-top:16px;width:100%;padding:10px;border:0;border-radius:8px;background:var(--acc);color:#fff;font:inherit;cursor:pointer}}
.err{{color:var(--err);margin:0 0 12px}}.alt{{font-size:.85rem;margin:12px 0 0;opacity:.8}}.alt a{{color:var(--acc)}}</style></head><body>
<form method="post" action="/connexion"><h1>Administration</h1>{flash}<input type="hidden" name="next" value="{next}"><label for="t">Jeton d'accès</label><input id="t" type="password" name="token" autocomplete="current-password" required autofocus><button type="submit">Ouvrir la session</button><p class="alt"><a href="/connexion?next={next_url}" target="_blank" rel="noopener">Ouvrir dans un onglet</a> (si ce formulaire est dans un cadre, comme le tableau Homarr)</p></form></body></html>"#,
        next = esc(next),
        next_url = esc(&encode(next))
    );
    let mut r = (status, Html(html)).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// `GET /connexion` : le formulaire.
pub async fn login_get(
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let next = safe_next(q.get("next").map(String::as_str).unwrap_or("/accounts"));
    login_page(&next, "", StatusCode::OK)
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    token: String,
    #[serde(default)]
    next: String,
}

/// `POST /connexion` : le jeton dans le corps (jamais dans l'adresse), cookie, retour à la page demandée.
pub async fn login_post(
    State(auth): State<AdminAuth>,
    Extension(client): Extension<Client>,
    axum::Form(form): axum::Form<LoginForm>,
) -> Response {
    let ip = client.ip.as_str();
    let next = safe_next(&form.next);
    if auth.blocked(&client).await {
        return login_page(
            &next,
            "Trop d'essais, réessaie dans 15 minutes.",
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    match auth.scope_of(form.token.trim()) {
        Some(scope) => {
            info!(
                task = "admin",
                ip,
                scope = scope.tag(),
                "admin session opened"
            );
            let next = match scope {
                Scope::Status
                    if required(next.split('?').next().unwrap_or("")) != Some(Scope::Status) =>
                {
                    "/status.html".to_string()
                }
                _ => next,
            };
            redirect(&next, auth.set_cookie(scope))
        }
        None => {
            auth.fail(ip, "/connexion").await;
            login_page(&next, "Jeton incorrect.", StatusCode::UNAUTHORIZED)
        }
    }
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// Réponse d'un POST bloqué : JSON pour `/onboard` (lu par la page de création), HTML ailleurs.
fn too_many(path: &str) -> Response {
    const MSG: &str = "Trop d'essais, réessaie dans 15 minutes.";
    if path == "/onboard" {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "success": false, "error": MSG })),
        )
            .into_response()
    } else {
        (StatusCode::TOO_MANY_REQUESTS, Html(format!("<p>{MSG}</p>"))).into_response()
    }
}

/// Couche posée sur toute l'application : pose `Client` dans les extensions de chaque requête, puis garde les
/// portes d'administration.
pub async fn layer(State(auth): State<AdminAuth>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let client = auth.proxy.client(peer, req.headers()).await;
    req.extensions_mut().insert(client.clone());
    if is_admin_path(&path) {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| req.uri().authority().map(|a| a.to_string()));
        if !host_allowed(host.as_deref(), auth.onboard_host.as_deref()) {
            return not_found();
        }
    }
    // Routes de la CLI : appel local seulement, jamais à travers NPM ni depuis un conteneur.
    if path.starts_with("/admin/") {
        if !client.local {
            if req.method() == Method::POST {
                warn!(task = "admin", ip = %client.ip, path, "API admin refusée : appel non local");
            }
            return not_found();
        }
        let resp = next.run(req).await;
        if resp.status() == StatusCode::UNAUTHORIZED {
            warn!(task = "admin", path, "API admin : jeton incorrect");
        }
        return resp;
    }
    let Some(need) = required(&path) else {
        return next.run(req).await;
    };
    // `/status*` sans jeton d'état configuré : ouvert (comportement d'origine).
    if need == Scope::Status && auth.token(Scope::Status).is_none() {
        return next.run(req).await;
    }
    let query = req.uri().query().unwrap_or("").to_string();
    let (given, rest) = split_token(&query);
    let ip = client.ip.as_str();
    let is_get = req.method() == Method::GET || req.method() == Method::HEAD;

    // Ancien lien avec `?token=` : ouvre la session et renvoie vers l'adresse sans jeton.
    if let (Some(given), true) = (given.as_deref(), is_get) {
        if auth.blocked(&client).await {
            return login_page(
                &with_query(&path, &rest),
                "Trop d'essais, réessaie dans 15 minutes.",
                StatusCode::TOO_MANY_REQUESTS,
            );
        }
        return match auth.scope_of(given) {
            Some(scope) if scope.covers(need) => {
                info!(
                    task = "admin",
                    ip,
                    scope = scope.tag(),
                    "admin session opened from a legacy ?token= link"
                );
                // une session admin déjà ouverte n'est pas rétrogradée par un lien d'état
                let keep =
                    cookie_of(&req).and_then(|c| auth.verify(&c, now())) == Some(Scope::Admin);
                redirect(
                    &with_query(&path, &rest),
                    if keep { None } else { auth.set_cookie(scope) },
                )
            }
            _ => {
                auth.fail(ip, &path).await;
                login_page(
                    &with_query(&path, &rest),
                    "Jeton incorrect.",
                    StatusCode::UNAUTHORIZED,
                )
            }
        };
    }

    let cookie = cookie_of(&req).and_then(|c| auth.verify(&c, now()));
    let from_home = at_home(&client, &auth.home_ips);
    let session = if from_home {
        Some(Scope::Admin)
    } else {
        cookie
    };
    // IP de la maison sans cookie admin : on le pose au passage
    let new_cookie = if from_home && cookie != Some(Scope::Admin) {
        auth.set_cookie(Scope::Admin)
    } else {
        None
    };
    match session {
        Some(scope) if scope.covers(need) => {
            // réinjecte le jeton attendu par la page, en interne seulement
            if let Some(t) = auth.token(need).map(|t| t.expose().to_string()) {
                if path == "/onboard" {
                    if !req.headers().contains_key("x-onboard-token") {
                        if let Ok(v) = HeaderValue::from_str(&t) {
                            req.headers_mut().insert("x-onboard-token", v);
                        }
                    }
                } else if given.is_none() {
                    let q = if query.is_empty() {
                        format!("token={}", encode(&t))
                    } else {
                        format!("{query}&token={}", encode(&t))
                    };
                    if let Ok(uri) = Uri::builder().path_and_query(format!("{path}?{q}")).build() {
                        *req.uri_mut() = uri;
                    }
                }
                // formulaire de la page : jeton de formulaire → jeton
                req = match unmask_request(req, &t).await {
                    Ok(r) => r,
                    Err(code) => return code.into_response(),
                };
            }
            let mut resp = mask_response(next.run(req).await, &auth.tokens()).await;
            strip_location(&mut resp);
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(c) = new_cookie {
                resp.headers_mut().append(header::SET_COOKIE, c);
            }
            resp
        }
        // Pas de session : une page s'ouvre sur le formulaire ; un POST garde son propre contrôle (jeton du
        // formulaire ou de l'en-tête, pour la CLI), et ses échecs comptent comme ceux de `/connexion`.
        _ if is_get => login_page(&with_query(&path, &rest), "", StatusCode::UNAUTHORIZED),
        _ => {
            if auth.blocked(&client).await {
                return too_many(&path);
            }
            let resp = next.run(req).await;
            if resp.status() == StatusCode::UNAUTHORIZED {
                auth.fail(ip, &path).await;
            }
            let mut resp = mask_response(resp, &auth.tokens()).await;
            strip_location(&mut resp);
            resp
        }
    }
}

/// Une redirection d'une page ne doit jamais remettre le jeton dans l'adresse.
fn strip_location(resp: &mut Response<Body>) {
    let Some(loc) = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
    else {
        return;
    };
    if let Some((p, q)) = loc.split_once('?') {
        let (tok, rest) = split_token(q);
        if tok.is_some() {
            if let Ok(v) = HeaderValue::from_str(&with_query(p, &rest)) {
                resp.headers_mut().insert(header::LOCATION, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn routes_are_classified() {
        assert_eq!(required("/status.html"), Some(Scope::Status));
        assert_eq!(required("/accounts/premium"), Some(Scope::Admin));
        assert_eq!(required("/recherche/resultats"), Some(Scope::Admin));
        assert_eq!(required("/"), Some(Scope::Admin));
        assert_eq!(required("/bienvenue/abc"), None);
        assert_eq!(required("/admin/link"), None); // CLI : en-tête
        assert_eq!(required("/connexion"), None);
        assert_eq!(required("/accountsx"), None);
    }

    #[test]
    fn token_is_split_from_the_query() {
        let (t, rest) = split_token("token=a%2Bb&job=3&msg=ok");
        assert_eq!(t.as_deref(), Some("a+b"));
        assert_eq!(rest, "job=3&msg=ok");
        let (t, rest) = split_token("job=3");
        assert_eq!(t, None);
        assert_eq!(rest, "job=3");
    }

    #[test]
    fn next_stays_local() {
        assert_eq!(safe_next("/recherche?q=x"), "/recherche?q=x");
        assert_eq!(safe_next("//evil.example"), "/accounts");
        assert_eq!(safe_next("https://evil.example"), "/accounts");
        assert_eq!(safe_next("/\\evil"), "/accounts");
    }

    #[test]
    fn scope_coverage() {
        assert!(Scope::Admin.covers(Scope::Status));
        assert!(Scope::Admin.covers(Scope::Admin));
        assert!(Scope::Status.covers(Scope::Status));
        assert!(!Scope::Status.covers(Scope::Admin));
    }

    #[test]
    fn cookie_round_trip() {
        let tok = |s: Scope| match s {
            Scope::Admin => Some("admin-token".to_string()),
            Scope::Status => Some("status-token".to_string()),
        };
        let exp = 2_000;
        let a = format!("a.{exp}.{}", sign("admin-token", Scope::Admin, exp));
        assert_eq!(check_cookie(&a, 1_000, tok), Some(Scope::Admin));
        assert_eq!(check_cookie(&a, 2_001, tok), None, "expired");
        // signé avec le jeton d'état mais présenté comme admin : refusé
        let forged = format!("a.{exp}.{}", sign("status-token", Scope::Admin, exp));
        assert_eq!(check_cookie(&forged, 1_000, tok), None);
        // expiration modifiée : la signature ne suit pas
        let stretched = a.replacen("2000", "9000", 1);
        assert_eq!(check_cookie(&stretched, 1_000, tok), None);
        // jeton renouvelé : les anciennes sessions tombent
        assert_eq!(check_cookie(&a, 1_000, |_| Some("new".to_string())), None);
        assert_eq!(check_cookie("garbage", 1_000, tok), None);
    }

    fn client(ip: &str, via_proxy: bool, local: bool) -> Client {
        Client {
            ip: ip.into(),
            via_proxy,
            local,
        }
    }

    #[test]
    fn home_ip_only_through_the_proxy() {
        let list = vec!["203.0.113.9".to_string(), "127.0.0.1".to_string()];
        assert!(at_home(&client("203.0.113.9", true, false), &list));
        assert!(!at_home(&client("203.0.113.10", true, false), &list));
        assert!(!at_home(&client("203.0.113.9", true, false), &[]));
        // même listée, une adresse qui ne vient pas de NPM n'ouvre rien (processus local, autre conteneur)
        assert!(!at_home(&client("127.0.0.1", false, true), &list));
        assert!(!at_home(&client("203.0.113.9", false, false), &list));
    }

    #[test]
    fn admin_paths() {
        for p in [
            "/",
            "/accounts",
            "/accounts/delete",
            "/recherche/telecharger",
            "/status",
            "/status.html",
            "/onboard",
            "/connexion",
            "/admin/run",
            "/admin/chat/announce",
        ] {
            assert!(is_admin_path(p), "{p}");
        }
        for p in [
            "/premium",
            "/premium/lier",
            "/inscription",
            "/bienvenue/abc",
            "/premiers-pas",
            "/guide",
            "/paypal/webhook",
            "/chat/api/me",
            "/compte/api/me",
            "/health",
            "/healthz",
            "/don",
            "/administration",
        ] {
            assert!(!is_admin_path(p), "{p}");
        }
    }

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(
            url_host("https://Onboarder.Example.org/").as_deref(),
            Some("onboarder.example.org")
        );
        assert_eq!(
            url_host("https://user@onboarder.example.org:8443/x?y").as_deref(),
            Some("onboarder.example.org")
        );
        assert_eq!(
            url_host("onboarder.example.org").as_deref(),
            Some("onboarder.example.org")
        );
        assert_eq!(url_host("https://"), None);
        assert_eq!(bare_host("127.0.0.1:8766"), "127.0.0.1");
        assert_eq!(bare_host("[::1]:8766"), "::1");
        assert_eq!(bare_host("Onboarder.Example.org."), "onboarder.example.org");
    }

    #[test]
    fn admin_paths_only_on_the_onboarding_host_or_locally() {
        let ob = Some("onboarder.example.org");
        assert!(host_allowed(Some("onboarder.example.org"), ob));
        assert!(host_allowed(Some("ONBOARDER.example.org:443"), ob));
        // l'hôte premium (NPM 20) et l'hôte de Jellyfin : 404
        assert!(!host_allowed(Some("premium.example.org"), ob));
        assert!(!host_allowed(Some("jellyfin.example.org"), ob));
        assert!(!host_allowed(
            Some("onboarder.example.org.evil.example"),
            ob
        ));
        assert!(!host_allowed(None, ob));
        assert!(!host_allowed(Some(""), ob));
        // la CLI, un conteneur qui vise la passerelle
        assert!(host_allowed(Some("127.0.0.1:8766"), ob));
        assert!(host_allowed(Some("localhost:8766"), ob));
        assert!(host_allowed(Some("[::1]:8766"), ob));
        assert!(host_allowed(Some("172.18.0.1:8766"), ob));
        // une IP publique n'est pas « locale »
        assert!(!host_allowed(Some("203.0.113.9"), ob));
        // sans ONBOARD_PUBLIC_URL : pas de filtre (comportement d'origine)
        assert!(host_allowed(Some("premium.example.org"), None));
    }

    #[test]
    fn tokens_compare_in_constant_time_and_never_match_nothing() {
        let t = Secret::new("s3cret-token");
        assert!(token_matches(Some(&t), Some("s3cret-token")));
        assert!(!token_matches(Some(&t), Some("s3cret-tokeN")));
        assert!(!token_matches(Some(&t), Some("")));
        assert!(!token_matches(Some(&t), None));
        assert!(!token_matches(None, Some("s3cret-token")));
        assert!(!token_matches(Some(&Secret::new("")), Some("")));
    }

    #[test]
    fn global_cap_never_locks_the_cli_out() {
        assert!(!over_limit(MAX_FAILS - 1, 0, false));
        assert!(over_limit(MAX_FAILS, 0, false));
        assert!(
            over_limit(MAX_FAILS, 0, true),
            "la CLI garde sa propre limite"
        );
        assert!(over_limit(0, MAX_FAILS_TOTAL, false));
        assert!(!over_limit(0, MAX_FAILS_TOTAL, true));
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[test]
    fn pages_only_show_the_form_token() {
        let form = form_token("a+b&c");
        assert_eq!(form.len(), 64);
        assert_ne!(form, form_token("autre"), "change avec le jeton");
        let page = r#"<input name="token" value="a+b&amp;c"><a href="/x?t=a%2Bb%26c">a+b&c</a>"#;
        let masked = mask_token(page, "a+b&c", &form).unwrap();
        assert!(!masked.contains("a+b"), "{masked}");
        assert!(!masked.contains("a%2Bb"), "{masked}");
        assert_eq!(masked.matches(&form).count(), 3);
        assert_eq!(mask_token("<p>rien</p>", "a+b&c", &form), None);
    }

    #[test]
    fn form_token_maps_back_to_the_token() {
        let form = form_token("s3cret");
        let body = format!("token={form}&user_id=42&on=1");
        assert_eq!(
            unmask_form(&body, "s3cret", &form).as_deref(),
            Some("token=s3cret&user_id=42&on=1")
        );
        // le vrai jeton (ancienne page encore ouverte) ou un faux : inchangés, la page décide
        assert_eq!(
            unmask_form("token=s3cret&user_id=42", "s3cret", &form),
            None
        );
        assert_eq!(unmask_form("token=faux&user_id=42", "s3cret", &form), None);
        // seul le champ `token` est touché
        assert_eq!(unmask_form(&format!("q={form}"), "s3cret", &form), None);
        // le jeton est encodé dans le corps
        assert_eq!(
            unmask_form(
                &format!("token={}", form_token("a+b")),
                "a+b",
                &form_token("a+b")
            )
            .as_deref(),
            Some("token=a%2Bb")
        );
    }

    /// La couche entière, devant de fausses pages : pair TCP, `Host` et `X-Forwarded-For` choisis par le test.
    mod gates {
        use super::super::*;
        use axum::routing::{get, post};
        use axum::Router;
        use tower::ServiceExt;

        const ONBOARD: &str = "onboarder.example.org";
        const PREMIUM: &str = "premium.example.org";
        const HOME: &str = "203.0.113.9";
        const NPM: &str = "172.18.0.19";
        const TOKEN: &str = "jeton-admin-de-test";

        fn app() -> Router {
            // réseau Docker seul : aucun `docker inspect` pendant les tests
            let web = homelab_core::config::Web {
                trusted_proxy_container: String::new(),
                ..Default::default()
            };
            let auth = AdminAuth::from_parts(
                Some(Secret::new(TOKEN)),
                None,
                vec![HOME.to_string()],
                Some(ONBOARD.to_string()),
                ProxyTrust::new(&web),
            );
            let whoami = |Extension(c): Extension<Client>| async move {
                format!("{}|{}|{}", c.ip, c.via_proxy, c.local)
            };
            // la page vérifie le jeton réinjecté par la session, comme les vraies
            let accounts = |axum::extract::Query(q): axum::extract::Query<
                HashMap<String, String>,
            >| async move {
                if token_matches(
                    Some(&Secret::new(TOKEN)),
                    q.get("token").map(String::as_str),
                ) {
                    (StatusCode::OK, "comptes")
                } else {
                    (StatusCode::UNAUTHORIZED, "refusé")
                }
            };
            // page à formulaire, comme `/accounts` : le jeton reçu est écrit dans un champ caché
            let page = |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                let t = q.get("token").cloned().unwrap_or_default();
                Html(format!(
                    r#"<form method="post" action="/accounts/premium"><input type="hidden" name="token" value="{}"></form>"#,
                    esc(&t)
                ))
            };
            #[derive(serde::Deserialize)]
            struct Toggle {
                token: String,
            }
            let toggle = |axum::Form(f): axum::Form<Toggle>| async move {
                if token_matches(Some(&Secret::new(TOKEN)), Some(&f.token)) {
                    (StatusCode::OK, "basculé")
                } else {
                    (StatusCode::UNAUTHORIZED, "refusé")
                }
            };
            let onboard = |headers: axum::http::HeaderMap| async move {
                let given = headers.get("x-onboard-token").and_then(|v| v.to_str().ok());
                if token_matches(Some(&Secret::new(TOKEN)), given) {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
            };
            Router::new()
                .route("/accounts", get(accounts))
                .route("/accounts/page", get(page))
                .route("/accounts/premium", post(toggle))
                .route("/onboard", post(onboard))
                .route("/admin/run", post(|| async { "passage lancé" }))
                .route("/premium", get(|| async { "page publique" }))
                .route("/whoami", get(whoami))
                .route("/connexion", get(login_get).post(login_post))
                .with_state(auth.clone())
                .layer(axum::middleware::from_fn_with_state(auth, layer))
        }

        fn req(method: Method, path: &str, host: &str, peer: &str, xff: Option<&str>) -> Request {
            let mut b = Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, host);
            if let Some(x) = xff {
                b = b.header("x-forwarded-for", x);
            }
            let mut r = b.body(Body::empty()).unwrap();
            let peer: SocketAddr = format!("{peer}:40000").parse().unwrap();
            r.extensions_mut().insert(ConnectInfo(peer));
            r
        }

        async fn send(app: &Router, r: Request) -> (StatusCode, bool, String) {
            let resp = app.clone().oneshot(r).await.unwrap();
            let status = resp.status();
            let cookie = resp.headers().contains_key(header::SET_COOKIE);
            let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (status, cookie, String::from_utf8_lossy(&body).into_owned())
        }

        #[tokio::test]
        async fn forged_home_ip_from_the_host_gets_nothing() {
            let app = app();
            // `curl -H 'X-Forwarded-For: <IP de la maison>' http://127.0.0.1:8766/accounts`
            let (st, cookie, _) = send(
                &app,
                req(
                    Method::GET,
                    "/accounts",
                    "127.0.0.1:8766",
                    "127.0.0.1",
                    Some(HOME),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
            assert!(!cookie, "aucune session");
            // forme « client, IP de confiance » : pareil
            let (st, cookie, _) = send(
                &app,
                req(
                    Method::GET,
                    "/accounts",
                    "127.0.0.1:8766",
                    "127.0.0.1",
                    Some(&format!("192.0.2.7, {HOME}")),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
            assert!(!cookie);
        }

        #[tokio::test]
        async fn home_ip_through_npm_still_opens_the_session() {
            let app = app();
            let (st, cookie, body) = send(
                &app,
                req(
                    Method::GET,
                    "/accounts",
                    ONBOARD,
                    NPM,
                    Some(&format!("192.0.2.7, {HOME}")),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{body}");
            assert!(cookie, "cookie posé au passage");
            assert_eq!(body, "comptes");
        }

        #[tokio::test]
        async fn client_address_is_the_npm_hop_only_from_npm() {
            let app = app();
            let (_, _, b) = send(
                &app,
                req(
                    Method::GET,
                    "/whoami",
                    PREMIUM,
                    NPM,
                    Some("1.1.1.1, 198.51.100.7"),
                ),
            )
            .await;
            assert_eq!(b, "198.51.100.7|true|false");
            let (_, _, b) = send(
                &app,
                req(
                    Method::GET,
                    "/whoami",
                    "127.0.0.1:8766",
                    "127.0.0.1",
                    Some("198.51.100.7"),
                ),
            )
            .await;
            assert_eq!(b, "127.0.0.1|false|false");
            let (_, _, b) = send(
                &app,
                req(Method::GET, "/whoami", "127.0.0.1:8766", "127.0.0.1", None),
            )
            .await;
            assert_eq!(b, "127.0.0.1|false|true");
            // hors du réseau Docker : l'en-tête est ignoré
            let (_, _, b) = send(
                &app,
                req(Method::GET, "/whoami", PREMIUM, "10.9.8.7", Some(HOME)),
            )
            .await;
            assert_eq!(b, "10.9.8.7|false|false");
        }

        #[tokio::test]
        async fn cli_routes_answer_local_calls_only() {
            let app = app();
            let (st, _, b) = send(
                &app,
                req(
                    Method::POST,
                    "/admin/run",
                    "127.0.0.1:8766",
                    "127.0.0.1",
                    None,
                ),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(b, "passage lancé");
            // à travers NPM, sur l'hôte d'onboarding comme sur l'hôte premium
            for host in [ONBOARD, PREMIUM] {
                let (st, _, _) =
                    send(&app, req(Method::POST, "/admin/run", host, NPM, Some(HOME))).await;
                assert_eq!(st, StatusCode::NOT_FOUND, "{host}");
            }
            // un autre conteneur qui vise la passerelle
            let (st, _, _) = send(
                &app,
                req(
                    Method::POST,
                    "/admin/run",
                    "172.18.0.1:8766",
                    "172.18.0.5",
                    None,
                ),
            )
            .await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            // local mais relayé
            let (st, _, _) = send(
                &app,
                req(
                    Method::POST,
                    "/admin/run",
                    "127.0.0.1:8766",
                    "127.0.0.1",
                    Some("198.51.100.7"),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn admin_paths_are_hidden_on_other_hosts() {
            let app = app();
            for path in ["/accounts", "/connexion"] {
                let (st, cookie, _) =
                    send(&app, req(Method::GET, path, PREMIUM, NPM, Some(HOME))).await;
                assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
                assert!(!cookie, "{path}");
            }
            // l'hôte d'onboarding sert le formulaire de connexion
            let (st, _, b) = send(
                &app,
                req(
                    Method::GET,
                    "/connexion",
                    ONBOARD,
                    NPM,
                    Some("198.51.100.7"),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert!(b.contains("Jeton d'accès"));
            // les pages publiques restent servies sur l'hôte premium
            let (st, _, b) = send(
                &app,
                req(Method::GET, "/premium", PREMIUM, NPM, Some("198.51.100.7")),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(b, "page publique");
            // sans en-tête Host : refusé
            let mut r = req(Method::GET, "/accounts", ONBOARD, NPM, Some(HOME));
            r.headers_mut().remove(header::HOST);
            assert_eq!(send(&app, r).await.0, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn token_login_always_works_through_npm() {
            let app = app();
            let mut r = req(
                Method::POST,
                "/connexion",
                ONBOARD,
                NPM,
                Some("198.51.100.7"),
            );
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            *r.body_mut() = Body::from(format!("token={TOKEN}&next=%2Faccounts"));
            let resp = app.clone().oneshot(r).await.unwrap();
            assert_eq!(resp.status(), StatusCode::SEE_OTHER);
            let cookie = resp
                .headers()
                .get(header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            assert!(cookie.starts_with("gc_admin=a."), "session admin");
            // le cookie ouvre la page depuis n'importe quelle adresse
            let value = cookie.split(';').next().unwrap().to_string();
            let mut r = req(Method::GET, "/accounts", ONBOARD, NPM, Some("192.0.2.50"));
            r.headers_mut()
                .insert(header::COOKIE, HeaderValue::from_str(&value).unwrap());
            let (st, _, b) = send(&app, r).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(b, "comptes");
        }

        #[tokio::test]
        async fn the_token_never_reaches_a_page_and_forms_still_work() {
            let app = app();
            // IP de la maison : session d'office
            let (st, _, page) = send(
                &app,
                req(Method::GET, "/accounts/page", ONBOARD, NPM, Some(HOME)),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert!(
                !page.contains(TOKEN),
                "jeton en clair dans la page : {page}"
            );
            let form = form_token(TOKEN);
            assert!(page.contains(&form));
            // le formulaire renvoie le jeton de formulaire : la page reçoit le vrai jeton
            let post = |xff: &str, body: String| {
                let mut r = req(Method::POST, "/accounts/premium", ONBOARD, NPM, Some(xff));
                r.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                *r.body_mut() = Body::from(body);
                r
            };
            let (st, _, b) = send(&app, post(HOME, format!("token={form}&user_id=1"))).await;
            assert_eq!(st, StatusCode::OK, "{b}");
            assert_eq!(b, "basculé");
            // sans session, le jeton de formulaire ne vaut rien
            let (st, _, _) = send(
                &app,
                post("198.51.100.7", format!("token={form}&user_id=1")),
            )
            .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn failed_posts_without_session_are_capped_per_address() {
            let app = app();
            let post = |ip: &'static str| {
                let mut r = req(Method::POST, "/onboard", ONBOARD, NPM, Some(ip));
                r.headers_mut()
                    .insert("x-onboard-token", HeaderValue::from_static("faux"));
                r
            };
            for _ in 0..MAX_FAILS {
                assert_eq!(
                    send(&app, post("198.51.100.7")).await.0,
                    StatusCode::UNAUTHORIZED
                );
            }
            let (st, _, b) = send(&app, post("198.51.100.7")).await;
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
            assert!(
                b.contains("\"success\":false"),
                "JSON pour la page de création"
            );
            // une autre adresse n'est pas bloquée
            assert_eq!(
                send(&app, post("198.51.100.8")).await.0,
                StatusCode::UNAUTHORIZED
            );
            // la CLI non plus (sa propre limite), avec le bon jeton
            let mut cli = req(
                Method::POST,
                "/onboard",
                "127.0.0.1:8766",
                "127.0.0.1",
                None,
            );
            cli.headers_mut()
                .insert("x-onboard-token", HeaderValue::from_static(TOKEN));
            assert_eq!(send(&app, cli).await.0, StatusCode::OK);
        }
    }
}
