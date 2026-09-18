//! Détecte les **boucles HLS** : un client qui redemande sans fin le même segment d'un flux transcodé.
//!
//! C'est le symptôme que le membre voit comme « ça saccade / ça charge » : le 13/09/2026, une TV webOS a
//! redemandé deux segments ~950 fois en 6 min (tous servis en 200, avec des tailles différentes : le
//! segment était régénéré sous elle). Jellyfin ne journalise que « non-keyframe breaks » ; NPM, lui, voit
//! chaque requête. On lit donc la fin du journal d'accès de NPM (horodatages **UTC**) et on compte, sur une
//! fenêtre glissante, les requêtes par (client, média, segment). Au-delà du seuil : avertissement dans le
//! journal et mail à l'admin, une fois par (client, média) et par fenêtre. Rien n'est modifié.
//!
//! Le compteur « non-keyframe breaks » du jour et les 500 sur `hls1/` sont relevés au passage (résumé du
//! rapport), pour suivre l'effet des réglages de lecture sans rouvrir les journaux à la main.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::mail;

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

/// Compte les lignes d'un fichier contenant `needle` (journal Jellyfin du jour).
pub fn count_lines_with(path: &Path, needle: &str) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter(|l| l.contains(needle)).count())
        .unwrap_or(0)
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

        // repères du jour dans le journal Jellyfin (relance HLS après blocage)
        let day = chrono::Local::now().format("%Y%m%d").to_string();
        let jf_log = ctx
            .cfg
            .cleanup
            .jellyfin_log_dir
            .join(format!("log_{day}.log"));
        let breaks = count_lines_with(&jf_log, "non-keyframe breaks");

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
            let body = fresh
                .iter()
                .map(|l| {
                    format!(
                        "- média {} : segment {} redemandé {} fois en {} s par {}",
                        l.item,
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
                 voit une lecture qui charge ou saccade.\n\n{body}\n\nRepères du jour : \
                 {breaks} relance(s) HLS (« non-keyframe breaks »), {errors_5xx} erreur(s) 5xx sur des \
                 segments dans la fenêtre.\n\nJournal : /opt/homelab/npm/data/logs/proxy-host-1_access.log \
                 (UTC) et jellyfin/config/log/log_{day}.log."
            );
            if let (Some(smtp), Some(to)) = (&ctx.secrets.smtp, &ctx.secrets.chat_admin_email) {
                if let Err(e) =
                    mail::send_plain(smtp, "Admin Groscailloux", to, &subject, &body).await
                {
                    warn!(task = "hls_loop_watch", error = %e, "mail admin en échec");
                }
            }
        }
        info!(
            task = "hls_loop_watch",
            segments = hits.len(),
            loops = loops.len(),
            breaks,
            errors_5xx,
            "passage"
        );
        Ok(Report::new(
            format!(
                "segments={} boucles={} relances_jour={breaks} 5xx={errors_5xx}",
                hits.len(),
                loops.len()
            ),
            fresh.len() as u32,
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
    fn tail_keeps_only_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("access.log");
        std::fs::write(&p, "ligne 1 tronquée\nligne 2\nligne 3\n").unwrap();
        let t = tail(&p, 16).unwrap();
        assert_eq!(t, "ligne 3\n");
        assert_eq!(tail(&p, 10_000).unwrap().lines().count(), 3);
    }
}
