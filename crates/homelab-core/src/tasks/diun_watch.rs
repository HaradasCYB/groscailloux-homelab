//! Surveillance de diun lui-même (revue Kaizen du 2026-10-07).
//!
//! Du 18/09 au 07/10, diun n'a surveillé aucune image : `diun/images.yml` contenait des clés en double, son
//! décodeur YAML refusait tout le fichier, et chaque matin « No image found » est parti dans un journal que
//! personne ne lit. `homelabctl check` fait désormais le contrôle (`diun::check`), mais à la main. Cette tâche le
//! refait une fois par jour et prévient l'admin (mail + Discord) tant que le fichier est invalide ou qu'une image
//! du compose n'a pas d'entrée — le défaut qui aurait signalé la panne dès le premier matin.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::alerts::{self, Level};
use crate::config::Config;
use crate::context::TaskContext;
use crate::diun::{self, Findings};

pub struct DiunWatch;

/// Objet et corps de l'alerte, `None` si le fichier est sain. Les avertissements (entrée sans image) n'alertent pas.
pub fn message(f: &Findings) -> Option<(String, String)> {
    if f.errors.is_empty() {
        return None;
    }
    let shown: Vec<String> = f.errors.iter().take(10).map(|e| format!("- {e}")).collect();
    let more = f.errors.len().saturating_sub(10);
    let body = format!(
        "diun/images.yml pose {} problème(s) :\n{}{}\n\n\
         Tant qu'une clé est en double, diun refuse tout le fichier et ne surveille plus aucune image (« No image \
         found » chaque matin à 08:00, aucune mise à jour signalée). diun relit le fichier à chaque passage : \
         corriger suffit, sans redémarrage. Contrôle à la main : `homelabctl check`.\n\n\
         Sonde quotidienne `diun_watch` : elle reprévient chaque jour tant que ça dure.",
        f.errors.len(),
        shown.join("\n"),
        if more > 0 {
            format!("\n… et {more} autre(s)")
        } else {
            String::new()
        }
    );
    Some((
        format!(
            "diun aveugle : images.yml à corriger ({} problème(s))",
            f.errors.len()
        ),
        body,
    ))
}

/// Identité du défaut (2026-10-08) : empreinte des erreurs relevées, dans l'ordre du fichier. Les avertissements n'y
/// entrent pas (ils n'alertent pas). Sert à `alerts::watch`.
pub fn defect_key(f: &Findings) -> String {
    alerts::fingerprint(&f.errors)
}

#[async_trait]
impl Task for DiunWatch {
    fn name(&self) -> &'static str {
        "diun_watch"
    }

    fn label(&self) -> &'static str {
        "Contrôle de diun"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.diun_watch.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let base = &ctx.cfg.paths.base;
        let images_path = base.join("diun/images.yml");
        let compose_path = base.join("docker-compose.yml");
        let images = std::fs::read_to_string(&images_path)
            .with_context(|| format!("lecture de {}", images_path.display()))?;
        let compose = std::fs::read_to_string(&compose_path)
            .with_context(|| format!("lecture de {}", compose_path.display()))?;
        let f = diun::check(&images, &compose);
        match message(&f) {
            None => {
                info!(
                    task = "diun_watch",
                    entries = f.entries.len(),
                    warnings = f.warnings.len(),
                    "ok"
                );
                // retour à la normale : une rechute alertera normalement
                alerts::watch_clear(ctx, self.name()).await;
                Ok(Report::new(
                    format!(
                        "{} entrée(s), YAML strict OK, {} avertissement(s)",
                        f.entries.len(),
                        f.warnings.len()
                    ),
                    0,
                ))
            }
            Some((subject, body)) => {
                warn!(
                    task = "diun_watch",
                    errors = f.errors.len(),
                    "diun/images.yml invalide"
                );
                // le passage a lieu aussi à chaque démarrage de homelabd : le même défaut (mêmes erreurs) n'est pas
                // repris avant 20 h ; une erreur de plus, de moins ou différente alerte aussitôt
                let sent = alerts::watch(
                    ctx,
                    self.name(),
                    &defect_key(&f),
                    Level::Error,
                    &subject,
                    &body,
                )
                .await;
                Ok(Report::new(
                    format!("images.yml INVALIDE : {} problème(s)", f.errors.len()),
                    u32::from(sent),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_file_says_nothing_even_with_warnings() {
        let f = Findings {
            warnings: vec!["ligne 4 : entrée inutilisée".into()],
            ..Default::default()
        };
        assert_eq!(message(&f), None);
    }

    #[test]
    fn errors_make_one_message_with_the_first_ten() {
        let f = Findings {
            errors: (1..=13)
                .map(|i| format!("ligne {i} : clé en double"))
                .collect(),
            ..Default::default()
        };
        let (subject, body) = message(&f).unwrap();
        assert!(subject.contains("13 problème(s)"), "{subject}");
        assert!(body.contains("- ligne 10 :") && !body.contains("- ligne 11 :"));
        assert!(body.contains("… et 3 autre(s)"));
        assert!(body.contains("sans redémarrage") && body.contains("homelabctl check"));
    }

    #[test]
    fn the_same_errors_make_the_same_key_and_other_errors_another() {
        let one = Findings {
            errors: vec!["ligne 4 : clé en double".into()],
            warnings: vec!["ligne 9 : entrée inutilisée".into()],
            ..Default::default()
        };
        // les avertissements n'entrent pas dans l'identité du défaut
        let same = Findings {
            errors: vec!["ligne 4 : clé en double".into()],
            ..Default::default()
        };
        assert_eq!(defect_key(&one), defect_key(&same));
        // une erreur de plus : défaut différent, alerté aussitôt
        let two = Findings {
            errors: vec![
                "ligne 4 : clé en double".into(),
                "ligne 12 : image sans digest".into(),
            ],
            ..Default::default()
        };
        assert_ne!(defect_key(&one), defect_key(&two));
    }

    #[test]
    fn the_september_breakage_is_what_the_task_would_have_caught() {
        let broken = "- name: a:1\n  watch_repo: true\n  watch_repo: true\n";
        let f = diun::check(broken, "    image: a:1@sha256:x\n");
        let (subject, body) = message(&f).expect("alerte dès le premier matin");
        assert!(subject.contains("1 problème"), "{subject}");
        assert!(body.contains("watch_repo"), "{body}");
    }
}
