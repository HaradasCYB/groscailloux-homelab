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

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::clients::{ArrClient, ProwlarrClient, QbitClient, TorrentFile};
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
    /// `codec_rank` : 2 = HEVC, 1 = H.264 ou non indiqué, 0 = AV1.
    pub codec: u8,
    /// `audio_rank` : 3 = AAC/E-AC3/AC3/Opus, 2 = FLAC, 1 = DTS, 0 = DTS-HD/TrueHD.
    pub audio: u8,
    pub seeders: i64,
    pub size: i64,
}

/// Marqueurs de langue présents dans le titre d'une release. Une même release peut en porter
/// plusieurs (« MULTI.VFF ») : c'est le classement qui tranche, pas la lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Langs {
    /// VFF, TRUEFRENCH, VFQ… : doublage français.
    pub vf: bool,
    /// Plusieurs pistes audio.
    pub multi: bool,
    /// FRENCH sans précision.
    pub french: bool,
    /// Version originale sous-titrée en français.
    pub vostfr: bool,
}

pub fn langs_of(title: &str) -> Langs {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(&x.as_str()));
    Langs {
        // VOF = version originale française (série ou film tourné en français) : audio français, comme une VF.
        // Absent jusqu'au 2026-09-23 : Le Voyageur S04E04 (VOF) était classé « sans français ».
        vf: has(&["VFF", "TRUEFRENCH", "VFQ", "VFI", "VF2", "VFB", "VOF"]),
        multi: has(&["MULTI"]),
        french: has(&["FRENCH"]),
        vostfr: has(&["VOSTFR", "SUBFRENCH"]),
    }
}

/// Rang de langue d'après le titre. Une release de rang 0 (aucun marqueur français) n'est prise qu'en
/// dernier recours (voir `choose`).
///
/// **Séries et films** : VF 4 > MULTi 3 > FRENCH 2 > VOSTFR 1 > VO 0. Un « MULTI.VFF » porte les deux
/// marqueurs et compte comme VF, comme avant.
///
/// **Animés** (2026-09-18, demandé par l'utilisateur) : **MULTi 4 > VOSTFR 3 > VF 2 > FRENCH 1 > VO 0**.
/// Un MULTi porte les deux pistes audio, donc il sert tout le monde ; la VOSTFR garde l'audio japonais,
/// que la plupart des spectateurs d'animés préfèrent au doublage. Un « MULTI.VFF » compte donc ici
/// comme MULTi.
pub fn lang_rank_for(title: &str, anime: bool) -> u8 {
    let l = langs_of(title);
    if anime {
        if l.multi {
            4
        } else if l.vostfr {
            3
        } else if l.vf {
            2
        } else if l.french {
            1
        } else {
            0
        }
    } else if l.vf {
        4
    } else if l.multi {
        3
    } else if l.french {
        2
    } else if l.vostfr {
        1
    } else {
        0
    }
}

/// Classement hors animé (séries et films).
pub fn lang_rank(title: &str) -> u8 {
    lang_rank_for(title, false)
}

/// La fiche est-elle un animé ? (`seriesType` de Sonarr.)
pub fn is_anime(series: &Value) -> bool {
    series.get("seriesType").and_then(Value::as_str) == Some("anime")
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

/// Préférence de codec, **après** la langue, la résolution et « au moins 2 sources » (2026-09-26) :
/// 2 = HEVC (x265), 1 = H.264 ou codec non indiqué, 0 = AV1. Mesuré sur la médiathèque : un 1080p HEVC
/// pèse 1,1 à 1,3 Go/h contre 3,4 à 4,25 Go/h en H.264, et il n'est pas plus transcodé (13 % des lectures
/// HEVC réencodées sur 30 jours, 20 % des H.264). L'AV1, aussi léger, est mal lu par les vieux clients.
/// AV1 = mot entier, pour ne pas attraper un nom de groupe.
pub fn codec_rank(title: &str) -> u8 {
    let t = title.to_ascii_uppercase();
    if ["X265", "H265", "H.265", "H 265", "HEVC"]
        .iter()
        .any(|m| t.contains(m))
    {
        2
    } else if t
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "AV1")
    {
        0
    } else {
        1
    }
}

/// Départage audio, **après** le codec vidéo (2026-09-26) : 3 = AAC, E-AC3, AC3, Opus ou non indiqué ;
/// 2 = FLAC ; 1 = DTS ; 0 = DTS-HD MA, DTS:X ou TrueHD. Mesuré sur 30 jours : 80 % des lectures d'une
/// piste DTS réencodaient le son (Chromecast et appli iOS ne le lisent pas), aucune en AAC ; et une piste
/// DTS (1,5 Mbit/s) pèse 60 % de l'image d'un épisode HEVC, un DTS-HD MA 2,6 à 3,5 Mbit/s.
pub fn audio_rank(title: &str) -> u8 {
    let w: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .collect();
    let at = |i: usize| w.get(i).map(String::as_str).unwrap_or("");
    let dts = w
        .iter()
        .position(|x| x == "DTS" || x.starts_with("DTSHD") || x == "DTSX");
    if w.iter().any(|x| x == "TRUEHD")
        || dts.is_some_and(|i| {
            w[i] != "DTS" || matches!(at(i + 1), "HD" | "MA" | "X") || at(i + 1) == "HDMA"
        })
    {
        0
    } else if dts.is_some() {
        1
    } else if w.iter().any(|x| x == "FLAC") {
        2
    } else {
        3
    }
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

/// Numéro d'épisode d'un fichier de pack nommé **à la manière des fansubs** : `Erased S01 - 06 VOSTFR
/// [1080p][X265].mkv`. Sonarr lit `S01` comme un marqueur de saison entière et **ne voit jamais le
/// « - 06 »** : il refuse alors chaque fichier (« Single episode file contains all episodes in seasons »)
/// et le pack entier reste sur le carreau (Erased, le 2026-09-18 : 2,11 Gio téléchargés, 0 importé).
///
/// Forme exigée : le marqueur de saison, un tiret entouré d'espaces, puis 1 à 3 chiffres. Volontairement
/// rigide pour ne jamais attraper `1080p`, `x265`, `10BITS`, une année ou un suffixe de groupe.
pub fn fansub_episode(path: &str) -> Option<i64> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\bs\d{1,3}\s+-\s+(\d{1,3})(?:\s|$)").expect("regex valide")
    });
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
    pub codec: u8,
    pub audio: u8,
}

/// Cette release est-elle le pack d'un cours de **notre** saison, publié sous son propre titre ?
///
/// Conditions toutes obligatoires : l'Arr rattache la release à **cette** fiche, il en lit une **autre**
/// saison, c'est un pack complet, et — le point qui manque à l'intuition — il ne sait **pas déjà** la
/// mapper sur la saison cible. Ce dernier point écarte « Thousand-Year Blood War **S01** », que le scene
/// mapping TVDB traduit déjà en saison 17 (50/50 épisodes, mesuré le 2026-09-18) : le prendre par ce
/// chemin lui inventerait un décalage et le placerait de travers.
pub fn cour_pack(
    result: &Value,
    parse: &Value,
    series_id: i64,
    season: i64,
    anime: bool,
) -> Option<CourPack> {
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
        lang_rank: lang_rank_for(&title, anime),
        codec: codec_rank(&title),
        audio: audio_rank(&title),
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

/// Sonarr ne lit **aucune saison** dans ce titre (intégrale : `parsedEpisodeInfo` vide), ou c'est un pack de
/// plusieurs saisons : la release ne peut être jugée que fichier par fichier (voir `integrale_pick`).
pub fn season_less_pack(parse: &Value) -> bool {
    match parse.get("parsedEpisodeInfo") {
        None | Some(Value::Null) => true,
        Some(info) => {
            info.get("isMultiSeason").and_then(Value::as_bool) == Some(true)
                || info.get("seasonNumber").and_then(Value::as_i64).is_none()
        }
    }
}

/// Titre à soumettre au `parse` pour connaître la **qualité** d'une intégrale : Sonarr ne lit rien dans un titre
/// sans saison, pas même la résolution. Le marqueur (« INTEGRALE », « COMPLETE ») devient `S01` ; sans marqueur,
/// `S01` est inséré devant la première étiquette de langue ou de qualité. `None` : rien à quoi se raccrocher.
pub fn integrale_quality_title(title: &str) -> Option<String> {
    static MARK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let mark = MARK.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:int[ée]grale?|compl[eè]te)\b").expect("regex valide")
    });
    if let Some(m) = mark.find(title) {
        return Some(format!("{}S01{}", &title[..m.start()], &title[m.end()..]));
    }
    let tag = TAG.get_or_init(|| {
        regex::Regex::new(
            r"(?i)[ ._\-\[(](?:multi|vff|vfq|vfi|vf2|truefrench|french|vostfr|subfrench|vof|\d{3,4}p|bluray|bdrip|web-?dl|webrip|hdtv)\b",
        )
        .expect("regex valide")
    });
    let m = tag.find(title)?;
    Some(format!(
        "{}.S01{}",
        &title[..m.start()],
        &title[m.start()..]
    ))
}

/// Un fichier vidéo d'une intégrale : chemin dans le `.torrent`, taille, et épisodes de **notre** fiche que Sonarr
/// lit dans son nom, en `(saison, numéro)` — scene mapping compris (`02x01` → S01E14 pour Space Dandy).
#[derive(Debug, Clone)]
pub struct IntegraleFile {
    pub path: String,
    pub size: i64,
    pub episodes: Vec<(i64, i64)>,
}

