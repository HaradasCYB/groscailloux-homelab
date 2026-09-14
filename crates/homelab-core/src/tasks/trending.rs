//! Rangée « Tendances cette semaine » de l'accueil Jellyfin (toutes les 6 h).
//!
//! Classement depuis Playback Reporting : pour chaque titre (film, ou série pour ses épisodes),
//! nombre de spectateurs distincts puis heures vues ; un spectateur ne compte que s'il a regardé au
//! moins `min_minutes`. Fenêtre `days`, complétée par `fallback_days` si elle donne trop peu de titres.
//! La collection `collection_name` est créée si besoin puis alignée sur le classement (ajouts et
//! retraits ciblés). Collection et non playlist : une playlist qui reçoit une série y déplie tous
//! ses épisodes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::info;

use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct Trending;

/// Ligne de visionnage : (id de l'élément joué, utilisateur, secondes).
pub type Play = (String, String, i64);

#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub id: String,
    pub viewers: usize,
    pub seconds: i64,
}

/// `title_of` : élément joué → titre (lui-même pour un film, sa série pour un épisode).
pub fn rank(
    plays: &[Play],
    title_of: &HashMap<String, String>,
    min_seconds: i64,
    size: usize,
) -> Vec<Ranked> {
    // secondes par (titre, spectateur)
    let mut per: BTreeMap<(String, String), i64> = BTreeMap::new();
    for (item, user, secs) in plays {
        if let Some(title) = title_of.get(item) {
            *per.entry((title.clone(), user.clone())).or_default() += secs;
        }
    }
    let mut agg: BTreeMap<String, (BTreeSet<String>, i64)> = BTreeMap::new();
    for ((title, user), secs) in per {
        let e = agg.entry(title).or_default();
        e.1 += secs;
        if secs >= min_seconds {
            e.0.insert(user);
        }
    }
    let mut out: Vec<Ranked> = agg
        .into_iter()
        .filter(|(_, (v, _))| !v.is_empty())
        .map(|(id, (v, s))| Ranked {
            id,
            viewers: v.len(),
            seconds: s,
        })
        .collect();
    out.sort_by(|a, b| {
        b.viewers
            .cmp(&a.viewers)
            .then(b.seconds.cmp(&a.seconds))
            .then(a.id.cmp(&b.id))
    });
    out.truncate(size);
    out
}

/// Ajouts et retraits pour passer de `current` à `wanted`.
pub fn diff(current: &[String], wanted: &[String]) -> (Vec<String>, Vec<String>) {
    let add = wanted
        .iter()
        .filter(|w| !current.contains(w))
        .cloned()
        .collect();
    let remove = current
        .iter()
        .filter(|c| !wanted.contains(c))
        .cloned()
        .collect();
    (add, remove)
}

async fn plays_since(ctx: &TaskContext, days: i64) -> Result<Vec<Play>> {
    let sql = format!(
        "SELECT ItemId, UserId, SUM(PlayDuration) AS secs FROM PlaybackActivity \
         WHERE DateCreated > datetime('now', '-{days} day') GROUP BY ItemId, UserId"
    );
    let (cols, rows) = ctx.jellyfin.playback_query(&sql).await?;
    let idx = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
    let (i, u, s) = (
        idx("ItemId").context("colonne ItemId absente")?,
        idx("UserId").context("colonne UserId absente")?,
        idx("secs").context("colonne secs absente")?,
    );
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some((
                r.get(i)?.clone(),
                r.get(u)?.clone(),
                r.get(s)?.parse::<f64>().ok()? as i64,
            ))
        })
        .collect())
}

