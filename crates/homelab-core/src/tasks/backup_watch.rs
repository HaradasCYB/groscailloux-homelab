//! Fraîcheur de la sauvegarde d'état (revue Kaizen du 2026-10-07).
//!
//! La sauvegarde (`homelab-backup.timer`, dimanche 04:30) tourne en root hors de homelabd : ni `/status.html` ni
//! `stack_health` n'en regardaient l'âge, et rien ne prévenait si le minuteur ne tournait plus. Un échec franc de
//! l'unité prévient par `OnFailure=homelab-alert@%n.service` ; cette tâche couvre le reste (minuteur désactivé,
//! unité jamais lancée, archive supprimée). Une fois par jour : la dernière archive `homelab-state-*.tar.zst` de
//! `paths.backups` a-t-elle plus de `max_age_days` jours ? Alerte admin, répétée chaque jour tant que ça dure.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::now;

pub struct BackupWatch;

/// Verdict sur l'âge de la dernière archive.
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh {
        age_days: i64,
    },
    Stale {
        age_days: i64,
    },
    /// Aucune archive dans le dossier.
    Missing,
}

/// `newest` : date de modification de la plus récente archive (secondes Unix), `None` s'il n'y en a aucune.
pub fn freshness(newest: Option<i64>, ts: i64, max_age_days: i64) -> Freshness {
    match newest {
        None => Freshness::Missing,
        Some(t) => {
            let age_days = (ts - t).max(0) / 86_400;
            if (ts - t) > max_age_days * 86_400 {
                Freshness::Stale { age_days }
            } else {
                Freshness::Fresh { age_days }
            }
        }
    }
}

impl Freshness {
    /// Identité du défaut pour `alerts::watch` (2026-10-08) : « trop ancienne » ou « aucune archive », sans l'âge qui
    /// grandit chaque jour. `None` : tout va bien.
    pub fn defect_key(&self) -> Option<&'static str> {
        match self {
            Freshness::Fresh { .. } => None,
            Freshness::Stale { .. } => Some("stale"),
            Freshness::Missing => Some("missing"),
        }
    }
}

/// Une archive d'état de `backup.rs` : `homelab-state-AAAAMMJJ-HHMMSS.tar.zst` (pas son `.sha256` ni son `.list.gz`).
pub fn is_archive(name: &str) -> bool {
    name.starts_with("homelab-state-") && name.ends_with(".tar.zst")
}

/// Plus récente archive : (nom, date de modification en secondes Unix).
pub fn newest_archive(dir: &Path) -> std::io::Result<Option<(String, i64)>> {
    let mut best: Option<(String, i64)> = None;
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !is_archive(&name) {
            continue;
        }
        let Some(mtime) = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, t)| mtime > *t) {
            best = Some((name, mtime));
        }
    }
    Ok(best)
}

