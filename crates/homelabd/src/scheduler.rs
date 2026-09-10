//! Une boucle tokio par tâche : jitter initial, exécution bornée dans le temps,
//! puis attente de l'intervalle configuré (sémantique `OnUnitActiveSec`).

use std::sync::Arc;
use std::time::Duration;

use homelab_core::state::{now, RunInfo};
use homelab_core::tasks::{registry, Task};
use homelab_core::TaskContext;
use rand::Rng;
use tokio::task::JoinHandle;
use tracing::{info, warn};

const RUN_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_JITTER: Duration = Duration::from_secs(30);

pub fn spawn_all(ctx: Arc<TaskContext>) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();
    for task in registry() {
        if !ctx.cfg.task_enabled(task.name()) {
            info!(task = task.name(), "disabled in config, not scheduled");
            continue;
        }
        let interval = task.interval(&ctx.cfg);
        if interval.is_zero() {
            info!(task = task.name(), "interval 0, not scheduled");
            continue;
        }
        info!(
            task = task.name(),
            interval_secs = interval.as_secs(),
            "scheduled"
        );
        handles.push(tokio::spawn(run_loop(ctx.clone(), task, interval)));
    }
    handles
}

async fn run_loop(ctx: Arc<TaskContext>, task: Box<dyn Task>, interval: Duration) {
    let jitter =
        Duration::from_millis(rand::thread_rng().gen_range(0..MAX_JITTER.as_millis() as u64));
    tokio::time::sleep(jitter).await;
    loop {
        run_once(&ctx, task.as_ref()).await;
        tokio::time::sleep(interval).await;
    }
}

pub async fn run_once(ctx: &TaskContext, task: &dyn Task) {
    let name = task.name();
    let start = now();
    let _ = ctx
        .state
        .update(|s| {
            let e = s.task_runs.entry(name.into()).or_insert(RunInfo {
                last_start: start,
                last_end: None,
                last_ok: None,
                last_summary: String::new(),
                runs: 0,
                errors: 0,
            });
            e.last_start = start;
            e.last_end = None;
            e.runs += 1;
        })
        .await;
    let outcome = tokio::time::timeout(RUN_TIMEOUT, task.run(ctx)).await;
    let (ok, summary) = match outcome {
        Ok(Ok(rep)) => {
            info!(task = name, actions = rep.actions, summary = %rep.summary, "run_done");
            (true, rep.summary)
        }
        Ok(Err(e)) => {
            warn!(task = name, error = %e, "run_failed");
            (false, format!("error: {e}"))
        }
        Err(_) => {
            warn!(
                task = name,
                timeout_secs = RUN_TIMEOUT.as_secs(),
                "run_timeout"
            );
            (false, "timeout".into())
        }
    };
    let _ = ctx
        .state
        .update(|s| {
            if let Some(e) = s.task_runs.get_mut(name) {
                e.last_end = Some(now());
                e.last_ok = Some(ok);
                e.last_summary = summary;
                if !ok {
                    e.errors += 1;
                }
            }
        })
        .await;
}
