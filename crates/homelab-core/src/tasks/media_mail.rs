//! Mails « ta demande est disponible » (2026-10-15) : quand un film demandé dans Jellyseerr devient disponible, ou une
//! saison demandée, le membre qui l'a demandé reçoit un mail en français (gabarit [`crate::available_mail`] : fond,
//! affiche, puces, un bouton vers la fiche Jellyfin). Remplace les mails de Jellyseerr (anglais mêlé de français, logo
//! SVG cassé dans Gmail, bouton vers Jellyseerr au lieu de Jellyfin), que l'admin coupe une fois ceux-ci validés.
//!
//! **Détection** ([`ready_events`]) : toutes les demandes et toutes les fiches média à chaque passage — la fin d'une
//! saison ne change pas la date de sa demande, un filtre par date en manquerait. Film : média disponible
//! (`media.status = 5`), clé `r<id>`. Série : chaque saison demandée dont la demande de saison est terminée
//! (`COMPLETED`) **et** dont la saison du média est disponible : Jellyseerr termine aussi la demande d'une saison
//! supprimée (sa synchro de disponibilité supprime une saison incomplète introuvable). Clé `r<id>:s<n>` : une demande
//! de deux saisons dont une seule est arrivée donne un mail maintenant et un autre plus tard (Jellyseerr attendait les
//! deux). Demande de série en échec : ses saisons arrivées comptent. La saison 0 est ignorée (Jellyseerr ne la termine
//! jamais). Demandes 4K, en attente ou refusées, films en échec, médias supprimés : ignorés.
//!
//! **Jamais deux fois, jamais l'arriéré** :
//! - premier passage = référence : tout ce qui est déjà disponible est noté `baseline`, aucun mail ;
//! - une clé notée n'est jamais réévaluée, sauf en attente (`pending`) ou en échec d'envoi (`error`) ; une clé notée
//!   pour un autre titre que sa demande actuelle (base Jellyseerr restaurée, ids réutilisés) est une nouvelle clé ;
//! - disponible depuis plus de `max_age_hours` selon Jellyseerr (daemon arrêté) : `stale`, jamais envoyée ; plus de
//!   `max_gap_mins` sans passage noté dans l'état (état restauré d'une sauvegarde, daemon longtemps arrêté) : ce qui
//!   est arrivé pendant l'écart est `stale` aussi (l'état perdu l'avait peut-être envoyé), et ce qui attendait avant
//!   lui (`pending`, `error`) passe en `sent_uncertain`, sans mail ni alerte (l'état perdu l'a très probablement
//!   envoyé) ; une clé en attente qui dépasse `max_age_hours` sans pouvoir partir est abandonnée (`gave_up`, alerte) ;
//! - `send_to_members = false` : tout le chemin tourne (destinataire, choix, visibilité, rendu) et les clés sont
//!   notées `logged` au moment où le mail serait parti : passer à `true` n'envoie que ce qui arrive ensuite (le
//!   premier passage avec envoi note aussi `logged` ce qui attendait encore : Jellyseerr l'a déjà annoncé) ;
//! - envoi : clés notées `sending` sur disque **avant** `curl`. Un passage interrompu pendant l'envoi (arrêt,
//!   `RUN_TIMEOUT`) les retrouve ainsi au passage suivant et les passe en `sent_uncertain`, jamais renvoyées (au
//!   plus un mail) ; de même un `curl` en échec après avoir passé le message entier ([`mail::MaybeSent`]). Un échec
//!   net : nouvel essai après `retry_mins` (15, 60, 240 min), puis `gave_up` et une alerte ;
//! - alerte d'abandon ([`queue_alert`]) : sa ligne est notée dans `State::media_mail_alerts` **dans la même écriture**
//!   que `gave_up`, puis effacée seulement quand l'alerte est partie ([`alerts::retry_later`]) ; une seule alerte par
//!   passage, qui reprend tout ce qui attend encore (passages précédents compris), même si Jellyseerr ne répond pas ;
//! - demande absente de la liste : `gone` seulement si Jellyseerr répond 404 sur la demande (une suppression pendant
//!   la lecture fait glisser les pages).
//!
//! **Regroupement par membre** ([`due_groups`]) : les titres d'un membre attendent `settle_mins` sans nouvelle
//! arrivée, `max_hold_mins` au plus (les 10 saisons d'une intégrale terminées en 15 min = un mail ; 8 films demandés
//! ensemble = un récapitulatif). Un titre = un film, ou les saisons d'une série réunies (même plusieurs demandes). Une
//! demande réattribuée dans Jellyseerr suit son nouveau demandeur. Un titre illisible à ce passage reste en attente,
//! les autres partent sans lui.
//!
//! **Destinataire** : l'adresse Jellyseerr du demandeur (sinon `no_email`) ; pas de mail s'il a décoché
//! « disponible » dans ses notifications par mail Jellyseerr ou s'il est dans `MEDIA_MAIL_OPTOUT` (adresse, pseudo ou
//! id : `opted_out`), si son compte Jellyfin est suspendu (`suspended`) ou introuvable (`no_account`). Chaque titre
//! est relu **comme le membre** par son id (`GET /Items/{id}?userId=` : la liste `GET /Items?Ids=&UserId=` renvoie
//! aussi ce que le compte ne voit pas) : invisible pour lui (bibliothèque non partagée, fiche disparue), il attend
//! `max_hold_mins` puis `not_visible`, jamais envoyé. Images (fond, affiches) lues dans Jellyfin (`format=Jpg`), TMDB
//! en secours, et jointes au mail (`cid:`) ; un mail part sans l'image qui manque.
//!
//! Journal : id Jellyseerr et nom du membre, clés, objet ; jamais l'adresse. Sans `Reply-To` (une réponse arrive à
//! l'adresse d'envoi, comme les mails d'abonnement) ; `List-Unsubscribe` en `mailto:` vers l'adresse d'envoi.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::DateTime;
use serde_json::Value;
use tracing::{info, warn};

use super::onboard::valid_email;
use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::available_mail::{self, AvailableMail, Title, TitleKind};
use crate::config::{Config, MediaMail as Cfg};
use crate::context::TaskContext;
use crate::mail;
use crate::mail_layout::{fetch_image, Img, RenderedMail, POSTER_SLOTS};
use crate::state::{now, MediaMailPass, MediaMailRecord};

pub struct MediaMail;

const TASK: &str = "media_mail";

// Suites données à une clé (voir `MediaMailRecord::outcome`).
pub const PENDING: &str = "pending";
pub const ERROR: &str = "error";
pub const SENDING: &str = "sending";
pub const BASELINE: &str = "baseline";
pub const STALE: &str = "stale";
pub const LOGGED: &str = "logged";
pub const SENT: &str = "sent";
pub const SENT_UNCERTAIN: &str = "sent_uncertain";
pub const NO_EMAIL: &str = "no_email";
pub const NO_ACCOUNT: &str = "no_account";
pub const SUSPENDED: &str = "suspended";
pub const OPTED_OUT: &str = "opted_out";
pub const NOT_VISIBLE: &str = "not_visible";
pub const GONE: &str = "gone";
pub const GAVE_UP: &str = "gave_up";

/// Bit « disponible » (`MEDIA_AVAILABLE`) des types de notification de Jellyseerr.
pub const SEERR_MEDIA_AVAILABLE: i64 = 8;
const REQUEST_APPROVED: i64 = 2;
const REQUEST_FAILED: i64 = 4;
const REQUEST_COMPLETED: i64 = 5;
const MEDIA_AVAILABLE: i64 = 5;
const MEDIA_BLACKLISTED: i64 = 6;
const MEDIA_DELETED: i64 = 7;

/// Au-delà, le passage ne compose plus de nouveau mail (`RUN_TIMEOUT` = 10 min ; un envoi peut prendre 60 s et
/// chaque image 10 s) : les suivants attendent le passage d'après.
const RUN_BUDGET: Duration = Duration::from_secs(240);
const TMDB_IMAGES: &str = "https://image.tmdb.org/t/p/";

// ---------------------------------------------------------------------------------------------
// Détection (fonctions pures)
// ---------------------------------------------------------------------------------------------

/// Un film ou une saison demandés, disponibles selon Jellyseerr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub key: String,
    pub request: i64,
    /// `movie` ou `tv`.
    pub kind: &'static str,
    pub tmdb: i64,
    pub season: Option<u32>,
    /// Passage à « disponible » selon Jellyseerr (0 = inconnu : traité comme ancien).
    pub ready_at: i64,
    /// Demandeur (id Jellyseerr).
    pub user: i64,
}

fn ts(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// Saisons des médias Jellyseerr (`GET /api/v1/media` : la liste des demandes ne les porte pas) : id du média →
/// numéro de saison → (état, dernière mise à jour).
pub type MediaSeasons = HashMap<i64, HashMap<u32, (i64, i64)>>;

pub fn media_seasons(media: &[Value]) -> MediaSeasons {
    media
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_i64()?;
            let seasons = m
                .get("seasons")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| {
                    let n = u32::try_from(s.get("seasonNumber")?.as_u64()?).ok()?;
                    let status = s.get("status").and_then(Value::as_i64).unwrap_or(0);
                    Some((n, (status, ts(s.get("updatedAt")))))
                })
                .collect();
            Some((id, seasons))
        })
        .collect()
}

