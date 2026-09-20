//! Auto-réparation du stack compose : première passe juste après le boot
//! (homelabd démarre après `homelab-stack.service`), puis périodique.
//!   - service attendu absent / exited / created  → `docker compose up -d <svc>`
//!   - `unhealthy` depuis ≥ unhealthy_grace_secs   → `docker compose restart <svc>`
//!   - sonde applicative en échec                  → restart, si ses dépendances sont healthy
//!
//! Un cooldown par service évite les boucles de redémarrage. Guacamole en est le
//! cas d'école : « Up (healthy) » mais login impossible si MySQL n'était pas prêt.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Method;
use serde::Deserialize;
use tracing::{error, info, warn};

use super::{Report, Task};
use crate::config::{Config, Probe, StackHealth as Cfg};
use crate::context::TaskContext;
use crate::docker;
use crate::state::now;

pub struct StackHealth;

#[derive(Debug, Clone, Deserialize)]
pub struct Container {
    #[serde(rename = "Service")]
    pub service: String,
    #[serde(rename = "State", default)]
    pub state: String,
    #[serde(rename = "Health", default)]
    pub health: String,
}

/// `docker compose ps --format json` : tableau (anciens compose) ou un objet par ligne.
pub fn parse_ps(raw: &str) -> Result<Vec<Container>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).context("compose ps : tableau JSON invalide");
    }
    trimmed
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).with_context(|| format!("compose ps : ligne invalide {l}"))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Rien à faire (running et healthy/sans healthcheck, ou en start_period).
    Nothing,
    /// Conteneur absent ou arrêté : `up -d`.
    Start,
    /// Unhealthy depuis assez longtemps et cooldown écoulé : `restart`.
    Restart,
    /// Unhealthy mais on attend (grâce ou cooldown).
    Wait,
}

pub fn decide(
    c: Option<&Container>,
    unhealthy_since: Option<i64>,
    last_restart: Option<i64>,
    ts: i64,
    cfg: &Cfg,
) -> Action {
    let Some(c) = c else { return Action::Start };
    if c.state != "running" {
        return Action::Start;
    }
    if c.health != "unhealthy" {
        return Action::Nothing;
    }
    let since = unhealthy_since.unwrap_or(ts);
    if ts - since < cfg.unhealthy_grace_secs || !cooldown_ok(last_restart, ts, cfg) {
        return Action::Wait;
    }
    Action::Restart
}

