//! Recherche des saisons manquantes **par identifiant TMDB**, chez un seul indexer (C411), via Prowlarr.
//!
//! Pourquoi pas la recherche de Sonarr : elle interroge l'indexer avec les titres de Sonarr (« Shingeki no
//! Kyojin », « Attack on Titan ») alors que C411 range la série sous son titre français (« L'Attaque des
//! Titans ») ; et pour un animé elle envoie 3 à 4 requêtes par épisode, ce qui déclenche la limite d'API de
//! C411 (429) et bloque l'indexeur jusqu'à 24 h dans Sonarr. Par identifiant TMDB, C411 renvoie les releases
//! de la série quel que soit leur nom (une requête par saison) et porte l'identifiant de chacune : on vérifie
//! l'identifiant, jamais le titre. Jellyseerr ne demande plus de recherche à Sonarr (`preventSearch`).
//!
//! Pour quelques saisons suivies avec des épisodes diffusés manquants (nouvelles demandes d'abord) : requête
//! `{TmdbId}{Season}` → releases de cette série (attribut `tmdbId`) → `parse` Sonarr (saison, pack, qualité)
//! → garde-fous (français, qualité du profil ≤ 1080p, sources, épisodes manquants) → meilleur candidat →
//! `release/push` à Sonarr ; s'il refuse pour une raison d'identification ou d'indexeur bloqué, le `.torrent`
//! est ajouté au qBittorrent du même côté avec l'étiquette `homelab:series=<id>`, que `torrent_import` lit
//! pour importer dans cette fiche sans analyser de nom. Rien par identifiant : un essai en texte libre avec
//! les noms connus de la série (titre vérifié). Requêtes plafonnées et espacées (limite de C411).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::{ArrClient, ProwlarrClient, QbitClient};
use crate::config::Config;
use crate::context::TaskContext;
use crate::matching::{normalize, parsed_series};
use crate::state::{now, SeasonSearchRecord};

pub struct SeriesSearch;

/// Une release candidate, réduite à ce qui sert au choix.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// `title`, `downloadUrl`, `publishDate`, `quality` (qualité parsée par l'Arr).
    pub release: Value,
    pub title: String,
    pub season: i64,
    pub episodes: Vec<i64>,
    pub full_season: bool,
    pub lang_rank: u8,
    pub resolution: i64,
    pub h264: bool,
    pub seeders: i64,
    pub size: i64,
}

/// Rang de langue d'après le titre : VF 4 > MULTi 3 > FRENCH 2 > VOSTFR 1 > **VO 0** (aucun marqueur
/// français). Une release de rang 0 n'est prise qu'en dernier recours (voir `choose`).
pub fn lang_rank(title: &str) -> u8 {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(&x.as_str()));
    if has(&["VFF", "TRUEFRENCH", "VFQ", "VFI", "VF2", "VFB"]) {
        4
    } else if has(&["MULTI"]) {
        3
    } else if has(&["FRENCH"]) {
        2
    } else if has(&["VOSTFR", "SUBFRENCH"]) {
        1
    } else {
        0
    }
}

/// Taille acceptable pour le choix automatique : `max_gb` par épisode (ou par film). Une saison
/// complète est jugée sur sa taille divisée par le nombre d'épisodes annoncés.
pub fn size_ok(size: i64, units: usize, max_gb: f64) -> bool {
    let units = units.max(1) as f64;
    size <= 0 || (size as f64 / units) / 1_073_741_824.0 <= max_gb
}

/// Le titre parsé d'une release correspond-il **exactement** (après normalisation) à un nom de la série ?
/// Ne sert qu'à l'essai en texte libre ; la recherche par identifiant n'en a pas besoin.
pub fn title_matches(release_series_title: &str, names: &[String]) -> bool {
    let t = normalize(release_series_title);
    !t.is_empty() && names.iter().any(|n| normalize(n) == t)
}

pub fn is_h264(title: &str) -> bool {
    let t = title.to_ascii_uppercase();
    !(t.contains("265") || t.contains("HEVC"))
}

/// La release porte-t-elle l'identifiant TMDB de l'œuvre cherchée ? (C411 renvoie `tmdbId` sur chacune.)
pub fn tmdb_matches(release: &Value, tmdb_id: i64) -> bool {
    tmdb_id > 0 && release.get("tmdbId").and_then(Value::as_i64) == Some(tmdb_id)
}

/// Candidat d'après un résultat Prowlarr et l'analyse (`parsedEpisodeInfo`) de son titre par Sonarr.
pub fn series_candidate(result: &Value, info: &Value, season: i64) -> Option<Candidate> {
    let title = result.get("title").and_then(Value::as_str)?.to_string();
    let url = result.get("downloadUrl").and_then(Value::as_str)?;
    if info.get("seasonNumber").and_then(Value::as_i64) != Some(season) {
        return None;
    }
    let quality = info.get("quality").cloned().unwrap_or(Value::Null);
    Some(Candidate {
        lang_rank: lang_rank(&title),
        season,
        episodes: info
            .get("episodeNumbers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        full_season: info
            .get("fullSeason")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        resolution: quality
            .pointer("/quality/resolution")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        h264: is_h264(&title),
        seeders: result.get("seeders").and_then(Value::as_i64).unwrap_or(0),
        size: result
            .get("size")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0),
        release: json!({
            "title": title,
            "downloadUrl": url,
            "publishDate": result.get("publishDate"),
            "quality": quality,
        }),
        title,
    })
}

