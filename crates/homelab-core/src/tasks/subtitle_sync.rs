//! Sous-titres incrustés des fichiers de la **seedbox** → fichiers externes à côté de la vidéo, **à codec
//! identique**, extraits **sur la seedbox** (disque local, rien sur le lien VPS), puis fiche Jellyfin relue.
//!
//! Pourquoi : Jellyfin extrait une piste incrustée en relisant tout le fichier par le lien (108 à 757 s mesurés
//! du 19 au 21/09) ; les membres attendaient plusieurs minutes, ou abandonnaient. Un fichier externe est lu
//! instantanément, et l'ASS externe est rendu exactement comme l'ASS incrusté (libass), panneaux à leur place.
//! Un fichier annexe n'est vu que par un **FullRefresh** de l'item (ni `Library/Media/Updated`, ni Refresh
//! « Default », vérifié le 21/09) : `jellyfin::refresh_streams` après chaque extraction.
//!
//! Sorties : `.fr.default.ass` (piste complète, passe devant), `.fr.forced.ass`, `.fr.hi.ass` (malentendants,
//! à part), `.fr.srt` pour une piste SRT ; pour une piste ASS, un `.fr.srt` **sans les panneaux** est dérivé
//! (AirPlay, téléviseurs). Les plus récents d'abord, `max_per_run` items par passage, jamais pendant une lecture.
//!
//! Lecture de la médiathèque : un balayage complet par jour (`full_scan_hour`, et au premier passage après un
//! démarrage), seul passage qui élague tous les `subtitle_tries` d'un coup (un passage court n'oublie que les essais
//! des éléments qu'il a relus et qui n'ont plus rien à extraire, `prune_seen`, 2026-10-08). Les autres passages ne
//! relisent que les éléments sauvés par
//! Jellyfin depuis le passage précédent (`MinDateLastSaved`, marge d'une heure : un nouvel élément, une analyse, un
//! FullRefresh), le reliquat du passage précédent et les essais dont la relance est due. Le plan d'un élément
//! (`plan_jobs`) ne dépend que de son chemin et de ses pistes : il ne change pas sans que Jellyfin le sauve. Avant le
//! 07/10, chaque passage relisait les ~2 400 éléments avec leurs pistes : 9 pages de 1,8 Mo, ~14 s de CPU Jellyfin
//! toutes les 5 min, pour « rien à extraire » 98 fois sur 100.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use serde_json::Value;
use tracing::{info, warn};

use super::seedbox_refresh::refresh_params;
use super::{Report, Task};
use crate::config::{Config, Seedbox};
use crate::context::TaskContext;
use crate::docker;
use crate::state::SubtitleTry;

pub struct SubtitleSync;

/// Marge d'un passage court : un élément sauvé jusqu'à une heure avant le début du passage précédent est relu
/// (horloges, sauvegarde d'une analyse commencée avant le passage).
const INCREMENTAL_MARGIN_SECS: i64 = 3600;

/// Ce que la tâche garde d'un passage à l'autre. En mémoire seulement : après un redémarrage, le premier passage
/// est un balayage complet.
#[derive(Debug, Default)]
struct Memo {
    /// Début du dernier passage abouti (secondes).
    last_pass: Option<i64>,
    /// Dernier balayage complet abouti (heure locale).
    last_full: Option<NaiveDateTime>,
    /// Éléments à extraire restés sans essai noté (au-delà de `max_per_run`, en lecture, budget atteint, échec
    /// passager) : relus au passage suivant même si Jellyfin ne les a pas modifiés.
    backlog: HashSet<String>,
}

fn memo() -> &'static Mutex<Memo> {
    static MEMO: OnceLock<Mutex<Memo>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(Memo::default()))
}

/// Balayage complet ? Au premier passage (rien en mémoire), puis une fois par jour : au premier passage qui suit
/// `hour` (heure locale), hors pointe.
pub fn full_scan_due(last_full: Option<NaiveDateTime>, now: NaiveDateTime, hour: u32) -> bool {
    let Some(last) = last_full else {
        return true;
    };
    let Some(today) = now.date().and_hms_opt(hour.min(23), 0, 0) else {
        return true;
    };
    let mark = if now >= today {
        today
    } else {
        today - chrono::Duration::days(1)
    };
    last < mark
}