/// Films et saisons disponibles, lus dans la liste complète des demandes (`GET /api/v1/request?filter=all`) et les
/// saisons des médias ([`media_seasons`]).
///
/// Une saison compte quand sa demande de saison est terminée **et** que la saison du média est disponible :
/// Jellyseerr termine aussi la demande d'une saison **supprimée** (`MediaSubscriber` : saison `AVAILABLE` ou
/// `DELETED`, 3.5), ce que fait sa synchro de disponibilité pour une saison incomplète introuvable dans Sonarr et
/// Jellyfin. Disponible alors : sa date est la plus récente des deux. Demande de série en échec (`FAILED`) : ses
/// saisons arrivées comptent aussi (Jellyseerr les termine, et ne termine la demande qu'avec la dernière).
pub fn ready_events(requests: &[Value], seasons: &MediaSeasons) -> Vec<Ready> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for r in requests {
        let Some(id) = r.get("id").and_then(Value::as_i64) else {
            continue;
        };
        // la pagination glisse si une demande arrive pendant la lecture : une demande peut revenir deux fois
        if !seen.insert(id) {
            continue;
        }
        let status = r.get("status").and_then(Value::as_i64).unwrap_or(0);
        let accepted = match r.get("type").and_then(Value::as_str) {
            Some("tv") => matches!(
                status,
                REQUEST_APPROVED | REQUEST_FAILED | REQUEST_COMPLETED
            ),
            _ => matches!(status, REQUEST_APPROVED | REQUEST_COMPLETED),
        };
        if r.get("is4k").and_then(Value::as_bool).unwrap_or(false) || !accepted {
            continue;
        }
        let Some(media) = r.get("media") else {
            continue;
        };
        let media_status = media.get("status").and_then(Value::as_i64).unwrap_or(0);
        let (Some(tmdb), Some(user)) = (
            media.get("tmdbId").and_then(Value::as_i64),
            r.pointer("/requestedBy/id").and_then(Value::as_i64),
        ) else {
            continue;
        };
        match r.get("type").and_then(Value::as_str) {
            Some("movie") if media_status == MEDIA_AVAILABLE => {
                let ready_at = if status == REQUEST_COMPLETED {
                    ts(r.get("updatedAt"))
                } else {
                    ts(media.get("mediaAddedAt"))
                };
                out.push(Ready {
                    key: format!("r{id}"),
                    request: id,
                    kind: "movie",
                    tmdb,
                    season: None,
                    ready_at,
                    user,
                });
            }
            // l'état d'une série change avec chaque nouvelle demande (« en cours » dès qu'une autre saison est
            // demandée) : seule la demande de saison fait foi
            Some("tv") if !matches!(media_status, MEDIA_BLACKLISTED | MEDIA_DELETED) => {
                let of_media = media
                    .get("id")
                    .and_then(Value::as_i64)
                    .and_then(|m| seasons.get(&m));
                for s in r
                    .get("seasons")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(n) = s
                        .get("seasonNumber")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                    else {
                        continue;
                    };
                    if n == 0 || s.get("status").and_then(Value::as_i64) != Some(REQUEST_COMPLETED)
                    {
                        continue;
                    }
                    // saison du média supprimée, incomplète ou absente de la liste : pas (encore) disponible
                    let Some(&(MEDIA_AVAILABLE, available_at)) = of_media.and_then(|m| m.get(&n))
                    else {
                        continue;
                    };
                    out.push(Ready {
                        key: format!("r{id}:s{n}"),
                        request: id,
                        kind: "tv",
                        tmdb,
                        season: Some(n),
                        ready_at: ts(s.get("updatedAt")).max(available_at),
                        user,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn record(r: &Ready, now: i64, outcome: &str) -> MediaMailRecord {
    MediaMailRecord {
        at: now,
        seen_at: now,
        ready_at: r.ready_at,
        user: r.user,
        kind: r.kind.to_string(),
        tmdb: r.tmdb,
        outcome: outcome.to_string(),
        tries: 0,
        next_at: None,
    }
}

fn with_outcome(r: &MediaMailRecord, outcome: &str, now: i64) -> MediaMailRecord {
    MediaMailRecord {
        at: now,
        outcome: outcome.to_string(),
        next_at: None,
        ..r.clone()
    }
}

/// Premier passage : tout ce qui est disponible est noté `baseline` (jamais envoyé).
pub fn baseline(ready: &[Ready], now: i64) -> Vec<(String, MediaMailRecord)> {
    ready
        .iter()
        .map(|r| (r.key.clone(), record(r, now, BASELINE)))
        .collect()
}

/// Ce que le passage précédent (`State::media_mail_last_pass`) dit de l'état, pour ce passage-ci.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Continuity {
    /// L'état n'a rien noté depuis cette date pendant plus de `max_gap_mins` (état restauré d'une sauvegarde, daemon
    /// longtemps arrêté) : une clé devenue disponible depuis a peut-être été envoyée par l'état perdu → `stale`.
    pub gap_since: Option<i64>,
    /// Premier passage avec envoi après des passages sans envoi : les clés en attente ont déjà eu le mail de
    /// Jellyseerr, encore actif pendant ces passages → `logged` (sinon le membre recevrait les deux).
    pub switch_on: bool,
}

/// `max_gap_secs` : `[tasks.media_mail] max_gap_mins` en secondes.
pub fn continuity(
    last: Option<MediaMailPass>,
    now: i64,
    mode: Mode,
    max_gap_secs: i64,
) -> Continuity {
    Continuity {
        gap_since: last.map(|p| p.at).filter(|at| now - at > max_gap_secs),
        switch_on: mode == Mode::Send && !last.is_some_and(|p| p.send),
    }
}

/// Ce qu'un passage change avant de composer : nouvelles clés, envois interrompus, attentes trop longues.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub changes: BTreeMap<String, MediaMailRecord>,
    pub new_pending: usize,
    pub new_stale: usize,
    /// Nouvelles clés devenues disponibles pendant un écart de l'état ([`Continuity::gap_since`]) → `stale`.
    pub gap_stale: usize,
    /// `sending` retrouvées : passage précédent interrompu pendant l'envoi → `sent_uncertain`.
    pub uncertain: Vec<String>,
    /// `pending` / `error` notées avant un écart de l'état ([`Continuity::gap_since`]) : l'état perdu les a très
    /// probablement envoyées → `sent_uncertain`, sans mail (au plus un mail) ni alerte d'abandon.
    pub gap_held: Vec<String>,
    /// `pending` / `error` devenues disponibles il y a plus de `max_age_hours` → `gave_up`.
    pub expired: Vec<String>,
    /// `pending` / `error` au passage de bascule vers l'envoi ([`Continuity::switch_on`]) → `logged`.
    pub handed_over: Vec<String>,
    /// En attente, demandeur changé dans Jellyseerr (l'admin peut réattribuer une demande) : suit le nouveau.
    pub reassigned: Vec<String>,
    /// Même id de demande, autre titre (base Jellyseerr restaurée : ses ids repartent en arrière) : nouvelle clé.
    pub replaced: Vec<String>,
}

/// Une clé déjà notée n'est jamais réévaluée : seules les nouvelles clés entrent (`pending`, ou `stale` si elles sont
/// disponibles depuis plus de `max_age_secs` ou pendant un écart de l'état). Une clé notée pour un autre titre que
/// celui de sa demande actuelle est une nouvelle clé. Après un écart de l'état, les clés `pending` / `error` notées
/// avant lui passent en `sent_uncertain` (l'état perdu les a très probablement envoyées).
pub fn prepare(
    known: &BTreeMap<String, MediaMailRecord>,
    ready: &[Ready],
    now: i64,
    max_age_secs: i64,
    cont: Continuity,
) -> Prepared {
    let mut p = Prepared::default();
    let current: HashMap<&str, &Ready> = ready.iter().map(|r| (r.key.as_str(), r)).collect();
    let other_title = |r: &MediaMailRecord, c: &Ready| r.kind != c.kind || r.tmdb != c.tmdb;
    for (k, r) in known {
        let now_ready = current.get(k.as_str()).copied();
        if now_ready.is_some_and(|c| other_title(r, c)) {
            continue;
        }
        match r.outcome.as_str() {
            SENDING => {
                p.changes
                    .insert(k.clone(), with_outcome(r, SENT_UNCERTAIN, now));
                p.uncertain.push(k.clone());
            }
            // état restauré (ou daemon longtemps arrêté) : l'état perdu les a très probablement envoyées
            PENDING | ERROR if cont.gap_since.is_some_and(|g| r.seen_at <= g) => {
                p.changes
                    .insert(k.clone(), with_outcome(r, SENT_UNCERTAIN, now));
                p.gap_held.push(k.clone());
            }
            PENDING | ERROR if now - r.ready_at > max_age_secs => {
                p.changes.insert(k.clone(), with_outcome(r, GAVE_UP, now));
                p.expired.push(k.clone());
            }
            PENDING | ERROR if cont.switch_on => {
                p.changes.insert(k.clone(), with_outcome(r, LOGGED, now));
                p.handed_over.push(k.clone());
            }
            PENDING | ERROR => {
                if let Some(c) = now_ready.filter(|c| c.user != r.user) {
                    p.changes.insert(
                        k.clone(),
                        MediaMailRecord {
                            user: c.user,
                            ..r.clone()
                        },
                    );
                    p.reassigned.push(k.clone());
                }
            }
            _ => {}
        }
    }
    for r in ready {
        let replaced = known.get(&r.key).is_some_and(|k| other_title(k, r));
        if (known.contains_key(&r.key) && !replaced) || p.changes.contains_key(&r.key) {
            continue;
        }
        if replaced {
            p.replaced.push(r.key.clone());
        }
        let outcome = if now - r.ready_at > max_age_secs {
            p.new_stale += 1;
            STALE
        } else if cont.gap_since.is_some_and(|g| r.ready_at > g) {
            p.gap_stale += 1;
            STALE
        } else {
            p.new_pending += 1;
            PENDING
        };
        p.changes.insert(r.key.clone(), record(r, now, outcome));
    }
    p
}

/// Les clés d'un membre qui partent ensemble (un mail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub user: i64,
    pub keys: Vec<String>,
}

/// Mails à faire maintenant, le plus ancien d'abord. Les clés en attente d'un membre partent quand plus rien de neuf
/// n'est arrivé pour lui depuis `settle_secs`, ou que la plus ancienne attend depuis `hold_secs` ; ses échecs d'envoi
/// dont l'heure est venue partent avec elles, ou seuls s'il n'a rien en attente.
pub fn due_groups(
    records: &BTreeMap<String, MediaMailRecord>,
    now: i64,
    settle_secs: i64,
    hold_secs: i64,
) -> Vec<Due> {
    #[derive(Default)]
    struct Acc {
        pending: Vec<String>,
        retry: Vec<String>,
        first: Option<i64>,
        last: Option<i64>,
    }
    let mut by_user: BTreeMap<i64, Acc> = BTreeMap::new();
    for (k, r) in records {
        let a = by_user.entry(r.user).or_default();
        match r.outcome.as_str() {
            PENDING => {
                a.pending.push(k.clone());
                a.first = Some(a.first.map_or(r.seen_at, |f| f.min(r.seen_at)));
                a.last = Some(a.last.map_or(r.seen_at, |l| l.max(r.seen_at)));
            }
            ERROR if r.next_at.is_none_or(|t| t <= now) => a.retry.push(k.clone()),
            _ => {}
        }
    }
    let mut out: Vec<(i64, Due)> = Vec::new();
    for (user, a) in by_user {
        let due = match (a.first, a.last) {
            (Some(first), Some(last)) => now - last >= settle_secs || now - first >= hold_secs,
            _ => !a.retry.is_empty(),
        };
        if !due {
            continue;
        }
        let oldest = a
            .pending
            .iter()
            .chain(&a.retry)
            .filter_map(|k| records.get(k).map(|r| r.seen_at))
            .min()
            .unwrap_or(now);
        let mut keys = a.pending;
        keys.extend(a.retry);
        out.push((oldest, Due { user, keys }));
    }
    out.sort_by_key(|(oldest, d)| (*oldest, d.user));
    out.into_iter().map(|(_, d)| d).collect()
}

/// Après un envoi en échec : nouvel essai après le délai suivant de `retry_mins`, ou abandon (`gave_up`).
pub fn after_failure(r: &MediaMailRecord, now: i64, retry_mins: &[i64]) -> MediaMailRecord {
    let tries = r.tries + 1;
    let mut out = MediaMailRecord {
        at: now,
        tries,
        ..r.clone()
    };
    match retry_mins.get(tries as usize - 1) {
        Some(m) => {
            out.outcome = ERROR.into();
            out.next_at = Some(now + (*m).max(1) * 60);
        }
        None => {
            out.outcome = GAVE_UP.into();
            out.next_at = None;
        }
    }
    out
}

/// Titre invisible pour le membre au moment d'envoyer : il attend encore (`None`) ou n'est pas envoyé (`not_visible`).
pub fn hidden_outcome(r: &MediaMailRecord, now: i64, hold_secs: i64) -> Option<&'static str> {
    (now - r.seen_at >= hold_secs).then_some(NOT_VISIBLE)
}

/// Id de la demande d'une clé (`r274` → 274, `r273:s5` → 273).
pub fn request_of(key: &str) -> Option<i64> {
    key.strip_prefix('r')?.split(':').next()?.parse().ok()
}

fn season_of(key: &str) -> Option<u32> {
    key.split_once(":s")?.1.parse().ok()
}

/// Ménage, seulement après une liste complète des demandes : clés terminées d'une demande qui n'existe plus,
/// notées il y a plus de `keep_secs`.
pub fn prune(
    records: &BTreeMap<String, MediaMailRecord>,
    live: &HashSet<i64>,
    now: i64,
    keep_secs: i64,
) -> Vec<String> {
    records
        .iter()
        .filter(|(k, r)| {
            !matches!(r.outcome.as_str(), PENDING | ERROR | SENDING)
                && now - r.at > keep_secs
                && request_of(k).is_some_and(|id| !live.contains(&id))
        })
        .map(|(k, _)| k.clone())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Alerte d'abandon (fonctions pures)
// ---------------------------------------------------------------------------------------------

/// Ligne d'alerte d'une clé abandonnée parce qu'elle attend depuis plus de `max_age_hours` sans pouvoir partir.
pub fn expired_line(key: &str, r: &MediaMailRecord, max_age_hours: i64) -> String {
    format!(
        "{key} ({} TMDB {}, compte Jellyseerr {}) : disponible depuis plus de {max_age_hours} h sans pouvoir partir",
        r.kind, r.tmdb, r.user
    )
}

/// Note une ligne dans l'alerte en attente (`State::media_mail_alerts`), sous la ou les clés abandonnées : la même
/// ligne n'y entre jamais deux fois. Une autre ligne pour la même clé (id de demande réutilisé et abandonné de nouveau
/// avant que la première alerte parte) s'ajoute à la première. Une ligne = une ligne (retours à la ligne remplacés).
pub fn queue_alert(pending: &mut BTreeMap<String, String>, key: String, line: String) {
    let line = line.replace(['\n', '\r'], " ");
    let cur = pending.entry(key).or_default();
    if cur.split('\n').any(|l| l == line) {
        return;
    }
    if !cur.is_empty() {
        cur.push('\n');
    }
    cur.push_str(&line);
}

/// Objet et corps de l'**unique** alerte d'un passage : elle reprend tout ce qui attend (`None` : rien n'attend).
pub fn alert_message(pending: &BTreeMap<String, String>) -> Option<(String, String)> {
    let lines: Vec<&str> = pending
        .values()
        .flat_map(|v| v.split('\n'))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some((
        format!("Mails « disponible » abandonnés : {}", lines.len()),
        format!(
            "Ces mails « disponible » ne partiront pas (tâche media_mail) :\n\n{}\n",
            lines.join("\n")
        ),
    ))
}

/// Après l'alerte : ce qu'elle reprenait (`sent`) est effacé, sauf s'il faut la réessayer (`retry` =
/// [`alerts::retry_later`] : rien n'est parti alors qu'un canal est configuré). Une ligne notée ou complétée depuis
/// la lecture de `sent` reste, pour l'alerte suivante.
pub fn settle_alerts(
    pending: &mut BTreeMap<String, String>,
    sent: &BTreeMap<String, String>,
    retry: bool,
) {
    if !retry {
        pending.retain(|k, v| sent.get(k) != Some(v));
    }
}

// ---------------------------------------------------------------------------------------------
// Destinataire (fonctions pures)
// ---------------------------------------------------------------------------------------------

/// Demandeur, tel que Jellyseerr le donne dans la demande.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub seerr_id: i64,
    /// Adresse en minuscules, si elle est valable (3 comptes ont un pseudo à la place).
    pub email: Option<String>,
    /// Nom affiché (salutation), jamais l'adresse.
    pub name: String,
    /// Id Jellyfin compact, en minuscules.
    pub jellyfin_id: Option<String>,
}

/// Id Jellyfin sans tirets, en minuscules (Jellyseerr et `/Users` ne l'écrivent pas pareil).
pub fn compact_id(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn recipient_of(request: &Value) -> Option<Recipient> {
    let u = request.get("requestedBy")?;
    let seerr_id = u.get("id")?.as_i64()?;
    let email = u
        .get("email")
        .and_then(Value::as_str)
        .map(|e| e.trim().to_lowercase())
        .filter(|e| valid_email(e));
    let name = ["displayName", "jellyfinUsername", "username"]
        .iter()
        .find_map(|f| {
            u.get(*f)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.contains('@'))
        })
        .unwrap_or("")
        .to_string();
    let jellyfin_id = u
        .get("jellyfinUserId")
        .and_then(Value::as_str)
        .map(compact_id)
        .filter(|s| !s.is_empty());
    Some(Recipient {
        seerr_id,
        email,
        name,
        jellyfin_id,
    })
}

/// Le membre a décoché « disponible » dans ses notifications par mail Jellyseerr (`notificationTypes.email`).
/// Rien de choisi (`None`) = tout, comme Jellyseerr.
pub fn email_opted_out(types: Option<i64>) -> bool {
    types.is_some_and(|t| t & SEERR_MEDIA_AVAILABLE == 0)
}

/// Compte listé dans `MEDIA_MAIL_OPTOUT` (liste déjà en minuscules) : par son adresse (ce que porte un « stop » reçu
/// par mail ou par le bouton de désabonnement de Gmail), son pseudo ou son id Jellyfin. `email` : en minuscules.
pub fn listed_out(
    list: &[String],
    names: &[&str],
    jellyfin_id: Option<&str>,
    email: Option<&str>,
) -> bool {
    list.iter().any(|x| {
        names
            .iter()
            .any(|n| !n.is_empty() && x == &n.to_lowercase())
            || jellyfin_id.is_some_and(|id| compact_id(x) == id)
            || email.is_some_and(|e| !e.is_empty() && x == e)
    })
}

/// Compte Jellyfin du membre : (suspendu, pseudo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub disabled: bool,
    pub name: String,
}