/// Ids de qualité autorisés par un profil Sonarr/Radarr (groupes compris).
pub fn allowed_qualities(profile: &Value) -> HashSet<i64> {
    let mut out = HashSet::new();
    for it in profile
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let allowed = it.get("allowed").and_then(Value::as_bool).unwrap_or(false);
        if !allowed {
            continue;
        }
        if let Some(id) = it.pointer("/quality/id").and_then(Value::as_i64) {
            out.insert(id);
        }
        for sub in it
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = sub.pointer("/quality/id").and_then(Value::as_i64) {
                out.insert(id);
            }
        }
    }
    out
}

/// Qualité et résolution acceptables (profil de l'Arr, jamais au-delà de 1080p), avec au moins une source.
pub fn acceptable(release: &Value, resolution: i64, seeders: i64, allowed: &HashSet<i64>) -> bool {
    seeders > 0
        && resolution <= 1080
        && release
            .pointer("/quality/quality/id")
            .and_then(Value::as_i64)
            .map(|q| allowed.is_empty() || allowed.contains(&q))
            .unwrap_or(false)
}

/// Meilleur candidat pour une saison : pack si `want_pack`, sinon épisodes tous manquants.
/// Les releases trop grosses (`max_gb` par épisode) sont écartées ; une release sans français n'est
/// prise que si `allow_vo` et qu'aucune release française n'est acceptable ; à langue et qualité
/// égales, une release à une seule source passe derrière les autres.
pub fn choose<'a>(
    cands: &'a [Candidate],
    season: i64,
    want_pack: bool,
    missing: &HashSet<i64>,
    allowed: &HashSet<i64>,
    max_gb: f64,
    allow_vo: bool,
) -> Option<&'a Candidate> {
    let ok = |c: &&Candidate| {
        let units = if c.full_season {
            missing.len().max(c.episodes.len())
        } else {
            c.episodes.len()
        };
        c.season == season
            && acceptable(&c.release, c.resolution, c.seeders, allowed)
            && size_ok(c.size, units, max_gb)
            && if want_pack {
                c.full_season
            } else {
                !c.full_season
                    && !c.episodes.is_empty()
                    && c.episodes.iter().all(|e| missing.contains(e))
            }
    };
    let best = |vo: bool| {
        cands
            .iter()
            .filter(|c| ok(c) && (vo || c.lang_rank > 0))
            .max_by_key(|c| (c.lang_rank, c.resolution, c.seeders >= 2, c.h264, c.seeders))
    };
    best(false).or_else(|| if allow_vo { best(true) } else { None })
}

/// Délai après la prise d'épisodes seuls (le temps du téléchargement et de l'import).
pub const EPISODE_RETRY_HOURS: i64 = 2;

/// Faut-il (re)chercher ? Une erreur (indexeur indisponible, délai dépassé) est retentée vite.
pub fn due(
    rec: Option<&SeasonSearchRecord>,
    now: i64,
    retry_h: i64,
    grabbed_h: i64,
    error_h: i64,
) -> bool {
    match rec {
        None => true,
        Some(r) => {
            let wait = match r.outcome.as_str() {
                "grabbed" => grabbed_h,
                // un épisode seul pris : le reste de la saison est recherché peu après son import
                "grabbed_episode" => grabbed_h.min(EPISODE_RETRY_HOURS),
                "error" => error_h,
                _ => retry_h,
            };
            now - r.at >= wait * 3600
        }
    }
}

/// Refus de l'Arr qui ne tiennent qu'à l'identification ou à l'état de l'indexeur : on passe alors par
/// qBittorrent. Tout autre refus (liste noire, taille, profil…) est respecté.
pub fn bypassable_rejection(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "unknown series",
        "unknown movie",
        "blocked till",
        "is disabled",
        "unable to parse",
        "matched to",
        "wasn't requested",
    ]
    .iter()
    .any(|k| r.contains(k))
}

