//! Accès à l'indexer, partagé par les tâches et la page `/recherche`.
//!
//! Deux clés C411 sont déclarées dans Prowlarr (« C411 » et « C411 (2) ») : chacune a son propre compteur
//! horaire (`[indexers] c411_max_per_hour`, **par clé**) et son propre temps mort. Une clé qui répond 429 est
//! mise de côté `cooldown_after_429_mins` et la requête repart aussitôt sur l'autre : tant qu'une clé répond,
//! rien ne s'arrête. Les tâches de fond laissent `manual_reserve` requêtes à la page.

use anyhow::{bail, Result};
use serde_json::Value;

use crate::clients::ProwlarrClient;
use crate::context::TaskContext;
use crate::state::now;

/// Une clé utilisable : son indexer dans Prowlarr.
#[derive(Debug, Clone)]
pub struct Key {
    pub name: String,
    pub id: i64,
}

/// Requêtes encore permises pour une clé dans l'heure glissante.
pub fn left(times: &[i64], now: i64, max: usize) -> usize {
    max.saturating_sub(times.iter().filter(|t| now - **t < 3600).count())
}

/// Plafond applicable : les tâches de fond laissent la réserve à la page.
pub fn ceiling(max: usize, reserve: usize, manual: bool) -> usize {
    if manual {
        max
    } else {
        max.saturating_sub(reserve)
    }
}

/// Réponse de l'indexer signalant une limite atteinte (429, « request limit »).
pub fn is_limit(err: &anyhow::Error) -> bool {
    let s = format!("{err:#}").to_ascii_lowercase();
    s.contains("429") || s.contains("too many requests") || s.contains("request limit")
}

/// Choisit une clé et consomme une requête : la moins chargée, hors temps mort, sous son plafond.
pub async fn take_key(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    manual: bool,
) -> Result<Option<Key>> {
    let cfg = &ctx.cfg.indexers;
    let prefix = ctx.cfg.manual_search.c411_indexer.to_ascii_lowercase();
    let mut keys: Vec<Key> = prow
        .indexers()
        .await?
        .into_iter()
        .filter_map(|i| {
            let name = i.get("name").and_then(Value::as_str)?.to_string();
            let id = i.get("id").and_then(Value::as_i64)?;
            name.to_ascii_lowercase()
                .starts_with(&prefix)
                .then_some(Key { name, id })
        })
        .collect();
    if keys.is_empty() {
        bail!("aucun indexer « {} » dans Prowlarr", prefix);
    }
    keys.sort_by(|a, b| a.name.cmp(&b.name));
    let cap = ceiling(cfg.c411_max_per_hour, cfg.manual_reserve, manual);
    let t = now();
    ctx.state
        .update(|s| {
            s.c411_cooldowns.retain(|_, until| *until > t);
            let mut best: Option<(usize, Key)> = None;
            for k in &keys {
                if s.c411_cooldowns.contains_key(&k.name) {
                    continue;
                }
                let used = s.c411_key_queries.get(&k.name).cloned().unwrap_or_default();
                let free = left(&used, t, cap);
                if free > 0 && best.as_ref().is_none_or(|(b, _)| free > *b) {
                    best = Some((free, k.clone()));
                }
            }
            best.map(|(_, k)| {
                let q = s.c411_key_queries.entry(k.name.clone()).or_default();
                q.retain(|x| t - *x < 3600);
                q.push(t);
                k
            })
        })
        .await
}

/// Met une clé de côté après un 429.
pub async fn mark_limited(ctx: &TaskContext, name: &str) {
    let until = now() + ctx.cfg.indexers.cooldown_after_429_mins * 60;
    let _ = ctx
        .state
        .update(|s| s.c411_cooldowns.insert(name.to_string(), until))
        .await;
    tracing::warn!(
        indexer = name,
        "limite atteinte : clé mise de côté, bascule sur l'autre"
    );
}

/// Recherche par identifiant TMDB, avec choix de clé et bascule sur 429. `None` = plus de budget.
pub async fn search_tmdb(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    tmdb: i64,
    season: Option<i64>,
    manual: bool,
) -> Result<Option<Vec<Value>>> {
    run(ctx, prow, manual, |id| {
        prow.search_by_tmdb(tmdb, season, id)
    })
    .await
}

/// Recherche en texte libre, mêmes règles.
pub async fn search_text(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    query: &str,
    limit: u32,
    manual: bool,
) -> Result<Option<Vec<Value>>> {
    run(ctx, prow, manual, |id| prow.search(query, id, limit)).await
}

async fn run<F, Fut>(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    manual: bool,
    call: F,
) -> Result<Option<Vec<Value>>>
where
    F: Fn(i64) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Value>>>,
{
    // au plus deux essais : une clé, puis l'autre si la première a pris un 429
    for _ in 0..2 {
        let Some(key) = take_key(ctx, prow, manual).await? else {
            return Ok(None);
        };
        match call(key.id).await {
            Ok(v) => return Ok(Some(v)),
            Err(e) if is_limit(&e) => mark_limited(ctx, &key.name).await,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_reserve_and_limit_detection() {
        let now = 100_000;
        assert_eq!(left(&[], now, 40), 40);
        assert_eq!(left(&[now - 10, now - 3599, now - 3600], now, 40), 38);
        assert_eq!(left(&[now; 50], now, 40), 0);
        assert_eq!(ceiling(40, 10, false), 30);
        assert_eq!(ceiling(40, 10, true), 40);
        assert!(is_limit(&anyhow::anyhow!("HTTP 429 Too Many Requests")));
        assert!(is_limit(&anyhow::anyhow!(
            "API Request Limit reached for C411"
        )));
        assert!(!is_limit(&anyhow::anyhow!("connection refused")));
    }
}