#[async_trait]
impl Task for BackupWatch {
    fn name(&self) -> &'static str {
        "backup_watch"
    }

    fn label(&self) -> &'static str {
        "Fraîcheur des sauvegardes"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.backup_watch.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.backup_watch;
        let dir = &ctx.cfg.paths.backups;
        let ts = now();
        let newest = match newest_archive(dir) {
            Ok(n) => n,
            Err(e) => {
                // dossier illisible : ce n'est pas « pas de sauvegarde », c'est une erreur de la tâche
                return Err(anyhow::anyhow!("lecture de {} : {e}", dir.display()));
            }
        };
        let verdict = freshness(newest.as_ref().map(|(_, t)| *t), ts, cfg.max_age_days);
        // identité du défaut pour `alerts::watch` (vide si tout va bien : jamais utilisée dans ce cas)
        let key = verdict.defect_key().unwrap_or_default();
        match verdict {
            Freshness::Fresh { age_days } => {
                let name = newest.map(|(n, _)| n).unwrap_or_default();
                info!(task = "backup_watch", age_days, %name, "ok");
                // retour à la normale : une rechute alertera normalement
                alerts::watch_clear(ctx, self.name()).await;
                Ok(Report::new(
                    format!("dernière archive : {name} ({age_days} j)"),
                    0,
                ))
            }
            Freshness::Stale { age_days } => {
                let name = newest.map(|(n, _)| n).unwrap_or_default();
                warn!(task = "backup_watch", age_days, %name, "sauvegarde trop ancienne");
                // le passage a lieu aussi à chaque démarrage de homelabd : le même défaut n'est pas repris avant 20 h
                let sent = alerts::watch(
                    ctx,
                    self.name(),
                    key,
                    Level::Error,
                    &format!("Sauvegarde : dernière archive vieille de {age_days} jours"),
                    &format!(
                        "La dernière archive d'état ({name}) date de {age_days} jours ; la sauvegarde est \
                         hebdomadaire (seuil d'alerte : {} jours). Le minuteur ne tourne plus ou l'unité échoue en \
                         silence : `systemctl list-timers homelab-backup.timer`, \
                         `systemctl status homelab-backup.service`, `journalctl -u homelab-backup -n 50`. \
                         Relance à la main : `sudo systemctl start homelab-backup.service`.\n\n\
                         Sonde quotidienne `backup_watch` : elle reprévient chaque jour tant que ça dure.",
                        cfg.max_age_days
                    ),
                )
                .await;
                Ok(Report::new(
                    format!("TROP ANCIENNE : {name} ({age_days} j)"),
                    u32::from(sent),
                ))
            }
            Freshness::Missing => {
                warn!(task = "backup_watch", dir = %dir.display(), "aucune archive");
                let sent = alerts::watch(
                    ctx,
                    self.name(),
                    key,
                    Level::Error,
                    "Sauvegarde : aucune archive d'état",
                    &format!(
                        "Aucune archive homelab-state-*.tar.zst dans {}. La sauvegarde n'a jamais tourné ou \
                         les archives ont été supprimées : `systemctl status homelab-backup.service`, \
                         `sudo systemctl start homelab-backup.service`.\n\n\
                         Sonde quotidienne `backup_watch` : elle reprévient chaque jour tant que ça dure.",
                        dir.display()
                    ),
                )
                .await;
                Ok(Report::new("AUCUNE ARCHIVE", u32::from(sent)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn a_weekly_backup_is_fresh_until_8_days() {
        let now = 100 * DAY;
        // dimanche dernier : 3 jours
        assert_eq!(
            freshness(Some(now - 3 * DAY), now, 8),
            Freshness::Fresh { age_days: 3 }
        );
        // pile 8 jours : pas encore
        assert_eq!(
            freshness(Some(now - 8 * DAY), now, 8),
            Freshness::Fresh { age_days: 8 }
        );
        // un passage manqué : 8 jours et une seconde
        assert_eq!(
            freshness(Some(now - 8 * DAY - 1), now, 8),
            Freshness::Stale { age_days: 8 }
        );
        assert_eq!(
            freshness(Some(now - 15 * DAY), now, 8),
            Freshness::Stale { age_days: 15 }
        );
    }

    #[test]
    fn no_archive_is_its_own_verdict_and_the_future_is_clamped() {
        assert_eq!(freshness(None, 1000, 8), Freshness::Missing);
        // horloge qui a reculé : âge 0, jamais négatif
        assert_eq!(
            freshness(Some(2000), 1000, 8),
            Freshness::Fresh { age_days: 0 }
        );
    }

    #[test]
    fn the_defect_key_does_not_move_with_the_age() {
        let now = 100 * DAY;
        let key = |t| freshness(t, now, 8).defect_key();
        // 9 puis 10 jours : même défaut, donc pas de nouvelle alerte à un redémarrage
        assert_eq!(key(Some(now - 9 * DAY)), Some("stale"));
        assert_eq!(key(Some(now - 10 * DAY)), Some("stale"));
        // plus d'archive du tout : défaut différent, alerté aussitôt
        assert_eq!(key(None), Some("missing"));
        // sauvegarde à jour : rien (la mémoire de l'alerte est effacée, une rechute alertera)
        assert_eq!(key(Some(now - 2 * DAY)), None);
    }

    #[test]
    fn only_state_archives_count() {
        assert!(is_archive("homelab-state-20261004-043844.tar.zst"));
        assert!(!is_archive("homelab-state-20261004-043844.tar.zst.sha256"));
        assert!(!is_archive("homelab-state-20261004-043844.tar.zst.list.gz"));
        assert!(!is_archive("guacdb-20261004-043844.sql.gz"));
        assert!(!is_archive("systemd-20261004-043844.tar.gz"));
    }

    #[test]
    fn the_newest_archive_is_found_by_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let touch = |name: &str, secs_ago: u64| {
            let p = dir.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            let t = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        assert_eq!(newest_archive(dir.path()).unwrap(), None);
        touch("homelab-state-20260927-043610.tar.zst", 10 * 86_400);
        touch("homelab-state-20261004-043844.tar.zst", 3 * 86_400);
        // plus récents mais pas des archives d'état : ignorés
        touch("homelab-state-20261004-043844.tar.zst.sha256", 60);
        touch("guacdb-20261004-043844.sql.gz", 60);
        let (name, mtime) = newest_archive(dir.path()).unwrap().unwrap();
        assert_eq!(name, "homelab-state-20261004-043844.tar.zst");
        let age = crate::state::now() - mtime;
        assert!((3 * 86_400 - 5..=3 * 86_400 + 5).contains(&age), "{age}");
        assert!(newest_archive(&dir.path().join("absent")).is_err());
    }
}
