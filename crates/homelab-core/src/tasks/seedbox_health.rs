//! Santé de la **seedbox** vue du VPS (2026-10-02).
//!
//! Le 01/10 vers 20:50, l'hôte partagé de la seedbox a redémarré : qBittorrent est reparti, mais Sonarr, Radarr,
//! Bazarr, Jackett, FlareSolverr, autobrr et unpackerr sont restés arrêtés **16 h** — ~1 900 avertissements dans le
//! journal de homelabd, aucune alerte. `stack_health` ne surveille que les conteneurs du VPS.
//!
//! La relance est faite **sur la seedbox** (crontab : `scripts/seedbox/homelab-apps-watch.sh`, au démarrage et toutes
//! les 5 min). Cette tâche-ci **prévient** : un service injoignable depuis `alert_after_mins` → mail + Discord admin,
//! une seule fois par panne ; un second message quand il répond de nouveau.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::now;

pub struct SeedboxHealth;

/// Ce que l'on fait pour un service à ce passage.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// En panne depuis assez longtemps et pas encore signalé.
    Alert,
    /// Signalé en panne, il répond de nouveau.
    Recovered,
}

/// `down_since` : premier échec constaté (None = répondait au passage précédent) ; `alerted` : panne déjà signalée.
pub fn decide(
    up: bool,
    down_since: Option<i64>,
    alerted: bool,
    now: i64,
    alert_after_mins: i64,
) -> Action {
    match (up, down_since) {
        (true, _) if alerted => Action::Recovered,
        (true, _) => Action::Nothing,
        (false, Some(since)) if !alerted && now - since >= alert_after_mins * 60 => Action::Alert,
        _ => Action::Nothing,
    }
}

