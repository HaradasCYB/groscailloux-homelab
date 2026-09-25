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
use homelab_core::requests_progress::{self as rp, QueueSummary};
use homelab_core::state::now;
use homelab_core::subscription_ops as ops;
use homelab_core::subscriptions::{self as subs, Status};
use homelab_core::welcome;
use homelab_core::TaskContext;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio::sync::Mutex;
use tracing::{info, warn};

const APP_JS: &str = include_str!("../assets/compte/app.js");
const AUTH_TTL: Duration = Duration::from_secs(300);

/// (type, tmdb) → (instant, {title, original_title, year, poster}).
type DetailsCache = HashMap<(String, i64), (Instant, Value)>;
/// (côté, id série) → saison → (fichiers, épisodes diffusés, prochaine diffusion).
type SeasonInfo = HashMap<(&'static str, i64), HashMap<i64, (i64, i64, Option<String>)>>;

#[derive(Clone)]
pub struct SubsState {
    pub ctx: Arc<TaskContext>,
    auth: Arc<Mutex<HashMap<String, (Me, Instant)>>>,
    last: Arc<Mutex<HashMap<String, Instant>>>,
    /// Avancement des demandes, partagé entre tous les membres (20 s).
    requests_cache: Arc<Mutex<Option<(Instant, Value)>>>,
    /// Titre, année et affiche TMDB par (type, tmdb), pour apparier les cartes de Jellyfin Enhanced (1 h).
    details_cache: Arc<Mutex<DetailsCache>>,
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
        requests_cache: Arc::new(Mutex::new(None)),
        details_cache: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/paypal/webhook", post(webhook))
        .route("/premium/lier", post(link_from_page))
        .route("/compte/app.js", get(app_js))
        .route("/compte/api/me", get(me))
        .route("/compte/api/devices/{id}", delete(logout_device))
        .route("/compte/api/password", post(password_link))
        .route("/compte/api/language", post(set_language))
        .route("/compte/api/original", get(original_language))
        .route("/compte/api/requests", get(requests_progress))
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
        "language": language_mode(&st, &u.id).await,
    })))
}

/// `fr` (piste française d'abord) ou `vo` (audio d'origine, sous-titres français toujours), lu dans la
/// configuration Jellyfin du compte. VO = sous-titres `Always` et langue audio vide (ancien mode, avant le
/// 2026-09-25) ou `[accounts] vo_audio_language`.
async fn language_mode(st: &SubsState, user_id: &str) -> &'static str {
    match st.ctx.jellyfin.user(user_id).await {
        Ok(u) => {
            let cfg = u.get("Configuration").cloned().unwrap_or(Value::Null);
            let audio = cfg
                .get("AudioLanguagePreference")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mode = cfg
                .get("SubtitleMode")
                .and_then(Value::as_str)
                .unwrap_or("");
            if mode == "Always"
                && (audio.is_empty() || audio == st.ctx.cfg.accounts.vo_audio_language)
            {
                "vo"
            } else {
                "fr"
            }
        }
        Err(_) => "fr",
    }
}

#[derive(Deserialize)]
struct LanguageBody {
    mode: String,
}

/// POST /compte/api/language {mode: "fr"|"vo"} : préférences audio/sous-titres du compte.
async fn set_language(
    State(st): State<SubsState>,
    headers: HeaderMap,
    Json(b): Json<LanguageBody>,
) -> ApiResult<Json<Value>> {
    let u = auth(&st, &headers).await?;
    let a = &st.ctx.cfg.accounts;
    let r = match b.mode.as_str() {
        "fr" => {
            st.ctx
                .jellyfin
                .set_language_prefs(
                    &u.id,
                    &a.audio_language,
                    &a.subtitle_language,
                    &a.subtitle_mode,
                    false,
                )
                .await
        }
        "vo" => {
            st.ctx
                .jellyfin
                .set_language_prefs(
                    &u.id,
                    &a.vo_audio_language,
                    &a.subtitle_language,
                    "Always",
                    true,
                )
                .await
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "mode inconnu")),
    };
    r.map_err(|e| {
        warn!(task = "subs", user = %u.name, error = %e, "language prefs failed");
        err(StatusCode::BAD_GATEWAY, "Jellyfin injoignable")
    })?;
    info!(task = "subs", user = %u.name, mode = %b.mode, "language mode set from Mon compte");
    Ok(Json(json!({ "ok": true, "language": b.mode })))
}

