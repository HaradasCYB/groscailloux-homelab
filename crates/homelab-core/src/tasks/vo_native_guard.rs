//! Garde du mode VO « Langue d'origine » (2026-10-08, lot 4 ; voir `crate::vo_native`).
//!
//! Avec `[accounts] vo_native = true`, un compte en mode VO ne passe en « Langue d'origine » que s'il ne lit que par des
//! clients sûrs (`vo_native_clients`). Toutes les `interval_secs`, cette tâche lit les sessions Jellyfin : un compte en
//! « Langue d'origine » qui a une session vidéo ouverte depuis un autre client (appli Android TV ou Fire TV, Chromecast,
//! client tiers) revient sur `vo_audio_language`, avant sa première lecture sur ce client — sinon le bogue de Jellyfin
//! 12.1 lui donnerait la VF là où il a le japonais aujourd'hui. La garde ne fait jamais l'inverse : repasser un compte
//! gardé en « Langue d'origine » se fait par `homelabctl accounts vo-native` (historique relu).
//!
//! Interrupteur coupé : aucun appel à Jellyfin. Sessions toutes sûres : une seule lecture (`/Sessions`). Dry-run respecté.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, VoNativeRecord};
use crate::vo_native::{self, ORIGINAL_LANGUAGE};

pub struct VoNativeGuard;

/// Résumé d'un passage sans rien à faire : toujours le même (passage « calme », journal en `debug`).
pub const IDLE: &str = "rien à garder";
pub const OFF: &str = "inactive ([accounts] vo_native = false)";

#[async_trait]
impl Task for VoNativeGuard {
    fn name(&self) -> &'static str {
        "vo_native_guard"
    }

    fn label(&self) -> &'static str {
        "Garde de la VO « Langue d'origine »"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.vo_native_guard.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let a = &ctx.cfg.accounts;
        if !a.vo_native {
            return Ok(Report::new(OFF, 0));
        }
        let sessions = ctx.jellyfin.sessions().await?;
        let any_unsafe = vo_native::video_session_clients(&sessions)
            .iter()
            .any(|(_, c)| !vo_native::is_safe(c, &a.vo_native_clients));
        if !any_unsafe {
            return Ok(Report::new(IDLE, 0));
        }
        let users = ctx.jellyfin.users().await?;
        let holds = vo_native::guard_holds(&sessions, &users, a);
        if holds.is_empty() {
            return Ok(Report::new(IDLE, 0));
        }
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for h in holds {
            let shown = format!("{} ({})", h.name, h.clients.join(", "));
            if ctx.dry_run {
                done.push(shown);
                continue;
            }
            match ctx
                .jellyfin
                .set_vo_audio(&h.user_id, &a.vo_audio_language)
                .await
            {
                Ok(()) => {
                    info!(task = "vo_native_guard", user = %h.name, clients = %h.clients.join(", "), audio = %a.vo_audio_language, "compte gardé : client hors liste");
                    let rec = VoNativeRecord {
                        at: now(),
                        name: h.name.clone(),
                        source: "garde".into(),
                        outcome: vo_native::Outcome::Held.as_str().into(),
                        old: ORIGINAL_LANGUAGE.into(),
                        new: a.vo_audio_language.clone(),
                        clients: h.clients.clone(),
                    };
                    if let Err(e) = vo_native::record(&ctx.state, &h.user_id, rec).await {
                        warn!(task = "vo_native_guard", user = %h.name, error = format!("{e:#}"), "garde faite mais non notée dans l'état");
                    }
                    done.push(shown);
                }
                Err(e) => {
                    warn!(task = "vo_native_guard", user = %h.name, error = format!("{e:#}"), "garde impossible");
                    failed.push(format!("{} : {e:#}", h.name));
                }
            }
        }
        let verb = if ctx.dry_run { "garderait" } else { "gardé" };
        let mut summary = format!(
            "{verb} sur {} : {}",
            a.vo_audio_language,
            if done.is_empty() {
                "—".to_string()
            } else {
                done.join(", ")
            }
        );
        if !failed.is_empty() {
            summary.push_str(&format!(" ; en échec : {}", failed.join(", ")));
            // un compte qui reste en « Langue d'origine » sur un client hors liste : le passage est un échec
            anyhow::bail!(summary);
        }
        let actions = if ctx.dry_run { 0 } else { done.len() as u32 };
        Ok(Report::new(summary, actions))
    }
}
