//! Réconcilie `seasons[].monitored` de Sonarr avec les saisons demandées dans
//! Jellyseerr (ex `jellyseerr-sonarr-monitor-sync.sh`, toutes les 10 min).
//! Jellyseerr ajoute les séries sans `addOptions.monitor` ⇒ Sonarr monitore tout.
//! Règle : monitored = (demandée ET pas déjà présente sur l'autre machine) OR a déjà des
//! fichiers ici ; S00 jamais touchée ;
//! les séries inconnues de Jellyseerr ne sont pas modifiées. Une saison supprimée dans Jellyfin
//! (`deletion_cleanup`, état `deletions.seasons`) n'est plus demandée, sauf nouvelle demande.
//! Plusieurs Sonarr (VPS, seedbox) : chaque demande est routée vers le Sonarr dont
//! l'id Jellyseerr vaut `media.serviceId` — les ids de séries diffèrent d'une instance
//! à l'autre, un mauvais routage modifierait une autre série.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;

pub struct MonitorSync;

/// Statuts Jellyseerr exclus : 3 = DECLINED, 4 = FAILED.
const EXCLUDED_STATUS: [i64; 2] = [3, 4];

/// (id du serveur Sonarr dans Jellyseerr, sonarrSeriesId) → saisons demandées.
pub type Requested = BTreeMap<(i64, i64), BTreeSet<i64>>;

pub fn requested_seasons(requests: &[Value]) -> Requested {
    let mut out: Requested = BTreeMap::new();
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
        let server = r
            .pointer("/media/serviceId")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(seasons) = r.get("seasons").and_then(Value::as_array) else {
            continue;
        };
        for s in seasons {
            let status = s.get("status").and_then(Value::as_i64).unwrap_or(0);
            if EXCLUDED_STATUS.contains(&status) {
                continue;
            }
            if let Some(n) = s.get("seasonNumber").and_then(Value::as_i64) {
                out.entry((server, sid)).or_default().insert(n);
            }
        }
    }
    out
}

/// Saisons supprimées sur une machine (`côté:tvdb:saison` → date), par tvdbId, sauf celles
/// redemandées depuis dans Jellyseerr (demande créée après la suppression).
pub fn deleted_seasons(
    deleted: &BTreeMap<String, i64>,
    side: &str,
    requests: &[Value],
) -> BTreeMap<i64, BTreeSet<i64>> {
    let mut out: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for (key, at) in deleted {
        let mut parts = key.splitn(3, ':');
        let (Some(s), Some(tvdb), Some(season)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(tvdb), Ok(season)) = (tvdb.parse::<i64>(), season.parse::<i64>()) else {
            continue;
        };
        if s != side {
            continue;
        }
        let asked_again = requests.iter().any(|r| {
            r.get("type").and_then(Value::as_str) == Some("tv")
                && r.pointer("/media/tvdbId").and_then(Value::as_i64) == Some(tvdb)
                && r.get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                    .map(|d| d.timestamp() > *at)
                    .unwrap_or(false)
                && r.get("seasons")
                    .and_then(Value::as_array)
                    .map(|l| {
                        l.iter().any(|x| {
                            x.get("seasonNumber").and_then(Value::as_i64) == Some(season)
                                && !EXCLUDED_STATUS
                                    .contains(&x.get("status").and_then(Value::as_i64).unwrap_or(0))
                        })
                    })
                    .unwrap_or(false)
        });
        if !asked_again {
            out.entry(tvdb).or_default().insert(season);
        }
    }
    out
}

/// Sous-ensemble des demandes qui concernent un serveur Sonarr donné.
pub fn for_server(map: &Requested, server: i64) -> BTreeMap<i64, BTreeSet<i64>> {
    map.iter()
        .filter(|((srv, _), _)| *srv == server)
        .map(|((_, sid), seasons)| (*sid, seasons.clone()))
        .collect()
}

