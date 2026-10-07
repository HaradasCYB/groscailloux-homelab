//! Détecte les **boucles HLS** : un client qui redemande sans fin le même segment d'un flux transcodé.
//!
//! C'est le symptôme que le membre voit comme « ça saccade / ça charge » : le 13/09/2026, une TV webOS a
//! redemandé deux segments ~950 fois en 6 min (tous servis en 200, avec des tailles différentes : le
//! segment était régénéré sous elle). NPM voit chaque requête : on lit la fin de son journal d'accès
//! (horodatages **UTC**) et on compte, sur une fenêtre glissante, les requêtes par (client, média, segment).
//! Au-delà du seuil : avertissement dans le journal et mail à l'admin, une fois par (client, média) et par
//! fenêtre. Rien n'est modifié.
//!
//! **Rafales de ffmpeg** (2026-10-04) : un lecteur qui relance son flux en boucle ne redemande pas forcément
//! le même segment — une télé Samsung a fait relancer son remux ~2 fois par minute pendant 2 h 30 (116
//! lancements en une heure, le 30/09), un Chromecast 42 en 22 min (04/10), sans jamais déclencher la règle
//! des segments. Chaque lancement laisse un `FFmpeg.<Type>-<AAAA-MM-JJ>_<HH-MM-SS>_<id>_<n>.log` (heure locale)
//! dans le dossier des journaux Jellyfin : on compte, par titre, les lancements des 60 dernières minutes (sans
//! les titres du canari), et au-delà de `max_jobs_per_item_hour` : mail à l'admin, **une fois par rafale** (2026-10-07).
//! Le titre signalé est mémorisé dans `state.burst_alerts` jusqu'à ce que son compteur horaire retombe sous le seuil :
//! une seconde rafale du même jour alerte de nouveau (avant : une fois par titre et par jour, en mémoire, perdu à
//! chaque redémarrage), et un redémarrage de homelabd ne répète pas une rafale déjà signalée. L'alerte donne le
//! titre Jellyfin et pas seulement son identifiant. L'ancien repère « non-keyframe breaks » n'existe plus en 12.x
//! et ne comptait de toute façon que le canari. Les 5xx sur `hls1/` restent relevés dans le résumé.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct HlsLoopWatch;

/// Une requête de segment HLS lue dans le journal NPM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHit {
    /// Secondes UTC.
    pub at: i64,
    pub client: String,
    pub item: String,
    pub segment: String,
    pub status: u16,
}

/// Une boucle constatée : le même segment, du même client, `count` fois dans la fenêtre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loop {
    pub client: String,
    pub item: String,
    pub segment: String,
    pub count: usize,
    pub first: i64,
    pub last: i64,
}