pub fn cooldown_ok(last_restart: Option<i64>, ts: i64, cfg: &Cfg) -> bool {
    last_restart
        .map(|t| ts - t >= cfg.restart_cooldown_secs)
        .unwrap_or(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Fail,
    /// Le service répond mais refuse la sonde (429 anti-bruteforce) : on ne conclut rien.
    Inconclusive,
}

pub fn probe_verdict(p: &Probe, status: u16, body: &str) -> Verdict {
    if status == 429 {
        return Verdict::Inconclusive;
    }
    if status == p.expect_status && body.contains(&p.expect_body_contains) {
        Verdict::Ok
    } else {
        Verdict::Fail
    }
}

async fn run_probe(ctx: &TaskContext, p: &Probe) -> Result<(u16, String)> {
    let method = Method::from_bytes(p.method.as_bytes()).context("méthode HTTP invalide")?;
    let mut req = ctx
        .http
        .request(method, &p.url)
        .timeout(Duration::from_secs(10));
    if !p.body.is_empty() {
        req = req
            .header("Content-Type", &p.content_type)
            .body(p.body.clone());
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    Ok((status, body))
}

/// `verb` = "start" (→ `up -d`, respecte depends_on) ou "restart".
async fn compose_action(ctx: &TaskContext, verb: &str, service: &str) -> bool {
    let args: &[&str] = match verb {
        "start" => &["up", "-d", service],
        _ => &["restart", service],
    };
    if ctx.dry_run {
        warn!(
            task = "stack_health",
            service, "dry-run: would run compose {verb}"
        );
        return true;
    }
    match docker::compose(&ctx.cfg.paths.base, args).await {
        Ok(_) => {
            warn!(task = "stack_health", service, "compose {verb} done");
            if ctx.cfg.discord.admin_alerts {
                crate::discord::notify(
                    ctx,
                    crate::discord::Channel::Admin,
                    crate::discord::Embed::warn(
                        format!("Service relancé : {service}"),
                        format!("`docker compose {verb} {service}` par stack_health (conteneur unhealthy ou sonde en échec). Voir `journalctl -u homelabd`."),
                    ),
                )
                .await;
            }
            true
        }
        Err(e) => {
            error!(task = "stack_health", service, error = %e, "compose {verb} failed");
            false
        }
    }
}

#[async_trait]
impl Task for StackHealth {
    fn name(&self) -> &'static str {
        "stack_health"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.stack_health.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.stack_health;
        let base = &ctx.cfg.paths.base;
        let expected: Vec<String> = docker::compose(base, &["config", "--services"])
            .await?
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !cfg.ignore.iter().any(|i| i == s))
            .map(str::to_string)
            .collect();
        let raw = docker::compose(base, &["ps", "-a", "--format", "json"]).await?;
        let containers: BTreeMap<String, Container> = parse_ps(&raw)?
            .into_iter()
            .map(|c| (c.service.clone(), c))
            .collect();
        let ts = now();
        let (mut started, mut restarted, mut waiting, mut failed) =
            (vec![], vec![], vec![], vec![]);

        for svc in &expected {
            let c = containers.get(svc);
            let (since, last) = ctx
                .state
                .read(|s| {
                    (
                        s.unhealthy_since.get(svc).copied(),
                        s.restarts.get(svc).copied(),
                    )
                })
                .await;
            match decide(c, since, last, ts, cfg) {
                Action::Nothing => {
                    if since.is_some() {
                        ctx.state.update(|s| s.unhealthy_since.remove(svc)).await?;
                    }
                }
                Action::Start => {
                    let state = c.map(|c| c.state.as_str()).unwrap_or("absent");
                    warn!(task = "stack_health", service = %svc, state, "not running");
                    if compose_action(ctx, "start", svc).await {
                        started.push(svc.clone());
                    } else {
                        failed.push(svc.clone());
                    }
                }
                Action::Wait => {
                    if since.is_none() {
                        ctx.state
                            .update(|s| s.unhealthy_since.insert(svc.clone(), ts))
                            .await?;
                    }
                    info!(task = "stack_health", service = %svc, "unhealthy, waiting (grace/cooldown)");
                    waiting.push(svc.clone());
                }
                Action::Restart => {
                    warn!(task = "stack_health", service = %svc, since = ts - since.unwrap_or(ts), "unhealthy too long");
                    if compose_action(ctx, "restart", svc).await {
                        restarted.push(svc.clone());
                        ctx.state
                            .update(|s| {
                                s.restarts.insert(svc.clone(), ts);
                                s.unhealthy_since.remove(svc);
                            })
                            .await?;
                    } else {
                        failed.push(svc.clone());
                    }
                }
            }
        }

        for p in &cfg.probes {
            if !expected.contains(&p.service)
                || restarted.contains(&p.service)
                || started.contains(&p.service)
            {
                continue;
            }
            if containers
                .get(&p.service)
                .map(|c| c.state != "running")
                .unwrap_or(true)
            {
                continue;
            }
            let outcome = run_probe(ctx, p).await;
            let verdict = match &outcome {
                Ok((status, body)) => probe_verdict(p, *status, body),
                Err(_) => Verdict::Fail,
            };
            match verdict {
                Verdict::Ok => continue,
                Verdict::Inconclusive => {
                    info!(task = "stack_health", service = %p.service, "probe throttled (429), inconclusive");
                    continue;
                }
                Verdict::Fail => {}
            }
            let detail = match &outcome {
                Ok((status, body)) => format!(
                    "HTTP {status} {}",
                    body.chars().take(120).collect::<String>()
                ),
                Err(e) => e.to_string(),
            };
            let deps_ready: Vec<&String> = p
                .requires_healthy
                .iter()
                .filter(|d| {
                    containers
                        .get(*d)
                        .map(|c| c.health != "healthy")
                        .unwrap_or(true)
                })
                .collect();
            if !deps_ready.is_empty() {
                info!(task = "stack_health", service = %p.service, %detail, waiting_for = ?deps_ready, "probe failed, dependency not healthy yet");
                waiting.push(p.service.clone());
                continue;
            }
            let last = ctx
                .state
                .read(|s| s.restarts.get(&p.service).copied())
                .await;
            if !cooldown_ok(last, ts, cfg) {
                info!(task = "stack_health", service = %p.service, %detail, "probe failed, restart cooldown active");
                waiting.push(p.service.clone());
                continue;
            }
            warn!(task = "stack_health", service = %p.service, %detail, url = %p.url, "probe failed");
            if compose_action(ctx, "restart", &p.service).await {
                restarted.push(p.service.clone());
                ctx.state
                    .update(|s| s.restarts.insert(p.service.clone(), ts))
                    .await?;
            } else {
                failed.push(p.service.clone());
            }
        }

        let running = expected
            .iter()
            .filter(|s| {
                containers
                    .get(*s)
                    .map(|c| c.state == "running")
                    .unwrap_or(false)
            })
            .count();
        let healthy = expected
            .iter()
            .filter(|s| {
                containers
                    .get(*s)
                    .map(|c| c.health == "healthy")
                    .unwrap_or(false)
            })
            .count();
        let actions = (started.len() + restarted.len()) as u32;
        if !failed.is_empty() {
            error!(
                task = "stack_health",
                ?failed,
                "services still broken after action"
            );
        }
        Ok(Report::new(
            format!(
                "expected={} running={running} healthy={healthy} started={started:?} restarted={restarted:?} waiting={waiting:?} failed={failed:?}",
                expected.len()
            ),
            actions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(state: &str, health: &str) -> Container {
        Container {
            service: "x".into(),
            state: state.into(),
            health: health.into(),
        }
    }

    #[test]
    fn parses_lines_and_arrays() {
        let lines = "{\"Service\":\"a\",\"State\":\"running\",\"Health\":\"healthy\"}\n{\"Service\":\"b\",\"State\":\"exited\",\"Health\":\"\"}\n";
        let v = parse_ps(lines).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].state, "exited");
        let arr = "[{\"Service\":\"a\",\"State\":\"running\"}]";
        assert_eq!(parse_ps(arr).unwrap()[0].health, "");
        assert!(parse_ps("   ").unwrap().is_empty());
    }

    #[test]
    fn decision_table() {
        let cfg = Cfg::default();
        assert_eq!(decide(None, None, None, 1000, &cfg), Action::Start);
        assert_eq!(
            decide(Some(&c("exited", "")), None, None, 1000, &cfg),
            Action::Start
        );
        assert_eq!(
            decide(Some(&c("running", "")), None, None, 1000, &cfg),
            Action::Nothing
        );
        assert_eq!(
            decide(Some(&c("running", "healthy")), None, None, 1000, &cfg),
            Action::Nothing
        );
        assert_eq!(
            decide(Some(&c("running", "starting")), None, None, 1000, &cfg),
            Action::Nothing
        );
        // unhealthy tout juste vu : grâce
        assert_eq!(
            decide(Some(&c("running", "unhealthy")), None, None, 1000, &cfg),
            Action::Wait
        );
        // unhealthy depuis 3 min, jamais redémarré : restart
        assert_eq!(
            decide(
                Some(&c("running", "unhealthy")),
                Some(820),
                None,
                1000,
                &cfg
            ),
            Action::Restart
        );
        // redémarré il y a 1 min : cooldown
        assert_eq!(
            decide(
                Some(&c("running", "unhealthy")),
                Some(820),
                Some(940),
                1000,
                &cfg
            ),
            Action::Wait
        );
        assert_eq!(
            decide(
                Some(&c("running", "unhealthy")),
                Some(0),
                Some(0),
                1000,
                &cfg
            ),
            Action::Restart
        );
    }

    #[test]
    fn probe_evaluation() {
        let p = Cfg::default().probes.remove(0);
        assert_eq!(
            probe_verdict(&p, 403, r#"{"type":"INVALID_CREDENTIALS"}"#),
            Verdict::Ok
        );
        assert_eq!(
            probe_verdict(&p, 500, r#"{"type":"INTERNAL_ERROR"}"#),
            Verdict::Fail
        );
        assert_eq!(
            probe_verdict(&p, 403, r#"{"type":"INTERNAL_ERROR"}"#),
            Verdict::Fail
        );
        assert_eq!(probe_verdict(&p, 429, ""), Verdict::Inconclusive);
    }
}
