//! Lectures simultanées par compte (`accounts.max_playbacks_per_user`, 0 = illimité).
//!
//! Remplace la limite d'appareils de Jellyfin (`MaxActiveSessions`), qui ne jouait qu'à la connexion
//! et bloquait des comptes à cause des sessions fantômes de l'appli iOS (le 2026-09-15). Toutes les
//! `interval_secs`, pour chaque compte non protégé : au-delà du maximum, les lectures les plus
//! récentes (première apparition la plus tardive ; à égalité, la moins avancée) sont arrêtées, avec
//! un message à l'écran, si elles durent depuis au moins `grace_secs` (laisse le temps de passer
//! d'un appareil à l'autre). Au plus `max_actions_per_run` arrêts par passage.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::now;

pub struct PlaybackLimit;

#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    pub session_id: String,
    pub user_id: String,
    pub user_name: String,
    pub device: String,
    pub item: String,
    pub item_id: String,
    pub position_ticks: i64,
    /// Première fois que cette lecture (session + titre) a été vue par la tâche.
    pub first_seen: i64,
}

/// Lectures à arrêter, par compte, au-delà de `max` (0 = illimité), hors comptes `exempt`.
pub fn to_stop(
    pbs: &[Playback],
    max: usize,
    exempt: &[String],
    now: i64,
    grace: i64,
) -> Vec<Playback> {
    if max == 0 {
        return Vec::new();
    }
    let mut by_user: BTreeMap<&str, Vec<&Playback>> = BTreeMap::new();
    for p in pbs {
        if !exempt.iter().any(|e| e.eq_ignore_ascii_case(&p.user_name)) {
            by_user.entry(p.user_id.as_str()).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    for (_, mut list) in by_user {
        if list.len() <= max {
            continue;
        }
        // les plus anciennes d'abord (à égalité : la plus avancée), on garde les `max` premières
        list.sort_by(|a, b| {
            a.first_seen
                .cmp(&b.first_seen)
                .then(b.position_ticks.cmp(&a.position_ticks))
                .then(a.session_id.cmp(&b.session_id))
        });
        out.extend(
            list.into_iter()
                .skip(max)
                .filter(|p| now - p.first_seen >= grace)
                .cloned(),
        );
    }
    out
}

/// Message affiché avant l'arrêt.
pub fn stop_message(max: usize) -> String {
    format!(
        "Ce compte regarde déjà sur {max} écrans : cette lecture est arrêtée. Arrête une autre lecture pour regarder ici."
    )
}

/// Tentatives d'arrêt par lecture : un appareil qui ignore l'ordre n'est pas relancé indéfiniment.
const MAX_ATTEMPTS: u32 = 3;

fn attempts() -> &'static Mutex<HashMap<String, u32>> {
    static A: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

fn seen() -> &'static Mutex<HashMap<String, i64>> {
    static SEEN: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn playbacks(sessions: &[Value], now: i64) -> Vec<Playback> {
    let mut seen = seen().lock().unwrap_or_else(|e| e.into_inner());
    let mut current = Vec::new();
    let mut out = Vec::new();
    for s in sessions {
        let (Some(item), Some(sid), Some(uid)) = (
            s.get("NowPlayingItem"),
            s.get("Id").and_then(Value::as_str),
            s.get("UserId").and_then(Value::as_str),
        ) else {
            continue;
        };
        let item_id = item.get("Id").and_then(Value::as_str).unwrap_or("");
        let key = format!("{sid}|{item_id}");
        let first = *seen.entry(key.clone()).or_insert(now);
        current.push(key);
        out.push(Playback {
            session_id: sid.to_string(),
            user_id: uid.to_string(),
            user_name: s
                .get("UserName")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            device: s
                .get("DeviceName")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            item: item
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            item_id: item_id.to_string(),
            position_ticks: s
                .pointer("/PlayState/PositionTicks")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            first_seen: first,
        });
    }
    seen.retain(|k, _| current.contains(k));
    attempts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|k, _| current.contains(k));
    out
}

#[async_trait]
impl Task for PlaybackLimit {
    fn name(&self) -> &'static str {
        "playback_limit"
    }

    fn label(&self) -> &'static str {
        "Lectures simultanées"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.playback_limit.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let max = ctx.cfg.accounts.max_playbacks_per_user;
        let cfg = &ctx.cfg.tasks.playback_limit;
        let t = now();
        let pbs = playbacks(&ctx.jellyfin.sessions().await?, t);
        let stop = to_stop(&pbs, max, &ctx.cfg.accounts.protected, t, cfg.grace_secs);
        let mut done = 0u32;
        for p in stop.iter().take(cfg.max_actions_per_run) {
            let key = format!("{}|{}", p.session_id, p.item_id);
            {
                let mut a = attempts().lock().unwrap_or_else(|e| e.into_inner());
                let n = a.entry(key).or_insert(0);
                if *n >= MAX_ATTEMPTS {
                    continue;
                }
                if !ctx.dry_run {
                    *n += 1;
                }
            }
            if ctx.dry_run {
                info!(task = "playback_limit", user = %p.user_name, device = %p.device, item = %p.item, "dry-run: would stop playback");
                continue;
            }
            if let Err(e) = ctx
                .jellyfin
                .send_message(&p.session_id, "Groscailloux", &stop_message(max), 12_000)
                .await
            {
                warn!(task = "playback_limit", user = %p.user_name, error = %e, "message not shown");
            }
            match ctx.jellyfin.stop_playback(&p.session_id).await {
                Ok(()) => {
                    done += 1;
                    info!(task = "playback_limit", user = %p.user_name, device = %p.device, item = %p.item, max, "playback stopped: over the limit");
                }
                Err(e) => {
                    warn!(task = "playback_limit", user = %p.user_name, error = %e, "stop failed")
                }
            }
        }
        Ok(Report::new(
            format!(
                "{} lecture(s), {} au-delà de la limite",
                pbs.len(),
                stop.len()
            ),
            done,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(sid: &str, user: &str, first: i64, pos: i64) -> Playback {
        Playback {
            session_id: sid.into(),
            user_id: user.into(),
            user_name: user.into(),
            device: sid.into(),
            item: "film".into(),
            item_id: "f1".into(),
            position_ticks: pos,
            first_seen: first,
        }
    }

    #[test]
    fn newest_playback_over_the_limit_is_stopped() {
        let pbs = [
            p("tv", "val", 100, 9),
            p("tel", "val", 200, 5),
            p("auto", "val", 300, 1),
            p("x", "nina", 300, 1),
        ];
        let got = to_stop(&pbs, 2, &[], 400, 30);
        assert_eq!(
            got.iter()
                .map(|x| x.session_id.as_str())
                .collect::<Vec<_>>(),
            ["auto"]
        );
    }

    #[test]
    fn grace_lets_people_switch_devices() {
        let pbs = [
            p("tv", "val", 100, 9),
            p("tel", "val", 200, 5),
            p("auto", "val", 390, 1),
        ];
        assert!(
            to_stop(&pbs, 2, &[], 400, 30).is_empty(),
            "3e lecture trop récente"
        );
        assert_eq!(to_stop(&pbs, 2, &[], 420, 30).len(), 1);
    }

    #[test]
    fn ties_after_restart_keep_the_most_advanced() {
        let pbs = [
            p("a", "val", 0, 100),
            p("b", "val", 0, 900),
            p("c", "val", 0, 500),
        ];
        let got = to_stop(&pbs, 2, &[], 60, 30);
        assert_eq!(
            got.iter()
                .map(|x| x.session_id.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn exempt_accounts_and_unlimited() {
        let pbs = [
            p("a", "Haradas", 0, 1),
            p("b", "Haradas", 0, 2),
            p("c", "Haradas", 0, 3),
        ];
        assert!(to_stop(&pbs, 2, &["haradas".into()], 60, 0).is_empty());
        assert!(to_stop(&pbs, 0, &[], 60, 0).is_empty());
        assert_eq!(to_stop(&pbs, 1, &[], 60, 0).len(), 2);
    }

    #[test]
    fn message_names_the_limit() {
        assert!(stop_message(2).contains("2 écrans"));
    }
}
