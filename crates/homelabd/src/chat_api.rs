//! API du tchat (`/chat/*`), publiée par NPM sous `/gc-chat/` sur l'adresse de Jellyfin (même
//! origine que l'interface, pas de CORS). Chaque requête porte le jeton de session Jellyfin du
//! membre, vérifié par `/Users/Me` (cache mémoire 5 min, jamais écrit ni journalisé).
//! Mails : récapitulatif aux modérateurs (entraide + privé, au plus un par intervalle) et annonce
//! aux membres sur demande. Règles et stockage : `homelab_core::chat`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use homelab_core::chat::{self, Channel, ChatStore, ChatUser, Message, RateLimiter};
use homelab_core::config::Chat as ChatConfig;
use homelab_core::{mail, TaskContext};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

const APP_JS: &str = include_str!("../assets/chat/app.js");
const AUTH_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct ChatState {
    ctx: Arc<TaskContext>,
    store: Arc<ChatStore>,
    auth: Arc<Mutex<HashMap<String, (ChatUser, Instant)>>>,
    limiter: Arc<StdMutex<RateLimiter>>,
}

type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "error": msg })))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Refus d'une règle du tchat (code HTTP + message lisible pour le membre) ou panne de la base. Les règles
/// vivent dans des fonctions `svc_*` qui ne touchent ni au réseau ni au `TaskContext` : elles sont testées
/// directement (module `tests`), les gestionnaires axum ne font qu'identifier le membre et brancher les effets
/// de bord (Discord, mail).
#[derive(Debug)]
enum Fail {
    Http(StatusCode, &'static str),
    Db(anyhow::Error),
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        Fail::Db(e)
    }
}

fn refuse<T>(code: StatusCode, msg: &'static str) -> Result<T, Fail> {
    Err(Fail::Http(code, msg))
}

/// Exécute une opération SQLite hors du runtime async.
async fn db<T, E, F>(st: &ChatState, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    E: Into<Fail> + Send + 'static,
    F: FnOnce(&ChatStore) -> Result<T, E> + Send + 'static,
{
    let store = st.store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne"))?
        .map_err(|e| match e.into() {
            Fail::Http(code, msg) => err(code, msg),
            Fail::Db(e) => {
                warn!(task = "chat", error = format!("{e:#}"), "database error");
                err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne")
            }
        })
}

/// Limite d'envoi par membre (`[chat] min_gap_secs`, `burst_max`) ; vrai si l'envoi est permis.
fn limited(st: &ChatState, user: &str, cfg: &ChatConfig) -> bool {
    st.limiter
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .check(user, now(), cfg)
}

pub fn router(ctx: Arc<TaskContext>) -> anyhow::Result<Router> {
    if !ctx.cfg.chat.enabled {
        return Ok(Router::new());
    }
    let store = Arc::new(ChatStore::open(&ctx.cfg.chat.db_file)?);
    // premier démarrage sur une base existante : pas de mail pour l'historique
    if store.meta("mail_last_id")? == 0 {
        store.set_meta("mail_last_id", store.max_id()?)?;
    }
    let st = ChatState {
        ctx,
        store,
        auth: Arc::new(Mutex::new(HashMap::new())),
        limiter: Arc::new(StdMutex::new(RateLimiter::default())),
    };
    spawn_moderator_mail(st.clone());
    Ok(Router::new()
        .route("/chat/app.js", get(app_js))
        .route("/chat/api/me", get(me))
        .route("/chat/api/messages", get(list).post(send))
        .route("/chat/api/messages/{id}", delete(remove))
        .route("/chat/api/read", post(read))
        .route("/chat/api/private", get(private_threads))
        .route("/chat/api/members", get(members))
        .route("/chat/api/direct", post(direct))
        .route("/admin/chat/announce", post(admin_announce))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(st))
}

fn etag() -> &'static str {
    static E: OnceLock<String> = OnceLock::new();
    E.get_or_init(|| {
        // FNV-1a : suffit pour invalider le cache à chaque nouvelle version du script
        let h = APP_JS.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
        format!("\"{h:016x}\"")
    })
}

async fn app_js(headers: HeaderMap) -> Response {
    let tag = etag();
    let cache = [
        (header::ETAG, HeaderValue::from_static(tag)),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
    ];
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(tag)
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

async fn auth(st: &ChatState, headers: &HeaderMap) -> ApiResult<ChatUser> {
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
    let user = match cached {
        Some(u) => u,
        None => {
            let me = st
                .ctx
                .jellyfin
                .user_from_token(&token)
                .await
                .map_err(|e| {
                    warn!(
                        task = "chat",
                        error = format!("{e:#}"),
                        "jellyfin Users/Me failed"
                    );
                    err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
                })?
                .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "session Jellyfin invalide"))?;
            let disabled = me
                .pointer("/Policy/IsDisabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (Some(id), Some(name)) = (
                me.get("Id").and_then(Value::as_str),
                me.get("Name").and_then(Value::as_str),
            ) else {
                return Err(err(StatusCode::UNAUTHORIZED, "session Jellyfin invalide"));
            };
            if disabled {
                return Err(err(StatusCode::UNAUTHORIZED, "compte suspendu"));
            }
            let u = ChatUser {
                id: chat::normalize_id(id),
                name: name.to_string(),
                moderator: chat::is_listed(name, &st.ctx.cfg.chat.moderators),
            };
            st.auth
                .lock()
                .await
                .insert(token, (u.clone(), Instant::now()));
            u
        }
    };
    if !chat::allowed(&user.name, &st.ctx.cfg.chat) {
        return Err(err(StatusCode::FORBIDDEN, "tchat pas encore ouvert"));
    }
    Ok(user)
}