/// Comptes Jellyfin (`GET /Users`) par id compact.
pub fn accounts_by_id(users: &[Value]) -> HashMap<String, Account> {
    users
        .iter()
        .filter_map(|u| {
            Some((
                compact_id(u.get("Id")?.as_str()?),
                Account {
                    disabled: !crate::accounts::is_premium(u),
                    name: u.get("Name")?.as_str()?.to_string(),
                },
            ))
        })
        .collect()
}

/// Pourquoi ce membre ne reçoit pas le mail (suite définitive de ses clés), `None` = il le reçoit.
pub fn recipient_verdict(
    who: &Recipient,
    opted_out: bool,
    account: Option<&Account>,
) -> Option<&'static str> {
    if who.email.is_none() {
        Some(NO_EMAIL)
    } else if opted_out {
        Some(OPTED_OUT)
    } else {
        match account {
            None => Some(NO_ACCOUNT),
            Some(a) if a.disabled => Some(SUSPENDED),
            Some(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Titres (fonctions pures)
// ---------------------------------------------------------------------------------------------

/// Un titre d'un mail : un film, ou les saisons d'une série (plusieurs demandes réunies), avec ses clés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub kind: String,
    pub tmdb: i64,
    /// Fiche Jellyfin (film ou série) selon Jellyseerr.
    pub jellyfin_id: Option<String>,
    pub seasons: Vec<u32>,
    pub keys: Vec<String>,
    /// Une des demandes (destinataire).
    pub request: i64,
    pub requested_at: Option<i64>,
    pub ready_at: i64,
}

/// Titres d'un groupe, le dernier arrivé d'abord (mis en avant), et clés dont la demande n'est plus dans la liste.
pub fn plan_items(
    keys: &[String],
    records: &BTreeMap<String, MediaMailRecord>,
    requests: &HashMap<i64, &Value>,
) -> (Vec<Planned>, Vec<String>) {
    let mut items: Vec<Planned> = Vec::new();
    let mut missing = Vec::new();
    for k in keys {
        let (Some(rec), Some(req_id)) = (records.get(k), request_of(k)) else {
            continue;
        };
        let Some(req) = requests.get(&req_id) else {
            missing.push(k.clone());
            continue;
        };
        let jellyfin_id = req
            .pointer("/media/jellyfinMediaId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let requested_at = Some(ts(req.get("createdAt"))).filter(|t| *t > 0);
        let item = match items
            .iter_mut()
            .find(|i| i.kind == rec.kind && i.tmdb == rec.tmdb)
        {
            Some(i) => i,
            None => {
                items.push(Planned {
                    kind: rec.kind.clone(),
                    tmdb: rec.tmdb,
                    jellyfin_id: None,
                    seasons: Vec::new(),
                    keys: Vec::new(),
                    request: req_id,
                    requested_at: None,
                    ready_at: 0,
                });
                items.last_mut().expect("ajouté")
            }
        };
        if item.jellyfin_id.is_none() {
            item.jellyfin_id = jellyfin_id;
        }
        if let Some(n) = season_of(k) {
            if !item.seasons.contains(&n) {
                item.seasons.push(n);
            }
        }
        item.keys.push(k.clone());
        item.requested_at = match (item.requested_at, requested_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        item.ready_at = item.ready_at.max(rec.ready_at);
    }
    for i in &mut items {
        i.seasons.sort_unstable();
    }
    items.sort_by(|a, b| b.ready_at.cmp(&a.ready_at).then(a.tmdb.cmp(&b.tmdb)));
    (items, missing)
}

/// Puce de langue d'après les pistes Jellyfin : audio français + autre → `VF · VO` ; français seul → `VF` ; sinon
/// sous-titres français → `VOSTFR`, sinon `VO`. Aucune piste audio étiquetée : pas de puce.
pub fn lang_chip(streams: &[Value]) -> Option<String> {
    let langs = |kind: &str| -> Vec<String> {
        streams
            .iter()
            .filter(|s| s.get("Type").and_then(Value::as_str) == Some(kind))
            .filter_map(|s| s.get("Language").and_then(Value::as_str))
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty() && l != "und")
            .collect()
    };
    let fr = |l: &String| matches!(l.as_str(), "fre" | "fra" | "fr" | "french");
    let audio = langs("Audio");
    if audio.is_empty() {
        return None;
    }
    let chip = if audio.iter().any(fr) {
        if audio.iter().any(|l| !fr(l)) {
            "VF · VO"
        } else {
            "VF"
        }
    } else if langs("Subtitle").iter().any(fr) {
        "VOSTFR"
    } else {
        "VO"
    };
    Some(chip.into())
}

fn year_of(date: &str) -> Option<String> {
    date.get(..4)
        .filter(|y| y.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// Années d'une série : `1993–2018` (terminée), `2013` (une seule année), `depuis 2019` (en cours).
pub fn series_years(first: &str, last: &str, status: &str) -> Option<String> {
    let f = year_of(first)?;
    Some(match status {
        "Ended" | "Canceled" => match year_of(last) {
            Some(l) if l != f => format!("{f}–{l}"),
            _ => f,
        },
        _ => format!("depuis {f}"),
    })
}

/// En-tête `List-Unsubscribe` des mails « disponible » : une réponse à l'adresse d'envoi (jamais de lien à jeton).
pub fn list_unsubscribe(from: &str) -> String {
    format!("<mailto:{from}?subject=Stop%20mails%20disponible>")
}

/// Fiche Jellyfin d'un élément (`#/details?id=`, le chemin de l'interface 12.1) : jamais de jeton dans le lien.
pub fn watch_url(public: &str, item_id: &str) -> String {
    format!(
        "{}/web/#/details?id={item_id}",
        public.trim_end_matches('/')
    )
}

fn text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn genres(d: &Value) -> Vec<String> {
    d.get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("name").and_then(Value::as_str))
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect()
}

fn rating(d: &Value) -> Option<f64> {
    d.get("voteAverage")
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0)
}

fn streams(item: Option<&Value>) -> Vec<Value> {
    item.and_then(|i| i.get("MediaStreams"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn non_empty(s: String) -> Option<String> {
    Some(s).filter(|s| !s.is_empty())
}

/// Film : fiche Jellyseerr (`?language=fr`) et élément Jellyfin vu par le membre (pistes).
pub fn movie_title(d: &Value, item: &Value, watch_url: String, requested_at: Option<i64>) -> Title {
    Title {
        kind: TitleKind::Movie,
        title: non_empty(text(d, "title"))
            .or_else(|| non_empty(text(item, "Name")))
            .unwrap_or_else(|| "Ton film".into()),
        original: non_empty(text(d, "originalTitle")),
        years: year_of(&text(d, "releaseDate")),
        seasons: Vec::new(),
        whole_series: false,
        episodes: None,
        runtime_mins: d
            .get("runtime")
            .and_then(Value::as_u64)
            .and_then(|m| u32::try_from(m).ok())
            .filter(|m| *m > 0),
        rating: rating(d),
        lang: lang_chip(&streams(Some(item))),
        genres: genres(d),
        synopsis: text(d, "overview"),
        poster: None,
        backdrop: None,
        watch_url,
        requested_at,
    }
}

/// Série : fiche Jellyseerr, saisons arrivées, premier épisode de la première (pistes), vu par le membre.
pub fn series_title(
    d: &Value,
    seasons: &[u32],
    episode: Option<&Value>,
    watch_url: String,
    requested_at: Option<i64>,
) -> Title {
    let tmdb_seasons: Vec<&Value> = d
        .get("seasons")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let episodes: u64 = tmdb_seasons
        .iter()
        .filter(|s| {
            s.get("seasonNumber")
                .and_then(Value::as_u64)
                .is_some_and(|n| seasons.iter().any(|x| u64::from(*x) == n))
        })
        .filter_map(|s| s.get("episodeCount").and_then(Value::as_u64))
        .sum();
    let whole = seasons == [1] && d.get("numberOfSeasons").and_then(Value::as_u64) == Some(1);
    Title {
        kind: TitleKind::Series,
        title: non_empty(text(d, "name"))
            .or_else(|| non_empty(text(d, "originalName")))
            .unwrap_or_else(|| "Ta série".into()),
        original: non_empty(text(d, "originalName")),
        years: series_years(
            &text(d, "firstAirDate"),
            &text(d, "lastAirDate"),
            &text(d, "status"),
        ),
        seasons: seasons.to_vec(),
        whole_series: whole,
        episodes: u32::try_from(episodes).ok().filter(|e| *e > 0),
        runtime_mins: None,
        rating: rating(d),
        lang: lang_chip(&streams(episode)),
        genres: genres(d),
        synopsis: text(d, "overview"),
        poster: None,
        backdrop: None,
        watch_url,
        requested_at,
    }
}

/// Affiche de saison TMDB (`seasons[n].posterPath`), si la fiche en a une.
fn tmdb_season_poster(d: &Value, season: u32) -> Option<String> {
    d.get("seasons")
        .and_then(Value::as_array)?
        .iter()
        .find(|s| s.get("seasonNumber").and_then(Value::as_u64) == Some(u64::from(season)))
        .and_then(|s| s.get("posterPath").and_then(Value::as_str))
        .map(str::to_string)
}

fn tmdb_url(size: &str, path: Option<&str>) -> Option<String> {
    path.filter(|p| p.starts_with('/') && p.len() > 1)
        .map(|p| format!("{TMDB_IMAGES}{size}{p}"))
}

// ---------------------------------------------------------------------------------------------
// Passage
// ---------------------------------------------------------------------------------------------

/// Que faire d'un mail composé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `homelabctl run media_mail --dry-run` : rien d'écrit, rien d'envoyé.
    DryRun,
    /// `send_to_members = false` : noté `logged`, jamais envoyé au membre (copie d'aperçu possible).
    Log,
    Send,
}

pub fn mode(dry_run: bool, send_to_members: bool) -> Mode {
    match (dry_run, send_to_members) {
        (true, _) => Mode::DryRun,
        (false, false) => Mode::Log,
        (false, true) => Mode::Send,
    }
}

/// Demandes et films ou saisons disponibles, lus dans Jellyseerr (GET seulement).
pub struct Snapshot {
    pub requests: Vec<Value>,
    pub ready: Vec<Ready>,
    /// Demandes et médias lus en entier (autant d'ids distincts que les totaux annoncés) : seule une lecture entière
    /// sert de référence ou au ménage.
    pub complete: bool,
}

/// Lit toutes les demandes, puis les saisons des médias, et en tire les films et saisons disponibles.
pub async fn snapshot(ctx: &TaskContext) -> Result<Snapshot> {
    let (requests, announced) = ctx.jellyseerr.all_requests_counted().await?;
    let (media, media_announced) = ctx.jellyseerr.all_media_counted().await?;
    let distinct = |v: &[Value]| {
        v.iter()
            .filter_map(|x| x.get("id").and_then(Value::as_i64))
            .collect::<HashSet<i64>>()
            .len() as u64
    };
    let complete = distinct(&requests) >= announced && distinct(&media) >= media_announced;
    let ready = ready_events(&requests, &media_seasons(&media));
    Ok(Snapshot {
        requests,
        ready,
        complete,
    })
}

/// Images à aller chercher pour un titre : candidates dans l'ordre (Jellyfin d'abord, TMDB en secours).
#[derive(Debug, Default, Clone)]
struct Art {
    poster: Vec<String>,
    row_poster: Vec<String>,
    backdrop: Vec<String>,
}

/// Un mail composé : ses clés, celles des titres que le membre ne voit pas dans Jellyfin, et celles des titres
/// illisibles à ce passage.
pub struct Composed {
    /// `None` si aucun titre n'est visible et lisible pour le membre.
    pub mail: Option<RenderedMail>,
    pub keys: Vec<String>,
    pub titles: usize,
    pub hidden: Vec<String>,
    /// Lecture en échec (Jellyseerr ou Jellyfin) : ces clés restent en attente, les autres titres partent sans eux.
    pub failed: Vec<String>,
}

async fn first_image(ctx: &TaskContext, urls: &[String], name: &str) -> Option<Img> {
    for u in urls {
        if let Some(i) = fetch_image(&ctx.http, u, name).await {
            return Some(Img::Inline(i));
        }
    }
    None
}

/// Un titre vu par le membre (`None` : il ne le voit pas) et ses images candidates.
async fn read_title(
    ctx: &TaskContext,
    p: &Planned,
    jellyfin_user: &str,
) -> Result<Option<(Title, Art)>> {
    let public = &ctx.secrets.jellyfin_public_url;
    let local = ctx.cfg.urls.jellyfin.trim_end_matches('/');
    let jf_image =
        |id: &str, kind: &str, q: &str| format!("{local}/Items/{id}/Images/{kind}?{q}&format=Jpg");
    let Some(jid) = p.jellyfin_id.as_deref() else {
        return Ok(None);
    };
    // par son id : la liste `GET /Items?Ids=&UserId=` renvoie aussi les éléments des bibliothèques que le compte
    // n'a pas (Jellyfin 12.1, vérifié le 2026-10-15) ; la fiche, elle, répond 404
    let Some(item) = ctx.jellyfin.item_visible_to(jid, jellyfin_user).await? else {
        return Ok(None);
    };
    let item = &item;
    let (title, art) = if p.kind == "movie" {
        let d = ctx.jellyseerr.movie_details(p.tmdb).await?;
        let poster = d.get("posterPath").and_then(Value::as_str);
        let art = Art {
            poster: [Some(jf_image(jid, "Primary", "maxWidth=320&quality=80"))]
                .into_iter()
                .chain([tmdb_url("w342", poster)])
                .flatten()
                .collect(),
            row_poster: [Some(jf_image(jid, "Primary", "maxWidth=340&quality=75"))]
                .into_iter()
                .chain([tmdb_url("w342", poster)])
                .flatten()
                .collect(),
            backdrop: [Some(jf_image(
                jid,
                "Backdrop/0",
                "maxWidth=1200&quality=70",
            ))]
            .into_iter()
            .chain([tmdb_url(
                "w780",
                d.get("backdropPath").and_then(Value::as_str),
            )])
            .flatten()
            .collect(),
        };
        (
            movie_title(&d, item, watch_url(public, jid), p.requested_at),
            art,
        )
    } else {
        let d = ctx.jellyseerr.tv_details(p.tmdb).await?;
        let seasons = ctx.jellyfin.seasons_as(jid, jellyfin_user).await?;
        let first = p.seasons.first().copied().unwrap_or(1);
        // saison seule : sa fiche et son affiche ; sinon la série
        let season_item = (p.seasons.len() == 1)
            .then(|| {
                seasons.iter().find(|s| {
                    s.get("IndexNumber").and_then(Value::as_u64) == Some(u64::from(first))
                })
            })
            .flatten();
        let season_id = season_item
            .and_then(|s| s.get("Id").and_then(Value::as_str))
            .map(str::to_string);
        let season_has_poster =
            season_item.is_some_and(|s| s.pointer("/ImageTags/Primary").is_some());
        let episode = ctx
            .jellyfin
            .season_episodes_as(jid, jellyfin_user, first, 1)
            .await?;
        let url = watch_url(public, season_id.as_deref().unwrap_or(jid));
        let series_poster = d.get("posterPath").and_then(Value::as_str);
        let season_poster = (p.seasons.len() == 1)
            .then(|| tmdb_season_poster(&d, first))
            .flatten();
        let mut poster: Vec<String> = Vec::new();
        if let (Some(sid), true) = (&season_id, season_has_poster) {
            poster.push(jf_image(sid, "Primary", "maxWidth=320&quality=80"));
        }
        poster.push(jf_image(jid, "Primary", "maxWidth=320&quality=80"));
        poster.extend(tmdb_url("w342", season_poster.as_deref()));
        poster.extend(tmdb_url("w342", series_poster));
        let mut row_poster: Vec<String> = Vec::new();
        if let (Some(sid), true) = (&season_id, season_has_poster) {
            row_poster.push(jf_image(sid, "Primary", "maxWidth=340&quality=75"));
        }
        row_poster.push(jf_image(jid, "Primary", "maxWidth=340&quality=75"));
        row_poster.extend(tmdb_url("w342", season_poster.as_deref()));
        row_poster.extend(tmdb_url("w342", series_poster));
        let art = Art {
            poster,
            row_poster,
            backdrop: [Some(jf_image(
                jid,
                "Backdrop/0",
                "maxWidth=1200&quality=70",
            ))]
            .into_iter()
            .chain([tmdb_url(
                "w780",
                d.get("backdropPath").and_then(Value::as_str),
            )])
            .flatten()
            .collect(),
        };
        (
            series_title(&d, &p.seasons, episode.first(), url, p.requested_at),
            art,
        )
    };
    Ok(Some((title, art)))
}

/// Rassemble les titres d'un mail, vus **par le membre** (`jellyfin_user`, id compact) dans Jellyfin, et le rend ;
/// `images` : fond et affiches lus (Jellyfin, TMDB en secours) et joints. Lecture seule (GET Jellyseerr et Jellyfin) :
/// sert aussi à un mail d'essai sans rien noter. Un titre illisible (fiche TMDB retirée, panne passagère) ne bloque
/// pas les autres : ses clés vont dans `failed`.
pub async fn compose(
    ctx: &TaskContext,
    who: &Recipient,
    jellyfin_user: &str,
    items: &[Planned],
    at: i64,
    images: bool,
) -> Result<Composed> {
    let public = &ctx.secrets.jellyfin_public_url;
    let mut titles: Vec<(Title, Art)> = Vec::new();
    let mut keys = Vec::new();
    let mut hidden = Vec::new();
    let mut failed = Vec::new();
    for p in items {
        match read_title(ctx, p, jellyfin_user).await {
            Ok(Some(t)) => {
                keys.extend(p.keys.iter().cloned());
                titles.push(t);
            }
            Ok(None) => hidden.extend(p.keys.iter().cloned()),
            Err(e) => {
                warn!(
                    task = TASK,
                    user = who.seerr_id,
                    keys = ?p.keys,
                    error = format!("{e:#}"),
                    "titre illisible : nouvel essai au passage suivant, les autres partent sans lui"
                );
                failed.extend(p.keys.iter().cloned());
            }
        }
    }
    if titles.is_empty() {
        return Ok(Composed {
            mail: None,
            keys,
            titles: 0,
            hidden,
            failed,
        });
    }
    let n = titles.len();
    let mut it = titles.into_iter();
    let (mut hero, hero_art) = it.next().expect("au moins un titre");
    if images {
        hero.backdrop = first_image(ctx, &hero_art.backdrop, "fond").await;
        hero.poster = first_image(ctx, &hero_art.poster, "affiche").await;
    }
    let mut others = Vec::new();
    for (i, (mut t, art)) in it.enumerate() {
        if images && i < POSTER_SLOTS {
            t.poster = first_image(ctx, &art.row_poster, &format!("affiche-{}", i + 2)).await;
        }
        others.push(t);
    }
    let mail = available_mail::render(&AvailableMail {
        name: who.name.clone(),
        hero,
        others,
        home_url: public.clone(),
        at,
    });
    Ok(Composed {
        mail: Some(mail),
        keys,
        titles: n,
        hidden,
        failed,
    })
}

/// Note `outcome` sur ces clés, sur disque.
async fn set_outcome(ctx: &TaskContext, keys: &[String], outcome: &str, t: i64) -> Result<()> {
    if ctx.dry_run || keys.is_empty() {
        return Ok(());
    }
    let keys = keys.to_vec();
    let outcome = outcome.to_string();
    ctx.state
        .update(move |s| {
            for k in &keys {
                if let Some(r) = s.media_mail.get_mut(k) {
                    *r = with_outcome(r, &outcome, t);
                }
            }
        })
        .await
}

/// Ce qu'un passage a fait, pour son résumé.
#[derive(Debug, Default)]
struct Tally {
    mails: u32,
    titles: usize,
    failed: u32,
    /// Envois peut-être partis (`curl` en échec après le message entier) : `sent_uncertain`, jamais renvoyés.
    uncertain: u32,
    deferred: usize,
    unreadable: u32,
    skipped: BTreeMap<&'static str, usize>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Envoie l'alerte d'abandon qui reprend tout ce qui attend (`State::media_mail_alerts`, et `queued` : lignes de ce
/// passage, seules en dry-run), puis efface ce qu'elle reprenait **seulement si elle est partie**
/// ([`alerts::retry_later`]). Rend le complément du résumé (`None` : rien n'attendait).
async fn flush_alerts(ctx: &TaskContext, queued: BTreeMap<String, String>) -> Option<String> {
    let mut pending = ctx.state.read(|s| s.media_mail_alerts.clone()).await;
    for (k, line) in queued {
        queue_alert(&mut pending, k, line);
    }
    let (subject, body) = alert_message(&pending)?;
    let sent = alerts::admin(ctx, Level::Warn, &subject, &body).await;
    if ctx.dry_run {
        return Some("dry-run : alerte d'abandon non envoyée".into());
    }
    let retry = alerts::retry_later(sent, alerts::configured(ctx));
    if retry {
        warn!(
            task = TASK,
            keys = ?pending.keys().collect::<Vec<_>>(),
            "alerte d'abandon non livrée : gardée, nouvel essai au passage suivant"
        );
        return Some("alerte d'abandon non livrée (nouvel essai au passage suivant)".into());
    }
    if let Err(e) = ctx
        .state
        .update(move |s| settle_alerts(&mut s.media_mail_alerts, &pending, retry))
        .await
    {
        warn!(
            task = TASK,
            error = format!("{e:#}"),
            "alerte d'abandon partie mais non effacée de l'état : elle repartira au passage suivant"
        );
    }
    Some("alerte d'abandon envoyée".into())
}

impl MediaMail {
    /// Un passage : détection, regroupement, envoi, ménage. `queued` reçoit les lignes d'alerte d'abandon de ce
    /// passage, déjà notées dans l'état avec leur `gave_up` (sauf en dry-run) ; [`flush_alerts`] les envoie ensuite.
    async fn pass(
        &self,
        ctx: &TaskContext,
        queued: &mut BTreeMap<String, String>,
    ) -> Result<Report> {
        let cfg: &Cfg = &ctx.cfg.tasks.media_mail;
        let started = Instant::now();
        let last_pass = ctx.state.read(|s| s.media_mail_last_pass).await;
        if !ctx.dry_run {
            // l'état a vu ce passage, même si Jellyseerr ne répond pas : seuls un état restauré ou un daemon arrêté
            // laissent un écart (`continuity`) ; le choix d'envoi, lui, n'est noté qu'avec les nouvelles clés
            let at = now();
            ctx.state
                .update_lazy(move |s| {
                    let send = s.media_mail_last_pass.is_some_and(|p| p.send);
                    s.media_mail_last_pass = Some(MediaMailPass { at, send });
                })
                .await?;
        }
        if ctx.jellyseerr.status().await.is_err() {
            return Ok(Report::new("jellyseerr_unreachable", 0));
        }
        let Snapshot {
            requests,
            ready,
            complete,
        } = snapshot(ctx).await?;
        let t = now();
        let max_age = cfg.max_age_hours * 3600;
        let settle = cfg.settle_mins * 60;
        let hold = cfg.max_hold_mins * 60;
        let mode = mode(ctx.dry_run, cfg.send_to_members);
        let this_pass = MediaMailPass {
            at: t,
            send: mode == Mode::Send,
        };
        let (baseline_at, mut records) = ctx
            .state
            .read(|s| (s.media_mail_baseline_at, s.media_mail.clone()))
            .await;

        // 1. Premier passage : référence, aucun mail.
        if baseline_at.is_none() {
            if !complete {
                return Ok(Report::new(
                    "liste des demandes incomplète : référence au passage suivant",
                    0,
                ));
            }
            let base = baseline(&ready, t);
            let n = base.len();
            if ctx.dry_run {
                return Ok(Report::new(
                    format!("dry-run : premier passage = référence, {n} films ou saisons déjà disponibles notés sans mail"),
                    0,
                ));
            }
            ctx.state
                .update(move |s| {
                    for (k, r) in base {
                        s.media_mail.entry(k).or_insert(r);
                    }
                    s.media_mail_baseline_at = Some(t);
                    s.media_mail_last_pass = Some(this_pass);
                })
                .await?;
            info!(
                task = TASK,
                n, "référence : déjà disponibles notés, aucun mail"
            );
            return Ok(Report::new(
                format!("référence : {n} films ou saisons déjà disponibles notés, aucun mail"),
                0,
            ));
        }

        // 2. Nouvelles clés, envois interrompus, attentes trop longues, bascule vers l'envoi.
        let cont = continuity(last_pass, t, mode, cfg.max_gap_mins * 60);
        let prep = prepare(&records, &ready, t, max_age, cont);
        for k in &prep.uncertain {
            warn!(task = TASK, key = %k, "envoi interrompu au passage précédent : noté sent_uncertain, jamais renvoyé");
        }
        if !prep.handed_over.is_empty() {
            info!(task = TASK, keys = ?prep.handed_over, "bascule vers l'envoi : titres en attente déjà annoncés par Jellyseerr, notés logged");
        }
        if !prep.gap_held.is_empty() {
            warn!(
                task = TASK,
                keys = ?prep.gap_held,
                max_gap_mins = cfg.max_gap_mins,
                "aucun passage noté depuis plus de max_gap_mins (état restauré ou daemon arrêté) : titres en attente notés sent_uncertain (l'état perdu les a sans doute envoyés), jamais renvoyés"
            );
        }
        if let (Some(since), true) = (cont.gap_since, prep.gap_stale > 0) {
            warn!(
                task = TASK,
                since,
                n = prep.gap_stale,
                max_gap_mins = cfg.max_gap_mins,
                "aucun passage noté depuis plus de max_gap_mins (état restauré ou daemon arrêté) : titres arrivés entre-temps notés stale, sans mail"
            );
        }
        if !prep.reassigned.is_empty() {
            info!(task = TASK, keys = ?prep.reassigned, "demandeur changé dans Jellyseerr : le titre suit le nouveau");
        }
        if !prep.replaced.is_empty() {
            warn!(task = TASK, keys = ?prep.replaced, "id de demande réutilisé pour un autre titre (base Jellyseerr restaurée ?) : nouvelle clé");
        }
        // abandons de ce passage : leurs lignes d'alerte sont notées avec eux, dans la même écriture
        let expired_alerts: Vec<(String, String)> = prep
            .expired
            .iter()
            .filter_map(|k| {
                Some((
                    k.clone(),
                    expired_line(k, records.get(k)?, cfg.max_age_hours),
                ))
            })
            .collect();
        let mut abandoned = expired_alerts.len();
        for (k, line) in &expired_alerts {
            queue_alert(queued, k.clone(), line.clone());
        }
        for (k, r) in &prep.changes {
            records.insert(k.clone(), r.clone());
        }
        if !ctx.dry_run {
            // le passage est noté avec ses nouvelles clés : sur disque tout de suite s'il change quelque chose (dont
            // la bascule), sinon avec la prochaine écriture
            let durable =
                !prep.changes.is_empty() || last_pass.map(|p| p.send) != Some(this_pass.send);
            let changes = prep.changes.clone();
            let apply = move |s: &mut crate::state::State| {
                s.media_mail.extend(changes);
                for (k, line) in expired_alerts {
                    queue_alert(&mut s.media_mail_alerts, k, line);
                }
                s.media_mail_last_pass = Some(this_pass);
            };
            if durable {
                ctx.state.update(apply).await?;
            } else {
                ctx.state.update_lazy(apply).await?;
            }
        }

        // 3. Mails dus.
        let groups = due_groups(&records, t, settle, hold);
        // demandes de ces mails absentes de la liste (une suppression pendant la lecture fait glisser les pages) :
        // relues une par une, `gone` seulement sur un 404
        let listed: HashSet<i64> = requests
            .iter()
            .filter_map(|r| r.get("id").and_then(Value::as_i64))
            .collect();
        let absent: std::collections::BTreeSet<i64> = groups
            .iter()
            .flat_map(|g| g.keys.iter())
            .filter_map(|k| request_of(k))
            .filter(|id| !listed.contains(id))
            .collect();
        let mut extra: Vec<Value> = Vec::new();
        let mut gone_ids: HashSet<i64> = HashSet::new();
        for id in absent {
            match ctx.jellyseerr.request_by_id(id).await {
                Ok(Some(v)) => extra.push(v),
                Ok(None) => {
                    gone_ids.insert(id);
                }
                Err(e) => warn!(
                    task = TASK,
                    request = id,
                    error = format!("{e:#}"),
                    "demande absente de la liste et illisible : nouvel essai au passage suivant"
                ),
            }
        }
        let by_id: HashMap<i64, &Value> = requests
            .iter()
            .chain(extra.iter())
            .filter_map(|r| Some((r.get("id")?.as_i64()?, r)))
            .collect();
        let smtp = ctx.secrets.smtp.as_ref();
        if mode == Mode::Send && smtp.is_none() && !groups.is_empty() {
            bail!(
                "SMTP absent de .env : {} mail(s) « disponible » en attente",
                groups.len()
            );
        }
        let preview = match mode {
            Mode::Log => ctx
                .secrets
                .media_mail_preview_to
                .as_deref()
                .map(|a| a.trim().to_lowercase())
                .filter(|a| valid_email(a) && smtp.is_some()),
            _ => None,
        };
        let images = mode == Mode::Send || preview.is_some();
        let unsubscribe = smtp.map(|s| list_unsubscribe(&s.from));
        let headers: Vec<(&str, &str)> = unsubscribe
            .as_deref()
            .map(|u| vec![("List-Unsubscribe", u)])
            .unwrap_or_default();
        let mut accounts: Option<HashMap<String, Account>> = None;
        let mut tally = Tally::default();
        for (n, g) in groups.iter().enumerate() {
            if tally.mails as usize >= cfg.max_mails_per_run || started.elapsed() > RUN_BUDGET {
                tally.deferred = groups.len() - n;
                break;
            }
            let (items, missing) = plan_items(&g.keys, &records, &by_id);
            let gone: Vec<String> = missing
                .into_iter()
                .filter(|k| request_of(k).is_some_and(|id| gone_ids.contains(&id)))
                .collect();
            if !gone.is_empty() {
                info!(task = TASK, user = g.user, keys = ?gone, "demande retirée : pas de mail");
                set_outcome(ctx, &gone, GONE, t).await?;
                *tally.skipped.entry(GONE).or_default() += gone.len();
            }
            let Some(who) = items
                .first()
                .and_then(|p| by_id.get(&p.request))
                .and_then(|r| recipient_of(r))
            else {
                continue;
            };
            // demandeur changé depuis la clé (demande relue hors liste) : regroupé au passage suivant, jamais l'adresse
            // de l'un avec les titres et la case « disponible » de l'autre
            if who.seerr_id != g.user {
                info!(
                    task = TASK,
                    user = g.user,
                    now_user = who.seerr_id,
                    "demandeur changé dans Jellyseerr : mail au passage suivant"
                );
                continue;
            }
            let all_keys: Vec<String> = items.iter().flat_map(|p| p.keys.clone()).collect();

            // destinataire
            let types = if who.email.is_some() {
                match ctx.jellyseerr.email_notification_types(g.user).await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(
                            task = TASK,
                            user = g.user,
                            error = format!("{e:#}"),
                            "choix de notifications illisible : nouvel essai au passage suivant"
                        );
                        tally.unreadable += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            if accounts.is_none() {
                match ctx.jellyfin.users().await {
                    Ok(u) => accounts = Some(accounts_by_id(&u)),
                    Err(e) => {
                        warn!(
                            task = TASK,
                            error = format!("{e:#}"),
                            "comptes Jellyfin illisibles : nouvel essai au passage suivant"
                        );
                        tally.unreadable += 1;
                        break;
                    }
                }
            }
            let account = who
                .jellyfin_id
                .as_deref()
                .and_then(|id| accounts.as_ref().and_then(|a| a.get(id)));
            let names = [who.name.as_str(), account.map_or("", |a| a.name.as_str())];
            let opted = email_opted_out(types)
                || listed_out(
                    &ctx.secrets.media_mail_optout,
                    &names,
                    who.jellyfin_id.as_deref(),
                    who.email.as_deref(),
                );
            if let Some(outcome) = recipient_verdict(&who, opted, account) {
                info!(task = TASK, user = g.user, name = %who.name, outcome, keys = ?all_keys, "pas de mail pour ce membre");
                set_outcome(ctx, &all_keys, outcome, t).await?;
                *tally.skipped.entry(outcome).or_default() += all_keys.len();
                continue;
            }
            let jellyfin_user = who.jellyfin_id.clone().unwrap_or_default();

            // composition, vue par le membre
            let composed = match compose(ctx, &who, &jellyfin_user, &items, t, images).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        task = TASK,
                        user = g.user,
                        error = format!("{e:#}"),
                        "mail non composé : nouvel essai au passage suivant"
                    );
                    tally.unreadable += 1;
                    continue;
                }
            };
            if !composed.failed.is_empty() {
                tally.unreadable += 1;
            }
            let invisible: Vec<String> = composed
                .hidden
                .iter()
                .filter(|k| {
                    records
                        .get(*k)
                        .and_then(|r| hidden_outcome(r, t, hold))
                        .is_some()
                })
                .cloned()
                .collect();
            if !invisible.is_empty() {
                info!(task = TASK, user = g.user, keys = ?invisible, "titre invisible pour ce compte dans Jellyfin : pas de mail");
                set_outcome(ctx, &invisible, NOT_VISIBLE, t).await?;
                *tally.skipped.entry(NOT_VISIBLE).or_default() += invisible.len();
            }
            let Some(mail) = composed.mail else {
                continue;
            };
            let keys = composed.keys;
            match mode {
                Mode::DryRun => {
                    info!(task = TASK, user = g.user, name = %who.name, keys = ?keys, subject = %mail.subject, "dry-run: mail « disponible » non envoyé");
                }
                Mode::Log => {
                    set_outcome(ctx, &keys, LOGGED, t).await?;
                    info!(task = TASK, user = g.user, name = %who.name, keys = ?keys, subject = %mail.subject, preview = preview.is_some(), "envoi aux membres coupé : mail noté, non envoyé");
                    if let (Some(to), Some(smtp)) = (preview.as_deref(), smtp) {
                        if let Err(e) =
                            mail::send_rendered(smtp, &who.name, to, &mail, None, &headers).await
                        {
                            warn!(
                                task = TASK,
                                user = g.user,
                                error = format!("{e:#}"),
                                "copie d'aperçu en échec"
                            );
                        }
                    }
                }
                Mode::Send => {
                    let (Some(smtp), Some(to)) = (smtp, who.email.as_deref()) else {
                        continue;
                    };
                    // noté AVANT l'envoi : un passage coupé pendant curl ne renverra jamais ce mail
                    set_outcome(ctx, &keys, SENDING, t).await?;
                    match mail::send_rendered(smtp, &who.name, to, &mail, None, &headers).await {
                        Ok(()) => {
                            set_outcome(ctx, &keys, SENT, t).await?;
                            info!(task = TASK, user = g.user, name = %who.name, keys = ?keys, subject = %mail.subject, "mail « disponible » envoyé");
                        }
                        Err(e) if mail::maybe_sent(&e) => {
                            // message passé en entier, réponse perdue : peut-être reçu, jamais renvoyé (au plus un mail)
                            set_outcome(ctx, &keys, SENT_UNCERTAIN, t).await?;
                            warn!(task = TASK, user = g.user, keys = ?keys, error = format!("{e:#}"), "mail « disponible » peut-être parti : noté sent_uncertain, jamais renvoyé");
                            tally.uncertain += 1;
                            continue;
                        }
                        Err(e) => {
                            let err = format!("{e:#}");
                            warn!(task = TASK, user = g.user, error = %err, "mail « disponible » en échec");
                            tally.failed += 1;
                            let next: BTreeMap<String, MediaMailRecord> = keys
                                .iter()
                                .filter_map(|k| {
                                    let r = records.get(k)?;
                                    Some((k.clone(), after_failure(r, t, &cfg.retry_mins)))
                                })
                                .collect();
                            // clés abandonnées par cet échec : la ligne d'alerte est notée avec elles, dans la même écriture
                            let given_up: Vec<&String> = next
                                .iter()
                                .filter(|(_, r)| r.outcome == GAVE_UP)
                                .map(|(k, _)| k)
                                .collect();
                            let alert = given_up.first().map(|k0| {
                                let keys = given_up.iter().map(|k| k.as_str()).collect::<Vec<_>>();
                                (
                                    keys.join(","),
                                    format!(
                                        "{} : « {} » pour {} (compte Jellyseerr {}) : {} échecs d'envoi, dernier : {err}",
                                        keys.join(", "),
                                        mail.subject,
                                        who.name,
                                        g.user,
                                        next[*k0].tries
                                    ),
                                )
                            });
                            if let Some((k, line)) = &alert {
                                abandoned += 1;
                                queue_alert(queued, k.clone(), line.clone());
                            }
                            for (k, r) in &next {
                                records.insert(k.clone(), r.clone());
                            }
                            ctx.state
                                .update(move |s| {
                                    s.media_mail.extend(next);
                                    if let Some((k, line)) = alert {
                                        queue_alert(&mut s.media_mail_alerts, k, line);
                                    }
                                })
                                .await?;
                            continue;
                        }
                    }
                }
            }
            tally.mails += 1;
            tally.titles += composed.titles;
        }

        // 4. Ménage (liste complète seulement : une demande absente d'une liste tronquée existe peut-être encore).
        let mut pruned = 0;
        if complete && !ctx.dry_run {
            let live: HashSet<i64> = by_id.keys().copied().collect();
            let old = prune(&records, &live, t, cfg.keep_days * 86_400);
            pruned = old.len();
            if pruned > 0 {
                ctx.state
                    .update(move |s| {
                        for k in &old {
                            s.media_mail.remove(k);
                        }
                    })
                    .await?;
            }
        }

        let waiting = ctx
            .state
            .read(|s| {
                s.media_mail
                    .values()
                    .filter(|r| matches!(r.outcome.as_str(), PENDING | ERROR))
                    .count()
            })
            .await;
        let mails = plural(tally.mails as usize, "mail", "mails");
        let titles = plural(tally.titles, "titre", "titres");
        let mut parts = vec![match (mode, tally.mails) {
            (Mode::Send, 0) => "aucun mail".to_string(),
            (Mode::Send, _) => format!("{mails} envoyé(s) ({titles})"),
            (Mode::Log, 0) => "envoi aux membres coupé, aucun mail".to_string(),
            (Mode::Log, _) => format!(
                "envoi aux membres coupé : {mails} noté(s) sans envoi ({titles}){}",
                if preview.is_some() {
                    ", copie d'aperçu"
                } else {
                    ""
                }
            ),
            (Mode::DryRun, _) => format!("dry-run : {mails} ({titles})"),
        }];
        if prep.new_pending > 0 {
            parts.push(format!("{} disponible(s)", prep.new_pending));
        }
        if waiting > 0 {
            parts.push(format!("{waiting} en attente"));
        }
        if tally.deferred > 0 {
            parts.push(format!("{} reporté(s)", tally.deferred));
        }
        if tally.failed > 0 {
            parts.push(format!("{} échec(s) d'envoi", tally.failed));
        }
        if tally.unreadable > 0 {
            parts.push(format!("{} lecture(s) en échec", tally.unreadable));
        }
        for (outcome, n) in &tally.skipped {
            parts.push(format!("{n} {outcome}"));
        }
        if prep.new_stale > 0 {
            parts.push(format!("{} trop ancien(s)", prep.new_stale));
        }
        if prep.gap_stale > 0 {
            parts.push(format!(
                "{} arrivé(s) pendant un écart de l'état",
                prep.gap_stale
            ));
        }
        if !prep.gap_held.is_empty() {
            parts.push(format!(
                "{} en attente avant un écart de l'état (sans mail)",
                prep.gap_held.len()
            ));
        }
        if !prep.handed_over.is_empty() {
            parts.push(format!(
                "{} déjà annoncé(s) par Jellyseerr (bascule)",
                prep.handed_over.len()
            ));
        }
        let uncertain = prep.uncertain.len() + tally.uncertain as usize;
        if uncertain > 0 {
            parts.push(format!("{uncertain} envoi(s) incertain(s)"));
        }
        if abandoned > 0 {
            parts.push(format!("{abandoned} abandonné(s)"));
        }
        if pruned > 0 {
            parts.push(format!("{pruned} oublié(s)"));
        }
        Ok(Report::new(parts.join(", "), tally.mails))
    }
}

#[async_trait]
impl Task for MediaMail {
    fn name(&self) -> &'static str {
        TASK
    }

    fn label(&self) -> &'static str {
        "Mails « disponible »"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.media_mail.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        // lignes d'alerte notées par ce passage (en dry-run, l'état n'en garde aucune)
        let mut queued = BTreeMap::new();
        let res = self.pass(ctx, &mut queued).await;
        // une seule alerte par passage, même pour un passage en échec ou sans Jellyseerr : ce qui attend dans l'état
        // n'attend ni Jellyseerr ni Jellyfin
        let alert = flush_alerts(ctx, queued).await;
        let mut report = res?;
        if let Some(a) = alert {
            report.summary = if report.summary.is_empty() {
                a
            } else {
                format!("{}, {a}", report.summary)
            };
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T: i64 = 1_800_000_000;
    const MIN: i64 = 60;
    const H: i64 = 3600;

    fn iso(t: i64) -> String {
        DateTime::from_timestamp(t, 0)
            .unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000Z")
            .to_string()
    }

    fn requester(id: i64) -> Value {
        json!({
            "id": id, "email": format!("membre{id}@example.test"), "displayName": format!("Membre{id}"),
            "jellyfinUserId": format!("{id:032x}"), "permissions": 16544
        })
    }

    fn movie(id: i64, user: i64, status: i64, media_status: i64, at: i64) -> Value {
        json!({
            "id": id, "type": "movie", "status": status, "is4k": false,
            "createdAt": iso(at - 2 * H), "updatedAt": iso(at),
            "media": {"tmdbId": 1000 + id, "status": media_status, "mediaAddedAt": iso(at - 10 * MIN),
                      "jellyfinMediaId": format!("jf{id}")},
            "requestedBy": requester(user)
        })
    }

    fn series(id: i64, user: i64, seasons: &[(u64, i64, i64)]) -> Value {
        json!({
            "id": id, "type": "tv", "status": 2, "is4k": false,
            "createdAt": iso(T - 30 * H), "updatedAt": iso(T - 30 * H),
            "media": {"id": 5000 + id, "tmdbId": 2000 + id, "status": 4, "jellyfinMediaId": format!("jf{id}")},
            "seasons": seasons.iter().map(|(n, st, at)| json!({"seasonNumber": n, "status": st, "updatedAt": iso(*at)})).collect::<Vec<_>>(),
            "requestedBy": requester(user)
        })
    }

    fn cfg() -> Cfg {
        Cfg::default()
    }

    /// Saisons des médias quand tout ce qui est terminé est vraiment arrivé (cas normal) : disponibles, à la date de
    /// leur demande de saison.
    fn media_of(requests: &[Value]) -> MediaSeasons {
        requests
            .iter()
            .filter(|r| r["type"] == "tv")
            .map(|r| {
                let seasons = r["seasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["status"] == 5)
                    .map(|s| {
                        (
                            s["seasonNumber"].as_u64().unwrap() as u32,
                            (MEDIA_AVAILABLE, ts(Some(&s["updatedAt"]))),
                        )
                    })
                    .collect();
                (r["media"]["id"].as_i64().unwrap(), seasons)
            })
            .collect()
    }

    fn ready_of(requests: &[Value]) -> Vec<Ready> {
        ready_events(requests, &media_of(requests))
    }

    /// Un passage sans entrée-sortie : nouvelles clés appliquées, puis mails dus.
    fn pass(
        records: &mut BTreeMap<String, MediaMailRecord>,
        requests: &[Value],
        now: i64,
    ) -> Vec<Due> {
        let c = cfg();
        let prep = prepare(
            records,
            &ready_of(requests),
            now,
            c.max_age_hours * H,
            Continuity::default(),
        );
        records.extend(prep.changes);
        due_groups(records, now, c.settle_mins * MIN, c.max_hold_mins * MIN)
    }

    fn mark(records: &mut BTreeMap<String, MediaMailRecord>, due: &[Due], outcome: &str, now: i64) {
        for d in due {
            for k in &d.keys {
                let r = records.get(k).unwrap().clone();
                records.insert(k.clone(), with_outcome(&r, outcome, now));
            }
        }
    }

    #[test]
    fn ready_events_movies_and_requested_seasons_only() {
        let reqs = vec![
            movie(1, 7, 5, 5, T), // film terminé
            movie(2, 7, 2, 3, T), // en cours de téléchargement
            movie(3, 7, 3, 5, T), // refusé
            movie(1, 7, 5, 5, T), // doublon de pagination
            {
                let mut m = movie(4, 7, 5, 5, T);
                m["is4k"] = json!(true);
                m
            },
            movie(5, 8, 2, 5, T), // approuvée, média déjà disponible : date d'ajout
            series(10, 7, &[(0, 2, T), (1, 5, T - H), (2, 2, T), (3, 5, T)]),
            {
                let mut s = series(11, 7, &[(1, 5, T)]);
                s["media"]["status"] = json!(7); // supprimé
                s
            },
        ];
        let r = ready_of(&reqs);
        let keys: Vec<&str> = r.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["r1", "r5", "r10:s1", "r10:s3"]);
        assert_eq!(r[0].ready_at, T);
        assert_eq!(r[1].ready_at, T - 10 * MIN, "mediaAddedAt");
        assert_eq!(r[2].season, Some(1));
        assert_eq!(r[2].ready_at, T - H);
        assert_eq!((r[2].kind, r[2].tmdb, r[2].user), ("tv", 2010, 7));
    }

    #[test]
    fn first_pass_is_a_baseline_and_nothing_old_is_ever_mailed() {
        let reqs = vec![
            movie(1, 7, 5, 5, T - 3 * 86_400),
            movie(2, 7, 5, 5, T - 10 * MIN),
        ];
        let mut records: BTreeMap<_, _> = baseline(&ready_of(&reqs), T).into_iter().collect();
        assert!(records.values().all(|r| r.outcome == BASELINE));
        // même liste, passages suivants : rien de neuf, rien à envoyer, même 2 h plus tard
        for later in [T + 5 * MIN, T + 2 * H] {
            assert!(pass(&mut records, &reqs, later).is_empty());
        }
        assert!(records.values().all(|r| r.outcome == BASELINE));
    }

    #[test]
    fn a_new_title_waits_for_the_member_to_settle_then_goes_once() {
        let mut records = BTreeMap::new();
        let mut reqs = vec![movie(1, 7, 5, 5, T - 3 * 86_400)];
        records.extend(baseline(&ready_of(&reqs), T));
        reqs.push(movie(2, 7, 5, 5, T + 4 * MIN));
        assert!(
            pass(&mut records, &reqs, T + 5 * MIN).is_empty(),
            "vu, pas encore posé"
        );
        assert_eq!(records["r2"].outcome, PENDING);
        assert!(pass(&mut records, &reqs, T + 10 * MIN).is_empty());
        let due = pass(&mut records, &reqs, T + 15 * MIN);
        assert_eq!(
            due,
            vec![Due {
                user: 7,
                keys: vec!["r2".into()]
            }]
        );
        mark(&mut records, &due, SENT, T + 15 * MIN);
        // jamais deux fois : la demande reste disponible, la clé est notée
        for later in [T + 20 * MIN, T + 86_400] {
            assert!(pass(&mut records, &reqs, later).is_empty());
        }
    }

    #[test]
    fn seasons_of_one_request_and_titles_of_one_member_go_together() {
        let mut records = BTreeMap::new();
        records.extend(baseline(&[], T));
        let mut seasons = vec![(1, 5, T)];
        let reqs = |s: &[(u64, i64, i64)]| {
            vec![
                series(20, 7, s),
                movie(21, 7, 5, 5, T),
                movie(22, 8, 5, 5, T),
            ]
        };
        assert!(pass(&mut records, &reqs(&seasons), T + MIN).is_empty());
        // les saisons suivantes finissent toutes les 5 min : le membre 7 attend, le membre 8 est servi seul
        for (i, n) in (2..=4).enumerate() {
            let at = T + (i as i64 + 1) * 5 * MIN;
            seasons.push((n, 5, at));
            let due = pass(&mut records, &reqs(&seasons), at + MIN);
            assert!(due.iter().all(|d| d.user == 8), "{due:?}");
            mark(&mut records, &due, SENT, at + MIN);
        }
        let due = pass(&mut records, &reqs(&seasons), T + 26 * MIN);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].user, 7);
        assert_eq!(due[0].keys, ["r20:s1", "r20:s2", "r20:s3", "r20:s4", "r21"]);
        // un seul mail : la série (4 saisons) puis le film
        let all = reqs(&seasons);
        let by_id: HashMap<i64, &Value> =
            all.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
        let (items, missing) = plan_items(&due[0].keys, &records, &by_id);
        assert!(missing.is_empty());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "tv", "dernier arrivé en avant");
        assert_eq!(items[0].seasons, [1, 2, 3, 4]);
        assert_eq!(items[0].jellyfin_id.as_deref(), Some("jf20"));
        assert_eq!(items[1].keys, ["r21"]);
    }

    #[test]
    fn a_member_who_keeps_receiving_titles_is_not_held_forever() {
        let mut records = BTreeMap::new();
        records.extend(baseline(&[], T));
        let mut reqs = Vec::new();
        let mut sent_at = None;
        for i in 0..20 {
            let at = T + i * 5 * MIN;
            reqs.push(movie(30 + i, 7, 5, 5, at));
            let due = pass(&mut records, &reqs, at + MIN);
            if !due.is_empty() {
                sent_at = Some(at + MIN);
                break;
            }
        }
        // premier vu à T+1 min, nouvelle arrivée toutes les 5 min : départ à l'heure de garde (60 min)
        assert_eq!(sent_at, Some(T + 61 * MIN));
    }

    #[test]
    fn stale_events_are_never_mailed() {
        let mut records = BTreeMap::new();
        records.extend(baseline(&[], T));
        // l'état a été restauré (ou le daemon arrêté) : une demande devenue disponible il y a 3 jours
        let reqs = vec![
            movie(40, 7, 5, 5, T - 3 * 86_400),
            movie(41, 7, 5, 5, T - 47 * H),
        ];
        assert!(pass(&mut records, &reqs, T).is_empty());
        assert_eq!(records["r40"].outcome, STALE);
        assert_eq!(records["r41"].outcome, PENDING);
        let due = pass(&mut records, &reqs, T + 10 * MIN);
        assert_eq!(due.len(), 1, "seule la plus récente");
        assert_eq!(due[0].keys, ["r41"]);
        // et une clé en attente qui dépasse le délai est abandonnée
        let prep = prepare(
            &records,
            &[],
            T + 2 * H,
            cfg().max_age_hours * H,
            Continuity::default(),
        );
        assert_eq!(prep.expired, ["r41"]);
        assert_eq!(prep.changes["r41"].outcome, GAVE_UP);
    }

    #[test]
    fn log_mode_records_what_would_have_gone_so_switching_on_never_backfills() {
        let mut records = BTreeMap::new();
        records.extend(baseline(&[], T));
        let reqs = vec![movie(50, 7, 5, 5, T)];
        assert!(pass(&mut records, &reqs, T + MIN).is_empty());
        let due = pass(&mut records, &reqs, T + 15 * MIN);
        assert_eq!(due.len(), 1);
        assert_eq!(mode(false, false), Mode::Log);
        mark(&mut records, &due, LOGGED, T + 15 * MIN);
        // l'admin passe send_to_members à true : rien de l'arriéré ne part
        assert_eq!(mode(false, true), Mode::Send);
        assert!(pass(&mut records, &reqs, T + 20 * MIN).is_empty());
        // ce qui arrive ensuite part
        let mut reqs = reqs;
        reqs.push(movie(51, 7, 5, 5, T + 30 * MIN));
        assert!(pass(&mut records, &reqs, T + 31 * MIN).is_empty());
        let due = pass(&mut records, &reqs, T + 45 * MIN);
        assert_eq!(due[0].keys, ["r51"]);
        assert_eq!(mode(true, true), Mode::DryRun);
    }

    #[test]
    fn failed_send_is_retried_later_then_given_up_and_never_duplicated() {
        let retry = cfg().retry_mins;
        let mut r = record(
            &Ready {
                key: "r60".into(),
                request: 60,
                kind: "movie",
                tmdb: 1,
                season: None,
                ready_at: T,
                user: 7,
            },
            T,
            PENDING,
        );
        let mut records = BTreeMap::new();
        let mut at = T + 15 * MIN;
        for (i, wait) in retry.iter().enumerate() {
            r = after_failure(&r, at, &retry);
            assert_eq!((r.outcome.as_str(), r.tries), (ERROR, i as u32 + 1));
            assert_eq!(r.next_at, Some(at + wait * MIN));
            records.insert("r60".to_string(), r.clone());
            assert!(
                due_groups(&records, at + wait * MIN - 1, 600, 3600).is_empty(),
                "pas avant l'heure"
            );
            at += wait * MIN;
            let due = due_groups(&records, at, 600, 3600);
            assert_eq!(
                due,
                vec![Due {
                    user: 7,
                    keys: vec!["r60".into()]
                }]
            );
        }
        r = after_failure(&r, at, &retry);
        assert_eq!((r.outcome.as_str(), r.next_at), (GAVE_UP, None));
        records.insert("r60".to_string(), r);
        assert!(due_groups(&records, at + 86_400, 600, 3600).is_empty());
        // un passage coupé pendant curl : jamais renvoyé
        let mut s = records["r60"].clone();
        s.outcome = SENDING.into();
        records.insert("r60".into(), s);
        let prep = prepare(&records, &[], at, 48 * H, Continuity::default());
        assert_eq!(prep.uncertain, ["r60"]);
        assert_eq!(prep.changes["r60"].outcome, SENT_UNCERTAIN);
        records.extend(prep.changes);
        assert!(due_groups(&records, at + 86_400, 600, 3600).is_empty());
    }

    #[test]
    fn a_retry_waits_for_new_titles_of_the_same_member() {
        let mut records = BTreeMap::new();
        let mut err = record(
            &Ready {
                key: "r70".into(),
                request: 70,
                kind: "movie",
                tmdb: 1,
                season: None,
                ready_at: T,
                user: 7,
            },
            T,
            PENDING,
        );
        err = after_failure(&err, T, &[15]);
        records.insert("r70".to_string(), err);
        let mut fresh = records["r70"].clone();
        fresh.outcome = PENDING.into();
        fresh.tries = 0;
        fresh.seen_at = T + 14 * MIN;
        records.insert("r71".to_string(), fresh);
        assert!(
            due_groups(&records, T + 16 * MIN, 600, 3600).is_empty(),
            "r71 pas encore posé"
        );
        let due = due_groups(&records, T + 24 * MIN, 600, 3600);
        assert_eq!(due[0].keys, ["r71", "r70"]);
    }

    #[test]
    fn oldest_members_first() {
        let mut records = BTreeMap::new();
        for (k, user, seen) in [
            ("r1", 9, T - 30 * MIN),
            ("r2", 3, T - 20 * MIN),
            ("r3", 5, T - 50 * MIN),
        ] {
            records.insert(
                k.to_string(),
                MediaMailRecord {
                    at: seen,
                    seen_at: seen,
                    ready_at: seen,
                    user,
                    kind: "movie".into(),
                    tmdb: 1,
                    outcome: PENDING.into(),
                    tries: 0,
                    next_at: None,
                },
            );
        }
        let users: Vec<i64> = due_groups(&records, T, 600, 3600)
            .iter()
            .map(|d| d.user)
            .collect();
        assert_eq!(users, [5, 9, 3]);
    }

    #[test]
    fn recipient_checks_and_opt_out() {
        let req = movie(1, 7, 5, 5, T);
        let who = recipient_of(&req).unwrap();
        assert_eq!(who.email.as_deref(), Some("membre7@example.test"));
        assert_eq!(who.name, "Membre7");
        assert_eq!(
            who.jellyfin_id.as_deref(),
            Some(format!("{:032x}", 7).as_str())
        );
        let ok = Account {
            disabled: false,
            name: "membre7".into(),
        };
        let off = Account {
            disabled: true,
            ..ok.clone()
        };
        assert_eq!(recipient_verdict(&who, false, Some(&ok)), None);
        assert_eq!(recipient_verdict(&who, true, Some(&ok)), Some(OPTED_OUT));
        assert_eq!(recipient_verdict(&who, false, Some(&off)), Some(SUSPENDED));
        assert_eq!(recipient_verdict(&who, false, None), Some(NO_ACCOUNT));
        let mut bad = req.clone();
        bad["requestedBy"]["email"] = json!("PseudoSansArobase");
        let nobody = recipient_of(&bad).unwrap();
        assert_eq!(recipient_verdict(&nobody, false, Some(&ok)), Some(NO_EMAIL));
        // jamais l'adresse comme nom
        bad["requestedBy"]["displayName"] = json!("membre7@example.test");
        bad["requestedBy"]["jellyfinUsername"] = json!("Nina");
        assert_eq!(recipient_of(&bad).unwrap().name, "Nina");

        // case « disponible » de Jellyseerr : 8190 = tout coché, absent = tout
        assert!(!email_opted_out(Some(8190)));
        assert!(!email_opted_out(None));
        assert!(email_opted_out(Some(0)));
        assert!(email_opted_out(Some(8190 & !SEERR_MEDIA_AVAILABLE)));
        assert!(!email_opted_out(Some(SEERR_MEDIA_AVAILABLE)));
        // MEDIA_MAIL_OPTOUT : adresse (le « stop » reçu par mail ne porte qu'elle), pseudo (sans casse) ou id
        let list = vec![
            "nina".to_string(),
            "0000-abcd".to_string(),
            "membre7@example.test".to_string(),
        ];
        assert!(listed_out(&list, &["Nina", ""], None, None));
        assert!(listed_out(&list, &["", "x"], Some("0000abcd"), None));
        assert!(listed_out(&list, &["Lea", ""], None, who.email.as_deref()));
        assert!(!listed_out(
            &list,
            &["Lea", ""],
            Some("1234"),
            Some("lea@example.test")
        ));
        assert!(!listed_out(&[], &["", ""], None, Some("")));

        let users = vec![
            json!({"Id": "ABCD-0001", "Name": "Nina", "Policy": {"IsDisabled": false}}),
            json!({"Id": "abcd0002", "Name": "Lea", "Policy": {"IsDisabled": true}}),
        ];
        let a = accounts_by_id(&users);
        assert!(!a["abcd0001"].disabled && a["abcd0002"].disabled);
    }

    #[test]
    fn titles_are_read_from_seerr_and_jellyfin() {
        let d = json!({
            "title": "Le Menu", "originalTitle": "The Menu", "releaseDate": "2022-11-17", "runtime": 106,
            "voteAverage": 7.18, "overview": "Sur une île isolée…", "genres": [{"name": "Comédie"}, {"name": "Horreur"}]
        });
        let item = json!({"Name": "Le Menu", "MediaStreams": [
            {"Type": "Video", "Height": 816}, {"Type": "Audio", "Language": "fra"}, {"Type": "Audio", "Language": "eng"},
            {"Type": "Subtitle", "Language": "fra"}
        ]});
        let t = movie_title(
            &d,
            &item,
            watch_url("https://tv.example.test/", "abc"),
            Some(T),
        );
        assert_eq!(t.title, "Le Menu");
        assert_eq!(t.years.as_deref(), Some("2022"));
        assert_eq!(t.runtime_mins, Some(106));
        assert_eq!(t.lang.as_deref(), Some("VF · VO"));
        assert_eq!(t.genres, ["Comédie", "Horreur"]);
        assert_eq!(t.watch_url, "https://tv.example.test/web/#/details?id=abc");

        let tv = json!({
            "name": "Dexter", "originalName": "Dexter", "firstAirDate": "2006-10-01", "lastAirDate": "2013-09-22",
            "status": "Ended", "numberOfSeasons": 8, "voteAverage": 8.241,
            "seasons": [{"seasonNumber": 0, "episodeCount": 42}, {"seasonNumber": 7, "episodeCount": 12},
                        {"seasonNumber": 8, "episodeCount": 12, "posterPath": "/s8.jpg"}]
        });
        let ep = json!({"MediaStreams": [{"Type": "Audio", "Language": "eng"}, {"Type": "Subtitle", "Language": "fre"}]});
        let t = series_title(&tv, &[7, 8], Some(&ep), "u".into(), None);
        assert_eq!(t.episodes, Some(24));
        assert_eq!(t.years.as_deref(), Some("2006–2013"));
        assert_eq!(t.lang.as_deref(), Some("VOSTFR"));
        assert!(!t.whole_series);
        assert_eq!(tmdb_season_poster(&tv, 8).as_deref(), Some("/s8.jpg"));
        assert_eq!(
            tmdb_url("w342", Some("/s8.jpg")).unwrap(),
            "https://image.tmdb.org/t/p/w342/s8.jpg"
        );
        assert_eq!(tmdb_url("w342", Some("")), None);

        let mini = json!({"name": "Chernobyl", "firstAirDate": "2019-05-06", "lastAirDate": "2019-06-03",
                          "status": "Ended", "numberOfSeasons": 1, "seasons": [{"seasonNumber": 1, "episodeCount": 5}]});
        let t = series_title(&mini, &[1], None, "u".into(), None);
        assert!(t.whole_series);
        assert_eq!(
            (t.years.as_deref(), t.episodes, t.lang),
            (Some("2019"), Some(5), None)
        );

        assert_eq!(
            series_years("2019-01-01", "", "Returning Series").as_deref(),
            Some("depuis 2019")
        );
        assert_eq!(series_years("", "", "Ended"), None);
        assert_eq!(
            lang_chip(&[json!({"Type": "Audio", "Language": "fre"})]).as_deref(),
            Some("VF")
        );
        assert_eq!(
            lang_chip(&[json!({"Type": "Audio", "Language": "jpn"})]).as_deref(),
            Some("VO")
        );
        assert_eq!(
            lang_chip(&[json!({"Type": "Audio", "Language": "und"})]),
            None
        );
    }

    #[test]
    fn housekeeping_forgets_only_finished_keys_of_vanished_requests() {
        let rec = |outcome: &str, at: i64| MediaMailRecord {
            at,
            outcome: outcome.into(),
            ..Default::default()
        };
        let mut records = BTreeMap::new();
        records.insert("r1".to_string(), rec(SENT, T - 100 * 86_400));
        records.insert("r2:s1".to_string(), rec(BASELINE, T - 100 * 86_400));
        records.insert("r3".to_string(), rec(SENT, T - 10 * 86_400));
        records.insert("r4".to_string(), rec(PENDING, T - 100 * 86_400));
        let live: HashSet<i64> = [2].into();
        assert_eq!(prune(&records, &live, T, 90 * 86_400), ["r1"]);
        assert_eq!(request_of("r273:s5"), Some(273));
        assert_eq!(season_of("r273:s5"), Some(5));
        assert_eq!(season_of("r274"), None);
    }

    #[test]
    fn hidden_titles_wait_then_are_dropped() {
        let r = MediaMailRecord {
            seen_at: T,
            ..Default::default()
        };
        assert_eq!(hidden_outcome(&r, T + 59 * MIN, 3600), None);
        assert_eq!(hidden_outcome(&r, T + 60 * MIN, 3600), Some(NOT_VISIBLE));
    }

    #[test]
    fn old_state_files_load_without_the_new_fields() {
        let s: crate::state::State = serde_json::from_str("{}").unwrap();
        assert!(s.media_mail.is_empty() && s.media_mail_baseline_at.is_none());
        assert!(s.media_mail_last_pass.is_none());
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v.get("media_mail_last_pass").is_none(),
            "absent tant qu'aucun passage"
        );
        let r: MediaMailRecord = serde_json::from_value(json!({
            "at": 1, "seen_at": 1, "ready_at": 1, "user": 7, "kind": "movie", "tmdb": 3, "outcome": "sent"
        }))
        .unwrap();
        assert_eq!((r.tries, r.next_at), (0, None));
        // champs vides non écrits : l'état grossit peu (≈ 280 clés à la référence)
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("tries").is_none() && v.get("next_at").is_none());
    }
    /// Jellyseerr termine la demande d'une saison supprimée comme celle d'une saison arrivée (`MediaSubscriber`,
    /// 3.5) : seule la saison du média fait foi, et son arrivée réelle, plus tard, donne le mail.
    #[test]
    fn a_deleted_season_is_never_announced_and_its_real_arrival_is() {
        let req = series(80, 7, &[(1, 5, T - 3 * 86_400), (2, 5, T - 10 * MIN)]);
        let mut media: MediaSeasons = HashMap::new();
        media.insert(
            5080,
            [
                (1, (MEDIA_AVAILABLE, T - 3 * 86_400)),
                (2, (MEDIA_DELETED, T - 10 * MIN)),
            ]
            .into(),
        );
        let keys = |m: &MediaSeasons| -> Vec<String> {
            ready_events(std::slice::from_ref(&req), m)
                .into_iter()
                .map(|r| r.key)
                .collect()
        };
        assert_eq!(
            keys(&media),
            ["r80:s1"],
            "saison 2 supprimée : pas disponible"
        );
        // incomplète, ou média absent de la liste lue : pas encore
        media.get_mut(&5080).unwrap().insert(2, (4, T));
        assert_eq!(keys(&media), ["r80:s1"]);
        assert!(keys(&HashMap::new()).is_empty());
        // la saison arrive vraiment trois jours après sa suppression : la date est celle de son arrivée
        media
            .get_mut(&5080)
            .unwrap()
            .insert(2, (MEDIA_AVAILABLE, T + 3 * 86_400));
        let r = ready_events(std::slice::from_ref(&req), &media);
        assert_eq!(r[1].key, "r80:s2");
        assert_eq!(r[1].ready_at, T + 3 * 86_400);
        let mut records: BTreeMap<_, _> = baseline(&r[..1], T).into_iter().collect();
        let prep = prepare(
            &records,
            &r,
            T + 3 * 86_400 + MIN,
            48 * H,
            Continuity::default(),
        );
        assert_eq!(prep.changes["r80:s2"].outcome, PENDING);
        records.extend(prep.changes);

        // demande de série en échec : ses saisons arrivées comptent quand même
        let mut failed = series(81, 7, &[(1, 5, T), (2, 2, T)]);
        failed["status"] = json!(4);
        let r = ready_of(std::slice::from_ref(&failed));
        assert_eq!(
            r.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(),
            ["r81:s1"]
        );
        // un film en échec, non
        assert!(ready_of(&[movie(82, 7, 4, 5, T)]).is_empty());
    }

    #[test]
    fn media_seasons_are_read_from_the_media_list() {
        let m = media_seasons(&[
            json!({"id": 5, "seasons": [{"seasonNumber": 1, "status": 5, "updatedAt": iso(T)},
                                        {"seasonNumber": 2, "status": 7, "updatedAt": iso(T + H)}]}),
            json!({"id": 6, "mediaType": "movie", "seasons": []}),
            json!({"seasons": []}),
        ]);
        assert_eq!(m[&5][&1], (MEDIA_AVAILABLE, T));
        assert_eq!(m[&5][&2], (MEDIA_DELETED, T + H));
        assert!(m[&6].is_empty());
        assert_eq!(m.len(), 2);
    }

    /// Bascule `send_to_members = true` : ce qui attendait pendant les passages sans envoi a déjà eu le mail de
    /// Jellyseerr (encore actif) ; seul ce qui arrive à partir de la bascule part.
    #[test]
    fn switching_sending_on_hands_over_what_seerr_already_mailed() {
        let log = MediaMailPass { at: T, send: false };
        let send = MediaMailPass { at: T, send: true };
        let gap = cfg().max_gap_mins * MIN;
        assert!(continuity(Some(log), T + 5 * MIN, Mode::Send, gap).switch_on);
        assert!(continuity(None, T + 5 * MIN, Mode::Send, gap).switch_on);
        assert!(!continuity(Some(send), T + 5 * MIN, Mode::Send, gap).switch_on);
        assert!(!continuity(Some(log), T + 5 * MIN, Mode::Log, gap).switch_on);
        assert!(!continuity(Some(log), T + 5 * MIN, Mode::DryRun, gap).switch_on);

        let mut records = BTreeMap::new();
        records.extend(baseline(&[], T));
        let mut reqs = vec![movie(90, 7, 5, 5, T - 20 * MIN)];
        assert!(pass(&mut records, &reqs, T).is_empty());
        assert_eq!(records["r90"].outcome, PENDING);
        reqs.push(movie(91, 8, 5, 5, T + 4 * MIN));
        let c = continuity(Some(log), T + 5 * MIN, Mode::Send, gap);
        let prep = prepare(&records, &ready_of(&reqs), T + 5 * MIN, 48 * H, c);
        assert_eq!(prep.handed_over, ["r90"]);
        assert_eq!(prep.changes["r90"].outcome, LOGGED);
        assert_eq!(
            prep.changes["r91"].outcome, PENDING,
            "arrivé à la bascule : à nous"
        );
        records.extend(prep.changes);
        let due = due_groups(&records, T + 20 * MIN, 600, 3600);
        assert_eq!(
            due,
            vec![Due {
                user: 8,
                keys: vec!["r91".into()]
            }]
        );
    }

    /// État restauré d'une sauvegarde (ou daemon longtemps arrêté) : ce qui est arrivé pendant l'écart a peut-être
    /// été envoyé par l'état perdu, jamais renvoyé.
    #[test]
    fn a_gap_in_the_state_marks_titles_of_the_gap_stale() {
        let last = MediaMailPass { at: T, send: true };
        // `[tasks.media_mail] max_gap_mins` : 120 par défaut, l'écart d'avant (code) gardé
        let gap = cfg().max_gap_mins * MIN;
        assert_eq!(gap, 2 * H);
        assert_eq!(
            continuity(Some(last), T + 10 * MIN, Mode::Send, gap).gap_since,
            None
        );
        // un redémarrage ou une installation : quelques minutes ; l'écart commence au-delà de la limite
        assert_eq!(
            continuity(Some(last), T + 2 * H, Mode::Send, gap).gap_since,
            None
        );
        assert_eq!(
            continuity(Some(last), T + 2 * H + 1, Mode::Send, gap).gap_since,
            Some(T)
        );
        // la limite vient du réglage
        assert_eq!(
            continuity(Some(last), T + 31 * MIN, Mode::Send, 30 * MIN).gap_since,
            Some(T)
        );
        let c = continuity(Some(last), T + 30 * H, Mode::Send, gap);
        assert_eq!(c.gap_since, Some(T));
        let reqs = vec![
            movie(100, 7, 5, 5, T - H),      // avant le dernier passage noté : normal
            movie(101, 7, 5, 5, T + 20 * H), // pendant l'écart
        ];
        let prep = prepare(&BTreeMap::new(), &ready_of(&reqs), T + 30 * H, 48 * H, c);
        assert_eq!(prep.changes["r100"].outcome, PENDING);
        assert_eq!(prep.changes["r101"].outcome, STALE);
        assert_eq!((prep.gap_stale, prep.new_pending), (1, 1));
    }

    /// État restauré : ce qui attendait encore dans la sauvegarde (`pending`, `error`) a très probablement été envoyé
    /// par l'état perdu. Noté `sent_uncertain` sans mail (au plus un mail), ni abandon signalé, même trop ancien.
    #[test]
    fn a_gap_in_the_state_settles_what_the_restored_state_was_holding() {
        let last = MediaMailPass { at: T, send: true };
        let c = cfg();
        let held = |key: &str, id: i64, user: i64, ready_at: i64, outcome: &str| {
            let mut r = record(
                &Ready {
                    key: key.into(),
                    request: id,
                    kind: "movie",
                    tmdb: 1000 + id,
                    season: None,
                    ready_at,
                    user,
                },
                T - 5 * MIN,
                outcome,
            );
            if outcome == ERROR {
                r.tries = 1;
                r.next_at = Some(T - MIN);
            }
            (key.to_string(), r)
        };
        let records: BTreeMap<String, MediaMailRecord> = [
            held("r120", 120, 7, T - 20 * MIN, PENDING),
            held("r121", 121, 8, T - 30 * MIN, ERROR),
            held("r122", 122, 9, T - 47 * H, PENDING), // dépasse `max_age_hours` pendant l'écart
            held("r123", 123, 9, T - 2 * H, SENT),
        ]
        .into();
        let reqs = vec![
            movie(120, 7, 5, 5, T - 20 * MIN),
            movie(121, 8, 5, 5, T - 30 * MIN),
            movie(122, 9, 5, 5, T - 47 * H),
            movie(123, 9, 5, 5, T - 2 * H),
        ];
        let now = T + 3 * H;
        // sans écart, ils partiraient (ou l'abandon serait signalé)
        let normal = prepare(
            &records,
            &ready_of(&reqs),
            now,
            c.max_age_hours * H,
            Continuity::default(),
        );
        assert_eq!(normal.expired, ["r122"]);
        let mut after = records.clone();
        after.extend(normal.changes);
        assert_eq!(
            due_groups(&after, now, c.settle_mins * MIN, c.max_hold_mins * MIN).len(),
            2
        );
        // écart : tout ce qui attendait passe en `sent_uncertain`, aucun mail dû, aucune alerte
        let cont = continuity(Some(last), now, Mode::Send, c.max_gap_mins * MIN);
        assert_eq!(cont.gap_since, Some(T));
        let prep = prepare(&records, &ready_of(&reqs), now, c.max_age_hours * H, cont);
        assert_eq!(prep.gap_held, ["r120", "r121", "r122"]);
        assert!(prep.expired.is_empty() && prep.uncertain.is_empty());
        for k in ["r120", "r121", "r122"] {
            assert_eq!(prep.changes[k].outcome, SENT_UNCERTAIN, "{k}");
            assert_eq!(prep.changes[k].next_at, None, "{k}");
        }
        assert!(!prep.changes.contains_key("r123"));
        let mut settled = records.clone();
        settled.extend(prep.changes);
        assert!(due_groups(&settled, now, c.settle_mins * MIN, c.max_hold_mins * MIN).is_empty());
        // écart et bascule vers l'envoi au même passage : l'écart l'emporte, même suite
        let switch = continuity(
            Some(MediaMailPass { at: T, send: false }),
            now,
            Mode::Send,
            c.max_gap_mins * MIN,
        );
        assert!(switch.switch_on && switch.gap_since.is_some());
        let both = prepare(&records, &ready_of(&reqs), now, c.max_age_hours * H, switch);
        assert_eq!(both.gap_held, ["r120", "r121", "r122"]);
        assert!(both.handed_over.is_empty());
    }

    /// Alerte d'abandon (CLAUDE.md : « signalée » seulement si elle est partie) : ses lignes attendent dans l'état
    /// jusqu'à ce qu'une alerte parte ; une seule alerte par passage reprend tout ce qui attend, sans doublon.
    #[test]
    fn an_abandon_alert_waits_in_the_state_until_it_goes_out() {
        let c = cfg();
        // une clé en attente depuis plus de `max_age_hours` est abandonnée, sa ligne d'alerte part avec elle
        let mut records = BTreeMap::new();
        records.insert(
            "r60".to_string(),
            record(
                &Ready {
                    key: "r60".into(),
                    request: 60,
                    kind: "movie",
                    tmdb: 1060,
                    season: None,
                    ready_at: T - 49 * H,
                    user: 7,
                },
                T - 49 * H,
                PENDING,
            ),
        );
        let prep = prepare(&records, &[], T, c.max_age_hours * H, Continuity::default());
        assert_eq!(prep.expired, ["r60"]);
        assert_eq!(prep.changes["r60"].outcome, GAVE_UP);
        let line = expired_line("r60", &records["r60"], c.max_age_hours);
        assert_eq!(
            line,
            "r60 (movie TMDB 1060, compte Jellyseerr 7) : disponible depuis plus de 48 h sans pouvoir partir"
        );

        // passage 1 : l'abandon et un échec d'envoi définitif d'un mail de deux clés ; l'alerte ne part pas
        let mut pending = BTreeMap::new();
        queue_alert(&mut pending, "r60".into(), line.clone());
        queue_alert(
            &mut pending,
            "r61,r62".into(),
            "r61, r62 : « X » pour Membre8 (compte Jellyseerr 8) : 4 échecs d'envoi, dernier : délai\ndépassé".into(),
        );
        let (subject, body) = alert_message(&pending).unwrap();
        assert_eq!(subject, "Mails « disponible » abandonnés : 2");
        assert!(body.contains(&line) && body.contains("dernier : délai dépassé"));
        let sent = pending.clone();
        let retry = alerts::retry_later((false, false), true);
        assert!(retry, "canal configuré mais rien n'est parti");
        settle_alerts(&mut pending, &sent, retry);
        assert_eq!(pending, sent, "rien d'effacé : l'alerte repartira");

        // passage 2 : la même ligne n'entre pas deux fois ; un nouvel abandon s'ajoute ; une seule alerte pour tout
        queue_alert(&mut pending, "r60".into(), line.clone());
        queue_alert(&mut pending, "r63".into(), "r63 : autre".into());
        let (subject, body) = alert_message(&pending).unwrap();
        assert_eq!(subject, "Mails « disponible » abandonnés : 3");
        assert_eq!(body.matches(line.as_str()).count(), 1);
        // partie : effacé ce qu'elle reprenait, pas ce qui a été noté depuis sa lecture
        let sent = pending.clone();
        queue_alert(
            &mut pending,
            "r64".into(),
            "r64 : noté pendant l'envoi".into(),
        );
        settle_alerts(
            &mut pending,
            &sent,
            alerts::retry_later((true, false), true),
        );
        assert_eq!(pending.keys().collect::<Vec<_>>(), ["r64"]);
        // aucun canal configuré : réessayer n'y changerait rien, effacé aussi (comme les autres alertes)
        let sent = pending.clone();
        settle_alerts(
            &mut pending,
            &sent,
            alerts::retry_later((false, false), false),
        );
        assert!(pending.is_empty() && alert_message(&pending).is_none());

        // id de demande réutilisé et abandonné de nouveau avant le départ de la première alerte : les deux lignes
        queue_alert(&mut pending, "r70".into(), "r70 : film A".into());
        queue_alert(&mut pending, "r70".into(), "r70 : film B".into());
        queue_alert(&mut pending, "r70".into(), "r70 : film A".into());
        assert_eq!(pending["r70"], "r70 : film A\nr70 : film B");
        assert_eq!(
            alert_message(&pending).unwrap().0,
            "Mails « disponible » abandonnés : 2"
        );
    }

    #[test]
    fn abandon_alerts_load_from_old_states_and_old_binaries_ignore_them() {
        // état d'avant : rien en attente, et rien d'écrit tant que rien n'attend
        let s: crate::state::State = serde_json::from_str("{}").unwrap();
        assert!(s.media_mail_alerts.is_empty());
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("media_mail_alerts").is_none());
        // en attente : relu tel quel après un redémarrage
        let mut s = s;
        queue_alert(&mut s.media_mail_alerts, "r60".into(), "r60 : ligne".into());
        let back: crate::state::State =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.media_mail_alerts, s.media_mail_alerts);
        // un ancien binaire (même `State`, sans ce champ) ignore les champs qu'il ne connaît pas
        let old: crate::state::State =
            serde_json::from_str(r#"{"media_mail_alerts":{"r60":"x"},"champ_futur":{"a":1}}"#)
                .unwrap();
        assert_eq!(old.media_mail_alerts["r60"], "x");
    }

    #[test]
    fn reused_request_ids_and_new_requesters_are_followed() {
        let mut records = BTreeMap::new();
        records.extend(baseline(
            &ready_of(&[movie(110, 7, 5, 5, T - 5 * 86_400)]),
            T,
        ));
        assert!(pass(&mut records, &[movie(111, 7, 5, 5, T)], T + MIN).is_empty());
        // base Jellyseerr restaurée : l'id 110 désigne maintenant un autre film, arrivé à l'instant
        let mut other = movie(110, 9, 5, 5, T + 2 * MIN);
        other["media"]["tmdbId"] = json!(4242);
        // l'admin a réattribué la demande 111 au membre 8
        let mut moved = movie(111, 8, 5, 5, T);
        moved["requestedBy"] = requester(8);
        let prep = prepare(
            &records,
            &ready_of(&[other, moved]),
            T + 3 * MIN,
            48 * H,
            Continuity::default(),
        );
        assert_eq!(prep.replaced, ["r110"]);
        assert_eq!(
            (
                prep.changes["r110"].outcome.as_str(),
                prep.changes["r110"].tmdb,
                prep.changes["r110"].user
            ),
            (PENDING, 4242, 9)
        );
        assert_eq!(prep.reassigned, ["r111"]);
        assert_eq!(prep.changes["r111"].user, 8);
        assert_eq!(prep.changes["r111"].seen_at, records["r111"].seen_at);
        records.extend(prep.changes);
        let users: Vec<i64> = due_groups(&records, T + 20 * MIN, 600, 3600)
            .iter()
            .map(|d| d.user)
            .collect();
        assert_eq!(users, [8, 9]);
    }
}
