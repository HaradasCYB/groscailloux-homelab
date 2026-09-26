//! Rangement de l'animation japonaise dans les bibliothèques « Anime » (séries) et « Films d'animation »
//! (films), sur le VPS et la seedbox.
//!
//! Pour chaque fiche Sonarr/Radarr, le classement vient de TMDB (`crate::anime::classify`, fiche lue par
//! Jellyseerr et mise en cache `recheck_days`), jamais du type « anime » de Sonarr, qui n'est pas fiable. Un
//! tag posé à la main l'emporte : `anime` ⇒ rangé, `pas-anime` ⇒ jamais déplacé.
//!
//! - Anime hors du dossier anime : tag `anime` ajouté et fiche déplacée par l'éditeur de l'Arr
//!   (`rootFolderPath` + `moveFiles`, renommage sur le même disque : hardlinks et torrents intacts). Au plus
//!   `max_moves_per_run` par passage ; une fiche avec un téléchargement en cours attend.
//! - Ensuite : fin des commandes de déplacement attendue, cache rclone rafraîchi (seedbox) et Jellyfin prévenu
//!   pour l'ancien et le nouveau chemin. Le déplacement est noté (`anime_moves`) : `deletion_cleanup` ignore
//!   ces fiches pendant `MOVE_GRACE_SECS`. Puis **analyse complète** de la médiathèque (`scan_after_move`) :
//!   sans elle, un titre déplacé d'une bibliothèque seedbox à une autre n'apparaît qu'à l'analyse de 05 h (ni
//!   `Library/Media/Updated` ni le rafraîchissement du dossier ne créent la fiche ; Your Name, 2026-09-26).
//!   Au plus une toutes les `scan_min_gap_mins` ; un déplacement pendant l'attente est rattrapé au passage suivant.
//! - Pas anime mais rangé dans le dossier anime : jamais ressorti automatiquement, seulement signalé.
//! - Fiche TMDB introuvable : rien n'est fait, signalé.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::deletion_cleanup::{map_path, rclone_refresh, side_maps};
use super::{Report, Task};
use crate::anime::{apply_not_russian, classify, manual_override, Class, TAG_ANIME, TAG_RUSSIAN};
use crate::clients::ArrClient;
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, AnimeClassRecord};

pub struct AnimeLibrary;

/// Analyse complète à lancer (un déplacement l'attend) et heure de la dernière lancée par cette tâche.
static SCAN_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_SCAN: AtomicI64 = AtomicI64::new(0);

/// Demande une analyse complète au prochain passage (un import dans une bibliothèque seedbox ne suffit pas toujours
/// à faire apparaître de nouveaux épisodes : *Кухня*, 2026-09-26).
pub fn request_library_scan() {
    SCAN_PENDING.store(true, Ordering::SeqCst);
}

/// Lancer l'analyse maintenant ? Seulement si un déplacement l'attend et que la précédente date d'au moins `gap_mins`.
pub fn scan_due(pending: bool, now: i64, last: i64, gap_mins: i64) -> bool {
    pending && now - last >= gap_mins * 60
}

/// Après un déplacement, `deletion_cleanup` laisse la fiche tranquille pendant ce temps.
pub const MOVE_GRACE_SECS: i64 = 6 * 3600;

/// Ce qu'il faut faire d'une fiche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Anime (ou russe) hors de son dossier : à déplacer.
    Move,
    /// Pas anime mais dans le dossier anime : signalé seulement.
    Misplaced,
    /// Classement TMDB impossible : signalé seulement.
    Unknown,
    /// Déjà au bon endroit.
    Keep,
}

/// Un fichier de la fiche (dossier `jf_dir`, chemin Jellyfin) est en cours de lecture.
pub fn is_playing(jf_dir: &str, playing: &[String]) -> bool {
    playing.iter().any(|p| in_root(p, jf_dir))
}

