//! Remise en service des indexeurs mis en pause par Sonarr/Radarr après des échecs.
//!
//! Après une erreur (délai dépassé, **429 « API Request Limit reached »** de C411), l'Arr met l'indexeur en
//! pause, de plus en plus longtemps : jusqu'à 24 h. Pendant ce temps, plus aucune recherche ni aucun
//! `release/push` ne passe. Sonarr et Radarr n'ont pas d'API pour lever cette pause (`indexerstatus` → 404) :
//! l'état est dans la table `IndexerStatus` de leur base.
//!
//! Toutes les 10 min, sur les 4 Arrs : lecture seule de `IndexerStatus`. Un indexeur en pause dont le
//! **dernier échec date d'au moins `quiet_mins`** (la fenêtre de limite de C411 est d'une heure ; plus tôt,
//! il se rebloquerait aussitôt) est remis en service : application arrêtée (~20 s), base sauvegardée, état
//! remis à zéro, application relancée et vérifiée. Au plus un arrêt par application par
//! `app_cooldown_mins`. Un indexeur débloqué `max_unblocks_per_day` fois en 24 h n'est plus débloqué
//! (site mort, Cloudflare…) : l'admin reçoit un mail pour décider. Chaque déblocage lui est aussi signalé.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;
use crate::docker;
use crate::state::now;

pub struct IndexerUnblock;

/// Une ligne de `IndexerStatus` (dates en secondes Unix).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusRow {
    pub provider_id: i64,
    pub name: String,
    pub most_recent_failure: Option<i64>,
    pub disabled_till: Option<i64>,
    pub escalation: i64,
}

/// Ce qu'il faut faire d'un indexeur en pause.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Pause levée maintenant.
    Unblock,
    /// Trop de déblocages en 24 h : on prévient l'admin, sans toucher à l'Arr.
    Alert,
    /// Dernier échec trop récent, ou pause déjà terminée : rien.
    Wait,
}

/// Dates des Arrs : `2026-09-17 19:33:45.9033185Z` (SQLite, .NET) → secondes Unix.
pub fn parse_arr_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let iso = s.replacen(' ', "T", 1);
    let iso = if iso.ends_with('Z') || iso.contains('+') {
        iso
    } else {
        format!("{iso}Z")
    };
    chrono::DateTime::parse_from_rfc3339(&iso)
        .ok()
        .map(|d| d.timestamp())
}

/// Sortie de `sqlite3 -separator '|'` : `id|nom|dernier échec|pause jusqu'à|niveau`.
pub fn parse_rows(out: &str) -> Vec<StatusRow> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            if f.len() < 5 {
                return None;
            }
            Some(StatusRow {
                provider_id: f[0].trim().parse().ok()?,
                name: f[1].trim().to_string(),
                most_recent_failure: parse_arr_date(f[2]),
                disabled_till: parse_arr_date(f[3]),
                escalation: f[4].trim().parse().unwrap_or(0),
            })
        })
        .collect()
}

/// Décision pour un indexeur. `past_unblocks` : dates des déblocages déjà faits pour lui.
pub fn decide(
    row: &StatusRow,
    now_secs: i64,
    quiet_mins: i64,
    past_unblocks: &[i64],
    max_per_day: u32,
) -> Decision {
    let Some(till) = row.disabled_till else {
        return Decision::Wait;
    };
    if till <= now_secs {
        return Decision::Wait;
    }
    let recent = past_unblocks
        .iter()
        .filter(|t| now_secs - **t < 24 * 3600)
        .count() as u32;
    if recent >= max_per_day {
        return Decision::Alert;
    }
    match row.most_recent_failure {
        Some(f) if now_secs - f < quiet_mins * 60 => Decision::Wait,
        _ => Decision::Unblock,
    }
}

const STATUS_SQL: &str =
    "SELECT s.ProviderId, COALESCE(i.Name, ''), COALESCE(s.MostRecentFailure, ''), \
     COALESCE(s.DisabledTill, ''), COALESCE(s.EscalationLevel, 0) FROM IndexerStatus s \
     LEFT JOIN Indexers i ON i.Id = s.ProviderId WHERE s.DisabledTill IS NOT NULL";

fn reset_sql(ids: &[i64]) -> String {
    let list: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!(
        "UPDATE IndexerStatus SET DisabledTill = NULL, EscalationLevel = 0, InitialFailure = NULL, \
         MostRecentFailure = NULL WHERE ProviderId IN ({})",
        list.join(",")
    )
}