fn channel_of(key: &str) -> Result<Channel, Fail> {
    Channel::parse(key).ok_or(Fail::Http(StatusCode::BAD_REQUEST, "salon inconnu"))
}

fn label(ch: &Channel) -> &'static str {
    match ch {
        Channel::Annonces => "Annonces",
        Channel::Entraide => "Entraide",
        Channel::Private(_) => "Écrire à l'admin",
    }
}

/// Corps de `/me` : identité, salons publics avec leurs non-lus, fil privé, annonce et message de l'admin à
/// afficher en bandeau. Le même objet est joint (clé `me`) aux réponses de `/messages` et de `/read` : la bulle
/// n'a plus à redemander `/me` après chaque lecture ni pendant que le panneau est ouvert (2026-10-08).
/// Première visite d'un compte : les messages publics de plus de `new_member_read_days` jours comptent comme lus.
fn svc_me(s: &ChatStore, cfg: &ChatConfig, u: &ChatUser, now: i64) -> anyhow::Result<Value> {
    s.init_reads(&u.id, now, cfg.new_member_read_days)?;
    let own = Channel::Private(u.id.clone());
    let mut channels = Vec::new();
    let mut announcements_unread = 0;
    for ch in Channel::PUBLIC {
        let unread = s.unread(&u.id, &ch)?;
        if ch == Channel::Annonces {
            announcements_unread = unread;
        }
        channels.push(json!({
            "key": ch.key(),
            "label": label(&ch),
            "can_post": chat::can_post(u, &ch),
            "unread": unread,
        }));
    }
    let private_unread = if u.moderator {
        s.unread_private_total(&u.id)?
    } else {
        s.unread(&u.id, &own)?
    };
    // dernière annonce non lue (bannière de l'accueil)
    let latest = if announcements_unread > 0 {
        s.list(&Channel::Annonces, None, None, 1)?
            .into_iter()
            .next()
            .filter(|m| !m.deleted)
    } else {
        None
    };
    // message privé de l'admin non lu (bannière de l'accueil, prioritaire sur l'annonce)
    let latest_private = if u.moderator {
        None
    } else {
        s.latest_unread_from_moderator(&u.id, &own)?
    };
    Ok(json!({
        "user": { "id": u.id, "name": u.name, "moderator": u.moderator },
        "channels": channels,
        "private": { "key": own.key(), "unread": private_unread },
        "latest_announcement": latest,
        "latest_private": latest_private,
        "limits": { "max_chars": cfg.max_chars, "delete_own_within_secs": cfg.delete_own_within_mins * 60 },
    }))
}

async fn me(State(st): State<ChatState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let cfg = st.ctx.cfg.clone();
    Ok(Json(
        db(&st, move |s| svc_me(s, &cfg.chat, &u, now())).await?,
    ))
}

#[derive(Deserialize)]
struct ListQuery {
    channel: String,
    after: Option<i64>,
    before: Option<i64>,
    limit: Option<usize>,
}

/// Messages d'un salon (suite après `after`, ou page avant `before`) + l'état des non-lus (`me`).
fn svc_list(
    s: &ChatStore,
    cfg: &ChatConfig,
    u: &ChatUser,
    q: &ListQuery,
    now: i64,
) -> Result<Value, Fail> {
    let ch = channel_of(&q.channel)?;
    if !chat::can_read(u, &ch) {
        return refuse(StatusCode::FORBIDDEN, "salon réservé");
    }
    let limit = q.limit.unwrap_or(50).min(100);
    let msgs = s.list(&ch, q.after, q.before, limit)?;
    Ok(json!({ "messages": msgs, "limit": limit, "me": svc_me(s, cfg, u, now)? }))
}

async fn list(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let cfg = st.ctx.cfg.clone();
    Ok(Json(
        db(&st, move |s| svc_list(s, &cfg.chat, &u, &q, now())).await?,
    ))
}

#[derive(Deserialize)]
struct SendBody {
    channel: String,
    body: String,
    #[serde(default)]
    email_members: bool,
}

/// Publication d'un message : droits (Annonces = modérateurs seulement, fil privé = son propriétaire et les
/// modérateurs), texte nettoyé, cadence, puis insertion. L'ancien nom `discussion` écrit dans Entraide.
fn svc_send(
    s: &ChatStore,
    cfg: &ChatConfig,
    limiter: &mut RateLimiter,
    u: &ChatUser,
    channel: &str,
    body: &str,
    now: i64,
) -> Result<Message, Fail> {
    let ch = channel_of(channel)?;
    if !chat::can_read(u, &ch) {
        return refuse(StatusCode::FORBIDDEN, "salon réservé");
    }
    if !chat::can_post(u, &ch) {
        return refuse(StatusCode::FORBIDDEN, "seul l'admin publie ici");
    }
    let text = chat::validate_body(body, cfg.max_chars)
        .map_err(|e| Fail::Http(StatusCode::BAD_REQUEST, e))?;
    if !limiter.check(&u.id, now, cfg) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "doucement : attends quelques secondes",
        );
    }
    Ok(s.insert(&ch, u, &text, now)?)
}

