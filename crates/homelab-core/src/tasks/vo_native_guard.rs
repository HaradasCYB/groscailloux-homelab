//! Garde du mode VO « Langue d'origine » (2026-10-08, lot 4 ; voir `crate::vo_native`).
//!
//! Avec `[accounts] vo_native = true`, un compte en mode VO ne passe en « Langue d'origine » que si aucun client hors de
//! `vo_native_clients` n'apparaît pour lui (lectures, sessions, appareils). Toutes les `interval_secs`, cette tâche relit
//! les comptes, puis, s'il y en a en « Langue d'origine », les sessions et les appareils Jellyfin : un compte en « Langue
//! d'origine » qui a une session ouverte ou un appareil enregistré sur un autre client (appli Android TV ou Fire TV,
//! Chromecast, client tiers) revient sur `vo_audio_language` — sinon le bogue de Jellyfin 12.1 lui donnerait la VF là où
//! il a le japonais aujourd'hui. Toute session compte, même sans capacités déclarées : l'appli Android TV reprise après
//! un redémarrage de Jellyfin a `PlayableMediaTypes = []` tant qu'elle ne lit rien. Le retour se fait en général avant
//! la première lecture sur ce client ; sinon dès cette lecture (sondage toutes les `interval_secs`) : la première
//! lecture d'un appareil connecté moins d'un intervalle plus tôt peut encore partir en VF sur un titre touché.
//! La garde ne fait jamais l'inverse : repasser un compte gardé en « Langue d'origine » se fait par
//! `homelabctl accounts vo-native` (clients relus).
//!
//! Interrupteur coupé : aucun appel à Jellyfin. Aucun compte en « Langue d'origine » : une seule lecture (`/Users`).
//! Appareils illisibles : les sessions suffisent pour ce passage, qui finit en échec (alerte s'il dure). Dry-run
//! respecté.

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
        // comptes surveillés d'abord : sans compte en « Langue d'origine », rien d'autre à lire
        let users = ctx.jellyfin.users().await?;
        if !users.iter().any(|u| vo_native::is_native_vo(u, a)) {
            return Ok(Report::new(IDLE, 0));
        }
        let sessions = ctx.jellyfin.sessions().await?;
        let mut failed = Vec::new();
        let devices = match ctx.jellyfin.devices().await {
            Ok(d) => d,
            Err(e) => {
                warn!(
                    task = "vo_native_guard",
                    error = format!("{e:#}"),
                    "appareils illisibles : sessions seules pour ce passage"
                );
                failed.push(format!("appareils illisibles : {e:#}"));
                Vec::new()
            }
        };
        let holds = vo_native::guard_holds(&sessions, &devices, &users, a);
        if holds.is_empty() && failed.is_empty() {
            return Ok(Report::new(IDLE, 0));
        }
        let mut done = Vec::new();
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