/// Où vit un Arr et comment l'arrêter.
enum Place {
    /// Conteneur du VPS : service compose et base sur l'hôte.
    Vps { service: String, db: PathBuf },
    /// Application de la seedbox : `app-<nom> stop|start`, base lue et écrite par `ssh`.
    Seedbox { app: String, db: String },
}

struct App<'a> {
    key: String,
    arr: &'a ArrClient,
    place: Place,
}

async fn read_status(ctx: &TaskContext, app: &App<'_>) -> Result<Vec<StatusRow>> {
    match &app.place {
        Place::Vps { db, .. } => {
            let db = db.clone();
            tokio::task::spawn_blocking(move || -> Result<Vec<StatusRow>> {
                let conn = rusqlite::Connection::open_with_flags(
                    &db,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .with_context(|| format!("ouverture de {}", db.display()))?;
                let mut stmt = conn.prepare(STATUS_SQL)?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(format!(
                            "{}|{}|{}|{}|{}",
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(parse_rows(&rows.join("\n")))
            })
            .await?
        }
        Place::Seedbox { db, .. } => {
            let cmd =
                format!("sqlite3 -readonly -separator '|' \"file:{db}?mode=ro\" \"{STATUS_SQL}\"");
            let out = ssh(ctx, &cmd).await?;
            Ok(parse_rows(&out))
        }
    }
}

async fn ssh(ctx: &TaskContext, remote: &str) -> Result<String> {
    let host = ctx.cfg.tasks.indexer_unblock.ssh_host.as_str();
    docker::run(
        "ssh",
        &[
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=20",
            host,
            remote,
        ],
        None,
    )
    .await
}

/// Arrêt, sauvegarde, remise à zéro, relance, vérification.
async fn unblock(ctx: &TaskContext, app: &App<'_>, ids: &[i64]) -> Result<()> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    match &app.place {
        Place::Vps { service, db } => {
            let base = &ctx.cfg.paths.base;
            docker::compose(base, &["stop", service]).await?;
            let result = async {
                let backup = ctx
                    .cfg
                    .paths
                    .backups
                    .join(format!("arr-db-{}-{stamp}.db", app.key));
                std::fs::copy(db, &backup)
                    .with_context(|| format!("sauvegarde de {}", db.display()))?;
                // la base contient les clés des indexeurs
                std::fs::set_permissions(
                    &backup,
                    std::os::unix::fs::PermissionsExt::from_mode(0o600),
                )?;
                let (db, sql) = (db.clone(), reset_sql(ids));
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let conn = rusqlite::Connection::open(&db)?;
                    conn.execute(&sql, [])?;
                    Ok(())
                })
                .await??;
                prune_backups(&ctx.cfg.paths.backups, &app.key, 7);
                anyhow::Ok(())
            }
            .await;
            // l'application repart même si la remise à zéro a échoué
            docker::compose(base, &["start", service]).await?;
            result?;
        }
        Place::Seedbox { app: name, db } => {
            let sql = reset_sql(ids);
            let cmd = format!(
                "app-{name} stop >/dev/null 2>&1; cp {db} {db}.homelab-{stamp} && chmod 600 {db}.homelab-{stamp} && sqlite3 {db} \"{sql}\"; rc=$?; \
                 app-{name} start >/dev/null 2>&1; find $(dirname {db}) -maxdepth 1 -name '*.homelab-*' -mtime +7 -delete; exit $rc"
            );
            ssh(ctx, &cmd).await?;
        }
    }
    // l'application doit répondre de nouveau
    for _ in 0..24 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if app.arr.get("api/v3/system/status", &[]).await.is_ok() {
            return Ok(());
        }
    }
    bail!("{} ne répond plus après le redémarrage", app.arr.name)
}