/// Seuil `MinDateLastSaved` d'un passage court : début du passage précédent moins la marge, en UTC.
pub fn saved_since(last_pass: i64) -> String {
    chrono::DateTime::from_timestamp(last_pass - INCREMENTAL_MARGIN_SECS, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Éléments à relire par leur id lors d'un passage court : le reliquat du passage précédent et les essais dont la
/// relance est due (triés : requêtes stables).
pub fn ids_to_recheck(
    backlog: &HashSet<String>,
    tries: &BTreeMap<String, SubtitleTry>,
    now: i64,
    retry: i64,
    failed_retry: i64,
) -> Vec<String> {
    let mut ids: BTreeSet<String> = backlog.iter().cloned().collect();
    ids.extend(
        tries
            .iter()
            .filter(|(_, t)| due(Some(t), now, retry, failed_retry))
            .map(|(id, _)| id.clone()),
    );
    ids.into_iter().collect()
}

/// Essais en attente de relance (pas encore dus), et parmi eux ceux sans piste extractible.
pub fn waiting_counts<'a>(
    tries: impl Iterator<Item = &'a SubtitleTry>,
    now: i64,
    retry: i64,
    failed_retry: i64,
) -> (usize, usize) {
    tries
        .filter(|t| !due(Some(t), now, retry, failed_retry))
        .fold((0, 0), |(w, n), t| (w + 1, n + usize::from(t.no_track)))
}

/// Essais à oublier lors d'un passage **court** : ceux d'éléments relus ce passage (`read`) qui n'ont plus rien à
/// extraire (absents de `pending`, les éléments au plan non vide). Un élément extrait avec succès garde son essai
/// pendant `retry_hours` ; sans ça il restait compté « en attente de relance » (3 → 16 après 13 extractions, vu le
/// 2026-10-08) et relu par Ids après 6 h, jusqu'au balayage de 05:00 qui seul élaguait. Un élément non relu garde
/// son essai : une liste partielle n'en dit rien.
pub fn prune_seen(
    tries: &BTreeMap<String, SubtitleTry>,
    read: &HashSet<String>,
    pending: &HashSet<String>,
) -> HashSet<String> {
    tries
        .keys()
        .filter(|id| read.contains(*id) && !pending.contains(*id))
        .cloned()
        .collect()
}

/// Reliquat : éléments dus ce passage pour lesquels aucun essai n'a été noté.
pub fn backlog_after(due_ids: &HashSet<String>, recorded: &HashSet<String>) -> HashSet<String> {
    due_ids.difference(recorded).cloned().collect()
}

/// Mémorise un passage abouti.
fn remember(pass_start: i64, full: Option<NaiveDateTime>, backlog: HashSet<String>) {
    let mut m = memo().lock().unwrap_or_else(|e| e.into_inner());
    m.last_pass = Some(pass_start);
    if full.is_some() {
        m.last_full = full;
    }
    m.backlog = backlog;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// `ass` ou `subrip` (nom de codec ffmpeg, tel que Jellyfin le rapporte).
    pub codec: String,
    /// `full`, `forced` ou `hi`.
    pub kind: String,
    /// Chemin de sortie **côté Jellyfin** (`/seedbox/media/...`).
    pub out: String,
    /// SRT sans panneaux à dériver (piste ASS complète seulement).
    pub srt: Option<String>,
}

fn is_french(s: &Value) -> bool {
    matches!(
        s.get("Language").and_then(Value::as_str),
        Some("fre") | Some("fra") | Some("fr")
    )
}

fn flag(s: &Value, key: &str) -> bool {
    s.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Kind d'une piste d'après ses drapeaux Jellyfin (titre en secours : « Malentendants », « SDH », « Forced »).
fn kind_of(s: &Value) -> &'static str {
    let title = s
        .get("Title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if flag(s, "IsForced") || title.contains("forc") {
        "forced"
    } else if flag(s, "IsHearingImpaired")
        || title.contains("malentendant")
        || title.contains("sdh")
        || title.contains("[cc]")
    {
        "hi"
    } else {
        "full"
    }
}

/// Chemin de la vidéo sans extension → nom du fichier externe pour (codec, kind).
pub fn out_name(stem: &str, codec: &str, kind: &str) -> String {
    let ext = if codec == "subrip" { "srt" } else { "ass" };
    match kind {
        "forced" => format!("{stem}.fr.forced.{ext}"),
        "hi" => format!("{stem}.fr.hi.{ext}"),
        _ if ext == "ass" => format!("{stem}.fr.default.ass"),
        _ => format!("{stem}.fr.srt"),
    }
}

/// Extractions à faire pour un item Jellyfin : une par piste française incrustée texte (`ass`/`ssa`/`subrip`)
/// dont le fichier externe attendu n'est pas encore listé. Une piste ASS complète entraîne aussi le SRT dérivé.
pub fn plan_jobs(item: &Value) -> Vec<Job> {
    let Some(path) = item.get("Path").and_then(Value::as_str) else {
        return vec![];
    };
    let stem = Path::new(path)
        .with_extension("")
        .to_string_lossy()
        .to_string();
    let streams: Vec<&Value> = item
        .get("MediaStreams")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("Type").and_then(Value::as_str) == Some("Subtitle"))
                .collect()
        })
        .unwrap_or_default();
    let external: HashSet<String> = streams
        .iter()
        .filter(|s| flag(s, "IsExternal"))
        .filter_map(|s| s.get("Path").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut jobs: Vec<Job> = Vec::new();
    for s in streams
        .iter()
        .filter(|s| !flag(s, "IsExternal") && is_french(s))
    {
        let codec = match s.get("Codec").and_then(Value::as_str) {
            Some("ass") | Some("ssa") => "ass",
            Some("subrip") | Some("srt") => "subrip",
            _ => continue, // PGS/VobSub : images, rien à extraire en texte
        };
        let kind = kind_of(s);
        let out = out_name(&stem, codec, kind);
        // un ASS complet trop lourd est rangé sans « .default. » par le script (`max_default_ass_mb`)
        let heavy = format!("{stem}.fr.ass");
        let present = external.contains(&out)
            || (out.ends_with(".fr.default.ass") && external.contains(&heavy));
        if present || jobs.iter().any(|j| j.out == out) {
            continue;
        }
        let srt = (codec == "ass" && kind == "full").then(|| format!("{stem}.fr.srt"));
        jobs.push(Job {
            codec: codec.into(),
            kind: kind.into(),
            out,
            srt,
        });
    }
    jobs
}