/// `path` est dans `root` (ou est `root`).
pub fn in_root(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Décision pour une fiche : tag manuel prioritaire, sinon classement TMDB. `in_ru_root` : `None` quand ce côté
/// n'a pas de dossier russe (VPS, ou `russian = false`) — le russe y est alors traité comme le reste.
pub fn decide(
    tmdb_class: Class,
    labels: &[String],
    in_anime_root: bool,
    in_ru_root: Option<bool>,
) -> Decision {
    let class = apply_not_russian(manual_override(labels).unwrap_or(tmdb_class), labels);
    let class = match (class, in_ru_root) {
        (Class::Russian, None) => Class::NotAnime,
        (c, _) => c,
    };
    match class {
        Class::Anime if !in_anime_root => Decision::Move,
        Class::Russian if in_ru_root == Some(false) => Decision::Move,
        Class::NotAnime if in_anime_root => Decision::Misplaced,
        Class::Unknown if !in_anime_root => Decision::Unknown,
        _ => Decision::Keep,
    }
}

/// La voie russe est un choix, pas un classement : `Russian` seulement si la fiche est dans le dossier russe ou porte
/// le tag `russe` ; sinon une fiche russe par TMDB reste dans Séries/Films. Un anime reste anime.
pub fn russian_choice(class: Class, labels: &[String], in_ru_root: Option<bool>) -> Class {
    let tagged = labels.iter().any(|l| l.eq_ignore_ascii_case(TAG_RUSSIAN));
    match class {
        Class::Anime => Class::Anime,
        _ if in_ru_root == Some(true) || (tagged && in_ru_root.is_some()) => Class::Russian,
        Class::Russian => Class::NotAnime,
        c => c,
    }
}

/// Fiche de la **voie russe** (dans un dossier russe de la seedbox) : cherchée par l'Arr sur RuTracker, jamais par
/// `series_search` / `movie_search` sur C411 (sinon une VF de C411 remplacerait le choix du membre).
pub fn russian_route(item: &Value, cfg: &crate::config::AnimeLibrary) -> bool {
    cfg.russian
        && item.get("path").and_then(Value::as_str).is_some_and(|p| {
            in_root(p, &cfg.seedbox_ru_series_root) || in_root(p, &cfg.seedbox_ru_movies_root)
        })
}

/// Relancer la recherche d'une fiche russe sans fichier ? Jamais cherchée, ou dernière recherche assez ancienne.
pub fn search_due(last: Option<i64>, now: i64, retry_hours: i64) -> bool {
    last.is_none_or(|t| now - t >= retry_hours * 3600)
}

pub fn class_name(c: Class) -> &'static str {
    match c {
        Class::Anime => "anime",
        Class::Russian => "russian",
        Class::NotAnime => "not_anime",
        Class::Unknown => "unknown",
    }
}

fn class_from(name: &str) -> Class {
    match name {
        "anime" => Class::Anime,
        "russian" => Class::Russian,
        "not_anime" => Class::NotAnime,
        _ => Class::Unknown,
    }
}

/// Classement en cache encore valable : `recheck_days` jours, un jour pour une fiche introuvable.
pub fn cached(rec: Option<&AnimeClassRecord>, now: i64, recheck_days: i64) -> Option<Class> {
    let rec = rec?;
    let class = class_from(&rec.class);
    let days = if class == Class::Unknown {
        1
    } else {
        recheck_days
    };
    (now - rec.at < days * 86_400).then_some(class)
}

/// Fiche à traiter ce passage.
#[derive(Debug, Clone)]
struct Item {
    id: i64,
    tmdb: i64,
    title: String,
    path: String,
    tags: Vec<i64>,
    /// Séries : `standard`, `anime` ou `daily`.
    series_type: Option<String>,
    /// Fiche surveillée à qui il manque des fichiers (film sans fichier, épisodes diffusés manquants).
    missing: bool,
}

