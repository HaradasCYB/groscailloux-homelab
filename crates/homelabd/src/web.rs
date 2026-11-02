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
use homelab_core::chat;
use homelab_core::config::Donation;
use homelab_core::mail;
use homelab_core::mail_samples;
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
use crate::{accounts_page, guide, status_page, ui};
use homelab_core::html::esc;

/// Scripts en ligne des pages de bienvenue (`/bienvenue`, `/premiers-pas`) et de l'onboarding admin (`GET /`).
const WELCOME_JS: &str = include_str!("../assets/bienvenue.js");
const ONBOARD_ADMIN_JS: &str = include_str!("../assets/onboard-admin.js");
/// Bouton PayPal de `/premium` (marque `{{PLAN_ID}}`) ; lit `data-deja` sur `#premium`.
const PREMIUM_JS: &str = include_str!("../assets/premium.js");
/// Messages de `/premium/activate` (paramètres `ok`, `err`, `sub`, `compte` de l'adresse).
const PREMIUM_ACTIVATE_JS: &str = include_str!("../assets/premium-activate.js");

/// Règles propres aux pages Premium (sur [`ui::BASE_CSS`]) : carte étroite en grille, avatar (`/avatar.jpg`,
/// servi par NPM), prix, liste des avantages, état « déjà abonné ». Le cadre du bouton PayPal reste en
/// `color-scheme:light` : sinon le navigateur peint un fond opaque derrière l'iframe (schémas différents).
const PREMIUM_CSS: &str = r#".gc-premium{display:grid;gap:18px}
.gc-premium p,.gc-premium form,.gc-premium ul{margin:0}
.gc-premium h1{margin:0;font-size:clamp(1.75rem,1.3rem + 1.8vw,2.25rem);text-wrap:balance}
.gc-premium .lead{font-size:1.0625rem;color:var(--text)}
.gc-premium .muted{font-size:.9375rem}
.avatar{width:88px;height:88px;border-radius:50%;object-fit:cover;display:block;padding:3px;border:2px solid var(--accent);background:var(--bg)}
.price{justify-self:start;padding:6px 14px;border-radius:999px;background:var(--accent-fill);color:var(--strong);font-weight:700;font-size:1rem;font-variant-numeric:tabular-nums}
.facts{padding:14px 16px;border-radius:var(--r-md);background:var(--raised);display:grid;gap:8px;list-style:none;font-size:.9375rem}
.facts li{position:relative;padding-left:17px}
.facts li::before{content:"";position:absolute;left:0;top:.62em;width:7px;height:7px;border-radius:50%;background:var(--accent)}
.gc-premium .notice{margin:0}
.gc-premium .field{margin:0}
.gc-premium .field label{margin-bottom:6px}
.gc-premium input:invalid:not(:placeholder-shown){border-color:var(--err)}
.gc-premium form:not(.who){display:grid;gap:14px}
#paypal-button-container{min-height:54px}
#paypal-button-container,#paypal-button-container iframe{color-scheme:light}
.fallback{font-size:.9375rem;color:var(--dim)}
.gc-premium footer{border-top:1px solid var(--line);padding-top:14px}
#premium:not([data-deja="1"]) .deja-only,
#premium[data-deja="1"] #who,#premium[data-deja="1"] #paypal-button-container,#premium[data-deja="1"] .fallback{display:none}"#;

/// Avatar de la page (le fichier est servi par NPM sur l'hôte de `/premium`, pas par homelabd).
const PREMIUM_AVATAR: &str =
    r#"<img class="avatar" src="/avatar.jpg" alt="Homeflix GrosCailloux" width="88" height="88">"#;

/// Corps de `/premium` : marques `{{DEJA}}`, `{{PRICE}}`, `{{COMPTE}}`, `{{SUBSCRIBE_URL}}`, `{{SDK_HOST}}`,
/// `{{CLIENT_ID}}`, `{{PLAN_ID}}` (dans [`PREMIUM_JS`]) remplacées par [`premium_page`].
const PREMIUM_BODY: &str = r#"<div class="gc-narrow" id="premium" data-deja="{{DEJA}}"><div class="card gc-premium">
  {{AVATAR}}

  <h1>Homeflix GrosCailloux Premium</h1>

  <p class="price">{{PRICE}}</p>

  <p class="lead">Abonne-toi et active le compte <strong>Premium</strong> de ton Homeflix GrosCailloux
  pour profiter de tout le catalogue, où que tu sois, sans limite de durée de lecture.</p>

  <ul class="facts">
    <li>{{PRICE}}, sans engagement.</li>
    <li>Activation <strong>immédiate</strong> dès le paiement : indique ton nom de compte ci-dessous.</li>
    <li>Jusqu'à 2 lectures simultanées et catalogue complet.</li>
    <li>Résiliable à tout moment depuis ton compte PayPal.</li>
  </ul>

  <p class="notice info deja deja-only" role="status">Le compte <strong>{{COMPTE}}</strong> a déjà un abonnement PayPal actif,
  qui se renouvelle tout seul chaque mois : inutile de t'abonner une deuxième fois (ce serait deux prélèvements).
  Pour le consulter ou l'arrêter : ton compte PayPal → Paiements automatiques. Si un paiement a échoué, mets à jour
  ton moyen de paiement dans PayPal : le prélèvement sera retenté.</p>
  <p class="muted deja-only"><a href="/premium">S'abonner pour un autre compte</a></p>

  <form id="who" class="who field" onsubmit="return false" autocomplete="off">
    <label for="compte">Ton nom de compte Groscailloux</label>
    <input id="compte" name="compte" type="text" value="{{COMPTE}}" maxlength="32" autocapitalize="none" spellcheck="false"
      pattern="[a-zA-Z0-9_\-]{2,32}" placeholder="Ex. : john_doe" aria-describedby="who-hint" required>
    <p class="hint" id="who-hint">Celui avec lequel tu te connectes à Jellyfin. Il sera activé (ou prolongé) tout seul après le paiement.</p>
  </form>

  <div id="paypal-button-container" aria-live="polite"></div>
  <form id="link" method="post" action="/premium/lier" hidden>
    <input type="hidden" name="subscription_id" id="sub_id"><input type="hidden" name="compte" id="sub_compte">
  </form>
  <p class="muted" id="linking" role="status" hidden>Paiement approuvé, activation en cours…</p>

  <p class="fallback">Le bouton ne s'affiche pas ?
    <a href="{{SUBSCRIBE_URL}}" rel="noopener noreferrer" target="_blank">S'abonner directement sur PayPal</a>,
    puis <a href="/premium/activate">indique ton compte ici</a>.</p>

  <footer><p class="muted">Paiement sécurisé par PayPal. Activation automatique dès confirmation du paiement ; sans engagement, résiliable depuis ton compte PayPal.</p></footer>
</div></div>
<script src="{{SDK_HOST}}/sdk/js?client-id={{CLIENT_ID}}&vault=true&intent=subscription&locale=fr_FR" data-sdk-integration-source="button-factory"></script>
<script>
{{JS}}</script>"#;

/// Corps de `/premium/activate` (statique ; les messages viennent de [`PREMIUM_ACTIVATE_JS`]).
const PREMIUM_ACTIVATE_BODY: &str = r#"<div class="gc-narrow"><div class="card gc-premium">
  {{AVATAR}}

  <h1>Rattacher ton abonnement</h1>

  <p class="lead">Tu as payé sur PayPal sans passer par notre page, ou l'activation automatique n'a pas
  abouti&nbsp;? Indique ton <strong>nom de compte</strong> et l'<strong>identifiant de l'abonnement</strong>
  (il commence par <code>I-</code>, dans le mail de confirmation PayPal ou dans PayPal → Paiements automatiques).
  L'activation est immédiate.</p>

  <form method="post" action="/premium/lier">
    <div class="field">
      <label for="compte">Nom exact du compte</label>
      <input id="compte" name="compte" type="text" required autocomplete="off" autocapitalize="none"
        spellcheck="false" placeholder="Ex. : john_doe" maxlength="32">
    </div>
    <div class="field">
      <label for="subscription_id">Identifiant de l'abonnement PayPal</label>
      <input id="subscription_id" name="subscription_id" type="text" required autocomplete="off" autocapitalize="characters"
        spellcheck="false" placeholder="I-XXXXXXXXXXXX" maxlength="24" pattern="I-[A-Za-z0-9]{10,22}">
    </div>
    <button type="submit" class="block">Activer mon compte</button>
  </form>

  <p class="muted">Pas d'identifiant sous la main&nbsp;? Envoie une demande à l'administrateur avec ton nom de compte&nbsp;:</p>
  <form method="post" action="/premium/activate">
    <div class="field">
      <label for="name">Nom exact du compte</label>
      <input id="name" name="name" type="text" required autocomplete="off" autocapitalize="none"
        spellcheck="false" placeholder="Ex. : john_doe" maxlength="64">
    </div>
    <button type="submit" class="secondary block">Demander l'activation à l'administrateur</button>
  </form>

  <p class="notice ok" id="ok" role="status" hidden>Demande envoyée&nbsp;! L'administrateur activera ton compte après vérification.</p>
  <p class="notice err" id="err" role="alert" hidden></p>

  <footer>
    <p class="muted"><a href="/premium">← Retour à la page d'abonnement</a></p>
  </footer>