/// Raisons de refus d'une décision `release/push` (tableau de décisions ou décision seule).
pub fn push_rejections(decision: &Value) -> Vec<String> {
    decision
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(std::slice::from_ref(decision))
        .iter()
        .filter(|d| d.get("approved").and_then(Value::as_bool) != Some(true))
        .flat_map(|d| {
            d.get("rejections")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|r| {
            r.as_str()
                .map(str::to_string)
                .or_else(|| r.get("reason").and_then(Value::as_str).map(str::to_string))
        })
        .collect()
}

/// Ordre de traitement : séries ajoutées le plus récemment d'abord (une nouvelle demande passe devant
/// l'arriéré), puis diffusion la plus récente. `entries` : (date d'ajout, dernière diffusion manquante).
pub fn pick_order(entries: &[(String, String)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..entries.len()).collect();
    idx.sort_by(|a, b| {
        let (ea, eb) = (&entries[*a], &entries[*b]);
        eb.0.cmp(&ea.0).then_with(|| eb.1.cmp(&ea.1))
    });
    idx
}

/// Budget de requêtes à l'indexer pour un passage, avec un écart minimal entre deux requêtes.
pub struct Throttle {
    left: usize,
    gap: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(budget: usize, gap_secs: u64) -> Self {
        Self {
            left: budget,
            gap: Duration::from_secs(gap_secs),
            last: None,
        }
    }

    pub fn remaining(&self) -> usize {
        self.left
    }

    /// Consomme une requête (en attendant l'écart minimal) ; `false` si le budget est épuisé.
    pub async fn take(&mut self) -> bool {
        if self.left == 0 {
            return false;
        }
        if let Some(t) = self.last {
            let since = t.elapsed();
            if since < self.gap {
                tokio::time::sleep(self.gap - since).await;
            }
        }
        self.left -= 1;
        self.last = Some(Instant::now());
        true
    }
}

/// qBittorrent de la machine d'un Arr (`…-seedbox` → seedbox).
pub fn qbit_for<'a>(ctx: &'a TaskContext, arr: &ArrClient) -> Option<&'a QbitClient> {
    if arr.name.ends_with("seedbox") {
        ctx.seedbox_qbit.as_ref()
    } else {
        Some(&ctx.qbit)
    }
}

/// Lien de téléchargement Prowlarr tel que les Arrs du VPS le joignent : Prowlarr renvoie son adresse vue
/// de l'hôte (`http://localhost:9696/4/download?…`), qui, depuis un conteneur, désigne le conteneur lui-même
/// (« Connection refused », vu le 2026-09-17 : l'envoi est accepté mais rien ne télécharge).
pub fn url_for_arrs(url: &str, host_base: &str, arr_base: &str) -> String {
    let host = host_base.trim_end_matches('/');
    match url.strip_prefix(host) {
        Some(rest) if !arr_base.is_empty() => format!("{}{rest}", arr_base.trim_end_matches('/')),
        _ => url.to_string(),
    }
}

async fn to_qbittorrent(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    url: &str,
    c_title: &str,
    tag: &str,
    why: &str,
) -> Result<(String, String)> {
    let qbit = qbit_for(ctx, arr).context("aucun qBittorrent pour ce côté")?;
    let torrent = prow.download(url).await?;
    qbit.add_torrent(torrent, "", tag).await?;
    Ok((
        "grabbed".into(),
        format!("{c_title} (ajouté à qBittorrent, {} : {why})", arr.name),
    ))
}

/// Confie la release. Arr du VPS : `release/push` avec un lien qu'il peut joindre, puis vérification qu'il
/// l'a bien mise en file (un envoi accepté peut échouer au téléchargement sans le dire). Arr de la seedbox
/// (Prowlarr du VPS injoignable), refus d'identification, indexeur bloqué ou téléchargement non pris : le
/// `.torrent` va au qBittorrent du même côté avec l'étiquette `tag`, lue par `torrent_import`.
/// Ce qu'on télécharge : une saison d'une série, ou un film (fiche de l'Arr).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Season { series_id: i64, season: i64 },
    Movie { movie_id: i64 },
}

impl Target {
    /// Étiquette qBittorrent lue par `torrent_import`.
    pub fn tag(&self) -> String {
        match self {
            Target::Season { series_id, season } => {
                format!("homelab:series={series_id}:season={season}")
            }
            Target::Movie { movie_id } => format!("homelab:movie={movie_id}"),
        }
    }

    /// L'élément de file d'attente de l'Arr concerne-t-il cette cible ? (Le titre de la file est le nom
    /// interne du torrent, souvent différent du titre de la release : on compare les identifiants.)
    pub fn in_queue(&self, record: &Value) -> bool {
        let id = |k: &str| record.get(k).and_then(Value::as_i64);
        match self {
            Target::Season { series_id, season } => {
                id("seriesId") == Some(*series_id)
                    && (id("seasonNumber").or_else(|| {
                        record
                            .pointer("/episode/seasonNumber")
                            .and_then(Value::as_i64)
                    }) == Some(*season))
            }
            Target::Movie { movie_id } => id("movieId") == Some(*movie_id),
        }
    }
}