fn items(list: &[Value]) -> Vec<Item> {
    list.iter()
        .filter_map(|v| {
            Some(Item {
                id: v.get("id").and_then(Value::as_i64)?,
                tmdb: v.get("tmdbId").and_then(Value::as_i64).unwrap_or(0),
                title: v
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
                path: v.get("path").and_then(Value::as_str)?.to_string(),
                tags: v
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default(),
                series_type: v
                    .get("seriesType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                missing: v.get("monitored").and_then(Value::as_bool) == Some(true)
                    && match v.get("statistics") {
                        // série : épisodes diffusés et suivis sans fichier
                        Some(st) if v.get("seriesType").is_some() => {
                            let n = |k: &str| st.get(k).and_then(Value::as_i64).unwrap_or(0);
                            n("episodeCount") > n("episodeFileCount")
                        }
                        _ => v.get("hasFile").and_then(Value::as_bool) == Some(false),
                    },
            })
        })
        .collect()
}

/// Une application et ses réglages pour ce passage.
struct Place<'a> {
    side: &'static str,
    arr: &'a ArrClient,
    movies: bool,
    root: String,
    /// Dossier russe de ce côté (seedbox seulement, `russian = true`).
    ru_root: Option<String>,
}

async fn tag_map(arr: &ArrClient) -> Result<HashMap<i64, String>> {
    let v = arr.get("api/v3/tag", &[]).await?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some((
                        t.get("id").and_then(Value::as_i64)?,
                        t.get("label").and_then(Value::as_str)?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
}

async fn tag_id(arr: &ArrClient, tags: &HashMap<i64, String>, label: &str) -> Result<i64> {
    if let Some((id, _)) = tags.iter().find(|(_, l)| l.eq_ignore_ascii_case(label)) {
        return Ok(*id);
    }
    let v = arr.post("api/v3/tag", &json!({ "label": label })).await?;
    v.get("id")
        .and_then(Value::as_i64)
        .with_context(|| format!("tag {label} non créé"))
}

async fn has_root_folder(arr: &ArrClient, root: &str) -> Result<bool> {
    let v = arr.get("api/v3/rootfolder", &[]).await?;
    Ok(v.as_array().is_some_and(|a| {
        a.iter().any(|r| {
            r.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| p.trim_end_matches('/') == root.trim_end_matches('/'))
        })
    }))
}

/// Classement TMDB, depuis le cache ou Jellyseerr (compte dans `lookups`).
async fn tmdb_class(
    ctx: &TaskContext,
    movies: bool,
    tmdb: i64,
    lookups: &mut usize,
) -> Option<Class> {
    let cfg = &ctx.cfg.tasks.anime_library;
    if tmdb <= 0 {
        return Some(Class::Unknown);
    }
    let key = format!("{}:{tmdb}", if movies { "movie" } else { "tv" });
    let t = now();
    if let Some(c) = ctx
        .state
        .read(|s| cached(s.anime_class.get(&key), t, cfg.recheck_days))
        .await
    {
        return Some(c);
    }
    // l'essai à blanc lit tout : il sert à valider la liste complète
    if !ctx.dry_run && *lookups >= cfg.max_lookups_per_run {
        return None;
    }
    *lookups += 1;
    let details = if movies {
        ctx.jellyseerr.movie_details(tmdb).await
    } else {
        ctx.jellyseerr.tv_details(tmdb).await
    };
    let class = match details {
        Ok(d) => classify(&d),
        Err(e) => {
            warn!(task = "anime_library", tmdb, error = %e, "tmdb details unavailable");
            Class::Unknown
        }
    };
    let rec = AnimeClassRecord {
        at: t,
        class: class_name(class).into(),
    };
    if !ctx.dry_run {
        let _ = ctx.state.update(|s| s.anime_class.insert(key, rec)).await;
    }
    Some(class)
}

/// Attend la fin des commandes de déplacement de l'Arr (4 min au plus : un passage est coupé à 10 min).
async fn wait_moves(arr: &ArrClient) {
    for _ in 0..24 {
        let running = match arr.get("api/v3/command", &[]).await {
            Ok(v) => v.as_array().is_some_and(|a| {
                a.iter().any(|c| {
                    let name = c.get("name").and_then(Value::as_str).unwrap_or("");
                    let status = c.get("status").and_then(Value::as_str).unwrap_or("");
                    name.contains("Move") && matches!(status, "queued" | "started")
                })
            }),
            Err(_) => true,
        };
        if !running {
            return;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    warn!(
        task = "anime_library",
        service = arr.name,
        "move commands still running after 4 min"
    );
}

/// Déplace une fiche : tag (`anime` ou `russe`) + nouveau dossier racine, fichiers compris. Une série animée
/// passe aussi en type « anime » : sans ça, Sonarr ne comprend pas la numérotation absolue (« Bleach - 367 »)
/// et les imports tombent à côté.
async fn move_item(
    arr: &ArrClient,
    movies: bool,
    anime: bool,
    id: i64,
    tag: i64,
    root: &str,
) -> Result<()> {
    let (path, ids) = if movies {
        ("api/v3/movie/editor", "movieIds")
    } else {
        ("api/v3/series/editor", "seriesIds")
    };
    let mut body = json!({
        ids: [id],
        "tags": [tag],
        "applyTags": "add",
        "rootFolderPath": root,
        "moveFiles": true,
    });
    if !movies && anime {
        body["seriesType"] = json!("anime");
    }
    arr.put(path, &body).await?;
    Ok(())
}

/// Une série déjà rangée dans Anime mais restée en type « standard » : on corrige, puis on relit ses
/// fichiers (le changement de type change l'analyse des noms).
async fn fix_series_type(arr: &ArrClient, id: i64) -> Result<()> {
    arr.put(
        "api/v3/series/editor",
        &json!({ "seriesIds": [id], "seriesType": "anime" }),
    )
    .await?;
    arr.command(json!({ "name": "RescanSeries", "seriesId": id }))
        .await?;
    Ok(())
}

#[async_trait]
impl Task for AnimeLibrary {
    fn name(&self) -> &'static str {
        "anime_library"
    }

    fn label(&self) -> &'static str {
        "Rangement des animés et du russe"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.anime_library.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.anime_library;
        let mut places = vec![
            Place {
                side: "vps",
                arr: &ctx.sonarr,
                movies: false,
                root: cfg.vps_series_root.clone(),
                ru_root: None,
            },
            Place {
                side: "vps",
                arr: &ctx.radarr,
                movies: true,
                root: cfg.vps_movies_root.clone(),
                ru_root: None,
            },
        ];
        if let Some(a) = &ctx.seedbox_sonarr {
            places.push(Place {
                side: "seedbox",
                arr: a,
                movies: false,
                root: cfg.seedbox_series_root.clone(),
                ru_root: cfg.russian.then(|| cfg.seedbox_ru_series_root.clone()),
            });
        }
        if let Some(a) = &ctx.seedbox_radarr {
            places.push(Place {
                side: "seedbox",
                arr: a,
                movies: true,
                root: cfg.seedbox_movies_root.clone(),
                ru_root: cfg.russian.then(|| cfg.seedbox_ru_movies_root.clone()),
            });
        }
        let mut lookups = 0usize;
        // un titre en lecture n'est jamais déplacé (le lecteur perdrait son fichier) ; Jellyfin injoignable ⇒ rien
        let playing = match ctx.jellyfin.playing_paths().await {
            Ok(p) => p,
            Err(e) => return Ok(Report::new(format!("jellyfin injoignable : {e:#}"), 0)),
        };
        let mut moves_left = cfg.max_moves_per_run;
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        let mut notes: Vec<String> = Vec::new();
        for place in &places {
            let arr = place.arr;
            let prepared = async {
                if !has_root_folder(arr, &place.root).await? {
                    anyhow::bail!("dossier racine {} absent de l'Arr", place.root);
                }
                if let Some(r) = &place.ru_root {
                    if !has_root_folder(arr, r).await? {
                        anyhow::bail!("dossier racine {r} absent de l'Arr");
                    }
                }
                let list = if place.movies {
                    arr.movies().await?
                } else {
                    arr.series().await?
                };
                let key = if place.movies { "movieId" } else { "seriesId" };
                let queued: HashSet<i64> = arr
                    .queue_records()
                    .await?
                    .iter()
                    .filter_map(|r| r.get(key).and_then(Value::as_i64))
                    .collect();
                anyhow::Ok((items(&list), queued, tag_map(arr).await?))
            }
            .await;
            let (list, queued, tags) = match prepared {
                Ok(p) => p,
                Err(e) => {
                    warn!(task = "anime_library", service = arr.name, error = %e, "skipped");
                    notes.push(format!("{}: {e:#}", arr.name));
                    continue;
                }
            };
            let mut moved: Vec<Item> = Vec::new();
            let maps = side_maps(ctx, place.side);
            for it in list {
                if !cfg.only_tmdb.is_empty() && !cfg.only_tmdb.contains(&it.tmdb) {
                    continue;
                }
                let labels: Vec<String> = it
                    .tags
                    .iter()
                    .filter_map(|t| tags.get(t).cloned())
                    .collect();
                let in_anime = in_root(&it.path, &place.root);
                let in_ru = place.ru_root.as_deref().map(|r| in_root(&it.path, r));
                // un tag manuel évite la lecture TMDB
                let class = match manual_override(&labels) {
                    Some(c) => c,
                    None => match tmdb_class(ctx, place.movies, it.tmdb, &mut lookups).await {
                        Some(c) => c,
                        None => {
                            *counts.entry("en_attente").or_default() += 1;
                            continue;
                        }
                    },
                };
                // voie russe = CHOIX du membre (dossier « Russian » choisi dans Jellyseerr) ou tag `russe` posé à la
                // main, jamais la seule langue TMDB : un film russe demandé normalement suit la voie classique
                let class = russian_choice(apply_not_russian(class, &labels), &labels, in_ru);
                let (target, tag_label) = if class == Class::Russian {
                    (place.ru_root.clone().unwrap_or_default(), TAG_RUSSIAN)
                } else {
                    (place.root.clone(), TAG_ANIME)
                };
                match decide(class, &labels, in_anime, in_ru) {
                    Decision::Keep if class == Class::Russian && in_ru == Some(true) => {
                        // demandée dans le dossier russe : l'indexer RuTracker ne sert qu'aux fiches taguées `russe`
                        if !labels.iter().any(|l| l.eq_ignore_ascii_case(TAG_RUSSIAN))
                            && !ctx.dry_run
                        {
                            let editor = if place.movies {
                                ("api/v3/movie/editor", "movieIds")
                            } else {
                                ("api/v3/series/editor", "seriesIds")
                            };
                            match tag_id(arr, &tags, TAG_RUSSIAN).await {
                                Ok(tag) => {
                                    if let Err(e) = arr.put(editor.0, &json!({ editor.1: [it.id], "tags": [tag], "applyTags": "add" })).await {
                                        warn!(task = "anime_library", service = arr.name, title = %it.title, error = %e, "russe tag not set");
                                        continue;
                                    }
                                    info!(task = "anime_library", service = arr.name, title = %it.title, "tag russe posé");
                                    *counts.entry("tag_russe").or_default() += 1;
                                }
                                Err(e) => {
                                    warn!(task = "anime_library", service = arr.name, error = %e, "tag unavailable");
                                    continue;
                                }
                            }
                        }
                        // Jellyseerr est en preventSearch : sans ça, personne ne chercherait. L'Arr n'interroge
                        // que les indexers de la fiche : RuTracker (tag `russe`), C411 étant sans recherche.
                        if !it.missing || queued.contains(&it.id) {
                            continue;
                        }
                        let kind = if place.movies { "movie" } else { "series" };
                        let k = format!("{}:{kind}:{}", place.side, it.id);
                        let t = now();
                        let last = ctx
                            .state
                            .read(|s| s.russian_searches.get(&k).copied())
                            .await;
                        if !search_due(last, t, cfg.ru_search_retry_hours) {
                            continue;
                        }
                        if ctx.dry_run {
                            info!(task = "anime_library", service = arr.name, title = %it.title, "dry-run: would search (RuTracker)");
                            *counts.entry("recherche_russe").or_default() += 1;
                            continue;
                        }
                        let cmd = if place.movies {
                            json!({ "name": "MoviesSearch", "movieIds": [it.id] })
                        } else {
                            json!({ "name": "SeriesSearch", "seriesId": it.id })
                        };
                        match arr.command(cmd).await {
                            Ok(_) => {
                                info!(task = "anime_library", service = arr.name, title = %it.title, "search requested (RuTracker)");
                                *counts.entry("recherche_russe").or_default() += 1;
                                ctx.state
                                    .update(|s| {
                                        s.russian_searches.retain(|_, at| t - *at < 30 * 86_400);
                                        s.russian_searches.insert(k, t);
                                    })
                                    .await?;
                            }
                            Err(e) => {
                                warn!(task = "anime_library", service = arr.name, title = %it.title, error = %e, "search request failed");
                            }
                        }
                    }
                    Decision::Keep => {
                        // déjà rangée : reste le type, indispensable à la numérotation absolue
                        if in_anime && !place.movies && it.series_type.as_deref() != Some("anime") {
                            if ctx.dry_run {
                                info!(task = "anime_library", service = arr.name, title = %it.title, "dry-run: would set seriesType=anime");
                            } else if let Err(e) = fix_series_type(arr, it.id).await {
                                warn!(task = "anime_library", service = arr.name, title = %it.title, error = %e, "seriesType not set");
                            } else {
                                info!(task = "anime_library", service = arr.name, title = %it.title, "seriesType=anime posé");
                            }
                            *counts.entry("type_corrige").or_default() += 1;
                        }
                    }
                    Decision::Unknown => {
                        *counts.entry("inconnu").or_default() += 1;
                        info!(task = "anime_library", service = arr.name, title = %it.title, tmdb = it.tmdb, "unknown: left in place");
                    }
                    Decision::Misplaced => {
                        *counts.entry("a_verifier").or_default() += 1;
                        warn!(task = "anime_library", service = arr.name, title = %it.title, tmdb = it.tmdb, "not anime but in the anime folder: left in place");
                    }
                    Decision::Move => {
                        if map_path(&maps, &it.path)
                            .is_some_and(|(_, jf)| is_playing(&jf, &playing))
                        {
                            *counts.entry("en_lecture").or_default() += 1;
                            continue;
                        }
                        if queued.contains(&it.id) {
                            *counts.entry("telechargement_en_cours").or_default() += 1;
                            continue;
                        }
                        if moves_left == 0 && !ctx.dry_run {
                            *counts.entry("au_prochain_passage").or_default() += 1;
                            continue;
                        }
                        if ctx.dry_run {
                            info!(task = "anime_library", service = arr.name, title = %it.title, tmdb = it.tmdb, from = %it.path, to = %target, "dry-run: would move");
                            *counts.entry("a_deplacer").or_default() += 1;
                            continue;
                        }
                        let tag = match tag_id(arr, &tags, tag_label).await {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(task = "anime_library", service = arr.name, error = %e, "tag unavailable");
                                break;
                            }
                        };
                        // noté avant le déplacement : deletion_cleanup ne doit rien conclure pendant
                        let kind = if place.movies { "movie" } else { "series" };
                        let k = format!("{}:{kind}:{}", place.side, it.id);
                        let t = now();
                        ctx.state
                            .update(|s| {
                                s.anime_moves.retain(|_, at| t - *at < MOVE_GRACE_SECS);
                                s.anime_moves.insert(k, t);
                            })
                            .await?;
                        let anime = class == Class::Anime;
                        match move_item(arr, place.movies, anime, it.id, tag, &target).await {
                            Ok(()) => {
                                info!(task = "anime_library", service = arr.name, title = %it.title, tmdb = it.tmdb, from = %it.path, to = %target, "moved");
                                *counts.entry("deplace").or_default() += 1;
                                moves_left -= 1;
                                moved.push(it);
                            }
                            Err(e) => {
                                warn!(task = "anime_library", service = arr.name, title = %it.title, error = %e, "move failed");
                                *counts.entry("erreur").or_default() += 1;
                            }
                        }
                    }
                }
            }
            if moved.is_empty() {
                continue;
            }
            wait_moves(arr).await;
            // nouveaux chemins lus dans l'Arr (le nom de dossier peut changer)
            let fresh = if place.movies {
                arr.movies().await
            } else {
                arr.series().await
            }
            .map(|l| items(&l))
            .unwrap_or_default();
            let mut jf_paths = Vec::new();
            for it in &moved {
                let new_path = fresh.iter().find(|f| f.id == it.id).map(|f| f.path.clone());
                for p in std::iter::once(it.path.clone()).chain(new_path) {
                    if let Some((host, jf)) = map_path(&maps, &p) {
                        if place.side == "seedbox" {
                            rclone_refresh(ctx, &PathBuf::from(&host)).await;
                        }
                        jf_paths.push(jf);
                    }
                }
            }
            if let Err(e) = ctx.jellyfin.media_updated(&jf_paths).await {
                warn!(task = "anime_library", error = %e, "jellyfin notification failed");
            }
        }
        if counts.get("deplace").copied().unwrap_or(0) > 0 && cfg.scan_after_move && !ctx.dry_run {
            SCAN_PENDING.store(true, Ordering::SeqCst);
        }
        let now = chrono::Utc::now().timestamp();
        if scan_due(
            SCAN_PENDING.load(Ordering::SeqCst),
            now,
            LAST_SCAN.load(Ordering::SeqCst),
            cfg.scan_min_gap_mins,
        ) {
            match ctx.jellyfin.library_refresh().await {
                Ok(()) => {
                    SCAN_PENDING.store(false, Ordering::SeqCst);
                    LAST_SCAN.store(now, Ordering::SeqCst);
                    info!(task = "anime_library", "library scan requested after move");
                    notes.push("analyse de la médiathèque lancée".into());
                }
                Err(e) => warn!(task = "anime_library", error = %e, "library scan request failed"),
            }
        } else if SCAN_PENDING.load(Ordering::SeqCst) {
            notes.push("analyse de la médiathèque au passage suivant".into());
        }
        let actions = counts.get("deplace").copied().unwrap_or(0);
        let mut summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        summary.extend(notes);
        let summary = if summary.is_empty() {
            "rien à ranger".to_string()
        } else {
            summary.join(" ")
        };
        Ok(Report::new(summary, actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn russian_is_a_choice_not_a_language() {
        let none: Vec<String> = vec![];
        let tag = l(&["russe"]);
        // film russe demandé normalement : voie classique, rien ne bouge
        assert_eq!(
            russian_choice(Class::Russian, &none, Some(false)),
            Class::NotAnime
        );
        assert_eq!(
            decide(
                russian_choice(Class::Russian, &none, Some(false)),
                &none,
                false,
                Some(false)
            ),
            Decision::Keep
        );
        // demandé dans le dossier russe (même si TMDB dit autre chose) : voie russe, reste en place
        assert_eq!(
            russian_choice(Class::NotAnime, &none, Some(true)),
            Class::Russian
        );
        assert_eq!(
            decide(Class::Russian, &none, false, Some(true)),
            Decision::Keep
        );
        // tag russe posé à la main hors du dossier : déplacé
        assert_eq!(
            russian_choice(Class::NotAnime, &tag, Some(false)),
            Class::Russian
        );
        assert_eq!(
            decide(Class::Russian, &tag, false, Some(false)),
            Decision::Move
        );
        // côté VPS (pas de dossier russe) : jamais russe
        assert_eq!(russian_choice(Class::Russian, &tag, None), Class::NotAnime);
        // un anime reste un anime
        assert_eq!(russian_choice(Class::Anime, &tag, Some(true)), Class::Anime);
        // seule la voie russe de la seedbox est écartée de C411
        let cfg = crate::config::AnimeLibrary::default();
        assert!(russian_route(
            &json!({"path": "/home/kakaouette/media/Russian Movies/Brat (1997)"}),
            &cfg
        ));
        assert!(russian_route(
            &json!({"path": "/home/kakaouette/media/Russian/Kukhnya"}),
            &cfg
        ));
        assert!(!russian_route(
            &json!({"path": "/home/kakaouette/media/Movies/Brat (1997)"}),
            &cfg
        ));
        assert!(!russian_route(
            &json!({"path": "/home/kakaouette/media/Russian Moviesque/x"}),
            &cfg
        ));
        // recherche : jamais faite, ou assez ancienne
        assert!(search_due(None, 100_000, 24));
        assert!(!search_due(Some(100_000 - 3600), 100_000, 24));
        assert!(search_due(Some(100_000 - 24 * 3600), 100_000, 24));
    }

    #[test]
    fn library_scan_waits_for_a_move_and_the_gap() {
        assert!(!scan_due(false, 10_000, 0, 20)); // aucun déplacement
        assert!(scan_due(true, 10_000, 0, 20)); // première fois
        assert!(!scan_due(true, 10_000, 10_000 - 5 * 60, 20)); // trop tôt : attend
        assert!(scan_due(true, 10_000, 10_000 - 20 * 60, 20)); // écart atteint
    }

    #[test]
    fn root_prefix_is_a_whole_folder() {
        assert!(in_root("/anime/Bleach", "/anime"));
        assert!(in_root("/anime", "/anime/"));
        assert!(!in_root("/anime-films/Marnie", "/anime"));
        assert!(!in_root("/tv/Bleach", "/anime"));
        assert!(in_root(
            "/home/kakaouette/media/Anime Movies/Marnie (2014)",
            "/home/kakaouette/media/Anime Movies"
        ));
        assert!(!in_root(
            "/home/kakaouette/media/Anime Movies/Marnie (2014)",
            "/home/kakaouette/media/Anime"
        ));
    }

    #[test]
    fn playing_titles_are_detected_by_folder() {
        let playing = vec!["/seedbox/media/TV Shows/Attack on Titan/Season 1/e01.mkv".to_string()];
        assert!(is_playing(
            "/seedbox/media/TV Shows/Attack on Titan",
            &playing
        ));
        assert!(!is_playing(
            "/seedbox/media/TV Shows/Attack on Titan Junior",
            &playing
        ));
        assert!(!is_playing("/media/tvshows/Bleach", &playing));
        assert!(!is_playing("/media/tvshows/Bleach", &[]));
    }

    #[test]
    fn decisions() {
        assert_eq!(decide(Class::Anime, &[], false, None), Decision::Move);
        assert_eq!(decide(Class::Anime, &[], true, None), Decision::Keep);
        assert_eq!(decide(Class::NotAnime, &[], false, None), Decision::Keep);
        // jamais ressorti automatiquement
        assert_eq!(
            decide(Class::NotAnime, &[], true, None),
            Decision::Misplaced
        );
        assert_eq!(decide(Class::Unknown, &[], false, None), Decision::Unknown);
        assert_eq!(decide(Class::Unknown, &[], true, None), Decision::Keep);
        // tags manuels
        assert_eq!(
            decide(Class::NotAnime, &l(&["anime"]), false, None),
            Decision::Move
        );
        assert_eq!(
            decide(Class::Anime, &l(&["pas-anime"]), false, None),
            Decision::Keep
        );
        assert_eq!(
            decide(Class::Unknown, &l(&["pas-anime"]), false, None),
            Decision::Keep
        );
    }

    #[test]
    fn cache_expiry() {
        let rec = |at: i64, c: &str| AnimeClassRecord {
            at,
            class: c.into(),
        };
        let now = 100 * 86_400;
        assert_eq!(
            cached(Some(&rec(now - 29 * 86_400, "anime")), now, 30),
            Some(Class::Anime)
        );
        assert_eq!(
            cached(Some(&rec(now - 31 * 86_400, "anime")), now, 30),
            None
        );
        assert_eq!(
            cached(Some(&rec(now - 3600, "unknown")), now, 30),
            Some(Class::Unknown)
        );
        assert_eq!(
            cached(Some(&rec(now - 2 * 86_400, "unknown")), now, 30),
            None
        );
        assert_eq!(cached(None, now, 30), None);
    }
}
