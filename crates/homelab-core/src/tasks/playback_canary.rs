//! Canari de lecture (`[tasks.playback_canary]`, 15 min) : un vrai transcodage HLS de deux segments,
//! en alternance sur un petit fichier du VPS et un de la seedbox, comme le ferait l'appli d'un membre
//! (PlaybackInfo forcé en h264/aac 2 Mbit/s, `master.m3u8` → `main.m3u8` → segments). Échec = segment
//! vide (tmpfs plein, le 2026-09-20 : « chargement infini »), premier segment trop lent (lien seedbox,
//! CPU), ou PlaybackInfo refusé. Alerte admin (mail + Discord) au premier échec, message de retour à la
//! normale ; état dans `state.canary`, affiché sur `/status.html`. Sauté si des membres transcodent déjà.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::now;

pub struct PlaybackCanary;

const DEVICE_ID: &str = "gc-canary";

fn profile() -> Value {
    json!({
        "MaxStreamingBitrate": 2_000_000,
        "DirectPlayProfiles": [],
        "TranscodingProfiles": [{
            "Container": "mp4", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
            "Protocol": "hls", "Context": "Streaming", "MaxAudioChannels": "2",
            "MinSegments": 1, "BreakOnNonKeyFrames": true
        }],
        "CodecProfiles": [], "SubtitleProfiles": []
    })
}

