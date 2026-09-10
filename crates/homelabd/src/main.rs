//! `homelabd` — daemon d'automatisation : scheduler des tâches périodiques,
//! watcher du dossier de téléchargement, UI web d'onboarding.

mod scheduler;
mod watcher;
mod web;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use homelab_core::{Config, Secrets, TaskContext};
use tracing::{error, info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(version, about = "Homelab automation daemon")]
struct Args {
    #[arg(long, default_value = "/opt/homelab/homelab.toml")]
    config: PathBuf,
    /// Fichier .env (secrets). Ignoré si absent : les variables d'environnement suffisent.
    #[arg(long, default_value = "/opt/homelab/.env")]
    env_file: PathBuf,
    /// Aucune écriture : les tâches loggent ce qu'elles feraient. (aussi HOMELABD_DRY_RUN=1)
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    no_web: bool,
    #[arg(long)]
    no_watcher: bool,
}

/// Sous systemd, stdout part dans le journal : format compact sans couleur ni
/// horodatage (journald les fournit), champs inline pour rester greppable.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let under_systemd = std::env::var_os("JOURNAL_STREAM").is_some();
    let layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(!under_systemd);
    if under_systemd {
        tracing_subscriber::registry()
            .with(filter)
            .with(layer.without_time().compact())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .init();
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing();
    let dry_run = args.dry_run || env_flag("HOMELABD_DRY_RUN");

    let cfg = Config::load(&args.config)?;
    let secrets = Secrets::load(Some(&args.env_file)).context("chargement des secrets")?;
    let ctx = Arc::new(TaskContext::new(cfg, secrets, dry_run)?);
    info!(version = env!("CARGO_PKG_VERSION"), dry_run, config = %args.config.display(), "homelabd starting");
    if dry_run {
        warn!("DRY-RUN : aucune écriture ne sera faite");
    }

    let mut handles = scheduler::spawn_all(ctx.clone());

    if !args.no_watcher && ctx.cfg.auto_import.enabled && ctx.cfg.task_enabled("auto_import") {
        let c = ctx.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = watcher::run(c).await {
                error!(error = %e, "watcher stopped");
            }
        }));
    } else {
        info!("auto_import watcher disabled");
    }

    if !args.no_web {
        let c = ctx.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = web::serve(c).await {
                error!(error = %e, "web server stopped");
            }
        }));
    }

    shutdown_signal().await;
    info!("shutdown requested");
    for h in handles {
        h.abort();
    }
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => {},
        _ = int.recv() => {},
    }
}
