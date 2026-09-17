//! Contrôle des identifications de Jellyfin (toutes les 30 min).
//!
//! Jellyfin identifie un dossier **d'après son nom** : un titre proche de celui d'un spin-off part sur la
//! mauvaise fiche (le 2026-09-16 *Attack on Titan* → *Junior High School*, le 17 *That Time I Got Reincarnated
//! as a Slime* → *Slime Diaries* et *The Walking Dead* → *Dead City*). Sonarr et Radarr, eux, connaissent le
//! bon identifiant : c'est la référence.
//!
//! Pour chaque fiche des Arrs, on compare son identifiant à celui de l'élément Jellyfin qui porte le même
//! chemin. En cas d'écart : `RemoteSearch` avec le bon identifiant, `Apply`, puis rafraîchissement complet.
//! Au plus `max_fixes_per_run` corrections par passage, jamais pendant une lecture du titre, `dry_run` respecté.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::anime_library::is_playing;
use super::deletion_cleanup::{map_path, side_maps};
use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;

pub struct IdentityCheck;

/// Ce que l'Arr sait d'un titre : chemin vu par Jellyfin et identifiants de référence.
#[derive(Debug, Clone, PartialEq)]
pub struct Expected {
    pub name: String,
    pub jellyfin_path: String,
    pub tvdb: i64,
    pub tmdb: i64,
    pub movie: bool,
}