fn month(s: &str) -> Option<u32> {
    Some(match s {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// `[13/Sep/2026:21:29:32 +0000]` → secondes UTC (le décalage est toujours +0000 dans ce journal).
pub fn parse_time(field: &str) -> Option<i64> {
    let f = field.trim_start_matches('[');
    let (date, rest) = f.split_once(':')?;
    let mut d = date.split('/');
    let day: u32 = d.next()?.parse().ok()?;
    let mon = month(d.next()?)?;
    let year: i32 = d.next()?.parse().ok()?;
    let time = rest.split_whitespace().next()?;
    let mut t = time.split(':');
    let (h, m, s): (u32, u32, u32) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    let date = chrono::NaiveDate::from_ymd_opt(year, mon, day)?;
    let dt = date.and_hms_opt(h, m, s)?;
    Some(dt.and_utc().timestamp())
}

/// Une ligne du journal d'accès NPM (format « proxy-host ») :
/// `[date] - 200 200 - GET https host "/videos/<id>/hls1/main/58.ts?..." [Client 1.2.3.4] [Length n] …`
/// Seules les requêtes de segment HLS renvoient quelque chose.
pub fn parse_line(line: &str) -> Option<SegmentHit> {
    let at = parse_time(line.split(']').next()?)?;
    let status: u16 = line
        .split_whitespace()
        .nth(3)
        .and_then(|s| s.parse().ok())?;
    let url = line.split('"').nth(1)?;
    let path = url.split('?').next()?;
    let lower = path.to_ascii_lowercase();
    let rest = lower.strip_prefix("/videos/")?;
    let (item, tail) = rest.split_once('/')?;
    let seg = tail.strip_prefix("hls1/")?;
    let segment = seg.rsplit('/').next()?;
    if !(segment.ends_with(".ts") || segment.ends_with(".mp4") || segment.ends_with(".m4s")) {
        return None;
    }
    let client = line
        .split("[Client ")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .unwrap_or("?")
        .to_string();
    Some(SegmentHit {
        at,
        client,
        item: item.to_string(),
        segment: segment.to_string(),
        status,
    })
}

/// Boucles dans `hits` : mêmes (client, média, segment) au moins `threshold` fois dans `window_secs`.
/// Les requêtes sont supposées dans l'ordre du journal.
pub fn find_loops(hits: &[SegmentHit], window_secs: i64, threshold: usize) -> Vec<Loop> {
    let mut by: BTreeMap<(String, String, String), Vec<i64>> = BTreeMap::new();
    for h in hits {
        by.entry((h.client.clone(), h.item.clone(), h.segment.clone()))
            .or_default()
            .push(h.at);
    }
    let mut out = Vec::new();
    for ((client, item, segment), times) in by {
        // fenêtre glissante : le plus grand nombre de requêtes dans `window_secs`
        let mut best = 0usize;
        let mut best_range = (0, 0);
        let mut start = 0usize;
        for end in 0..times.len() {
            while times[end] - times[start] > window_secs {
                start += 1;
            }
            let n = end - start + 1;
            if n > best {
                best = n;
                best_range = (times[start], times[end]);
            }
        }
        if best >= threshold {
            out.push(Loop {
                client,
                item,
                segment,
                count: best,
                first: best_range.0,
                last: best_range.1,
            });
        }
    }
    out.sort_by_key(|l| std::cmp::Reverse(l.count));
    out
}

/// Lit au plus `max_bytes` à la fin du fichier (le journal fait des dizaines de Mo ; on ne relit jamais
/// tout). La première ligne, probablement tronquée, est ignorée.
pub fn tail(path: &Path, max_bytes: u64) -> Result<String> {
    let mut f =
        std::fs::File::open(path).with_context(|| format!("ouverture {}", path.display()))?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    if start > 0 {
        if let Some(i) = buf.find('\n') {
            buf.drain(..=i);
        }
    }
    Ok(buf)
}

/// Un lancement de ffmpeg par Jellyfin, lu dans le nom de son journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStart {
    /// Secondes « heure locale naïve » (celle du nom de fichier), comparables entre elles seulement.
    pub at: i64,
    /// `Transcode`, `Remux` ou `DirectStream`.
    pub kind: String,
    /// Identifiant Jellyfin en 32 caractères hexadécimaux minuscules.
    pub item: String,
}

/// `FFmpeg.Remux-2026-10-04_14-09-09_2d171b6c855ed6d055e724023cdc905d_2b7e5cf7.log` → lancement.
/// Les autres journaux (`log_*.log`, `FFmpeg.Subtitles-…`, extraction d'images…) ne renvoient rien.
pub fn parse_job_log_name(name: &str) -> Option<JobStart> {
    let rest = name.strip_prefix("FFmpeg.")?.strip_suffix(".log")?;
    let (kind, rest) = rest.split_once('-')?;
    if !matches!(kind, "Transcode" | "Remux" | "DirectStream") {
        return None;
    }
    let mut parts = rest.splitn(4, '_');
    let date = parts.next()?;
    let time = parts.next()?;
    let item = parts.next()?.to_ascii_lowercase();
    if item.len() != 32 || !item.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let dt = chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H-%M-%S")
        .ok()?;
    Some(JobStart {
        at: dt.and_utc().timestamp(),
        kind: kind.to_string(),
        item,
    })
}

/// Une rafale : `count` lancements de ffmpeg pour le même titre dans la fenêtre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Burst {
    pub item: String,
    pub count: usize,
    /// Nombre de lancements par type (`Remux: 40`…), pour le mail.
    pub kinds: BTreeMap<String, usize>,
}

/// Titres lancés au moins `threshold` fois depuis `since` (même échelle que `JobStart::at`), hors `exclude`
/// (titres du canari, en 32 caractères hexadécimaux). Les plus touchés d'abord.
pub fn find_bursts(
    jobs: &[JobStart],
    since: i64,
    threshold: usize,
    exclude: &HashSet<String>,
) -> Vec<Burst> {
    let mut by: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
    for j in jobs
        .iter()
        .filter(|j| j.at >= since && !exclude.contains(&j.item))
    {
        *by.entry(&j.item)
            .or_default()
            .entry(j.kind.clone())
            .or_default() += 1;
    }
    let mut out: Vec<Burst> = by
        .into_iter()
        .map(|(item, kinds)| Burst {
            item: item.to_string(),
            count: kinds.values().sum(),
            kinds,
        })
        .filter(|b| b.count >= threshold)
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.count));
    out
}