</div></div>
<script>
{{JS}}</script>"#;

/// Corps de `/premium/merci` : marques `{{COMPTE}}`, `{{JUSQU}}`, `{{JELLYFIN_URL}}` (déjà échappées).
const PREMIUM_MERCI_BODY: &str = r#"<div class="gc-narrow"><div class="card gc-premium">
  {{AVATAR}}
  <h1>Merci, c'est activé&nbsp;!</h1>
  <p class="notice ok" role="status">Le compte <strong>{{COMPTE}}</strong> est Premium{{JUSQU}}.</p>
  <p>L'abonnement se renouvelle tout seul chaque mois et ton compte se prolonge à chaque paiement. Tu peux tout suivre dans « Mon compte » sur Jellyfin (icône en haut à droite), et résilier à tout moment depuis ton compte PayPal.</p>
  <p><a class="btn block-sm" href="{{JELLYFIN_URL}}">Regarder maintenant</a></p>
  <p class="muted">Un mail de confirmation part sur l'adresse de ton compte. Une question ? Réponds-y.</p>
</div></div>"#;

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
    let red_list = crate::red_list_page::router(ctx.clone());
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
        .route(crate::ui::LOGO_PATH, get(crate::ui::logo))
        .route("/bienvenue/renouveler", post(welcome_renew))
        .route("/premiers-pas", get(first_steps_page))
        .route("/bienvenue/{token}", get(welcome_get).post(welcome_post))
        .route("/inscription", get(signup_get).post(signup_post))
        .route("/accounts/link", post(accounts_link))
        .route("/admin/link", post(admin_link))
        .route("/admin/mail-test", post(admin_mail_test))
        .route("/admin/mail-test/announce", post(admin_mail_test_announce))
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
        .merge(red_list)
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

async fn index() -> Html<String> {
    Html(index_page())
}