/// Identifiant d'un élément Jellyfin (`ProviderIds`), en nombre.
pub fn provider_id(item: &Value, key: &str) -> i64 {
    item.pointer(&format!("/ProviderIds/{key}"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

/// L'élément Jellyfin porte-t-il la mauvaise fiche ? (Identifiant connu des deux côtés et différent.)
pub fn mismatched(item: &Value, exp: &Expected) -> bool {
    let (tvdb, tmdb) = (provider_id(item, "Tvdb"), provider_id(item, "Tmdb"));
    if exp.movie {
        exp.tmdb > 0 && tmdb > 0 && tmdb != exp.tmdb
    } else {
        // une série a presque toujours les deux : un seul écart suffit à trancher
        (exp.tvdb > 0 && tvdb > 0 && tvdb != exp.tvdb)
            || (exp.tmdb > 0 && tmdb > 0 && tmdb != exp.tmdb)
    }
}

/// Chemin d'un élément Jellyfin ramené au dossier du titre (un film pointe sur son fichier).
pub fn item_folder(item: &Value) -> Option<String> {
    let path = item.get("Path").and_then(Value::as_str)?;
    if item.get("Type").and_then(Value::as_str) == Some("Movie") {
        return path.rsplit_once('/').map(|(dir, _)| dir.to_string());
    }
    Some(path.to_string())
}

async fn expected_titles(ctx: &TaskContext) -> Vec<Expected> {
    let mut out = Vec::new();
    for side in ctx.sides() {
        let maps = side_maps(ctx, side.name);
        for (list, movie) in [
            (side.sonarr.series().await.unwrap_or_default(), false),
            (side.radarr.movies().await.unwrap_or_default(), true),
        ] {
            for v in list {
                let Some(path) = v.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let Some((_, jellyfin_path)) = map_path(&maps, path) else {
                    continue;
                };
                out.push(Expected {
                    name: v
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    jellyfin_path,
                    tvdb: v.get("tvdbId").and_then(Value::as_i64).unwrap_or(0),
                    tmdb: v.get("tmdbId").and_then(Value::as_i64).unwrap_or(0),
                    movie,
                });
            }
        }
    }
    out
}

#[async_trait]
impl Task for IdentityCheck {
    fn name(&self) -> &'static str {
        "identity_check"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.identity_check.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.identity_check;
        let expected = expected_titles(ctx).await;
        if expected.is_empty() {
            return Ok(Report::new("aucune fiche lisible", 0));
        }
        let items = ctx.jellyfin.titles_with_ids().await?;
        let by_path: HashMap<String, &Value> = items
            .iter()
            .filter_map(|i| item_folder(i).map(|p| (p, i)))
            .collect();
        let playing = ctx.jellyfin.playing_paths().await.unwrap_or_default();
        let mut fixed = 0u32;
        let mut names = Vec::new();
        for exp in &expected {
            let Some(item) = by_path.get(&exp.jellyfin_path) else {
                continue;
            };
            if !mismatched(item, exp) {
                continue;
            }
            let wrong = item.get("Name").and_then(Value::as_str).unwrap_or("?");
            if is_playing(&exp.jellyfin_path, &playing) {
                info!(task = "identity_check", title = %exp.name, "en lecture : corrigé au prochain passage");
                continue;
            }
            warn!(task = "identity_check", title = %exp.name, wrong = %wrong, tvdb = exp.tvdb, tmdb = exp.tmdb, "jellyfin a la mauvaise fiche");
            if ctx.dry_run {
                names.push(format!("{} (vu « {wrong} »)", exp.name));
                fixed += 1;
                continue;
            }
            let id = item.get("Id").and_then(Value::as_str).unwrap_or_default();
            let kind = if exp.movie { "Movie" } else { "Series" };
            let ids = if exp.movie {
                json!({ "Tmdb": exp.tmdb.to_string() })
            } else {
                json!({ "Tvdb": exp.tvdb.to_string(), "Tmdb": exp.tmdb.to_string() })
            };
            let found = ctx.jellyfin.remote_search(kind, id, ids).await?;
            let good = found.iter().find(|c| {
                (exp.tvdb > 0 && provider_id(c, "Tvdb") == exp.tvdb)
                    || (exp.tmdb > 0 && provider_id(c, "Tmdb") == exp.tmdb)
            });
            let Some(good) = good else {
                warn!(task = "identity_check", title = %exp.name, "aucune fiche proposée avec le bon identifiant");
                continue;
            };
            match ctx.jellyfin.apply_remote(id, good).await {
                Ok(()) => {
                    info!(task = "identity_check", title = %exp.name, "fiche corrigée");
                    names.push(exp.name.clone());
                    fixed += 1;
                }
                Err(e) => {
                    warn!(task = "identity_check", title = %exp.name, error = %e, "correction impossible")
                }
            }
            if fixed as usize >= cfg.max_fixes_per_run {
                break;
            }
        }
        let summary = if names.is_empty() {
            format!("{} titre(s) vérifié(s), rien à corriger", expected.len())
        } else {
            format!("corrigé(s) : {}", names.join(" ; "))
        };
        Ok(Report::new(summary, fixed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(movie: bool, tvdb: i64, tmdb: i64) -> Expected {
        Expected {
            name: "The Walking Dead".into(),
            jellyfin_path: "/seedbox/media/TV Shows/The Walking Dead".into(),
            tvdb,
            tmdb,
            movie,
        }
    }

    #[test]
    fn spinoff_is_detected() {
        // cas réel : le dossier de la série est parti sur Dead City
        let wrong = json!({"Name": "The Walking Dead : Dead City", "ProviderIds": {"Tvdb": "417549", "Tmdb": "194583"}});
        assert!(mismatched(&wrong, &exp(false, 153021, 1402)));
        let good =
            json!({"Name": "The Walking Dead", "ProviderIds": {"Tvdb": "153021", "Tmdb": "1402"}});
        assert!(!mismatched(&good, &exp(false, 153021, 1402)));
        // identifiant absent d'un côté : on ne conclut pas
        assert!(!mismatched(
            &json!({"ProviderIds": {}}),
            &exp(false, 153021, 1402)
        ));
        assert!(!mismatched(&good, &exp(false, 0, 0)));
        // film : seul TMDB compte
        let movie = json!({"Type": "Movie", "ProviderIds": {"Tmdb": "999"}});
        assert!(mismatched(&movie, &exp(true, 0, 1402)));
        assert!(!mismatched(
            &json!({"ProviderIds": {"Tvdb": "417549"}}),
            &exp(true, 0, 1402)
        ));
    }

    #[test]
    fn movie_paths_fold_to_their_folder() {
        let m = json!({"Type": "Movie", "Path": "/media/movies/Marnie (2014)/Marnie.mkv"});
        assert_eq!(item_folder(&m).unwrap(), "/media/movies/Marnie (2014)");
        let s = json!({"Type": "Series", "Path": "/media/tvshows/Bleach"});
        assert_eq!(item_folder(&s).unwrap(), "/media/tvshows/Bleach");
        assert!(item_folder(&json!({"Type": "Series"})).is_none());
    }
}
