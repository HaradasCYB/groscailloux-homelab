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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

/// Mots qui trahissent une œuvre **dérivée** (mini-série, spéciaux, parodie…). Présents dans le titre de la
/// release mais absents des titres de la fiche, ils veulent dire « ce n'est pas la série demandée » : le
/// 2026-09-17, *Smoking Behind the Supermarket with You (Mini Episodes)* — 12 min par épisode — a été pris
/// pour la série officielle (25 min).
pub fn derivative(title: &str, names: &[String]) -> Option<&'static str> {
    const WORDS: [&str; 10] = [
        "mini", "specials", "special", "ova", "oad", "recap", "abridged", "junior", "shorts",
        "chibi",
    ];
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let in_names = |w: &str| {
        names.iter().any(|n| {
            n.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|x| x.eq_ignore_ascii_case(w))
        })
    };
    WORDS
        .into_iter()
        .find(|w| words.iter().any(|x| x == w) && !in_names(w))
}

pub fn is_h264(title: &str) -> bool {
    let t = title.to_ascii_uppercase();
    !(t.contains("265") || t.contains("HEVC"))
}

/// La release porte-t-elle l'identifiant TMDB de l'œuvre cherchée ? (C411 renvoie `tmdbId` sur chacune.)
pub fn tmdb_matches(release: &Value, tmdb_id: i64) -> bool {
    tmdb_id > 0 && release.get("tmdbId").and_then(Value::as_i64) == Some(tmdb_id)
}

/// Épisodes manquants qu'aucun candidat ne couvre. Un pack de saison les couvre tous.
///
/// Sert à décider si la recherche par identifiant suffit : C411 peut très bien renvoyer 41 releases
/// pour une saison et n'en couvrir que les deux tiers (Bleach S17 le 2026-09-18 : E01–26 et E41–48
/// par identifiant, E27–40 seulement sous le titre du cours « Thousand-Year Blood War »).
pub fn uncovered(cands: &[Candidate], missing: &HashSet<i64>) -> BTreeSet<i64> {
    if cands.iter().any(|c| c.full_season) {
        return BTreeSet::new();
    }
    let mut left: BTreeSet<i64> = missing.iter().copied().collect();
    for c in cands {
        for e in &c.episodes {
            left.remove(e);
        }
    }
    left
}

/// Numéro d'épisode annoncé par le nom d'un fichier de pack : `...S03E07...` → 7.
///
/// Volontairement strict : uniquement la forme `SxxEyy`. Un pack de cours numérote toujours ses fichiers
/// ainsi ; tout le reste (numéro absolu, nom libre) est déjà rattaché correctement par Sonarr et n'a pas
/// besoin de cette mécanique — mieux vaut refuser que deviner.
pub fn claimed_episode(path: &str) -> Option<i64> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)s\d{1,3}e(\d{1,3})").expect("regex valide"));
    let base = path.rsplit('/').next().unwrap_or(path);
    re.captures(base)?.get(1)?.as_str().parse().ok()
}

/// Correspondance « fichier du pack → épisode de la saison cible », quand elle est **certaine**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapping {
    /// À ajouter au numéro annoncé par le fichier pour obtenir l'épisode de la saison cible.
    pub offset: i64,
    pub first: i64,
    pub last: i64,
    pub files: usize,
}

/// Un cours d'animé publié sous son propre titre (« Thousand-Year Blood War S03 ») couvre-t-il **exactement**
/// le trou de la saison ? `None` = on ne touche à rien, c'est le cas par défaut.
///
/// Toutes les conditions sont obligatoires : le trou doit être d'un seul tenant, le pack doit contenir
/// autant de fichiers vidéo que d'épisodes manquants, ces fichiers doivent être numérotés en suite continue,
/// et aucun épisode visé ne doit déjà avoir un fichier. Au moindre écart on refuse : se tromper ici
/// écraserait une vraie saison (Sonarr lit « S03E01 » comme la saison 3 de Bleach).
pub fn offset_mapping(
    video_paths: &[String],
    missing: &BTreeSet<i64>,
    have_file: &BTreeSet<i64>,
) -> Option<Mapping> {
    let (first, last) = (*missing.iter().next()?, *missing.iter().next_back()?);
    // 1. trou d'un seul tenant
    if last - first + 1 != missing.len() as i64 {
        return None;
    }
    // 2. chaque fichier doit annoncer un numéro
    let mut nums: Vec<i64> = Vec::with_capacity(video_paths.len());
    for p in video_paths {
        nums.push(claimed_episode(p)?);
    }
    // 3. suite continue, sans doublon
    nums.sort_unstable();
    nums.dedup();
    if nums.len() != video_paths.len() {
        return None;
    }
    let lo = *nums.first()?;
    if nums.last()? - lo + 1 != nums.len() as i64 {
        return None;
    }
    // 4. le pack couvre exactement le trou
    if nums.len() != missing.len() {
        return None;
    }
    // 5. jamais de remplacement
    if (first..=last).any(|e| have_file.contains(&e)) {
        return None;
    }
    // 6. décalage vers l'avant seulement
    let offset = first - lo;
    if offset < 0 {
        return None;
    }
    Some(Mapping {
        offset,
        first,
        last,
        files: video_paths.len(),
    })
}

/// Pack d'un cours : la même série d'après l'Arr, mais rangé sous une autre saison.
#[derive(Debug, Clone)]
pub struct CourPack {
    pub release: Value,
    pub title: String,
    pub seeders: i64,
    pub size: i64,
    pub lang_rank: u8,
}