/// Durée lisible : « 16 h 25 », « 12 min ».
pub fn human(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m >= 60 {
        format!("{} h {:02}", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

async fn probes(ctx: &TaskContext) -> Vec<(&'static str, bool, String)> {
    let mut out = Vec::new();
    let res = |r: Result<()>| match r {
        Ok(()) => (true, String::new()),
        Err(e) => (false, format!("{e:#}").chars().take(160).collect()),
    };
    if let Some(a) = &ctx.seedbox_sonarr {
        let (ok, why) = res(a.ping().await);
        out.push(("Sonarr", ok, why));
    }
    if let Some(a) = &ctx.seedbox_radarr {
        let (ok, why) = res(a.ping().await);
        out.push(("Radarr", ok, why));
    }
    if let Some(q) = &ctx.seedbox_qbit {
        let (ok, why) = res(q.version().await.map(|_| ()));
        out.push(("qBittorrent", ok, why));
    }
    if let Some(b) = &ctx.bazarr {
        let (ok, why) = res(b.history_episodes(1).await.map(|_| ()));
        out.push(("Bazarr", ok, why));
    }
    let mount = &ctx.cfg.tasks.seedbox_health.mount_check;
    if !mount.as_os_str().is_empty() {
        let ok = std::fs::read_dir(mount)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        out.push((
            "montage rclone",
            ok,
            if ok {
                String::new()
            } else {
                format!("{} vide ou illisible", mount.display())
            },
        ));
    }
    out
}

#[async_trait]
impl Task for SeedboxHealth {
    fn name(&self) -> &'static str {
        "seedbox_health"
    }

    fn label(&self) -> &'static str {
        "Santé de la seedbox"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.seedbox_health.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.seedbox_health;
        let t = now();
        let results = probes(ctx).await;
        let (down, alerted): (BTreeMap<String, i64>, BTreeMap<String, i64>) = ctx
            .state
            .read(|s| (s.seedbox_down.clone(), s.seedbox_alerted.clone()))
            .await;
        let mut new_down = down.clone();
        let mut new_alerted = alerted.clone();
        let mut alerts_to_send: Vec<(String, String)> = Vec::new();
        let mut recovered: Vec<(String, i64)> = Vec::new();
        let mut failing = Vec::new();
        for (name, up, why) in &results {
            let key = name.to_string();
            let since = down.get(&key).copied();
            match decide(
                *up,
                since,
                alerted.contains_key(&key),
                t,
                cfg.alert_after_mins,
            ) {
                Action::Alert => {
                    alerts_to_send.push((key.clone(), why.clone()));
                    new_alerted.insert(key.clone(), t);
                }
                Action::Recovered => {
                    recovered.push((key.clone(), t - since.unwrap_or(t)));
                    new_alerted.remove(&key);
                }
                Action::Nothing => {}
            }
            if *up {
                new_down.remove(&key);
            } else {
                new_down.entry(key.clone()).or_insert(t);
                failing.push(key.clone());
                warn!(task = "seedbox_health", service = %name, error = %why, "service seedbox injoignable");
            }
        }
        if !ctx.dry_run {
            ctx.state
                .update(|s| {
                    s.seedbox_down = new_down.clone();
                    s.seedbox_alerted = new_alerted.clone();
                })
                .await?;
        }
        if !alerts_to_send.is_empty() {
            let names: Vec<&str> = alerts_to_send.iter().map(|(n, _)| n.as_str()).collect();
            let subject = format!("Seedbox : {} injoignable(s)", names.join(", "));
            let mut body =
                String::from("Service(s) de la seedbox injoignable(s) depuis le VPS :\n");
            for (n, why) in &alerts_to_send {
                let since = new_down.get(n).copied().unwrap_or(t);
                body.push_str(&format!("- {n} depuis {} : {why}\n", human(t - since)));
            }
            body.push_str(
                "\nLa seedbox relance seule ses applis toutes les 5 min (crontab : homelab-apps-watch.sh). \
                 Si ça persiste : ssh seedbox 'app-sonarr start; app-radarr start; app-bazarr start' \
                 et journal ~/.local/state/homelab-apps-watch/watch.log.",
            );
            alerts::admin(ctx, Level::Warn, &subject, &body).await;
            info!(task = "seedbox_health", services = ?names, "alerte envoyée");
        }
        if !recovered.is_empty() {
            let txt: Vec<String> = recovered
                .iter()
                .map(|(n, d)| format!("{n} (panne d'environ {})", human(*d)))
                .collect();
            alerts::admin(
                ctx,
                Level::Info,
                "Seedbox : de nouveau joignable",
                &format!("Rétabli : {}.", txt.join(", ")),
            )
            .await;
        }
        let summary = if failing.is_empty() {
            format!("{} service(s) joignables", results.len())
        } else {
            format!("injoignable(s) : {}", failing.join(", "))
        };
        Ok(Report::new(
            summary,
            (alerts_to_send.len() + recovered.len()) as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_once_after_delay_then_recovery() {
        // répond : rien
        assert_eq!(decide(true, None, false, 1000, 10), Action::Nothing);
        // tombe à t=0 : pas d'alerte avant 10 min
        assert_eq!(decide(false, Some(0), false, 9 * 60, 10), Action::Nothing);
        assert_eq!(decide(false, Some(0), false, 10 * 60, 10), Action::Alert);
        // déjà signalé : pas de seconde alerte
        assert_eq!(decide(false, Some(0), true, 16 * 3600, 10), Action::Nothing);
        // revient : message de retour, une fois
        assert_eq!(
            decide(true, Some(0), true, 16 * 3600, 10),
            Action::Recovered
        );
        // revenu sans avoir été signalé (panne courte) : silence
        assert_eq!(decide(true, Some(0), false, 300, 10), Action::Nothing);
        // premier échec : on note seulement
        assert_eq!(decide(false, None, false, 0, 10), Action::Nothing);
    }

    #[test]
    fn durations_are_readable() {
        assert_eq!(human(12 * 60), "12 min");
        assert_eq!(human(16 * 3600 + 25 * 60), "16 h 25");
    }
}