async fn send(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<SendBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let (cfg, limiter, uc) = (st.ctx.cfg.clone(), st.limiter.clone(), u.clone());
    let (channel, body) = (b.channel, b.body);
    let msg = db(&st, move |s| {
        let mut l = limiter.lock().unwrap_or_else(|e| e.into_inner());
        svc_send(s, &cfg.chat, &mut l, &uc, &channel, &body, now())
    })
    .await?;
    info!(task = "chat", channel = %msg.channel, author = %u.name, chars = msg.body.chars().count(), "message posted");
    let ch = Channel::parse(&msg.channel).unwrap_or(Channel::Entraide);
    announce_side_effects(
        &st,
        &u,
        &ch,
        msg.body.clone(),
        st.ctx.cfg.discord.announcements,
        b.email_members,
    );
    Ok(Json(json!({ "message": msg })))
}

/// Une annonce d'un modérateur part aussi sur le salon Discord des membres (si un webhook est configuré)
/// et, sur demande, par mail aux membres. Rien pour les autres salons ni les autres auteurs.
fn announce_side_effects(
    st: &ChatState,
    u: &ChatUser,
    ch: &Channel,
    text: String,
    discord: bool,
    email_members: bool,
) {
    if !u.moderator || *ch != Channel::Annonces {
        return;
    }
    if discord {
        let st3 = st.clone();
        let (author, body) = (u.name.clone(), text.clone());
        tokio::spawn(async move {
            homelab_core::discord::notify(
                &st3.ctx,
                homelab_core::discord::Channel::Members,
                homelab_core::discord::Embed::info(format!("Annonce — {author}"), body)
                    .link(st3.ctx.secrets.jellyfin_public_url.clone()),
            )
            .await;
        });
    }
    if email_members {
        let (st2, u2) = (st.clone(), u.clone());
        tokio::spawn(async move { mail_members(st2, u2, text).await });
    }
}

#[derive(Deserialize)]
struct AnnounceBody {
    /// Compte modérateur au nom duquel l'annonce est publiée (`[chat] moderators`).
    author: String,
    body: String,
    #[serde(default)]
    email_members: bool,
    /// `false` pour ne pas relayer sur Discord (défaut : le réglage `[discord] announcements`).
    discord: Option<bool>,
}