/// Règles propres à l'onboarding admin (résultat, identifiants, notification « Copié »), sur [`ui::BASE_CSS`].
const INDEX_CSS: &str = r#".ob form{margin-top:8px}
.spinner{display:none;width:16px;height:16px;border:2px solid rgba(11,11,12,.25);border-top-color:var(--btn-text);border-radius:50%;animation:gc-spin .8s linear infinite}
button.loading .spinner{display:inline-block}
@keyframes gc-spin{to{transform:rotate(360deg)}}
.result{display:none;margin-top:24px;padding:20px;background:var(--bg);border:1px solid var(--line);border-left:4px solid var(--line);border-radius:var(--r-md)}
.result.visible{display:block}
.result.success{border-left-color:var(--ok)}
.result.error{border-left-color:var(--err)}
.result-title{margin:0 0 14px;font-size:1.05rem}
.result.success .result-title{color:var(--ok)}
.result.error .result-title{color:#ff8a8a}
.creds{background:var(--raised);border-radius:var(--r-md);padding:14px 16px;font:.875rem/1.7 var(--mono);overflow-wrap:anywhere}
.creds .key{color:var(--accent-hover)}
.creds .pwd{color:var(--strong);font-weight:700}
.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:16px}
.actions button{flex:1 1 10rem}
.error-msg{color:#ff8a8a;margin:0}
.note{font-size:.875rem;color:var(--dim);margin:12px 0 0}
.footer{margin:24px 0 0;text-align:center;font-size:.8125rem;color:var(--dim)}
.toast{position:fixed;left:50%;bottom:24px;z-index:100;transform:translate(-50%,120px);visibility:hidden;max-width:calc(100% - 32px);background:var(--raised);color:var(--text);border:1px solid var(--line);border-left:4px solid var(--ok);border-radius:var(--r-md);padding:10px 16px;font-weight:600;transition:transform .2s ease,visibility .2s}
.toast.visible{transform:translate(-50%,0);visibility:visible}"#;

/// Onboarding admin (`GET /`, derrière la session d'administration) : formulaire, résultat et script
/// `assets/onboard-admin.js` (identifiants `onboardForm`, `submitBtn`, `result`, `toast`, `username`, `email`).
fn index_page() -> String {
    let body = format!(
        r#"<div class="gc-narrow"><section class="card ob" aria-labelledby="ob-title">
<h1 id="ob-title">Nouveau compte</h1>
<p class="lead">Créer le compte d'un nouveau membre</p>
<form id="onboardForm">
<div class="field"><label for="username">Identifiant</label><input type="text" id="username" name="username" placeholder="ex. : hippo" required pattern="[a-zA-Z0-9_-]{{2,32}}" autocomplete="off" autofocus></div>
<div class="field"><label for="email">Adresse e-mail</label><input type="email" id="email" name="email" placeholder="hippo@example.com" required autocomplete="off"></div>
<button type="submit" id="submitBtn" class="block"><span class="spinner" aria-hidden="true"></span><span class="label">Créer le compte</span></button>
</form>
<div id="result" class="result" aria-live="polite"></div>
<p class="footer">Jellyfin · Jellyseerr · mail de bienvenue · homelabd</p>
</section></div>
<div id="toast" class="toast" role="status" aria-live="polite">Copié</div>
<script>
{js}</script>"#,
        js = ONBOARD_ADMIN_JS
    );
    ui::page_admin("Groscailloux — Nouveau compte", None, &body, INDEX_CSS)
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
    let body = PREMIUM_BODY
        .replace("{{AVATAR}}", PREMIUM_AVATAR)
        .replace("{{JS}}", PREMIUM_JS)
        .replace("{{CLIENT_ID}}", client_id)
        .replace("{{PLAN_ID}}", plan_id)
        .replace("{{SDK_HOST}}", host)
        .replace(
            "{{SUBSCRIBE_URL}}",
            &format!("{host}/webapps/billing/plans/subscribe?plan_id={plan_id}"),
        )
        .replace("{{COMPTE}}", &esc(compte))
        .replace("{{PRICE}}", &esc(price))
        .replace("{{DEJA}}", if deja { "1" } else { "0" });
    ui::page_described(
        "Homeflix GrosCailloux — Abonnement Premium",
        "Active le compte Premium de ton Homeflix GrosCailloux pour 3,50 € par mois.",
        &body,
        PREMIUM_CSS,
    )
}

/// Page `/premium/activate` (statique).
fn premium_activate_page() -> String {
    let body = PREMIUM_ACTIVATE_BODY
        .replace("{{AVATAR}}", PREMIUM_AVATAR)
        .replace("{{JS}}", PREMIUM_ACTIVATE_JS);
    ui::page_described(
        "Homeflix GrosCailloux — Activation Premium",
        "Indique le nom exact de ton compte pour activer ton abonnement Premium.",
        &body,
        PREMIUM_CSS,
    )
}

/// Page `/premium/merci` : `compte` et `jusqu` (date affichée) viennent de l'adresse et sont échappés ici.
fn premium_merci_page(compte: &str, jusqu: &str, jellyfin_url: &str) -> String {
    let jusqu = if jusqu.is_empty() {
        String::new()
    } else {
        format!(" jusqu'au {}", esc(jusqu))
    };
    let body = PREMIUM_MERCI_BODY
        .replace("{{AVATAR}}", PREMIUM_AVATAR)
        .replace("{{COMPTE}}", &esc(compte))
        .replace("{{JUSQU}}", &jusqu)
        .replace("{{JELLYFIN_URL}}", &esc(jellyfin_url));
    ui::page("Homeflix GrosCailloux — Merci", &body, PREMIUM_CSS)
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
    let jusqu = q.get("jusqu").map(String::as_str).unwrap_or("");
    let html = premium_merci_page(compte, jusqu, &st.ctx.secrets.jellyfin_public_url);
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
    let mut resp = Html(premium_activate_page()).into_response();
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
            Html(crate::ui::error_page(
                "Accès refusé",
                "token manquant ou invalide",
                None,
            )),
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
    crate::ui::error_response(
        StatusCode::UNAUTHORIZED,
        "Session expirée",
        "Session expirée : <a href=\"/connexion\">se reconnecter</a>.",
        None,
    )
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
            // lien « Réessayer » = la même adresse (href vide), donc la même requête
            return crate::ui::error_response(
                StatusCode::BAD_GATEWAY,
                "Jellyfin injoignable",
                "Jellyfin injoignable, réessayer dans un instant.",
                Some(("", "Réessayer")),
            );
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

/// Familles de pages membres : chacune garde le titre d'onglet de son ancien gabarit ; seules les pages de
/// bienvenue portent le script `assets/bienvenue.js` (copie de l'adresse, contrôle du mot de passe).
#[derive(Clone, Copy)]
enum Public {
    /// `/bienvenue/…` (mot de passe, accès, lien expiré, renouvellement) et `/premiers-pas`.
    Welcome,
    /// `/inscription` (formulaire et réponse).
    Signup,
}

/// Règles propres aux pages membres (sur [`ui::BASE_CSS`]) : carte étroite centrée, case à cocher, champ
/// leurre, identifiant, premiers pas dépliables.
const PUBLIC_CSS: &str = r#".gc-public h1{margin-bottom:.4em}
.gc-public form{margin-top:8px}
.gc-public form .btn,.links .btn{width:100%}
.dim{color:var(--dim);font-size:.9375rem}
.opt{font-weight:400;color:var(--dim)}
.field+.hint{margin-top:-8px}
label.check{display:flex;gap:10px;align-items:flex-start;font-weight:400;font-size:.9375rem;line-height:1.45;color:var(--dim);margin:0 0 20px}
label.check input{flex:none;width:20px;height:20px;margin:1px 0 0}
.hp{position:absolute;left:-9999px;width:1px;height:1px;overflow:hidden;opacity:0}
.links{display:grid;gap:10px;margin:18px 0 6px}
.who{font:1rem/1.4 var(--mono);background:var(--raised);border:1px solid var(--line);border-radius:var(--r-md);padding:10px 12px;margin:0 0 4px;overflow-wrap:anywhere}
.foot{margin:22px 0 0;font-size:.875rem;color:var(--dim)}
.gc-narrow:has(.steps){max-width:40rem}
.steps{display:grid;gap:20px;margin-top:28px}
.step{display:grid;grid-template-columns:32px 1fr;gap:14px;align-items:start}
.step .n{display:grid;place-items:center;width:30px;height:30px;border-radius:50%;font-weight:700;font-size:.875rem;color:var(--strong);background:rgba(47,143,255,.14);border:1px solid var(--accent)}
.step h2{margin:3px 0 6px;font-size:1.125rem;letter-spacing:-.015em}
.step p{font-size:.9375rem;margin-bottom:8px}
.sc{min-width:0}
.addr{display:flex;gap:8px;align-items:stretch;margin:0 0 12px}
.addr code{display:flex;align-items:center;flex:1;min-width:0;overflow-wrap:anywhere;font-size:.875rem;background:var(--raised);border:1px solid var(--line);border-radius:var(--r-md);padding:9px 12px}
.addr .copy{flex:none;padding:8px 14px;font-size:.875rem}
details{border:1px solid var(--line);border-radius:var(--r-md);background:var(--bg);margin:0 0 8px}
details summary{cursor:pointer;padding:11px 14px;font-weight:600;font-size:.9375rem}
details summary:hover{color:var(--strong)}
details[open] summary{border-bottom:1px solid var(--line)}
details p{padding:10px 14px 0}
details p:last-child{padding-bottom:10px}
.qc{color:var(--dim)}
@media (max-width:420px){.step{grid-template-columns:1fr;gap:6px}}"#;

/// Page membre complète (fonction pure) : `heading` est échappé, `body` est du HTML déjà échappé.
fn public_html(kind: Public, heading: &str, body: &str) -> String {
    let (title, script) = match kind {
        Public::Welcome => ("Bienvenue sur Groscailloux", Some(WELCOME_JS)),
        Public::Signup => ("Créer un compte Groscailloux", None),
    };
    let mut main = format!(
        "<div class=\"gc-narrow\"><div class=\"card gc-public\">\n<h1>{}</h1>\n{body}\n</div></div>",
        esc(heading)
    );
    if let Some(js) = script {
        main.push_str("\n<script>\n");
        main.push_str(js);
        main.push_str("</script>");
    }
    ui::page(title, &main, PUBLIC_CSS)
}

fn public_page(kind: Public, heading: &str, body: &str, status: StatusCode) -> Response {
    let mut resp = (status, Html(public_html(kind, heading, body))).into_response();
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
    let host = esc(&jellyfin_host(s));
    let jf = esc(&s.jellyfin_public_url);
    let js = esc(&s.jellyseerr_public_url);
    let guide = s
        .onboard_public_url
        .as_deref()
        .map(|u| format!("{}/guide", esc(u.trim_end_matches('/'))))
        .unwrap_or_else(|| "/guide".into());
    let discord = s
        .guide_contact_discord
        .as_deref()
        .map(|d| format!(" Sur Discord, écris à <b>{}</b>.", esc(d)))
        .unwrap_or_default();
    let quick = r#"<p class="qc"><b>Connexion sans clavier :</b> sur la télé, choisis <b>Quick Connect</b>. Un code s'affiche ; sur ton téléphone déjà connecté, touche ton profil (en haut à droite) → <b>Quick Connect</b> → saisis le code.</p>"#;
    format!(
        r#"<section class="steps" aria-label="Premiers pas">
<div class="step"><div class="n">1</div><div class="sc"><h2>Ton écran</h2>
<p class="dim">Adresse du serveur, à saisir une seule fois dans l'appli :</p>
<div class="addr"><code id="gc-host">{host}</code><button type="button" class="btn secondary copy" data-copy="{host}">Copier</button></div>
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
<div class="links"><a class="btn" href="{jf}">Regarder : ouvrir Groscailloux</a><a class="btn secondary" href="{js}">Demander un film ou une série</a></div>
{steps}"#,
        user = esc(username),
        jf = esc(&s.jellyfin_public_url),
        js = esc(&s.jellyseerr_public_url),
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
        jf = esc(&s.jellyfin_public_url),
        steps = first_steps(s),
    );
    public_page(Public::Welcome, "Premiers pas", &body, StatusCode::OK)
}

fn password_form(username: &str, token: &str, demo: bool) -> String {
    format!(
        r#"<form id="pw-form" method="post" action="/bienvenue/{token}" data-username="{user}" autocomplete="off">
<div class="field"><label for="pw">Choisis ton mot de passe</label><input id="pw" name="pw" type="password" minlength="8" maxlength="128" required autocomplete="new-password" autofocus{dis}></div>
<div class="field"><label for="pw2">Confirme-le</label><input id="pw2" name="pw2" type="password" minlength="8" maxlength="128" required autocomplete="new-password"{dis}></div>
<p class="hint">8 caractères minimum, sans ton pseudo dedans. Il servira pour regarder et pour demander des titres.</p>
<div id="pw-err" class="notice err" aria-live="polite" hidden></div>
<button id="pw-btn" class="btn" type="submit" disabled>Enregistrer mon mot de passe</button>
</form>"#,
        token = esc(token),
        user = esc(username),
        dis = if demo { " disabled" } else { "" },
    )
}

fn expired_page() -> Response {
    public_page(
        Public::Welcome,
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
        user = esc(&link.username),
        form = password_form(&link.username, &token, demo),
    );
    public_page(
        Public::Welcome,
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
            r#"<div class="notice err" role="alert">{msg}</div>{form}"#,
            form = password_form(&link.username, &token, false)
        );
        return public_page(
            Public::Welcome,
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
        return public_page(Public::Welcome, "C'est prêt !", &body, StatusCode::OK);
    }
    if st.ctx.dry_run {
        return public_page(
            Public::Welcome,
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
            r#"<div class="notice err" role="alert">Impossible d'enregistrer le mot de passe pour l'instant. Réessaie dans un moment, ou écris à l'administrateur.</div>{}"#,
            password_form(&link.username, &token, false)
        );
        return public_page(
            Public::Welcome,
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
        Public::Welcome,
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
            Public::Welcome,
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

fn signup_form(trial: Option<u32>, username: &str, email: &str, err: Option<&str>) -> String {
    signup_form_p(trial, username, email, "", err)
}

/// Ce que le formulaire annonce : essai configuré (`trial` = `[subscriptions] trial_days` si les abonnements sont
/// en service) ou validation par l'administrateur. Phrase d'introduction et case à cocher.
fn signup_promise(trial: Option<u32>) -> (String, &'static str) {
    match trial {
        Some(days) => (
            format!("Ton compte démarre par un essai gratuit de {days} jours dès que ton mot de passe est choisi (s'il ne reste plus de place, l'administrateur l'active ensuite)."),
            "Je sais que l'administrateur peut suspendre un compte qu'il ne connaît pas.",
        ),
        None => (
            "Chaque compte est ensuite validé par l'administrateur.".to_string(),
            "Je sais que mon compte sera validé par l'administrateur avant de pouvoir regarder.",
        ),
    }
}

fn signup_form_p(
    trial: Option<u32>,
    username: &str,
    email: &str,
    parrain: &str,
    err: Option<&str>,
) -> String {
    let (intro, accept) = signup_promise(trial);
    format!(
        r#"{err}<p>Indique le pseudo que tu veux et ton adresse e-mail : tu recevras un lien pour choisir ton mot de passe. {intro}</p>
<form method="post" action="/inscription" autocomplete="off">
<div class="field"><label for="username">Pseudo</label><input id="username" name="username" type="text" required pattern="[a-zA-Z0-9_-]{{2,32}}" maxlength="32" value="{u}" autocomplete="username"></div>
<div class="field"><label for="email">Adresse e-mail</label><input id="email" name="email" type="email" required maxlength="120" value="{e}" autocomplete="email"></div>
<div class="field"><label for="parrain">Code de parrainage <span class="opt">(facultatif)</span></label><input id="parrain" name="parrain" type="text" maxlength="8" value="{p}" autocomplete="off" autocapitalize="characters" spellcheck="false" pattern="[A-Za-z0-9]{{8}}"></div>
<div class="hp" aria-hidden="true"><label for="website">Site web</label><input id="website" name="website" type="text" tabindex="-1" autocomplete="off"></div>
<label class="check"><input type="checkbox" name="accept" value="1" required> {accept}</label>
<button class="btn" type="submit">Créer mon compte</button>
</form>
<p class="foot">Groscailloux est un service privé entre amis : l'administrateur peut refuser un compte qu'il ne connaît pas.</p>"#,
        err = err
            .map(|m| format!(r#"<div class="notice err" role="alert">{}</div>"#, esc(m)))
            .unwrap_or_default(),
        u = esc(username),
        e = esc(email),
        p = esc(parrain),
    )
}

/// GET /inscription : page publique de création de compte.
async fn signup_get(State(st): State<AppState>) -> Response {
    if !st.ctx.cfg.onboard.public_signup {
        return StatusCode::NOT_FOUND.into_response();
    }
    let form_trial = configured_trial_days(&st.ctx);
    public_page(
        Public::Signup,
        "Créer ton compte",
        &signup_form(form_trial, "", "", None),
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

/// Corps de la page qui suit l'inscription (fonction pure) : `trial_days` = essai démarré (de N jours), sinon le
/// compte attend l'administrateur.
fn signup_done_body(trial_days: Option<u32>) -> String {
    let next = match trial_days {
        Some(days) => format!(
            "<p>Ton essai de {days} jours démarre dès que ton mot de passe est choisi : films et séries sont alors à toi.</p>"
        ),
        None => "<p>L'administrateur activera ton compte ensuite : tu recevras un second mail à ce moment-là.</p>".to_string(),
    };
    format!(
        r#"<div class="notice ok">Un lien vient de partir pour choisir ton mot de passe (valable une heure).</div>{next}<p class="foot">Rien reçu ? Regarde dans les courriers indésirables.</p>"#
    )
}

/// Essai annoncé par le formulaire : la configuration seule (la place sous le plafond se décide à l'envoi).
fn configured_trial_days(ctx: &TaskContext) -> Option<u32> {
    let s = &ctx.cfg.subscriptions;
    subscription_ops::trial_days_for(s.enabled, s.trial_days).ok()
}

/// Essai qu'une inscription démarrerait maintenant (configuration **et** place sous le plafond premium) : la page
/// des réponses neutres (adresse déjà connue, champ leurre) dit la même chose qu'une vraie inscription, sans rien
/// révéler de l'adresse. Jellyfin injoignable : la page « l'administrateur activera » (une vraie inscription
/// échouerait de toute façon).
async fn expected_trial_days(ctx: &TaskContext) -> Option<u32> {
    let s = &ctx.cfg.subscriptions;
    configured_trial_days(ctx)?;
    match accounts::premium_usage(ctx).await {
        Ok((premium, max)) => {
            subscription_ops::expected_trial(s.enabled, s.trial_days, premium, max)
        }
        Err(e) => {
            warn!(
                task = "onboard",
                error = format!("{e:#}"),
                "premium count unavailable for the neutral signup page"
            );
            None
        }
    }
}

fn signup_done(trial_days: Option<u32>) -> Response {
    public_page(
        Public::Signup,
        "Vérifie ta boîte mail",
        &signup_done_body(trial_days),
        StatusCode::OK,
    )
}

/// POST /inscription : crée le compte, démarre l'essai (sinon le compte reste suspendu), envoie le lien, prévient
/// l'admin (tout dans `onboard::run`).
async fn signup_post(
    State(st): State<AppState>,
    Extension(client): Extension<Client>,
    Form(f): Form<SignupForm>,
) -> Response {
    if !st.ctx.cfg.onboard.public_signup {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ip = client.ip;
    let form_trial = configured_trial_days(&st.ctx);
    let username = f.username.trim().to_string();
    let email = f.email.trim().to_lowercase();
    if !f.website.trim().is_empty() {
        // champ leurre rempli : un robot ; réponse neutre, rien de créé
        info!(task = "onboard", %ip, "signup honeypot hit");
        return signup_done(expected_trial_days(&st.ctx).await);
    }
    if !onboard::valid_username(&username) {
        return public_page(
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
                &username,
                &email,
                Some("Pseudo invalide : 2 à 32 caractères, lettres, chiffres, tiret ou souligné."),
            ),
            StatusCode::BAD_REQUEST,
        );
    }
    if !onboard::valid_email(&email) {
        return public_page(
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
                &username,
                &email,
                Some("Adresse e-mail invalide."),
            ),
            StatusCode::BAD_REQUEST,
        );
    }
    if f.accept != "1" {
        return public_page(
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
                &username,
                &email,
                Some("Coche la case pour continuer."),
            ),
            StatusCode::BAD_REQUEST,
        );
    }
    if rate_limited(&st, &ip).await {
        return public_page(
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
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
            return signup_done(expected_trial_days(&st.ctx).await);
        }
        Ok(None) => {}
        Err(e) => {
            warn!(
                task = "onboard",
                error = format!("{e:#}"),
                "signup: jellyseerr unreachable"
            );
            return public_page(
                Public::Signup,
                "Créer ton compte",
                &signup_form(
                    form_trial,
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
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
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
            Public::Signup,
            "Créer ton compte",
            &signup_form(
                form_trial,
                &username,
                &email,
                Some("Trop d'inscriptions aujourd'hui : réessaie demain."),
            ),
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    info!(task = "onboard", %ip, %username, "public signup");
    let parrain = f.parrain.trim().to_uppercase();
    let req = OnboardRequest {
        username: username.clone(),
        email: email.clone(),
        password: None,
        source: onboard::Source::SelfSignup,
        referral: (!parrain.is_empty()).then_some(parrain),
    };
    match onboard::run(&st.ctx, req).await {
        // essai gratuit (`[subscriptions] trial_days`) démarré dans `onboard::run`, avant les mails
        Ok(r) => signup_done(r.trial.as_ref().and_then(|t| t.started_days())),
        Err(e) => {
            warn!(task = "onboard", %ip, error = format!("{e:#}"), "public signup failed");
            public_page(Public::Signup, "Créer ton compte", &signup_form(form_trial, &username, &email, Some("La création a échoué, réessaie dans un instant ou écris à l'administrateur.")), StatusCode::BAD_GATEWAY)
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
    /// Un exemple de chaque mail aux membres (`homelabctl mail-test --all`).
    #[serde(default)]
    all: bool,
    /// Ces types seulement (`--kind`, voir `mail_samples::KINDS`).
    #[serde(default)]
    kinds: Vec<String>,
}

/// `homelabctl mail-test <adresse> --announce <fichier>` : auteur déjà choisi par la CLI (`--author` ou premier
/// modérateur), texte brut tel que `chat announce` l'enverrait.
#[derive(Deserialize)]
struct AnnouncePreviewBody {
    to: String,
    author: String,
    body: String,
}

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

/// `POST /admin/mail-test` (jeton en en-tête). `{to}` : mail de bienvenue d'exemple, lien de démonstration.
/// `{to, all: true}` ou `{to, kinds: [...]}` : exemples des mails aux membres (`mail_samples`), à 2 s d'écart ;
/// réponse `{success, results: [{kind, subject, origin, sent, error?}]}` une fois le dernier parti (une à deux
/// minutes pour tous). Aucun compte ni abonnement touché ; seul un lien de démonstration est émis si un des mails
/// en porte un.
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
    if b.all || !b.kinds.is_empty() {
        return mail_samples_send(&st.ctx, &to, b.all, &b.kinds).await;
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

/// `POST /admin/mail-test/announce` (jeton en en-tête) `{to, author, body}` : aperçu d'une annonce, le mail exact
/// que `POST /admin/chat/announce` avec `email_members` enverrait aux membres (même auteur, même texte, même
/// fonction, même enveloppe), objet « [Aperçu] … », à `to` seulement. Route à part : un homelabd plus ancien répond
/// 404 au lieu d'envoyer le mail de bienvenue d'exemple. Ce gestionnaire n'a ni la base ni l'état du tchat : il ne
/// peut rien publier, ni dans le tchat ni sur Discord. Réponse `{success, results: [Sent], author, origin}`.
async fn admin_mail_test_announce(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<AnnouncePreviewBody>,
) -> Response {
    if !accounts_allowed(&st, header_token(&headers)) {
        return fail(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide").into_response();
    }
    let ctx = &st.ctx;
    let to = b.to.trim().to_lowercase();
    if !onboard::valid_email(&to) {
        return fail(StatusCode::BAD_REQUEST, "adresse invalide").into_response();
    }
    let Some(smtp) = &ctx.secrets.smtp else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "SMTP absent de .env").into_response();
    };
    let author = match chat::announcement_author(ctx, &b.author).await {
        Ok(u) => u,
        Err(e) => {
            if let chat::AuthorError::Jellyfin(cause) = &e {
                warn!(
                    task = "chat",
                    error = format!("{cause:#}"),
                    "announcement preview: jellyfin unreachable"
                );
            }
            let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::BAD_GATEWAY);
            return fail(status, e.to_string()).into_response();
        }
    };
    let sample = match mail_samples::announcement_sample(
        &author.name,
        &b.body,
        &ctx.secrets.jellyfin_public_url,
        ctx.cfg.chat.max_chars,
    ) {
        Ok(s) => s,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let origin = sample.origin.clone();
    let results: Vec<mail_samples::Sent> = if ctx.dry_run {
        vec![mail_samples::Sent {
            kind: sample.kind,
            subject: sample.mail.subject.clone(),
            origin: origin.clone(),
            sent: false,
            error: Some("dry-run : non envoyé".into()),
        }]
    } else {
        mail_samples::send(smtp, &to, &[sample], mail_samples::SEND_GAP).await
    };
    let sent = results.iter().all(|r| r.sent);
    // jamais l'adresse dans le journal
    info!(task = "chat", author = %author.name, sent, "announcement preview mailed");
    Json(json!({ "success": true, "results": results, "author": author.name, "origin": origin }))
        .into_response()
}

/// Exemples des mails aux membres envoyés à `to` (voir `admin_mail_test`).
async fn mail_samples_send(ctx: &TaskContext, to: &str, all: bool, kinds: &[String]) -> Response {
    let kinds = match mail_samples::select(all, kinds) {
        Ok(k) => k,
        Err(e) => return fail(StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    let Some(smtp) = &ctx.secrets.smtp else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "SMTP absent de .env").into_response();
    };
    let link = if !mail_samples::needs_link(&kinds) {
        None
    } else if ctx.dry_run {
        Some(welcome::url(&welcome::link_base(ctx), "dry-run"))
    } else {
        match welcome::issue(ctx, "demo", mail_samples::NAME, to, welcome::KIND_DEMO).await {
            Ok(token) => Some(welcome::url(&welcome::link_base(ctx), &token)),
            Err(e) => {
                return fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("lien de démonstration : {e:#}"),
                )
                .into_response()
            }
        }
    };
    let samples = mail_samples::build(ctx, &kinds, link).await;
    let results: Vec<mail_samples::Sent> = if ctx.dry_run {
        samples
            .iter()
            .map(|s| mail_samples::Sent {
                kind: s.kind,
                subject: s.mail.subject.clone(),
                origin: s.origin.clone(),
                sent: false,
                error: Some("dry-run : non envoyé".into()),
            })
            .collect()
    } else {
        mail_samples::send(smtp, to, &samples, mail_samples::SEND_GAP).await
    };
    let failed = results.iter().filter(|r| !r.sent).count();
    info!(
        task = "onboard",
        kinds = results.len(),
        failed,
        "mail-test : exemples des mails aux membres"
    );
    Json(json!({ "success": true, "results": results })).into_response()
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
            "Identifiant invalide (2 à 32 caractères : lettres, chiffres, _ ou -)",
        );
    }
    if !onboard::valid_email(&email) {
        return fail(StatusCode::BAD_REQUEST, "Adresse e-mail invalide");
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
                format!(
                    "Trop de demandes : attendre {} s entre deux créations",
                    limit.as_secs()
                ),
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
        referral: None,
    };
    match onboard::run(&st.ctx, req).await {
        Ok(r) => {
            let log = if r.dry_run {
                "Simulation : pré-contrôles réussis, aucun compte créé".to_string()
            } else {
                format!(
                    "identifiant Jellyfin {} · identifiant Jellyseerr {} · mail : {} · {}",
                    r.jellyfin_id,
                    r.jellyseerr_id,
                    if r.mail_sent {
                        "lien envoyé"
                    } else {
                        "lien non envoyé (transmettre le lien à la main)"
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
    fn signup_done_page_says_what_happens_next() {
        let trial = signup_done_body(Some(7));
        assert!(trial.contains("Ton essai de 7 jours démarre dès que ton mot de passe est choisi"));
        assert!(!trial.contains("L'administrateur activera"));
        let manual = signup_done_body(None);
        assert!(manual.contains("L'administrateur activera ton compte ensuite"));
        assert!(!manual.contains("essai"));
        for b in [&trial, &manual] {
            assert!(b.contains("Un lien vient de partir pour choisir ton mot de passe"));
        }
    }

    /// Le formulaire annonce l'essai quand il est configuré, la validation par l'administrateur sinon.
    #[test]
    fn signup_form_announces_the_trial() {
        let trial = signup_form(Some(7), "", "", None);
        assert!(trial.contains("essai gratuit de 7 jours"));
        assert!(!trial.contains("validé par l'administrateur"));
        let manual = signup_form(None, "", "", None);
        assert!(manual.contains("Chaque compte est ensuite validé par l'administrateur."));
        assert!(manual.contains("Je sais que mon compte sera validé par l'administrateur"));
        assert!(!manual.contains("essai"));
        for f in [&trial, &manual] {
            assert!(f.contains(r#"name="accept" value="1" required"#));
        }
    }

    /// Pages membres sur le squelette commun : titre d'onglet d'origine, titre échappé, carte étroite, script
    /// des pages de bienvenue seulement, aucune marque `{{…}}` restante.
    #[test]
    fn public_pages_use_the_shared_skeleton() {
        let w = public_html(Public::Welcome, "Bienvenue, <jo> !", "<p>corps</p>");
        assert!(w.contains("<title>Bienvenue sur Groscailloux</title>"));
        assert!(w.contains("<h1>Bienvenue, &lt;jo&gt; !</h1>"));
        assert!(w.contains(r#"<main id="main" class="gc-main">"#));
        assert!(w.contains(r#"<div class="gc-narrow"><div class="card gc-public">"#));
        assert!(w.contains("<p>corps</p>"));
        assert!(w.contains("document.querySelectorAll('.copy[data-copy]')"));
        assert!(w.contains("document.getElementById('pw-form')"));
        assert!(w.contains(".gc-public form .btn"));
        let i = public_html(
            Public::Signup,
            "Créer ton compte",
            &signup_form(Some(7), "", "", None),
        );
        assert!(i.contains("<title>Créer un compte Groscailloux</title>"));
        assert!(!i.contains("<script"));
        for h in [&w, &i] {
            assert!(!h.contains("{{"));
            assert!(h.contains(r#"<meta name="robots" content="noindex">"#));
            // aucune ressource tierce : seuls les liens écrits par la page en apportent
            assert!(!h.to_ascii_lowercase().contains("https://"));
        }
    }

    /// Les identifiants que lit `bienvenue.js` sont bien dans le formulaire ; le script en ligne ne ferme pas
    /// sa balise.
    #[test]
    fn password_form_matches_its_script() {
        let f = password_form("jo", "tok<en", false);
        for id in [
            r#"id="pw-form""#,
            r#"data-username="jo""#,
            r#"id="pw""#,
            r#"id="pw2""#,
            r#"id="pw-err""#,
            r#"id="pw-btn""#,
        ] {
            assert!(f.contains(id), "{id}");
        }
        assert!(f.contains(r#"action="/bienvenue/tok&lt;en""#));
        assert!(password_form("jo", "t", true).contains(" disabled>"));
        for js in [WELCOME_JS, ONBOARD_ADMIN_JS] {
            assert!(!js.to_ascii_lowercase().contains("</script"));
        }
    }

    /// Onboarding admin : squelette commun, identifiants lus par `onboard-admin.js`, script conservé.
    #[test]
    fn admin_onboarding_page_keeps_its_script() {
        let h = index_page();
        assert!(h.contains("<title>Groscailloux — Nouveau compte</title>"));
        for id in [
            r#"id="onboardForm""#,
            r#"id="submitBtn""#,
            r#"id="result""#,
            r#"id="toast""#,
            r#"id="username""#,
            r#"id="email""#,
            r#"pattern="[a-zA-Z0-9_-]{2,32}""#,
        ] {
            assert!(h.contains(id), "{id}");
        }
        assert!(h.contains("fetch('/onboard'"));
        assert!(h.contains("location.href = '/connexion?next=/';"));
        assert!(
            h.find("id=\"onboardForm\"")
                < h.find("const form = document.getElementById('onboardForm');")
        );
        assert!(!h.contains("btn-secondary") && !h.contains("{{"));
        // 3.3b : texte visible en français, aucun émoji (page et script)
        for fr in [
            r#"<label for="username">Identifiant</label>"#,
            r#"<label for="email">Adresse e-mail</label>"#,
            r#"placeholder="ex. : hippo""#,
            "Copier les informations",
            "Créer un autre compte",
            "Mot de passe",
        ] {
            assert!(h.contains(fr), "{fr}");
        }
        for en in [
            ">Username<",
            ">Email<",
            "ex: hippo",
            ">Password<",
            "Dry-run",
            ">Streaming<",
        ] {
            assert!(!h.contains(en), "{en}");
        }
        assert!(
            !h.chars()
                .any(|c| matches!(c as u32, 0x2600..=0x27BF | 0x1F000..=0x1FAFF | 0xFE0F)),
            "émoji dans l'onboarding admin"
        );
    }

    /// Un homelabctl d'avant `--all` n'envoie que `to` : mail de bienvenue d'exemple, comme avant.
    #[test]
    fn mail_test_body_without_kinds_is_the_welcome_sample() {
        let b: MailTestBody = serde_json::from_value(json!({ "to": "a@example.test" })).unwrap();
        assert!(!b.all && b.kinds.is_empty());
        let b: MailTestBody =
            serde_json::from_value(json!({ "to": "a@example.test", "kinds": ["demo"] })).unwrap();
        assert!(!b.all && b.kinds == ["demo"]);
        let b: MailTestBody =
            serde_json::from_value(json!({ "to": "a@example.test", "all": true })).unwrap();
        assert!(b.all);
    }

    /// Aperçu d'une annonce : les trois champs sont requis (un homelabctl qui en oublierait un est refusé).
    #[test]
    fn announce_preview_body_needs_address_author_and_text() {
        let b: AnnouncePreviewBody = serde_json::from_value(
            json!({ "to": "a@example.test", "author": "Admin", "body": "Titre\n\nTexte" }),
        )
        .unwrap();
        assert_eq!(
            (b.to.as_str(), b.author.as_str(), b.body.as_str()),
            ("a@example.test", "Admin", "Titre\n\nTexte")
        );
        for missing in ["to", "author", "body"] {
            let mut v = json!({ "to": "a@example.test", "author": "Admin", "body": "x" });
            v.as_object_mut().unwrap().remove(missing);
            assert!(
                serde_json::from_value::<AnnouncePreviewBody>(v).is_err(),
                "{missing}"
            );
        }
    }

    #[test]
    fn premium_page_fills_every_placeholder() {
        let html = premium_page("CID-1", "P-9", false, "jo<hn", "3,50 € / mois", false);
        assert!(html.contains("client-id=CID-1"));
        assert!(html.contains("plan_id: 'P-9'"));
        assert!(html.contains("jo&lt;hn"));
        assert!(html.contains("https://www.paypal.com/sdk/js?client-id=CID-1"));
        assert!(html.contains("https://www.paypal.com/webapps/billing/plans/subscribe?plan_id=P-9"));
        assert!(html.contains("3,50 € / mois, sans engagement."));
        // `data-deja` porté par `#premium` (le `<main>` vient de `ui::page`), lu par le script
        assert!(html.contains(r#"<div class="gc-narrow" id="premium" data-deja="0">"#));
        assert!(html.contains("document.getElementById('premium').getAttribute('data-deja')"));
        assert!(html.contains(r#"<main id="main" class="gc-main">"#));
        assert!(html.contains("<title>Homeflix GrosCailloux — Abonnement Premium</title>"));
        assert!(html.contains(r#"<meta name="description" content="Active le compte Premium"#));
        // avatar servi par NPM ; bouton PayPal et formulaire de rattachement présents
        assert!(html.contains(r#"src="/avatar.jpg""#));
        assert!(html.contains(r#"<div id="paypal-button-container""#));
        assert!(html.contains(r#"<form id="link" method="post" action="/premium/lier" hidden>"#));
        // DA sombre unique : plus de titre serif ni de mode clair
        assert!(!html.contains("ui-serif") && !html.contains("Georgia"));
        assert!(!html.contains("prefers-color-scheme"));
        // seul script tiers : le SDK PayPal
        assert_eq!(html.matches("<script src=").count(), 1);
        assert!(!html.contains("{{"));
        let sb = premium_page("CID-1", "P-9", true, "", "x", false);
        assert!(sb.contains("https://www.sandbox.paypal.com/sdk/js?client-id=CID-1"));
    }

    #[test]
    fn premium_page_hides_the_button_for_an_account_already_subscribed() {
        let html = premium_page("CID-1", "P-9", false, "john", "3,50 € / mois", true);
        assert!(html.contains(r#"<div class="gc-narrow" id="premium" data-deja="1">"#));
        // l'état « déjà abonné » masque formulaire, bouton et repli (le serveur ne les retire pas)
        assert!(html.contains(r#"#premium[data-deja="1"] #who"#));
        assert!(html.contains(r#"#premium[data-deja="1"] #paypal-button-container"#));
        assert!(html.contains(r#"#premium:not([data-deja="1"]) .deja-only"#));
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
        let html = premium_activate_page();
        assert!(html.contains(r#"<form method="post" action="/premium/activate">"#));
        assert!(html.contains(r#"<form method="post" action="/premium/lier">"#));
        assert!(html.contains(r#"name="name""#));
        assert!(html.contains(r#"name="subscription_id""#));
        assert!(html.contains(r#"type="submit""#));
        assert!(html.contains("/premium"));
        // identifiants lus par le script des messages
        for id in [
            r#"id="ok""#,
            r#"id="err""#,
            r#"id="subscription_id""#,
            r#"id="compte""#,
        ] {
            assert!(html.contains(id), "{id}");
        }
        assert!(html.contains("var msgs = {"));
        assert!(html.contains("<title>Homeflix GrosCailloux — Activation Premium</title>"));
        assert!(!html.contains("{{"));
        assert!(!html.contains("<script src="));
        assert!(!html.to_ascii_lowercase().contains("https://"));
    }

    #[test]
    fn merci_page_escapes_its_query_values() {
        let html = premium_merci_page("jo<hn", "1 <déc>", "https://jf.invalid/?a=1&b=2");
        assert!(html
            .contains("Le compte <strong>jo&lt;hn</strong> est Premium jusqu'au 1 &lt;déc&gt;."));
        assert!(html.contains(r#"href="https://jf.invalid/?a=1&amp;b=2""#));
        assert!(html.contains(r#"src="/avatar.jpg""#));
        assert!(!html.contains("{{"));
        assert!(!html.contains("<script"));
        let sans = premium_merci_page("jo", "", "/");
        assert!(sans.contains("<strong>jo</strong> est Premium."));
    }

    /// Les scripts Premium en ligne ne ferment pas leur balise.
    #[test]
    fn premium_scripts_stay_inline_safe() {
        for js in [PREMIUM_JS, PREMIUM_ACTIVATE_JS] {
            assert!(!js.to_ascii_lowercase().contains("</script"));
        }
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

/// Rendus statiques des pages servies par homelabd, avec des données factices (aucun pseudo réel, aucune adresse
/// réelle) : relecture de la DA hors ligne et captures avant/après. Ignoré par défaut, n'écrit rien sans
/// `GC_RENDER_DIR` : `GC_RENDER_DIR=<dossier> cargo test -p homelabd render_ -- --ignored`.
#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::{accounts_page, admin_auth, red_list_page, search_page, status_page};
    use homelab_core::accounts::Account;
    use homelab_core::config::Secrets;
    use homelab_core::manual_search::{Found, Job, Row};
    use homelab_core::quota::SeedboxQuota;
    use homelab_core::secret::Secret;
    use homelab_core::state::{
        AlertStats, CleanJob, Exemption, RedEntry, RedListReport, RedSide, RunInfo,
    };
    use homelab_core::tasks::series_search::Target;
    use std::collections::BTreeMap;

    const JF: &str = "https://tv.example.org";
    const JS: &str = "https://demandes.example.org";

    fn secrets() -> Secrets {
        let k = || Secret::new("factice");
        Secrets {
            sonarr_api_key: k(),
            radarr_api_key: k(),
            prowlarr_api_key: None,
            jellyfin_api_key: k(),
            jellyseerr_api_key: k(),
            jellyfin_lib_films: String::new(),
            jellyfin_lib_series: String::new(),
            jellyfin_lib_extra: Vec::new(),
            seedbox_radarr_api_key: None,
            seedbox_sonarr_api_key: None,
            seedbox_qbit_password: None,
            seedbox_bazarr_api_key: None,
            jellyfin_public_url: JF.into(),
            jellyseerr_public_url: JS.into(),
            onboard_public_url: Some("https://compte.example.org".into()),
            onboard_fallback_url: None,
            guide_contact_email: None,
            guide_contact_discord: Some("admin-exemple".into()),
            chat_admin_email: None,
            discord_webhook_members: None,
            discord_webhook_admin: None,
            discord_role_members: None,
            quality_profile_id: 6,
            onboard_token: None,
            status_token: None,
            admin_trusted_ips: Vec::new(),
            russian_route_users: Vec::new(),
            seedbox_jackett_api_key: None,
            donation: None,
            donation_notify_email: String::new(),
            smtp: None,
            paypal: None,
            premium_public_url: None,
            media_mail_preview_to: None,
            media_mail_optout: Vec::new(),
        }
    }

    async fn body_of(r: Response) -> String {
        let b = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    fn account(
        name: &str,
        premium: bool,
        admin: bool,
        protected: bool,
        seen: Option<&str>,
    ) -> Account {
        Account {
            id: format!("id-{name}"),
            name: name.into(),
            premium,
            admin,
            protected,
            max_streams: 2,
            last_activity: seen.map(Into::into),
        }
    }

    fn red(key: &str, title: &str, gb: u64, movie: bool, side: &str) -> RedEntry {
        let bytes = gb * 1_073_741_824;
        RedEntry {
            key: key.into(),
            title: title.into(),
            kind: if movie { "film" } else { "série" }.into(),
            movie,
            sides: vec![RedSide {
                side: side.into(),
                arr_id: 7,
                bytes,
                folder: "/data/exemple".into(),
                ..Default::default()
            }],
            bytes,
            arrived: Some(1_790_000_000),
            due: Some(1_795_000_000),
            reason: "jamais lu, arrivé le 21/09/2026".into(),
            requester: "membre".into(),
            ..Default::default()
        }
    }

    fn release(title: &str, gb: i64, lang: u8, flags: Vec<&'static str>) -> Row {
        Row {
            indexer: "C411".into(),
            title: title.into(),
            size: gb * 1_073_741_824,
            seeders: 24,
            lang,
            resolution: 1080,
            codec: 1,
            audio: 3,
            quality: "WEBDL-1080p".into(),
            season: Some(1),
            episodes: vec![],
            full_season: true,
            flags,
            release: serde_json::json!({}),
        }
    }

    fn run(ok: Option<bool>, end: i64, summary: &str) -> RunInfo {
        RunInfo {
            last_start: end - 5,
            last_end: Some(end),
            last_ok: ok,
            last_summary: summary.into(),
            runs: 12,
            ..Default::default()
        }
    }

    /// Une page par fichier `<nom>.html` dans `GC_RENDER_DIR`.
    #[tokio::test]
    #[ignore = "rendus pour relecture : GC_RENDER_DIR=<dossier> cargo test -p homelabd render_ -- --ignored"]
    async fn render_pages_to_dir() {
        let Some(dir) = std::env::var_os("GC_RENDER_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = secrets();
        let now = 1_793_000_000;
        let mut pages: Vec<(&str, String)> = Vec::new();
        // prix affiché : défaut du code (= `[subscriptions] price_text` de homelab.toml)
        let price = homelab_core::config::Subscriptions::default().price_text;

        // pages membres
        pages.push((
            "inscription-essai",
            public_html(
                Public::Signup,
                "Créer ton compte",
                &signup_form(Some(7), "", "", None),
            ),
        ));
        pages.push((
            "inscription-sans-essai",
            public_html(
                Public::Signup,
                "Créer ton compte",
                &signup_form(
                    None,
                    "camille",
                    "camille@example.org",
                    Some("Ce pseudo est déjà pris."),
                ),
            ),
        ));
        pages.push((
            "inscription-envoyee",
            public_html(
                Public::Signup,
                "Vérifie ta boîte mail",
                &signup_done_body(Some(7)),
            ),
        ));
        // même corps que `welcome_get` pour un compte en attente d'activation
        let welcome = format!(
            r#"<p>Ton compte <b>{user}</b> est prêt. Il ne reste qu'à choisir ton mot de passe.</p><div class="notice warn">Ton compte sera activé par l'administrateur sous peu : tu recevras un mail à ce moment-là. Tu peux déjà choisir ton mot de passe.</div>{form}"#,
            user = esc("camille"),
            form = password_form("camille", "jeton-factice", false),
        );
        pages.push((
            "bienvenue-mot-de-passe",
            public_html(Public::Welcome, "Bienvenue, camille !", &welcome),
        ));
        pages.push(("lien-expire", body_of(expired_page()).await));
        // même corps que `first_steps_page`
        let steps = format!(
            r#"<p>Quatre minutes pour tout installer : ton écran, ta langue, ta première demande.</p>
<div class="links"><a class="btn" href="{jf}">Regarder : ouvrir Groscailloux</a></div>
{steps}"#,
            jf = esc(&s.jellyfin_public_url),
            steps = first_steps(&s),
        );
        pages.push((
            "premiers-pas",
            public_html(Public::Welcome, "Premiers pas", &steps),
        ));
        pages.push((
            "premium",
            premium_page(
                "CLIENT-FACTICE",
                "P-FACTICE",
                true,
                "camille",
                &price,
                false,
            ),
        ));
        pages.push((
            "premium-deja-abonne",
            premium_page("CLIENT-FACTICE", "P-FACTICE", true, "camille", &price, true),
        ));
        pages.push(("premium-activate", premium_activate_page()));
        pages.push((
            "premium-merci",
            premium_merci_page("camille", "02/12/2026", JF),
        ));
        pages.push((
            "guide",
            guide::render(
                JF,
                JS,
                None,
                Some("admin-exemple"),
                Some("contact@example.org"),
            ),
        ));

        // pages d'administration
        pages.push(("onboarding-admin", index_page()));
        pages.push((
            "connexion",
            body_of(admin_auth::login_page("/accounts", "", StatusCode::OK)).await,
        ));
        pages.push((
            "connexion-erreur",
            body_of(admin_auth::login_page(
                "/accounts",
                "Jeton incorrect.",
                StatusCode::UNAUTHORIZED,
            ))
            .await,
        ));
        let found = vec![
            Found {
                arr: "sonarr-seedbox",
                id: 7,
                title: "Série animée d'exemple".into(),
                year: 2023,
                movie: false,
                anime: true,
                has_file: true,
                seasons: vec![(1, 28, 28), (2, 3, 12)],
            },
            Found {
                arr: "radarr-seedbox",
                id: 9,
                title: "Film d'exemple".into(),
                year: 2021,
                movie: true,
                anime: false,
                has_file: false,
                seasons: vec![],
            },
        ];
        pages.push((
            "recherche",
            search_page::render_home("jeton-factice", "exemple", &found, None),
        ));
        let mut job = Job {
            id: "abc123".into(),
            arr: "sonarr-seedbox",
            label: "Série animée d'exemple — saison 2".into(),
            target: Target::Season {
                series_id: 7,
                season: 2,
            },
            started: now - 30,
            done: true,
            rows: vec![
                release("Serie.Exemple.S02.MULTi.1080p.WEB.x264-GRP", 14, 3, vec![]),
                release("Serie.Exemple.S02.FRENCH.1080p.WEB.x265-GRP", 9, 2, vec![]),
                release(
                    "Serie.Exemple.S02.VOSTFR.1080p.WEB.x264-GRP",
                    11,
                    1,
                    vec!["VOSTFR"],
                ),
            ],
            notes: vec!["C411 : 3 release(s)".into()],
            sent: vec![],
        };
        pages.push((
            "recherche-resultats",
            search_page::render_results(
                "jeton-factice",
                &job,
                Some(("ok", "Release envoyée à Sonarr.")),
                now,
            ),
        ));
        job.done = false;
        job.rows.clear();
        pages.push((
            "recherche-en-cours",
            search_page::render_results("jeton-factice", &job, None, now),
        ));
        let accounts = [
            account(
                "admin-exemple",
                true,
                true,
                true,
                Some("2026-11-02T08:10:00Z"),
            ),
            account("camille", true, false, false, Some("2026-10-30T21:00:00Z")),
            account("sam", false, false, false, None),
        ];
        let mut links = HashMap::new();
        links.insert("id-sam".to_string(), "lien envoyé il y a 2 h".to_string());
        let mut subs = HashMap::new();
        subs.insert(
            "id-camille".to_string(),
            accounts_page::SubInfo {
                status: "active".into(),
                label: "Abonné".into(),
                expires: "02/12/2026".into(),
                source: "paypal".into(),
                hand_due: false,
            },
        );
        subs.insert(
            "id-sam".to_string(),
            accounts_page::SubInfo {
                status: "trial".into(),
                label: "Essai".into(),
                expires: "09/11/2026".into(),
                source: "trial".into(),
                hand_due: false,
            },
        );
        pages.push((
            "comptes",
            accounts_page::render(&accounts_page::PageData {
                now,
                accounts: &accounts,
                max_premium: 25,
                max_playbacks: 2,
                token: "jeton-factice",
                msg: Some(("activated", "camille")),
                links: &links,
                subs: &subs,
            }),
        ));
        let report = RedListReport {
            at: now - 3600,
            min_idle_days: 60,
            soon_days: 14,
            complete: true,
            evaluated: 120,
            entries: vec![
                red("movie:tmdb:1", "Film d'exemple", 12, true, "seedbox"),
                red("series:tvdb:2", "Série d'exemple", 48, false, "seedbox"),
                red("movie:tmdb:3", "Autre film", 4, true, "vps"),
            ],
            soon: vec![red(
                "movie:tmdb:4",
                "Film bientôt rouge",
                6,
                true,
                "seedbox",
            )],
            ..Default::default()
        };
        let mut exemptions = BTreeMap::new();
        exemptions.insert(
            "movie:tmdb:5".to_string(),
            Exemption {
                at: now - 86_400,
                title: "Film exempté".into(),
            },
        );
        let jobs = [CleanJob {
            id: "job1".into(),
            started: now - 7200,
            ended: Some(now - 7100),
            dry_run: false,
            status: "done".into(),
            keys: vec!["movie:tmdb:6".into()],
            ..Default::default()
        }];
        pages.push((
            "liste-rouge",
            red_list_page::render_list(&red_list_page::ListData {
                now,
                report: Some(&report),
                exemptions: &exemptions,
                jobs: &jobs,
                pending: 0,
                token: "jeton-factice",
                msg: None,
                clean_dry_run: false,
                global_dry: false,
                busy: false,
                max: 50,
            }),
        ));
        let mut runs = BTreeMap::new();
        runs.insert(
            "stack_health".to_string(),
            run(Some(true), now - 60, "healthy=18"),
        );
        runs.insert(
            "torrent_import".to_string(),
            run(Some(false), now - 300, "seedbox injoignable"),
        );
        runs.insert(
            "series_search".to_string(),
            run(None, now - 600, "rien à chercher"),
        );
        let tasks = ["stack_health", "torrent_import", "series_search", "cleanup"];
        let stuck = ["seedbox · Un.Anime.S01.VOSTFR.1080p · jamais".to_string()];
        let blocked = ["Série d'exemple S02 (seedbox) : E05, E06".to_string()];
        pages.push((
            "status",
            status_page::render(&status_page::PageData {
                now,
                runs: &runs,
                tasks: &tasks,
                vps_disk_pct: Some(74),
                seedbox: Some(SeedboxQuota {
                    used_kb: 1_429_000_000,
                    quota_kb: 3_725_000_000,
                    at: now - 600,
                }),
                mount_ok: Some(true),
                stuck_torrents: &stuck,
                blocked_seasons: &blocked,
                canary: Some((true, "lecture OK il y a 4 min".into())),
                catalogue: None,
                alerts: &AlertStats::default(),
            }),
        ));
        pages.push((
            "erreur-session",
            ui::error_page(
                "Session expirée",
                "Session expirée : <a href=\"/connexion?next=/accounts\">se reconnecter</a>.",
                None,
            ),
        ));
        pages.push((
            "erreur-jellyfin",
            ui::error_page(
                "Jellyfin injoignable",
                "Jellyfin ne répond pas : réessaie dans un instant.",
                Some(("", "Réessayer")),
            ),
        ));

        for (name, html) in &pages {
            assert!(html.starts_with("<!doctype html>"), "{name}");
            assert!(!html.contains("{{"), "{name} : marque restante");
            std::fs::write(dir.join(format!("{name}.html")), html).unwrap();
        }
    }
}
