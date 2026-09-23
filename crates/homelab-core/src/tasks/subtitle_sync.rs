//! Sous-titres incrustés des fichiers de la **seedbox** → fichiers externes à côté de la vidéo, **à codec
//! identique**, extraits **sur la seedbox** (disque local, rien sur le lien VPS), puis fiche Jellyfin relue.
//!
//! Pourquoi : Jellyfin extrait une piste incrustée en relisant tout le fichier par le lien (108 à 757 s mesurés
//! du 19 au 21/09) ; les membres attendaient plusieurs minutes, ou abandonnaient. Un fichier externe est lu
//! instantanément, et l'ASS externe est rendu exactement comme l'ASS incrusté (libass), panneaux à leur place.
//! Un fichier annexe n'est vu que par un **FullRefresh** de l'item (ni `Library/Media/Updated`, ni Refresh
//! « Default », vérifié le 21/09) : `jellyfin::refresh_streams` après chaque extraction.
//!
//! Sorties : `.fr.default.ass` (piste complète, passe devant), `.fr.forced.ass`, `.fr.hi.ass` (malentendants,
//! à part), `.fr.srt` pour une piste SRT ; pour une piste ASS, un `.fr.srt` **sans les panneaux** est dérivé
//! (AirPlay, téléviseurs). Les plus récents d'abord, `max_per_run` items par passage, jamais pendant une lecture.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::seedbox_refresh::refresh_params;
use super::{Report, Task};
use crate::config::{Config, Seedbox};
use crate::context::TaskContext;
use crate::docker;

pub struct SubtitleSync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// `ass` ou `subrip` (nom de codec ffmpeg, tel que Jellyfin le rapporte).
    pub codec: String,
    /// `full`, `forced` ou `hi`.
    pub kind: String,
    /// Chemin de sortie **côté Jellyfin** (`/seedbox/media/...`).
    pub out: String,
    /// SRT sans panneaux à dériver (piste ASS complète seulement).
    pub srt: Option<String>,
}

fn is_french(s: &Value) -> bool {
    matches!(
        s.get("Language").and_then(Value::as_str),
        Some("fre") | Some("fra") | Some("fr")
    )
}

fn flag(s: &Value, key: &str) -> bool {
    s.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Kind d'une piste d'après ses drapeaux Jellyfin (titre en secours : « Malentendants », « SDH », « Forced »).
fn kind_of(s: &Value) -> &'static str {
    let title = s
        .get("Title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if flag(s, "IsForced") || title.contains("forc") {
        "forced"
    } else if flag(s, "IsHearingImpaired")
        || title.contains("malentendant")
        || title.contains("sdh")
        || title.contains("[cc]")
    {
        "hi"
    } else {
        "full"
    }
}

/// Chemin de la vidéo sans extension → nom du fichier externe pour (codec, kind).
pub fn out_name(stem: &str, codec: &str, kind: &str) -> String {
    let ext = if codec == "subrip" { "srt" } else { "ass" };
    match kind {
        "forced" => format!("{stem}.fr.forced.{ext}"),
        "hi" => format!("{stem}.fr.hi.{ext}"),
        _ if ext == "ass" => format!("{stem}.fr.default.ass"),
        _ => format!("{stem}.fr.srt"),
    }
}

/// Extractions à faire pour un item Jellyfin : une par piste française incrustée texte (`ass`/`ssa`/`subrip`)
/// dont le fichier externe attendu n'est pas encore listé. Une piste ASS complète entraîne aussi le SRT dérivé.
pub fn plan_jobs(item: &Value) -> Vec<Job> {
    let Some(path) = item.get("Path").and_then(Value::as_str) else {
        return vec![];
    };
    let stem = Path::new(path)
        .with_extension("")
        .to_string_lossy()
        .to_string();
    let streams: Vec<&Value> = item
        .get("MediaStreams")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|s| s.get("Type").and_then(Value::as_str) == Some("Subtitle"))
                .collect()
        })
        .unwrap_or_default();
    let external: HashSet<String> = streams
        .iter()
        .filter(|s| flag(s, "IsExternal"))
        .filter_map(|s| s.get("Path").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut jobs: Vec<Job> = Vec::new();
    for s in streams
        .iter()
        .filter(|s| !flag(s, "IsExternal") && is_french(s))
    {
        let codec = match s.get("Codec").and_then(Value::as_str) {
            Some("ass") | Some("ssa") => "ass",
            Some("subrip") | Some("srt") => "subrip",
            _ => continue, // PGS/VobSub : images, rien à extraire en texte
        };
        let kind = kind_of(s);
        let out = out_name(&stem, codec, kind);
        if external.contains(&out) || jobs.iter().any(|j| j.out == out) {
            continue;
        }
        let srt = (codec == "ass" && kind == "full").then(|| format!("{stem}.fr.srt"));
        jobs.push(Job {
            codec: codec.into(),
            kind: kind.into(),
            out,
            srt,
        });
    }
    jobs
}