/// GET /compte/api/requests : avancement de chaque demande Jellyseerr en cours (toutes, la vue
/// d'ensemble est ouverte aux membres), calculé côté serveur et mis en cache 20 s.
async fn requests_progress(
    State(st): State<SubsState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let _u = auth(&st, &headers).await?;
    {
        let c = st.requests_cache.lock().await;
        if let Some((at, v)) = c.as_ref() {
            if at.elapsed() < Duration::from_secs(20) {
                return Ok(Json(v.clone()));
            }
        }
    }
    let v = build_requests_progress(&st).await.map_err(|e| {
        warn!(task = "subs", error = %e, "requests progress failed");
        err(StatusCode::BAD_GATEWAY, "services injoignables")
    })?;
    *st.requests_cache.lock().await = Some((Instant::now(), v.clone()));
    Ok(Json(v))
}

fn side_of(service_id: i64, vps_id: i64) -> &'static str {
    if service_id == vps_id {
        "vps"
    } else {
        "seedbox"
    }
}

/// Titre (fr), année et chemin d'affiche TMDB d'une demande, mis en cache 1 h.
async fn details(st: &SubsState, kind: &str, tmdb: i64) -> Value {
    let key = (kind.to_string(), tmdb);
    if let Some((at, v)) = st.details_cache.lock().await.get(&key) {
        if at.elapsed() < Duration::from_secs(3600) {
            return v.clone();
        }
    }
    let d = if kind == "tv" {
        st.ctx.jellyseerr.tv_details(tmdb).await
    } else {
        st.ctx.jellyseerr.movie_details(tmdb).await
    };
    let v = match d {
        Ok(d) => {
            let title = d
                .get("name")
                .or_else(|| d.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let original = d
                .get("originalName")
                .or_else(|| d.get("originalTitle"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let date = d
                .get("firstAirDate")
                .or_else(|| d.get("releaseDate"))
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({
                "title": title,
                "original_title": original,
                "year": date.get(..4).unwrap_or(""),
                "poster": d.get("posterPath").and_then(Value::as_str).unwrap_or(""),
                "lang": d.get("originalLanguage").and_then(Value::as_str).unwrap_or(""),
            })
        }
        Err(_) => {
            json!({ "title": "", "original_title": "", "year": "", "poster": "", "lang": "" })
        }
    };
    st.details_cache
        .lock()
        .await
        .insert(key, (Instant::now(), v.clone()));
    v
}

async fn build_requests_progress(st: &SubsState) -> anyhow::Result<Value> {
    let ctx = &st.ctx;
    let t = now();
    let cfg = &ctx.cfg;
    let requests = ctx.jellyseerr.all_requests().await?;
    // files d'attente des 4 Arrs, indexées par (côté, type, id Arr)
    let mut queues: HashMap<(&'static str, &'static str, i64), Vec<QueueSummary>> = HashMap::new();
    let sides: Vec<(
        &'static str,
        &homelab_core::clients::ArrClient,
        &'static str,
    )> = {
        let mut v = vec![("vps", &ctx.sonarr, "tv"), ("vps", &ctx.radarr, "movie")];
        if let Some(s) = ctx.seedbox_sonarr.as_ref() {
            v.push(("seedbox", s, "tv"));
        }
        if let Some(r) = ctx.seedbox_radarr.as_ref() {
            v.push(("seedbox", r, "movie"));
        }
        v
    };
    // hashs déjà suivis par un Arr : un torrent étiqueté `homelab:` qui serait aussi dans une file Arr
    // ne doit pas compter deux fois
    let mut arr_hashes: HashSet<String> = HashSet::new();
    for (side, arr, kind) in &sides {
        let recs = match arr.queue_records().await {
            Ok(r) => r,
            Err(e) => {
                warn!(task = "subs", side, kind, error = %e, "queue unreadable");
                continue;
            }
        };
        for r in recs {
            if let Some(h) = r.get("downloadId").and_then(Value::as_str) {
                arr_hashes.insert(h.to_ascii_lowercase());
            }
            let id = if *kind == "tv" {
                r.get("seriesId")
            } else {
                r.get("movieId")
            }
            .and_then(Value::as_i64)
            .unwrap_or(0);
            if id == 0 {
                continue;
            }
            queues
                .entry((side, kind, id))
                .or_default()
                .push(QueueSummary {
                    season: r.get("seasonNumber").and_then(Value::as_i64),
                    size: r.get("size").and_then(Value::as_f64).unwrap_or(0.0),
                    size_left: r.get("sizeleft").and_then(Value::as_f64).unwrap_or(0.0),
                    time_left_secs: r
                        .get("timeleft")
                        .and_then(Value::as_str)
                        .and_then(rp::parse_timeleft),
                    tracked_state: r
                        .get("trackedDownloadState")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
        }
    }
    // torrents lancés par la plateforme elle-même (étiquette `homelab:`) : côté seedbox, c'est le SEUL chemin
    // (pas de Prowlarr là-bas, series_search/movie_search ajoutent au qBittorrent), et Sonarr/Radarr ne les
    // voient pas dans leur file ; `torrent_import` les range une fois finis
    let import_records = ctx.state.read(|s| s.torrent_import.clone()).await;
    let mut qbits: Vec<(&'static str, &homelab_core::clients::QbitClient)> =
        vec![("vps", &ctx.qbit)];
    if let Some(q) = ctx.seedbox_qbit.as_ref() {
        qbits.push(("seedbox", q));
    }
    for (side, qbit) in qbits {
        let torrents = match qbit.torrents().await {
            Ok(t) => t,
            Err(e) => {
                warn!(task = "subs", side, error = %e, "qbittorrent unreadable");
                continue;
            }
        };
        for t in torrents {
            let Some((movie, id, season)) = rp::homelab_tag(&t.tags) else {
                continue;
            };
            if arr_hashes.contains(&t.hash.to_ascii_lowercase()) {
                continue;
            }
            let outcome = import_records
                .get(&homelab_core::tasks::torrent_import::state_key(
                    side, &t.hash,
                ))
                .map(|r| r.outcome.as_str());
            if let Some(q) = rp::from_torrent(season, t.size, t.progress, t.eta, outcome) {
                queues
                    .entry((side, if movie { "movie" } else { "tv" }, id))
                    .or_default()
                    .push(q);
            }
        }
    }
    // fichiers déjà présents côté Arr (séries : par saison)
    let mut series_files: HashMap<(&'static str, i64), HashSet<i64>> = HashMap::new();
    // saisons par série : numéro → (fichiers, épisodes diffusés, prochaine diffusion)
    let mut season_info: SeasonInfo = HashMap::new();
    let mut movie_files: HashSet<(&'static str, i64)> = HashSet::new();
    // films connus de l'Arr : (côté, id) → (isAvailable, date de sortie numérique/physique la plus proche)
    // (disponible selon Radarr, date numérique/physique, attente de la VOD pour un film français : salle, VOD estimée)
    #[allow(clippy::type_complexity)]
    let mut movie_info: HashMap<
        (&'static str, i64),
        (
            bool,
            Option<String>,
            Option<(chrono::NaiveDate, chrono::NaiveDate)>,
        ),
    > = HashMap::new();
    let vod_days = ctx.cfg.tasks.movie_search.min_days_after_cinema;
    for (side, arr, kind) in &sides {
        if *kind == "tv" {
            if let Ok(list) = arr.series().await {
                for s in list {
                    let id = s.get("id").and_then(Value::as_i64).unwrap_or(0);
                    let seasons: HashSet<i64> = s
                        .get("seasons")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter(|x| {
                                    x.pointer("/statistics/episodeFileCount")
                                        .and_then(Value::as_i64)
                                        .unwrap_or(0)
                                        > 0
                                })
                                .filter_map(|x| x.get("seasonNumber").and_then(Value::as_i64))
                                .collect()
                        })
                        .unwrap_or_default();
                    series_files.insert((side, id), seasons);
                    let mut per: HashMap<i64, (i64, i64, Option<String>)> = HashMap::new();
                    for x in s
                        .get("seasons")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        let n = x.get("seasonNumber").and_then(Value::as_i64).unwrap_or(-1);
                        let st = x.get("statistics").cloned().unwrap_or(Value::Null);
                        per.insert(
                            n,
                            (
                                st.get("episodeFileCount")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(0),
                                st.get("episodeCount").and_then(Value::as_i64).unwrap_or(0),
                                st.get("nextAiring")
                                    .and_then(Value::as_str)
                                    .map(|d| d[..10.min(d.len())].to_string()),
                            ),
                        );
                    }
                    season_info.insert((side, id), per);
                }
            }
        } else if let Ok(list) = arr.movies().await {
            for m in list {
                let Some(id) = m.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                if m.get("hasFile").and_then(Value::as_bool).unwrap_or(false) {
                    movie_files.insert((side, id));
                }
                let avail = m
                    .get("isAvailable")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let release = ["digitalRelease", "physicalRelease"]
                    .iter()
                    .filter_map(|k| m.get(*k).and_then(Value::as_str))
                    .map(|d| d[..10.min(d.len())].to_string())
                    .min();
                let vod = homelab_core::tasks::movie_search::awaiting_vod(
                    &m,
                    chrono::Utc::now(),
                    vod_days,
                );
                movie_info.insert((side, id), (avail, release, vod));
            }
        }
    }
    let (unknown_series, movie_search) = ctx
        .state
        .read(|s| (s.unknown_series.clone(), s.movie_search.clone()))
        .await;
    let vps_id = cfg.seedbox.jellyseerr_vps_sonarr_id;
    let scan_delay = cfg.tasks.seedbox_refresh.interval_secs as i64;
    // import Arr (~2 min) ou passage de torrent_import pour un torrent étiqueté
    let import_allowance = (cfg.tasks.torrent_import.interval_secs as i64).max(120);
    let mut out = Vec::new();
    for r in &requests {
        let status = r.get("status").and_then(Value::as_i64).unwrap_or(0);
        if status != 2 {
            continue; // 1 en attente d'approbation, 3 refusée : pas de progression à montrer
        }
        let media = r.get("media").cloned().unwrap_or(Value::Null);
        let media_status = media.get("status").and_then(Value::as_i64).unwrap_or(0);
        let kind = r.get("type").and_then(Value::as_str).unwrap_or("movie");
        let tmdb = media.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
        let ext = media
            .get("externalServiceId")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let side = side_of(
            media.get("serviceId").and_then(Value::as_i64).unwrap_or(1),
            vps_id,
        );
        let seasons: Vec<i64> = r
            .get("seasons")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.get("seasonNumber").and_then(Value::as_i64))
                    .collect()
            })
            .unwrap_or_default();
        let arr_name = if side == "vps" {
            if kind == "tv" {
                "sonarr"
            } else {
                "radarr"
            }
        } else if kind == "tv" {
            "sonarr-seedbox"
        } else {
            "radarr-seedbox"
        };
        // séries : saisons demandées encore incomplètes (diffusées) et saisons pas encore diffusées
        let mut missing_seasons: Vec<i64> = Vec::new();
        let mut unaired: Vec<(i64, Option<String>)> = Vec::new();
        let (has_file, available, search, uncovered) = if kind == "tv" {
            let info = season_info.get(&(side, ext)).cloned().unwrap_or_default();
            let wanted: Vec<i64> = seasons.iter().copied().filter(|s| *s != 0).collect();
            for sn in &wanted {
                match info.get(sn) {
                    Some((_, aired, next)) if *aired == 0 => unaired.push((*sn, next.clone())),
                    Some((files, aired, _)) if files < aired => missing_seasons.push(*sn),
                    Some(_) => {}
                    None => missing_seasons.push(*sn),
                }
            }
            let complete = !wanted.is_empty() && missing_seasons.is_empty() && unaired.is_empty();
            let rec = missing_seasons
                .iter()
                .filter_map(|s| unknown_series.get(&format!("{arr_name}:{ext}:{s}")))
                .max_by_key(|r| r.at)
                .cloned();
            let unc = missing_seasons
                .iter()
                .filter_map(|s| unknown_series.get(&format!("{arr_name}:{ext}:{s}")))
                .any(|r| !r.uncovered.is_empty());
            (
                complete,
                media_status == 5 || (media_status == 4 && complete),
                rec,
                unc,
            )
        } else {
            let hf = movie_files.contains(&(side, ext));
            (
                hf,
                media_status == 5,
                movie_search.get(&format!("{arr_name}:{ext}")).cloned(),
                false,
            )
        };
        let mut queue = queues
            .get(&(side, if kind == "tv" { "tv" } else { "movie" }, ext))
            .cloned()
            .unwrap_or_default();
        if kind == "tv" && !missing_seasons.is_empty() {
            // les éléments de file portent la saison ; on ne garde que celles qui manquent encore
            let ms: HashSet<i64> = missing_seasons.iter().copied().collect();
            queue.retain(|q| q.season.map(|sn| ms.contains(&sn)).unwrap_or(true));
        }
        let ss = &cfg.tasks.series_search;
        let ms = &cfg.tasks.movie_search;
        let (retry_h, grabbed_h, error_h) = if kind == "tv" {
            (
                ss.retry_after_hours,
                ss.grabbed_retry_hours,
                ss.error_retry_hours,
            )
        } else {
            (ms.retry_after_hours, 168, ms.error_retry_hours)
        };
        let created = r
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(homelab_core::clients::paypal::parse_time)
            .unwrap_or(t);
        let mut p = rp::progress(
            &queue,
            has_file,
            available,
            search.as_ref().map(|r| (r, retry_h, grabbed_h, error_h)),
            t,
            import_allowance,
            scan_delay,
            uncovered,
        );
        if p.stage == rp::Stage::Search && queue.is_empty() && !has_file {
            if kind == "movie" {
                match movie_info.get(&(side, ext)) {
                    None => {
                        p.label =
                            "Fiche absente côté téléchargement : l'administrateur doit la refaire"
                                .into()
                    }
                    Some((true, _, Some((cinema, vod)))) => {
                        let d =
                            |n: &chrono::NaiveDate| rp::date_fr(&n.format("%Y-%m-%d").to_string());
                        p.label = format!(
                            "En salle depuis le {} · VOD vers le {}, récupéré dès sa sortie",
                            d(cinema),
                            d(vod)
                        );
                    }
                    Some((false, release, _)) => {
                        p.label = match release {
                            Some(d) => format!(
                                "Pas encore sorti en numérique (prévu le {})",
                                rp::date_fr(d)
                            ),
                            None => "Pas encore sorti en numérique".into(),
                        }
                    }
                    Some((true, _, _)) if search.is_none() && t - created > 6 * 3600 => {
                        p.label =
                            "Recherche régulière en cours, rien de trouvé pour l'instant".into();
                    }
                    _ => {}
                }
            } else if kind == "tv" && !season_info.contains_key(&(side, ext)) {
                p.label =
                    "Fiche absente côté téléchargement : l'administrateur doit la refaire".into();
            } else if missing_seasons.is_empty() && !unaired.is_empty() {
                let (sn, next) = &unaired[0];
                p.label = match next {
                    Some(d) => format!(
                        "Saison {sn} pas encore diffusée (prochain épisode le {})",
                        rp::date_fr(d)
                    ),
                    None => format!("Saison {sn} pas encore diffusée"),
                };
            } else if search.is_none() && t - created > 6 * 3600 {
                p.label = "Recherche régulière en cours, rien de trouvé pour l'instant".into();
            }
        }
        let wanted_n = seasons.iter().filter(|s| **s != 0).count();
        if kind == "tv"
            && !missing_seasons.is_empty()
            && missing_seasons.len() < wanted_n
            && p.stage != rp::Stage::Available
        {
            p.label = format!(
                "Saison{} {} : {}",
                if missing_seasons.len() > 1 { "s" } else { "" },
                missing_seasons
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                p.label
            );
        }
        let d = details(st, kind, tmdb).await;
        out.push(json!({
            "request_id": r.get("id"),
            "tmdb_id": tmdb,
            "media_type": kind,
            "title": d.get("title"),
            "original_title": d.get("original_title"),
            "year": d.get("year"),
            "poster": d.get("poster"),
            "seasons": seasons,
            "requested_by": r.pointer("/requestedBy/displayName"),
            "stage": p.stage,
            "percent": p.percent,
            "eta_secs": p.eta_secs,
            "label": p.label,
        }));
    }
    Ok(json!({ "at": t, "requests": out }))
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

#[derive(Deserialize)]
struct OriginalQuery {
    kind: String,
    tmdb: i64,
}

/// GET /compte/api/original?kind=movie|tv&tmdb=<id> : langue d'origine (ISO 639-1, TMDB via Jellyseerr, cache 1 h).
/// Sert au mode « VO » du script Mon compte : Jellyfin ne connaît pas la langue d'origine d'un titre, et un
/// film français doublé en anglais ne doit pas passer en anglais.
async fn original_language(
    State(st): State<SubsState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<OriginalQuery>,
) -> ApiResult<Json<Value>> {
    let _u = auth(&st, &headers).await?;
    let kind = if q.kind == "tv" { "tv" } else { "movie" };
    let d = details(&st, kind, q.tmdb).await;
    Ok(Json(
        json!({ "lang": d.get("lang").cloned().unwrap_or(Value::Null) }),
    ))
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
    let event_id = facts.event_id.clone();
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
            warn!(task = "subs", error = %e, "webhook : traitement en échec, relance de PayPal attendue");
            if let Err(e) = st.ctx.subs.forget_paypal_event(&event_id) {
                warn!(task = "subs", error = %e, "webhook : événement non oublié, la relance sera ignorée");
            }
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