/// Un item doit-il être (re)traité maintenant ? Jamais essayé : oui. Déjà traité : une fois par `retry` secondes
/// (Jellyfin peut mettre du temps à lister le fichier). Sans piste extractible : une fois par `failed_retry`.
pub fn due(last: Option<&SubtitleTry>, now: i64, retry: i64, failed_retry: i64) -> bool {
    match last {
        None => true,
        Some(t) => now - t.at >= if t.no_track { failed_retry } else { retry },
    }
}

/// Le script répond « code 3 » quand la vidéo n'a pas de piste extractible : pas la peine d'y revenir avant longtemps.
fn is_no_track(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains("exit status: 3")
}

/// Chemin vu par Jellyfin (`/seedbox/media/...`) → chemin sur la seedbox (`/home/x/media/...`) et dossier
/// relatif pour rclone.
pub fn seedbox_path(sb: &Seedbox, jf: &str) -> Option<(String, String)> {
    let root = sb.jellyfin_root.trim_end_matches('/');
    let rel = jf.strip_prefix(root)?.trim_start_matches('/');
    let dir = Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Some((
        format!("{}/{}", sb.media_root.trim_end_matches('/'), rel),
        dir,
    ))
}

/// Argument sûr pour le shell distant.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[async_trait]
impl Task for SubtitleSync {
    fn name(&self) -> &'static str {
        "subtitle_sync"
    }

    fn label(&self) -> &'static str {
        "Sous-titres extraits"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.subtitle_sync.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let sb = &ctx.cfg.seedbox;
        let cfg = &ctx.cfg.tasks.subtitle_sync;
        if !sb.enabled || cfg.extract_script.is_empty() {
            return Ok(Report::new("disabled", 0));
        }
        if !sb.mount_point.is_dir() {
            warn!(task = "subtitle_sync", mount = %sb.mount_point.display(), "seedbox mount unavailable, retry next run");
            return Ok(Report::new("mount unavailable", 0));
        }
        // `Items` SANS UserId renvoie une liste incomplète (21/09 : les 13 épisodes du matin absents) : compte admin.
        let admin = ctx
            .jellyfin
            .users()
            .await?
            .iter()
            .find(|u| crate::accounts::is_admin(u))
            .and_then(|u| u.get("Id").and_then(Value::as_str).map(str::to_string))
            .context("aucun compte admin Jellyfin")?;
        // début du passage : le suivant relira ce que Jellyfin aura sauvé depuis (moins la marge)
        let now = crate::state::now();
        let local_now = chrono::Local::now().naive_local();
        let tries = ctx.state.read(|s| s.subtitle_tries.clone()).await;
        let retry = (cfg.retry_hours * 3600) as i64;
        let failed_retry = (cfg.failed_retry_days * 86_400) as i64;
        let (since, recheck) = {
            let m = memo().lock().unwrap_or_else(|e| e.into_inner());
            if full_scan_due(m.last_full, local_now, cfg.full_scan_hour) {
                (None, Vec::new())
            } else {
                (
                    m.last_pass.map(saved_since),
                    ids_to_recheck(&m.backlog, &tries, now, retry, failed_retry),
                )
            }
        };
        // balayage complet : jour neuf, ou rien en mémoire (premier passage après un démarrage)
        let full = since.is_none();
        let mut query = vec![
            ("UserId", admin.as_str()),
            ("Recursive", "true"),
            ("IncludeItemTypes", "Episode,Movie"),
            ("Fields", "Path,MediaStreams,DateCreated"),
            ("EnableImages", "false"),
            ("SortBy", "DateCreated,SortName"),
            ("SortOrder", "Descending"),
        ];
        if let Some(since) = since.as_deref() {
            query.push(("MinDateLastSaved", since));
        }
        let mut items = ctx
            .jellyfin
            .items_paged(&query, 300)
            .await
            .context("jellyfin Items")?;
        if !recheck.is_empty() {
            let mut seen: HashSet<String> = items
                .iter()
                .filter_map(|i| i.get("Id").and_then(Value::as_str).map(str::to_string))
                .collect();
            for chunk in recheck.chunks(80) {
                let ids = chunk.join(",");
                let batch = ctx
                    .jellyfin
                    .items(&[
                        ("UserId", admin.as_str()),
                        ("Ids", ids.as_str()),
                        ("Fields", "Path,MediaStreams,DateCreated"),
                        ("EnableImages", "false"),
                    ])
                    .await
                    .context("jellyfin Items?Ids")?;
                for it in batch {
                    let id = it
                        .get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if seen.insert(id) {
                        items.push(it);
                    }
                }
            }
        }
        let root = format!("{}/", sb.jellyfin_root.trim_end_matches('/'));
        items.retain(|i| {
            i.get("Path")
                .and_then(Value::as_str)
                .map(|p| p.starts_with(&root))
                .unwrap_or(false)
        });
        // les plus récents d'abord (ce que les membres vont lancer)
        items.sort_by(|a, b| {
            let da = a.get("DateCreated").and_then(Value::as_str).unwrap_or("");
            let db = b.get("DateCreated").and_then(Value::as_str).unwrap_or("");
            db.cmp(da)
        });
        let playing: HashSet<String> = ctx
            .jellyfin
            .sessions()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                s.pointer("/NowPlayingItem/Id")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        let mut todo: Vec<(&Value, Vec<Job>)> = Vec::new();
        let mut pending_ids: HashSet<String> = HashSet::new();
        // dus ce passage : sans essai noté à la fin, ils passent au reliquat
        let mut due_ids: HashSet<String> = HashSet::new();
        let mut pending_total = 0usize;
        for it in &items {
            let jobs = plan_jobs(it);
            if jobs.is_empty() {
                continue;
            }
            pending_total += 1;
            let id = it.get("Id").and_then(Value::as_str).unwrap_or("");
            pending_ids.insert(id.to_string());
            if !due(tries.get(id), now, retry, failed_retry) {
                continue;
            }
            due_ids.insert(id.to_string());
            if todo.len() < cfg.max_per_run.max(1) {
                todo.push((it, jobs));
            }
        }
        // balayage complet : les essais d'éléments qui n'attendent plus rien sont oubliés, l'état ne grossit pas sans
        // fin (rien en simulation). Jamais sur un passage court : sa liste partielle effacerait des relances en attente.
        let (waiting, no_track) = if full {
            if !ctx.dry_run {
                let _ = ctx
                    .state
                    .update(|s| s.subtitle_tries.retain(|id, _| pending_ids.contains(id)))
                    .await;
            }
            waiting_counts(
                tries
                    .iter()
                    .filter(|(id, _)| pending_ids.contains(*id))
                    .map(|(_, t)| t),
                now,
                retry,
                failed_retry,
            )
        } else {
            // passage court : les éléments relus qui n'ont plus rien à extraire perdent leur essai (extraits avec
            // succès depuis, fichier annexe désormais listé par Jellyfin), puis on compte ce qui attend vraiment
            let read: HashSet<String> = items
                .iter()
                .filter_map(|i| i.get("Id").and_then(Value::as_str).map(str::to_string))
                .collect();
            let done = prune_seen(&tries, &read, &pending_ids);
            if !done.is_empty() && !ctx.dry_run {
                let _ = ctx
                    .state
                    .update(|s| s.subtitle_tries.retain(|id, _| !done.contains(id)))
                    .await;
            }
            waiting_counts(
                tries
                    .iter()
                    .filter(|(id, _)| !done.contains(*id))
                    .map(|(_, t)| t),
                now,
                retry,
                failed_retry,
            )
        };
        let full_note = if full { " ; balayage complet" } else { "" };
        let full_at = full.then_some(local_now);
        if todo.is_empty() {
            remember(now, full_at, HashSet::new());
            return Ok(Report::new(
                format!(
                    "rien à extraire ({waiting} en attente de relance, dont {no_track} sans piste{full_note})"
                ),
                0,
            ));
        }
        if ctx.dry_run {
            remember(now, full_at, due_ids);
            for (it, jobs) in &todo {
                let name = it.get("Name").and_then(Value::as_str).unwrap_or("?");
                let kinds: Vec<String> = jobs
                    .iter()
                    .map(|j| format!("{}/{}", j.codec, j.kind))
                    .collect();
                info!(task = "subtitle_sync", item = %name, ?kinds, "dry-run");
            }
            return Ok(Report::new(
                format!("dry_run items={} pending={pending_total}", todo.len()),
                todo.len() as u32,
            ));
        }
        let host = ctx.cfg.tasks.indexer_unblock.ssh_host.as_str();
        let (mut extracted, mut refreshed, mut skipped, mut failed) = (0u32, 0u32, 0u32, 0u32);
        let mut dirs: Vec<String> = Vec::new();
        let mut to_refresh: Vec<String> = Vec::new();
        let mut recorded: HashSet<String> = HashSet::new();
        // une extraction lit tout le fichier sur le disque de la seedbox (~30 s) : on s'arrête avant que le
        // planificateur ne coupe la tâche, le reste est repris au passage suivant (tout est idempotent)
        let started = std::time::Instant::now();
        let budget = Duration::from_secs(cfg.max_seconds.max(60));
        let mut out_of_time = false;
        for (it, jobs) in &todo {
            if started.elapsed() >= budget {
                out_of_time = true;
                break;
            }
            let id = it.get("Id").and_then(Value::as_str).unwrap_or("");
            let path = it.get("Path").and_then(Value::as_str).unwrap_or("");
            if playing.contains(id) {
                skipped += 1;
                continue;
            }
            let Some((video, dir)) = seedbox_path(sb, path) else {
                continue;
            };
            let mut ok_any = false;
            let mut all_no_track = true;
            for j in jobs {
                let Some((out, _)) = seedbox_path(sb, &j.out) else {
                    continue;
                };
                let mut cmd = format!(
                    "GC_ASS_DEFAULT_MAX={} {} {} {} {} {}",
                    cfg.max_default_ass_mb * 1024 * 1024,
                    cfg.extract_script,
                    sh_quote(&video),
                    j.codec,
                    j.kind,
                    sh_quote(&out)
                );
                if let Some(srt) = &j.srt {
                    if let Some((srt_sb, _)) = seedbox_path(sb, srt) {
                        cmd.push(' ');
                        cmd.push_str(&sh_quote(&srt_sb));
                    }
                }
                match docker::run(
                    "ssh",
                    &[
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=20",
                        // connexion morte détectée en une minute au lieu de bloquer le passage
                        "-o",
                        "ServerAliveInterval=15",
                        "-o",
                        "ServerAliveCountMax=4",
                        host,
                        &cmd,
                    ],
                    None,
                )
                .await
                {
                    Ok(_) => {
                        extracted += 1;
                        ok_any = true;
                    }
                    Err(e) => {
                        failed += 1;
                        if !is_no_track(&e) {
                            all_no_track = false;
                        }
                        warn!(task = "subtitle_sync", item = %path, kind = %j.kind, error = format!("{e:#}"), "extraction failed");
                    }
                }
            }
            // réussite ou « aucune piste » : on n'y revient qu'après le délai ; erreur passagère (ssh, délai) :
            // rien n'est noté, l'item est repris au passage suivant
            if ok_any || all_no_track {
                let entry = SubtitleTry {
                    at: crate::state::now(),
                    no_track: !ok_any,
                };
                let key = id.to_string();
                let _ = ctx
                    .state
                    .update(move |s| {
                        s.subtitle_tries.insert(key, entry);
                    })
                    .await;
                recorded.insert(id.to_string());
            }
            if ok_any {
                dirs.push(dir);
                to_refresh.push(id.to_string());
            }
        }
        dirs.sort();
        dirs.dedup();
        if !dirs.is_empty() {
            let rc = format!("{}/vfs/refresh", sb.rclone_rc.trim_end_matches('/'));
            match ctx.http.post(&rc).json(&refresh_params(&dirs)).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    warn!(task = "subtitle_sync", status = %resp.status(), "rclone vfs/refresh failed")
                }
                Err(e) => {
                    warn!(task = "subtitle_sync", error = %crate::clients::cause_chain(e), "rclone rc injoignable")
                }
                _ => {}
            }
        }
        for id in &to_refresh {
            match ctx.jellyfin.refresh_streams(id).await {
                Ok(()) => refreshed += 1,
                Err(e) => {
                    failed += 1;
                    warn!(task = "subtitle_sync", item = %id, error = format!("{e:#}"), "refresh failed");
                }
            }
        }
        // reliquat : repris au passage suivant même si Jellyfin ne touche pas à ces éléments
        let backlog = backlog_after(&due_ids, &recorded);
        let left = backlog.len();
        remember(now, full_at, backlog);
        info!(
            task = "subtitle_sync",
            extracted,
            refreshed,
            skipped,
            failed,
            pending = pending_total,
            left,
            out_of_time,
            full,
            "done"
        );
        Ok(Report::new(
            format!(
                "{extracted} extrait(s), {refreshed} rafraîchi(s), {skipped} en lecture, {failed} échec(s), {left} à reprendre, {waiting} en relance différée{}{full_note}",
                if out_of_time { " (budget atteint)" } else { "" }
            ),
            extracted,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jobs_follow_flags_and_codecs() {
        let item = json!({"Path": "/seedbox/media/Anime/A/Season 1/A - S01E01.mkv", "MediaStreams": [
            {"Type": "Video"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsForced": true, "Title": "Forced"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsDefault": true},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsHearingImpaired": true},
            {"Type": "Subtitle", "Language": "eng", "Codec": "ass"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "PGSSUB"}
        ]});
        let jobs = plan_jobs(&item);
        let outs: Vec<&str> = jobs.iter().map(|j| j.out.as_str()).collect();
        assert_eq!(
            outs,
            vec![
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.forced.ass",
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.default.ass",
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.hi.ass"
            ]
        );
        assert_eq!(jobs[0].srt, None);
        assert_eq!(
            jobs[1].srt.as_deref(),
            Some("/seedbox/media/Anime/A/Season 1/A - S01E01.fr.srt")
        );
        assert_eq!(jobs[1].kind, "full");
    }

    #[test]
    fn existing_external_files_are_skipped() {
        let item = json!({"Path": "/seedbox/media/Movies/M (2020)/M (2020).mkv", "MediaStreams": [
            {"Type": "Subtitle", "Language": "fra", "Codec": "subrip", "IsExternal": true, "Path": "/seedbox/media/Movies/M (2020)/M (2020).fr.srt"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "subrip"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "Title": "Malentendants"}
        ]});
        let jobs = plan_jobs(&item);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "hi");
        assert!(jobs[0].out.ends_with("M (2020).fr.hi.ass"));
        let none = json!({"Path": "/seedbox/media/x.mkv", "MediaStreams": [{"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsExternal": true, "Path": "/seedbox/media/x.fr.default.ass"}]});
        assert!(plan_jobs(&none).is_empty());
    }

    #[test]
    fn paths_and_quotes() {
        let sb = Seedbox {
            media_root: "/home/u/media".into(),
            jellyfin_root: "/seedbox/media".into(),
            ..Seedbox::default()
        };
        let (p, d) = seedbox_path(
            &sb,
            "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.default.ass",
        )
        .unwrap();
        assert_eq!(
            p,
            "/home/u/media/Anime/A/Season 1/A - S01E01.fr.default.ass"
        );
        assert_eq!(d, "Anime/A/Season 1");
        assert!(seedbox_path(&sb, "/media/tvshows/x.mkv").is_none());
        assert_eq!(sh_quote("l'été (2020)"), "'l'\"'\"'été (2020)'");
        assert_eq!(out_name("/x/a", "subrip", "full"), "/x/a.fr.srt");
        assert_eq!(out_name("/x/a", "ass", "forced"), "/x/a.fr.forced.ass");
    }

    #[test]
    fn retries_are_spaced_out() {
        let (retry, failed) = (6 * 3600, 7 * 86_400);
        assert!(due(None, 1_000_000, retry, failed));
        let done = SubtitleTry {
            at: 1_000_000,
            no_track: false,
        };
        assert!(!due(Some(&done), 1_000_000 + 300, retry, failed));
        assert!(due(Some(&done), 1_000_000 + retry, retry, failed));
        let none = SubtitleTry {
            at: 1_000_000,
            no_track: true,
        };
        assert!(!due(Some(&none), 1_000_000 + retry, retry, failed));
        assert!(due(Some(&none), 1_000_000 + failed, retry, failed));
    }

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn full_scan_once_a_day_after_the_hour_and_after_a_restart() {
        // rien en mémoire (démarrage) : complet, quelle que soit l'heure
        assert!(full_scan_due(None, at("2026-10-07 21:00"), 5));
        // fait hier à 05:10 : pas avant aujourd'hui 05:00
        assert!(!full_scan_due(
            Some(at("2026-10-06 05:10")),
            at("2026-10-07 04:59"),
            5
        ));
        assert!(full_scan_due(
            Some(at("2026-10-06 05:10")),
            at("2026-10-07 05:02"),
            5
        ));
        // fait aujourd'hui après l'heure : plus rien jusqu'à demain
        assert!(!full_scan_due(
            Some(at("2026-10-07 05:02")),
            at("2026-10-07 23:59"),
            5
        ));
        // démarrage la veille au soir (complet à 21:00) : le suivant reste dû à 05:00
        assert!(full_scan_due(
            Some(at("2026-10-06 21:00")),
            at("2026-10-07 05:00"),
            5
        ));
        assert!(!full_scan_due(
            Some(at("2026-10-06 21:00")),
            at("2026-10-07 01:00"),
            5
        ));
        // heure hors bornes : ramenée à 23 h
        assert!(!full_scan_due(
            Some(at("2026-10-07 00:30")),
            at("2026-10-07 22:00"),
            99
        ));
    }

    #[test]
    fn short_pass_window_and_rechecked_ids() {
        // 2026-10-07 21:05:00 UTC, marge d'une heure
        assert_eq!(saved_since(1_791_407_100), "2026-10-07T20:05:00Z");
        let backlog: HashSet<String> = ["b".to_string(), "a".to_string()].into();
        let (retry, failed) = (6 * 3600, 7 * 86_400);
        let now = 1_000_000;
        let tries: BTreeMap<String, SubtitleTry> = [
            // relance due : relu
            (
                "c".to_string(),
                SubtitleTry {
                    at: now - retry,
                    no_track: false,
                },
            ),
            // pas encore dû : laissé
            (
                "d".to_string(),
                SubtitleTry {
                    at: now - 60,
                    no_track: false,
                },
            ),
            // sans piste, essayé il y a 6 h : laissé (une fois par semaine)
            (
                "e".to_string(),
                SubtitleTry {
                    at: now - retry,
                    no_track: true,
                },
            ),
            // aussi au reliquat : une seule fois
            (
                "a".to_string(),
                SubtitleTry {
                    at: now - failed,
                    no_track: true,
                },
            ),
        ]
        .into();
        assert_eq!(
            ids_to_recheck(&backlog, &tries, now, retry, failed),
            vec!["a", "b", "c"]
        );
        assert_eq!(waiting_counts(tries.values(), now, retry, failed), (2, 1));
    }

    #[test]
    fn a_short_pass_forgets_the_tries_of_items_it_read_with_nothing_left_to_extract() {
        let (retry, failed) = (6 * 3600, 7 * 86_400);
        let now = 1_000_000;
        let fresh = |no_track| SubtitleTry {
            at: now - 60,
            no_track,
        };
        // 3 essais « normaux » avant le lot, puis 13 extractions réussies : 16 essais pas encore dus
        let mut tries: BTreeMap<String, SubtitleTry> = BTreeMap::new();
        for i in 0..3 {
            tries.insert(format!("old{i}"), fresh(i == 0));
        }
        for i in 0..13 {
            tries.insert(format!("new{i:02}"), fresh(false));
        }
        assert_eq!(waiting_counts(tries.values(), now, retry, failed), (16, 1));
        // le passage suivant relit les 13 (Jellyfin les a sauvés après le rafraîchissement) : plus rien à extraire,
        // sauf un dont le fichier annexe n'est pas encore listé ; les 3 anciens ne sont pas relus
        let read: HashSet<String> = (0..13).map(|i| format!("new{i:02}")).collect();
        let pending: HashSet<String> = ["new05".to_string()].into();
        let done = prune_seen(&tries, &read, &pending);
        assert_eq!(done.len(), 12);
        assert!(!done.contains("new05"), "toujours à extraire : essai gardé");
        assert!(
            !done.iter().any(|id| id.starts_with("old")),
            "non relus : gardés, une liste partielle n'en dit rien"
        );
        assert_eq!(
            waiting_counts(
                tries
                    .iter()
                    .filter(|(id, _)| !done.contains(*id))
                    .map(|(_, t)| t),
                now,
                retry,
                failed
            ),
            (4, 1),
            "3 anciens + 1 en attente de son fichier annexe"
        );
        // rien relu, rien oublié ; élément relu et encore à extraire : gardé
        assert!(prune_seen(&tries, &HashSet::new(), &HashSet::new()).is_empty());
        let all: HashSet<String> = tries.keys().cloned().collect();
        assert!(prune_seen(&tries, &all, &all).is_empty());
    }

    #[test]
    fn what_was_not_tried_goes_to_the_backlog() {
        let due: HashSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        // x extrait, y sans piste (essais notés) ; z en lecture, au-delà du plafond, ou échec passager
        let recorded: HashSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            backlog_after(&due, &recorded),
            ["z".to_string()].into_iter().collect()
        );
        assert!(backlog_after(&HashSet::new(), &recorded).is_empty());
    }

    #[test]
    fn a_heavy_ass_kept_without_default_counts_as_present() {
        let item = serde_json::json!({
            "Path": "/seedbox/media/Anime/B/B - S01E18.mkv",
            "MediaStreams": [
                {"Type": "Subtitle", "Codec": "ass", "Language": "fre", "IsExternal": false},
                {"Type": "Subtitle", "Codec": "ass", "Language": "fre", "IsExternal": true,
                 "Path": "/seedbox/media/Anime/B/B - S01E18.fr.ass"}
            ]
        });
        assert!(plan_jobs(&item).is_empty());
    }
}
