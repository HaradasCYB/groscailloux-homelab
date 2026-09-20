//! Cœur de l'automatisation homelab : configuration, clients d'API, tâches.
//!
//! Chaque module de `tasks` porte 1:1 un ancien script bash ; les clients
//! encapsulent les endpoints exacts utilisés. `TaskContext` fournit les
//! verrous partagés (qBittorrent, onboarding) et le mode dry-run global.

pub mod accounts;
pub mod alerts;
pub mod anime;
pub mod chat;
pub mod classify;
pub mod clients;
pub mod config;
pub mod context;
pub mod discord;
pub mod disk;
pub mod docker;
pub mod indexer;
pub mod mail;
pub mod manual_search;
pub mod matching;
pub mod secret;
pub mod state;
pub mod subscription_ops;
pub mod subscriptions;
pub mod tasks;
pub mod torrent_file;
pub mod welcome;

pub use config::{Config, Secrets};
pub use context::TaskContext;
pub use secret::Secret;
