//! Une boucle tokio par tâche : jitter initial, exécution bornée dans le temps,
//! puis attente de l'intervalle configuré (sémantique `OnUnitActiveSec`).
//!
//! **Battement de cœur** (2026-10-07) : chaque tour de boucle de n'importe quelle tâche note l'heure ; `/health`
//! répond 503 quand plus aucune boucle n'a tourné depuis [`stale_after`]. Avant, il répondait « ok » tant que le
//! serveur web vivait : un ordonnanceur figé (tâches bloquées, état verrouillé) restait invisible du chien de garde.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use homelab_core::alerts;
use homelab_core::state::{now, RunInfo};
use homelab_core::tasks::{registry, Task};
use homelab_core::TaskContext;
use rand::Rng;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// Dernier tour de boucle de l'une des tâches (secondes Unix) ; 0 : aucun ordonnanceur n'a démarré.
static BEAT: AtomicI64 = AtomicI64::new(0);
/// Silence toléré avant de déclarer l'ordonnanceur figé (secondes) ; 0 : rien n'est planifié, pas de contrôle.
static STALE_AFTER_SECS: AtomicI64 = AtomicI64::new(0);

fn beat() {
    BEAT.store(now(), Ordering::Relaxed);
}

/// Silence toléré : le plus long de deux passages complets au plafond (20 min) et de « intervalle de la tâche la plus
/// fréquente + un passage au plafond ». Un seul passage lent mais légitime (plafond de 10 min) ne doit jamais faire
/// répondre 503 : le chien de garde redémarrerait homelabd en boucle sans rien réparer, la panne externe restant en place.
pub fn stale_after(min_interval: Duration) -> Duration {
    (RUN_TIMEOUT * 2).max(min_interval + RUN_TIMEOUT)
}

/// État de l'ordonnanceur pour `/health`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Health {
    pub ok: bool,
    /// Secondes depuis le dernier tour de boucle ; `None` : aucun ordonnanceur à surveiller.
    pub age_secs: Option<i64>,
}

/// Décision pure : `last` = dernier battement (0 : jamais), `limit` = silence toléré (0 : pas de contrôle).
pub fn evaluate(last: i64, limit: i64, ts: i64) -> Health {
    if last <= 0 || limit <= 0 {
        return Health {
            ok: true,
            age_secs: None,
        };
    }
    let age = (ts - last).max(0);
    Health {
        ok: age <= limit,
        age_secs: Some(age),
    }
}