/// Identifiant Jellyfin ramené à la forme des noms de journaux (32 hexadécimaux minuscules, sans tirets).
pub fn compact_id(id: &str) -> String {
    id.chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Rafales à signaler et à réarmer. `current` : titres en rafale à ce passage ; `alerted` : titres déjà signalés
/// (`state.burst_alerts`). Un titre en rafale non encore signalé donne une alerte ; un titre signalé dont le
/// compteur horaire est retombé sous le seuil (absent de `current`) est réarmé : la rafale suivante alertera.
pub fn burst_transitions(
    current: &[String],
    alerted: &BTreeMap<String, i64>,
) -> (Vec<String>, Vec<String>) {
    let fresh = current
        .iter()
        .filter(|id| !alerted.contains_key(*id))
        .cloned()
        .collect();
    let cleared = alerted
        .keys()
        .filter(|id| !current.contains(id))
        .cloned()
        .collect();
    (fresh, cleared)
}

/// Libellé lisible d'un élément Jellyfin : « Série S01E03 — Titre », « Film (2021) » ou son nom seul.
pub fn item_title(item: &Value) -> Option<String> {
    let name = item
        .get("Name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty());
    match item.get("Type").and_then(Value::as_str) {
        Some("Episode") => {
            let series = item.get("SeriesName").and_then(Value::as_str)?;
            let num = |k: &str| item.get(k).and_then(Value::as_u64);
            let code = match (num("ParentIndexNumber"), num("IndexNumber")) {
                (Some(s), Some(e)) => format!(" S{s:02}E{e:02}"),
                _ => String::new(),
            };
            Some(match name {
                Some(n) => format!("{series}{code} — {n}"),
                None => format!("{series}{code}"),
            })
        }
        Some("Movie") => {
            let year = item.get("ProductionYear").and_then(Value::as_u64);
            Some(match (name?, year) {
                (n, Some(y)) => format!("{n} ({y})"),
                (n, None) => n.to_string(),
            })
        }
        _ => name.map(str::to_string),
    }
}

/// « « Titre » (id) » quand le titre est connu, sinon « média id » : l'identifiant reste dans le message, c'est
/// lui que portent les journaux de Jellyfin et de NPM.
pub fn describe_item(id: &str, titles: &HashMap<String, String>) -> String {
    match titles.get(&compact_id(id)) {
        Some(t) => format!("« {t} » ({id})"),
        None => format!("média {id}"),
    }
}

/// Titres Jellyfin des médias d'une alerte (clé : identifiant compact). Au mieux : Jellyfin injoignable, ou un
/// titre disparu, laisse l'identifiant seul dans le message.
async fn titles_of(ctx: &TaskContext, ids: &[String]) -> HashMap<String, String> {
    let wanted: Vec<String> = ids
        .iter()
        .map(|i| compact_id(i))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if wanted.is_empty() {
        return HashMap::new();
    }
    match ctx.jellyfin.items_by_ids(&wanted).await {
        Ok(items) => items
            .iter()
            .filter_map(|it| {
                let id = it.get("Id").and_then(Value::as_str)?;
                Some((compact_id(id), item_title(it)?))
            })
            .collect(),
        Err(e) => {
            warn!(
                task = "hls_loop_watch",
                error = format!("{e:#}"),
                "titres Jellyfin non lus pour l'alerte"
            );
            HashMap::new()
        }
    }
}

/// Boucles déjà signalées (client, média) : pas de second mail dans la même fenêtre.
fn reported() -> &'static Mutex<HashSet<(String, String)>> {
    static SET: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

#[async_trait]
impl Task for HlsLoopWatch {
    fn name(&self) -> &'static str {
        "hls_loop_watch"
    }

    fn label(&self) -> &'static str {
        "Lectures qui bouclent"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.hls_loop_watch.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.hls_loop_watch;
        let now = chrono::Utc::now().timestamp();
        let text = match tail(&cfg.npm_access_log, cfg.tail_bytes) {
            Ok(t) => t,
            Err(e) => {
                warn!(task = "hls_loop_watch", error = %e, "journal NPM illisible");
                return Ok(Report::new("journal NPM illisible", 0));
            }
        };
        let since = now - cfg.window_secs;
        let hits: Vec<SegmentHit> = text
            .lines()
            .filter_map(parse_line)
            .filter(|h| h.at >= since)
            .collect();
        let errors_5xx = hits.iter().filter(|h| h.status >= 500).count();
        let loops = find_loops(&hits, cfg.window_secs, cfg.threshold);

        // rafales de ffmpeg : lancements par titre dans l'heure écoulée, d'après les noms des journaux FFmpeg.*
        let log_dir = &ctx.cfg.cleanup.jellyfin_log_dir;
        let local_now = chrono::Local::now().naive_local().and_utc().timestamp();
        let jobs: Vec<JobStart> = std::fs::read_dir(log_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| parse_job_log_name(&e.file_name().to_string_lossy()))
                    .collect()
            })
            .unwrap_or_default();
        let canary: HashSet<String> = ctx
            .state
            .read(|s| s.canary.items.values().map(|v| compact_id(v)).collect())
            .await;
        let bursts = find_bursts(&jobs, local_now - 3600, cfg.max_jobs_per_item_hour, &canary);
        let day = chrono::Local::now().format("%Y%m%d").to_string();
        // une alerte par rafale : le titre signalé reste dans l'état jusqu'à ce que son compteur retombe sous le seuil
        let in_burst: Vec<String> = bursts.iter().map(|b| b.item.clone()).collect();
        let alerted = ctx.state.read(|s| s.burst_alerts.clone()).await;
        let (fresh_ids, cleared) = burst_transitions(&in_burst, &alerted);
        if !cleared.is_empty() && !ctx.dry_run {
            let _ = ctx
                .state
                .update(|s| {
                    for id in &cleared {
                        s.burst_alerts.remove(id);
                    }
                })
                .await;
        }
        let fresh_bursts: Vec<&Burst> = bursts
            .iter()
            .filter(|b| fresh_ids.contains(&b.item))
            .collect();
        for b in &fresh_bursts {
            warn!(
                task = "hls_loop_watch",
                item = %b.item,
                count = b.count,
                "rafale de ffmpeg : le lecteur relance son flux en boucle"
            );
        }
        if !fresh_bursts.is_empty() && !ctx.dry_run {
            let ids: Vec<String> = fresh_bursts.iter().map(|b| b.item.clone()).collect();
            let titles = titles_of(ctx, &ids).await;
            let body = fresh_bursts
                .iter()
                .map(|b| {
                    let kinds = b
                        .kinds
                        .iter()
                        .map(|(k, n)| format!("{k} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "- {} : {} lancements de ffmpeg en 60 min ({kinds})",
                        describe_item(&b.item, &titles),
                        b.count
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let subject = format!(
                "[Groscailloux] {} lecture(s) relancée(s) en boucle",
                fresh_bursts.len()
            );
            let body = format!(
                "Un lecteur fait relancer ffmpeg sans arrêt pour le même titre : le membre voit une lecture qui \
                 recharge, et le processeur travaille pour rien.\n\n{body}\n\nJournaux : \
                 jellyfin/config/log/FFmpeg.*_<id>_*.log et /opt/homelab/npm/data/logs/proxy-host-1_access.log (UTC)."
            );
            let sent = crate::alerts::admin(ctx, crate::alerts::Level::Warn, &subject, &body).await;
            // signalées, sauf si rien n'est parti alors qu'un canal existe : on réessaiera au passage suivant
            if !crate::alerts::retry_later(sent, crate::alerts::configured(ctx)) {
                let at = chrono::Utc::now().timestamp();
                let _ = ctx
                    .state
                    .update(|s| {
                        for id in &ids {
                            s.burst_alerts.insert(id.clone(), at);
                        }
                    })
                    .await;
            }
        }
        let max_jobs = find_bursts(&jobs, local_now - 3600, 1, &canary)
            .first()
            .map(|b| b.count)
            .unwrap_or(0);

        let mut fresh: Vec<&Loop> = Vec::new();
        {
            let mut seen = reported().lock().expect("mutex sain");
            // une boucle finie depuis plus d'une fenêtre peut être signalée à nouveau
            let live: HashSet<(String, String)> = loops
                .iter()
                .map(|l| (l.client.clone(), l.item.clone()))
                .collect();
            seen.retain(|k| live.contains(k));
            for l in &loops {
                if seen.insert((l.client.clone(), l.item.clone())) {
                    fresh.push(l);
                }
            }
        }
        for l in &fresh {
            warn!(
                task = "hls_loop_watch",
                client = %l.client,
                item = %l.item,
                segment = %l.segment,
                count = l.count,
                secs = l.last - l.first,
                "boucle HLS : le client redemande le même segment"
            );
        }
        if !fresh.is_empty() && !ctx.dry_run {
            let ids: Vec<String> = fresh.iter().map(|l| l.item.clone()).collect();
            let titles = titles_of(ctx, &ids).await;
            let body = fresh
                .iter()
                .map(|l| {
                    format!(
                        "- {} : segment {} redemandé {} fois en {} s par {}",
                        describe_item(&l.item, &titles),
                        l.segment,
                        l.count,
                        l.last - l.first,
                        l.client
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let subject = format!("[Groscailloux] {} lecture(s) en boucle HLS", fresh.len());
            let body = format!(
                "Un ou plusieurs lecteurs redemandent sans fin le même segment (flux transcodé) : le membre \
                 voit une lecture qui charge ou saccade.\n\n{body}\n\nRepères : {max_jobs} lancement(s) de \
                 ffmpeg au plus pour un même titre dans l'heure, {errors_5xx} erreur(s) 5xx sur des segments \
                 dans la fenêtre.\n\nJournal : /opt/homelab/npm/data/logs/proxy-host-1_access.log (UTC) et \
                 jellyfin/config/log/log_{day}.log."
            );
            crate::alerts::admin(ctx, crate::alerts::Level::Warn, &subject, &body).await;
        }
        info!(
            task = "hls_loop_watch",
            segments = hits.len(),
            loops = loops.len(),
            bursts = bursts.len(),
            max_jobs,
            errors_5xx,
            "passage"
        );
        Ok(Report::new(
            format!(
                "segments={} boucles={} rafales_ffmpeg={} max_ffmpeg_titre_heure={max_jobs} 5xx={errors_5xx}",
                hits.len(),
                loops.len(),
                bursts.len()
            ),
            (fresh.len() + fresh_bursts.len()) as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "[13/Sep/2026:21:29:32 +0000] - 200 200 - GET https jellyfin.example.org \"/videos/8030262a-ba75-3a33-991f-30310cfdf9a5/hls1/main/58.ts?DeviceId=abc&MediaSourceId=8030262a\" [Client 86.1.2.3] [Length 1292048] [Gzip -] [Sent-to jellyfin] \"Mozilla/5.0 (Web0S; Linux/SmartTV)\" \"-\"";

    #[test]
    fn a_segment_request_is_parsed() {
        let h = parse_line(LINE).unwrap();
        assert_eq!(h.client, "86.1.2.3");
        assert_eq!(h.item, "8030262a-ba75-3a33-991f-30310cfdf9a5");
        assert_eq!(h.segment, "58.ts");
        assert_eq!(h.status, 200);
        // 13/09/2026 21:29:32 UTC
        let expect = chrono::NaiveDate::from_ymd_opt(2026, 9, 13)
            .unwrap()
            .and_hms_opt(21, 29, 32)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(h.at, expect);
    }

    #[test]
    fn only_hls_segments_count() {
        let other = LINE.replace("/hls1/main/58.ts", "/stream.mkv");
        assert!(
            parse_line(&other).is_none(),
            "lecture directe : pas un segment"
        );
        let img = LINE.replace(
            "/videos/8030262a-ba75-3a33-991f-30310cfdf9a5/hls1/main/58.ts",
            "/Items/x/Images/Primary",
        );
        assert!(parse_line(&img).is_none());
        let init = LINE.replace("58.ts", "-1.mp4");
        assert_eq!(
            parse_line(&init).unwrap().segment,
            "-1.mp4",
            "segment d'initialisation fMP4"
        );
        assert!(parse_line("n'importe quoi").is_none());
    }

    fn hit(at: i64, seg: &str) -> SegmentHit {
        SegmentHit {
            at,
            client: "86.1.2.3".into(),
            item: "item".into(),
            segment: seg.into(),
            status: 200,
        }
    }

    #[test]
    fn a_loop_is_the_same_segment_many_times_in_the_window() {
        // 25 demandes du segment 58 en 100 s : boucle ; le 59 demandé 3 fois : normal
        let mut hits: Vec<SegmentHit> = (0..25).map(|i| hit(1000 + i * 4, "58.ts")).collect();
        hits.extend((0..3).map(|i| hit(1000 + i * 30, "59.ts")));
        let loops = find_loops(&hits, 300, 20);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].segment, "58.ts");
        assert_eq!(loops[0].count, 25);
        assert_eq!((loops[0].first, loops[0].last), (1000, 1096));
    }

    #[test]
    fn a_normal_playback_never_trips() {
        // un segment toutes les 3 s, chacun une seule fois
        let hits: Vec<SegmentHit> = (0..200).map(|i| hit(i * 3, &format!("{i}.ts"))).collect();
        assert!(find_loops(&hits, 300, 20).is_empty());
        // et 25 demandes étalées sur une heure ne sont pas une boucle
        let slow: Vec<SegmentHit> = (0..25).map(|i| hit(i * 150, "58.ts")).collect();
        assert!(find_loops(&slow, 300, 20).is_empty());
    }

    #[test]
    fn ffmpeg_log_names_are_parsed() {
        let j = parse_job_log_name(
            "FFmpeg.Remux-2026-10-04_14-09-09_2d171b6c855ed6d055e724023cdc905d_2b7e5cf7.log",
        )
        .unwrap();
        assert_eq!(j.kind, "Remux");
        assert_eq!(j.item, "2d171b6c855ed6d055e724023cdc905d");
        let expect = chrono::NaiveDate::from_ymd_opt(2026, 10, 4)
            .unwrap()
            .and_hms_opt(14, 9, 9)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(j.at, expect);
        assert!(parse_job_log_name(
            "FFmpeg.Transcode-2026-10-04_13-56-46_F047B6578C173C539AD4AA5B2972A2F3_2326c39e.log"
        )
        .is_some());
        assert!(parse_job_log_name("log_20261004.log").is_none());
        assert!(parse_job_log_name(
            "FFmpeg.Subtitles-2026-10-04_13-56-46_f047b6578c173c539ad4aa5b2972a2f3_1.log"
        )
        .is_none());
        assert!(
            parse_job_log_name("FFmpeg.Remux-2026-10-04_14-09-09_pasunid_2b7e5cf7.log").is_none()
        );
    }

    fn job(at: i64, item: &str, kind: &str) -> JobStart {
        JobStart {
            at,
            kind: kind.into(),
            item: item.into(),
        }
    }

    #[test]
    fn a_relaunch_storm_is_a_burst_but_normal_viewing_is_not() {
        let storm = "a".repeat(32);
        let calm = "b".repeat(32);
        let canary = "c".repeat(32);
        // Samsung du 30/09 : remux relancé toutes les ~31 s ; un film normal : 4 lancements (départ + 3 sauts)
        let mut jobs: Vec<JobStart> = (0..116)
            .map(|i| job(10_000 + i * 31, &storm, "Remux"))
            .collect();
        jobs.extend((0..4).map(|i| job(12_000 + i * 600, &calm, "Transcode")));
        // le canari, deux fois par heure : jamais compté
        jobs.extend((0..60).map(|i| job(10_000 + i * 60, &canary, "Transcode")));
        let exclude: HashSet<String> = [canary.clone()].into_iter().collect();
        let since = 10_000 + 116 * 31 - 3600;
        let b = find_bursts(&jobs, since, 30, &exclude);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].item, storm);
        assert!(b[0].count >= 30 && b[0].count <= 116);
        assert_eq!(b[0].kinds.get("Remux"), Some(&b[0].count));
        // sous le seuil, ou hors de la fenêtre : rien
        assert!(find_bursts(&jobs, since, 200, &exclude).is_empty());
        assert!(find_bursts(&jobs, 1_000_000, 1, &exclude).is_empty());
        // sans exclusion, le canari ressortirait
        assert!(find_bursts(&jobs, 0, 30, &HashSet::new())
            .iter()
            .any(|x| x.item == canary));
    }

    #[test]
    fn a_burst_alerts_once_and_again_only_after_the_count_fell_below_the_threshold() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut alerted: BTreeMap<String, i64> = BTreeMap::new();
        // 1re rafale (le 04/10 à 18:34) : alerte
        let (fresh, cleared) = burst_transitions(&ids(&["a"]), &alerted);
        assert_eq!((fresh, cleared), (ids(&["a"]), vec![]));
        alerted.insert("a".into(), 100);
        // elle dure : silence, même au passage suivant et après un redémarrage (l'état est sur disque)
        let (fresh, cleared) = burst_transitions(&ids(&["a"]), &alerted);
        assert!(fresh.is_empty() && cleared.is_empty());
        // le compteur horaire retombe sous le seuil (2 à 19:34) : réarmé
        let (fresh, cleared) = burst_transitions(&[], &alerted);
        assert_eq!((fresh, cleared), (vec![], ids(&["a"])));
        alerted.clear();
        // seconde rafale du même titre le même jour (32 à 00:12) : nouvelle alerte
        let (fresh, _) = burst_transitions(&ids(&["a"]), &alerted);
        assert_eq!(fresh, ids(&["a"]));
        // un autre titre en même temps ne dépend pas du premier
        alerted.insert("a".into(), 100);
        let (fresh, cleared) = burst_transitions(&ids(&["a", "b"]), &alerted);
        assert_eq!((fresh, cleared), (ids(&["b"]), vec![]));
    }

    #[test]
    fn item_titles_read_like_a_library() {
        let ep = serde_json::json!({
            "Type": "Episode", "Name": "La chute", "SeriesName": "Bleach",
            "ParentIndexNumber": 17, "IndexNumber": 3
        });
        assert_eq!(item_title(&ep).unwrap(), "Bleach S17E03 — La chute");
        let ep_no_num =
            serde_json::json!({"Type": "Episode", "Name": "Pilote", "SeriesName": "Série"});
        assert_eq!(item_title(&ep_no_num).unwrap(), "Série — Pilote");
        let movie =
            serde_json::json!({"Type": "Movie", "Name": "Your Name", "ProductionYear": 2016});
        assert_eq!(item_title(&movie).unwrap(), "Your Name (2016)");
        let movie_no_year = serde_json::json!({"Type": "Movie", "Name": "Sans année"});
        assert_eq!(item_title(&movie_no_year).unwrap(), "Sans année");
        // type inconnu : le nom seul ; rien d'exploitable : pas de titre
        assert_eq!(
            item_title(&serde_json::json!({"Type": "Video", "Name": "Clip"})).unwrap(),
            "Clip"
        );
        assert!(item_title(&serde_json::json!({"Type": "Movie"})).is_none());
        assert!(item_title(&serde_json::json!({"Type": "Episode", "Name": "x"})).is_none());
    }

    #[test]
    fn the_alert_names_the_title_and_keeps_the_id() {
        let mut titles = HashMap::new();
        titles.insert(
            compact_id("bc146385-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
            "Bleach S17E03 — La chute".to_string(),
        );
        // l'identifiant de l'alerte peut porter des tirets et des majuscules (chemin NPM) : même média
        assert_eq!(
            describe_item("BC146385-AAAA-AAAA-AAAA-AAAAAAAAAAAA", &titles),
            "« Bleach S17E03 — La chute » (BC146385-AAAA-AAAA-AAAA-AAAAAAAAAAAA)"
        );
        // titre inconnu (Jellyfin injoignable ou élément disparu) : l'identifiant seul, comme avant
        assert_eq!(describe_item("zzz", &titles), "média zzz");
    }

    #[test]
    fn ids_are_compared_without_dashes() {
        assert_eq!(
            compact_id("F047B657-8C17-3C53-9AD4-AA5B2972A2F3"),
            "f047b6578c173c539ad4aa5b2972a2f3"
        );
    }

    #[test]
    fn tail_keeps_only_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("access.log");
        std::fs::write(&p, "ligne 1 tronquée\nligne 2\nligne 3\n").unwrap();
        let t = tail(&p, 16).unwrap();
        assert_eq!(t, "ligne 3\n");
        assert_eq!(tail(&p, 10_000).unwrap().lines().count(), 3);
    }
}