/// Cette release est-elle le pack d'un cours de **notre** saison, publié sous son propre titre ?
///
/// Conditions toutes obligatoires : l'Arr rattache la release à **cette** fiche, il en lit une **autre**
/// saison, c'est un pack complet, et — le point qui manque à l'intuition — il ne sait **pas déjà** la
/// mapper sur la saison cible. Ce dernier point écarte « Thousand-Year Blood War **S01** », que le scene
/// mapping TVDB traduit déjà en saison 17 (50/50 épisodes, mesuré le 2026-09-18) : le prendre par ce
/// chemin lui inventerait un décalage et le placerait de travers.
pub fn cour_pack(result: &Value, parse: &Value, series_id: i64, season: i64) -> Option<CourPack> {
    if parse.pointer("/series/id").and_then(Value::as_i64) != Some(series_id) {
        return None;
    }
    let info = parse.get("parsedEpisodeInfo")?;
    let read = info.get("seasonNumber").and_then(Value::as_i64)?;
    if read == season || read <= 0 {
        return None;
    }
    if info.get("fullSeason").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    // l'Arr sait déjà viser la bonne saison : ce n'est pas notre affaire
    if parse
        .get("episodes")
        .and_then(Value::as_array)
        .is_some_and(|e| {
            e.iter()
                .any(|x| x.get("seasonNumber").and_then(Value::as_i64) == Some(season))
        })
    {
        return None;
    }
    let title = result.get("title").and_then(Value::as_str)?.to_string();
    let url = result.get("downloadUrl").and_then(Value::as_str)?;
    Some(CourPack {
        lang_rank: lang_rank(&title),
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
            "quality": info.get("quality").cloned().unwrap_or(Value::Null),
            "infoHash": result.get("infoHash"),
        }),
        title,
    })
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
            // sert à retrouver un torrent déjà présent dans qBittorrent (titre re-demandé)
            "infoHash": result.get("infoHash"),
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
///
/// **Le codec ne départage rien** (mesuré le 2026-09-18) : h264 et HEVC transcodent à la même vitesse
/// sur ce serveur (1,85× tous les deux, le coût est l'encodage x264 et pas le décodage), et les clients
/// des membres lisent le HEVC en direct. Écarter le x265 revenait à refuser la seule version française
/// disponible, ce qui est le cas courant des animés sur C411.
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
            .max_by_key(|c| (c.lang_rank, c.resolution, c.seeders >= 2, c.seeders))
    };
    best(false).or_else(|| if allow_vo { best(true) } else { None })
}

/// Une release par épisode manquant, prise dans le **même lot de résultats** (aucune requête de plus) :
/// sans pack de saison, c'est ce qui permet de récupérer une saison entière d'un coup. Mêmes règles que
/// `choose` ; au plus `max` releases, les épisodes les plus anciens d'abord.
pub fn choose_episodes<'a>(
    cands: &'a [Candidate],
    season: i64,
    missing: &HashSet<i64>,
    allowed: &HashSet<i64>,
    max_gb: f64,
    allow_vo: bool,
    max: usize,
) -> Vec<&'a Candidate> {
    let mut wanted: Vec<i64> = missing.iter().copied().collect();
    wanted.sort_unstable();
    let mut out = Vec::new();
    for ep in wanted {
        if out.len() >= max {
            break;
        }
        let one: HashSet<i64> = [ep].into_iter().collect();
        if let Some(c) = choose(cands, season, false, &one, allowed, max_gb, allow_vo) {
            // une release couvrant plusieurs épisodes ne doit pas être prise deux fois
            if !out.iter().any(|x: &&Candidate| x.title == c.title) {
                out.push(c);
            }
        }
    }
    out
}

/// Faut-il (re)chercher ? Une erreur (indexeur indisponible, délai dépassé) est retentée vite.
pub fn due(
    rec: Option<&SeasonSearchRecord>,
    now: i64,
    retry_h: i64,
    grabbed_h: i64,
    error_h: i64,
    episode_mins: i64,
) -> bool {
    match rec {
        None => true,
        Some(r) => {
            let wait_secs = match r.outcome.as_str() {
                "grabbed" => grabbed_h * 3600,
                // des épisodes viennent d'être pris : on reprend vite, le temps de l'import
                "grabbed_episode" => (grabbed_h * 3600).min(episode_mins * 60),
                "error" => error_h * 3600,
                _ => retry_h * 3600,
            };
            now - r.at >= wait_secs
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

/// Fiche ajoutée il y a moins de `hours` : c'est une demande fraîche, elle passe devant.
pub fn is_fresh(added: &str, now: i64, hours: i64) -> bool {
    chrono::DateTime::parse_from_rfc3339(added)
        .map(|d| now - d.timestamp() < hours * 3600)
        .unwrap_or(false)
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

/// Torrent déjà présent dans ce qBittorrent, complet, portant cet `infoHash`.
///
/// Cas courant depuis que `deletion_cleanup` garde les torrents en partage : le titre est supprimé de
/// Jellyfin (fiche, demande et **fichiers** effacés) mais ses torrents restent pour tenir le ratio C411.
/// Redemandé, `qbit.add_torrent` répond « Fails. » (déjà présent) et plus rien n'avançait — le
/// 2026-09-18 sur Bleach S17, 0/20 épisodes pris. Les données sont pourtant intactes : on rattache le
/// torrent à la nouvelle fiche et `torrent_import` l'importe **sans rien retélécharger**.
async fn already_there(qbit: &QbitClient, info_hash: &str) -> Option<crate::clients::Torrent> {
    if info_hash.is_empty() {
        return None;
    }
    qbit.torrents()
        .await
        .ok()?
        .into_iter()
        .find(|t| t.hash.eq_ignore_ascii_case(info_hash) && t.progress >= 1.0)
}

/// Ce qu'il faut pour confier une release à qBittorrent.
struct Grab<'a> {
    /// Lien de téléchargement Prowlarr.
    url: &'a str,
    /// Titre de la release (journaux, détail).
    title: &'a str,
    /// Étiquette `homelab:` lue par `torrent_import`.
    tag: &'a str,
    /// Empreinte du torrent : sert à retrouver celui qui serait déjà là.
    info_hash: &'a str,
}

async fn to_qbittorrent(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    g: &Grab<'_>,
    why: &str,
) -> Result<(String, String)> {
    let (url, c_title, tag, info_hash) = (g.url, g.title, g.tag, g.info_hash);
    let qbit = qbit_for(ctx, arr).context("aucun qBittorrent pour ce côté")?;
    if let Some(t) = already_there(qbit, info_hash).await {
        qbit.retag(&t.hash, &t.tags, tag).await?;
        // le torrent avait été importé sous l'ancienne fiche : sans ça `torrent_import` le laisse
        if !ctx.dry_run {
            let key = super::torrent_import::state_key(side_name(arr), &t.hash);
            ctx.state
                .update(|s| s.torrent_import.remove(&key))
                .await
                .ok();
        }
        info!(task = "series_search", service = arr.name, release = %c_title, hash = %t.hash, "torrent déjà présent : rattaché à la nouvelle fiche");
        return Ok((
            "grabbed".into(),
            format!("{c_title} (déjà dans qBittorrent, rattaché à la nouvelle fiche)"),
        ));
    }
    let torrent = prow.download(url).await?;
    qbit.add_torrent(torrent, "", tag).await?;
    Ok((
        "grabbed".into(),
        format!("{c_title} (ajouté à qBittorrent, {} : {why})", arr.name),
    ))
}

/// `sonarr-seedbox` → `seedbox` (clé d'état de `torrent_import`).
fn side_name(arr: &ArrClient) -> &'static str {
    if arr.name.ends_with("seedbox") {
        "seedbox"
    } else {
        "vps"
    }
}

/// Confie la release. Arr du VPS : `release/push` avec un lien qu'il peut joindre, puis vérification qu'il
/// l'a bien mise en file (un envoi accepté peut échouer au téléchargement sans le dire). Arr de la seedbox
/// (Prowlarr du VPS injoignable), refus d'identification, indexeur bloqué ou téléchargement non pris : le
/// `.torrent` va au qBittorrent du même côté avec l'étiquette `tag`, lue par `torrent_import`.
/// Ce qu'on télécharge : une saison d'une série, ou un film (fiche de l'Arr).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Season {
        series_id: i64,
        season: i64,
    },
    /// Pack d'un cours d'animé publié sous son propre titre : les fichiers sont numérotés à partir de 1,
    /// l'épisode visé vaut `numéro + offset` dans `season`, et doit tomber dans `from..=to`.
    CourPack {
        series_id: i64,
        season: i64,
        offset: i64,
        from: i64,
        to: i64,
    },
    Movie {
        movie_id: i64,
    },
}