/// Fichiers d'une intégrale retenus pour une saison, et épisodes qu'ils pourvoient.
#[derive(Debug, Clone, PartialEq)]
pub struct IntegralePick {
    pub paths: Vec<String>,
    pub covered: BTreeSet<i64>,
    pub bytes: i64,
}

/// Fichiers à prendre pour `season` : ceux dont **tous** les épisodes lus sont des épisodes manquants de cette
/// saison — jamais un remplacement, jamais une autre saison. Refus si deux fichiers visent le même épisode (deux
/// versions : on ne choisit pas au hasard) ou si aucun fichier ne tombe dans le trou.
pub fn integrale_pick(
    files: &[IntegraleFile],
    season: i64,
    missing: &HashSet<i64>,
) -> std::result::Result<IntegralePick, &'static str> {
    let mut paths = Vec::new();
    let mut covered = BTreeSet::new();
    let mut bytes = 0i64;
    for f in files {
        if f.episodes.is_empty()
            || !f
                .episodes
                .iter()
                .all(|(s, n)| *s == season && missing.contains(n))
        {
            continue;
        }
        for (_, n) in &f.episodes {
            if !covered.insert(*n) {
                return Err("deux fichiers pour un même épisode");
            }
        }
        paths.push(f.path.clone());
        bytes += f.size;
    }
    if paths.is_empty() {
        return Err("aucun fichier pour les épisodes manquants de cette saison");
    }
    Ok(IntegralePick {
        paths,
        covered,
        bytes,
    })
}

/// Le dernier épisode manquant est-il sorti depuis au moins `days` jours ? Date absente ou illisible : non.
pub fn aired_before(latest_air: &str, now: i64, days: i64) -> bool {
    chrono::DateTime::parse_from_rfc3339(latest_air)
        .map(|d| now - d.timestamp() >= days * 86_400)
        .unwrap_or(false)
}

/// Rangs (`torrents/files`) des fichiers voulus. qBittorrent préfixe les chemins du `.torrent` par le dossier
/// racine du torrent (son nom) ; un torrent mono-fichier n'a que son nom. Comparaison exacte, jamais par suffixe.
pub fn file_ids(files: &[TorrentFile], wanted: &[String]) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            wanted.iter().any(|w| {
                f.name == *w
                    || f.name
                        .split_once('/')
                        .is_some_and(|(_, rest)| rest == w.as_str())
            })
        })
        .map(|(i, _)| i)
        .collect()
}