fn prune_backups(dir: &Path, key: &str, days: u64) {
    let prefix = format!("arr-db-{key}-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|d| d > Duration::from_secs(days * 86400))
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

async fn notify(ctx: &TaskContext, subject: &str, body: &str) {
    crate::alerts::admin(ctx, crate::alerts::Level::Warn, subject, body).await;
}

fn fmt_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

#[async_trait]
impl Task for IndexerUnblock {
    fn name(&self) -> &'static str {
        "indexer_unblock"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.indexer_unblock.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.indexer_unblock;
        let base = &ctx.cfg.paths.base;
        let mut apps: Vec<App> = vec![
            App {
                key: "sonarr".into(),
                arr: &ctx.sonarr,
                place: Place::Vps {
                    service: "sonarr".into(),
                    db: base.join("sonarr/config/sonarr.db"),
                },
            },
            App {
                key: "radarr".into(),
                arr: &ctx.radarr,
                place: Place::Vps {
                    service: "radarr".into(),
                    db: base.join("radarr/config/radarr.db"),
                },
            },
        ];
        if let Some(arr) = &ctx.seedbox_sonarr {
            apps.push(App {
                key: "sonarr-seedbox".into(),
                arr,
                place: Place::Seedbox {
                    app: "sonarr".into(),
                    db: format!("{}/sonarr/sonarr.db", cfg.seedbox_apps_dir),
                },
            });
        }
        if let Some(arr) = &ctx.seedbox_radarr {
            apps.push(App {
                key: "radarr-seedbox".into(),
                arr,
                place: Place::Seedbox {
                    app: "radarr".into(),
                    db: format!("{}/radarr/radarr.db", cfg.seedbox_apps_dir),
                },
            });
        }

        let t = now();
        let (unblocks, restarts, alerts) = ctx
            .state
            .read(|s| {
                (
                    s.indexer_unblocks.clone(),
                    s.indexer_app_restarts.clone(),
                    s.indexer_alerts.clone(),
                )
            })
            .await;
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        let mut actions = 0;
        for app in &apps {
            let rows = match read_status(ctx, app).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(task = "indexer_unblock", app = %app.key, error = %e, "lecture de l'état impossible");
                    *counts.entry("error").or_default() += 1;
                    continue;
                }
            };
            let mut to_unblock: Vec<&StatusRow> = Vec::new();
            for row in &rows {
                let k = format!("{}:{}", app.key, row.provider_id);
                let past = unblocks.get(&k).cloned().unwrap_or_default();
                match decide(row, t, cfg.quiet_mins, &past, cfg.max_unblocks_per_day) {
                    Decision::Unblock => to_unblock.push(row),
                    Decision::Wait => {
                        if row.disabled_till.is_some_and(|d| d > t) {
                            *counts.entry("waiting").or_default() += 1;
                            info!(task = "indexer_unblock", app = %app.key, indexer = %row.name, till = %fmt_time(row.disabled_till.unwrap_or(0)), "en pause, dernier échec trop récent : on attend");
                        }
                    }
                    Decision::Alert => {
                        *counts.entry("given_up").or_default() += 1;
                        let last = alerts.get(&k).copied().unwrap_or(0);
                        if t - last >= 24 * 3600 && !ctx.dry_run {
                            notify(
                                ctx,
                                &format!("Indexeur {} toujours en panne ({})", row.name, app.key),
                                &format!(
                                    "L'indexeur « {} » de {} a déjà été remis en service {} fois en 24 h et retombe en panne.\n\
                                     Il reste en pause jusqu'au {}. Il est probablement mort ou derrière Cloudflare :\n\
                                     à vérifier, et à retirer de {} s'il ne sert plus.\n",
                                    row.name,
                                    app.key,
                                    cfg.max_unblocks_per_day,
                                    fmt_time(row.disabled_till.unwrap_or(0)),
                                    app.key
                                ),
                            )
                            .await;
                            let k2 = k.clone();
                            ctx.state.update(|s| s.indexer_alerts.insert(k2, t)).await?;
                        }
                    }
                }
            }
            if to_unblock.is_empty() {
                continue;
            }
            let last_restart = restarts.get(&app.key).copied().unwrap_or(0);
            if t - last_restart < cfg.app_cooldown_mins * 60 {
                *counts.entry("cooldown").or_default() += to_unblock.len() as u32;
                continue;
            }
            let names: Vec<String> = to_unblock.iter().map(|r| r.name.clone()).collect();
            if ctx.dry_run {
                info!(task = "indexer_unblock", app = %app.key, indexers = ?names, "dry-run: would unblock");
                *counts.entry("dry_run").or_default() += to_unblock.len() as u32;
                continue;
            }
            let ids: Vec<i64> = to_unblock.iter().map(|r| r.provider_id).collect();
            match unblock(ctx, app, &ids).await {
                Ok(()) => {
                    actions += ids.len() as u32;
                    *counts.entry("unblocked").or_default() += ids.len() as u32;
                    info!(task = "indexer_unblock", app = %app.key, indexers = ?names, "indexeurs remis en service");
                    let key = app.key.clone();
                    let entries: Vec<String> =
                        ids.iter().map(|id| format!("{}:{id}", app.key)).collect();
                    ctx.state
                        .update(|s| {
                            s.indexer_app_restarts.insert(key, t);
                            for e in entries {
                                let v = s.indexer_unblocks.entry(e).or_default();
                                v.retain(|d| t - *d < 7 * 24 * 3600);
                                v.push(t);
                            }
                        })
                        .await?;
                    let lines: Vec<String> = to_unblock
                        .iter()
                        .map(|r| {
                            format!(
                                "- {} : en pause jusqu'au {} (niveau {}), dernier échec le {}",
                                r.name,
                                fmt_time(r.disabled_till.unwrap_or(0)),
                                r.escalation,
                                r.most_recent_failure
                                    .map(fmt_time)
                                    .unwrap_or_else(|| "?".into())
                            )
                        })
                        .collect();
                    notify(
                        ctx,
                        &format!("Indexeurs remis en service ({})", app.key),
                        &format!(
                            "homelabd a levé la pause de {} indexeur(s) sur {} (application arrêtée ~20 s) :\n\n{}\n\n\
                             Base sauvegardée avant modification.\n",
                            ids.len(),
                            app.key,
                            lines.join("\n")
                        ),
                    )
                    .await;
                }
                Err(e) => {
                    *counts.entry("error").or_default() += 1;
                    warn!(task = "indexer_unblock", app = %app.key, error = %e, "déblocage en échec");
                    // l'application a quand même été arrêtée : pas de nouvel essai avant le délai de grâce
                    let key = app.key.clone();
                    ctx.state
                        .update(|s| {
                            s.indexer_app_restarts.insert(key, t);
                        })
                        .await?;
                }
            }
        }
        let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let summary = if summary.is_empty() {
            "aucun indexeur en pause".to_string()
        } else {
            summary.join(" ")
        };
        Ok(Report::new(summary, actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(failure: Option<i64>, till: Option<i64>) -> StatusRow {
        StatusRow {
            provider_id: 2,
            name: "C411".into(),
            most_recent_failure: failure,
            disabled_till: till,
            escalation: 9,
        }
    }

    #[test]
    fn parses_arr_dates_and_sqlite_rows() {
        let d = parse_arr_date("2026-09-17 19:33:45.9033185Z").unwrap();
        assert_eq!(
            d,
            chrono::DateTime::parse_from_rfc3339("2026-09-17T19:33:45Z")
                .unwrap()
                .timestamp()
        );
        assert_eq!(parse_arr_date(""), None);
        let rows = parse_rows(
            "2|C411|2026-09-16 19:33:45.9Z|2026-09-17 19:33:45.9Z|9\n6|JK-thepiratebay|||3\nbad",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "C411");
        assert_eq!(rows[0].escalation, 9);
        assert!(rows[1].disabled_till.is_none());
    }

    #[test]
    fn unblocks_only_after_the_quiet_window() {
        let now = 1_000_000;
        // en pause, dernier échec il y a 20 min : C411 se rebloquerait
        assert_eq!(
            decide(&row(Some(now - 1200), Some(now + 3600)), now, 60, &[], 3),
            Decision::Wait
        );
        // dernier échec il y a 2 h : on lève la pause
        assert_eq!(
            decide(&row(Some(now - 7200), Some(now + 80000)), now, 60, &[], 3),
            Decision::Unblock
        );
        // pause déjà terminée
        assert_eq!(
            decide(&row(Some(now - 7200), Some(now - 10)), now, 60, &[], 3),
            Decision::Wait
        );
        assert_eq!(decide(&row(None, None), now, 60, &[], 3), Decision::Wait);
    }

    #[test]
    fn repeated_failures_only_alert() {
        let now = 1_000_000;
        let past = [now - 3600, now - 7200, now - 10800];
        assert_eq!(
            decide(&row(Some(now - 7200), Some(now + 3600)), now, 60, &past, 3),
            Decision::Alert
        );
        // déblocages vieux de plus de 24 h : ils ne comptent plus
        let old = [now - 90000, now - 100000, now - 110000];
        assert_eq!(
            decide(&row(Some(now - 7200), Some(now + 3600)), now, 60, &old, 3),
            Decision::Unblock
        );
    }

    #[test]
    fn reset_touches_only_the_given_indexers() {
        assert_eq!(
            reset_sql(&[2, 6]),
            "UPDATE IndexerStatus SET DisabledTill = NULL, EscalationLevel = 0, InitialFailure = NULL, \
             MostRecentFailure = NULL WHERE ProviderId IN (2,6)"
        );
    }
}
