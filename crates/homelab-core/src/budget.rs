//! Budget commun des requêtes à l'indexer (C411) : `series_search`, `movie_search` et la page
//! `/recherche` puisent dans le même compteur horaire glissant (`state.c411_queries`).
//!
//! Avant, chacun avait son plafond (12/h, 1/h, 6/h) sans voir les autres, alors que la limite est
//! celle de l'indexer (429 vers 50 requêtes/h, partagé avec les 4 Arrs) et celle de Prowlarr (30/h).
//! Les tâches de fond s'arrêtent `manual_reserve` requêtes avant le plafond, pour qu'une recherche
//! lancée à la main passe toujours.

use anyhow::Result;

use crate::context::TaskContext;
use crate::state::now;

/// Requêtes encore permises dans l'heure glissante.
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

/// Consomme une requête ; `false` si le plafond applicable est atteint.
pub async fn take(ctx: &TaskContext, manual: bool) -> Result<bool> {
    let cfg = &ctx.cfg.indexers;
    let cap = ceiling(cfg.c411_max_per_hour, cfg.manual_reserve, manual);
    let t = now();
    ctx.state
        .update(|s| {
            s.c411_queries.retain(|q| t - *q < 3600);
            if left(&s.c411_queries, t, cap) > 0 {
                s.c411_queries.push(t);
                true
            } else {
                false
            }
        })
        .await
}

/// Requêtes restantes pour l'affichage (page de statut, résumés).
pub async fn remaining(ctx: &TaskContext, manual: bool) -> usize {
    let cfg = &ctx.cfg.indexers;
    let cap = ceiling(cfg.c411_max_per_hour, cfg.manual_reserve, manual);
    let t = now();
    ctx.state.read(|s| left(&s.c411_queries, t, cap)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hourly_window_and_reserve() {
        let now = 100_000;
        assert_eq!(left(&[], now, 20), 20);
        assert_eq!(
            left(&[now - 10, now - 3599, now - 3600, now - 7200], now, 20),
            18
        );
        assert_eq!(left(&[now; 25], now, 20), 0);
        // les tâches de fond s'arrêtent avant, la page garde la réserve
        assert_eq!(ceiling(20, 6, false), 14);
        assert_eq!(ceiling(20, 6, true), 20);
        assert_eq!(ceiling(4, 6, false), 0);
    }
}
