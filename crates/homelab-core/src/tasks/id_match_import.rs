//! Débloque les imports que Radarr/Sonarr refusent avec « Found matching movie/series via grab
//! history, but release was matched to … by ID » : fréquent avec les releases C411 titrées en
//! français, que le parseur ne relie pas au titre anglais de la fiche alors que l'indexer a
//! fourni l'id TMDB/TVDB au moment du grab. L'historique désigne donc la bonne fiche : on lance
//! un import manuel rattaché à cette fiche et au téléchargement.
//! Garde-fous : téléchargement terminé, état `importBlocked`, ce message précis ; seuls les
//! fichiers sans aucun rejet sont importés ; séries : uniquement les fichiers dont l'épisode est
//! identifié. Plafond de téléchargements traités par passage.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;

pub struct IdMatchImport;

const MAX_PER_RUN: usize = 10;

pub fn is_id_match_blocked(record: &Value) -> bool {
    let completed = record.get("status").and_then(Value::as_str) == Some("completed");
    let blocked =
        record.get("trackedDownloadState").and_then(Value::as_str) == Some("importBlocked");
    let message = record
        .get("statusMessages")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| m.get("messages").and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .any(|m| m.contains("was matched to") && m.contains("by ID"))
        })
        .unwrap_or(false);
    completed && blocked && message
}

/// Fichiers importables d'un aperçu d'import manuel, prêts pour la commande `ManualImport`.
pub fn select_files(
    candidates: &[Value],
    radarr: bool,
    id: i64,
    download_id: &str,
) -> (Vec<Value>, Vec<String>) {
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    for c in candidates {
        let path = c.get("path").and_then(Value::as_str).unwrap_or("");
        let rel = c
            .get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_string();
        let rejections: Vec<&str> = c
            .get("rejections")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(|x| x.get("reason").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        if !rejections.is_empty() {
            skipped.push(format!("{rel}: {}", rejections.join(", ")));
            continue;
        }
        let mut f = json!({
            "path": path,
            "quality": c.get("quality"),
            "languages": c.get("languages").cloned().unwrap_or_else(|| json!([])),
            "releaseGroup": c.get("releaseGroup"),
            "indexerFlags": c.get("indexerFlags").and_then(Value::as_i64).unwrap_or(0),
            "downloadId": download_id,
        });
        if radarr {
            f["movieId"] = json!(id);
        } else {
            let eps: Vec<Value> = c
                .get("episodes")
                .and_then(Value::as_array)
                .map(|e| e.iter().filter_map(|x| x.get("id").cloned()).collect())
                .unwrap_or_default();
            if eps.is_empty() {
                skipped.push(format!("{rel}: épisode non identifié"));
                continue;
            }
            f["seriesId"] = json!(id);
            f["episodeIds"] = json!(eps);
            f["releaseType"] = c
                .get("releaseType")
                .cloned()
                .unwrap_or_else(|| json!("singleEpisode"));
        }
        files.push(f);
    }
    (files, skipped)
}

async fn process(ctx: &TaskContext, arr: &ArrClient, budget: &mut usize) -> Result<u32> {
    let radarr = arr.is_radarr();
    let (id_field, id_param) = if radarr {
        ("movieId", "movieId")
    } else {
        ("seriesId", "seriesId")
    };
    let mut imported = 0u32;
    for rec in arr
        .queue_records()
        .await?
        .iter()
        .filter(|r| is_id_match_blocked(r))
    {
        if *budget == 0 {
            break;
        }
        let title = rec.get("title").and_then(Value::as_str).unwrap_or("?");
        let (Some(id), Some(did)) = (
            rec.get(id_field).and_then(Value::as_i64),
            rec.get("downloadId").and_then(Value::as_str),
        ) else {
            continue;
        };
        *budget -= 1;
        let candidates = arr.manual_import_download(did, id_param, id).await?;
        let (files, skipped) = select_files(&candidates, radarr, id, did);
        for s in &skipped {
            warn!(task = "id_match_import", service = arr.name, %title, file = %s, "skipped, manual import needed");
        }
        if files.is_empty() {
            continue;
        }
        if ctx.dry_run {
            info!(task = "id_match_import", service = arr.name, %title, files = files.len(), "dry-run: would ManualImport");
            imported += files.len() as u32;
            continue;
        }
        let cmd = json!({ "name": "ManualImport", "files": files, "importMode": "auto" });
        match arr.command(cmd).await {
            Ok(r) => {
                imported += files.len() as u32;
                let cmd_id = r.get("id").and_then(Value::as_i64).unwrap_or(0);
                info!(task = "id_match_import", service = arr.name, %title, files = files.len(), cmd_id, "manual import triggered");
            }
            Err(e) => {
                warn!(task = "id_match_import", service = arr.name, %title, error = %e, "manual import failed")
            }
        }
    }
    Ok(imported)
}

#[async_trait]
impl Task for IdMatchImport {
    fn name(&self) -> &'static str {
        "id_match_import"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.id_match_import.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let mut budget = MAX_PER_RUN;
        let mut total = 0u32;
        for arr in ctx.all_arrs() {
            match process(ctx, arr, &mut budget).await {
                Ok(n) => total += n,
                Err(e) => {
                    warn!(task = "id_match_import", service = arr.name, error = %e, "queue check failed")
                }
            }
        }
        Ok(Report::new(format!("files_imported={total}"), total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(status: &str, state: &str, msg: &str) -> Value {
        json!({"status": status, "trackedDownloadState": state, "movieId": 36, "downloadId": "ABC",
               "statusMessages": [{"title": "x", "messages": [msg]}]})
    }

    #[test]
    fn detects_only_the_id_match_block() {
        let m = "Found matching movie via grab history, but release was matched to movie by ID. Manual Import required.";
        assert!(is_id_match_blocked(&rec("completed", "importBlocked", m)));
        let s = "Found matching series via grab history, but release was matched to series by ID. Automatic import is not possible.";
        assert!(is_id_match_blocked(&rec("completed", "importBlocked", s)));
        assert!(!is_id_match_blocked(&rec(
            "downloading",
            "importBlocked",
            m
        )));
        assert!(!is_id_match_blocked(&rec("completed", "importPending", m)));
        assert!(!is_id_match_blocked(&rec(
            "completed",
            "importBlocked",
            "Not an upgrade for existing movie file"
        )));
    }

    #[test]
    fn keeps_rejections_and_unidentified_episodes_out() {
        let cands = vec![
            json!({"path": "/d/a.mkv", "relativePath": "a.mkv", "rejections": [], "quality": {}, "episodes": [{"id": 7}]}),
            json!({"path": "/d/b.mkv", "relativePath": "b.mkv", "rejections": [{"reason": "Not an upgrade"}], "episodes": [{"id": 8}]}),
            json!({"path": "/d/c.mkv", "relativePath": "c.mkv", "rejections": [], "episodes": []}),
        ];
        let (files, skipped) = select_files(&cands, false, 5, "H");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["episodeIds"], json!([7]));
        assert_eq!(files[0]["seriesId"], 5);
        assert_eq!(files[0]["downloadId"], "H");
        assert_eq!(skipped.len(), 2);
        let (movie, _) = select_files(&cands[..1], true, 36, "H");
        assert_eq!(movie[0]["movieId"], 36);
        assert!(movie[0].get("seriesId").is_none());
    }
}