/// Chemin vu par Jellyfin (`/seedbox/media/...`) → chemin sur la seedbox (`/home/x/media/...`) et dossier
/// relatif pour rclone.
pub fn seedbox_path(sb: &Seedbox, jf: &str) -> Option<(String, String)> {
    let root = sb.jellyfin_root.trim_end_matches('/');
    let rel = jf.strip_prefix(root)?.trim_start_matches('/');
    let dir = Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Some((
        format!("{}/{}", sb.media_root.trim_end_matches('/'), rel),
        dir,
    ))
}

/// Argument sûr pour le shell distant.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[async_trait]
impl Task for SubtitleSync {
    fn name(&self) -> &'static str {
        "subtitle_sync"
    }

    fn label(&self) -> &'static str {
        "Sous-titres extraits"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.subtitle_sync.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let sb = &ctx.cfg.seedbox;
        let cfg = &ctx.cfg.tasks.subtitle_sync;
        if !sb.enabled || cfg.extract_script.is_empty() {
            return Ok(Report::new("disabled", 0));
        }
        if !sb.mount_point.is_dir() {
            warn!(task = "subtitle_sync", mount = %sb.mount_point.display(), "seedbox mount unavailable, retry next run");
            return Ok(Report::new("mount unavailable", 0));
        }
        // `Items` SANS UserId renvoie une liste incomplète (21/09 : les 13 épisodes du matin absents) : compte admin.
        let admin = ctx
            .jellyfin
            .users()
            .await?
            .iter()
            .find(|u| crate::accounts::is_admin(u))
            .and_then(|u| u.get("Id").and_then(Value::as_str).map(str::to_string))
            .context("aucun compte admin Jellyfin")?;
        let mut items = ctx
            .jellyfin
            .items(&[
                ("UserId", admin.as_str()),
                ("Recursive", "true"),
                ("IncludeItemTypes", "Episode,Movie"),
                ("Fields", "Path,MediaStreams,DateCreated"),
                ("EnableImages", "false"),
            ])
            .await
            .context("jellyfin Items")?;
        let root = format!("{}/", sb.jellyfin_root.trim_end_matches('/'));
        items.retain(|i| {
            i.get("Path")
                .and_then(Value::as_str)
                .map(|p| p.starts_with(&root))
                .unwrap_or(false)
        });
        // les plus récents d'abord (ce que les membres vont lancer)
        items.sort_by(|a, b| {
            let da = a.get("DateCreated").and_then(Value::as_str).unwrap_or("");
            let db = b.get("DateCreated").and_then(Value::as_str).unwrap_or("");
            db.cmp(da)
        });
        let playing: HashSet<String> = ctx
            .jellyfin
            .sessions()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                s.pointer("/NowPlayingItem/Id")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        let mut todo: Vec<(&Value, Vec<Job>)> = Vec::new();
        let mut pending_total = 0usize;
        for it in &items {
            let jobs = plan_jobs(it);
            if jobs.is_empty() {
                continue;
            }
            pending_total += 1;
            if todo.len() < cfg.max_per_run.max(1) {
                todo.push((it, jobs));
            }
        }
        if todo.is_empty() {
            return Ok(Report::new("nothing to extract", 0));
        }
        if ctx.dry_run {
            for (it, jobs) in &todo {
                let name = it.get("Name").and_then(Value::as_str).unwrap_or("?");
                let kinds: Vec<String> = jobs
                    .iter()
                    .map(|j| format!("{}/{}", j.codec, j.kind))
                    .collect();
                info!(task = "subtitle_sync", item = %name, ?kinds, "dry-run");
            }
            return Ok(Report::new(
                format!("dry_run items={} pending={pending_total}", todo.len()),
                todo.len() as u32,
            ));
        }
        let host = ctx.cfg.tasks.indexer_unblock.ssh_host.as_str();
        let (mut extracted, mut refreshed, mut skipped, mut failed) = (0u32, 0u32, 0u32, 0u32);
        let mut dirs: Vec<String> = Vec::new();
        let mut to_refresh: Vec<String> = Vec::new();
        // une extraction lit tout le fichier sur le disque de la seedbox (~30 s) : on s'arrête avant que le
        // planificateur ne coupe la tâche, le reste est repris au passage suivant (tout est idempotent)
        let started = std::time::Instant::now();
        let budget = Duration::from_secs(cfg.max_seconds.max(60));
        let mut out_of_time = false;
        for (it, jobs) in &todo {
            if started.elapsed() >= budget {
                out_of_time = true;
                break;
            }
            let id = it.get("Id").and_then(Value::as_str).unwrap_or("");
            let path = it.get("Path").and_then(Value::as_str).unwrap_or("");
            if playing.contains(id) {
                skipped += 1;
                continue;
            }
            let Some((video, dir)) = seedbox_path(sb, path) else {
                continue;
            };
            let mut ok_any = false;
            for j in jobs {
                let Some((out, _)) = seedbox_path(sb, &j.out) else {
                    continue;
                };
                let mut cmd = format!(
                    "{} {} {} {} {}",
                    cfg.extract_script,
                    sh_quote(&video),
                    j.codec,
                    j.kind,
                    sh_quote(&out)
                );
                if let Some(srt) = &j.srt {
                    if let Some((srt_sb, _)) = seedbox_path(sb, srt) {
                        cmd.push(' ');
                        cmd.push_str(&sh_quote(&srt_sb));
                    }
                }
                match docker::run(
                    "ssh",
                    &["-o", "BatchMode=yes", "-o", "ConnectTimeout=20", host, &cmd],
                    None,
                )
                .await
                {
                    Ok(_) => {
                        extracted += 1;
                        ok_any = true;
                    }
                    Err(e) => {
                        failed += 1;
                        warn!(task = "subtitle_sync", item = %path, kind = %j.kind, error = %e, "extraction failed");
                    }
                }
            }
            if ok_any {
                dirs.push(dir);
                to_refresh.push(id.to_string());
            }
        }
        dirs.sort();
        dirs.dedup();
        if !dirs.is_empty() {
            let rc = format!("{}/vfs/refresh", sb.rclone_rc.trim_end_matches('/'));
            match ctx.http.post(&rc).json(&refresh_params(&dirs)).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    warn!(task = "subtitle_sync", status = %resp.status(), "rclone vfs/refresh failed")
                }
                Err(e) => warn!(task = "subtitle_sync", error = %e, "rclone rc injoignable"),
                _ => {}
            }
        }
        for id in &to_refresh {
            match ctx.jellyfin.refresh_streams(id).await {
                Ok(()) => refreshed += 1,
                Err(e) => {
                    failed += 1;
                    warn!(task = "subtitle_sync", item = %id, error = %e, "refresh failed");
                }
            }
        }
        info!(
            task = "subtitle_sync",
            extracted,
            refreshed,
            skipped,
            failed,
            pending = pending_total,
            out_of_time,
            "done"
        );
        Ok(Report::new(
            format!(
                "{extracted} extrait(s), {refreshed} rafraîchi(s), {skipped} en lecture, {failed} échec(s), {pending_total} en attente{}",
                if out_of_time { " (budget atteint)" } else { "" }
            ),
            extracted,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jobs_follow_flags_and_codecs() {
        let item = json!({"Path": "/seedbox/media/Anime/A/Season 1/A - S01E01.mkv", "MediaStreams": [
            {"Type": "Video"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsForced": true, "Title": "Forced"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsDefault": true},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsHearingImpaired": true},
            {"Type": "Subtitle", "Language": "eng", "Codec": "ass"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "PGSSUB"}
        ]});
        let jobs = plan_jobs(&item);
        let outs: Vec<&str> = jobs.iter().map(|j| j.out.as_str()).collect();
        assert_eq!(
            outs,
            vec![
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.forced.ass",
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.default.ass",
                "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.hi.ass"
            ]
        );
        assert_eq!(jobs[0].srt, None);
        assert_eq!(
            jobs[1].srt.as_deref(),
            Some("/seedbox/media/Anime/A/Season 1/A - S01E01.fr.srt")
        );
        assert_eq!(jobs[1].kind, "full");
    }

    #[test]
    fn existing_external_files_are_skipped() {
        let item = json!({"Path": "/seedbox/media/Movies/M (2020)/M (2020).mkv", "MediaStreams": [
            {"Type": "Subtitle", "Language": "fra", "Codec": "subrip", "IsExternal": true, "Path": "/seedbox/media/Movies/M (2020)/M (2020).fr.srt"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "subrip"},
            {"Type": "Subtitle", "Language": "fra", "Codec": "ass", "Title": "Malentendants"}
        ]});
        let jobs = plan_jobs(&item);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "hi");
        assert!(jobs[0].out.ends_with("M (2020).fr.hi.ass"));
        let none = json!({"Path": "/seedbox/media/x.mkv", "MediaStreams": [{"Type": "Subtitle", "Language": "fra", "Codec": "ass", "IsExternal": true, "Path": "/seedbox/media/x.fr.default.ass"}]});
        assert!(plan_jobs(&none).is_empty());
    }

    #[test]
    fn paths_and_quotes() {
        let sb = Seedbox {
            media_root: "/home/u/media".into(),
            jellyfin_root: "/seedbox/media".into(),
            ..Seedbox::default()
        };
        let (p, d) = seedbox_path(
            &sb,
            "/seedbox/media/Anime/A/Season 1/A - S01E01.fr.default.ass",
        )
        .unwrap();
        assert_eq!(
            p,
            "/home/u/media/Anime/A/Season 1/A - S01E01.fr.default.ass"
        );
        assert_eq!(d, "Anime/A/Season 1");
        assert!(seedbox_path(&sb, "/media/tvshows/x.mkv").is_none());
        assert_eq!(sh_quote("l'été (2020)"), "'l'\"'\"'été (2020)'");
        assert_eq!(out_name("/x/a", "subrip", "full"), "/x/a.fr.srt");
        assert_eq!(out_name("/x/a", "ass", "forced"), "/x/a.fr.forced.ass");
    }
}