/// POST /admin/chat/announce (hors du préfixe `/gc-chat/` publié par NPM ; jeton `HOMELABD_ONBOARD_TOKEN` en en-tête `x-onboard-token`, utilisé par
/// `homelabctl chat announce`) : publie une annonce dans le salon Annonces au nom d'un modérateur, découpée
/// en plusieurs messages si elle dépasse `[chat] max_chars`. Discord et mail reçoivent le texte entier, une fois.
async fn admin_announce(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<AnnounceBody>,
) -> ApiResult<Json<Value>> {
    // appel local seulement (`admin_auth::layer`), jeton comparé à temps constant
    let given = headers.get("x-onboard-token").and_then(|v| v.to_str().ok());
    if !crate::admin_auth::token_matches(st.ctx.secrets.onboard_token.as_ref(), given) {
        return Err(err(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide"));
    }
    let cfg = &st.ctx.cfg.chat;
    if !chat::is_listed(&b.author, &cfg.moderators) {
        return Err(err(StatusCode::FORBIDDEN, "l'auteur n'est pas modérateur"));
    }
    let jf = st.ctx.jellyfin.find_user(&b.author).await.map_err(|e| {
        warn!(
            task = "chat",
            error = format!("{e:#}"),
            "announce: jellyfin unreachable"
        );
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    let Some(jf) = jf else {
        return Err(err(StatusCode::NOT_FOUND, "compte modérateur introuvable"));
    };
    let u = ChatUser {
        id: chat::normalize_id(jf.get("Id").and_then(Value::as_str).unwrap_or("")),
        name: jf
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or(&b.author)
            .to_string(),
        moderator: true,
    };
    // même nettoyage que pour un message ordinaire, sans le plafond de taille (découpage ensuite)
    let text =
        chat::validate_body(&b.body, usize::MAX).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let parts = chat::split_parts(&text, cfg.max_chars);
    let mut ids = Vec::new();
    for p in &parts {
        let (uc, t) = (u.clone(), p.clone());
        let msg = db(&st, move |s| s.insert(&Channel::Annonces, &uc, &t, now())).await?;
        ids.push(msg.id);
    }
    info!(task = "chat", author = %u.name, parts = parts.len(), chars = text.chars().count(), "announcement posted by CLI");
    let discord = b.discord.unwrap_or(st.ctx.cfg.discord.announcements);
    announce_side_effects(&st, &u, &Channel::Annonces, text, discord, b.email_members);
    Ok(Json(
        json!({ "success": true, "message_ids": ids, "parts": parts.len(), "discord": discord, "email_members": b.email_members }),
    ))
}

/// Suppression : son propre message dans le délai (`delete_own_within_mins`), ou n'importe lequel pour un
/// modérateur ; jamais dans un fil privé d'un autre membre, ni un message déjà supprimé.
fn svc_remove(
    s: &ChatStore,
    cfg: &ChatConfig,
    u: &ChatUser,
    id: i64,
    now: i64,
) -> Result<(), Fail> {
    let m = s
        .get(id)?
        .ok_or(Fail::Http(StatusCode::NOT_FOUND, "message introuvable"))?;
    let ch = channel_of(&m.channel)?;
    let window = cfg.delete_own_within_mins * 60;
    if !chat::can_read(u, &ch) || !chat::can_delete(u, &m, now, window) {
        return refuse(StatusCode::FORBIDDEN, "suppression impossible");
    }
    s.mark_deleted(id, &u.id, now)?;
    Ok(())
}

async fn remove(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let (cfg, uc) = (st.ctx.cfg.clone(), u.clone());
    db(&st, move |s| svc_remove(s, &cfg.chat, &uc, id, now())).await?;
    info!(task = "chat", id, by = %u.name, "message deleted");
    Ok(Json(json!({ "deleted": id })))
}

#[derive(Deserialize)]
struct ReadBody {
    channel: String,
    last_id: i64,
}

/// Repère de lecture (jamais en arrière) ; la réponse porte l'état des non-lus à jour (`me`).
fn svc_read(
    s: &ChatStore,
    cfg: &ChatConfig,
    u: &ChatUser,
    channel: &str,
    last_id: i64,
    now: i64,
) -> Result<Value, Fail> {
    let ch = channel_of(channel)?;
    if !chat::can_read(u, &ch) {
        return refuse(StatusCode::FORBIDDEN, "salon réservé");
    }
    s.mark_read(&u.id, &ch, last_id)?;
    Ok(json!({ "ok": true, "me": svc_me(s, cfg, u, now)? }))
}

async fn read(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<ReadBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let cfg = st.ctx.cfg.clone();
    Ok(Json(
        db(&st, move |s| {
            svc_read(s, &cfg.chat, &u, &b.channel, b.last_id, now())
        })
        .await?,
    ))
}

async fn private_threads(
    State(st): State<ChatState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    if !u.moderator {
        return Err(err(StatusCode::FORBIDDEN, "réservé à l'admin"));
    }
    let mut threads = db(&st, move |s| s.private_threads(&u.id)).await?;
    // nom actuel du compte (un fil ouvert par l'admin n'a pas encore de message du membre)
    if let Ok(names) = active_members(&st).await {
        for t in &mut threads {
            if let Some((_, n)) = names.iter().find(|(id, _)| *id == t.member_id) {
                t.member_name = n.clone();
            }
        }
    }
    Ok(Json(json!({ "threads": threads })))
}

/// Comptes Jellyfin actifs (non suspendus) : (id normalisé, nom), par nom.
async fn active_members(st: &ChatState) -> anyhow::Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = st
        .ctx
        .jellyfin
        .users()
        .await?
        .iter()
        .filter(|u| {
            !u.pointer("/Policy/IsDisabled")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|u| {
            Some((
                chat::normalize_id(u.get("Id")?.as_str()?),
                u.get("Name")?.as_str()?.to_string(),
            ))
        })
        .collect();
    out.sort_by_key(|(_, n)| n.to_lowercase());
    Ok(out)
}

/// Adresses des membres, prises dans Jellyseerr (Jellyfin n'en a pas) : id Jellyfin → email valide.
async fn member_emails(st: &ChatState) -> anyhow::Result<HashMap<String, String>> {
    Ok(st
        .ctx
        .jellyseerr
        .users(1000)
        .await?
        .iter()
        .filter_map(|j| {
            let id = j.get("jellyfinUserId").and_then(Value::as_str)?;
            let email = j.get("email").and_then(Value::as_str)?;
            homelab_core::tasks::onboard::valid_email(email)
                .then(|| (chat::normalize_id(id), email.to_string()))
        })
        .collect())
}

fn moderator_only(u: &ChatUser) -> ApiResult<()> {
    if u.moderator {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "réservé à l'admin"))
    }
}

/// Modérateurs : destinataires possibles d'un message privé (comptes actifs, hors modérateurs).
async fn members(State(st): State<ChatState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    moderator_only(&u)?;
    let all = active_members(&st).await.map_err(|e| {
        warn!(
            task = "chat",
            error = format!("{e:#}"),
            "members unreadable"
        );
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    let emails = member_emails(&st).await.unwrap_or_default();
    let list: Vec<Value> = all
        .into_iter()
        .filter(|(_, n)| !chat::is_listed(n, &st.ctx.cfg.chat.moderators))
        .map(|(id, name)| json!({ "id": id, "name": name, "has_email": emails.contains_key(&id) }))
        .collect();
    Ok(Json(json!({ "members": list })))
}

#[derive(Deserialize)]
struct DirectBody {
    user_ids: Vec<String>,
    body: String,
    #[serde(default)]
    email: bool,
}

/// Modérateurs : le même message, envoyé séparément dans le fil privé de chaque destinataire (personne ne
/// voit la liste des autres), avec mail facultatif.
async fn direct(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<DirectBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    moderator_only(&u)?;
    let cfg = &st.ctx.cfg.chat;
    let text =
        chat::validate_body(&b.body, cfg.max_chars).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let mut wanted: Vec<String> = b.user_ids.iter().map(|i| chat::normalize_id(i)).collect();
    wanted.sort();
    wanted.dedup();
    if wanted.is_empty() || wanted.len() > 50 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "choisis entre 1 et 50 destinataires",
        ));
    }
    let members = active_members(&st).await.map_err(|e| {
        warn!(
            task = "chat",
            error = format!("{e:#}"),
            "members unreadable"
        );
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    let targets: Vec<(String, String)> = members
        .into_iter()
        .filter(|(id, _)| wanted.contains(id) && *id != u.id)
        .collect();
    if targets.len() != wanted.len() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "destinataire inconnu ou suspendu",
        ));
    }
    if !limited(&st, &u.id, cfg) {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "doucement : attends quelques secondes",
        ));
    }
    let (uc, t2, tx) = (u.clone(), targets.clone(), text.clone());
    db(&st, move |s| {
        for (id, _) in &t2 {
            s.insert(&Channel::Private(id.clone()), &uc, &tx, now())?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await?;
    let names: Vec<String> = targets.iter().map(|(_, n)| n.clone()).collect();
    info!(task = "chat", by = %u.name, to = ?names, email = b.email, chars = text.chars().count(), "direct message sent");
    if b.email {
        let st2 = st.clone();
        tokio::spawn(async move { mail_direct(st2, u, targets, text).await });
    }
    Ok(Json(json!({ "sent": names.len() })))
}

async fn mail_direct(
    st: ChatState,
    author: ChatUser,
    targets: Vec<(String, String)>,
    text: String,
) {
    let s = &st.ctx.secrets;
    let Some(smtp) = &s.smtp else {
        warn!(task = "chat", "direct mail skipped: no SMTP");
        return;
    };
    let emails = match member_emails(&st).await {
        Ok(e) => e,
        Err(e) => {
            warn!(
                task = "chat",
                error = format!("{e:#}"),
                "direct mail: Jellyseerr unreadable"
            );
            return;
        }
    };
    let (mut sent, mut missing, mut failed) = (0, 0, 0);
    for (id, name) in &targets {
        let Some(email) = emails.get(id) else {
            missing += 1;
            continue;
        };
        let (subject, body) = chat::direct_mail(&author.name, name, &text, &s.jellyfin_public_url);
        if st.ctx.dry_run {
            sent += 1;
            continue;
        }
        match mail::send_plain(smtp, name, email, &subject, &body).await {
            Ok(()) => sent += 1,
            Err(e) => {
                failed += 1;
                warn!(task = "chat", user = %name, error = format!("{e:#}"), "direct mail failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    info!(
        task = "chat",
        sent,
        missing,
        failed,
        dry_run = st.ctx.dry_run,
        "direct message mailed"
    );
}

fn spawn_moderator_mail(st: ChatState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = moderator_mail_pass(&st).await {
                warn!(
                    task = "chat",
                    error = format!("{e:#}"),
                    "moderator mail failed"
                );
            }
        }
    });
}

/// Récapitulatif des messages d'entraide et privés des membres, au plus un par intervalle.
async fn moderator_mail_pass(st: &ChatState) -> anyhow::Result<()> {
    let s = &st.ctx.secrets;
    let (Some(smtp), Some(to)) = (&s.smtp, &s.chat_admin_email) else {
        return Ok(());
    };
    let store = st.store.clone();
    let interval = st.ctx.cfg.chat.moderator_mail_interval_mins * 60;
    let (last_id, last_sent) = (store.meta("mail_last_id")?, store.meta("mail_last_sent")?);
    if now() - last_sent < interval {
        return Ok(());
    }
    let pending = store.pending_for_moderators(last_id)?;
    let Some(max) = pending.iter().map(|m| m.id).max() else {
        return Ok(());
    };
    let (subject, body) = chat::moderator_digest(&pending, &s.jellyfin_public_url);
    if st.ctx.dry_run {
        info!(
            task = "chat",
            messages = pending.len(),
            "dry-run: moderator mail not sent"
        );
        return Ok(());
    }
    mail::send_plain(smtp, "Admin Groscailloux", to, &subject, &body).await?;
    if st.ctx.cfg.discord.admin_alerts {
        homelab_core::discord::notify(
            &st.ctx,
            homelab_core::discord::Channel::Admin,
            homelab_core::discord::Embed::info(subject.clone(), body.clone()),
        )
        .await;
    }
    store.set_meta("mail_last_id", max)?;
    store.set_meta("mail_last_sent", now())?;
    info!(
        task = "chat",
        messages = pending.len(),
        "moderator mail sent"
    );
    Ok(())
}

/// Annonce envoyée par mail aux comptes actifs ayant une adresse (Jellyseerr), sauf l'auteur.
/// Pendant la phase de test, seulement aux `beta_users`.
async fn mail_members(st: ChatState, author: ChatUser, text: String) {
    let s = &st.ctx.secrets;
    let Some(smtp) = &s.smtp else {
        warn!(task = "chat", "announcement mail skipped: no SMTP");
        return;
    };
    let (users, emails) = match (st.ctx.jellyfin.users().await, member_emails(&st).await) {
        (Ok(u), Ok(e)) => (u, e),
        (Err(e), _) | (_, Err(e)) => {
            warn!(
                task = "chat",
                error = format!("{e:#}"),
                "announcement mail: users unreadable"
            );
            return;
        }
    };
    let cfg = &st.ctx.cfg.chat;
    let (subject, body) = chat::announcement_mail(&author.name, &text, &s.jellyfin_public_url);
    let (mut sent, mut failed) = (0, 0);
    for u in &users {
        let (Some(id), Some(name)) = (
            u.get("Id").and_then(Value::as_str),
            u.get("Name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let id = chat::normalize_id(id);
        let disabled = u
            .pointer("/Policy/IsDisabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if disabled || id == author.id || !chat::allowed(name, cfg) {
            continue;
        }
        let Some(email) = emails.get(&id) else {
            continue;
        };
        if st.ctx.dry_run {
            sent += 1;
            continue;
        }
        match mail::send_plain(smtp, name, email, &subject, &body).await {
            Ok(()) => sent += 1,
            Err(e) => {
                failed += 1;
                warn!(task = "chat", user = %name, error = format!("{e:#}"), "announcement mail failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    info!(
        task = "chat",
        sent,
        failed,
        dry_run = st.ctx.dry_run,
        "announcement mailed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn user(id: &str, moderator: bool) -> ChatUser {
        ChatUser {
            id: id.into(),
            name: id.to_uppercase(),
            moderator,
        }
    }

    /// Réglages d'essai : pas de délai minimal entre deux messages (les tests de cadence le remettent).
    fn cfg() -> ChatConfig {
        ChatConfig {
            min_gap_secs: 0,
            ..ChatConfig::default()
        }
    }

    fn store() -> ChatStore {
        ChatStore::open_in_memory().unwrap()
    }

    /// Code HTTP d'un résultat : 200 si la règle a laissé passer, 500 pour une panne de base.
    fn code<T>(r: &Result<T, Fail>) -> u16 {
        match r {
            Ok(_) => 200,
            Err(Fail::Http(c, _)) => c.as_u16(),
            Err(Fail::Db(_)) => 500,
        }
    }

    fn refusal<T>(r: Result<T, Fail>) -> (u16, &'static str) {
        match r {
            Err(Fail::Http(c, m)) => (c.as_u16(), m),
            _ => panic!("un refus était attendu"),
        }
    }

    fn send(
        s: &ChatStore,
        u: &ChatUser,
        channel: &str,
        body: &str,
        at: i64,
    ) -> Result<Message, Fail> {
        svc_send(s, &cfg(), &mut RateLimiter::default(), u, channel, body, at)
    }

    fn list(s: &ChatStore, u: &ChatUser, channel: &str) -> Result<Value, Fail> {
        let q = ListQuery {
            channel: channel.into(),
            after: None,
            before: None,
            limit: None,
        };
        svc_list(s, &cfg(), u, &q, NOW)
    }

    fn bodies(v: &Value) -> Vec<String> {
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["body"].as_str().unwrap().to_string())
            .collect()
    }

    fn unread(me: &Value, key: &str) -> i64 {
        me["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == key)
            .unwrap_or_else(|| panic!("salon {key} absent"))["unread"]
            .as_i64()
            .unwrap()
    }

    #[test]
    fn a_member_cannot_post_announcements_a_moderator_can() {
        let s = store();
        let (m, a) = (user("mod", true), user("a", false));
        let (c, why) = refusal(send(&s, &a, "annonces", "je publie quand même", NOW));
        assert_eq!((c, why), (403, "seul l'admin publie ici"));
        assert!(
            s.list(&Channel::Annonces, None, None, 50)
                .unwrap()
                .is_empty(),
            "rien n'a été écrit"
        );
        let ok = send(&s, &m, "annonces", "  Maintenance dimanche  ", NOW).unwrap();
        assert_eq!(
            (ok.channel.as_str(), ok.body.as_str(), ok.author_moderator),
            ("annonces", "Maintenance dimanche", true)
        );
        // tout le monde lit les annonces
        assert_eq!(
            bodies(&list(&s, &a, "annonces").unwrap()),
            ["Maintenance dimanche"]
        );
        // et personne ne passe par un salon qui n'existe pas
        assert_eq!(code(&send(&s, &m, "general", "x", NOW)), 400);
    }

    #[test]
    fn the_old_discussion_name_reaches_the_merged_room() {
        let s = store();
        let a = user("a", false);
        // une page restée ouverte avant la fusion écrit encore dans « discussion »
        let m = send(&s, &a, "discussion", "ça marche toujours", NOW).unwrap();
        assert_eq!(m.channel, "entraide");
        send(&s, &a, "entraide", "et ici aussi", NOW + 5).unwrap();
        let old = list(&s, &a, "discussion").unwrap();
        let new = list(&s, &a, "entraide").unwrap();
        assert_eq!(bodies(&old), ["ça marche toujours", "et ici aussi"]);
        assert_eq!(bodies(&old), bodies(&new));
        // un repère de lecture envoyé sous l'ancien nom compte pour Entraide
        let r = svc_read(&s, &cfg(), &user("b", false), "discussion", 2, NOW).unwrap();
        assert_eq!(unread(&r["me"], "entraide"), 0);
        // la liste des salons n'en propose plus que deux
        let me = svc_me(&s, &cfg(), &a, NOW).unwrap();
        let keys: Vec<_> = me["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["key"].as_str().unwrap(), c["label"].as_str().unwrap()))
            .collect();
        assert_eq!(keys, [("annonces", "Annonces"), ("entraide", "Entraide")]);
    }

    #[test]
    fn a_private_thread_is_for_its_member_and_the_moderators_only() {
        let s = store();
        let (m, a, b) = (user("mod", true), user("a", false), user("b", false));
        send(&s, &a, "prive:a", "j'ai un souci de lecture", NOW).unwrap();
        // le membre concerné et les modérateurs lisent
        assert_eq!(
            bodies(&list(&s, &a, "prive:a").unwrap()),
            ["j'ai un souci de lecture"]
        );
        assert_eq!(
            bodies(&list(&s, &m, "prive:a").unwrap()),
            ["j'ai un souci de lecture"]
        );
        // un autre membre ne lit, n'écrit, ne marque lu ni ne supprime rien
        assert_eq!(refusal(list(&s, &b, "prive:a")), (403, "salon réservé"));
        assert_eq!(
            refusal(send(&s, &b, "prive:a", "je m'invite", NOW + 5)),
            (403, "salon réservé")
        );
        assert_eq!(code(&svc_read(&s, &cfg(), &b, "prive:a", 1, NOW)), 403);
        assert_eq!(code(&svc_remove(&s, &cfg(), &b, 1, NOW)), 403);
        // l'identifiant du fil est normalisé comme celui de Jellyfin (majuscules)
        assert_eq!(bodies(&list(&s, &a, "prive:A").unwrap()).len(), 1);
        // le modérateur répond ; le membre ne voit que son fil dans son compteur
        send(&s, &m, "prive:a", "je regarde ça", NOW + 10).unwrap();
        let (me_a, me_b) = (
            svc_me(&s, &cfg(), &a, NOW + 20).unwrap(),
            svc_me(&s, &cfg(), &b, NOW + 20).unwrap(),
        );
        assert_eq!(me_a["private"]["unread"], 1);
        assert_eq!(me_a["latest_private"]["body"], "je regarde ça");
        assert_eq!(
            me_b["private"]["unread"], 0,
            "le fil de a n'est pas celui de b"
        );
        assert!(me_b["latest_private"].is_null());
        // un modérateur compte les messages des membres de tous les fils
        send(&s, &b, "prive:b", "et moi", NOW + 30).unwrap();
        let me_m = svc_me(&s, &cfg(), &m, NOW + 40).unwrap();
        assert_eq!(
            me_m["private"]["unread"], 1,
            "seul « et moi » est nouveau pour lui"
        );
        assert!(
            me_m["latest_private"].is_null(),
            "pas de bandeau pour un modérateur"
        );
    }

    #[test]
    fn deleting_follows_the_ownership_and_time_rules() {
        let s = store();
        let (m, a, b) = (user("mod", true), user("a", false), user("b", false));
        let window = cfg().delete_own_within_mins * 60;
        let mine = send(&s, &a, "entraide", "à effacer", NOW).unwrap();
        let other = send(&s, &b, "entraide", "de b", NOW + 1).unwrap();
        // pas celui d'un autre membre
        assert_eq!(
            refusal(svc_remove(&s, &cfg(), &a, other.id, NOW + 2)),
            (403, "suppression impossible")
        );
        // le sien, une fois le délai passé : non
        assert_eq!(
            code(&svc_remove(&s, &cfg(), &a, mine.id, NOW + window + 1)),
            403
        );
        // le sien, dans le délai : oui, et il apparaît « supprimé », sans texte
        svc_remove(&s, &cfg(), &a, mine.id, NOW + window).unwrap();
        let v = list(&s, &b, "entraide").unwrap();
        assert_eq!(v["messages"][0]["deleted"], true);
        assert_eq!(v["messages"][0]["body"], "");
        // supprimé une fois : plus de seconde suppression
        assert_eq!(code(&svc_remove(&s, &cfg(), &m, mine.id, NOW + 5)), 403);
        // un modérateur supprime n'importe quel message, sans délai
        svc_remove(&s, &cfg(), &m, other.id, NOW + 10 * DAY).unwrap();
        // message inconnu
        assert_eq!(
            refusal(svc_remove(&s, &cfg(), &m, 9999, NOW)),
            (404, "message introuvable")
        );
        // un message d'un fil privé n'est supprimable que par son propriétaire (dans le délai) ou un modérateur
        let p = send(&s, &a, "prive:a", "privé", NOW).unwrap();
        assert_eq!(code(&svc_remove(&s, &cfg(), &b, p.id, NOW + 1)), 403);
        svc_remove(&s, &cfg(), &m, p.id, NOW + 1).unwrap();
    }

    #[test]
    fn a_new_account_sees_only_recent_announcements_as_unread() {
        let s = store();
        let (m, a) = (user("mod", true), user("a", false));
        for i in 0..7 {
            send(
                &s,
                &m,
                "annonces",
                &format!("ancienne {i}"),
                NOW - (40 - i) * DAY,
            )
            .unwrap();
        }
        send(&s, &m, "annonces", "d'il y a 12 jours", NOW - 12 * DAY).unwrap();
        send(&s, &m, "annonces", "d'hier", NOW - DAY).unwrap();
        let me = svc_me(&s, &cfg(), &a, NOW).unwrap();
        assert_eq!(unread(&me, "annonces"), 2, "pas « 9 non lus »");
        assert_eq!(
            me["latest_announcement"]["body"], "d'hier",
            "bandeau : la dernière annonce"
        );
        // l'auteur n'a jamais rien de non lu dans ses propres annonces
        assert_eq!(unread(&svc_me(&s, &cfg(), &m, NOW).unwrap(), "annonces"), 0);
        // réglage à 0 jour : tout est lu à la première visite
        let mut c = cfg();
        c.new_member_read_days = 0;
        let b = user("b", false);
        assert_eq!(unread(&svc_me(&s, &c, &b, NOW).unwrap(), "annonces"), 0);
        // lire jusqu'à la dernière annonce vide le compteur et le bandeau, sans second appel à /me
        let last = s.max_id().unwrap();
        let r = svc_read(&s, &cfg(), &a, "annonces", last, NOW).unwrap();
        assert_eq!(unread(&r["me"], "annonces"), 0);
        assert!(r["me"]["latest_announcement"].is_null());
        assert_eq!(r["ok"], true);
    }

    #[test]
    fn messages_and_reads_carry_the_fresh_unread_state() {
        let s = store();
        let (m, a) = (user("mod", true), user("a", false));
        let first = send(&s, &m, "annonces", "une", NOW - 20 * DAY).unwrap();
        svc_me(&s, &cfg(), &a, NOW).unwrap(); // première visite : l'ancienne annonce est lue
        send(&s, &m, "annonces", "deux", NOW).unwrap();
        send(&s, &m, "entraide", "bienvenue", NOW + 1).unwrap();
        let q = ListQuery {
            channel: "entraide".into(),
            after: Some(0),
            before: None,
            limit: Some(50),
        };
        let v = svc_list(&s, &cfg(), &a, &q, NOW + 2).unwrap();
        assert_eq!(unread(&v["me"], "annonces"), 1);
        assert_eq!(unread(&v["me"], "entraide"), 1, "avant la lecture");
        let r = svc_read(&s, &cfg(), &a, "entraide", first.id + 2, NOW + 3).unwrap();
        assert_eq!(unread(&r["me"], "entraide"), 0);
        assert_eq!(
            unread(&r["me"], "annonces"),
            1,
            "l'autre salon n'a pas bougé"
        );
    }

    #[test]
    fn sending_checks_the_text_and_the_pace() {
        let s = store();
        let a = user("a", false);
        let mut c = cfg();
        c.min_gap_secs = 3;
        let mut limiter = RateLimiter::default();
        let mut go = |body: &str, at: i64| svc_send(&s, &c, &mut limiter, &a, "entraide", body, at);
        assert_eq!(code(&go("   ", NOW)), 400, "message vide");
        assert_eq!(code(&go(&"é".repeat(2001), NOW)), 400, "message trop long");
        assert_eq!(code(&go("un", NOW)), 200);
        assert_eq!(refusal(go("deux", NOW + 1)).0, 429, "trop rapproché");
        assert_eq!(code(&go("trois", NOW + 3)), 200);
        assert_eq!(
            s.list(&Channel::Entraide, None, None, 50).unwrap().len(),
            2,
            "le message refusé n'est pas écrit"
        );
    }

    #[test]
    fn rooms_and_permissions_in_me_follow_the_role() {
        let s = store();
        let (m, a) = (user("mod", true), user("a", false));
        let (me_m, me_a) = (
            svc_me(&s, &cfg(), &m, NOW).unwrap(),
            svc_me(&s, &cfg(), &a, NOW).unwrap(),
        );
        let can_post = |me: &Value| -> Vec<(String, bool)> {
            me["channels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    (
                        c["key"].as_str().unwrap().to_string(),
                        c["can_post"].as_bool().unwrap(),
                    )
                })
                .collect()
        };
        assert_eq!(
            can_post(&me_a),
            [("annonces".into(), false), ("entraide".into(), true)]
        );
        assert_eq!(
            can_post(&me_m),
            [("annonces".into(), true), ("entraide".into(), true)]
        );
        assert_eq!(me_a["private"]["key"], "prive:a");
        assert_eq!(me_a["user"]["moderator"], false);
        assert_eq!(me_m["user"]["moderator"], true);
        assert_eq!(me_a["limits"]["max_chars"], 2000);
        assert_eq!(me_a["limits"]["delete_own_within_secs"], 900);
    }
}