/// État actuel pour `/health`.
pub fn health() -> Health {
    evaluate(
        BEAT.load(Ordering::Relaxed),
        STALE_AFTER_SECS.load(Ordering::Relaxed),
        now(),
    )
}

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
    let mut min_interval: Option<Duration> = None;
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
        min_interval = Some(min_interval.map_or(interval, |m| m.min(interval)));
        handles.push(tokio::spawn(run_loop(ctx.clone(), task, interval)));
    }
    if let Some(m) = min_interval {
        // battement d'abord, délai ensuite : /health ne doit jamais voir un délai neuf avec un vieux battement
        beat();
        STALE_AFTER_SECS.store(stale_after(m).as_secs() as i64, Ordering::Relaxed);
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
        beat();
        let (c, t) = (ctx.clone(), task.clone());
        let panicked = contained(async move {
            run_once(&c, t.as_ref()).await;
        })
        .await;
        beat();
        if panicked {
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
    settle(
        ctx,
        name,
        false,
        "panique : passage abandonné (voir le journal)".into(),
        false,
    )
    .await;
}

/// Enregistre l'issue d'un passage (résumé, compteurs d'erreurs, dernière erreur, série d'échecs) puis prévient
/// l'admin quand une série d'échecs dépasse les seuils de `[alerts]` — une seule fois —, et quand la tâche repasse.
/// Avant le 2026-10-07 un échec n'était qu'un `warn!` et un compteur cumulé : une tâche cassée pendant des heures
/// passait inaperçue. `quiet` (passage réussi qui répète le précédent, voir [`is_quiet`]) : la tenue part avec la
/// sauvegarde suivante, au plus tard dans la minute ; un tel passage ne peut ni ouvrir ni clore une série d'échecs.
async fn settle(ctx: &TaskContext, name: &'static str, ok: bool, summary: String, quiet: bool) {
    let (min_failures, min_mins) = (ctx.cfg.alerts.fail_streak, ctx.cfg.alerts.fail_minutes);
    let (cap, window_secs) = (
        ctx.cfg.alerts.fail_alerts_max as usize,
        ctx.cfg.alerts.fail_alerts_window_mins * 60,
    );
    let ts = now();
    let result = ctx
        .state
        .update_lazy_if(quiet, |s| {
            let e = s.task_runs.get_mut(name)?;
            let back = e.record_outcome(ts, ok, &summary);
            // plafond commun (Jellyfin ou la seedbox tombe : plusieurs tâches échouent ensemble) : au-delà, la série
            // n'est PAS marquée signalée, elle repart au prochain échec. La place est prise ICI, dans la même mise à
            // jour que la décision (2026-10-08) : `alerts::admin` n'enregistre l'alerte qu'après l'envoi, et deux
            // tâches au seuil à quelques secondes d'écart passaient toutes deux. Rendue après l'envoi (voir plus bas).
            let (alert, deferred) = if ok || !e.streak_due(ts, min_failures, min_mins) {
                (None, false)
            } else if s
                .alerts
                .reserve_streak(alerts::STREAK_SUBJECT, ts, window_secs, cap)
            {
                (e.take_streak_alert(ts, min_failures, min_mins), false)
            } else {
                (None, true)
            };
            Some((back, alert, deferred))
        })
        .await;
    let (back, alert, deferred) = match result {
        Ok(Some(x)) => x,
        Ok(None) => return,
        Err(e) => {
            warn!(task = name, error = format!("{e:#}"), "état non enregistré");
            return;
        }
    };
    let label = homelab_core::tasks::label_of(name);
    if deferred {
        // debug : une tâche toutes les 20 s en échec en loguerait une par passage pendant toute la tempête
        debug!(
            task = name,
            cap,
            window_mins = ctx.cfg.alerts.fail_alerts_window_mins,
            "alerte d'échecs répétés différée : plafond atteint, nouvel essai au prochain échec"
        );
    }
    if let Some(a) = alert {
        let lasted = homelab_core::tasks::seedbox_health::human(ts - a.since);
        let body = format!(
            "La tâche « {label} » ({name}) échoue depuis {} passages de suite, depuis le {} ({lasted}).\n\n\
             Dernière erreur : {}\n\n\
             Voir `homelabctl status` et `journalctl -u homelabd | grep {name}`. Un message partira quand \
             la tâche repassera ; les erreurs isolées ne préviennent pas.",
            a.failures,
            homelab_core::state::short_date(a.since),
            a.last_error
        );
        let sent = alerts::admin(
            ctx,
            alerts::Level::Error,
            &format!("{} : {label}", alerts::STREAK_SUBJECT),
            &body,
        )
        .await;
        // l'envoi est fini : `alerts::admin` a enregistré l'alerte (livrée ou non), qui compte à la place de la
        // réservation — y compris sans canal configuré. Seul le dry-run n'enregistre rien : la place est rendue aussi.
        let _ = ctx.state.update_lazy(|s| s.alerts.release_streak(ts)).await;
        warn!(
            task = name,
            failures = a.failures,
            mailed = sent.0,
            posted = sent.1,
            "échecs répétés signalés"
        );
        if alerts::retry_later(sent, alerts::configured(ctx)) && !ctx.dry_run {
            // rien n'est parti alors qu'un canal existe : la série n'est pas « signalée », le prochain échec
            // réessaiera. Sans canal configuré on ne réessaie pas (un avertissement et une écriture d'état par passage).
            let _ = ctx
                .state
                .update(|s| {
                    if let Some(e) = s.task_runs.get_mut(name) {
                        e.streak_alerted = false;
                    }
                })
                .await;
        }
    }
    if let Some(r) = back {
        let lasted = homelab_core::tasks::seedbox_health::human(ts - r.since);
        alerts::admin(
            ctx,
            alerts::Level::Info,
            &format!("Tâche rétablie : {label}"),
            &format!(
                "La tâche « {label} » ({name}) repasse après {} échecs de suite ({lasted} depuis le {}).",
                r.failures,
                homelab_core::state::short_date(r.since)
            ),
        )
        .await;
    }
}

/// Un passage de la tâche. `false` : elle tournait déjà, rien n'a été lancé.
pub async fn run_once(ctx: &TaskContext, task: &dyn Task) -> bool {
    let name = task.name();
    let Some(_running) = Running::take(name) else {
        info!(task = name, "already running, pass skipped");
        return false;
    };
    // résumé du dernier passage réussi : un passage qui le répète sans rien faire n'a rien de neuf à dire
    let previous = ctx
        .state
        .read(|s| {
            s.task_runs
                .get(name)
                .filter(|e| e.last_ok == Some(true))
                .map(|e| e.last_summary.clone())
        })
        .await;
    let start = now();
    // simple tenue : écrite avec la sauvegarde suivante (au plus tard la fin du passage)
    let _ = ctx
        .state
        .update_lazy(|s| {
            let e = s.task_runs.entry(name.into()).or_insert_with(|| RunInfo {
                last_start: start,
                ..Default::default()
            });
            e.last_start = start;
            e.last_end = None;
            e.runs += 1;
        })
        .await;
    let began = Instant::now();
    let outcome = tokio::time::timeout(RUN_TIMEOUT, task.run(ctx)).await;
    // durée du passage : repérer une tâche qui ralentit bien avant le plafond de 600 s
    let ms = began.elapsed().as_millis() as u64;
    // seul un passage réussi peut être « calme » : un échec est toujours écrit tout de suite
    let mut quiet = false;
    let (ok, summary) = match outcome {
        Ok(Ok(rep)) => {
            // 88 % des lignes du journal étaient des passages sans action identiques au précédent (revue du
            // 2026-10-07) : ils passent en `debug`, tout changement reste en `info`
            quiet = is_quiet(rep.actions, &rep.summary, previous.as_deref());
            if quiet {
                debug!(task = name, actions = rep.actions, summary = %rep.summary, ms, "run_done");
            } else {
                info!(task = name, actions = rep.actions, summary = %rep.summary, ms, "run_done");
            }
            (true, rep.summary)
        }
        Ok(Err(e)) => {
            // `{:#}` : toute la chaîne de causes (« jellyfin Items: … operation timed out »), pas seulement le contexte
            warn!(task = name, error = format!("{e:#}"), ms, "run_failed");
            (false, format!("error: {e:#}"))
        }
        Err(_) => {
            warn!(
                task = name,
                timeout_secs = RUN_TIMEOUT.as_secs(),
                ms,
                "run_timeout"
            );
            (false, "timeout".into())
        }
    };
    settle(ctx, name, ok, summary, quiet).await;
    true
}

/// Passage réussi, sans action, dont le résumé répète celui du passage réussi précédent : journalisé en `debug`, et
/// sa tenue (`last_end`) n'est pas écrite sur disque à elle seule.
fn is_quiet(actions: u32, summary: &str, previous_ok_summary: Option<&str>) -> bool {
    actions == 0 && previous_ok_summary == Some(summary)
}

#[cfg(test)]
mod tests {
    use super::{contained, evaluate, is_quiet, stale_after, Duration, Health, RUN_TIMEOUT};

    #[test]
    fn the_silence_tolerated_never_goes_below_two_full_runs() {
        // tâche la plus fréquente : 20 s (playback_limit) → 20 min, jamais « 3 intervalles = 60 s »
        assert_eq!(stale_after(Duration::from_secs(20)), RUN_TIMEOUT * 2);
        assert_eq!(
            stale_after(Duration::from_secs(600)),
            Duration::from_secs(1200)
        );
        // seules des tâches rares planifiées : on attend un intervalle plus un passage au plafond
        assert_eq!(
            stale_after(Duration::from_secs(3600)),
            Duration::from_secs(3600 + 600)
        );
    }

    #[test]
    fn health_is_stale_only_after_the_limit_and_never_without_a_scheduler() {
        let ok = |age| Health {
            ok: true,
            age_secs: Some(age),
        };
        // un battement récent
        assert_eq!(evaluate(1_000, 1_200, 1_030), ok(30));
        // pile à la limite : encore bon ; une seconde de plus : figé
        assert_eq!(evaluate(1_000, 1_200, 2_200), ok(1_200));
        assert_eq!(
            evaluate(1_000, 1_200, 2_201),
            Health {
                ok: false,
                age_secs: Some(1_201)
            }
        );
        // jamais démarré (--no-web de test, tâches toutes désactivées) : rien à surveiller
        let none = Health {
            ok: true,
            age_secs: None,
        };
        assert_eq!(evaluate(0, 1_200, 99_999), none);
        assert_eq!(evaluate(1_000, 0, 99_999), none);
        // horloge qui recule : âge nul, pas un âge négatif
        assert_eq!(evaluate(1_000, 1_200, 900), ok(0));
    }

    #[tokio::test]
    async fn a_panicking_run_is_contained() {
        assert!(contained(async { panic!("passage qui plante") }).await);
        assert!(!contained(async {}).await);
    }

    #[test]
    fn only_a_repeated_idle_pass_is_quiet() {
        let idle = "rien à extraire (3 en attente de relance, dont 1 sans piste)";
        assert!(is_quiet(0, idle, Some(idle)));
        // une action, un résumé qui change, un premier passage ou un passage précédent en échec : `info`
        assert!(!is_quiet(1, idle, Some(idle)));
        assert!(!is_quiet(
            0,
            idle,
            Some("rien à extraire (2 en attente de relance, dont 1 sans piste)")
        ));
        assert!(!is_quiet(0, idle, None));
    }
}
