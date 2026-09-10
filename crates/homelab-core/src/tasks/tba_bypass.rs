//! Contourne le garde-fou Sonarr « Episode has a TBA title and recently aired »
//! (ex `tba-import-bypass.sh`, toutes les 5 min). Conditions strictes : un seul
//! rejet, et c'est celui-là ; série + épisode identifiés ; pas déjà de fichier.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct TbaBypass;

pub fn is_candidate(item: &Value) -> bool {
    let rejections = item.get("rejections").and_then(Value::as_array);
    let only_tba = rejections
        .map(|r| {
            r.len() == 1
                && r[0]
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(|s| s.starts_with("Episode has a TBA title"))
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    let has_series = item.pointer("/series/id").and_then(Value::as_i64).is_some();
    let episodes = item.get("episodes").and_then(Value::as_array);
    let has_episode = episodes.map(|e| !e.is_empty()).unwrap_or(false);
    let no_file = episodes
        .and_then(|e| e.first())
        .and_then(|e| e.get("episodeFileId"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        == 0;
    only_tba && has_series && has_episode && no_file
}

pub fn import_file(item: &Value) -> Value {
    let path = item.get("path").and_then(Value::as_str).unwrap_or("");
    let folder = path
        .rsplit_once('/')
        .map(|(d, _)| d)
        .filter(|d| !d.is_empty())
        .unwrap_or("/downloads");
    let episode_ids: Vec<Value> = item
        .get("episodes")
        .and_then(Value::as_array)
        .map(|e| e.iter().filter_map(|x| x.get("id").cloned()).collect())
        .unwrap_or_default();
    json!({
        "path": path,
        "folderName": folder,
        "seriesId": item.pointer("/series/id"),
        "episodeIds": episode_ids,
        "releaseGroup": item.get("releaseGroup"),
        "quality": item.get("quality"),
        "languages": item.get("languages"),
        "indexerFlags": item.get("indexerFlags").and_then(Value::as_i64).unwrap_or(0),
        "releaseType": item.get("releaseType").and_then(Value::as_str).unwrap_or("singleEpisode"),
        "episodeFileId": 0,
        "downloadId": null
    })
}

fn label(item: &Value) -> String {
    let title = item
        .pointer("/series/title")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let s = item
        .get("seasonNumber")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let e = item
        .pointer("/episodes/0/episodeNumber")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    format!("{title} S{s:02}E{e:02}")
}

#[async_trait]
impl Task for TbaBypass {
    fn name(&self) -> &'static str {
        "tba_bypass"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.tba_bypass.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let folder = ctx.cfg.paths.downloads_in_container.clone();
        let preview = ctx.sonarr.manual_import(&folder).await?;
        let candidates: Vec<&Value> = preview.iter().filter(|i| is_candidate(i)).collect();
        let (mut actions, mut errors) = (0u32, 0u32);
        for item in &candidates {
            let what = label(item);
            let path = item.get("path").and_then(Value::as_str).unwrap_or("");
            if ctx.dry_run {
                info!(task = "tba_bypass", %what, path, "dry-run: would trigger ManualImport");
                actions += 1;
                continue;
            }
            let cmd = json!({ "name": "ManualImport", "files": [import_file(item)], "importMode": "auto" });
            match ctx.sonarr.command(cmd).await {
                Ok(resp) => {
                    actions += 1;
                    let cmd_id = resp.get("id").and_then(Value::as_i64).unwrap_or(0);
                    info!(task = "tba_bypass", %what, path, cmd_id, "bypass_triggered");
                }
                Err(e) => {
                    errors += 1;
                    warn!(task = "tba_bypass", %what, path, error = %e, "post_failed");
                }
            }
        }
        Ok(Report::new(
            format!(
                "candidates={} actions={actions} errors={errors}",
                candidates.len()
            ),
            actions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(rej: Vec<&str>, file_id: i64) -> Value {
        json!({
            "path": "/downloads/Show.S02E05.mkv",
            "series": {"id": 7, "title": "Show"},
            "seasonNumber": 2,
            "episodes": [{"id": 99, "episodeNumber": 5, "episodeFileId": file_id}],
            "rejections": rej.iter().map(|r| json!({"reason": r})).collect::<Vec<_>>(),
            "quality": {"quality": {"id": 1}},
            "languages": [],
        })
    }

    #[test]
    fn only_exact_single_tba_rejection() {
        assert!(is_candidate(&item(
            vec!["Episode has a TBA title and recently aired"],
            0
        )));
        assert!(!is_candidate(&item(
            vec!["Episode has a TBA title and recently aired", "Sample"],
            0
        )));
        assert!(!is_candidate(&item(vec!["Unknown Series"], 0)));
        assert!(!is_candidate(&item(vec![], 0)));
        assert!(!is_candidate(&item(
            vec!["Episode has a TBA title and recently aired"],
            42
        )));
    }

    #[test]
    fn import_payload_shape() {
        let f = import_file(&item(vec!["Episode has a TBA title and recently aired"], 0));
        assert_eq!(f["folderName"], "/downloads");
        assert_eq!(f["seriesId"], 7);
        assert_eq!(f["episodeIds"], json!([99]));
        assert_eq!(f["releaseType"], "singleEpisode");
        assert_eq!(f["indexerFlags"], 0);
        assert_eq!(label(&item(vec![], 0)), "Show S02E05");
    }
}
