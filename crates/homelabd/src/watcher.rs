//! Surveillance inotify de `paths.downloads` (non récursive, comme l'ancien
//! `inotifywait -e create,close_write,moved_to`). Chaque nom est traité au plus
//! une fois par fenêtre de 2 min : create + close_write ne déclenchent qu'un scan.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use homelab_core::tasks::auto_import;
use homelab_core::TaskContext;
use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{info, warn};

const DEDUPE_WINDOW: Duration = Duration::from_secs(120);

pub async fn run(ctx: Arc<TaskContext>) -> Result<()> {
    let dir = ctx.cfg.paths.downloads.clone();
    auto_import::ensure_watch_dir(&dir)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| match res {
            Ok(ev) => {
                let _ = tx.send(ev);
            }
            Err(e) => warn!(error = %e, "inotify error"),
        },
        notify::Config::default(),
    )
    .context("création du watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch {}", dir.display()))?;
    info!(dir = %dir.display(), "auto_import watcher started");

    let mut recent: HashMap<String, Instant> = HashMap::new();
    while let Some(ev) = rx.recv().await {
        let interesting = matches!(
            ev.kind,
            EventKind::Create(CreateKind::File)
                | EventKind::Create(CreateKind::Any)
                | EventKind::Modify(ModifyKind::Name(RenameMode::To))
                | EventKind::Modify(ModifyKind::Name(RenameMode::Any))
                | EventKind::Access(AccessKind::Close(AccessMode::Write))
        );
        if !interesting {
            continue;
        }
        for path in ev.paths {
            if path.parent() != Some(dir.as_path()) {
                continue;
            }
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let now = Instant::now();
            recent.retain(|_, t| now.duration_since(*t) < DEDUPE_WINDOW);
            if recent.contains_key(&name) {
                continue;
            }
            recent.insert(name.clone(), now);
            let c = ctx.clone();
            tokio::spawn(async move {
                if let Err(e) = auto_import::handle_new_entry(&c, &name).await {
                    warn!(task = "auto_import", file = %name, error = %e, "handling failed");
                }
            });
        }
    }
    Ok(())
}
