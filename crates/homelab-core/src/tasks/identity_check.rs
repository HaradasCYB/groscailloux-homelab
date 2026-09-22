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
//!
//! Même passage, deuxième défaut (2026-09-22) : une fiche peut porter le **nom de la release** au lieu de son
//! titre (« Matrix.Reloaded.2003.MULTi.VFF.1080p… »), parce que les groupes écrivent leur nom dans la métadonnée
//! `title` du fichier et que les bibliothèques étaient en `EnableEmbeddedTitles`. Sur une télé (Fire TV, Android
//! TV) c'est la moitié de l'écran, l'appli native n'ayant que les titres et les affiches à montrer. L'option est
//! désormais à `false`, mais une fiche déjà créée garde ce nom : **seule une ré-identification le remplace**
//! (un `Refresh`, même complet avec `ReplaceAllMetadata`, ne suffit pas — mesuré). Une fiche n'est réessayée
//! qu'une fois par mois (`state.renamed_items`).

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

/// Marqueurs qu'on ne trouve que dans un nom de release.
const RELEASE_TOKENS: [&str; 34] = [
    "1080p", "720p", "2160p", "480p", "540p", "4klight", "hdlight", "webrip", "web-dl", "webdl",
    "bluray", "brrip", "dvdrip", "hdtv", "x264", "x265", "h264", "h265", "hevc", "xvid", "aac",
    "ac3", "eac3", "dts", "ddp", "10bit", "multi", "vostfr", "vff", "vfi", "vf2", "vfq", "remux",
    "proper",
];

/// Le nom affiché est-il un nom de release plutôt qu'un titre ? (Découpage sur les séparateurs des noms de
/// fichiers ; il faut un marqueur entier, pour ne pas confondre avec un vrai titre.)
pub fn looks_like_release(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| RELEASE_TOKENS.contains(&t))
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
        let tried = ctx.state.read(|s| s.renamed_items.clone()).await;
        let now = crate::state::now();
        let mut fixed = 0u32;
        let mut names = Vec::new();
        for exp in &expected {
            let Some(item) = by_path.get(&exp.jellyfin_path) else {
                continue;
            };
            let wrong = item.get("Name").and_then(Value::as_str).unwrap_or("?");
            let id = item.get("Id").and_then(Value::as_str).unwrap_or_default();
            let bad_id = mismatched(item, exp);
            // nom de release : une seule tentative par mois, la ré-identification peut rendre le même nom
            let bad_name = !bad_id
                && cfg.fix_release_names
                && looks_like_release(wrong)
                && tried
                    .get(id)
                    .map(|at| now - at > 30 * 86_400)
                    .unwrap_or(true);
            if !bad_id && !bad_name {
                continue;
            }
            if is_playing(&exp.jellyfin_path, &playing) {
                info!(task = "identity_check", title = %exp.name, "en lecture : corrigé au prochain passage");
                continue;
            }
            if bad_id {
                warn!(task = "identity_check", title = %exp.name, wrong = %wrong, tvdb = exp.tvdb, tmdb = exp.tmdb, "jellyfin a la mauvaise fiche");
            } else {
                info!(task = "identity_check", title = %exp.name, wrong = %wrong, "fiche affichée sous un nom de release");
            }
            if ctx.dry_run {
                names.push(format!("{} (vu « {wrong} »)", exp.name));
                fixed += 1;
                if fixed as usize >= cfg.max_fixes_per_run {
                    break;
                }
                continue;
            }
            if bad_name {
                let (key, t) = (id.to_string(), now);
                ctx.state
                    .update(|s| {
                        s.renamed_items.insert(key, t);
                    })
                    .await?;
            }
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
    fn release_names_are_recognised() {
        for n in [
            "Matrix.Reloaded.2003.MULTi.VFF.1080p.10bit.BluRay.x265.DDP.5.1",
            "A Quiet Place - 2018 - BluRay Rip 1080p - PARISTOCAT",
            "8 Mile (2002) [1080p] MULTi BluRay x264-PopHD",
            "Saturn.3.1980.MULTi.1080p.x265.BluRay.AC3-Se12",
            "Caterina.Va.En.Ville.2003.VOSTFR.540p.WEBRip.E-AC-3.5.1.x264-LOLOPC",
            "Tom.Clancys.Without.Remorse.2021.MULTi.2160p.HDR.WEB",
        ] {
            assert!(looks_like_release(n), "{n}");
        }
        for n in [
            "Saturn 3",
            "Le Parrain 2",
            "Blade Runner 2049",
            "Re:ZERO -Starting Life in Another World- Director's Cut",
            "Mr. Robot",
            "The Walking Dead",
            "8 Mile",
            "Bleach",
            "L'Attaque des Titans",
        ] {
            assert!(!looks_like_release(n), "{n}");
        }
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
