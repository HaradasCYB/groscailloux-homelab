//! Une boucle tokio par tâche : jitter initial, exécution bornée dans le temps,
//! puis attente de l'intervalle configuré (sémantique `OnUnitActiveSec`).

use std::sync::Arc;
use std::time::Duration;

use homelab_core::state::{now, RunInfo};
use homelab_core::tasks::{registry, Task};
use homelab_core::TaskContext;
use rand::Rng;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// Tâches en cours : une tâche lancée à la main (`homelabctl run` → `POST /admin/run`) ne double pas le passage
/// planifié, et inversement.
static RUNNING: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

struct Running(&'static str);

impl Running {
    fn take(name: &'static str) -> Option<Self> {
        let mut r = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if r.contains(&name) {
            return None;
        }
        r.push(name);
        Some(Self(name))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut r = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        r.retain(|n| *n != self.0);
    }
}
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
    let task: Arc<dyn Task> = Arc::from(task);
    let name = task.name();
    loop {
        let (c, t) = (ctx.clone(), task.clone());
        if contained(async move {
            run_once(&c, t.as_ref()).await;
        })
        .await
        {
            // Jusqu'au 2026-09-23, une panique tuait la boucle : la tâche ne repassait plus jamais,
            // le service restait « actif » et systemd ne relançait rien.
            error!(
                task = name,
                "run_panicked: passage abandonné, la tâche repassera à l'intervalle suivant"
            );
            record_panic(&ctx, name).await;
        }
        tokio::time::sleep(interval).await;
    }
}

/// Résultat d'un passage demandé à la main (`POST /admin/run`, utilisé par `homelabctl run`).
pub enum RunNow {
    Unknown,
    Busy,
    Panicked,
    Done { ok: bool, summary: String },
}

/// Lance tout de suite un passage de la tâche `name` dans le daemon (état enregistré comme un passage planifié).
pub async fn run_now(ctx: Arc<TaskContext>, name: &str) -> RunNow {
    let Some(task) = registry().into_iter().find(|t| t.name() == name) else {
        return RunNow::Unknown;
    };
    let task: Arc<dyn Task> = Arc::from(task);
    let tname = task.name();
    let (c, t) = (ctx.clone(), task.clone());
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = started.clone();
    if contained(async move {
        flag.store(
            run_once(&c, t.as_ref()).await,
            std::sync::atomic::Ordering::SeqCst,
        );
    })
    .await
    {
        record_panic(&ctx, tname).await;
        return RunNow::Panicked;
    }
    if !started.load(std::sync::atomic::Ordering::SeqCst) {
        return RunNow::Busy;
    }
    let info = ctx.state.read(|s| s.task_runs.get(tname).cloned()).await;
    RunNow::Done {
        ok: info.as_ref().and_then(|i| i.last_ok).unwrap_or(false),
        summary: info.map(|i| i.last_summary).unwrap_or_default(),
    }
}

/// Exécute un passage dans sa propre tâche tokio : une panique y reste confinée. `true` si elle a paniqué.
async fn contained<F>(fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    matches!(tokio::spawn(fut).await, Err(e) if e.is_panic())
}

async fn record_panic(ctx: &TaskContext, name: &'static str) {
    let _ = ctx
        .state
        .update(|s| {
            if let Some(e) = s.task_runs.get_mut(name) {
                e.last_end = Some(now());
                e.last_ok = Some(false);
                e.last_summary = "panique : passage abandonné (voir le journal)".into();
                e.errors += 1;
            }
        })
        .await;
}

/// Un passage de la tâche. `false` : elle tournait déjà, rien n'a été lancé.
pub async fn run_once(ctx: &TaskContext, task: &dyn Task) -> bool {
    let name = task.name();
    let Some(_running) = Running::take(name) else {
        info!(task = name, "already running, pass skipped");
        return false;
    };
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
            // `{:#}` : toute la chaîne de causes (« jellyfin Items: … operation timed out »), pas seulement le contexte
            warn!(task = name, error = format!("{e:#}"), "run_failed");
            (false, format!("error: {e:#}"))
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
    true
}

#[cfg(test)]
mod tests {
    use super::contained;

    #[tokio::test]
    async fn a_panicking_run_is_contained() {
        assert!(contained(async { panic!("passage qui plante") }).await);
        assert!(!contained(async {}).await);
    }
}