/// Élément joué → titre à mettre en avant ; les éléments disparus de la bibliothèque sont ignorés.
async fn titles_of(ctx: &TaskContext, plays: &[Play]) -> Result<HashMap<String, String>> {
    let ids: Vec<String> = plays
        .iter()
        .map(|p| p.0.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut out = HashMap::new();
    for it in ctx.jellyfin.items_by_ids(&ids).await? {
        let (Some(id), Some(kind)) = (
            it.get("Id").and_then(Value::as_str),
            it.get("Type").and_then(Value::as_str),
        ) else {
            continue;
        };
        let title = match kind {
            "Movie" | "Series" => Some(id.to_string()),
            "Episode" => it
                .get("SeriesId")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        if let Some(t) = title {
            out.insert(id.to_string(), t);
        }
    }
    Ok(out)
}

#[async_trait]
impl Task for Trending {
    fn name(&self) -> &'static str {
        "trending"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.trending.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.trending;
        let min = cfg.min_minutes * 60;
        let week = plays_since(ctx, cfg.days).await?;
        let mut titles = titles_of(ctx, &week).await?;
        let mut ranked = rank(&week, &titles, min, cfg.size);
        if ranked.len() < cfg.size {
            // semaine calme : on complète avec le mois, sans déplacer ce qui est déjà classé
            let month = plays_since(ctx, cfg.fallback_days).await?;
            titles.extend(titles_of(ctx, &month).await?);
            for r in rank(&month, &titles, min, cfg.size * 2) {
                if ranked.len() >= cfg.size {
                    break;
                }
                if !ranked.iter().any(|x| x.id == r.id) {
                    ranked.push(r);
                }
            }
        }
        let wanted: Vec<String> = ranked.iter().map(|r| r.id.clone()).collect();
        if wanted.is_empty() {
            return Ok(Report::new("aucun visionnage exploitable", 0));
        }
        // un admin pour lire la collection (les comptes ordinaires la voient via la bibliothèque Collections)
        let users = ctx.jellyfin.users().await?;
        let admin = users
            .iter()
            .find(|u| {
                u.pointer("/Policy/IsAdministrator")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .and_then(|u| u.get("Id").and_then(Value::as_str))
            .context("aucun compte admin")?
            .to_string();
        let name = &cfg.collection_name;
        let existing = ctx.jellyfin.find_collection(&admin, name).await?;
        let current = match &existing {
            Some(id) => ctx.jellyfin.collection_children(&admin, id).await?,
            None => Vec::new(),
        };
        let (add, remove) = diff(&current, &wanted);
        let summary = format!("{} titres (+{} −{})", wanted.len(), add.len(), remove.len());
        if ctx.dry_run {
            info!(
                task = "trending",
                ?ranked,
                "dry-run: collection not changed"
            );
            return Ok(Report::new(format!("dry-run {summary}"), 0));
        }
        match existing {
            None => {
                ctx.jellyfin.create_collection(name, &wanted).await?;
            }
            Some(id) => {
                ctx.jellyfin.collection_edit(&id, &remove, false).await?;
                ctx.jellyfin.collection_edit(&id, &add, true).await?;
            }
        }
        info!(task = "trending", collection = %name, titles = wanted.len(), added = add.len(), removed = remove.len(), "trending collection updated");
        Ok(Report::new(summary, (add.len() + remove.len()) as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(item: &str, user: &str, mins: i64) -> Play {
        (item.into(), user.into(), mins * 60)
    }

    #[test]
    fn episodes_roll_up_to_series_and_viewers_rank_first() {
        let titles: HashMap<String, String> = [
            ("e1", "bleach"),
            ("e2", "bleach"),
            ("m1", "dune"),
            ("m2", "f1"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let plays = vec![
            p("e1", "u1", 20),
            p("e2", "u2", 25),  // Bleach : 2 spectateurs
            p("m1", "u1", 150), // Dune : 1 spectateur, beaucoup d'heures
            p("m2", "u3", 3),   // F1 : clic par erreur (< 10 min)
        ];
        let r = rank(&plays, &titles, 600, 10);
        assert_eq!(
            r.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["bleach", "dune"]
        );
        assert_eq!(r[0].viewers, 2);
    }

    #[test]
    fn one_viewer_accumulates_across_episodes() {
        let titles: HashMap<String, String> = [("e1", "s"), ("e2", "s")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        // 6 + 6 min sur deux épisodes : 12 min sur la série, le spectateur compte
        let r = rank(&[p("e1", "u", 6), p("e2", "u", 6)], &titles, 600, 10);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn size_limit_and_unknown_items() {
        let titles: HashMap<String, String> = (0..15)
            .map(|i| (format!("m{i}"), format!("m{i}")))
            .collect();
        let mut plays: Vec<Play> = (0..15).map(|i| p(&format!("m{i}"), "u", 30 + i)).collect();
        plays.push(p("disparu", "u", 500));
        let r = rank(&plays, &titles, 600, 10);
        assert_eq!(r.len(), 10);
        assert_eq!(
            r[0].id, "m14",
            "à spectateurs égaux, le plus regardé d'abord"
        );
        assert!(!r.iter().any(|x| x.id == "disparu"));
    }

    #[test]
    fn diff_is_targeted() {
        let (add, rm) = diff(&["a".into(), "b".into()], &["b".into(), "c".into()]);
        assert_eq!(add, vec!["c".to_string()]);
        assert_eq!(rm, vec!["a".to_string()]);
    }
}