pub async fn send_release(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    c_title: &str,
    release: &Value,
    indexer: &str,
    target: Target,
) -> Result<(String, String)> {
    let tag = target.tag();
    let tag = tag.as_str();
    // release sans .torrent (Nyaa) : lien magnet ajouté directement au qBittorrent du même côté
    if let (None, Some(magnet)) = (
        release.get("downloadUrl").and_then(Value::as_str),
        release.get("magnetUrl").and_then(Value::as_str),
    ) {
        let qbit = qbit_for(ctx, arr).context("aucun qBittorrent pour ce côté")?;
        qbit.add_url(&prow.resolve_magnet(magnet).await?, tag)
            .await?;
        return Ok((
            "grabbed".into(),
            format!("{c_title} (lien magnet ajouté à qBittorrent, {})", arr.name),
        ));
    }
    let url = release
        .get("downloadUrl")
        .and_then(Value::as_str)
        .context("release sans lien de téléchargement")?;
    if arr.name.ends_with("seedbox") {
        return to_qbittorrent(
            ctx,
            prow,
            arr,
            url,
            c_title,
            tag,
            "Prowlarr injoignable depuis la seedbox",
        )
        .await;
    }
    let arr_url = url_for_arrs(
        url,
        &ctx.cfg.urls.prowlarr,
        &ctx.cfg.tasks.series_search.prowlarr_url_for_arrs,
    );
    let decision = arr
        .push_release(
            c_title,
            &arr_url,
            release.get("publishDate").and_then(Value::as_str),
            indexer,
        )
        .await
        .with_context(|| format!("push {c_title}"))?;
    let rejected = push_rejections(&decision);
    if !rejected.is_empty() {
        if !rejected.iter().all(|r| bypassable_rejection(r)) {
            // « blocked till … » : l'indexeur est en pause, pas un mauvais candidat → retenté dans l'heure
            let blocked = rejected
                .iter()
                .any(|r| r.to_ascii_lowercase().contains("blocked till"));
            return Ok((
                if blocked { "error" } else { "none" }.into(),
                format!("{c_title} refusé : {}", rejected.join(" ; ")),
            ));
        }
        return to_qbittorrent(ctx, prow, arr, url, c_title, tag, &rejected.join(" ; ")).await;
    }
    // accepté : il doit apparaître dans la file de l'Arr
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let queued = arr
            .queue_records()
            .await?
            .iter()
            .any(|r| target.in_queue(r));
        if queued {
            return Ok((
                "grabbed".into(),
                format!("{c_title} (confié à {})", arr.name),
            ));
        }
    }
    to_qbittorrent(
        ctx,
        prow,
        arr,
        url,
        c_title,
        tag,
        "accepté mais pas mis en file",
    )
    .await
}

struct SeasonTodo {
    series_id: i64,
    season: i64,
    latest_air: String,
    missing_numbers: HashSet<i64>,
}

async fn names_for(ctx: &TaskContext, series: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(tmdb) = series
        .get("tmdbId")
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
    {
        match ctx.jellyseerr.tv_details(tmdb).await {
            Ok(tv) => {
                for k in ["name", "originalName"] {
                    if let Some(t) = tv.get(k).and_then(Value::as_str) {
                        names.push(t.to_string());
                    }
                }
            }
            Err(e) => {
                warn!(task = "series_search", tmdb, error = %e, "jellyseerr title lookup failed")
            }
        }
    }
    if let Some(t) = series.get("title").and_then(Value::as_str) {
        names.push(t.to_string());
    }
    for a in series
        .get("alternateTitles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(t) = a.get("title").and_then(Value::as_str) {
            names.push(t.to_string());
        }
    }
    let mut seen = HashSet::new();
    names.retain(|n| seen.insert(normalize(n)));
    names
}

/// Candidats d'une saison : par identifiant TMDB, puis (rien trouvé) en texte libre avec les noms connus.
async fn season_candidates(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    indexer_id: i64,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    throttle: &mut Throttle,
) -> Result<(Vec<Candidate>, &'static str)> {
    let cfg = &ctx.cfg.tasks.series_search;
    let tmdb = series.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let mut out = Vec::new();
    if tmdb > 0 {
        if !throttle.take().await || !crate::budget::take(ctx, false).await? {
            return Ok((out, "budget"));
        }
        for r in prow
            .search_by_tmdb(tmdb, Some(todo.season), indexer_id)
            .await?
        {
            if !tmdb_matches(&r, tmdb) {
                continue;
            }
            let Some(title) = r.get("title").and_then(Value::as_str) else {
                continue;
            };
            let parse = arr.parse(title).await?;
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if let Some(c) = series_candidate(&r, &info, todo.season) {
                out.push(c);
            }
        }
        if !out.is_empty() {
            return Ok((out, "tmdb"));
        }
    }
    // secours : série sans identifiant TMDB, ou identifiant absent des releases de l'indexer
    let names = names_for(ctx, series).await;
    let mut seen: HashSet<String> = HashSet::new();
    for name in names.iter().take(cfg.text_queries) {
        if !throttle.take().await || !crate::budget::take(ctx, false).await? {
            break;
        }
        for r in prow.search(name, indexer_id, 100).await? {
            let Some(title) = r.get("title").and_then(Value::as_str) else {
                continue;
            };
            if !seen.insert(title.to_string()) {
                continue;
            }
            // une release portant l'identifiant d'une autre œuvre est écartée d'office
            if tmdb > 0
                && r.get("tmdbId")
                    .and_then(Value::as_i64)
                    .is_some_and(|t| t > 0 && t != tmdb)
            {
                continue;
            }
            let parse = arr.parse(title).await?;
            match parsed_series(&parse) {
                Some(p) if title_matches(&p.title, &names) => {}
                _ => continue,
            }
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if let Some(c) = series_candidate(&r, &info, todo.season) {
                out.push(c);
            }
        }
    }
    Ok((out, "texte"))
}