/// Applique la règle sur la copie de `series`. Renvoie (avant, après) des saisons monitorées.
/// Saisons ayant au moins un fichier, par tvdbId (pour savoir ce qu'a l'autre machine).
pub fn seasons_with_files(all_series: &[Value]) -> BTreeMap<i64, BTreeSet<i64>> {
    let mut out: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for s in all_series {
        let Some(tvdb) = s.get("tvdbId").and_then(Value::as_i64).filter(|t| *t > 0) else {
            continue;
        };
        for season in s
            .get("seasons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let files = season
                .pointer("/statistics/episodeFileCount")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if let (Some(n), true) = (
                season.get("seasonNumber").and_then(Value::as_i64),
                files > 0,
            ) {
                out.entry(tvdb).or_default().insert(n);
            }
        }
    }
    out
}

/// Applique la règle sur la copie de `series`. `elsewhere` : saisons déjà présentes sur l'autre
/// machine (jamais suivies ici, sinon doublon dans Jellyfin). Renvoie (avant, après).
pub fn apply_with(
    series: &mut Value,
    requested: &BTreeSet<i64>,
    elsewhere: &BTreeSet<i64>,
) -> (Vec<i64>, Vec<i64>) {
    let wanted: BTreeSet<i64> = requested.difference(elsewhere).copied().collect();
    apply(series, &wanted)
}

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

async fn sync_one(
    ctx: &TaskContext,
    sonarr: &ArrClient,
    all_series: Vec<Value>,
    map: &BTreeMap<i64, BTreeSet<i64>>,
    elsewhere: &BTreeMap<i64, BTreeSet<i64>>,
) -> Result<u32> {
    if map.is_empty() {
        return Ok(0);
    }
    let empty = BTreeSet::new();
    let mut changes = 0u32;
    for mut series in all_series {
        let Some(id) = series.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let Some(req) = map.get(&id) else { continue };
        let title = series
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let tvdb = series.get("tvdbId").and_then(Value::as_i64).unwrap_or(0);
        let other = elsewhere.get(&tvdb).unwrap_or(&empty);
        let (before, after) = apply_with(&mut series, req, other);
        if before == after {
            continue;
        }
        if ctx.dry_run {
            info!(task = "monitor_sync", service = sonarr.name, series_id = id, %title, ?req, ?before, ?after, "dry-run: would sync");
            changes += 1;
            continue;
        }
        match sonarr.put_series(id, &series).await {
            Ok(_) => {
                changes += 1;
                info!(task = "monitor_sync", service = sonarr.name, series_id = id, %title, ?req, ?before, ?after, "synced");
            }
            Err(e) => {
                warn!(task = "monitor_sync", service = sonarr.name, series_id = id, %title, error = %e, "put_failed")
            }
        }
    }
    Ok(changes)
}