/// Une cible que l'Arr identifierait de travers ne lui est **jamais** proposée : il accepterait le pack
/// comme la saison qu'il croit lire, l'importerait seul et écraserait une vraie saison.
pub fn goes_straight_to_qbittorrent(t: &Target) -> bool {
    matches!(t, Target::CourPack { .. })
}

impl Target {
    /// Étiquette qBittorrent lue par `torrent_import`.
    pub fn tag(&self) -> String {
        match self {
            Target::Season { series_id, season } => {
                format!("homelab:series={series_id}:season={season}")
            }
            Target::CourPack {
                series_id,
                season,
                offset,
                from,
                to,
            } => format!(
                "homelab:series={series_id}:season={season}:offset={offset}:eps={from}-{to}"
            ),
            Target::Movie { movie_id } => format!("homelab:movie={movie_id}"),
        }
    }

    /// L'élément de file d'attente de l'Arr concerne-t-il cette cible ? (Le titre de la file est le nom
    /// interne du torrent, souvent différent du titre de la release : on compare les identifiants.)
    pub fn in_queue(&self, record: &Value) -> bool {
        let id = |k: &str| record.get(k).and_then(Value::as_i64);
        match self {
            Target::Season { series_id, season }
            | Target::CourPack {
                series_id, season, ..
            } => {
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
    let info_hash = release
        .get("infoHash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let grab = Grab {
        url,
        title: c_title,
        tag,
        info_hash,
    };
    if goes_straight_to_qbittorrent(&target) {
        return to_qbittorrent(
            ctx,
            prow,
            arr,
            &grab,
            "pack d'un cours : import avec décalage",
        )
        .await;
    }
    if arr.name.ends_with("seedbox") {
        return to_qbittorrent(
            ctx,
            prow,
            arr,
            &grab,
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
        return to_qbittorrent(ctx, prow, arr, &grab, &rejected.join(" ; ")).await;
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
    to_qbittorrent(ctx, prow, arr, &grab, "accepté mais pas mis en file").await
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

/// Noms à essayer en priorité quand il reste un trou dans la saison : ceux qui **ajoutent** quelque chose
/// au titre principal passent devant.
///
/// Un cours d'animé est publié sous son propre nom (« Bleach Thousand-Year Blood War »), que Sonarr connaît
/// comme titre alternatif — mais il arrivait après « Bleach » et « BLEACH », donc `text_queries` (2) ne
/// l'atteignait jamais et le pack restait introuvable (mesuré le 2026-09-18).
pub fn gap_names(names: &[String]) -> Vec<String> {
    let Some(main) = names.first().map(|n| normalize(n)) else {
        return Vec::new();
    };
    let mut extra: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for n in names {
        let k = normalize(n);
        if k == main {
            continue;
        }
        // « bleach thousand year blood war » commence par « bleach » : c'est un sous-titre de cours
        if k.starts_with(&main) && k.len() > main.len() + 1 {
            extra.push(n.clone());
        } else {
            rest.push(n.clone());
        }
    }
    extra.extend(rest);
    extra
}

/// Candidats d'une saison : par identifiant TMDB, puis (rien trouvé) en texte libre avec les noms connus.
async fn season_candidates(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    throttle: &mut Throttle,
) -> Result<(Vec<Candidate>, Vec<CourPack>, &'static str)> {
    let cfg = &ctx.cfg.tasks.series_search;
    let tmdb = series.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let names = names_for(ctx, series).await;
    let mut out = Vec::new();
    let mut packs: Vec<CourPack> = Vec::new();
    if tmdb > 0 {
        if !throttle.take().await {
            return Ok((out, packs, "budget"));
        }
        let Some(found) =
            crate::indexer::search_tmdb(ctx, prow, tmdb, Some(todo.season), false).await?
        else {
            return Ok((out, packs, "budget"));
        };
        for r in found {
            if !tmdb_matches(&r, tmdb) {
                continue;
            }
            let Some(title) = r.get("title").and_then(Value::as_str) else {
                continue;
            };
            if let Some(w) = derivative(title, &names) {
                info!(task = "series_search", release = %title, word = w, "œuvre dérivée : écartée");
                continue;
            }
            let parse = arr.parse(title).await?;
            if let Some(p) = cour_pack(&r, &parse, todo.series_id, todo.season) {
                packs.push(p);
            }
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if let Some(c) = series_candidate(&r, &info, todo.season) {
                out.push(c);
            }
        }
        // l'identifiant suffit seulement s'il couvre TOUS les épisodes manquants ; s'il en laisse,
        // on complète en texte libre (les cours d'un animé sont souvent nommés autrement)
        let left = uncovered(&out, &todo.missing_numbers);
        if !out.is_empty() && left.is_empty() {
            return Ok((out, packs, "tmdb"));
        }
        if !out.is_empty() {
            info!(
                task = "series_search",
                series_id = todo.series_id,
                season = todo.season,
                releases = out.len(),
                sans_candidat = left.len(),
                "identifiant incomplet : complément en texte libre"
            );
        }
    }
    // complément (ou secours) : titres de la fiche en texte libre. Les releases déjà vues par
    // identifiant sont ignorées, le garde-fou de saison reste le même.
    let by_id = out.len();
    let mut seen: HashSet<String> = out.iter().map(|c| c.title.clone()).collect();
    // il reste un trou : les sous-titres de cours passent devant (voir `gap_names`)
    let order: Vec<String> = if by_id > 0 {
        let mut v = gap_names(&names);
        v.extend(names.iter().cloned());
        let mut once = HashSet::new();
        v.retain(|n| once.insert(normalize(n)));
        v
    } else {
        names.clone()
    };
    for name in order.iter().take(cfg.text_queries) {
        if !throttle.take().await {
            break;
        }
        let Some(found) = crate::indexer::search_text(ctx, prow, name, 100, false).await? else {
            break;
        };
        for r in found {
            let Some(title) = r.get("title").and_then(Value::as_str) else {
                continue;
            };
            if !seen.insert(title.to_string()) {
                continue;
            }
            if let Some(w) = derivative(title, &names) {
                info!(task = "series_search", release = %title, word = w, "œuvre dérivée : écartée");
                continue;
            }
            let other_work = tmdb > 0
                && r.get("tmdbId")
                    .and_then(Value::as_i64)
                    .is_some_and(|t| t > 0 && t != tmdb);
            let parse = arr.parse(title).await?;
            // Pack d'un cours : il porte le titre du cours (« BLEACH Thousand-Year Blood War »), et
            // l'indexer lui donne l'identifiant TMDB **du cours** (313552), pas celui de la série
            // (30984). C'est donc `parse./series/id` — la table d'alias de l'Arr — qui fait foi, et
            // la sécurité vient ensuite de la lecture du `.torrent` et de `offset_mapping`, jamais de
            // l'identifiant. Le chemin normal, lui, garde le refus par identifiant intact.
            if let Some(p) = cour_pack(&r, &parse, todo.series_id, todo.season) {
                packs.push(p);
            }
            // une release portant l'identifiant d'une autre œuvre est écartée d'office
            if other_work {
                continue;
            }
            match parsed_series(&parse) {
                Some(p) if title_matches(&p.title, &names) => {}
                _ => continue,
            }
            // la release doit être rattachée à CETTE fiche par l'Arr lui-même
            if parse
                .pointer("/series/id")
                .and_then(Value::as_i64)
                .is_some_and(|id| id != todo.series_id)
            {
                continue;
            }
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if let Some(c) = series_candidate(&r, &info, todo.season) {
                out.push(c);
            }
        }
    }
    Ok((out, packs, if by_id > 0 { "tmdb+texte" } else { "texte" }))
}

#[allow(clippy::too_many_arguments)]
/// Résultat d'un passage sur une saison : décision, détail lisible, et les épisodes manquants
/// qu'aucune release ne couvre (affichés sur `/status.html`).
struct SeasonOutcome {
    outcome: String,
    detail: String,
    uncovered: Vec<i64>,
}

impl SeasonOutcome {
    fn new(outcome: &str, detail: String, uncovered: Vec<i64>) -> Self {
        Self {
            outcome: outcome.into(),
            detail,
            uncovered,
        }
    }
}

/// Tente le pack d'un cours pour combler `gap`. `Ok(None)` = rien de sûr, on continue normalement.
///
/// Le `.torrent` est téléchargé (lecture seule chez Prowlarr, aucune annonce au tracker) pour lire la liste
/// de ses fichiers : c'est la seule façon de savoir ce qu'il contient avant de l'engager.
async fn try_cour_pack(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    packs: &[CourPack],
    gap: &BTreeSet<i64>,
) -> Result<Option<SeasonOutcome>> {
    let cfg = &ctx.cfg.tasks.series_search;
    let title = series.get("title").and_then(Value::as_str).unwrap_or("?");
    let have_file: BTreeSet<i64> = arr
        .episodes(todo.series_id)
        .await?
        .iter()
        .filter(|e| e.get("hasFile").and_then(Value::as_bool) == Some(true))
        .filter_map(|e| {
            e.get("seasonNumber").and_then(Value::as_i64).and_then(|s| {
                (s == todo.season)
                    .then(|| e.get("episodeNumber").and_then(Value::as_i64))
                    .flatten()
            })
        })
        .collect();
    // le mieux partagé d'abord ; à égalité, le français
    let mut order: Vec<&CourPack> = packs.iter().collect();
    order.sort_by_key(|p| std::cmp::Reverse((p.lang_rank, p.seeders)));
    let gap_list: Vec<i64> = gap.iter().copied().collect();
    for p in order {
        let Some(url) = p.release.get("downloadUrl").and_then(Value::as_str) else {
            continue;
        };
        let raw = match prow.download(url).await {
            Ok(b) => b,
            Err(e) => {
                warn!(task = "series_search", release = %p.title, error = %e, "pack : .torrent illisible");
                continue;
            }
        };
        let entries = match crate::torrent_file::files(&raw) {
            Ok(v) => v,
            Err(e) => {
                warn!(task = "series_search", release = %p.title, error = %e, "pack : bencode illisible");
                continue;
            }
        };
        let vids: Vec<String> = crate::torrent_file::video_paths(&entries)
            .into_iter()
            .map(str::to_string)
            .collect();
        if vids.len() > cfg.cour_max_files {
            continue;
        }
        let Some(m) = offset_mapping(&vids, gap, &have_file) else {
            info!(task = "series_search", service = arr.name, series = title, release = %p.title,
                  fichiers = vids.len(), manquants = gap.len(),
                  "pack d'un cours écarté : la correspondance n'est pas certaine");
            continue;
        };
        let detail = format!(
            "{} ({} fichiers, décalage {} → S{:02}E{:02}-E{:02})",
            p.title, m.files, m.offset, todo.season, m.first, m.last
        );
        if ctx.dry_run {
            info!(task = "series_search", service = arr.name, series = title, %detail,
                  "essai à blanc : pack d'un cours retenu");
            return Ok(Some(SeasonOutcome::new("dry_run", detail, gap_list)));
        }
        let target = Target::CourPack {
            series_id: todo.series_id,
            season: todo.season,
            offset: m.offset,
            from: m.first,
            to: m.last,
        };
        let (outcome, why) =
            send_release(ctx, prow, arr, &p.title, &p.release, &cfg.indexer, target).await?;
        info!(task = "series_search", service = arr.name, series = title, season = todo.season,
              %outcome, %detail, "pack d'un cours confié");
        return Ok(Some(SeasonOutcome::new(
            &outcome,
            format!("{detail} — {why}"),
            if outcome == "grabbed" {
                Vec::new()
            } else {
                gap_list
            },
        )));
    }
    Ok(None)
}

async fn process_season(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    throttle: &mut Throttle,
) -> Result<SeasonOutcome> {
    let cfg = &ctx.cfg.tasks.series_search;
    let title = series.get("title").and_then(Value::as_str).unwrap_or("?");
    let (cands, packs, how) = season_candidates(ctx, prow, arr, series, todo, throttle).await?;
    if how == "budget" {
        return Ok(SeasonOutcome::new("pending", String::new(), Vec::new()));
    }
    // ce que l'indexer n'a pas du tout : remonté tel quel sur /status.html
    let gap = uncovered(&cands, &todo.missing_numbers);
    let left: Vec<i64> = gap.iter().copied().collect();
    // Un cours publié sous son propre titre peut combler exactement ce trou. Animés seulement, et
    // uniquement si TOUT concorde (voir `offset_mapping`) : sinon on ne touche à rien.
    if cfg.cour_packs
        && !gap.is_empty()
        && series.get("seriesType").and_then(Value::as_str) == Some("anime")
    {
        if let Some(r) = try_cour_pack(ctx, prow, arr, series, todo, &packs, &gap).await? {
            return Ok(r);
        }
    }
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
    // pack de saison si possible ; sinon toutes les releases d'épisodes manquants, en une fois
    let pack = pick(true);
    let singles = if pack.is_some() {
        Vec::new()
    } else {
        choose_episodes(
            &cands,
            todo.season,
            &todo.missing_numbers,
            &allowed,
            ix.max_gb_per_episode,
            ix.allow_no_french,
            cfg.max_grabs_per_season,
        )
    };
    if pack.is_none() && singles.len() > 1 {
        if ctx.dry_run {
            let list: Vec<&str> = singles.iter().map(|c| c.title.as_str()).collect();
            info!(
                task = "series_search",
                service = arr.name,
                series = title,
                season = todo.season,
                releases = singles.len(),
                how,
                "dry-run: would grab every missing episode"
            );
            return Ok(SeasonOutcome::new(
                "dry_run",
                format!("{} épisode(s) : {}", singles.len(), list.join(" ; ")),
                left,
            ));
        }
        let target = Target::Season {
            series_id: todo.series_id,
            season: todo.season,
        };
        let mut ok = 0usize;
        let mut last = String::new();
        for c in &singles {
            match send_release(ctx, prow, arr, &c.title, &c.release, &cfg.indexer, target).await {
                Ok((outcome, detail)) => {
                    if outcome == "grabbed" {
                        ok += 1;
                    }
                    last = detail;
                }
                Err(e) => {
                    warn!(task = "series_search", service = arr.name, release = %c.title, error = %e, "envoi impossible");
                    last = format!("{e:#}");
                }
            }
        }
        info!(
            task = "series_search",
            service = arr.name,
            series = title,
            season = todo.season,
            grabbed = ok,
            total = singles.len(),
            how,
            "saison prise épisode par épisode"
        );
        return Ok(SeasonOutcome::new(
            if ok > 0 { "grabbed_episode" } else { "error" },
            format!("{ok}/{} épisode(s) pris ({last})", singles.len()),
            left,
        ));
    }
    let chosen = pack.or_else(|| pick(false));
    let Some(c) = chosen else {
        return Ok(SeasonOutcome::new(
            "none",
            format!(
                "{} candidat(s) {} (recherche {how}), aucun acceptable",
                cands.len(),
                cfg.indexer
            ),
            left,
        ));
    };
    if ctx.dry_run {
        info!(task = "series_search", service = arr.name, series = title, season = todo.season, release = %c.title, how, "dry-run: would send");
        return Ok(SeasonOutcome::new(
            "dry_run",
            format!("{} (recherche {how})", c.title),
            left,
        ));
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
    Ok(SeasonOutcome::new(&outcome, detail, left))
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
                cfg.episode_retry_mins,
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
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        struct Ctx<'a> {
            arr: &'a ArrClient,
            series: HashMap<i64, Value>,
        }
        let mut arrs: Vec<Ctx> = Vec::new();
        let mut all: Vec<(usize, SeasonTodo)> = Vec::new();
        // une machine hors de `[downloads] auto_sides` ne prend plus rien de neuf
        for arr in ctx
            .all_sonarr()
            .into_iter()
            .filter(|a| ctx.cfg.downloads.may_grab(a.name))
        {
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
        // une demande de moins d'une heure passe devant (tri) et ouvre des requêtes en plus
        let fresh = entries
            .iter()
            .filter(|(added, _)| is_fresh(added, now(), cfg.new_request_hours))
            .count();
        let budget = cfg.max_queries_per_run + fresh.min(cfg.max_new_per_run);
        let mut throttle = Throttle::new(budget, cfg.query_gap_secs);
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
            let res = match process_season(ctx, prow, c.arr, ser, s, &mut throttle).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(task = "series_search", service = c.arr.name, series_id = s.series_id, season = s.season, error = %e, "season failed");
                    SeasonOutcome::new(
                        "error",
                        format!("{e:#}").chars().take(200).collect(),
                        Vec::new(),
                    )
                }
            };
            let SeasonOutcome {
                outcome,
                detail,
                uncovered,
            } = res;
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
                    title: sname.to_string(),
                    uncovered,
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

    fn paths(prefix: &str, from: i64, to: i64) -> Vec<String> {
        (from..=to)
            .map(|n| format!("{prefix}.S03E{n:02}.MULTi.1080p.WEB.H264-TFA.mkv"))
            .collect()
    }

    fn parse_of(series: Option<i64>, season: i64, full: bool, mapped: &[i64]) -> Value {
        json!({
            "series": series.map(|id| json!({"id": id})),
            "parsedEpisodeInfo": {"seasonNumber": season, "fullSeason": full,
                                  "quality": {"quality": {"id": 9, "resolution": 1080}}},
            "episodes": mapped.iter().map(|s| json!({"seasonNumber": s})).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn cour_subtitles_are_tried_first_when_a_hole_remains() {
        // Ordre réel de la fiche Bleach : le titre du cours arrivait après « Bleach » et « BLEACH »,
        // donc les 2 requêtes texte ne l'atteignaient jamais.
        let names: Vec<String> = [
            "Bleach",
            "BLEACH",
            "Bleach - Thousand-Year Blood War",
            "Bleach Sennen Kessen-hen",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let first = gap_names(&names);
        assert_eq!(first[0], "Bleach - Thousand-Year Blood War");
        assert!(!first.iter().any(|n| normalize(n) == normalize("Bleach")));
        // un nom sans rapport reste disponible, mais derrière
        assert!(first.contains(&"Bleach Sennen Kessen-hen".to_string()));
    }

    #[test]
    fn a_cour_pack_is_only_taken_when_the_arr_names_our_fiche() {
        let r = result(
            "BLEACH.Thousand-Year.Blood.War.S03.MULTI.VFF.1080p.H264-TFA",
            30984,
            170,
        );
        // le cas réel : notre fiche, saison lue 3, pack complet, mappé sur la saison 3
        assert!(cour_pack(&r, &parse_of(Some(60), 3, true, &[3]), 60, 17).is_some());
        // une autre fiche, ou aucune : on ne sait rien
        assert!(cour_pack(&r, &parse_of(Some(61), 3, true, &[3]), 60, 17).is_none());
        assert!(cour_pack(&r, &parse_of(None, 3, true, &[3]), 60, 17).is_none());
        // déjà la bonne saison : le chemin normal s'en occupe
        assert!(cour_pack(&r, &parse_of(Some(60), 17, true, &[17]), 60, 17).is_none());
        // pas un pack complet
        assert!(cour_pack(&r, &parse_of(Some(60), 3, false, &[3]), 60, 17).is_none());
    }

    #[test]
    fn a_pack_the_arr_already_maps_onto_our_season_is_left_alone() {
        // « Thousand-Year Blood War S01 » : saison lue 1, mais le scene mapping TVDB le traduit déjà en
        // saison 17 (50/50 épisodes, mesuré le 2026-09-18). Lui inventer un décalage le placerait de travers.
        let r = result(
            "BLEACH.Thousand-Year.Blood.War.S01.MULTI.VFF.1080p.H264-TFA",
            30984,
            154,
        );
        assert!(cour_pack(&r, &parse_of(Some(60), 1, true, &[17, 17, 17]), 60, 17).is_none());
    }

    #[test]
    fn a_cour_pack_is_never_offered_to_the_arr() {
        // l'Arr l'accepterait comme la saison qu'il croit lire et écraserait une vraie saison
        assert!(goes_straight_to_qbittorrent(&Target::CourPack {
            series_id: 60,
            season: 17,
            offset: 26,
            from: 27,
            to: 40
        }));
        assert!(!goes_straight_to_qbittorrent(&Target::Season {
            series_id: 60,
            season: 17
        }));
        assert!(!goes_straight_to_qbittorrent(&Target::Movie {
            movie_id: 1
        }));
    }

    #[test]
    fn the_offset_tag_carries_the_mapping() {
        assert_eq!(
            Target::CourPack {
                series_id: 60,
                season: 17,
                offset: 26,
                from: 27,
                to: 40
            }
            .tag(),
            "homelab:series=60:season=17:offset=26:eps=27-40"
        );
    }

    #[test]
    fn a_cour_pack_that_fits_the_hole_exactly_is_mapped() {
        // Cas réel du 2026-09-18 : Bleach S17 manque 27-40 ; le pack « Thousand-Year Blood War S03 »
        // contient 14 fichiers numérotés S03E01 à S03E14. Décalage = 27 - 1 = 26.
        let missing: BTreeSet<i64> = (27..=40).collect();
        let m = offset_mapping(&paths("BLEACH.TYBW", 1, 14), &missing, &BTreeSet::new()).unwrap();
        assert_eq!(m.offset, 26);
        assert_eq!((m.first, m.last, m.files), (27, 40, 14));
        // le 7e fichier (S03E07) vise bien l'épisode 33
        assert_eq!(claimed_episode("X.S03E07.mkv").unwrap() + m.offset, 33);
    }

    #[test]
    fn anything_less_than_certain_is_refused() {
        let full: BTreeSet<i64> = (27..=40).collect();
        let none = BTreeSet::new();

        // 1. trou à deux morceaux : on ne sait pas où commence le pack
        let holed: BTreeSet<i64> = (27..=33).chain(35..=41).collect();
        assert!(
            offset_mapping(&paths("X", 1, 14), &holed, &none).is_none(),
            "trou non contigu"
        );

        // 2. un fichier sans numéro lisible
        let mut odd = paths("X", 1, 13);
        odd.push("X.bonus.mkv".to_string());
        assert!(
            offset_mapping(&odd, &full, &none).is_none(),
            "numéro illisible"
        );

        // 3. numéros à trou dans le pack
        let mut gap = paths("X", 1, 13);
        gap.push("X.S03E15.mkv".to_string());
        assert!(
            offset_mapping(&gap, &full, &none).is_none(),
            "suite discontinue"
        );

        // 4. le pack ne couvre pas exactement le trou (13 fichiers pour 14 épisodes)
        assert!(
            offset_mapping(&paths("X", 1, 13), &full, &none).is_none(),
            "compte différent"
        );

        // 5. un épisode visé a déjà un fichier : jamais de remplacement
        let have: BTreeSet<i64> = [33].into_iter().collect();
        assert!(
            offset_mapping(&paths("X", 1, 14), &full, &have).is_none(),
            "déjà pourvu"
        );

        // 6. décalage négatif (le pack prétend être plus loin que le trou)
        let early: BTreeSet<i64> = (1..=14).collect();
        assert!(
            offset_mapping(&paths("X", 27, 40), &early, &none).is_none(),
            "décalage négatif"
        );

        // rien ne manque
        assert!(
            offset_mapping(&paths("X", 1, 14), &none, &none).is_none(),
            "aucun manquant"
        );
    }

    #[test]
    fn a_pack_already_on_the_right_season_maps_to_itself() {
        // Sonarr saurait déjà le faire, mais le décalage 0 ne doit pas être refusé pour autant.
        let missing: BTreeSet<i64> = (1..=12).collect();
        let m = offset_mapping(&paths("X", 1, 12), &missing, &BTreeSet::new()).unwrap();
        assert_eq!(m.offset, 0);
    }

    #[test]
    fn the_info_hash_travels_with_the_release() {
        // Sans lui, un titre supprimé puis redemandé se heurte au torrent gardé en partage :
        // qBittorrent refuse « déjà présent » et rien ne s'importe (Bleach S17, le 2026-09-18).
        let mut r = result("Bleach.S17E09.MULTI.VFF.1080p.BluRay.x265-KAF", 30984, 12);
        r["infoHash"] = json!("3AEED2C5F1CD7F684F1702BC11190D937A7B5E0E");
        let c = series_candidate(&r, &info(17, false, &[9], 9, 1080), 17).unwrap();
        assert_eq!(
            c.release.get("infoHash").and_then(Value::as_str),
            Some("3AEED2C5F1CD7F684F1702BC11190D937A7B5E0E")
        );
        // release sans infoHash : le champ est présent mais nul, jamais d'erreur
        let c2 = series_candidate(
            &result("Bleach.S17E10.MULTI.VFF.1080p", 30984, 3),
            &info(17, false, &[10], 9, 1080),
            17,
        )
        .unwrap();
        assert!(c2
            .release
            .get("infoHash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty());
    }

    #[test]
    fn codec_no_longer_decides() {
        // Mesuré le 2026-09-18 : h264 et HEVC transcodent à la même vitesse sur ce serveur, et les
        // clients lisent le HEVC en direct. À langue et qualité égales, seules les sources comptent.
        let missing: HashSet<i64> = [1].into_iter().collect();
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let mk = |title: &str, seeders: i64| {
            series_candidate(
                &result(title, 30984, seeders),
                &info(1, false, &[1], 9, 1080),
                1,
            )
            .unwrap()
        };
        let cands = vec![
            mk("Show.S01E01.MULTI.VFF.1080p.x265-A", 120),
            mk("Show.S01E01.MULTI.VFF.1080p.x264-B", 30),
        ];
        let pick = choose(&cands, 1, false, &missing, &allowed, 6.0, true).unwrap();
        assert!(
            pick.title.ends_with("x265-A"),
            "le x265 mieux partagé doit gagner, pas le x264 : {}",
            pick.title
        );
        // le français reste prioritaire sur tout, codec compris
        let cands = vec![
            mk("Show.S01E01.VOSTFR.1080p.x264-B", 999),
            mk("Show.S01E01.MULTI.VFF.1080p.x265-A", 5),
        ];
        let pick = choose(&cands, 1, false, &missing, &allowed, 6.0, true).unwrap();
        assert!(pick.title.contains("VFF"), "{}", pick.title);
    }

    #[test]
    fn identifier_search_that_leaves_holes_is_not_enough() {
        // Bleach S17 le 2026-09-18 : C411 par identifiant couvre E01-26 et E41-48, jamais E27-40.
        let missing: HashSet<i64> = (1..=48).collect();
        let cands: Vec<Candidate> = (1..=26)
            .chain(41..=48)
            .filter_map(|e| {
                series_candidate(
                    &result(&format!("Bleach.S17E{e:02}.MULTI.VFF.1080p"), 30984, 10),
                    &info(17, false, &[e], 9, 1080),
                    17,
                )
            })
            .collect();
        assert_eq!(cands.len(), 34);
        let left: Vec<i64> = uncovered(&cands, &missing).into_iter().collect();
        assert_eq!(left, (27..=40).collect::<Vec<i64>>(), "le trou est vu");
    }

    #[test]
    fn a_season_pack_covers_everything() {
        let missing: HashSet<i64> = (1..=48).collect();
        let pack = series_candidate(
            &result("Bleach.S17.MULTI.VFF.1080p", 30984, 50),
            &info(17, true, &[], 9, 1080),
            17,
        )
        .unwrap();
        assert!(uncovered(&[pack], &missing).is_empty());
    }

    #[test]
    fn a_cour_pack_on_another_season_is_never_a_candidate() {
        // « Thousand-Year Blood War S03 » : Sonarr l'analyse en saison 3 de Bleach. La prendre pour
        // la saison 17 écraserait une vraie saison — le garde-fou de saison la refuse.
        assert!(series_candidate(
            &result(
                "BLEACH.Thousand-Year.Blood.War.S03.MULTI.VFF.1080p.H264-TFA",
                30984,
                163
            ),
            &info(3, true, &[], 9, 1080),
            17,
        )
        .is_none());
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
    fn a_whole_season_is_taken_in_one_pass() {
        let mk = |t: &str, ep: i64| {
            series_candidate(&result(t, 1, 20), &info(1, false, &[ep], 9, 1080), 1).unwrap()
        };
        let cands: Vec<Candidate> = (1..=11)
            .map(|e| mk(&format!("Show.S01E{e:02}.VOSTFR.1080p.x264"), e))
            .collect();
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let missing: HashSet<i64> = (1..=11).collect();
        let got = choose_episodes(&cands, 1, &missing, &allowed, 6.0, true, 20);
        assert_eq!(got.len(), 11, "toute la saison en une fois");
        let mut eps: Vec<i64> = got.iter().flat_map(|c| c.episodes.clone()).collect();
        eps.sort_unstable();
        assert_eq!(
            eps,
            (1..=11).collect::<Vec<_>>(),
            "un épisode chacun, sans doublon"
        );
        // plafond respecté, et seuls les épisodes manquants sont pris
        assert_eq!(
            choose_episodes(&cands, 1, &missing, &allowed, 6.0, true, 3).len(),
            3
        );
        let two: HashSet<i64> = [4, 7].into_iter().collect();
        let got = choose_episodes(&cands, 1, &two, &allowed, 6.0, true, 20);
        assert_eq!(got.len(), 2);
        assert!(got
            .iter()
            .all(|c| c.episodes == vec![4] || c.episodes == vec![7]));
        // sans candidat acceptable : rien
        assert!(choose_episodes(
            &cands,
            1,
            &missing,
            &[7].into_iter().collect(),
            6.0,
            true,
            20
        )
        .is_empty());
    }

    #[test]
    fn spinoffs_and_mini_series_are_rejected() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let real = names(&["Smoking Behind the Supermarket with You"]);
        assert_eq!(
            derivative(
                "Smoking.Behind.The.Supermarket.With.You.Mini.Episodes.S01.VOSTFR.1080p",
                &real
            ),
            Some("mini")
        );
        assert_eq!(
            derivative(
                "Smoking.Behind.The.Supermarket.With.You.S01.VOSTFR.1080p",
                &real
            ),
            None
        );
        assert_eq!(
            derivative(
                "L.Attaque.Des.Titans.Junior.High.School.S01.VF",
                &names(&["L'Attaque des Titans"])
            ),
            Some("junior")
        );
        assert_eq!(
            derivative("Bleach.S.Abridged.S01", &names(&["Bleach"])),
            Some("abridged")
        );
        assert_eq!(
            derivative("Show.S01.OVA.1080p", &names(&["Show"])),
            Some("ova")
        );
        // le mot fait partie du vrai titre : on ne l'écarte pas
        assert_eq!(
            derivative("Mini.Serie.Culte.S01.VFF", &names(&["Mini Série Culte"])),
            None
        );
        assert_eq!(
            derivative("Junior.S01.VFF.1080p", &names(&["Junior"])),
            None
        );
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
    fn fresh_requests_open_extra_queries() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-18T12:00:00Z")
            .unwrap()
            .timestamp();
        assert!(is_fresh("2026-09-18T11:30:00Z", now, 1));
        assert!(!is_fresh("2026-09-18T10:00:00Z", now, 1));
        assert!(!is_fresh("", now, 1));
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
            title: String::new(),
            uncovered: Vec::new(),
        };
        assert!(due(None, 1000, 24, 168, 1, 15));
        assert!(!due(Some(&r("none", 0)), 23 * 3600, 24, 168, 1, 15));
        assert!(due(Some(&r("none", 0)), 24 * 3600, 24, 168, 1, 15));
        assert!(!due(Some(&r("grabbed", 0)), 100 * 3600, 24, 168, 1, 15));
        assert!(!due(Some(&r("error", 0)), 1800, 24, 168, 1, 15));
        // épisodes pris : on reprend au bout de 15 min, plus 2 h
        assert!(!due(Some(&r("grabbed_episode", 0)), 600, 24, 168, 1, 15));
        assert!(due(Some(&r("grabbed_episode", 0)), 900, 24, 168, 1, 15));
        assert!(due(Some(&r("error", 0)), 3600, 24, 168, 1, 15));
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
