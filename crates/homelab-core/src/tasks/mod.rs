//! Une tâche = un ancien script bash. `registry()` liste celles que le scheduler
//! planifie ; `find()` sert à `homelabctl run <nom>`.

pub mod anime_library;
pub mod auto_import;
pub mod backup;
pub mod cleanup;
pub mod deletion_cleanup;
pub mod disk_pressure;
pub mod hls_loop_watch;
pub mod id_match_import;
pub mod identity_check;
pub mod indexer_unblock;
pub mod monitor_sync;
pub mod movie_search;
pub mod onboard;
pub mod playback_canary;
pub mod playback_limit;
pub mod seedbox_refresh;
pub mod series_search;
pub mod stack_health;
pub mod stuck_handler;
pub mod subscription_cycle;
pub mod subscription_reconcile;
pub mod subtitle_sync;
pub mod tba_bypass;
pub mod torrent_import;
pub mod tracker_ratio;
pub mod trending;
pub mod user_poller;
pub mod vpn;

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::Config;
use crate::context::TaskContext;

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub summary: String,
    pub actions: u32,
}

impl Report {
    pub fn new(summary: impl Into<String>, actions: u32) -> Self {
        Self {
            summary: summary.into(),
            actions,
        }
    }
}

#[async_trait]
pub trait Task: Send + Sync {
    fn name(&self) -> &'static str;
    /// Nom affiché sur `/status.html` et dans le panneau Automatisation de Homarr. Obligatoire, sans valeur
    /// par défaut : jusqu'au 2026-09-23, une liste tenue à la main dans `status_page.rs` affichait « Tâche »
    /// pour les six tâches ajoutées après elle.
    fn label(&self) -> &'static str;
    fn interval(&self, cfg: &Config) -> Duration;
    async fn run(&self, ctx: &TaskContext) -> Result<Report>;
}

pub fn registry() -> Vec<Box<dyn Task>> {
    vec![
        Box::new(stack_health::StackHealth),
        Box::new(seedbox_refresh::SeedboxRefresh),
        Box::new(tracker_ratio::TrackerRatio),
        Box::new(stuck_handler::StuckHandler),
        Box::new(disk_pressure::DiskPressure),
        Box::new(hls_loop_watch::HlsLoopWatch),
        Box::new(tba_bypass::TbaBypass),
        Box::new(id_match_import::IdMatchImport),
        Box::new(torrent_import::TorrentImport),
        Box::new(series_search::SeriesSearch),
        Box::new(movie_search::MovieSearch),
        Box::new(indexer_unblock::IndexerUnblock),
        Box::new(anime_library::AnimeLibrary),
        Box::new(identity_check::IdentityCheck),
        Box::new(deletion_cleanup::DeletionCleanup),
        Box::new(trending::Trending),
        Box::new(playback_limit::PlaybackLimit),
        Box::new(playback_canary::PlaybackCanary),
        Box::new(subtitle_sync::SubtitleSync),
        Box::new(subscription_cycle::SubscriptionCycle),
        Box::new(subscription_reconcile::SubscriptionReconcile),
        Box::new(monitor_sync::MonitorSync),
        Box::new(user_poller::UserPoller),
        Box::new(cleanup::Cleanup),
    ]
}

pub fn find(name: &str) -> Option<Box<dyn Task>> {
    let wanted = name.replace('-', "_");
    registry().into_iter().find(|t| t.name() == wanted)
}

pub fn names() -> Vec<&'static str> {
    registry().iter().map(|t| t.name()).collect()
}

/// Nom affiché d'une tâche, à partir de son identifiant ; l'identifiant lui-même si la tâche est inconnue.
pub fn label_of(name: &str) -> String {
    registry()
        .iter()
        .find(|t| t.name() == name)
        .map(|t| t.label().to_string())
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn every_task_has_its_own_label() {
        let tasks = registry();
        let mut seen = std::collections::HashSet::new();
        for t in &tasks {
            let l = t.label().trim();
            assert!(!l.is_empty(), "{} n'a pas de nom affiché", t.name());
            assert_ne!(l, "Tâche", "{} garde le nom générique", t.name());
            assert!(seen.insert(l.to_string()), "nom affiché en double : {l}");
        }
        assert_eq!(label_of("playback_canary"), "Test de lecture");
        assert_eq!(label_of("inconnue"), "inconnue");
    }
}