#[async_trait]
impl Task for MonitorSync {
    fn name(&self) -> &'static str {
        "monitor_sync"
    }

    fn label(&self) -> &'static str {
        "Saisons demandées"
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
        let sb = &ctx.cfg.seedbox;
        let mut targets = vec![(&ctx.sonarr, sb.jellyseerr_vps_sonarr_id, "vps")];
        if let Some(s) = &ctx.seedbox_sonarr {
            targets.push((s, sb.jellyseerr_sonarr_id, "seedbox"));
        }
        let deleted = ctx.state.read(|s| s.deletions.seasons.clone()).await;
        // séries de chaque Sonarr, puis saisons présentes « ailleurs » pour chacun
        let mut lists = Vec::new();
        for (sonarr, _, _) in &targets {
            lists.push(sonarr.series().await?);
        }
        let files: Vec<BTreeMap<i64, BTreeSet<i64>>> =
            lists.iter().map(|l| seasons_with_files(l)).collect();
        let mut changes = 0u32;
        let mut summary = Vec::new();
        for (i, ((sonarr, server, side), list)) in targets.into_iter().zip(lists).enumerate() {
            // saisons supprimées ici : traitées comme « déjà ailleurs » (jamais re-surveillées)
            let mut elsewhere = deleted_seasons(&deleted, side, &requests);
            for (j, f) in files.iter().enumerate() {
                if j != i {
                    for (tvdb, seasons) in f {
                        elsewhere.entry(*tvdb).or_default().extend(seasons);
                    }
                }
            }
            let subset = for_server(&map, server);
            changes += sync_one(ctx, sonarr, list, &subset, &elsewhere).await?;
            summary.push(format!("{}={}", sonarr.name, subset.len()));
        }
        Ok(Report::new(
            format!("changes={changes} managed_series[{}]", summary.join(" ")),
            changes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deleted_seasons_stay_excluded_until_requested_again() {
        let deleted = BTreeMap::from([
            ("vps:100:2".to_string(), 1_000_000_000),
            ("seedbox:100:3".to_string(), 1_000_000_000),
        ]);
        let old = json!({"type":"tv","createdAt":"2001-01-01T00:00:00.000Z","media":{"tvdbId":100},"seasons":[{"seasonNumber":2,"status":2}]});
        let ex = deleted_seasons(&deleted, "vps", std::slice::from_ref(&old));
        assert_eq!(ex.get(&100), Some(&BTreeSet::from([2])));
        let newer = json!({"type":"tv","createdAt":"2030-01-01T00:00:00.000Z","media":{"tvdbId":100},"seasons":[{"seasonNumber":2,"status":2}]});
        assert!(deleted_seasons(&deleted, "vps", &[old, newer]).is_empty());
        // l'autre machine n'est pas concernée par une suppression ici
        assert_eq!(
            deleted_seasons(&deleted, "seedbox", &[]).get(&100),
            Some(&BTreeSet::from([3]))
        );
    }

    #[test]
    fn requested_map_skips_declined_and_movies() {
        let reqs = vec![
            json!({"type":"tv","media":{"externalServiceId":5,"serviceId":0},"seasons":[{"seasonNumber":1,"status":2},{"seasonNumber":2,"status":3}]}),
            json!({"type":"tv","media":{"externalServiceId":5},"seasons":[{"seasonNumber":3,"status":5}]}),
            json!({"type":"movie","media":{"externalServiceId":9},"seasons":[]}),
            json!({"type":"tv","media":{},"seasons":[{"seasonNumber":1,"status":2}]}),
        ];
        let m = requested_seasons(&reqs);
        assert_eq!(m.len(), 1);
        assert_eq!(m[&(0, 5)], BTreeSet::from([1, 3]));
    }

    #[test]
    fn requests_are_routed_by_jellyseerr_server() {
        // même sonarrSeriesId 5 sur deux instances différentes : ce ne sont pas les mêmes séries
        let reqs = vec![
            json!({"type":"tv","media":{"externalServiceId":5,"serviceId":0},"seasons":[{"seasonNumber":1,"status":2}]}),
            json!({"type":"tv","media":{"externalServiceId":5,"serviceId":1},"seasons":[{"seasonNumber":4,"status":2}]}),
        ];
        let m = requested_seasons(&reqs);
        assert_eq!(for_server(&m, 0)[&5], BTreeSet::from([1]));
        assert_eq!(for_server(&m, 1)[&5], BTreeSet::from([4]));
        assert!(for_server(&m, 2).is_empty());
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

    #[test]
    fn seasons_present_on_the_other_machine_are_not_monitored() {
        let vps = vec![json!({"tvdbId": 72368, "seasons": [
            {"seasonNumber": 10, "statistics": {"episodeFileCount": 24}},
            {"seasonNumber": 9, "statistics": {"episodeFileCount": 0}}]})];
        let elsewhere = seasons_with_files(&vps);
        assert_eq!(elsewhere[&72368], BTreeSet::from([10]));
        let mut sb = json!({"id": 37, "tvdbId": 72368, "seasons": [
            {"seasonNumber": 9, "monitored": false, "statistics": {"episodeFileCount": 0}},
            {"seasonNumber": 10, "monitored": true, "statistics": {"episodeFileCount": 0}},
            {"seasonNumber": 14, "monitored": false, "statistics": {"episodeFileCount": 24}}]});
        let (_, after) = apply_with(&mut sb, &BTreeSet::from([9, 10, 14]), &elsewhere[&72368]);
        assert_eq!(after, vec![9, 14]);
    }
}
