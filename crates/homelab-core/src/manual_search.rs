//! Recherche manuelle (page `/recherche` de homelabd) : une saison ou un film, **par identifiant TMDB** chez C411
//! (via Prowlarr, une requête), avec les titres de la fiche en secours. Remplace la recherche de Sonarr/Radarr
//! pour les animés : celle-ci interroge chaque titre connu, épisode par épisode (le 2026-09-17, plusieurs
//! minutes, délai dépassé du proxy de la seedbox et 429 de C411).
//!
//! Rien n'est filtré : toutes les releases sont montrées, triées comme le choix automatique (français, qualité,
//! H.264, sources), avec leurs écarts signalés. « Télécharger » passe par `series_search::send_release`, le même
//! chemin que les recherches automatiques. Les requêtes C411 de la page ont leur propre plafond horaire.

use std::cmp::Reverse;
use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::clients::ArrClient;
use crate::context::TaskContext;
use crate::matching::normalize;
use crate::tasks::anime_library::in_root;
use crate::tasks::series_search::{
    allowed_qualities, is_h264, lang_rank, send_release, tmdb_matches, Target,
};

/// Résultats analysés au plus par recherche (un appel `parse` de l'Arr chacun).
const MAX_PARSED: usize = 80;

/// Un titre des Arrs proposé sur la page.
#[derive(Debug, Clone)]
pub struct Found {
    pub arr: &'static str,
    pub id: i64,
    pub title: String,
    pub year: i64,
    pub movie: bool,
    pub anime: bool,
    pub has_file: bool,
    /// Séries : (saison, fichiers, épisodes diffusés).
    pub seasons: Vec<(i64, i64, i64)>,
}

/// Une release trouvée, prête à afficher.
#[derive(Debug, Clone)]
pub struct Row {
    pub indexer: String,
    pub title: String,
    pub size: i64,
    pub seeders: i64,
    /// 4 VF, 3 MULTi, 2 FRENCH, 1 VOSTFR, 0 aucun marqueur français.
    pub lang: u8,
    pub resolution: i64,
    pub h264: bool,
    pub quality: String,
    pub season: Option<i64>,
    pub episodes: Vec<i64>,
    pub full_season: bool,
    /// Écarts à la politique (vide = la release que la recherche automatique pourrait prendre).
    pub flags: Vec<&'static str>,
    /// `title`, `downloadUrl`, `publishDate`, `quality`, pour `send_release`.
    pub release: Value,
}

/// Recherche en cours ou terminée, gardée en mémoire par la page.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub arr: &'static str,
    pub label: String,
    pub target: Target,
    pub started: i64,
    pub done: bool,
    pub rows: Vec<Row>,
    pub notes: Vec<String>,
    pub sent: Vec<String>,
}

/// Arr d'après son nom (`sonarr`, `radarr`, `sonarr-seedbox`, `radarr-seedbox`).
pub fn arr_by_name<'a>(ctx: &'a TaskContext, name: &str) -> Option<&'a ArrClient> {
    [
        Some(&ctx.sonarr),
        Some(&ctx.radarr),
        ctx.seedbox_sonarr.as_ref(),
        ctx.seedbox_radarr.as_ref(),
    ]
    .into_iter()
    .flatten()
    .find(|a| a.name == name)
}

fn anime_roots(ctx: &TaskContext) -> [String; 4] {
    let c = &ctx.cfg.tasks.anime_library;
    [
        c.vps_series_root.clone(),
        c.vps_movies_root.clone(),
        c.seedbox_series_root.clone(),
        c.seedbox_movies_root.clone(),
    ]
}

