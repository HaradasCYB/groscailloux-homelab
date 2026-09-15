//! API du tchat (`/chat/*`), publiée par NPM sous `/gc-chat/` sur l'adresse de Jellyfin (même
//! origine que l'interface, pas de CORS). Chaque requête porte le jeton de session Jellyfin du
//! membre, vérifié par `/Users/Me` (cache mémoire 5 min, jamais écrit ni journalisé).
//! Mails : récapitulatif aux modérateurs (entraide + privé, au plus un par intervalle) et annonce
//! aux membres sur demande. Règles et stockage : `homelab_core::chat`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use homelab_core::chat::{self, Channel, ChatStore, ChatUser, RateLimiter};
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
    limiter: Arc<Mutex<RateLimiter>>,
}

type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "error": msg })))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Exécute une opération SQLite hors du runtime async.
async fn db<T, F>(st: &ChatState, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&ChatStore) -> anyhow::Result<T> + Send + 'static,
{
    let store = st.store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne"))?
        .map_err(|e| {
            warn!(task = "chat", error = %e, "database error");
            err(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne")
        })
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
        limiter: Arc::new(Mutex::new(RateLimiter::default())),
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
                    warn!(task = "chat", error = %e, "jellyfin Users/Me failed");
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

fn channel(key: &str) -> ApiResult<Channel> {
    Channel::parse(key).ok_or_else(|| err(StatusCode::BAD_REQUEST, "salon inconnu"))
}

async fn me(State(st): State<ChatState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let own = Channel::Private(u.id.clone());
    let uc = u.clone();
    let (unread, private_unread, latest, latest_private) = db(&st, move |s| {
        let mut unread = Vec::new();
        for ch in Channel::PUBLIC {
            unread.push((ch.key(), s.unread(&uc.id, &ch)?));
        }
        let private_unread = if uc.moderator {
            s.private_threads(&uc.id)?.iter().map(|t| t.unread).sum()
        } else {
            s.unread(&uc.id, &own)?
        };
        // dernière annonce non lue (bannière de l'accueil)
        let latest = if unread[0].1 > 0 {
            s.list(&Channel::Annonces, None, None, 1)?
                .into_iter()
                .next()
                .filter(|m| !m.deleted)
        } else {
            None
        };
        // message privé de l'admin non lu (bannière de l'accueil, prioritaire sur l'annonce)
        let latest_private = if uc.moderator {
            None
        } else {
            s.latest_unread_from_moderator(&uc.id, &own)?
        };
        Ok((unread, private_unread, latest, latest_private))
    })
    .await?;
    let labels = [
        ("annonces", "Annonces"),
        ("entraide", "Entraide"),
        ("discussion", "Discussion"),
    ];
    let channels: Vec<Value> = labels
        .iter()
        .zip(unread)
        .map(|((key, label), (_, n))| {
            json!({
                "key": key,
                "label": label,
                "can_post": chat::can_post(&u, &Channel::parse(key).expect("salon connu")),
                "unread": n,
            })
        })
        .collect();
    let cfg = &st.ctx.cfg.chat;
    Ok(Json(json!({
        "user": { "id": u.id, "name": u.name, "moderator": u.moderator },
        "channels": channels,
        "private": { "key": format!("prive:{}", u.id), "unread": private_unread },
        "latest_announcement": latest,
        "latest_private": latest_private,
        "limits": { "max_chars": cfg.max_chars, "delete_own_within_secs": cfg.delete_own_within_mins * 60 },
    })))
}

#[derive(Deserialize)]
struct ListQuery {
    channel: String,
    after: Option<i64>,
    before: Option<i64>,
    limit: Option<usize>,
}

async fn list(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let ch = channel(&q.channel)?;
    if !chat::can_read(&u, &ch) {
        return Err(err(StatusCode::FORBIDDEN, "salon réservé"));
    }
    let limit = q.limit.unwrap_or(50).min(100);
    let msgs = db(&st, move |s| s.list(&ch, q.after, q.before, limit)).await?;
    Ok(Json(json!({ "messages": msgs, "limit": limit })))
}

#[derive(Deserialize)]
struct SendBody {
    channel: String,
    body: String,
    #[serde(default)]
    email_members: bool,
}

async fn send(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<SendBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let ch = channel(&b.channel)?;
    if !chat::can_post(&u, &ch) {
        return Err(err(StatusCode::FORBIDDEN, "seul l'admin publie ici"));
    }
    let cfg = &st.ctx.cfg.chat;
    let text =
        chat::validate_body(&b.body, cfg.max_chars).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    if !st.limiter.lock().await.check(&u.id, now(), cfg) {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "doucement : attends quelques secondes",
        ));
    }
    let (uc, chc, t) = (u.clone(), ch.clone(), text.clone());
    let msg = db(&st, move |s| s.insert(&chc, &uc, &t, now())).await?;
    info!(task = "chat", channel = %msg.channel, author = %u.name, chars = text.chars().count(), "message posted");
    if b.email_members && u.moderator && ch == Channel::Annonces {
        let st2 = st.clone();
        tokio::spawn(async move { mail_members(st2, u, text).await });
    }
    Ok(Json(json!({ "message": msg })))
}

async fn remove(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let m = db(&st, move |s| s.get(id))
        .await?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "message introuvable"))?;
    let ch = channel(&m.channel)?;
    let window = st.ctx.cfg.chat.delete_own_within_mins * 60;
    if !chat::can_read(&u, &ch) || !chat::can_delete(&u, &m, now(), window) {
        return Err(err(StatusCode::FORBIDDEN, "suppression impossible"));
    }
    let by = u.id.clone();
    db(&st, move |s| s.mark_deleted(id, &by, now())).await?;
    info!(task = "chat", id, by = %u.name, "message deleted");
    Ok(Json(json!({ "deleted": id })))
}

#[derive(Deserialize)]
struct ReadBody {
    channel: String,
    last_id: i64,
}

async fn read(
    State(st): State<ChatState>,
    headers: HeaderMap,
    Json(b): Json<ReadBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let ch = channel(&b.channel)?;
    if !chat::can_read(&u, &ch) {
        return Err(err(StatusCode::FORBIDDEN, "salon réservé"));
    }
    db(&st, move |s| s.mark_read(&u.id, &ch, b.last_id)).await?;
    Ok(Json(json!({ "ok": true })))
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
        warn!(task = "chat", error = %e, "members unreadable");
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
        warn!(task = "chat", error = %e, "members unreadable");
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
    if !st.limiter.lock().await.check(&u.id, now(), cfg) {
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
        Ok(())
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
            warn!(task = "chat", error = %e, "direct mail: Jellyseerr unreadable");
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
                warn!(task = "chat", user = %name, error = %e, "direct mail failed");
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
                warn!(task = "chat", error = %e, "moderator mail failed");
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
            warn!(task = "chat", error = %e, "announcement mail: users unreadable");
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
                warn!(task = "chat", user = %name, error = %e, "announcement mail failed");
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
