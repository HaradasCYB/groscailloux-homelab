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
//! - Échecs limités par adresse (`MAX_FAILS` par `FAIL_WINDOW`) et au total.
//! - IP de la maison (`HOMELABD_ADMIN_TRUSTED_IPS`) : session admin d'office, sans formulaire, et cookie posé au
//!   passage (si l'IP de la box change, le navigateur reste connecté). Demandé le 2026-09-23 : la connexion gênait
//!   la gestion depuis Homarr (iframe État comprise).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use homelab_core::html::esc;
use homelab_core::{Secret, TaskContext};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{info, warn};

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

#[derive(Clone)]
pub struct AdminAuth {
    ctx: Arc<TaskContext>,
    fails: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl AdminAuth {
    pub fn new(ctx: Arc<TaskContext>) -> Self {
        Self {
            ctx,
            fails: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn token(&self, scope: Scope) -> Option<&Secret> {
        match scope {
            Scope::Admin => self.ctx.secrets.onboard_token.as_ref(),
            Scope::Status => self.ctx.secrets.status_token.as_ref(),
        }
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
    async fn blocked(&self, ip: &str) -> bool {
        let mut f = self.fails.lock().await;
        let cutoff = Instant::now() - FAIL_WINDOW;
        f.retain(|_, v| {
            v.retain(|t| *t > cutoff);
            !v.is_empty()
        });
        let total: usize = f.values().map(Vec::len).sum();
        f.get(ip).map(Vec::len).unwrap_or(0) >= MAX_FAILS || total >= MAX_FAILS_TOTAL
    }

    async fn fail(&self, ip: &str) {
        self.fails
            .lock()
            .await
            .entry(ip.to_string())
            .or_default()
            .push(Instant::now());
        warn!(task = "admin", ip, "admin login: wrong token");
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

/// L'adresse (dernier saut de `X-Forwarded-For`, posé par NPM) est-elle une adresse de confiance ? Une requête sans
/// cet en-tête (`local`) ne l'est jamais.
fn trusted(ip: &str, list: &[String]) -> bool {
    ip != "local" && list.iter().any(|t| t == ip)
}

/// Adresse du client : le **dernier** élément de `X-Forwarded-For` (celui que NPM ajoute ; les précédents viennent
/// du client et se falsifient).
fn client_ip(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into())
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
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<LoginForm>,
) -> Response {
    let ip = client_ip(&headers);
    let next = safe_next(&form.next);
    if auth.blocked(&ip).await {
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
            auth.fail(&ip).await;
            login_page(&next, "Jeton incorrect.", StatusCode::UNAUTHORIZED)
        }
    }
}

/// Couche posée sur toute l'application.
pub async fn layer(State(auth): State<AdminAuth>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let Some(need) = required(&path) else {
        return next.run(req).await;
    };
    // `/status*` sans jeton d'état configuré : ouvert (comportement d'origine).
    if need == Scope::Status && auth.token(Scope::Status).is_none() {
        return next.run(req).await;
    }
    let query = req.uri().query().unwrap_or("").to_string();
    let (given, rest) = split_token(&query);
    let ip = client_ip(req.headers());
    let is_get = req.method() == Method::GET || req.method() == Method::HEAD;

    // Ancien lien avec `?token=` : ouvre la session et renvoie vers l'adresse sans jeton.
    if let (Some(given), true) = (given.as_deref(), is_get) {
        if auth.blocked(&ip).await {
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
                auth.fail(&ip).await;
                login_page(
                    &with_query(&path, &rest),
                    "Jeton incorrect.",
                    StatusCode::UNAUTHORIZED,
                )
            }
        };
    }

    let cookie = cookie_of(&req).and_then(|c| auth.verify(&c, now()));
    let from_home = trusted(&ip, &auth.ctx.secrets.admin_trusted_ips);
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
            }
            let mut resp = next.run(req).await;
            strip_location(&mut resp);
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(c) = new_cookie {
                resp.headers_mut().append(header::SET_COOKIE, c);
            }
            resp
        }
        // Pas de session : une page s'ouvre sur le formulaire ; un POST garde son propre contrôle (jeton du
        // formulaire ou de l'en-tête, pour la CLI).
        _ if is_get => login_page(&with_query(&path, &rest), "", StatusCode::UNAUTHORIZED),
        _ => {
            let mut resp = next.run(req).await;
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

    #[test]
    fn trusted_ips() {
        let list = vec!["203.0.113.9".to_string()];
        assert!(trusted("203.0.113.9", &list));
        assert!(!trusted("203.0.113.10", &list));
        assert!(!trusted("local", &["local".to_string()]));
        assert!(!trusted("203.0.113.9", &[]));
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }
}