/// Fiche d'Arr → titre proposé.
pub fn found_from(arr: &'static str, movie: bool, v: &Value, roots: &[String]) -> Option<Found> {
    let path = v.get("path").and_then(Value::as_str).unwrap_or("");
    let seasons = v
        .get("seasons")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let n = s.get("seasonNumber").and_then(Value::as_i64)?;
                    let st = s.get("statistics");
                    let get = |k: &str| {
                        st.and_then(|x| x.get(k))
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                    };
                    Some((n, get("episodeFileCount"), get("episodeCount")))
                })
                .filter(|(n, _, _)| *n > 0)
                .collect()
        })
        .unwrap_or_default();
    Some(Found {
        arr,
        id: v.get("id").and_then(Value::as_i64)?,
        title: v.get("title").and_then(Value::as_str)?.to_string(),
        year: v.get("year").and_then(Value::as_i64).unwrap_or(0),
        movie,
        anime: roots.iter().any(|r| in_root(path, r))
            || v.get("seriesType").and_then(Value::as_str) == Some("anime"),
        has_file: v.get("hasFile").and_then(Value::as_bool).unwrap_or(false),
        seasons,
    })
}

/// Le titre (ou un titre alternatif) contient la recherche, après normalisation.
pub fn title_hit(v: &Value, query: &str) -> bool {
    let q = normalize(query);
    if q.is_empty() {
        return false;
    }
    let mut names: Vec<&str> = vec![
        v.get("title").and_then(Value::as_str).unwrap_or(""),
        v.get("originalTitle").and_then(Value::as_str).unwrap_or(""),
    ];
    if let Some(a) = v.get("alternateTitles").and_then(Value::as_array) {
        names.extend(
            a.iter()
                .filter_map(|t| t.get("title").and_then(Value::as_str)),
        );
    }
    names.iter().any(|n| normalize(n).contains(&q))
}

/// Titres des 4 Arrs correspondant à la recherche (au plus 30).
pub async fn find(ctx: &TaskContext, query: &str) -> Vec<Found> {
    let roots = anime_roots(ctx);
    let mut out = Vec::new();
    for (arr, movie) in [
        (Some(&ctx.sonarr), false),
        (ctx.seedbox_sonarr.as_ref(), false),
        (Some(&ctx.radarr), true),
        (ctx.seedbox_radarr.as_ref(), true),
    ] {
        let Some(arr) = arr else { continue };
        let list = if movie {
            arr.movies().await
        } else {
            arr.series().await
        };
        for v in list.unwrap_or_default() {
            if title_hit(&v, query) {
                out.extend(found_from(arr.name, movie, &v, &roots));
            }
        }
    }
    out.truncate(30);
    out
}

/// Écarts d'une release à la politique de la plateforme.
pub fn flags_for(
    row: &Row,
    want_season: Option<i64>,
    allowed: &HashSet<i64>,
    matched: bool,
) -> Vec<&'static str> {
    let mut f = Vec::new();
    match row.lang {
        0 => f.push("sans français"),
        1 => f.push("VOSTFR"),
        _ => {}
    }
    if row.resolution > 1080 {
        f.push("plus de 1080p");
    }
    let q = row
        .release
        .pointer("/quality/quality/id")
        .and_then(Value::as_i64);
    if !allowed.is_empty() && !q.is_some_and(|q| allowed.contains(&q)) {
        f.push("hors profil");
    }
    if row.seeders == 0 {
        f.push("aucune source");
    }
    if let Some(want) = want_season {
        match row.season {
            Some(s) if s != want => f.push("autre saison"),
            None => f.push("saison inconnue"),
            _ => {}
        }
    }
    if !matched {
        f.push("titre non reconnu");
    }
    f
}

/// Ordre d'affichage : releases sans écart d'abord, puis la bonne œuvre et la bonne saison, langue, saison
/// complète, résolution, H.264, sources.
pub fn sort_rows(rows: &mut [Row]) {
    rows.sort_by_key(|r| {
        let right_item = !r
            .flags
            .iter()
            .any(|f| matches!(*f, "autre saison" | "titre non reconnu"));
        Reverse((
            r.flags.is_empty(),
            right_item,
            r.lang,
            r.full_season,
            r.resolution.min(1080),
            r.h264,
            r.seeders,
        ))
    });
}

fn num(v: Option<&Value>) -> i64 {
    v.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
    .unwrap_or(0)
}