#[allow(clippy::too_many_arguments)]
async fn process_season(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    indexer_id: i64,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    throttle: &mut Throttle,
) -> Result<(String, String)> {
    let cfg = &ctx.cfg.tasks.series_search;
    let title = series.get("title").and_then(Value::as_str).unwrap_or("?");
    let (cands, how) =
        season_candidates(ctx, prow, indexer_id, arr, series, todo, throttle).await?;
    if how == "budget" {
        return Ok(("pending".into(), String::new()));
    }
    let episodes = arr.episodes(todo.series_id).await?;
    let season_eps: Vec<&Value> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(todo.season))
        .collect();
    let want_pack = todo.missing_numbers.len() * 2 >= season_eps.len().max(1);
    let profile_id = series
        .get("qualityProfileId")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let allowed = allowed_qualities(&arr.quality_profile(profile_id).await?);
    let ix = &ctx.cfg.indexers;
    let pick = |pack: bool| {
        choose(
            &cands,
            todo.season,
            pack,
            &todo.missing_numbers,
            &allowed,
            ix.max_gb_per_episode,
            ix.allow_no_french,
        )
    };
    let chosen = pick(want_pack).or_else(|| pick(!want_pack));
    let Some(c) = chosen else {
        return Ok((
            "none".into(),
            format!(
                "{} candidat(s) {} (recherche {how}), aucun acceptable",
                cands.len(),
                cfg.indexer
            ),
        ));
    };
    if ctx.dry_run {
        info!(task = "series_search", service = arr.name, series = title, season = todo.season, release = %c.title, how, "dry-run: would send");
        return Ok(("dry_run".into(), format!("{} (recherche {how})", c.title)));
    }
    let target = Target::Season {
        series_id: todo.series_id,
        season: todo.season,
    };
    let (mut outcome, detail) =
        send_release(ctx, prow, arr, &c.title, &c.release, &cfg.indexer, target).await?;
    if outcome == "grabbed" && !c.full_season {
        outcome = "grabbed_episode".into();
    }
    info!(task = "series_search", service = arr.name, series = title, season = todo.season, release = %c.title, how, %outcome, %detail, "season sent");
    Ok((outcome, detail))
}

