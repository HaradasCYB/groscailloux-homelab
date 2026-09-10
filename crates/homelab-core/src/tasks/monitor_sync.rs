//! Réconcilie `seasons[].monitored` de Sonarr avec les saisons demandées dans
//! Jellyseerr (ex `jellyseerr-sonarr-monitor-sync.sh`, toutes les 10 min).
//! Jellyseerr ajoute les séries sans `addOptions.monitor` ⇒ Sonarr monitore tout.
//! Règle : monitored = demandée OR a déjà des fichiers ; S00 jamais touchée ;
//! les séries inconnues de Jellyseerr ne sont pas modifiées.
//! Source : l'API Jellyseerr (plus de lecture directe du SQLite).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct MonitorSync;

/// Statuts Jellyseerr exclus : 3 = DECLINED, 4 = FAILED.
const EXCLUDED_STATUS: [i64; 2] = [3, 4];

/// sonarrSeriesId → saisons demandées (hors demandes refusées/échouées).
pub fn requested_seasons(requests: &[Value]) -> BTreeMap<i64, BTreeSet<i64>> {
    let mut out: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for r in requests {
        if r.get("type").and_then(Value::as_str) != Some("tv") {
            continue;
        }
        let Some(sid) = r
            .pointer("/media/externalServiceId")
            .and_then(Value::as_i64)
        else {
            continue;
        };
        let Some(seasons) = r.get("seasons").and_then(Value::as_array) else {
            continue;
        };
        for s in seasons {
            let status = s.get("status").and_then(Value::as_i64).unwrap_or(0);
            if EXCLUDED_STATUS.contains(&status) {
                continue;
            }
            if let Some(n) = s.get("seasonNumber").and_then(Value::as_i64) {
                out.entry(sid).or_default().insert(n);
            }
        }
    }
    out
}

/// Applique la règle sur la copie de `series`. Renvoie (avant, après) des saisons monitorées.
pub fn apply(series: &mut Value, requested: &BTreeSet<i64>) -> (Vec<i64>, Vec<i64>) {
    let before = monitored(series);
    if let Some(seasons) = series.get_mut("seasons").and_then(Value::as_array_mut) {
        for s in seasons.iter_mut() {
            let n = s.get("seasonNumber").and_then(Value::as_i64).unwrap_or(-1);
            if n == 0 {
                continue;
            }
            let has_files = s
                .pointer("/statistics/episodeFileCount")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                > 0;
            s["monitored"] = Value::Bool(requested.contains(&n) || has_files);
        }
    }
    (before, monitored(series))
}

fn monitored(series: &Value) -> Vec<i64> {
    series
        .get("seasons")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter(|s| s.get("monitored").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|s| s.get("seasonNumber").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait]
impl Task for MonitorSync {
    fn name(&self) -> &'static str {
        "monitor_sync"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.monitor_sync.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let requests = ctx.jellyseerr.all_requests().await?;
        let map = requested_seasons(&requests);
        if map.is_empty() {
            return Ok(Report::new("no_jellyseerr_requests", 0));
        }
        let all = ctx.sonarr.series().await?;
        let mut changes = 0u32;
        for mut series in all {
            let Some(id) = series.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let Some(req) = map.get(&id) else { continue };
            let title = series
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let (before, after) = apply(&mut series, req);
            if before == after {
                continue;
            }
            if ctx.dry_run {
                info!(task = "monitor_sync", series_id = id, %title, ?req, ?before, ?after, "dry-run: would sync");
                changes += 1;
                continue;
            }
            match ctx.sonarr.put_series(id, &series).await {
                Ok(_) => {
                    changes += 1;
                    info!(task = "monitor_sync", series_id = id, %title, ?req, ?before, ?after, "synced");
                }
                Err(e) => {
                    warn!(task = "monitor_sync", series_id = id, %title, error = %e, "put_failed")
                }
            }
        }
        Ok(Report::new(
            format!("changes={changes} managed_series={}", map.len()),
            changes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requested_map_skips_declined_and_movies() {
        let reqs = vec![
            json!({"type":"tv","media":{"externalServiceId":5},"seasons":[{"seasonNumber":1,"status":2},{"seasonNumber":2,"status":3}]}),
            json!({"type":"tv","media":{"externalServiceId":5},"seasons":[{"seasonNumber":3,"status":5}]}),
            json!({"type":"movie","media":{"externalServiceId":9},"seasons":[]}),
            json!({"type":"tv","media":{},"seasons":[{"seasonNumber":1,"status":2}]}),
        ];
        let m = requested_seasons(&reqs);
        assert_eq!(m.len(), 1);
        assert_eq!(m[&5], BTreeSet::from([1, 3]));
    }

    #[test]
    fn apply_rule_keeps_specials_and_files() {
        let mut s = json!({"id":5,"seasons":[
            {"seasonNumber":0,"monitored":true},
            {"seasonNumber":1,"monitored":true,"statistics":{"episodeFileCount":0}},
            {"seasonNumber":2,"monitored":true,"statistics":{"episodeFileCount":3}},
            {"seasonNumber":3,"monitored":false,"statistics":{"episodeFileCount":0}},
        ]});
        let (before, after) = apply(&mut s, &BTreeSet::from([3]));
        assert_eq!(before, vec![0, 1, 2]);
        assert_eq!(after, vec![0, 2, 3]);
    }
}