/// Release Prowlarr + analyse de l'Arr (`parse`) → ligne.
pub fn build_row(
    indexer: &str,
    result: &Value,
    parse: &Value,
    item_id: i64,
    want_season: Option<i64>,
    allowed: &HashSet<i64>,
) -> Option<Row> {
    let title = result.get("title").and_then(Value::as_str)?.to_string();
    let url = result.get("downloadUrl").and_then(Value::as_str);
    let magnet = result.get("magnetUrl").and_then(Value::as_str);
    if url.is_none() && magnet.is_none() {
        return None;
    }
    let movie = want_season.is_none();
    let info = parse
        .get(if movie {
            "parsedMovieInfo"
        } else {
            "parsedEpisodeInfo"
        })
        .cloned()
        .unwrap_or(Value::Null);
    let matched = parse
        .pointer(if movie { "/movie/id" } else { "/series/id" })
        .and_then(Value::as_i64)
        == Some(item_id);
    let quality = info.get("quality").cloned().unwrap_or(Value::Null);
    let mut row = Row {
        indexer: indexer.to_string(),
        size: num(result.get("size")),
        seeders: num(result.get("seeders")),
        lang: lang_rank(&title),
        resolution: quality
            .pointer("/quality/resolution")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        h264: is_h264(&title),
        quality: quality
            .pointer("/quality/name")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        season: info.get("seasonNumber").and_then(Value::as_i64),
        episodes: info
            .get("episodeNumbers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        full_season: info
            .get("fullSeason")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        flags: Vec::new(),
        release: json!({
            "title": title,
            "downloadUrl": url,
            "magnetUrl": magnet,
            "publishDate": result.get("publishDate"),
            "quality": quality,
        }),
        title,
    };
    if movie {
        row.season = None;
    }
    row.flags = flags_for(&row, want_season, allowed, matched);
    Some(row)
}

/// Titres pour la recherche en texte libre : titre de l'Arr puis titres alternatifs différents, au plus `n`.
pub fn text_names(item: &Value, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |s: &str| {
        let k = normalize(s);
        if !k.is_empty() && seen.insert(k) {
            out.push(s.to_string());
        }
    };
    for key in ["title", "originalTitle"] {
        if let Some(t) = item.get(key).and_then(Value::as_str) {
            push(t);
        }
    }
    if let Some(a) = item.get("alternateTitles").and_then(Value::as_array) {
        for t in a
            .iter()
            .filter_map(|t| t.get("title").and_then(Value::as_str))
        {
            push(t);
        }
    }
    out.truncate(n);
    out
}

/// Lance la recherche : C411 par identifiant (plafond horaire), Nyaa pour un animé. Renvoie les lignes
/// triées et des remarques pour la page.
pub async fn run(
    ctx: &TaskContext,
    arr: &ArrClient,
    item_id: i64,
    season: Option<i64>,
) -> Result<(Vec<Row>, Vec<String>)> {
    let cfg = &ctx.cfg.manual_search;
    let prow = ctx
        .prowlarr
        .as_ref()
        .context("Prowlarr non configuré (PROWLARR_API_KEY)")?;
    let movie = arr.is_radarr();
    let item = arr
        .get(
            &format!(
                "api/v3/{}/{item_id}",
                if movie { "movie" } else { "series" }
            ),
            &[],
        )
        .await?;
    let tmdb = item.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let profile = item
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let allowed = allowed_qualities(&arr.quality_profile(profile).await?);
    let mut notes = Vec::new();
    let mut raw: Vec<(String, Value)> = Vec::new();

    match prow.indexer_id(&cfg.c411_indexer).await? {
        None => notes.push(format!("{} absent de Prowlarr", cfg.c411_indexer)),
        Some(_) if tmdb <= 0 => {
            notes.push("fiche sans identifiant TMDB : C411 non interrogé".into())
        }
        Some(id) => {
            if crate::budget::take(ctx, true).await? {
                match prow.search_by_tmdb(tmdb, season, id).await {
                    Ok(list) => {
                        let n = list.len();
                        raw.extend(
                            list.into_iter()
                                .filter(|r| r.get("tmdbId").is_none() || tmdb_matches(r, tmdb))
                                .map(|r| (cfg.c411_indexer.clone(), r)),
                        );
                        notes.push(format!(
                            "{} : {n} release(s) par identifiant",
                            cfg.c411_indexer
                        ));
                    }
                    Err(e) => notes.push(format!(
                        "{} indisponible ({e:#}) : indexeur en pause ou limite atteinte",
                        cfg.c411_indexer
                    )),
                }
            } else {
                notes.push(format!(
                    "plafond de {} requêtes C411 par heure atteint : réessayer plus tard",
                    ctx.cfg.indexers.c411_max_per_hour
                ));
            }
        }
    }

    // repli : rien par identifiant → titres de la fiche en texte libre (même indexer, même budget)
    if raw.is_empty() {
        if let Some(id) = prow.indexer_id(&cfg.c411_indexer).await? {
            for name in text_names(&item, cfg.text_queries) {
                if !crate::budget::take(ctx, true).await? {
                    notes.push(
                        "plafond horaire atteint : recherche en texte libre abandonnée".into(),
                    );
                    break;
                }
                match prow.search(&name, id, 100).await {
                    Ok(list) => {
                        notes.push(format!(
                            "{} « {name} » : {} release(s)",
                            cfg.c411_indexer,
                            list.len()
                        ));
                        raw.extend(list.into_iter().map(|r| (cfg.c411_indexer.clone(), r)));
                    }
                    Err(e) => {
                        notes.push(format!("{} « {name} » : erreur ({e:#})", cfg.c411_indexer))
                    }
                }
            }
        }
    }

    let mut seen = HashSet::new();
    raw.retain(|(_, r)| {
        r.get("title")
            .and_then(Value::as_str)
            .is_some_and(|t| seen.insert(t.to_string()))
    });
    if raw.len() > MAX_PARSED {
        notes.push(format!(
            "{} releases, seules les {MAX_PARSED} premières sont analysées",
            raw.len()
        ));
        raw.truncate(MAX_PARSED);
    }
    let mut rows = Vec::new();
    for (indexer, r) in &raw {
        let Some(title) = r.get("title").and_then(Value::as_str) else {
            continue;
        };
        let parse = arr.parse(title).await.unwrap_or(Value::Null);
        rows.extend(build_row(indexer, r, &parse, item_id, season, &allowed));
    }
    sort_rows(&mut rows);
    Ok((rows, notes))
}

/// Confie une release à l'Arr (ou au qBittorrent du même côté). Essai à blanc : rien n'est envoyé.
pub async fn grab(ctx: &TaskContext, arr: &ArrClient, row: &Row, target: Target) -> Result<String> {
    if ctx.dry_run {
        tracing::info!(task = "manual_search", service = arr.name, release = %row.title, "dry-run: would send");
        return Ok(format!("essai à blanc : {} aurait été envoyé", row.title));
    }
    let Some(prow) = ctx.prowlarr.as_ref() else {
        bail!("Prowlarr non configuré");
    };
    let (outcome, detail) = send_release(
        ctx,
        prow,
        arr,
        &row.title,
        &row.release,
        &row.indexer,
        target,
    )
    .await?;
    tracing::info!(task = "manual_search", service = arr.name, release = %row.title, %outcome, %detail, "manual grab");
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> HashSet<i64> {
        [7, 9].into_iter().collect()
    }

    fn result(title: &str, seeders: i64) -> Value {
        json!({"title": title, "downloadUrl": "http://p/dl", "seeders": seeders, "size": 1_000_000})
    }

    fn parse(series: i64, season: i64, full: bool, q: i64, res: i64) -> Value {
        json!({"series": {"id": series}, "parsedEpisodeInfo": {"seasonNumber": season, "fullSeason": full,
               "episodeNumbers": [], "quality": {"quality": {"id": q, "name": "WEBDL-1080p", "resolution": res}}}})
    }

    #[test]
    fn rows_are_flagged_not_dropped() {
        let ok = build_row(
            "C411",
            &result("Mushishi.S02.MULTi.1080p.WEB.x264", 5),
            &parse(3, 2, true, 7, 1080),
            3,
            Some(2),
            &allowed(),
        )
        .unwrap();
        assert!(ok.flags.is_empty(), "{:?}", ok.flags);
        let vost = build_row(
            "Nyaa.si",
            &result("[X] Mushishi S2 VOSTFR 2160p HEVC", 0),
            &parse(3, 1, true, 19, 2160),
            3,
            Some(2),
            &allowed(),
        )
        .unwrap();
        assert_eq!(
            vost.flags,
            vec![
                "VOSTFR",
                "plus de 1080p",
                "hors profil",
                "aucune source",
                "autre saison"
            ]
        );
        let other = build_row(
            "Nyaa.si",
            &result("[X] Other Show - 01 [1080p]", 9),
            &json!({}),
            3,
            Some(2),
            &allowed(),
        )
        .unwrap();
        assert!(
            other.flags.contains(&"sans français")
                && other.flags.contains(&"titre non reconnu")
                && other.flags.contains(&"saison inconnue")
        );
        assert!(
            build_row(
                "C411",
                &json!({"title": "x"}),
                &json!({}),
                3,
                Some(2),
                &allowed()
            )
            .is_none(),
            "sans lien"
        );
    }

    #[test]
    fn movies_ignore_seasons() {
        let p = json!({"movie": {"id": 8}, "parsedMovieInfo": {"quality": {"quality": {"id": 7, "name": "Bluray-1080p", "resolution": 1080}}}});
        let r = build_row(
            "C411",
            &result("Souvenirs.De.Marnie.2014.MULTi.1080p.BluRay.x264", 4),
            &p,
            8,
            None,
            &allowed(),
        )
        .unwrap();
        assert!(r.flags.is_empty(), "{:?}", r.flags);
        assert_eq!(r.season, None);
    }

    #[test]
    fn sorting_puts_clean_french_packs_first() {
        let mk = |t: &str, s: i64, full: bool, seeders: i64| {
            build_row(
                "C411",
                &result(t, seeders),
                &parse(3, s, full, 7, 1080),
                3,
                Some(2),
                &allowed(),
            )
            .unwrap()
        };
        let mut rows = vec![
            mk("Show.S02E01.VOSTFR.1080p.x264", 2, false, 99),
            mk("Show.S02.MULTi.1080p.x264", 2, true, 3),
            mk("Show.S02E01.VFF.1080p.x264", 2, false, 10),
            mk("Show.S01.VFF.1080p.x264", 1, true, 50),
        ];
        sort_rows(&mut rows);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles[0], "Show.S02E01.VFF.1080p.x264");
        assert_eq!(titles[1], "Show.S02.MULTi.1080p.x264");
        assert_eq!(
            *titles.last().unwrap(),
            "Show.S01.VFF.1080p.x264",
            "autre saison en dernier, même en VFF"
        );
    }

    #[test]
    fn magnet_only_releases_are_kept() {
        let nyaa = json!({"title": "Mushi-Shi.S02.VOSTFR.1080p", "magnetUrl": "http://p/5/download?link=x",
                          "seeders": "20", "size": "13314398208"});
        let r = build_row(
            "Nyaa.si",
            &nyaa,
            &parse(3, 2, true, 7, 1080),
            3,
            Some(2),
            &allowed(),
        )
        .unwrap();
        assert_eq!(r.release["magnetUrl"], "http://p/5/download?link=x");
        assert!(r.release["downloadUrl"].is_null());
        assert_eq!((r.seeders, r.size), (20, 13_314_398_208));
        assert_eq!(r.flags, vec!["VOSTFR"]);
    }

    #[test]
    fn titles_and_nyaa_names() {
        let v = json!({"title": "Mushi-Shi", "alternateTitles": [{"title": "Mushishi"}, {"title": "Mushi Shi"}, {"title": "Mushishi Zoku Shou"}]});
        assert!(title_hit(&v, "mushishi"));
        assert!(title_hit(&v, "Zoku"));
        assert!(!title_hit(&v, "Bleach"));
        assert!(!title_hit(&v, "  "));
        let names = text_names(&v, 2);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Mushi-Shi");
    }
}