async fn plan_seasons(ctx: &TaskContext, arr: &ArrClient) -> Result<Vec<SeasonTodo>> {
    let cfg = &ctx.cfg.tasks.series_search;
    let now_iso = chrono::Utc::now().to_rfc3339();
    let mut by: BTreeMap<(i64, i64), SeasonTodo> = BTreeMap::new();
    for e in arr.wanted_missing().await? {
        let (Some(sid), Some(season), Some(num)) = (
            e.get("seriesId").and_then(Value::as_i64),
            e.get("seasonNumber").and_then(Value::as_i64),
            e.get("episodeNumber").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let air = e
            .get("airDateUtc")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if season == 0 || air.is_empty() || air > now_iso {
            continue;
        }
        let t = by.entry((sid, season)).or_insert(SeasonTodo {
            series_id: sid,
            season,
            latest_air: String::new(),
            missing_numbers: HashSet::new(),
        });
        t.missing_numbers.insert(num);
        if air > t.latest_air {
            t.latest_air = air;
        }
    }
    let queued: HashSet<(i64, i64)> = arr
        .queue_records()
        .await?
        .iter()
        .filter_map(|r| {
            Some((
                r.get("seriesId").and_then(Value::as_i64)?,
                r.get("seasonNumber")
                    .and_then(Value::as_i64)
                    .or_else(|| r.pointer("/episode/seasonNumber").and_then(Value::as_i64))?,
            ))
        })
        .collect();
    let records = ctx.state.read(|s| s.unknown_series.clone()).await;
    let t = now();
    Ok(by
        .into_values()
        .filter(|s| !queued.contains(&(s.series_id, s.season)))
        .filter(|s| {
            due(
                records.get(&key(arr, s.series_id, s.season)),
                t,
                cfg.retry_after_hours,
                cfg.grabbed_retry_hours,
                cfg.error_retry_hours,
            )
        })
        .collect())
}

fn key(arr: &ArrClient, series: i64, season: i64) -> String {
    format!("{}:{series}:{season}", arr.name)
}

#[async_trait]
impl Task for SeriesSearch {
    fn name(&self) -> &'static str {
        "series_search"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.series_search.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.series_search;
        let Some(prow) = &ctx.prowlarr else {
            return Ok(Report::new("prowlarr non configuré (PROWLARR_API_KEY)", 0));
        };
        let Some(indexer_id) = prow.indexer_id(&cfg.indexer).await? else {
            return Ok(Report::new(
                format!("indexer « {} » absent de Prowlarr", cfg.indexer),
                0,
            ));
        };
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        struct Ctx<'a> {
            arr: &'a ArrClient,
            series: HashMap<i64, Value>,
        }
        let mut arrs: Vec<Ctx> = Vec::new();
        let mut all: Vec<(usize, SeasonTodo)> = Vec::new();
        for arr in ctx.all_sonarr() {
            let prepared = async {
                let todo = plan_seasons(ctx, arr).await?;
                let series: HashMap<i64, Value> = arr
                    .series()
                    .await?
                    .into_iter()
                    .filter_map(|s| Some((s.get("id").and_then(Value::as_i64)?, s)))
                    .collect();
                anyhow::Ok((todo, series))
            }
            .await;
            match prepared {
                Ok((todo, series)) => {
                    let i = arrs.len();
                    all.extend(todo.into_iter().map(|t| (i, t)));
                    arrs.push(Ctx { arr, series });
                }
                Err(e) => {
                    warn!(task = "series_search", service = arr.name, error = %e, "planning failed")
                }
            }
        }
        // saisons déjà présentes sur l'autre machine : jamais prises ici (doublon Jellyfin)
        let files: Vec<BTreeMap<i64, std::collections::BTreeSet<i64>>> = arrs
            .iter()
            .map(|c| {
                let list: Vec<Value> = c.series.values().cloned().collect();
                super::monitor_sync::seasons_with_files(&list)
            })
            .collect();
        let before = all.len();
        all.retain(|(i, t)| {
            let tvdb = arrs[*i]
                .series
                .get(&t.series_id)
                .and_then(|s| s.get("tvdbId").and_then(Value::as_i64))
                .unwrap_or(0);
            !files.iter().enumerate().any(|(j, f)| {
                j != *i && f.get(&tvdb).map(|s| s.contains(&t.season)).unwrap_or(false)
            })
        });
        if before > all.len() {
            counts.insert("dup_other_side".into(), (before - all.len()) as u32);
        }
        let entries: Vec<(String, String)> = all
            .iter()
            .map(|(i, t)| {
                let added = arrs[*i]
                    .series
                    .get(&t.series_id)
                    .and_then(|s| s.get("added").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                (added, t.latest_air.clone())
            })
            .collect();
        let mut throttle = Throttle::new(cfg.max_queries_per_run, cfg.query_gap_secs);
        for idx in pick_order(&entries) {
            if throttle.remaining() == 0 {
                *counts.entry("pending".into()).or_default() += 1;
                continue;
            }
            let (i, s) = &all[idx];
            let c = &arrs[*i];
            let Some(ser) = c.series.get(&s.series_id) else {
                continue;
            };
            let (outcome, detail) = match process_season(
                ctx,
                prow,
                indexer_id,
                c.arr,
                ser,
                s,
                &mut throttle,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(task = "series_search", service = c.arr.name, series_id = s.series_id, season = s.season, error = %e, "season failed");
                    ("error".into(), format!("{e:#}").chars().take(200).collect())
                }
            };
            *counts.entry(outcome.clone()).or_default() += 1;
            if outcome == "pending" {
                continue;
            }
            let sname = ser.get("title").and_then(Value::as_str).unwrap_or("?");
            info!(task = "series_search", service = c.arr.name, series = sname, season = s.season, %outcome, %detail, "season searched");
            if !ctx.dry_run {
                let rec = SeasonSearchRecord {
                    at: now(),
                    outcome,
                    detail,
                };
                let k = key(c.arr, s.series_id, s.season);
                ctx.state
                    .update(|st| st.unknown_series.insert(k, rec))
                    .await?;
            }
        }
        let grabbed = counts.get("grabbed").copied().unwrap_or(0)
            + counts.get("grabbed_episode").copied().unwrap_or(0);
        let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Ok(Report::new(summary.join(" "), grabbed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(title: &str, tmdb: i64, seeders: i64) -> Value {
        json!({"title": title, "downloadUrl": "http://p/dl", "tmdbId": tmdb, "seeders": seeders,
               "publishDate": "2026-09-01T00:00:00Z"})
    }

    fn info(season: i64, full: bool, eps: &[i64], qid: i64, res: i64) -> Value {
        json!({"seasonNumber": season, "fullSeason": full, "episodeNumbers": eps,
               "quality": {"quality": {"id": qid, "resolution": res}}})
    }

    #[test]
    fn language_ranks() {
        assert_eq!(lang_rank("L.Attaque.Des.Titans.S04.MULTI.VFF.1080p"), 4);
        assert_eq!(lang_rank("Show.S01.MULTi.1080p"), 3);
        assert_eq!(lang_rank("Show.S01.FRENCH.720p"), 2);
        assert_eq!(lang_rank("Show.S01E02.VOSTFR.1080p"), 1);
        assert_eq!(lang_rank("Shingeki.no.Kyojin.S04.1080p.WEB.x264"), 0, "VO");
    }

    #[test]
    fn releases_are_kept_by_tmdb_id_whatever_their_name() {
        // même série sous ses trois noms : seul l'identifiant compte
        assert!(tmdb_matches(
            &result("L.Attaque.Des.Titans.S04.VFF", 1429, 5),
            1429
        ));
        assert!(tmdb_matches(
            &result("Shingeki.no.Kyojin.S04.MULTI", 1429, 5),
            1429
        ));
        // le spin-off Junior High School a un autre identifiant
        assert!(!tmdb_matches(
            &result("L.Attaque.Des.Titans.Junior.High.VFF", 63510, 5),
            1429
        ));
        assert!(!tmdb_matches(&json!({"title": "sans id"}), 1429));
        assert!(!tmdb_matches(&result("x", 0, 5), 0));
    }

    #[test]
    fn candidate_needs_the_season_and_french() {
        let r = result(
            "L.Attaque.Des.Titans.S04.MULTI.VFF.1080p.BluRay.x264",
            1429,
            65,
        );
        let c = series_candidate(&r, &info(4, true, &[], 9, 1080), 4).unwrap();
        assert!(c.full_season && c.h264 && c.lang_rank == 4 && c.resolution == 1080);
        assert!(series_candidate(&r, &info(3, true, &[], 9, 1080), 4).is_none());
        // VO : gardée avec le rang 0 (prise seulement en dernier recours par `choose`)
        let vo = result("Shingeki.no.Kyojin.S04.1080p.WEB", 1429, 65);
        assert_eq!(
            series_candidate(&vo, &info(4, true, &[], 9, 1080), 4)
                .unwrap()
                .lang_rank,
            0
        );
    }

    #[test]
    fn chooses_best_pack_within_limits() {
        let mk = |t: &str, res: i64, qid: i64, seeders: i64| {
            series_candidate(&result(t, 1, seeders), &info(4, true, &[], qid, res), 4).unwrap()
        };
        let cands = vec![
            mk("A.S04.VFF.720p.HDTV.x264", 720, 4, 11),
            mk("A.S04.MULTi.1080p.WEB.x265", 1080, 9, 30),
            mk("A.S04.VFF.1080p.WEB.x264", 1080, 9, 3),
            mk("A.S04.VFF.2160p.WEB", 2160, 18, 50),
            mk("A.S04.VFF.1080p.dead", 1080, 9, 0),
        ];
        let allowed: HashSet<i64> = [4, 9].into_iter().collect();
        let missing: HashSet<i64> = (1..=30).collect();
        assert_eq!(
            choose(&cands, 4, true, &missing, &allowed, 6.0, true)
                .unwrap()
                .title,
            "A.S04.VFF.1080p.WEB.x264"
        );
        let only720: HashSet<i64> = [4].into_iter().collect();
        assert_eq!(
            choose(&cands, 4, true, &missing, &only720, 6.0, true)
                .unwrap()
                .title,
            "A.S04.VFF.720p.HDTV.x264"
        );
    }

    #[test]
    fn vo_only_as_a_last_resort_and_size_capped() {
        let big = json!({"title": "A.S04.VFF.1080p.BluRay.x264", "downloadUrl": "http://p/dl", "tmdbId": 1,
                         "seeders": 1, "size": 134_000_000_000i64});
        let mk = |v: Value, res: i64, qid: i64| {
            series_candidate(&v, &info(4, true, &[], qid, res), 4).unwrap()
        };
        let vo = json!({"title": "A.S04.1080p.BluRay.x265", "downloadUrl": "http://p/dl", "tmdbId": 1,
                        "seeders": 39, "size": 27_800_000_000i64});
        let cands = vec![mk(big, 1080, 9), mk(vo, 1080, 9)];
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let missing: HashSet<i64> = (1..=26).collect();
        // 134 Go pour 26 épisodes = 4,8 Gio/épisode : sous le plafond de 6, le français gagne
        assert_eq!(
            choose(&cands, 4, true, &missing, &allowed, 6.0, true)
                .unwrap()
                .lang_rank,
            4
        );
        // plafond serré : le pack français est écarté, la VO est prise en dernier recours
        assert_eq!(
            choose(&cands, 4, true, &missing, &allowed, 2.0, true)
                .unwrap()
                .lang_rank,
            0
        );
        // sans autorisation VO : rien
        assert!(choose(&cands, 4, true, &missing, &allowed, 2.0, false).is_none());
        assert!(size_ok(134_000_000_000, 26, 6.0), "4,8 Gio par épisode");
        assert!(!size_ok(134_000_000_000, 26, 4.0));
        assert!(!size_ok(30_000_000_000, 1, 25.0), "film de 28 Gio");
        assert!(size_ok(0, 1, 6.0), "taille inconnue : on ne bloque pas");
    }

    #[test]
    fn one_seeder_loses_to_a_healthy_release() {
        let mk = |t: &str, seeders: i64, qid: i64| {
            series_candidate(&result(t, 1, seeders), &info(4, true, &[], qid, 1080), 4).unwrap()
        };
        let cands = vec![
            mk("A.S04.VFF.1080p.x264", 1, 9),
            mk("A.S04.VFF.1080p.x265", 39, 9),
        ];
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let missing: HashSet<i64> = (1..=10).collect();
        assert_eq!(
            choose(&cands, 4, true, &missing, &allowed, 6.0, true)
                .unwrap()
                .title,
            "A.S04.VFF.1080p.x265"
        );
    }

    #[test]
    fn single_episodes_must_be_missing() {
        let mk = |t: &str, ep: i64| {
            series_candidate(&result(t, 1, 4), &info(2, false, &[ep], 9, 1080), 2).unwrap()
        };
        let cands = vec![mk("A.S02E03.VFF.1080p", 3), mk("A.S02E04.VFF.1080p", 4)];
        let missing: HashSet<i64> = [4].into_iter().collect();
        assert_eq!(
            choose(&cands, 2, false, &missing, &HashSet::new(), 6.0, true)
                .unwrap()
                .title,
            "A.S02E04.VFF.1080p"
        );
    }

    #[test]
    fn new_requests_first() {
        let e = |added: &str, air: &str| (added.to_string(), air.to_string());
        let entries = vec![
            e("2026-05-01", "2026-09-10"), // arriéré récent
            e("2026-09-13", "2008-10-01"), // demandé aujourd'hui, saison 1
            e("2026-09-13", "2015-02-01"), // demandé aujourd'hui, saison 7
        ];
        assert_eq!(pick_order(&entries), vec![2, 1, 0]);
    }

    #[test]
    fn retry_windows() {
        let r = |outcome: &str, at: i64| SeasonSearchRecord {
            at,
            outcome: outcome.into(),
            detail: String::new(),
        };
        assert!(due(None, 1000, 24, 168, 1));
        assert!(!due(Some(&r("none", 0)), 23 * 3600, 24, 168, 1));
        assert!(due(Some(&r("none", 0)), 24 * 3600, 24, 168, 1));
        assert!(!due(Some(&r("grabbed", 0)), 100 * 3600, 24, 168, 1));
        assert!(!due(Some(&r("error", 0)), 1800, 24, 168, 1));
        assert!(!due(Some(&r("grabbed_episode", 0)), 3600, 24, 168, 1));
        assert!(due(Some(&r("grabbed_episode", 0)), 2 * 3600, 24, 168, 1));
        assert!(due(Some(&r("error", 0)), 3600, 24, 168, 1));
    }

    #[test]
    fn only_identification_or_indexer_refusals_go_to_qbittorrent() {
        assert!(bypassable_rejection("Unknown Series"));
        assert!(bypassable_rejection(
            "Indexer C411 is blocked till 09/17/2026 19:33:45 due to failures, cannot grab release."
        ));
        assert!(!bypassable_rejection("Release is blocklisted"));
        assert!(!bypassable_rejection(
            "65.7 GB is larger than maximum allowed 40 GB"
        ));
        let d = json!([{"approved": false, "rejections": ["Unknown Series"]}]);
        assert_eq!(push_rejections(&d), vec!["Unknown Series".to_string()]);
        assert!(push_rejections(&json!([{"approved": true, "rejections": []}])).is_empty());
    }

    #[test]
    fn allowed_qualities_include_groups() {
        let p = json!({"items": [
            {"allowed": true, "quality": {"id": 4}},
            {"allowed": false, "quality": {"id": 18}},
            {"allowed": true, "name": "WEB 1080p", "items": [{"quality": {"id": 3}}, {"quality": {"id": 15}}]}
        ]});
        let a = allowed_qualities(&p);
        assert!(a.contains(&4) && a.contains(&3) && a.contains(&15) && !a.contains(&18));
    }

    #[tokio::test]
    async fn throttle_stops_at_budget() {
        let mut t = Throttle::new(2, 0);
        assert!(t.take().await && t.take().await);
        assert!(!t.take().await);
        assert_eq!(t.remaining(), 0);
    }

    #[test]
    fn download_links_are_rewritten_for_the_arr_containers() {
        let u = "http://localhost:9696/4/download?apikey=x&link=abc";
        assert_eq!(
            url_for_arrs(u, "http://localhost:9696", "http://prowlarr:9696"),
            "http://prowlarr:9696/4/download?apikey=x&link=abc"
        );
        assert_eq!(
            url_for_arrs(u, "http://localhost:9696/", "http://prowlarr:9696/"),
            "http://prowlarr:9696/4/download?apikey=x&link=abc"
        );
        assert_eq!(
            url_for_arrs(
                "http://ailleurs/x",
                "http://localhost:9696",
                "http://prowlarr:9696"
            ),
            "http://ailleurs/x"
        );
        assert_eq!(url_for_arrs(u, "http://localhost:9696", ""), u);
    }

    #[test]
    fn queue_is_matched_by_ids_not_titles() {
        let t = Target::Season {
            series_id: 17,
            season: 12,
        };
        assert!(t.in_queue(&json!({"title": "Bleach.S12.MULTi.1080p.WEB.H264-UwU", "seriesId": 17, "seasonNumber": 12})));
        assert!(t.in_queue(&json!({"seriesId": 17, "episode": {"seasonNumber": 12}})));
        assert!(!t.in_queue(&json!({"seriesId": 17, "seasonNumber": 13})));
        assert_eq!(t.tag(), "homelab:series=17:season=12");
        let m = Target::Movie { movie_id: 66 };
        assert!(m.in_queue(&json!({"movieId": 66})) && !m.in_queue(&json!({"movieId": 67})));
        assert_eq!(m.tag(), "homelab:movie=66");
    }
}