/// Lignes d'une playlist HLS qui ne sont pas des commentaires.
pub fn playlist_entries(m3u8: &str) -> Vec<String> {
    m3u8.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Verdict d'un passage : premier segment (durée, octets), second (octets).
pub fn verdict(first: (Duration, usize), second: usize, max_first: Duration) -> Result<()> {
    if first.1 < 10_000 {
        bail!("premier segment vide ou tronqué ({} octets)", first.1);
    }
    if second < 10_000 {
        bail!("second segment vide ou tronqué ({} octets)", second);
    }
    if first.0 > max_first {
        bail!(
            "premier segment en {:.1} s (limite {} s)",
            first.0.as_secs_f64(),
            max_first.as_secs()
        );
    }
    Ok(())
}

/// Choisit, une fois pour toutes, un petit épisode par côté (`/media` = VPS, `/seedbox` = seedbox).
async fn pick_items(ctx: &TaskContext) -> Result<Vec<(String, String)>> {
    let saved = ctx.state.read(|s| s.canary.items.clone()).await;
    if saved.len() == 2 {
        return Ok(saved.into_iter().collect());
    }
    let items = ctx
        .jellyfin
        .items(&[
            ("Recursive", "true"),
            ("IncludeItemTypes", "Episode,Movie"),
            ("Fields", "Path,RunTimeTicks"),
            ("SortBy", "Runtime"),
            ("SortOrder", "Ascending"),
            ("Limit", "400"),
        ])
        .await?;
    let mut out: Vec<(String, String)> = Vec::new();
    for side in ["vps", "seedbox"] {
        let prefix = if side == "vps" {
            "/media/"
        } else {
            "/seedbox/"
        };
        let found = items.iter().find(|i| {
            i.get("Path")
                .and_then(Value::as_str)
                .map(|p| p.starts_with(prefix))
                .unwrap_or(false)
                && i.get("RunTimeTicks")
                    .and_then(Value::as_i64)
                    .map(|t| t > 60 * 10_000_000)
                    .unwrap_or(false)
        });
        if let Some(i) = found {
            if let Some(id) = i.get("Id").and_then(Value::as_str) {
                out.push((side.to_string(), id.to_string()));
            }
        }
    }
    if out.is_empty() {
        bail!("aucun item avec un fichier pour le canari");
    }
    let map = out.iter().cloned().collect();
    ctx.state.update(|s| s.canary.items = map).await?;
    Ok(out)
}

async fn probe(
    ctx: &TaskContext,
    item_id: &str,
    user_id: &str,
    max_first: Duration,
) -> Result<String> {
    let pi = ctx
        .jellyfin
        .playback_info(
            item_id,
            user_id,
            &json!({
                "DeviceProfile": profile(),
                "EnableDirectPlay": false,
                "EnableDirectStream": false,
                "EnableTranscoding": true,
                "MaxStreamingBitrate": 2_000_000
            }),
        )
        .await?;
    let ms = pi
        .get("MediaSources")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .context("PlaybackInfo sans MediaSources")?;
    let url = ms
        .get("TranscodingUrl")
        .and_then(Value::as_str)
        .context("PlaybackInfo sans TranscodingUrl (transcodage refusé)")?
        .trim_start_matches('/')
        .to_string();
    let session = pi
        .get("PlaySessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base = url
        .rsplit_once('/')
        .map(|(b, _)| b.to_string())
        .unwrap_or_default();
    let result = async {
        let (_, master) = ctx.jellyfin.get_bytes(&url).await?;
        let main = playlist_entries(&String::from_utf8_lossy(&master))
            .into_iter()
            .next()
            .context("master.m3u8 vide")?;
        let (_, mainpl) = ctx.jellyfin.get_bytes(&format!("{base}/{main}")).await?;
        let segs = playlist_entries(&String::from_utf8_lossy(&mainpl));
        if segs.len() < 2 {
            bail!("main.m3u8 : {} segment(s)", segs.len());
        }
        let (t1, s1) = ctx
            .jellyfin
            .get_bytes(&format!("{base}/{}", segs[0]))
            .await?;
        let (_, s2) = ctx
            .jellyfin
            .get_bytes(&format!("{base}/{}", segs[1]))
            .await?;
        verdict((t1, s1.len()), s2.len(), max_first)?;
        Ok::<String, anyhow::Error>(format!("{:.1} s", t1.as_secs_f64()))
    }
    .await;
    if let Err(e) = ctx.jellyfin.stop_encodings(DEVICE_ID, &session).await {
        warn!(task = "playback_canary", error = %e, "transcodage non arrêté");
    }
    result
}

#[async_trait]
impl Task for PlaybackCanary {
    fn name(&self) -> &'static str {
        "playback_canary"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.playback_canary.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.playback_canary;
        let sessions = ctx.jellyfin.sessions().await.unwrap_or_default();
        let transcoding = sessions
            .iter()
            .filter(|s| s.get("NowPlayingItem").is_some() && s.get("TranscodingInfo").is_some())
            .count();
        if transcoding >= cfg.skip_if_transcodes_at_least {
            return Ok(Report::new(
                format!("sauté : {transcoding} transcodage(s) en cours"),
                0,
            ));
        }
        let users = ctx.jellyfin.users().await?;
        let admin = users
            .iter()
            .find(|u| crate::accounts::is_admin(u))
            .and_then(|u| u.get("Id").and_then(Value::as_str))
            .context("aucun compte admin pour le canari")?
            .to_string();
        let items = pick_items(ctx).await?;
        let t = now();
        let runs = ctx.state.read(|s| s.canary.runs).await as usize;
        ctx.state.update(|s| s.canary.runs += 1).await?;
        let (side, item) = &items[runs % items.len()];
        let max_first = Duration::from_secs(cfg.max_first_segment_secs);
        let outcome = probe(ctx, item, &admin, max_first).await;
        let prev = ctx.state.read(|s| s.canary.clone()).await;
        match outcome {
            Ok(detail) => {
                ctx.state
                    .update(|s| {
                        s.canary.last_run = t;
                        s.canary.last_ok = Some(true);
                        s.canary.last_ok_at = t;
                        s.canary.failures = 0;
                        s.canary.last_detail = format!("{side} : premier segment en {detail}");
                    })
                    .await?;
                if prev.failures > 0 {
                    alerts::admin(
                        ctx,
                        Level::Info,
                        "Lecture : retour à la normale",
                        &format!("Le canari de lecture ({side}) repasse : premier segment en {detail} après {} échec(s).", prev.failures),
                    )
                    .await;
                }
                info!(task = "playback_canary", side, detail, "ok");
                Ok(Report::new(format!("{side} ok ({detail})"), 0))
            }
            Err(e) => {
                let msg = format!("{side} : {e:#}");
                ctx.state
                    .update(|s| {
                        s.canary.last_run = t;
                        s.canary.last_ok = Some(false);
                        s.canary.failures += 1;
                        s.canary.last_detail = msg.clone();
                    })
                    .await?;
                if prev.failures == 0 {
                    alerts::admin(
                        ctx,
                        Level::Error,
                        "Lecture : le canari échoue",
                        &format!("Un transcodage de test vient d'échouer ({msg}). Les membres qui ne lisent pas en direct sont probablement bloqués. Vérifier : `scripts/jellyfin-transcodes-purge.sh --check`, `docker compose logs jellyfin`, le montage seedbox."),
                    )
                    .await;
                }
                warn!(task = "playback_canary", %msg, "échec");
                Ok(Report::new(format!("ÉCHEC {msg}"), 1))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_entries_skip_comments() {
        let m = "#EXTM3U\n#EXT-X-VERSION:3\nmain.m3u8?x=1\n\n#EXTINF:6\nhls1/main/0.mp4?y=2\n";
        assert_eq!(
            playlist_entries(m),
            vec!["main.m3u8?x=1", "hls1/main/0.mp4?y=2"]
        );
    }

    #[test]
    fn verdict_rejects_empty_or_slow() {
        let max = Duration::from_secs(20);
        assert!(verdict((Duration::from_secs(1), 120_000), 100_000, max).is_ok());
        assert!(verdict((Duration::from_secs(1), 0), 100_000, max).is_err());
        assert!(verdict((Duration::from_secs(1), 120_000), 0, max).is_err());
        assert!(verdict((Duration::from_secs(25), 120_000), 100_000, max).is_err());
    }
}