/// Candidat d'après un résultat Prowlarr et l'analyse (`parsedEpisodeInfo`) de son titre par Sonarr.
pub fn series_candidate(
    result: &Value,
    info: &Value,
    season: i64,
    anime: bool,
) -> Option<Candidate> {
    let title = result.get("title").and_then(Value::as_str)?.to_string();
    let url = result.get("downloadUrl").and_then(Value::as_str)?;
    if info.get("seasonNumber").and_then(Value::as_i64) != Some(season) {
        return None;
    }
    let quality = info.get("quality").cloned().unwrap_or(Value::Null);
    Some(Candidate {
        lang_rank: lang_rank_for(&title, anime),
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
        codec: codec_rank(&title),
        audio: audio_rank(&title),
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
/// **Codec** (2026-09-26) : à langue, résolution et partage égaux, le x265 passe devant le x264, l'AV1 en
/// dernier (`codec_rank`), puis l'audio départage (`audio_rank` : AAC/E-AC3/AC3 avant FLAC, DTS, DTS-HD). Le x265 n'est jamais **exigé** : un x264 en VF passe toujours devant un x265
/// sans français, et un x265 à une seule source derrière un x264 bien partagé.
pub fn choose<'a>(
    cands: &'a [Candidate],
    season: i64,
    want_pack: bool,
    missing: &HashSet<i64>,
    allowed: &HashSet<i64>,
    max_gb: f64,
    allow_vo: bool,
) -> Option<&'a Candidate> {
    choose_sized(
        cands,
        season,
        want_pack,
        missing,
        missing.len(),
        allowed,
        max_gb,
        allow_vo,
    )
}

/// `choose`, avec le nombre d'épisodes sur lequel jauger la taille d'un **pack** (`pack_units`).
///
/// `choose` y met tous les épisodes manquants. Or les épisodes sans date (`undated_missing`, voulus pour *Le
/// Voyageur*) gonflent ce nombre sans qu'on sache s'ils sont sortis : Black Clover S02 (03/10), 1 épisode diffusé
/// sur 13, comptait 11 manquants et son plafond 11 × 3 Gio = 33 Gio laissait passer un pack de 14 Gio qui n'était
/// pas la saison (51 fichiers de l'ancienne numérotation). `process_season` passe donc ici les seuls épisodes
/// manquants **datés** (diffusés, sans fichier) : avec le seul E01, ce pack est refusé.
#[allow(clippy::too_many_arguments)]
pub fn choose_sized<'a>(
    cands: &'a [Candidate],
    season: i64,
    want_pack: bool,
    missing: &HashSet<i64>,
    pack_units: usize,
    allowed: &HashSet<i64>,
    max_gb: f64,
    allow_vo: bool,
) -> Option<&'a Candidate> {
    let ok = |c: &&Candidate| {
        let units = if c.full_season {
            pack_units.max(c.episodes.len())
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
            .max_by_key(|c| {
                (
                    c.lang_rank,
                    c.resolution,
                    c.seeders >= 2,
                    c.codec,
                    c.audio,
                    c.seeders,
                )
            })
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

/// Numéros des épisodes **suivis** de la saison dont la diffusion est encore à venir (date connue, postérieure à
/// `now`). C'est la `FullSeasonSpecification` de Sonarr (« all episodes in full season release have aired ») : un
/// pack de saison ne peut pas être la saison tant qu'il en reste à diffuser. Elle ne joue pas pour les releases
/// qui partent directement dans qBittorrent (seedbox : « Prowlarr injoignable depuis la seedbox »), d'où ce
/// contrôle ici — Black Clover S02 (03/10) : 1 épisode sorti, 2 datés à venir, 10 sans date, et un pack de 51
/// fichiers de l'ancienne numérotation (14 Gio) pris pour la saison.
///
/// Un épisode **sans date** ne compte pas (TheTVDB date tard les séries françaises : *Le Voyageur*, ses épisodes
/// sans date sont sortis) ; seule une date à venir bloque.
pub fn future_episodes(
    episodes: &[Value],
    season: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<i64> {
    let mut out: Vec<i64> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(season))
        .filter(|e| e.get("monitored").and_then(Value::as_bool) == Some(true))
        .filter(|e| {
            e.get("airDateUtc")
                .and_then(Value::as_str)
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .is_some_and(|d| d.with_timezone(&chrono::Utc) > now)
        })
        .filter_map(|e| e.get("episodeNumber").and_then(Value::as_i64))
        .collect();
    out.sort_unstable();
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

/// Décision pour un passage noté `fallback_none` (C411 en panne, secours sans rien d'acceptable) : `None` pour
/// tout autre résultat. C411 revenu → aussitôt ; sinon, secours refait seulement au bout de `fallback_h` heures
/// (avant : une erreur, donc un passage par heure — 8 requêtes/h pour les 4 saisons du *Voyageur*, 01/10).
pub fn fallback_due(
    rec: Option<&SeasonSearchRecord>,
    now: i64,
    fallback_h: i64,
    c411_up: bool,
) -> Option<bool> {
    let r = rec?;
    (r.outcome == "fallback_none").then(|| c411_up || now - r.at >= fallback_h * 3600)
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
    // release sans .torrent (Nyaa), ou venue du secours public (World-torrent : son « .torrent » est une
    // redirection 301 vers un magnet, 30/09) : lien magnet ajouté directement au qBittorrent du même côté
    let ixc = &ctx.cfg.indexers;
    let from_fallback = (!ixc.fallback.is_empty() && indexer == ixc.fallback)
        || (!ixc.fallback_anime.is_empty() && indexer == ixc.fallback_anime);
    let dl = release.get("downloadUrl").and_then(Value::as_str);
    let magnet = release
        .get("magnetUrl")
        .and_then(Value::as_str)
        .or(if from_fallback { dl } else { None });
    if let (true, Some(magnet)) = (dl.is_none() || from_fallback, magnet) {
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

/// Une série suivie a-t-elle une saison suivie (hors spéciaux) déjà diffusée depuis moins de `window_days`
/// jours et encore incomplète ? Seules celles-là valent la lecture de leurs épisodes (statistiques de `series`).
fn open_recent_season(
    series: &Value,
    now: chrono::DateTime<chrono::Utc>,
    window_days: i64,
) -> bool {
    if !series
        .get("monitored")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    series
        .get("seasons")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|se| {
            let num = se.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0);
            let monitored = se
                .get("monitored")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let st = se.get("statistics");
            let total = st
                .and_then(|s| s.get("totalEpisodeCount"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let files = st
                .and_then(|s| s.get("episodeFileCount"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let recent = st
                .and_then(|s| s.get("previousAiring"))
                .and_then(Value::as_str)
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|d| (now - d.with_timezone(&chrono::Utc)).num_days() <= window_days)
                .unwrap_or(false);
            num > 0 && monitored && total > files && recent
        })
}

/// Épisodes suivis, sans fichier et SANS date de diffusion, d'une saison dont au moins un épisode est déjà
/// diffusé : `(saison, épisode, dernière diffusion de la saison)`. Une saison pas encore commencée n'est pas
/// concernée (ses épisodes n'existent pas encore chez les groupes de release).
fn undated_missing(episodes: &[Value], now_iso: &str) -> Vec<(i64, i64, String)> {
    let mut latest: HashMap<i64, String> = HashMap::new();
    for e in episodes {
        let season = e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0);
        let air = e.get("airDateUtc").and_then(Value::as_str).unwrap_or("");
        if season > 0 && !air.is_empty() && air <= now_iso {
            let l = latest.entry(season).or_default();
            if air > l.as_str() {
                *l = air.to_string();
            }
        }
    }
    let mut out: Vec<(i64, i64, String)> = episodes
        .iter()
        .filter_map(|e| {
            let season = e.get("seasonNumber").and_then(Value::as_i64)?;
            let num = e.get("episodeNumber").and_then(Value::as_i64)?;
            let undated = e
                .get("airDateUtc")
                .and_then(Value::as_str)
                .map(str::is_empty)
                .unwrap_or(true);
            let wanted = e.get("monitored").and_then(Value::as_bool).unwrap_or(false)
                && !e.get("hasFile").and_then(Value::as_bool).unwrap_or(false);
            let started = latest.get(&season)?;
            (season > 0 && undated && wanted).then(|| (season, num, started.clone()))
        })
        .collect();
    out.sort();
    out
}

struct SeasonTodo {
    series_id: i64,
    season: i64,
    latest_air: String,
    missing_numbers: HashSet<i64>,
    /// Parmi `missing_numbers`, ceux qui ont une date de diffusion passée (`wanted/missing`) : les autres sont des
    /// épisodes sans date, qu'on ne sait pas être sortis. Sert à jauger la taille d'un pack (`choose_sized`).
    dated_missing: HashSet<i64>,
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

/// Requêtes de secours : les premiers noms connus (titre français TMDB, titre d'origine…), sans ponctuation.
pub fn fallback_names(names: &[String], n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let q = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '\'' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !q.is_empty() && !out.contains(&q) {
            out.push(q);
        }
        if out.len() >= n {
            break;
        }
    }
    out
}

/// C411 en panne (2026-09-30) : secours publics en texte libre — Nyaa d'abord pour un animé, puis World-torrent.
/// Mêmes garde-fous que le texte libre de C411 (l'Arr doit rattacher la release à CETTE fiche et à cette
/// saison), plus un : **français seulement** (aucune VO prise pendant une panne). Rien de pris : erreur, pour
/// retenter C411 dans l'heure.
async fn fallback_candidates(
    ctx: &TaskContext,
    arr: &ArrClient,
    prow: &ProwlarrClient,
    todo: &SeasonTodo,
    names: &[String],
    anime: bool,
    outage: anyhow::Error,
) -> Result<Found> {
    let ix = &ctx.cfg.indexers;
    let mut sources: Vec<&str> = Vec::new();
    if anime && !ix.fallback_anime.trim().is_empty() {
        sources.push(ix.fallback_anime.trim());
    }
    if !ix.fallback.trim().is_empty() {
        sources.push(ix.fallback.trim());
    }
    if sources.is_empty() {
        return Err(outage);
    }
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for src in sources {
        for q in fallback_names(names, 2) {
            let Some(found) = crate::indexer::search_indexer(prow, src, &q, "5000").await? else {
                break; // indexer absent de Prowlarr
            };
            info!(task = "series_search", series_id = todo.series_id, season = todo.season, fallback = src, query = %q, results = found.len(), "C411 en panne : recherche de secours");
            for mut r in found.into_iter().take(60) {
                let Some(title) = r.get("title").and_then(Value::as_str).map(str::to_string) else {
                    continue;
                };
                if !seen.insert(title.clone())
                    || derivative(&title, names).is_some()
                    || lang_rank_for(&title, anime) == 0
                {
                    continue;
                }
                let parse = arr.parse(&title).await?;
                match parsed_series(&parse) {
                    Some(p) if title_matches(&p.title, names) => {}
                    _ => continue,
                }
                if parse.pointer("/series/id").and_then(Value::as_i64) != Some(todo.series_id) {
                    continue;
                }
                r["_gc_source"] = json!(src);
                let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
                if let Some(c) = series_candidate(&r, &info, todo.season, anime) {
                    out.push(c);
                }
            }
        }
        if !out.is_empty() && uncovered(&out, &todo.missing_numbers).is_empty() {
            break;
        }
    }
    Ok((out, Vec::new(), Vec::new(), "secours"))
}

/// Secours interrogés pour une fiche, dans l'ordre (« Nyaa.si + World-torrent » pour un animé).
pub fn fallback_label(ix: &crate::config::Indexers, anime: bool) -> String {
    let mut v: Vec<&str> = Vec::new();
    if anime && !ix.fallback_anime.trim().is_empty() {
        v.push(ix.fallback_anime.trim());
    }
    if !ix.fallback.trim().is_empty() {
        v.push(ix.fallback.trim());
    }
    v.join(" + ")
}

/// Indexer d'où vient un candidat (secours public), C411 par défaut.
fn source_of<'a>(c: &'a Candidate, default: &'a str) -> &'a str {
    c.release
        .get("_gc_source")
        .and_then(Value::as_str)
        .unwrap_or(default)
}

/// Ce que ramène la recherche d'une saison : candidats ordinaires, packs de cours, **intégrales** (résultats bruts
/// portant l'identifiant de la série mais où Sonarr ne lit aucune saison, voir `try_integrale`), et le chemin suivi
/// (`tmdb`, `tmdb+texte`, `texte`, `secours`, `budget`).
type Found = (Vec<Candidate>, Vec<CourPack>, Vec<Value>, &'static str);

/// Retient une intégrale vue dans les résultats : identifiant de la série, aucune saison lisible, pas déjà notée.
fn keep_integrale(integrales: &mut Vec<Value>, r: &Value, parse: &Value, tmdb: i64) {
    if tmdb_matches(r, tmdb)
        && season_less_pack(parse)
        && !integrales.iter().any(|x| x.get("title") == r.get("title"))
    {
        integrales.push(r.clone());
    }
}

/// Candidats d'une saison : par identifiant TMDB, puis (rien trouvé) en texte libre avec les noms connus.
async fn season_candidates(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    throttle: &mut Throttle,
) -> Result<Found> {
    let cfg = &ctx.cfg.tasks.series_search;
    let tmdb = series.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let anime = is_anime(series);
    let names = names_for(ctx, series).await;
    let mut out = Vec::new();
    let mut packs: Vec<CourPack> = Vec::new();
    let mut integrales: Vec<Value> = Vec::new();
    if tmdb > 0 {
        if !throttle.take().await {
            return Ok((out, packs, integrales, "budget"));
        }
        let found =
            match crate::indexer::search_tmdb(ctx, prow, tmdb, Some(todo.season), false).await {
                Ok(Some(f)) => f,
                Ok(None) => return Ok((out, packs, integrales, "budget")),
                Err(e) if crate::indexer::is_outage(&e) => {
                    return fallback_candidates(ctx, arr, prow, todo, &names, anime, e).await;
                }
                Err(e) => return Err(e),
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
            keep_integrale(&mut integrales, &r, &parse, tmdb);
            if let Some(p) = cour_pack(&r, &parse, todo.series_id, todo.season, anime) {
                packs.push(p);
            }
            let info = parse.get("parsedEpisodeInfo").cloned().unwrap_or_default();
            if let Some(c) = series_candidate(&r, &info, todo.season, anime) {
                out.push(c);
            }
        }
        // l'identifiant suffit seulement s'il couvre TOUS les épisodes manquants ; s'il en laisse,
        // on complète en texte libre (les cours d'un animé sont souvent nommés autrement)
        let left = uncovered(&out, &todo.missing_numbers);
        if !out.is_empty() && left.is_empty() {
            return Ok((out, packs, integrales, "tmdb"));
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
            // intégrale : Sonarr n'en lit rien (ni série ni saison), seul l'identifiant TMDB la rattache
            keep_integrale(&mut integrales, &r, &parse, tmdb);
            // Pack d'un cours : il porte le titre du cours (« BLEACH Thousand-Year Blood War »), et
            // l'indexer lui donne l'identifiant TMDB **du cours** (313552), pas celui de la série
            // (30984). C'est donc `parse./series/id` — la table d'alias de l'Arr — qui fait foi, et
            // la sécurité vient ensuite de la lecture du `.torrent` et de `offset_mapping`, jamais de
            // l'identifiant. Le chemin normal, lui, garde le refus par identifiant intact.
            if let Some(p) = cour_pack(&r, &parse, todo.series_id, todo.season, anime) {
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
            if let Some(c) = series_candidate(&r, &info, todo.season, anime) {
                out.push(c);
            }
        }
    }
    Ok((
        out,
        packs,
        integrales,
        if by_id > 0 { "tmdb+texte" } else { "texte" },
    ))
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
    // le français d'abord, puis le x265, puis le mieux partagé
    let mut order: Vec<&CourPack> = packs.iter().collect();
    order.sort_by_key(|p| std::cmp::Reverse((p.lang_rank, p.codec, p.audio, p.seeders)));
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

/// `.torrent` d'intégrales lus au plus par saison et par passage (les mieux classées d'abord).
const INTEGRALE_TRIES: usize = 2;

/// Clé de classement d'une intégrale : (français, rang de langue, résolution, ≥ 2 sources, codec, audio, sources).
type IntegraleRank = (bool, u8, i64, bool, u8, u8, i64);

/// Tente une **intégrale** quand aucune release ordinaire n'est acceptable pour la saison. `Ok(None)` = rien de sûr,
/// la saison reste « aucun candidat » comme avant.
///
/// Space Dandy (2026-10-03) : C411 n'avait qu'une intégrale, absente de la recherche par saison, et Sonarr ne lit rien
/// dans son nom ; la demande d'un membre restait « introuvable ». Les intégrales vues dans les résultats servent
/// d'abord ; sinon une requête par identifiant **sans saison**, seulement pour une saison dont le dernier épisode
/// manquant est sorti depuis `integrale_min_age_days`. Mêmes règles de choix que `choose` (qualité lue en mettant
/// `S01` à la place du marqueur), puis lecture du `.torrent` (aucune annonce) : chaque fichier vidéo est soumis au
/// `parse` de Sonarr, et seuls ceux qui pourvoient des épisodes manquants de CETTE saison sont téléchargés.
#[allow(clippy::too_many_arguments)]
async fn try_integrale(
    ctx: &TaskContext,
    prow: &ProwlarrClient,
    arr: &ArrClient,
    series: &Value,
    todo: &SeasonTodo,
    mut found: Vec<Value>,
    allowed: &HashSet<i64>,
    throttle: &mut Throttle,
) -> Result<Option<SeasonOutcome>> {
    let cfg = &ctx.cfg.tasks.series_search;
    let ix = &ctx.cfg.indexers;
    let title = series.get("title").and_then(Value::as_str).unwrap_or("?");
    let tmdb = series.get("tmdbId").and_then(Value::as_i64).unwrap_or(0);
    let anime = is_anime(series);
    if !cfg.integrale_packs || tmdb <= 0 {
        return Ok(None);
    }
    if found.is_empty() {
        if !aired_before(&todo.latest_air, now(), cfg.integrale_min_age_days)
            || !throttle.take().await
        {
            return Ok(None);
        }
        let list = match crate::indexer::search_series_tmdb_all(ctx, prow, tmdb, false).await {
            Ok(Some(l)) => l,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!(task = "series_search", service = arr.name, series = title, error = %e, "intégrales : recherche sans saison impossible");
                return Ok(None);
            }
        };
        let names = names_for(ctx, series).await;
        let total = list.len();
        for r in list {
            let Some(t) = r.get("title").and_then(Value::as_str) else {
                continue;
            };
            if !tmdb_matches(&r, tmdb) || derivative(t, &names).is_some() {
                continue;
            }
            let parse = arr.parse(t).await?;
            keep_integrale(&mut found, &r, &parse, tmdb);
        }
        info!(
            task = "series_search",
            service = arr.name,
            series = title,
            season = todo.season,
            results = total,
            integrales = found.len(),
            "intégrales : recherche par identifiant sans saison"
        );
    }
    // classement : mêmes clés que `choose` (français d'abord, résolution, partage, codec, audio)
    let mut ranked: Vec<(IntegraleRank, Value)> = Vec::new();
    for r in &found {
        let (Some(t), Some(url)) = (
            r.get("title").and_then(Value::as_str),
            r.get("downloadUrl").and_then(Value::as_str),
        ) else {
            continue;
        };
        let parse = arr.parse(t).await?;
        let mut quality = parse
            .pointer("/parsedEpisodeInfo/quality")
            .cloned()
            .unwrap_or(Value::Null);
        if quality.is_null() {
            if let Some(probe) = integrale_quality_title(t) {
                quality = arr
                    .parse(&probe)
                    .await?
                    .pointer("/parsedEpisodeInfo/quality")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
        }
        let res = quality
            .pointer("/quality/resolution")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let seeders = r.get("seeders").and_then(Value::as_i64).unwrap_or(0);
        let lang = lang_rank_for(t, anime);
        let release = json!({
            "title": t,
            "downloadUrl": url,
            "publishDate": r.get("publishDate"),
            "quality": quality,
            "infoHash": r.get("infoHash"),
        });
        if !acceptable(&release, res, seeders, allowed) || (lang == 0 && !ix.allow_no_french) {
            info!(task = "series_search", service = arr.name, series = title, release = %t, resolution = res, seeders, lang,
                  "intégrale écartée : qualité, sources ou langue");
            continue;
        }
        let rank = (
            lang > 0,
            lang,
            res,
            seeders >= 2,
            codec_rank(t),
            audio_rank(t),
            seeders,
        );
        ranked.push((rank, release));
    }
    ranked.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    for (_, release) in ranked.into_iter().take(INTEGRALE_TRIES) {
        let rtitle = release
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let url = release
            .get("downloadUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let raw = match prow.download(url).await {
            Ok(b) => b,
            Err(e) => {
                warn!(task = "series_search", release = %rtitle, error = %e, "intégrale : .torrent illisible");
                continue;
            }
        };
        let entries = match crate::torrent_file::files(&raw) {
            Ok(v) => v,
            Err(e) => {
                warn!(task = "series_search", release = %rtitle, error = %e, "intégrale : bencode illisible");
                continue;
            }
        };
        let vids = crate::torrent_file::video_paths(&entries);
        if vids.len() > cfg.integrale_max_files {
            info!(task = "series_search", service = arr.name, series = title, release = %rtitle, fichiers = vids.len(),
                  "intégrale écartée : trop de fichiers à examiner");
            continue;
        }
        let mut files = Vec::with_capacity(vids.len());
        for p in &vids {
            let base = p.rsplit('/').next().unwrap_or(p);
            let parse = arr.parse(base).await?;
            let episodes: Vec<(i64, i64)> =
                if parse.pointer("/series/id").and_then(Value::as_i64) == Some(todo.series_id) {
                    parse
                        .get("episodes")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| {
                            Some((
                                e.get("seasonNumber")?.as_i64()?,
                                e.get("episodeNumber")?.as_i64()?,
                            ))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            let size = entries
                .iter()
                .find(|e| e.path == *p)
                .map(|e| e.length)
                .unwrap_or(0);
            files.push(IntegraleFile {
                path: p.to_string(),
                size,
                episodes,
            });
        }
        let pick = match integrale_pick(&files, todo.season, &todo.missing_numbers) {
            Ok(p) => p,
            Err(why) => {
                info!(task = "series_search", service = arr.name, series = title, season = todo.season, release = %rtitle,
                      fichiers = files.len(), %why, "intégrale écartée");
                continue;
            }
        };
        if !size_ok(pick.bytes, pick.covered.len(), ix.max_gb_per_episode) {
            info!(task = "series_search", service = arr.name, series = title, release = %rtitle,
                  "intégrale écartée : trop lourde par épisode");
            continue;
        }
        let left: Vec<i64> = todo
            .missing_numbers
            .iter()
            .copied()
            .filter(|n| !pick.covered.contains(n))
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .collect();
        let detail = format!(
            "intégrale {rtitle} : {} fichier(s) sur {} → S{:02}, {} épisode(s)",
            pick.paths.len(),
            vids.len(),
            todo.season,
            pick.covered.len()
        );
        if ctx.dry_run {
            info!(task = "series_search", service = arr.name, series = title, %detail, "essai à blanc : intégrale retenue");
            return Ok(Some(SeasonOutcome::new("dry_run", detail, left)));
        }
        let target = Target::Season {
            series_id: todo.series_id,
            season: todo.season,
        };
        let why = grab_integrale(ctx, arr, raw, &release, &pick, &target.tag()).await?;
        info!(task = "series_search", service = arr.name, series = title, season = todo.season, %detail, %why, "intégrale confiée");
        return Ok(Some(SeasonOutcome::new(
            "grabbed",
            format!("{detail} — {why}"),
            left,
        )));
    }
    Ok(None)
}

/// Confie une intégrale au qBittorrent du côté de la fiche, **arrêtée**, puis désélectionne tout ce qui n'a pas été
/// retenu avant de la démarrer : rien d'autre n'est écrit sur le disque. Déjà présente (une autre saison de la même
/// intégrale) : nos fichiers rejoignent sa sélection, et son passage par `torrent_import` est effacé pour qu'il les
/// importe à leur tour (sans ça, le torrent déjà noté « importé » ne serait plus examiné).
async fn grab_integrale(
    ctx: &TaskContext,
    arr: &ArrClient,
    raw: Vec<u8>,
    release: &Value,
    pick: &IntegralePick,
    tag: &str,
) -> Result<String> {
    let qbit = qbit_for(ctx, arr).context("aucun qBittorrent pour ce côté")?;
    let announced = release
        .get("infoHash")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let before: HashSet<String> = qbit
        .torrents()
        .await?
        .into_iter()
        .map(|t| t.hash.to_ascii_lowercase())
        .collect();
    if !announced.is_empty() && before.contains(&announced) {
        let files = qbit.files(&announced).await?;
        let ids = file_ids(&files, &pick.paths);
        if ids.len() != pick.paths.len() {
            bail!(
                "intégrale déjà présente : {} fichier(s) retrouvé(s) sur {}",
                ids.len(),
                pick.paths.len()
            );
        }
        qbit.set_file_priority(&announced, &ids, 1).await?;
        qbit.start(&announced, false).await?;
        let key = super::torrent_import::state_key(side_name(arr), &announced);
        ctx.state
            .update(|s| s.torrent_import.remove(&key))
            .await
            .ok();
        return Ok(format!(
            "déjà dans qBittorrent : {} fichier(s) ajouté(s) à la sélection",
            ids.len()
        ));
    }
    qbit.add_torrent_with(raw, "", tag, true).await?;
    let mut hash = None;
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        hash = qbit.torrents().await?.into_iter().find_map(|t| {
            let h = t.hash.to_ascii_lowercase();
            let ours = if announced.is_empty() {
                !before.contains(&h)
            } else {
                h == announced
            };
            ours.then_some(h)
        });
        if hash.is_some() {
            break;
        }
    }
    let hash = hash.context("intégrale ajoutée mais introuvable dans qBittorrent")?;
    let mut files = Vec::new();
    for _ in 0..8 {
        files = qbit.files(&hash).await.unwrap_or_default();
        if !files.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let keep = file_ids(&files, &pick.paths);
    if keep.len() != pick.paths.len() {
        // correspondance incertaine : retirée aussitôt (ajoutée arrêtée, rien n'a été écrit)
        qbit.delete(std::slice::from_ref(&hash), true).await.ok();
        bail!(
            "intégrale retirée : {} fichier(s) retrouvé(s) sur {} dans qBittorrent",
            keep.len(),
            pick.paths.len()
        );
    }
    let skip: Vec<usize> = (0..files.len()).filter(|i| !keep.contains(i)).collect();
    qbit.set_file_priority(&hash, &skip, 0).await?;
    qbit.start(&hash, false).await?;
    Ok(format!(
        "ajoutée à qBittorrent ({}) : {} fichier(s) sur {} sélectionné(s)",
        arr.name,
        keep.len(),
        files.len()
    ))
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
    let (cands, packs, integrales, how) =
        season_candidates(ctx, prow, arr, series, todo, throttle).await?;
    if how == "budget" {
        return Ok(SeasonOutcome::new("pending", String::new(), Vec::new()));
    }
    // ce que l'indexer n'a pas du tout : remonté tel quel sur /status.html (pas pendant une panne de C411)
    let gap = uncovered(&cands, &todo.missing_numbers);
    let left: Vec<i64> = if how == "secours" {
        Vec::new()
    } else {
        gap.iter().copied().collect()
    };
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
        choose_sized(
            &cands,
            todo.season,
            pack,
            &todo.missing_numbers,
            // taille d'un pack : jaugée sur les seuls épisodes manquants datés (voir `choose_sized`)
            todo.dated_missing.len(),
            &allowed,
            ix.max_gb_per_episode,
            ix.allow_no_french,
        )
    };
    // pack de saison si possible ; sinon toutes les releases d'épisodes manquants, en une fois
    let mut pack = pick(true);
    // Un pack n'est pas la saison tant qu'il reste des épisodes suivis à diffuser (`future_episodes`) : sur la
    // seedbox la release part directement dans qBittorrent, sans le contrôle de Sonarr. Les épisodes sortis
    // sont alors pris un à un, comme pour une saison en cours.
    let mut pack_note = String::new();
    if let Some(p) = pack {
        let eps = arr.episodes(todo.series_id).await?;
        let ahead = future_episodes(&eps, todo.season, chrono::Utc::now());
        if !ahead.is_empty() {
            let list: Vec<String> = ahead.iter().map(i64::to_string).collect();
            info!(
                task = "series_search",
                service = arr.name,
                series = title,
                season = todo.season,
                release = %p.title,
                a_venir = ?ahead,
                "pack de saison écarté : des épisodes suivis ne sont pas encore diffusés"
            );
            pack_note = format!(" ; pack écarté (épisodes à venir : {})", list.join(", "));
            pack = None;
        }
    }
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
            let src = source_of(c, &cfg.indexer);
            match send_release(ctx, prow, arr, &c.title, &c.release, src, target).await {
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
        // rien d'acceptable : une intégrale peut contenir la saison (jamais pendant une panne de C411)
        if how != "secours" {
            if let Some(r) =
                try_integrale(ctx, prow, arr, series, todo, integrales, &allowed, throttle).await?
            {
                return Ok(r);
            }
        }
        return Ok(SeasonOutcome::new(
            if how == "secours" {
                "fallback_none"
            } else {
                "none"
            },
            if how == "secours" {
                format!(
                    "C411 en panne ; secours {} : {} candidat(s), aucun acceptable",
                    fallback_label(&ctx.cfg.indexers, is_anime(series)),
                    cands.len()
                )
            } else {
                format!(
                    "{} candidat(s) {} (recherche {how}), aucun acceptable{pack_note}",
                    cands.len(),
                    cfg.indexer
                )
            },
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
    let (mut outcome, detail) = send_release(
        ctx,
        prow,
        arr,
        &c.title,
        &c.release,
        source_of(c, &cfg.indexer),
        target,
    )
    .await?;
    if outcome == "grabbed" && !c.full_season {
        outcome = "grabbed_episode".into();
    }
    info!(task = "series_search", service = arr.name, series = title, season = todo.season, release = %c.title, how, %outcome, %detail, "season sent");
    Ok(SeasonOutcome::new(&outcome, detail, left))
}

async fn plan_seasons(
    ctx: &TaskContext,
    arr: &ArrClient,
    series: &HashMap<i64, Value>,
) -> Result<Vec<SeasonTodo>> {
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
            dated_missing: HashSet::new(),
        });
        t.missing_numbers.insert(num);
        t.dated_missing.insert(num);
        if air > t.latest_air {
            t.latest_air = air;
        }
    }
    // épisodes sans date d'une saison commencée : absents de `wanted/missing`, lus série par série
    if cfg.undated_episodes {
        let now_dt = chrono::Utc::now();
        for (sid, ser) in series {
            if !open_recent_season(ser, now_dt, cfg.undated_window_days) {
                continue;
            }
            let eps = match arr.episodes(*sid).await {
                Ok(e) => e,
                Err(e) => {
                    warn!(task = "series_search", service = arr.name, series_id = sid, error = %e, "episodes unreadable (undated check)");
                    continue;
                }
            };
            for (season, num, latest) in undated_missing(&eps, &now_iso) {
                let t = by.entry((*sid, season)).or_insert(SeasonTodo {
                    series_id: *sid,
                    season,
                    latest_air: String::new(),
                    missing_numbers: HashSet::new(),
                    dated_missing: HashSet::new(),
                });
                t.missing_numbers.insert(num);
                if latest > t.latest_air {
                    t.latest_air = latest;
                }
            }
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
    // une saison laissée par le secours repart dès que C411 répond (sonde sans quota, une fois par passage)
    let c411_up = match &ctx.prowlarr {
        Some(p) if records.values().any(|r| r.outcome == "fallback_none") => {
            crate::indexer::c411_up(ctx, p).await
        }
        _ => true,
    };
    // une saison notée « épisodes introuvables » et qui n'a plus rien de manquant (importée entre-temps,
    // par un pack de cours ou à la main) sort de la liste « Saisons sans release » de /status.html
    let stale = stale_uncovered(&records, arr.name, &by.keys().copied().collect());
    if !stale.is_empty() && !ctx.dry_run {
        ctx.state
            .update(|st| {
                for k in &stale {
                    if let Some(r) = st.unknown_series.get_mut(k) {
                        r.uncovered.clear();
                    }
                }
            })
            .await?;
    }
    let t = now();
    Ok(by
        .into_values()
        .filter(|s| !queued.contains(&(s.series_id, s.season)))
        // voie russe : RuTracker par l'Arr (anime_library), jamais C411
        .filter(|s| {
            !series.get(&s.series_id).is_some_and(|v| {
                super::anime_library::russian_route(v, &ctx.cfg.tasks.anime_library)
            })
        })
        .filter(|s| {
            let rec = records.get(&key(arr, s.series_id, s.season));
            fallback_due(rec, t, ctx.cfg.indexers.fallback_retry_hours, c411_up).unwrap_or_else(
                || {
                    due(
                        rec,
                        t,
                        cfg.retry_after_hours,
                        cfg.grabbed_retry_hours,
                        cfg.error_retry_hours,
                        cfg.episode_retry_mins,
                    )
                },
            )
        })
        .collect())
}

fn key(arr: &ArrClient, series: i64, season: i64) -> String {
    format!("{}:{series}:{season}", arr.name)
}

/// Clés des enregistrements de cet Arr qui gardent des épisodes « introuvables » alors que la saison n'a
/// plus aucun épisode manquant (`present` = saisons ayant encore un manque).
pub fn stale_uncovered(
    records: &BTreeMap<String, SeasonSearchRecord>,
    arr_name: &str,
    present: &HashSet<(i64, i64)>,
) -> Vec<String> {
    records
        .iter()
        .filter(|(_, r)| !r.uncovered.is_empty())
        .filter_map(|(k, _)| {
            let rest = k.strip_prefix(arr_name)?.strip_prefix(':')?;
            let (series, season) = rest.split_once(':')?;
            let id = (series.parse().ok()?, season.parse().ok()?);
            (!present.contains(&id)).then(|| k.clone())
        })
        .collect()
}

#[async_trait]
impl Task for SeriesSearch {
    fn name(&self) -> &'static str {
        "series_search"
    }

    fn label(&self) -> &'static str {
        "Recherche des séries (TMDB)"
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
                let series: HashMap<i64, Value> = arr
                    .series()
                    .await?
                    .into_iter()
                    .filter_map(|s| Some((s.get("id").and_then(Value::as_i64)?, s)))
                    .collect();
                let todo = plan_seasons(ctx, arr, &series).await?;
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

    #[test]
    fn fallback_names_are_clean_and_limited() {
        let n = vec![
            "Re:Zero".to_string(),
            "Re:Zero kara Hajimeru Isekai Seikatsu".to_string(),
            "Re: ZERO".to_string(),
        ];
        assert_eq!(
            fallback_names(&n, 2),
            vec!["Re Zero", "Re Zero kara Hajimeru Isekai Seikatsu"]
        );
        assert_eq!(
            fallback_names(&["L'Attaque des Titans".to_string()], 2),
            vec!["L'Attaque des Titans"]
        );
    }
    use super::*;

    fn ep(season: i64, num: i64, air: Option<&str>, monitored: bool, has_file: bool) -> Value {
        let mut v = serde_json::json!({
            "seasonNumber": season, "episodeNumber": num, "monitored": monitored, "hasFile": has_file
        });
        if let Some(a) = air {
            v["airDateUtc"] = Value::String(a.into());
        }
        v
    }

    #[test]
    fn vof_is_french_audio() {
        assert_eq!(
            lang_rank("Le.voyageur.S04E04.VOF.1080p.WEB.AAC.2.0.H264-THESYNDiCATE"),
            4
        );
        assert_eq!(lang_rank_for("Show.S01.VOF.1080p.WEB.H264", true), 2);
        assert_eq!(
            lang_rank("Show.S01.VOSTFR.1080p"),
            1,
            "VOSTFR reste une VO sous-titrée"
        );
    }

    #[test]
    fn undated_episodes_of_a_started_season_are_wanted() {
        // Le Voyageur (2026-09-23) : S04E01 daté et diffusé, E02-E03 sans date ; S05 pas commencée
        let eps = vec![
            ep(4, 1, Some("2025-11-29T20:00:00Z"), true, false),
            ep(4, 2, None, true, false),
            ep(4, 3, Some(""), true, false),
            ep(4, 4, None, false, false), // non suivi
            ep(3, 5, None, true, true),   // déjà là
            ep(5, 1, None, true, false),  // saison pas commencée
            ep(6, 1, Some("2027-01-01T00:00:00Z"), true, false), // à venir, pas commencée
            ep(6, 2, None, true, false),
            ep(0, 3, None, true, false), // spéciaux
        ];
        let got = undated_missing(&eps, "2026-09-23T12:00:00Z");
        assert_eq!(
            got,
            vec![
                (4, 2, "2025-11-29T20:00:00Z".to_string()),
                (4, 3, "2025-11-29T20:00:00Z".to_string())
            ]
        );
    }

    #[test]
    fn only_recent_incomplete_monitored_seasons_are_examined() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ser = |monitored: bool, files: i64, prev: &str| {
            serde_json::json!({"monitored": monitored, "seasons": [
                {"seasonNumber": 4, "monitored": true,
                 "statistics": {"totalEpisodeCount": 3, "episodeFileCount": files, "previousAiring": prev}}
            ]})
        };
        assert!(open_recent_season(
            &ser(true, 0, "2025-11-29T20:00:00Z"),
            now,
            730
        ));
        assert!(
            !open_recent_season(&ser(true, 3, "2025-11-29T20:00:00Z"), now, 730),
            "complète"
        );
        assert!(
            !open_recent_season(&ser(false, 0, "2025-11-29T20:00:00Z"), now, 730),
            "non suivie"
        );
        assert!(
            !open_recent_season(&ser(true, 0, "2020-01-01T00:00:00Z"), now, 730),
            "trop ancienne"
        );
    }

    #[test]
    fn stale_uncovered_only_for_complete_seasons_of_this_arr() {
        let rec = |unc: Vec<i64>| SeasonSearchRecord {
            at: 0,
            outcome: "none".into(),
            detail: String::new(),
            title: "Bleach".into(),
            uncovered: unc,
        };
        let mut records = BTreeMap::new();
        records.insert("sonarr-seedbox:5:17".to_string(), rec(vec![27, 28])); // complète depuis
        records.insert("sonarr-seedbox:5:16".to_string(), rec(vec![3])); // manque encore
        records.insert("sonarr-seedbox:9:1".to_string(), rec(vec![])); // rien à effacer
        records.insert("sonarr:5:17".to_string(), rec(vec![27])); // autre Arr, pas concerné
        let present: HashSet<(i64, i64)> = [(5, 16)].into_iter().collect();
        assert_eq!(
            stale_uncovered(&records, "sonarr-seedbox", &present),
            vec!["sonarr-seedbox:5:17".to_string()]
        );
    }

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
        assert!(cour_pack(&r, &parse_of(Some(60), 3, true, &[3]), 60, 17, true).is_some());
        // une autre fiche, ou aucune : on ne sait rien
        assert!(cour_pack(&r, &parse_of(Some(61), 3, true, &[3]), 60, 17, true).is_none());
        assert!(cour_pack(&r, &parse_of(None, 3, true, &[3]), 60, 17, true).is_none());
        // déjà la bonne saison : le chemin normal s'en occupe
        assert!(cour_pack(&r, &parse_of(Some(60), 17, true, &[17]), 60, 17, true).is_none());
        // pas un pack complet
        assert!(cour_pack(&r, &parse_of(Some(60), 3, false, &[3]), 60, 17, true).is_none());
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
        assert!(cour_pack(
            &r,
            &parse_of(Some(60), 1, true, &[17, 17, 17]),
            60,
            17,
            true
        )
        .is_none());
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
        let c = series_candidate(&r, &info(17, false, &[9], 9, 1080), 17, false).unwrap();
        assert_eq!(
            c.release.get("infoHash").and_then(Value::as_str),
            Some("3AEED2C5F1CD7F684F1702BC11190D937A7B5E0E")
        );
        // release sans infoHash : le champ est présent mais nul, jamais d'erreur
        let c2 = series_candidate(
            &result("Bleach.S17E10.MULTI.VFF.1080p", 30984, 3),
            &info(17, false, &[10], 9, 1080),
            17,
            false,
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
    fn codec_rank_reads_real_titles() {
        assert_eq!(
            codec_rank("Bleach.S17E01.MULTI.VFF.1080p.BluRay.x265-KAF"),
            2
        );
        assert_eq!(codec_rank("Dune.2021.MULTi.1080p.WEB.H.265-GRP"), 2);
        assert_eq!(codec_rank("Show S01 VOSTFR 1080p HEVC 10bits"), 2);
        assert_eq!(codec_rank("Show.S01E01.MULTI.VFF.1080p.WEB.H264-GRP"), 1);
        assert_eq!(codec_rank("Show.S01E01.FRENCH.1080p.HDTV"), 1);
        assert_eq!(codec_rank("Show.S01E01.MULTI.1080p.WEB.AV1.10bit-GRP"), 0);
        // un nom de groupe qui contient « av1 » n'est pas de l'AV1
        assert_eq!(codec_rank("Show.S01E01.MULTI.1080p.x264-AV1ON"), 1);
    }

    #[test]
    fn audio_rank_reads_real_titles() {
        assert_eq!(
            audio_rank(
                "Hunter.X.Hunter.2011.INTEGRALE.MULTI.VFF.1080p.BluRay.AAC.2.0.x265-Phoenix"
            ),
            3
        );
        assert_eq!(
            audio_rank("Repo.Men.2010.MULTI.VFF.1080p.WEB.EAC3.5.1.H265-Seigneuraltair"),
            3
        );
        assert_eq!(audio_rank("Show.S01.MULTI.1080p.WEB.DDP5.1.x265"), 3);
        assert_eq!(
            audio_rank("Hunter.x.Hunter.2011.S02.VOSTFR.1080p.WEB.FLAC.2.0.x264-Kitsune"),
            2
        );
        assert_eq!(
            audio_rank("L.Attaque.des.Titans.S03.MULTI.VFF.1080p.BluRay.DTS.2.0.x264-KAZETV"),
            1
        );
        assert_eq!(
            audio_rank("L.Attaque.des.Titans.S02.MULTI.VFF.1080p.BluRay.DTS.HD.MA.2.0.x264"),
            0
        );
        assert_eq!(
            audio_rank("Film.2001.MULTi.VFF.1080p.BluRay.DTS-HDMA.x264"),
            0
        );
        assert_eq!(
            audio_rank("Film.2021.MULTi.1080p.BluRay.TrueHD.Atmos.7.1.x265"),
            0
        );
        assert_eq!(audio_rank("Film.2021.MULTi.1080p.WEB.x265"), 3);
    }

    #[test]
    fn audio_breaks_ties_after_codec() {
        let missing: HashSet<i64> = [1].into_iter().collect();
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let mk = |title: &str, seeders: i64| {
            series_candidate(
                &result(title, 30984, seeders),
                &info(1, false, &[1], 9, 1080),
                1,
                false,
            )
            .unwrap()
        };
        let pick = |c: &[Candidate]| {
            choose(c, 1, false, &missing, &allowed, 6.0, true)
                .unwrap()
                .title
                .clone()
        };
        // même codec : l'AAC passe devant le DTS mieux partagé
        let c = vec![
            mk("Show.S01E01.MULTI.VFF.1080p.DTS.x265-A", 90),
            mk("Show.S01E01.MULTI.VFF.1080p.AAC.x265-B", 6),
        ];
        assert!(pick(&c).ends_with("x265-B"));
        // le codec vidéo passe avant l'audio : x265 DTS bat x264 AAC
        let c = vec![
            mk("Show.S01E01.MULTI.VFF.1080p.AAC.x264-A", 90),
            mk("Show.S01E01.MULTI.VFF.1080p.DTS.x265-B", 6),
        ];
        assert!(pick(&c).ends_with("x265-B"));
    }

    #[test]
    fn x265_first_then_x264_then_av1() {
        // 2026-09-26 : à langue, résolution et partage égaux, x265 > x264 > AV1 (voir `codec_rank`).
        let missing: HashSet<i64> = [1].into_iter().collect();
        let allowed: HashSet<i64> = [9].into_iter().collect();
        let mk = |title: &str, seeders: i64, anime: bool| {
            series_candidate(
                &result(title, 30984, seeders),
                &info(1, false, &[1], 9, 1080),
                1,
                anime,
            )
            .unwrap()
        };
        let pick = |cands: &[Candidate]| {
            choose(cands, 1, false, &missing, &allowed, 6.0, true)
                .unwrap()
                .title
                .clone()
        };
        // le x265 gagne même moins partagé
        let c = vec![
            mk("Show.S01E01.MULTI.VFF.1080p.x264-B", 120, false),
            mk("Show.S01E01.MULTI.VFF.1080p.x265-A", 5, false),
        ];
        assert!(pick(&c).ends_with("x265-A"));
        // le x264 passe devant l'AV1, l'AV1 seul est pris
        let c = vec![
            mk("Show.S01E01.MULTI.1080p.WEB.AV1-C", 80, false),
            mk("Show.S01E01.MULTI.1080p.WEB.x264-B", 10, false),
        ];
        assert!(pick(&c).ends_with("x264-B"));
        assert!(pick(&c[..1]).ends_with("AV1-C"));
        // la langue reste prioritaire : un x264 en VF bat un x265 en VOSTFR
        let c = vec![
            mk("Show.S01E01.VOSTFR.1080p.x265-A", 999, false),
            mk("Show.S01E01.VFF.1080p.x264-B", 5, false),
        ];
        assert!(pick(&c).contains("VFF"));
        // un x265 à une seule source ne passe pas devant un x264 bien partagé
        let c = vec![
            mk("Show.S01E01.MULTI.1080p.x265-A", 1, false),
            mk("Show.S01E01.MULTI.1080p.x264-B", 10, false),
        ];
        assert!(pick(&c).ends_with("x264-B"));
        // animé : entre deux MULTi, le x265
        let c = vec![
            mk("Show.S01E01.MULTI.VFF.1080p.x264-B", 50, true),
            mk("Show.S01E01.MULTI.VFF.1080p.x265-A", 8, true),
        ];
        assert!(pick(&c).ends_with("x265-A"));
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
                    true,
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
            false,
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
            false
        )
        .is_none());
    }

    #[test]
    fn fansub_numbering_is_read_where_sonarr_sees_nothing() {
        // Cas Erased : Sonarr lit « S01 » comme une saison entière et ignore le « - 06 ».
        assert_eq!(
            fansub_episode("Erased S01 - 06 VOSTFR [1080p][X265][10BITS][SR-71].mkv"),
            Some(6)
        );
        assert_eq!(
            fansub_episode("/dl/P/Erased S01 - 12 VOSTFR [1080p][X265].mkv"),
            Some(12)
        );
        // rien à attraper : ni la résolution, ni le codec, ni l'année, ni le groupe
        for n in [
            "Erased.S01E06.VOSTFR.1080p.x265-SR71.mkv",
            "Erased 2016 - 1080p - x265.mkv",
            "Erased S01 -06 VOSTFR.mkv",
            "BONUS/Creditless OP. Re Re.mkv",
        ] {
            assert_eq!(fansub_episode(n), None, "{n}");
        }
    }

    #[test]
    fn anime_prefers_multi_then_vostfr() {
        // Demandé le 2026-09-18 : pour un animé, MULTi (les deux pistes) puis VOSTFR (audio japonais)
        // passent devant le doublage seul.
        let r = |t: &str| lang_rank_for(t, true);
        assert_eq!(r("Bleach.S17E01.MULTI.VFF.1080p.BluRay.x265-KAF"), 4);
        assert_eq!(r("Erased.S01.VOSTFR.1080p.BluRay.x265-SR71"), 3);
        assert_eq!(r("Anime.S01.VFF.1080p"), 2);
        assert_eq!(r("Anime.S01.FRENCH.1080p"), 1);
        assert_eq!(r("Shingeki.no.Kyojin.S04.1080p.WEB.x264"), 0);
        // la VOSTFR bat le doublage pour un animé, l'inverse pour une série classique
        assert!(r("A.S01.VOSTFR.1080p") > r("A.S01.VFF.1080p"));
        assert!(lang_rank("A.S01.VOSTFR.1080p") < lang_rank("A.S01.VFF.1080p"));
        // « MULTI.VFF » reste du VF pour une série classique (comportement d'origine)
        assert_eq!(lang_rank("L.Attaque.Des.Titans.S04.MULTI.VFF.1080p"), 4);
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
        let c = series_candidate(&r, &info(4, true, &[], 9, 1080), 4, false).unwrap();
        assert!(c.full_season && c.codec == 1 && c.lang_rank == 4 && c.resolution == 1080);
        assert!(series_candidate(&r, &info(3, true, &[], 9, 1080), 4, false).is_none());
        // VO : gardée avec le rang 0 (prise seulement en dernier recours par `choose`)
        let vo = result("Shingeki.no.Kyojin.S04.1080p.WEB", 1429, 65);
        assert_eq!(
            series_candidate(&vo, &info(4, true, &[], 9, 1080), 4, false)
                .unwrap()
                .lang_rank,
            0
        );
    }

    #[test]
    fn chooses_best_pack_within_limits() {
        let mk = |t: &str, res: i64, qid: i64, seeders: i64| {
            series_candidate(
                &result(t, 1, seeders),
                &info(4, true, &[], qid, res),
                4,
                false,
            )
            .unwrap()
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

    fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn a_pack_waits_for_the_followed_episodes_still_to_air() {
        // Black Clover S02 le 05/10 : E01 diffusé (03/10), E02-E03 datés à venir, E04-E13 sans date
        let mut eps = vec![
            ep(2, 1, Some("2026-10-03T10:00:00Z"), true, true),
            ep(2, 2, Some("2026-10-10T10:00:00Z"), true, false),
            ep(2, 3, Some("2026-10-17T10:00:00Z"), true, false),
        ];
        eps.extend((4..=13).map(|n| ep(2, n, None, true, false)));
        let now = utc("2026-10-05T12:00:00Z");
        assert_eq!(future_episodes(&eps, 2, now), vec![2, 3]);
        // la veille de la sortie d'E02 comme le jour même : la date à venir bloque encore jusqu'à son heure
        assert_eq!(
            future_episodes(&eps, 2, utc("2026-10-10T09:59:59Z")),
            vec![2, 3]
        );
        assert_eq!(
            future_episodes(&eps, 2, utc("2026-10-10T10:00:00Z")),
            vec![3],
            "à l'heure exacte, l'épisode est sorti"
        );
        // tout est diffusé ou sans date : plus rien ne bloque
        assert!(future_episodes(&eps, 2, utc("2026-12-01T00:00:00Z")).is_empty());
        // une autre saison n'est pas concernée
        assert!(future_episodes(&eps, 1, now).is_empty());
        // un épisode non suivi n'empêche pas la saison
        let unfollowed = vec![ep(2, 2, Some("2026-10-10T10:00:00Z"), false, false)];
        assert!(future_episodes(&unfollowed, 2, now).is_empty());
        // Le Voyageur : saison dont les épisodes n'ont pas de date (ou une date illisible) reste servie
        let undated = vec![
            ep(4, 1, Some("2025-11-29T20:00:00Z"), true, true),
            ep(4, 2, None, true, false),
            ep(4, 3, Some(""), true, false),
            ep(4, 4, Some("pas une date"), true, false),
        ];
        assert!(future_episodes(&undated, 4, now).is_empty());
    }

    #[test]
    fn pack_size_is_judged_on_the_dated_missing_episodes() {
        // Black Clover S02 : « S02 » VOSTFR de 51 fichiers (ancienne numérotation), 15,2 Go
        let wrong = json!({"title": "Black.Clover.S02.VOSTFR.WebRip.1080p.H265-LTFR", "downloadUrl": "http://p/dl",
                           "tmdbId": 1, "seeders": 54, "size": 15_210_000_000i64});
        let cands = vec![series_candidate(&wrong, &info(2, true, &[], 9, 1080), 2, true).unwrap()];
        let allowed: HashSet<i64> = [9].into_iter().collect();
        // manquants : E01 (daté, diffusé) + E04-E13 (sans date) = 11 ; un seul est daté
        let missing: HashSet<i64> = [1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].into_iter().collect();
        // avant : 11 épisodes comptés → 1,3 Gio chacun, sous le plafond de 3 → pack pris
        assert!(choose(&cands, 2, true, &missing, &allowed, 3.0, true).is_some());
        // maintenant : 1 épisode daté → 14,2 Gio pour lui seul, pack refusé
        assert!(choose_sized(&cands, 2, true, &missing, 1, &allowed, 3.0, true).is_none());
        // une vraie saison de 13 épisodes tous diffusés et manquants reste prise (13 × 1 Gio)
        let real = json!({"title": "Black.Clover.S02.MULTi.1080p.WEB.x265", "downloadUrl": "http://p/dl",
                          "tmdbId": 1, "seeders": 20, "size": 13_000_000_000i64});
        let cands = vec![series_candidate(&real, &info(2, true, &[], 9, 1080), 2, true).unwrap()];
        let all: HashSet<i64> = (1..=13).collect();
        assert!(choose_sized(&cands, 2, true, &all, 13, &allowed, 3.0, true).is_some());
        // `choose` reste `choose_sized` avec tous les manquants
        assert_eq!(
            choose(&cands, 2, true, &all, &allowed, 3.0, true).map(|c| &c.title),
            choose_sized(&cands, 2, true, &all, all.len(), &allowed, 3.0, true).map(|c| &c.title)
        );
    }

    #[test]
    fn vo_only_as_a_last_resort_and_size_capped() {
        let big = json!({"title": "A.S04.VFF.1080p.BluRay.x264", "downloadUrl": "http://p/dl", "tmdbId": 1,
                         "seeders": 1, "size": 134_000_000_000i64});
        let mk = |v: Value, res: i64, qid: i64| {
            series_candidate(&v, &info(4, true, &[], qid, res), 4, false).unwrap()
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
            series_candidate(&result(t, 1, 20), &info(1, false, &[ep], 9, 1080), 1, false).unwrap()
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
            series_candidate(
                &result(t, 1, seeders),
                &info(4, true, &[], qid, 1080),
                4,
                false,
            )
            .unwrap()
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
            series_candidate(&result(t, 1, 4), &info(2, false, &[ep], 9, 1080), 2, false).unwrap()
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
        // secours sans résultat : 12 h tant que C411 est en panne, aussitôt s'il répond
        assert_eq!(
            fallback_due(Some(&r("fallback_none", 0)), 3600, 12, false),
            Some(false)
        );
        assert_eq!(
            fallback_due(Some(&r("fallback_none", 0)), 12 * 3600, 12, false),
            Some(true)
        );
        assert_eq!(
            fallback_due(Some(&r("fallback_none", 0)), 60, 12, true),
            Some(true)
        );
        assert_eq!(fallback_due(Some(&r("none", 0)), 60, 12, true), None);
        assert_eq!(fallback_due(None, 60, 12, true), None);
        let ix = crate::config::Indexers::default();
        assert_eq!(fallback_label(&ix, true), "Nyaa.si + World-torrent");
        assert_eq!(fallback_label(&ix, false), "World-torrent");
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

    #[test]
    fn an_integrale_is_a_pack_without_a_readable_season() {
        // Space Dandy, 2026-10-03 : Sonarr ne renvoie rien du tout pour le titre de l'intégrale
        assert!(season_less_pack(&json!({"title": "Space.Dandy.INTEGRALE"})));
        assert!(season_less_pack(
            &json!({"parsedEpisodeInfo": null, "title": "x"})
        ));
        assert!(season_less_pack(
            &json!({"parsedEpisodeInfo": {"seasonNumber": 1, "isMultiSeason": true}})
        ));
        assert!(!season_less_pack(
            &json!({"parsedEpisodeInfo": {"seasonNumber": 1, "fullSeason": true}})
        ));
        assert!(!season_less_pack(
            &json!({"parsedEpisodeInfo": {"seasonNumber": 2, "episodeNumbers": [5]}})
        ));
    }

    #[test]
    fn the_quality_of_an_integrale_is_read_with_a_season_marker() {
        assert_eq!(
            integrale_quality_title(
                "Space.Dandy.INTEGRALE.MULTI.VFF.1080p.BluRay.AAC.2.0.x265-NOTAG"
            )
            .as_deref(),
            Some("Space.Dandy.S01.MULTI.VFF.1080p.BluRay.AAC.2.0.x265-NOTAG")
        );
        assert_eq!(
            integrale_quality_title("Naruto L'INTÉGRALE VOSTFR 720p").as_deref(),
            Some("Naruto L'S01 VOSTFR 720p")
        );
        assert_eq!(
            integrale_quality_title("Breaking Bad COMPLETE MULTi 1080p").as_deref(),
            Some("Breaking Bad S01 MULTi 1080p")
        );
        // sans marqueur : S01 devant la première étiquette de langue ou de qualité
        assert_eq!(
            integrale_quality_title("Space.Dandy.MULTI.VFF.1080p.BluRay").as_deref(),
            Some("Space.Dandy.S01.MULTI.VFF.1080p.BluRay")
        );
        // « Complètement » n'est pas un marqueur
        assert_eq!(
            integrale_quality_title("Complètement.Cramé.FRENCH.720p").as_deref(),
            Some("Complètement.Cramé.S01.FRENCH.720p")
        );
        assert_eq!(integrale_quality_title("Space Dandy"), None);
    }

    /// Les 26 fichiers de l'intégrale de Space Dandy : `01xNN` puis `02xNN` (numérotation TMDB), que Sonarr
    /// rattache à S01E01–E26 (TVDB) par scene mapping.
    fn space_dandy() -> Vec<IntegraleFile> {
        (1..=26)
            .map(|n| IntegraleFile {
                path: if n <= 13 {
                    format!("Space.Dandy.S01/Space.Dandy.01x{n:02}.mkv")
                } else {
                    format!("Space.Dandy.S02/Space.Dandy.02x{:02}.mkv", n - 13)
                },
                size: 300_000_000,
                episodes: vec![(1, n)],
            })
            .collect()
    }

    #[test]
    fn an_integrale_gives_every_missing_episode_of_the_season() {
        let missing: HashSet<i64> = (1..=26).collect();
        let p = integrale_pick(&space_dandy(), 1, &missing).unwrap();
        assert_eq!(p.paths.len(), 26);
        assert_eq!(p.covered, (1..=26).collect::<BTreeSet<i64>>());
        assert_eq!(p.bytes, 26 * 300_000_000);
        assert!(p
            .paths
            .contains(&"Space.Dandy.S02/Space.Dandy.02x01.mkv".to_string()));
    }

    #[test]
    fn an_integrale_never_replaces_a_file_nor_touches_another_season() {
        // E01–13 déjà là : seuls les fichiers des épisodes 14 à 26
        let missing: HashSet<i64> = (14..=26).collect();
        let p = integrale_pick(&space_dandy(), 1, &missing).unwrap();
        assert_eq!(p.paths.len(), 13);
        assert!(p.paths.iter().all(|x| x.contains("02x")));
        // un fichier lu sur une autre saison n'est jamais pris
        let mut files = space_dandy();
        files.push(IntegraleFile {
            path: "Space.Dandy.S03/Space.Dandy.03x01.mkv".into(),
            size: 1,
            episodes: vec![(2, 1)],
        });
        let p = integrale_pick(&files, 1, &(1..=26).collect()).unwrap();
        assert_eq!(p.paths.len(), 26);
        // un fichier double (E03-E04) dont un épisode est déjà là : écarté
        let files = vec![IntegraleFile {
            path: "E03-E04.mkv".into(),
            size: 1,
            episodes: vec![(1, 3), (1, 4)],
        }];
        assert!(integrale_pick(&files, 1, &[3].into_iter().collect()).is_err());
        // rien qui tombe dans le trou, ou des fichiers que Sonarr ne rattache pas à la fiche
        assert!(integrale_pick(&space_dandy(), 2, &(1..=5).collect()).is_err());
        let unknown = vec![IntegraleFile {
            path: "01. Titre.mkv".into(),
            size: 1,
            episodes: vec![],
        }];
        assert!(integrale_pick(&unknown, 1, &(1..=26).collect()).is_err());
    }

    #[test]
    fn two_versions_of_one_episode_are_refused() {
        let mut files = space_dandy();
        files.push(IntegraleFile {
            path: "Extras/Space.Dandy.01x05.v2.mkv".into(),
            size: 1,
            episodes: vec![(1, 5)],
        });
        assert_eq!(
            integrale_pick(&files, 1, &(1..=26).collect()),
            Err("deux fichiers pour un même épisode")
        );
    }

    #[test]
    fn the_season_less_query_waits_for_old_episodes() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z")
            .unwrap()
            .timestamp();
        assert!(aired_before("2014-03-29T15:00:00Z", now, 14));
        assert!(aired_before("2026-09-19T12:00:00Z", now, 14));
        assert!(!aired_before("2026-09-30T12:00:00Z", now, 14));
        assert!(!aired_before("", now, 14));
    }

    #[test]
    fn qbittorrent_files_are_matched_by_exact_path() {
        let f = |name: &str| TorrentFile {
            name: name.into(),
            size: 1,
            progress: 0.0,
            priority: 1,
        };
        let files = vec![
            f("Space.Dandy/Space.Dandy.S01/Space.Dandy.01x01.mkv"),
            f("Space.Dandy/Space.Dandy.S01/Space.Dandy.01x02.mkv"),
            f("Space.Dandy/Bonus/Space.Dandy.S01/Space.Dandy.01x01.mkv"),
            f("Space.Dandy/Space.Dandy.nfo"),
        ];
        let wanted = vec!["Space.Dandy.S01/Space.Dandy.01x01.mkv".to_string()];
        // le même nom plus profond (Bonus/…) n'est pas pris : comparaison exacte sous la racine
        assert_eq!(file_ids(&files, &wanted), vec![0]);
        // torrent mono-fichier : le nom seul
        let single = vec![f("Film.2023.1080p.mkv")];
        assert_eq!(
            file_ids(&single, &["Film.2023.1080p.mkv".to_string()]),
            vec![0]
        );
    }
}
