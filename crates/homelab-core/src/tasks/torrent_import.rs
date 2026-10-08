//! Importe dans Radarr/Sonarr les torrents ajoutés à la main dans qBittorrent (VPS et seedbox),
//! pour qu'ils apparaissent dans Jellyfin comme le reste.
//!
//! Pour chaque torrent terminé pas encore jugé : ignoré s'il est suivi par un Arr (historique du
//! téléchargement), s'il n'a pas de vidéo, ou si ses vidéos sont déjà hardlinkées en bibliothèque
//! (VPS) ; sinon parse → recherche → choix de la fiche (`matching`), refus si la fiche a déjà des
//! fichiers sur l'autre machine (doublon dans Jellyfin), ajout de la fiche **non surveillée** si
//! besoin, puis `ManualImport` en `importMode: copy` (= hardlink) des seuls fichiers sans rejet.
//!
//! Jamais `importMode: auto` : pour un téléchargement que l'Arr n'a pas demandé, `auto` déplace
//! le fichier et casse le seed. Aucune modification des torrents (catégorie, chemin).
//! Jellyfin : le LibraryMonitor voit les imports du VPS ; `seedbox_refresh` ceux de la seedbox.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::{Report, Task};
use crate::classify::{classify, is_video, MediaKind};
use crate::clients::{ArrClient, Torrent, TorrentFile};
use crate::config::Config;
use crate::context::{Side, TaskContext};
use crate::matching::{normalize, parsed_movie, parsed_series, pick_movie, pick_series, Match};
use crate::state::{now, TorrentImportRecord};

pub struct TorrentImport;

/// Résultat de l'examen d'un torrent.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub outcome: &'static str,
    pub detail: String,
    pub files: u32,
    /// L'examen a coûté des appels lourds (parse, recherche, import) : compte dans le budget.
    pub costly: bool,
}

impl Outcome {
    fn cheap(outcome: &'static str, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: detail.into(),
            files: 0,
            costly: false,
        }
    }
    fn costly(outcome: &'static str, detail: impl Into<String>) -> Self {
        Self {
            costly: true,
            ..Self::cheap(outcome, detail)
        }
    }
}

/// Cible désignée par une étiquette qBittorrent `homelab:` (posée par `series_search` / `movie_search`).
/// Les étiquettes sont séparées par des virgules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagTarget {
    Movie {
        id: i64,
    },
    Series {
        id: i64,
        /// Saison visée à la prise (`season=` de l'étiquette) : seul cadre où un numéro d'épisode « nu »
        /// (`Angels of Death - 07`) ou en tête (`05. Titre de l'épisode`) est lu, voir `bare_episodes` et
        /// `numbered_episodes`.
        season: Option<i64>,
    },
    /// Pack d'un cours d'animé publié sous son propre titre : l'Arr lit la **mauvaise saison** dans les
    /// noms de fichiers (« S03E01 » = saison 3 de Bleach). L'épisode visé vaut `numéro lu + offset`,
    /// dans `season`, et doit tomber dans `from..=to` — somme de contrôle décidée à la prise.
    CourPack {
        id: i64,
        season: i64,
        offset: i64,
        from: i64,
        to: i64,
    },
    /// Étiquette `homelab:` reconnue mais illisible. **Jamais** traitée comme une étiquette ordinaire :
    /// retomber sur l'analyse de nom importerait le pack dans la saison que Sonarr croit lire.
    Broken(String),
}

impl TagTarget {
    pub fn id(&self) -> Option<i64> {
        match self {
            TagTarget::Movie { id } | TagTarget::Series { id, .. } => Some(*id),
            TagTarget::CourPack { id, .. } => Some(*id),
            TagTarget::Broken(_) => None,
        }
    }
    pub fn is_movie(&self) -> bool {
        matches!(self, TagTarget::Movie { .. })
    }
}

/// `homelab:series=<id>:season=<n>[:offset=<k>:eps=<from>-<to>]` ou `homelab:movie=<id>`.
/// Une étiquette sans `offset` garde le comportement d'origine ; une étiquette à décalage incomplète
/// donne `Broken` (on refuse plutôt que de deviner).
pub fn homelab_target(tags: &str) -> Option<TagTarget> {
    tags.split(',').map(str::trim).find_map(|t| {
        let rest = t.strip_prefix("homelab:")?;
        let (kind, tail) = rest.split_once('=')?;
        let mut parts = tail.split(':');
        let id: i64 = parts.next()?.parse().ok()?;
        let movie = match kind {
            "series" => false,
            "movie" => true,
            _ => return None,
        };
        let (mut season, mut offset, mut eps) = (None, None, None);
        for p in parts {
            match p.split_once('=') {
                Some(("season", v)) => season = v.parse::<i64>().ok(),
                Some(("offset", v)) => offset = v.parse::<i64>().ok(),
                Some(("eps", v)) => {
                    eps = v
                        .split_once('-')
                        .and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?)))
                }
                _ => {}
            }
        }
        // une étiquette qui annonce un décalage doit être lisible ENTIÈREMENT
        if t.contains(":offset=") {
            let (Some(season), Some(offset), Some((from, to))) = (season, offset, eps) else {
                return Some(TagTarget::Broken(t.to_string()));
            };
            if movie || from > to {
                return Some(TagTarget::Broken(t.to_string()));
            }
            return Some(TagTarget::CourPack {
                id,
                season,
                offset,
                from,
                to,
            });
        }
        Some(if movie {
            TagTarget::Movie { id }
        } else {
            TagTarget::Series { id, season }
        })
    })
}

pub fn state_key(side: &str, hash: &str) -> String {
    format!("{side}:{}", hash.to_ascii_lowercase())
}

/// Terminé, chemin connu, et pas encore jugé définitivement.
pub fn is_candidate(t: &Torrent, record: Option<&TorrentImportRecord>) -> bool {
    t.progress >= 1.0 && !t.content_path.is_empty() && record.map(|r| !r.is_final()).unwrap_or(true)
}

/// Chemins (vus par l'Arr, qui partage les chemins de qBittorrent) des fichiers du torrent **pas entièrement
/// téléchargés** : désélectionnés ou incomplets. Un torrent « terminé » peut en contenir (fichiers désélectionnés
/// après le début du téléchargement) : ils ne sont jamais importés.
pub fn incomplete_paths(files: &[TorrentFile], save_path: &str) -> HashSet<String> {
    let base = save_path.trim_end_matches('/');
    files
        .iter()
        .filter(|f| f.progress < 1.0 || f.priority == 0)
        .map(|f| format!("{base}/{}", f.name))
        .collect()
}

/// Vidéos du torrent, sans les extraits (`sample`).
pub fn video_files(files: &[TorrentFile]) -> Vec<&TorrentFile> {
    files
        .iter()
        .filter(|f| {
            let base = f.name.rsplit('/').next().unwrap_or(&f.name);
            is_video(base)
                && !f
                    .name
                    .to_ascii_lowercase()
                    .split(['/', '.', '-', '_', ' '])
                    .any(|w| w == "sample")
        })
        .collect()
}

/// Libellés à soumettre au parse : le nom du torrent, puis le premier fichier vidéo (ordre alphabétique).
pub fn parse_labels(name: &str, videos: &[&TorrentFile]) -> Vec<String> {
    let mut out = vec![name.to_string()];
    let first = videos
        .iter()
        .map(|f| f.name.rsplit('/').next().unwrap_or(&f.name))
        .min();
    if let Some(f) = first {
        if f != name {
            out.push(f.to_string());
        }
    }
    out
}

/// Rejet qui ne tient qu'à l'identification (on fournit nous-mêmes la fiche et les épisodes).
pub fn is_identification_rejection(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "unknown movie",
        "unknown series",
        "unable to parse",
        "unable to identify",
        "was matched to",
        "grab history",
        // « Single episode file contains all episodes in seasons » : Sonarr lit « Erased S01 - 06 » comme
        // une saison entière et ne voit pas le numéro. C'est une plainte sur le NOM, pas sur le fichier ;
        // nous fournissons les épisodes. Sans mappage de notre côté, le fichier est écarté juste après.
        "contains all episodes",
    ]
    .iter()
    .any(|k| r.contains(k))
}

/// Épisodes d'une série désignés par le `parse` d'un nom de fichier : saison + numéros, sinon
/// numéros absolus (anime).
pub fn map_episodes(parse: &Value, episodes: &[Value]) -> Vec<i64> {
    let Some(pei) = parse.get("parsedEpisodeInfo") else {
        return vec![];
    };
    let nums = |k: &str| -> Vec<i64> {
        pei.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    let season = pei.get("seasonNumber").and_then(Value::as_i64);
    let (eps, abs) = (nums("episodeNumbers"), nums("absoluteEpisodeNumbers"));
    let field = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64);
    let pick = |f: &dyn Fn(&Value) -> bool| -> Vec<i64> {
        episodes
            .iter()
            .filter(|e| f(e))
            .filter_map(|e| field(e, "id"))
            .collect()
    };
    if let (Some(s), false) = (season, eps.is_empty()) {
        let ids = pick(&|e| {
            field(e, "seasonNumber") == Some(s)
                && field(e, "episodeNumber")
                    .map(|n| eps.contains(&n))
                    .unwrap_or(false)
        });
        if !ids.is_empty() {
            return ids;
        }
    }
    if !abs.is_empty() {
        return pick(&|e| {
            field(e, "absoluteEpisodeNumber")
                .map(|n| abs.contains(&n))
                .unwrap_or(false)
        });
    }
    vec![]
}

/// D'où viennent les épisodes d'un fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeSource {
    /// Défaut : la correspondance de l'Arr quand il a reconnu cette fiche, sinon la nôtre.
    ArrFirst,
    /// Pack d'un cours : **seule** la nôtre compte. L'Arr reconnaît bien la série mais lit la mauvaise
    /// saison dans les noms de fichiers ; le croire écraserait une vraie saison.
    OursOnly,
}

/// Rejet de `manualimport` qui ne parle que de la saison que l'Arr a cru lire. Sur le chemin d'un pack de
/// cours, l'Arr voit « S03E01 » et la saison 3 est pourvue : il répond « not an upgrade ». Notre
/// correspondance explicite prime. Un extrait, une qualité hors profil ou un fichier illisible restent bloquants.
pub fn is_wrong_season_rejection(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "not an upgrade",
        "existing file",
        "already imported",
        "matches existing",
    ]
    .iter()
    .any(|k| r.contains(k))
}

/// Épisode visé par chaque fichier d'un pack décalé. `Err(raison)` au moindre écart : **tout** le pack est
/// refusé, jamais à moitié — un demi-pack laisse un trou et casse la contiguïté sur laquelle tout repose.
#[allow(clippy::too_many_arguments)]
pub fn cour_pack_episodes(
    paths: &[String],
    episodes: &[Value],
    season: i64,
    offset: i64,
    from: i64,
    to: i64,
    has_file: &HashSet<i64>,
) -> Result<HashMap<String, Vec<i64>>, String> {
    let want = (to - from + 1) as usize;
    if paths.len() != want {
        return Err(format!(
            "{} fichier(s) pour {want} épisode(s) visés",
            paths.len()
        ));
    }
    let mut nums: Vec<(i64, &String)> = Vec::with_capacity(paths.len());
    for p in paths {
        let base = p.rsplit('/').next().unwrap_or(p);
        let n = crate::tasks::series_search::claimed_episode(base)
            .ok_or_else(|| format!("fichier sans numéro d'épisode : {base}"))?;
        nums.push((n, p));
    }
    let mut seen: Vec<i64> = nums.iter().map(|(n, _)| *n).collect();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != nums.len() {
        return Err("deux fichiers portent le même numéro".into());
    }
    let (lo, hi) = (seen[0], seen[seen.len() - 1]);
    if hi - lo + 1 != seen.len() as i64 {
        return Err("les numéros des fichiers ne se suivent pas".into());
    }
    if lo + offset != from || hi + offset != to {
        return Err(format!(
            "décalage incohérent : {lo}..{hi} + {offset} ≠ {from}..{to}"
        ));
    }
    let mut out: HashMap<String, Vec<i64>> = HashMap::new();
    for (n, p) in nums {
        let target = n + offset;
        let ids: Vec<i64> = episodes
            .iter()
            .filter(|e| {
                e.get("seasonNumber").and_then(Value::as_i64) == Some(season)
                    && e.get("episodeNumber").and_then(Value::as_i64) == Some(target)
            })
            .filter_map(|e| e.get("id").and_then(Value::as_i64))
            .collect();
        let [id] = ids[..] else {
            return Err(format!("S{season:02}E{target:02} inconnu de la fiche"));
        };
        if has_file.contains(&id) {
            return Err(format!("S{season:02}E{target:02} a déjà un fichier"));
        }
        out.entry(p.clone()).or_default().push(id);
    }
    Ok(out)
}

/// Numéro d'épisode **nu** d'un nom de fichier, avec le titre (normalisé) qui le précède :
/// `Angels of Death - 07 (WEBRip 1920x1080 x264 AAC Rus + Eng)-NOTAG.mkv` → (« angels of death », 7).
///
/// Sonarr ne lit cette forme que pour un **animé** (numérotation absolue) : pour une série ordinaire il ne
/// reconnaît aucun épisode, et le pack restait « téléchargé mais pas rangé » (Angels of Death, 2026-10-03).
/// Forme exigée : un tiret entouré d'espaces, 1 à 3 chiffres, puis un espace, un crochet, une parenthèse ou
/// l'extension — jamais une année, une résolution ni un codec, le chiffre suivant casse la forme. Un nom qui
/// porte sa propre saison (`Erased S01 - 06`) n'est pas un numéro nu : c'est le cas fansub, lu ailleurs.
pub fn bare_episode(path: &str) -> Option<(String, i64)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static SEASON: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^(.+?)\s+-\s+(\d{1,3})(?:[\s\[(]|\.[A-Za-z0-9]{2,4}$)")
            .expect("regex valide")
    });
    let season = SEASON.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:s\d{1,3}|season\s*\d+|saison\s*\d+)\b").expect("regex valide")
    });
    let base = path.rsplit('/').next().unwrap_or(path);
    let c = re.captures(base)?;
    let raw = c.get(1)?.as_str();
    if season.is_match(raw) {
        return None;
    }
    let prefix = normalize(raw);
    if prefix.is_empty() {
        return None;
    }
    Some((prefix, c.get(2)?.as_str().parse().ok()?))
}

/// Numéros nus d'un torrent pris pour **une** saison (`season=` de l'étiquette) → épisodes de cette saison.
/// `found` : (chemin, titre normalisé, numéro). Refus **en bloc** — aucun fichier rangé par ce chemin — si les
/// fichiers ne portent pas tous le même titre (une autre œuvre glissée dans le pack), si deux fichiers ont le même
/// numéro, ou si un numéro dépasse la saison (numérotation absolue d'une saison suivante : on ne devine pas).
pub fn bare_episodes(
    found: &[(String, String, i64)],
    season: i64,
    episodes: &[Value],
) -> Result<HashMap<String, Vec<i64>>, String> {
    let titles: HashSet<&str> = found.iter().map(|(_, t, _)| t.as_str()).collect();
    if titles.len() > 1 {
        return Err(format!(
            "{} titres différents devant les numéros",
            titles.len()
        ));
    }
    let in_season: Vec<(i64, i64)> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(season))
        .filter_map(|e| {
            Some((
                e.get("episodeNumber").and_then(Value::as_i64)?,
                e.get("id").and_then(Value::as_i64)?,
            ))
        })
        .collect();
    let last = in_season.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut seen = HashSet::new();
    let mut out = HashMap::new();
    for (path, _, n) in found {
        if !seen.insert(*n) {
            return Err(format!("deux fichiers portent le numéro {n}"));
        }
        if *n < 1 || *n > last {
            return Err(format!(
                "numéro {n} hors de la saison {season} ({last} épisodes)"
            ));
        }
        let ids: Vec<i64> = in_season
            .iter()
            .filter(|(e, _)| e == n)
            .map(|(_, id)| *id)
            .collect();
        let [id] = ids[..] else {
            return Err(format!("S{season:02}E{n:02} inconnu de la fiche"));
        };
        out.insert(path.clone(), vec![id]);
    }
    Ok(out)
}

/// Numéro d'épisode **en tête** d'un nom de fichier : `05. Deux Magiciens (1re partie).mkv` → 5.
///
/// Packs d'animés publiés sans nom de série ni `SxxEyy`, le texte après le numéro étant le titre de l'ÉPISODE.
/// Sonarr n'y lit rien, pas même en numérotation absolue (2026-10-08 : saison 1 d'un animé de 2006, 24 fichiers
/// « `01. …` » à « `24. …` », restée « téléchargée mais pas rangée »). Forme exigée : 1 à 3 chiffres au tout début
/// du nom, un point, une ou plusieurs espaces, un titre qui contient au moins une lettre, l'extension. Jamais un
/// nom qui porte sa saison (`S01E05`, `S01 - 05`, `1x05`, « Saison 2 »), ni dont le titre contient un autre nombre :
/// un groupe de chiffres qui n'est pas suivi d'une lettre (`Show 13`, `E13`, `#13`, `013 [1080p]`, « Episode 13 »,
/// `Titre - 07`) écarte le nom — Sonarr y lit ce numéro-là (absolu 13 ou S01E13, vérifié par `parse` sur le Sonarr
/// de la seedbox le 2026-10-08), on ne lit pas deux numéros à la fois. Un ordinal collé (`1re partie`, `2e partie`)
/// passe. Ce n'est qu'un filtre de forme : `13th` passe aussi alors que Sonarr y lit l'absolu 13. La vraie garde est
/// ailleurs, `numbered_pack` n'est consulté que si Sonarr n'a rien lu dans aucun nom du torrent.
pub fn numbered_episode(path: &str) -> Option<i64> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static SEASON: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^(\d{1,3})\. +(.*\p{L}.*)\.[A-Za-z0-9]{2,4}$").expect("regex valide")
    });
    let season = SEASON.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?:s\d{1,3}(?:\s*e\d{1,3})?|season\s*\d+|saison\s*\d+|\d{1,2}x\d{1,3})\b",
        )
        .expect("regex valide")
    });
    let number =
        NUMBER.get_or_init(|| regex::Regex::new(r"\d+(?:[^\p{L}\d]|$)").expect("regex valide"));
    let base = path.rsplit('/').next().unwrap_or(path);
    let c = re.captures(base)?;
    let title = c.get(2)?.as_str();
    if season.is_match(title)
        || number.is_match(title)
        || bare_episode(base).is_some()
        || crate::tasks::series_search::fansub_episode(base).is_some()
        || crate::tasks::series_search::claimed_episode(base).is_some()
    {
        return None;
    }
    c.get(1)?.as_str().parse().ok()
}

/// Pack « `NN. Titre de l'épisode.mkv` » d'un torrent pris pour **une** saison (`season=` de l'étiquette posée par
/// `series_search`) → épisodes de cette saison. `paths` : **toutes** les vidéos du torrent (hors extraits, fichiers
/// désélectionnés compris : c'est la forme du pack publié qui est jugée ; l'import, lui, n'en prendra que les
/// fichiers complets). Le titre diffère d'un fichier à l'autre, la règle « mêmes titres » de `bare_episodes` ne
/// s'applique pas : la sûreté vient de la forme du pack entier. Refus **en bloc** (2026-10-08) si une seule vidéo
/// n'a pas cette forme (bonus, NCOP, un `S01E05` glissé), si deux vidéos ont le même numéro, si un numéro vaut 0 ou
/// dépasse la saison, si les numéros ne se suivent pas (1..24 ou 13..24, jamais 1..12 + 14..24), ou si une suite
/// qui ne commence pas à 1 vise une saison précédée d'une autre (13..24 pourrait être une numérotation absolue).
pub fn numbered_episodes(
    paths: &[String],
    season: i64,
    episodes: &[Value],
) -> Result<HashMap<String, Vec<i64>>, String> {
    let mut nums: Vec<(i64, &String)> = Vec::with_capacity(paths.len());
    let mut odd: Vec<&str> = Vec::new();
    for p in paths {
        match numbered_episode(p) {
            Some(n) => nums.push((n, p)),
            None => odd.push(p.rsplit('/').next().unwrap_or(p)),
        }
    }
    if let Some(first) = odd.first() {
        return Err(format!(
            "{} vidéo(s) sur {} sans numéro en tête, dont « {first} »",
            odd.len(),
            paths.len()
        ));
    }
    let mut seen: Vec<i64> = nums.iter().map(|(n, _)| *n).collect();
    seen.sort_unstable();
    let (Some(&lo), Some(&hi)) = (seen.first(), seen.last()) else {
        return Err("aucune vidéo".into());
    };
    if let Some(w) = seen.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("deux fichiers portent le numéro {}", w[0]));
    }
    let in_season: Vec<(i64, i64)> = episodes
        .iter()
        .filter(|e| e.get("seasonNumber").and_then(Value::as_i64) == Some(season))
        .filter_map(|e| {
            Some((
                e.get("episodeNumber").and_then(Value::as_i64)?,
                e.get("id").and_then(Value::as_i64)?,
            ))
        })
        .collect();
    let last = in_season.iter().map(|(n, _)| *n).max().unwrap_or(0);
    if lo < 1 || hi > last {
        return Err(format!(
            "numéros {lo}..{hi} hors de la saison {season} ({last} épisodes)"
        ));
    }
    if hi - lo + 1 != seen.len() as i64 {
        return Err(format!(
            "les numéros {lo}..{hi} ne se suivent pas ({} fichiers)",
            seen.len()
        ));
    }
    let earlier_season = episodes.iter().any(|e| {
        e.get("seasonNumber")
            .and_then(Value::as_i64)
            .is_some_and(|s| (1..season).contains(&s))
    });
    if lo > 1 && earlier_season {
        return Err(format!(
            "numéros {lo}..{hi} en saison {season} : peut-être une numérotation absolue, on ne devine pas"
        ));
    }
    let mut out = HashMap::new();
    for (n, path) in nums {
        let ids: Vec<i64> = in_season
            .iter()
            .filter(|(e, _)| *e == n)
            .map(|(_, id)| *id)
            .collect();
        let [id] = ids[..] else {
            return Err(format!("S{season:02}E{n:02} inconnu de la fiche"));
        };
        out.insert(path.clone(), vec![id]);
    }
    Ok(out)
}

/// Sonarr a-t-il lu quelque chose dans ce torrent ? (2026-10-08) Tant que oui, la lecture « NN. Titre » ne sert pas :
/// la correspondance de l'Arr prime sur l'analyse du nom. Oui dès que :
/// - le `parse` d'un nom (`parses`, un par fichier candidat) donne une information d'épisode (`parsedEpisodeInfo`),
///   même un numéro que la fiche ne connaît pas (`01. Show 13th Night` → absolu 13) ou une saison seule ;
/// - `manualimport` propose un épisode pour l'un des fichiers (`candidates`), quelle que soit la fiche ;
/// - un fichier a déjà ses épisodes (`by_path` : `map_episodes` ou lecture fansub, qui passe `source` à `OursOnly`).
///
/// Les 24 noms du pack réel n'ont aucun `parsedEpisodeInfo` (vérifié par `parse` sur le Sonarr de la seedbox).
pub fn sonarr_read_something(
    candidates: &[Value],
    parses: &[Value],
    by_path: &HashMap<String, Vec<i64>>,
    source: EpisodeSource,
) -> bool {
    parses
        .iter()
        .any(|p| p.get("parsedEpisodeInfo").is_some_and(|i| !i.is_null()))
        || candidates.iter().any(|c| {
            c.get("episodes")
                .and_then(Value::as_array)
                .is_some_and(|e| !e.is_empty())
        })
        || by_path.values().any(|e| !e.is_empty())
        || source != EpisodeSource::ArrFirst
}

/// Lecture « NN. Titre » d'un torrent, décidée pour le torrent entier, **en dernier recours** (2026-10-08) : comme la
/// lecture fansub et le numéro nu, elle ne sert que si Sonarr n'a rien lu (`arr_read`, voir `sonarr_read_something`)
/// — la correspondance de l'Arr prime sur l'analyse du nom. `None` : rien à lire ainsi — Sonarr a lu, l'étiquette ne
/// nomme pas de saison (torrent ajouté à la main, ancienne étiquette `homelab:series=<id>` : la saison ne se devine
/// jamais) ou aucune vidéo n'a cette forme ; les chemins ordinaires suivent sans changement. `Some(Err)` : la forme
/// est là mais la correspondance n'est pas certaine, aucun fichier n'est rangé par ce chemin.
pub fn numbered_pack(
    tag_season: Option<i64>,
    arr_read: bool,
    paths: &[String],
    episodes: &[Value],
) -> Option<Result<HashMap<String, Vec<i64>>, String>> {
    let season = tag_season?;
    if arr_read || !paths.iter().any(|p| numbered_episode(p).is_some()) {
        return None;
    }
    Some(numbered_episodes(paths, season, episodes))
}

/// Fichiers importables dans une fiche : aperçu sans id, fiche fournie par nous ; les rejets
/// d'identification sont ignorés, les autres (sample…) excluent le fichier. `has_file` : épisodes (ou le
/// film) qui ont déjà un fichier — jamais remplacés.
pub fn fresh_files(
    candidates: &[Value],
    movie: bool,
    id: i64,
    download_id: &str,
    episodes_by_path: &HashMap<String, Vec<i64>>,
    has_file: &HashSet<i64>,
    source: EpisodeSource,
) -> (Vec<Value>, Vec<String>) {
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    for c in candidates {
        let path = c.get("path").and_then(Value::as_str).unwrap_or("");
        let rel = c
            .get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_string();
        let blocking: Vec<&str> = c
            .get("rejections")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(|x| x.get("reason").and_then(Value::as_str))
                    .filter(|r| !is_identification_rejection(r))
                    // pack de cours : l'Arr voit la saison qu'il a cru lire, déjà pourvue, et répond
                    // « not an upgrade ». Notre correspondance explicite prime sur ce seul motif.
                    .filter(|r| source != EpisodeSource::OursOnly || !is_wrong_season_rejection(r))
                    .collect()
            })
            .unwrap_or_default();
        if !blocking.is_empty() {
            skipped.push(format!("{rel}: {}", blocking.join(", ")));
            continue;
        }
        let mut f = json!({
            "path": path,
            "quality": c.get("quality"),
            "languages": c.get("languages").cloned().unwrap_or_else(|| json!([])),
            "releaseGroup": c.get("releaseGroup"),
            "indexerFlags": c.get("indexerFlags").and_then(Value::as_i64).unwrap_or(0),
            "downloadId": download_id,
        });
        if movie {
            // jamais de remplacement d'un film déjà présent
            if has_file.contains(&id) {
                skipped.push(format!("{rel}: film déjà présent, pas de remplacement"));
                continue;
            }
            f["movieId"] = json!(id);
        } else {
            // 1. la correspondance de l'Arr quand il a reconnu **cette** fiche : il connaît les saisons et la
            //    numérotation absolue. Notre analyse du nom ne sert que s'il ne l'a pas reconnue : le
            //    2026-09-17, « The.Final.Season.E01 » (sans saison) a été lu S01E01 et la saison 1 écrasée.
            let arr_eps: Vec<i64> = if source == EpisodeSource::OursOnly {
                Vec::new()
            } else if c.pointer("/series/id").and_then(Value::as_i64) == Some(id) {
                c.get("episodes")
                    .and_then(Value::as_array)
                    .map(|e| {
                        e.iter()
                            .filter_map(|x| x.get("id").and_then(Value::as_i64))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let eps = if arr_eps.is_empty() {
                episodes_by_path.get(path).cloned().unwrap_or_default()
            } else {
                arr_eps
            };
            // 2. jamais de remplacement : un épisode qui a déjà un fichier n'est pas touché
            if eps.iter().any(|e| has_file.contains(e)) {
                skipped.push(format!("{rel}: épisode déjà présent, pas de remplacement"));
                continue;
            }
            if eps.is_empty() {
                skipped.push(format!("{rel}: épisode non identifié"));
                continue;
            }
            f["releaseType"] = json!(if eps.len() > 1 {
                "multiEpisode"
            } else {
                "singleEpisode"
            });
            f["seriesId"] = json!(id);
            f["episodeIds"] = json!(eps);
        }
        files.push(f);
    }
    (files, skipped)
}

/// La fiche de l'autre machine a-t-elle déjà des fichiers ?
pub fn has_files(movie: bool, item: &Value) -> bool {
    if movie {
        item.get("hasFile")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    } else {
        item.pointer("/statistics/episodeFileCount")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
    }
}

/// Corps d'ajout d'une fiche, non surveillée et sans recherche.
pub fn add_body(movie: bool, hit: &Value, root: &str, profile: i64) -> Value {
    let mut body = hit.clone();
    let overrides = if movie {
        json!({
            "qualityProfileId": profile, "rootFolderPath": root, "monitored": false,
            "minimumAvailability": "released",
            "addOptions": {"searchForMovie": false, "monitor": "none"}
        })
    } else {
        json!({
            "qualityProfileId": profile, "rootFolderPath": root, "monitored": false,
            "seasonFolder": true,
            "addOptions": {"monitor": "none", "searchForMissingEpisodes": false,
                           "searchForCutoffUnmetEpisodes": false}
        })
    };
    if let (Some(b), Some(o)) = (body.as_object_mut(), overrides.as_object()) {
        b.remove("id");
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
    body
}

/// Après une erreur : nouvel essai tant que `max` n'est pas atteint.
pub fn after_error(prev_attempts: u32, max: u32) -> (&'static str, u32) {
    let attempts = prev_attempts + 1;
    (if attempts >= max { "error" } else { "retry" }, attempts)
}

/// Toutes les vidéos ont déjà un autre lien (importées par hardlink) : rien à faire.
fn all_linked(side: &Side<'_>, t: &Torrent, videos: &[&TorrentFile]) -> bool {
    let Some((container, host)) = &side.local_downloads else {
        return false;
    };
    !videos.is_empty()
        && videos.iter().all(|f| {
            host_path(container, host, &t.save_path, &f.name)
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.nlink() > 1)
                .unwrap_or(false)
        })
}

/// Chemin hôte d'un fichier de torrent : `save_path` vu par qBit → dossier de l'hôte.
fn host_path(
    container_root: &str,
    host_root: &Path,
    save_path: &str,
    name: &str,
) -> Option<std::path::PathBuf> {
    let rel = save_path
        .trim_end_matches('/')
        .strip_prefix(container_root.trim_end_matches('/'))?;
    if !rel.is_empty() && !rel.starts_with('/') {
        return None; // « /downloads2 » n'est pas sous « /downloads »
    }
    Some(host_root.join(rel.trim_start_matches('/')).join(name))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn examine(
    ctx: &TaskContext,
    side: &Side<'_>,
    others: &[&Side<'_>],
    t: &Torrent,
) -> Result<Outcome> {
    let hash = t.hash.to_ascii_uppercase();
    let managed = side.radarr.history_count_for_download(&hash).await?
        + side.sonarr.history_count_for_download(&hash).await?;
    if managed > 0 {
        return Ok(Outcome::cheap("arr_managed", ""));
    }
    let files = side.qbit.files(&t.hash).await?;
    let videos = video_files(&files);
    if videos.is_empty() {
        return Ok(Outcome::cheap("no_video", ""));
    }
    if all_linked(side, t, &videos) {
        return Ok(Outcome::cheap("already_linked", ""));
    }

    // série si le nom ou l'un des fichiers porte un marqueur d'épisode ou de saison
    // étiquette posée par series_search / movie_search : la fiche est connue, aucun nom à analyser
    let target = homelab_target(&t.tags);
    let movie = match &target {
        Some(t) => t.is_movie(),
        None => {
            classify(&t.name) == MediaKind::Movie
                && videos.iter().all(|f| classify(&f.name) == MediaKind::Movie)
        }
    };
    let (arr, kind, ext_param, root) = if movie {
        (side.radarr, "movie", "tmdbId", &side.radarr_root)
    } else {
        (side.sonarr, "series", "tvdbId", &side.sonarr_root)
    };
    if let Some(TagTarget::Broken(tag)) = &target {
        return Ok(Outcome::cheap(
            "error",
            format!("étiquette homelab illisible : {tag}"),
        ));
    }
    let m = if let Some(id) = target.as_ref().and_then(TagTarget::id) {
        let hit = arr
            .get(&format!("api/v3/{kind}/{id}"), &[])
            .await
            .with_context(|| format!("fiche {kind} {id} de l'étiquette introuvable"))?;
        Match { hit, fuzzy: false }
    } else {
        // nom du torrent, puis celui du premier fichier vidéo (« Star Wars Rebels (2014) » ne se
        // parse pas, « Star.Wars.Rebels.S01E01.mkv » si)
        let mut parsed = None;
        for label in parse_labels(&t.name, &videos) {
            let parse = arr.parse(&label).await?;
            parsed = if movie {
                parsed_movie(&parse)
            } else {
                parsed_series(&parse)
            };
            if parsed.is_some() {
                break;
            }
        }
        let Some(parsed) = parsed else {
            return Ok(Outcome::costly(
                "no_match",
                "titre non reconnu par le parse",
            ));
        };
        let term = if movie && parsed.year > 0 {
            format!("{} {}", parsed.title, parsed.year)
        } else {
            parsed.title.clone()
        };
        // D'abord les fiches déjà suivies : elles portent leurs titres alternatifs (une release peut s'appeler
        // « Shingeki no Kyojin » alors que la fiche s'appelle « Attack on Titan »), que la recherche TVDB, elle,
        // ne renvoie pas. Sinon seulement, on interroge le catalogue.
        let existing = if movie {
            arr.movies().await.unwrap_or_default()
        } else {
            arr.series().await.unwrap_or_default()
        };
        let picked = if movie {
            pick_movie(&existing, &parsed)
        } else {
            pick_series(&existing, &parsed)
        };
        let (hits, picked) = match picked {
            Some(m) => (existing, Some(m)),
            None => {
                let hits = arr.lookup(kind, &term).await?;
                let picked = if movie {
                    pick_movie(&hits, &parsed)
                } else {
                    pick_series(&hits, &parsed)
                };
                (hits, picked)
            }
        };
        let _ = &hits;
        let Some(m) = picked else {
            return Ok(Outcome::costly(
                "no_match",
                format!("aucune fiche pour « {term} »"),
            ));
        };
        m
    };
    let ext_id = m.hit.get(ext_param).and_then(Value::as_i64).unwrap_or(0);
    let title = format!(
        "{} ({})",
        m.hit.get("title").and_then(Value::as_str).unwrap_or("?"),
        m.hit.get("year").and_then(Value::as_i64).unwrap_or(0)
    );
    if m.fuzzy {
        info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, "approximate match (first result, same first word)");
    }

    for other in others {
        let other_arr: &ArrClient = if movie { other.radarr } else { other.sonarr };
        if let Some(item) = other_arr.find_by(kind, ext_param, ext_id).await? {
            if has_files(movie, &item) {
                return Ok(Outcome::costly(
                    "dup_other_side",
                    format!("{title} a déjà des fichiers sur {}", other.name),
                ));
            }
        }
    }

    let existing = arr.find_by(kind, ext_param, ext_id).await?;
    if ctx.dry_run {
        let what = if existing.is_some() {
            "import"
        } else {
            "add unmonitored + import"
        };
        info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, fuzzy = m.fuzzy, "dry-run: would {what}");
        return Ok(Outcome::costly("dry_run", format!("{what} → {title}")));
    }
    let existing_has_file = existing
        .as_ref()
        .map(|i| has_files(movie, i))
        .unwrap_or(false);
    let id = match existing {
        Some(item) => item
            .get("id")
            .and_then(Value::as_i64)
            .context("fiche sans id")?,
        None => {
            let added = arr
                .add(
                    kind,
                    &add_body(movie, &m.hit, root, side.quality_profile_id),
                )
                .await?;
            let id = added
                .get("id")
                .and_then(Value::as_i64)
                .context("ajout sans id")?;
            info!(task = "torrent_import", side = side.name, %title, id, "added unmonitored");
            if !movie
                && !wait_episodes(arr, id, ctx.cfg.tasks.torrent_import.series_ready_secs).await?
            {
                return Ok(Outcome::costly(
                    "retry",
                    format!("{title} : épisodes pas encore chargés"),
                ));
            }
            id
        }
    };

    // Dossier du torrent listé **sans** l'id de la fiche : avec l'id, Sonarr liste les fichiers déjà rangés de
    // la série et aucun du torrent (le 2026-09-17, 59 fichiers de la fiche et 0 des 28 épisodes de la saison 4),
    // et pour une fiche vide il répond 500. Les épisodes sont ensuite rattachés à **cette** fiche par le nom de
    // chaque fichier : l'identification de la série par Sonarr (titre japonais, anglais, français) ne compte pas.
    let incomplete = incomplete_paths(&files, &t.save_path);
    let candidates: Vec<Value> =
        from_torrent(&arr.manual_import(&t.content_path).await?, &t.content_path)
            .into_iter()
            .filter(|c| {
                let keep = c
                    .get("path")
                    .and_then(Value::as_str)
                    .is_none_or(|p| !incomplete.contains(p));
                if !keep {
                    info!(task = "torrent_import", side = side.name, torrent = %t.name, file = ?c.get("path"), "incomplete file skipped");
                }
                keep
            })
            .collect();
    let mut by_path: HashMap<String, Vec<i64>> = HashMap::new();
    let mut has_file: HashSet<i64> = HashSet::new();
    let mut source = EpisodeSource::ArrFirst;
    if movie {
        if existing_has_file {
            has_file.insert(id);
        }
    } else {
        let episodes = arr.episodes(id).await?;
        has_file.extend(
            episodes
                .iter()
                .filter(|e| e.get("hasFile").and_then(Value::as_bool) == Some(true))
                .filter_map(|e| e.get("id").and_then(Value::as_i64)),
        );
        // pack d'un cours : la correspondance a été décidée à la prise et voyage dans l'étiquette.
        // On ne demande son avis ni à l'Arr ni à notre analyse de nom : tous deux liraient la saison
        // annoncée par les fichiers (« S03E01 » = saison 3 de Bleach).
        if let Some(TagTarget::CourPack {
            season,
            offset,
            from,
            to,
            ..
        }) = target
        {
            let paths: Vec<String> = candidates
                .iter()
                .filter_map(|c| c.get("path").and_then(Value::as_str).map(str::to_string))
                .collect();
            match cour_pack_episodes(&paths, &episodes, season, offset, from, to, &has_file) {
                Ok(m) => {
                    info!(task = "torrent_import", side = side.name, torrent = %t.name,
                          season, offset, from, to, files = m.len(),
                          "pack d'un cours : correspondance explicite appliquée");
                    by_path = m;
                    source = EpisodeSource::OursOnly;
                }
                Err(why) => {
                    warn!(task = "torrent_import", side = side.name, torrent = %t.name, %why,
                          "pack d'un cours refusé : rien n'est importé");
                    return Ok(Outcome::costly("nothing_importable", why));
                }
            }
        } else {
            // torrent pris pour une saison précise : un numéro en tête (« 05. Titre ») ou « nu » (« - 07 ») pourra y
            // être lu, voir plus bas
            let tag_season = match &target {
                Some(TagTarget::Series { season, .. }) => *season,
                _ => None,
            };
            let mut bare: Vec<(String, String, i64)> = Vec::new();
            // ce que Sonarr a lu dans chaque nom : tant qu'il a lu quelque chose, pas de lecture « NN. Titre »
            let mut parses: Vec<Value> = Vec::with_capacity(candidates.len());
            for c in &candidates {
                let Some(path) = c.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let base = path.rsplit('/').next().unwrap_or(path);
                let parse = arr.parse(base).await?;
                let mut eps = map_episodes(&parse, &episodes);
                // Sonarr n'a rien su lire : dernier recours, la numérotation des fansubs
                // (« Erased S01 - 06 ») dans la saison qu'il a reconnue.
                if eps.is_empty() {
                    if let (Some(season), Some(n)) = (
                        parse
                            .pointer("/parsedEpisodeInfo/seasonNumber")
                            .and_then(Value::as_i64),
                        crate::tasks::series_search::fansub_episode(base),
                    ) {
                        eps = episodes
                            .iter()
                            .filter(|e| {
                                e.get("seasonNumber").and_then(Value::as_i64) == Some(season)
                                    && e.get("episodeNumber").and_then(Value::as_i64) == Some(n)
                            })
                            .filter_map(|e| e.get("id").and_then(Value::as_i64))
                            .collect();
                        if !eps.is_empty() {
                            info!(
                                task = "torrent_import",
                                side = side.name,
                                file = base,
                                season,
                                episode = n,
                                "numérotation fansub lue"
                            );
                            // Sonarr, lui, croit que CHAQUE fichier contient toute la saison et
                            // proposerait les 12 épisodes pour le premier : notre lecture doit primer
                            // sur la sienne pour tout ce torrent.
                            source = EpisodeSource::OursOnly;
                        }
                    }
                }
                // toujours rien : numéro nu, jamais pour un fichier que Sonarr attribue à une AUTRE fiche
                if eps.is_empty() && tag_season.is_some() {
                    let other = parse
                        .pointer("/series/id")
                        .and_then(Value::as_i64)
                        .is_some_and(|s| s != id);
                    if let (false, Some((title, n))) = (other, bare_episode(base)) {
                        bare.push((path.to_string(), title, n));
                    }
                }
                by_path.insert(path.to_string(), eps);
                parses.push(parse);
            }
            // Pack « 05. Titre de l'épisode.mkv » (2026-10-08), en DERNIER recours : seulement si Sonarr n'a rien lu
            // dans aucun nom (ni `parse`, ni épisode proposé par `manualimport`, ni lecture fansub). Sinon l'ancien
            // chemin, inchangé : « 01. Show 13.mkv » … est rangé par Sonarr en E13…, jamais par nous en E01…. Jugé
            // sur TOUTES les vidéos du torrent, chemins vus par l'Arr (`incomplete_paths` les construit de même).
            // Retenu, il fournit les épisodes de chaque fichier ; la source reste `ArrFirst` : Sonarr n'ayant rien
            // proposé, il n'y a rien à faire taire, et ses autres refus (extrait, qualité, déjà importé) restent
            // bloquants. Écarté, les numéros nus suivent comme avant.
            let arr_read = sonarr_read_something(&candidates, &parses, &by_path, source);
            let root = t.save_path.trim_end_matches('/');
            let video_paths: Vec<String> = videos
                .iter()
                .map(|f| format!("{root}/{}", f.name))
                .collect();
            let numbered = match numbered_pack(tag_season, arr_read, &video_paths, &episodes) {
                Some(Ok(m)) => {
                    info!(task = "torrent_import", side = side.name, torrent = %t.name, season = tag_season.unwrap_or_default(),
                          files = m.len(), "numéros « NN. Titre » lus dans la saison de l'étiquette");
                    Some(m)
                }
                Some(Err(why)) => {
                    warn!(task = "torrent_import", side = side.name, torrent = %t.name, season = tag_season.unwrap_or_default(), %why,
                          "numéros « NN. Titre » écartés : correspondance incertaine");
                    None
                }
                None => None,
            };
            if let Some(m) = numbered {
                by_path = m;
            } else if let (Some(season), false) = (tag_season, bare.is_empty()) {
                match bare_episodes(&bare, season, &episodes) {
                    Ok(m) => {
                        info!(task = "torrent_import", side = side.name, torrent = %t.name, season, files = m.len(),
                              "numéros « - NN » lus dans la saison de l'étiquette");
                        by_path.extend(m);
                    }
                    Err(why) => {
                        warn!(task = "torrent_import", side = side.name, torrent = %t.name, season, %why,
                              "numéros « - NN » écartés : correspondance incertaine");
                    }
                }
            }
        }
    }
    let (files, skipped) = fresh_files(&candidates, movie, id, &hash, &by_path, &has_file, source);
    for s in &skipped {
        info!(task = "torrent_import", side = side.name, torrent = %t.name, file = %s, "file skipped");
    }
    if files.is_empty() {
        let why = skipped
            .first()
            .cloned()
            .unwrap_or_else(|| "aucun fichier proposé".into());
        return Ok(Outcome::costly(
            "nothing_importable",
            format!("{title} : {}", truncate(&why, 160)),
        ));
    }
    let n = files.len() as u32;
    let cmd = arr
        .command(json!({ "name": "ManualImport", "files": files, "importMode": "copy" }))
        .await?;
    let cmd_id = cmd.get("id").and_then(Value::as_i64).unwrap_or(0);
    info!(task = "torrent_import", side = side.name, torrent = %t.name, %title, files = n, cmd_id, "manual import (hardlink) triggered");
    Ok(Outcome {
        outcome: "imported",
        detail: format!("{title} : {n} fichier(s)"),
        files: n,
        costly: true,
    })
}

/// Avec l'id de la fiche, `manualimport` renvoie aussi les fichiers déjà rangés dans le dossier de la
/// série (le 2026-09-16, les 25 épisodes de la saison 1 au lieu des 12 de la saison 2 qu'on venait de
/// télécharger, et l'import ne faisait rien). On ne garde que ce qui vient du torrent.
pub fn from_torrent(candidates: &[Value], content_path: &str) -> Vec<Value> {
    candidates
        .iter()
        .filter(|c| {
            c.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| p.starts_with(content_path))
        })
        .cloned()
        .collect()
}

async fn wait_episodes(arr: &ArrClient, series_id: i64, max_secs: u64) -> Result<bool> {
    let mut waited = 0;
    loop {
        if arr.episode_count(series_id).await? > 0 {
            return Ok(true);
        }
        if waited >= max_secs {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += 5;
    }
}

async fn process_side(
    ctx: &TaskContext,
    side: &Side<'_>,
    others: &[&Side<'_>],
    budget: &mut usize,
    counts: &mut BTreeMap<&'static str, u32>,
) -> Result<u32> {
    let max_attempts = ctx.cfg.tasks.torrent_import.max_attempts;
    let torrents = side.qbit.torrents().await?;
    let records = ctx.state.read(|s| s.torrent_import.clone()).await;

    if !ctx.dry_run {
        let prefix = format!("{}:", side.name);
        let live: std::collections::HashSet<String> = torrents
            .iter()
            .map(|t| state_key(side.name, &t.hash))
            .collect();
        ctx.state
            .update(|s| {
                s.torrent_import
                    .retain(|k, _| !k.starts_with(&prefix) || live.contains(k))
            })
            .await?;
    }

    let mut cands: Vec<&Torrent> = torrents
        .iter()
        .filter(|t| is_candidate(t, records.get(&state_key(side.name, &t.hash))))
        .collect();
    cands.sort_by_key(|t| Reverse(t.completion_on));

    let mut imported = 0u32;
    for t in cands {
        if *budget == 0 {
            *counts.entry("pending").or_default() += 1;
            continue;
        }
        let key = state_key(side.name, &t.hash);
        let prev = records.get(&key).map(|r| r.attempts).unwrap_or(0);
        let (outcome, detail, attempts, costly) = match examine(ctx, side, others, t).await {
            Ok(o) => {
                imported += o.files;
                let attempts = if o.outcome == "retry" { prev + 1 } else { prev };
                (o.outcome, o.detail, attempts, o.costly)
            }
            Err(e) => {
                let (outcome, attempts) = after_error(prev, max_attempts);
                warn!(task = "torrent_import", side = side.name, torrent = %t.name, error = format!("{e:#}"), attempts, "examine failed");
                (outcome, truncate(&format!("{e:#}"), 200), attempts, true)
            }
        };
        // un « retry » qui a épuisé ses essais devient définitif
        let outcome = if outcome == "retry" && attempts >= max_attempts {
            "error"
        } else {
            outcome
        };
        if costly {
            *budget -= 1;
        }
        *counts.entry(outcome).or_default() += 1;
        if !matches!(outcome, "arr_managed" | "already_linked" | "no_video") {
            info!(task = "torrent_import", side = side.name, torrent = %t.name, outcome, %detail, "decision");
        }
        if ctx.dry_run || outcome == "dry_run" {
            continue;
        }
        let rec = TorrentImportRecord {
            at: now(),
            name: t.name.clone(),
            outcome: outcome.to_string(),
            detail,
            attempts,
        };
        ctx.state
            .update(|s| s.torrent_import.insert(key, rec))
            .await?;
    }
    Ok(imported)
}

#[async_trait]
impl Task for TorrentImport {
    fn name(&self) -> &'static str {
        "torrent_import"
    }

    fn label(&self) -> &'static str {
        "Torrents ajoutés à la main"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.torrent_import.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let mut budget = ctx.cfg.tasks.torrent_import.max_per_run;
        let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        let mut files = 0u32;
        let sides = ctx.sides();
        for (i, side) in sides.iter().enumerate() {
            let others: Vec<&Side<'_>> = sides
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, s)| s)
                .collect();
            match process_side(ctx, side, &others, &mut budget, &mut counts).await {
                Ok(n) => files += n,
                Err(e) => {
                    *counts.entry("side_error").or_default() += 1;
                    warn!(
                        task = "torrent_import",
                        side = side.name,
                        error = format!("{e:#}"),
                        "side skipped"
                    )
                }
            }
        }
        let summary = if counts.is_empty() {
            "nothing new".to_string()
        } else {
            let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("files={files} {}", parts.join(" "))
        };
        Ok(Report::new(summary, files))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(progress: f64, content: &str) -> Torrent {
        serde_json::from_value(
            json!({"hash": "ABC", "name": "x", "progress": progress, "content_path": content,
                                      "save_path": "/downloads"}),
        )
        .unwrap()
    }

    fn rec(outcome: &str) -> TorrentImportRecord {
        TorrentImportRecord {
            at: 0,
            name: "x".into(),
            outcome: outcome.into(),
            detail: String::new(),
            attempts: 1,
        }
    }

    #[test]
    fn deselected_or_partial_files_are_never_imported() {
        // Кухня, 2026-09-27 : 117 fichiers désélectionnés APRÈS le début du téléchargement, torrent « terminé »
        let files: Vec<TorrentFile> = serde_json::from_value(json!([
            {"name": "Kukhnya/S2E02.mkv", "size": 10, "progress": 1.0, "priority": 1},
            {"name": "Kukhnya/S6E12.mkv", "size": 10, "progress": 0.14, "priority": 0},
            {"name": "Kukhnya/S5E01.mkv", "size": 10, "progress": 1.0, "priority": 0},
            {"name": "Kukhnya/S1E01.mkv", "size": 10}
        ]))
        .unwrap();
        let bad = incomplete_paths(&files, "/home/x/downloads/");
        assert!(bad.contains("/home/x/downloads/Kukhnya/S6E12.mkv"));
        assert!(bad.contains("/home/x/downloads/Kukhnya/S5E01.mkv"));
        assert!(!bad.contains("/home/x/downloads/Kukhnya/S2E02.mkv"));
        // sans champ progress (ancienne API) : considéré complet
        assert!(!bad.contains("/home/x/downloads/Kukhnya/S1E01.mkv"));
    }

    #[test]
    fn candidates_are_complete_and_not_yet_final() {
        assert!(is_candidate(&torrent(1.0, "/downloads/x"), None));
        assert!(!is_candidate(&torrent(0.99, "/downloads/x"), None));
        assert!(!is_candidate(&torrent(1.0, ""), None));
        assert!(!is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("imported"))
        ));
        assert!(!is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("no_match"))
        ));
        assert!(is_candidate(
            &torrent(1.0, "/downloads/x"),
            Some(&rec("retry"))
        ));
    }

    #[test]
    fn keeps_videos_and_drops_samples() {
        let files: Vec<TorrentFile> = serde_json::from_value(json!([
            {"name": "Show.S01/Show.S01E01.mkv", "size": 1},
            {"name": "Show.S01/Sample/show-sample.mkv", "size": 1},
            {"name": "Show.S01/show.sample.mkv", "size": 1},
            {"name": "Show.S01/Show.nfo", "size": 1},
            {"name": "Movie.2003.mp4", "size": 1},
            {"name": "Samples.Of.Life.2020.mkv", "size": 1}
        ]))
        .unwrap();
        let names: Vec<&str> = video_files(&files)
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Show.S01/Show.S01E01.mkv",
                "Movie.2003.mp4",
                "Samples.Of.Life.2020.mkv"
            ]
        );
    }

    #[test]
    fn parse_falls_back_to_the_first_video_name() {
        let files: Vec<TorrentFile> = serde_json::from_value(json!([
            {"name": "Star Wars Rebels (2014)/Star.Wars.Rebels.S01E02.mkv"},
            {"name": "Star Wars Rebels (2014)/Star.Wars.Rebels.S01E01.mkv"}
        ]))
        .unwrap();
        let videos: Vec<&TorrentFile> = files.iter().collect();
        assert_eq!(
            parse_labels("Star Wars Rebels (2014)", &videos),
            vec!["Star Wars Rebels (2014)", "Star.Wars.Rebels.S01E01.mkv"]
        );
        let single: Vec<TorrentFile> =
            serde_json::from_value(json!([{"name": "Fusion (2003).mkv"}])).unwrap();
        let v: Vec<&TorrentFile> = single.iter().collect();
        assert_eq!(
            parse_labels("Fusion (2003).mkv", &v),
            vec!["Fusion (2003).mkv"]
        );
    }

    #[test]
    fn maps_parsed_episodes_to_ids() {
        let eps = vec![
            json!({"id": 10, "seasonNumber": 1, "episodeNumber": 7, "absoluteEpisodeNumber": 7}),
            json!({"id": 11, "seasonNumber": 1, "episodeNumber": 8, "absoluteEpisodeNumber": 8}),
            json!({"id": 20, "seasonNumber": 2, "episodeNumber": 1, "absoluteEpisodeNumber": 13}),
        ];
        let p = json!({"parsedEpisodeInfo": {"seasonNumber": 1, "episodeNumbers": [7, 8], "absoluteEpisodeNumbers": []}});
        assert_eq!(map_episodes(&p, &eps), vec![10, 11]);
        let anime = json!({"parsedEpisodeInfo": {"seasonNumber": 0, "episodeNumbers": [], "absoluteEpisodeNumbers": [13]}});
        assert_eq!(map_episodes(&anime, &eps), vec![20]);
        assert!(map_episodes(&json!({}), &eps).is_empty());
        let wrong = json!({"parsedEpisodeInfo": {"seasonNumber": 5, "episodeNumbers": [1]}});
        assert!(map_episodes(&wrong, &eps).is_empty());
    }

    #[test]
    fn fresh_items_ignore_identification_rejections_only() {
        let cands = vec![
            json!({"path": "/d/a.mkv", "rejections": [{"reason": "Unknown Movie"}], "quality": {}}),
            json!({"path": "/d/s.mkv", "rejections": [{"reason": "Sample"}, {"reason": "Unknown Movie"}]}),
        ];
        let (files, skipped) = fresh_files(
            &cands,
            true,
            68,
            "H",
            &HashMap::new(),
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["movieId"], 68);
        assert_eq!(skipped, vec!["/d/s.mkv: Sample".to_string()]);

        let series = vec![
            json!({"path": "/d/e1.mkv", "rejections": [{"reason": "Unknown Series"}]}),
            json!({"path": "/d/e2.mkv", "rejections": []}),
        ];
        let mut by = HashMap::new();
        by.insert("/d/e1.mkv".to_string(), vec![10, 11]);
        let (files, skipped) = fresh_files(
            &series,
            false,
            5,
            "H",
            &by,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["episodeIds"], json!([10, 11]));
        assert_eq!(files[0]["releaseType"], "multiEpisode");
        assert_eq!(skipped.len(), 1);
        assert!(is_identification_rejection(
            "Found matching series via grab history, but release was matched to series by ID"
        ));
        assert!(!is_identification_rejection(
            "Not an upgrade for existing episode file(s)"
        ));
    }

    #[test]
    fn other_side_duplicate_needs_files() {
        assert!(has_files(true, &json!({"hasFile": true})));
        assert!(!has_files(true, &json!({"hasFile": false})));
        assert!(has_files(
            false,
            &json!({"statistics": {"episodeFileCount": 3}})
        ));
        assert!(!has_files(
            false,
            &json!({"statistics": {"episodeFileCount": 0}})
        ));
    }

    #[test]
    fn added_items_are_unmonitored_without_search() {
        let hit = json!({"id": 0, "title": "The Core", "tmdbId": 9341, "monitored": true});
        let b = add_body(true, &hit, "/movies", 6);
        assert_eq!(b["monitored"], false);
        assert_eq!(b["addOptions"]["searchForMovie"], false);
        assert_eq!(b["rootFolderPath"], "/movies");
        assert_eq!(b["qualityProfileId"], 6);
        assert_eq!(b["tmdbId"], 9341);
        assert!(b.get("id").is_none());
        let s = add_body(
            false,
            &json!({"title": "Daybreak (2019)", "tvdbId": 1}),
            "/tv",
            7,
        );
        assert_eq!(s["monitored"], false);
        assert_eq!(s["addOptions"]["monitor"], "none");
        assert_eq!(s["addOptions"]["searchForMissingEpisodes"], false);
    }

    #[test]
    fn errors_retry_then_give_up() {
        assert_eq!(after_error(0, 3), ("retry", 1));
        assert_eq!(after_error(1, 3), ("retry", 2));
        assert_eq!(after_error(2, 3), ("error", 3));
    }

    #[test]
    fn state_keys_are_side_scoped_and_lowercase() {
        assert_eq!(state_key("seedbox", "ABCdef"), "seedbox:abcdef");
    }

    #[test]
    fn maps_torrent_files_to_host_paths() {
        let p = host_path(
            "/downloads",
            Path::new("/opt/homelab/library/downloads"),
            "/downloads/",
            "Show.S01/E01.mkv",
        );
        assert_eq!(
            p.unwrap(),
            Path::new("/opt/homelab/library/downloads/Show.S01/E01.mkv")
        );
        assert!(host_path("/downloads", Path::new("/x"), "/elsewhere", "a.mkv").is_none());
        assert!(host_path("/downloads", Path::new("/x"), "/downloads2", "a.mkv").is_none());
    }

    #[test]
    fn only_files_from_the_torrent_are_imported() {
        let cands = vec![
            json!({"path": "/downloads/Serie.S02/E01.mkv"}),
            json!({"path": "/media/TV Shows/Serie/Saison 1/E01.mkv"}),
        ];
        let kept = from_torrent(&cands, "/downloads/Serie.S02");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["path"], "/downloads/Serie.S02/E01.mkv");
    }

    #[test]
    fn homelab_tags_name_the_target_fiche() {
        // non-régression : les étiquettes d'avant le décalage se lisent comme avant
        assert_eq!(
            homelab_target("homelab:series=50:season=4"),
            Some(TagTarget::Series {
                id: 50,
                season: Some(4)
            })
        );
        assert_eq!(
            homelab_target("autre, homelab:movie=66"),
            Some(TagTarget::Movie { id: 66 })
        );
        assert_eq!(homelab_target("C411,radarr"), None);
        assert_eq!(homelab_target("homelab:series=abc"), None);
        assert_eq!(homelab_target(""), None);
    }

    #[test]
    fn an_offset_tag_carries_the_whole_mapping() {
        assert_eq!(
            homelab_target("homelab:series=60:season=17:offset=26:eps=27-40"),
            Some(TagTarget::CourPack {
                id: 60,
                season: 17,
                offset: 26,
                from: 27,
                to: 40
            })
        );
        // une étiquette à décalage incomplète ou absurde n'est JAMAIS traitée comme ordinaire :
        // retomber sur l'analyse de nom importerait le pack dans la saison que Sonarr croit lire.
        for bad in [
            "homelab:series=60:season=17:offset=abc:eps=27-40",
            "homelab:series=60:season=17:offset=26",
            "homelab:series=60:offset=26:eps=27-40",
            "homelab:series=60:season=17:offset=26:eps=40-27",
            "homelab:movie=60:season=17:offset=26:eps=27-40",
        ] {
            assert!(
                matches!(homelab_target(bad), Some(TagTarget::Broken(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_cour_pack_maps_each_file_to_its_episode() {
        // 14 fichiers S03E01..14 → S17E27..40 de la fiche
        let episodes: Vec<Value> = (1..=50)
            .map(|n| json!({"id": 9000 + n, "seasonNumber": 17, "episodeNumber": n}))
            .collect();
        let paths: Vec<String> = (1..=14)
            .map(|n| format!("/dl/P/BLEACH.TYBW.S03E{n:02}.mkv"))
            .collect();
        let m = cour_pack_episodes(&paths, &episodes, 17, 26, 27, 40, &HashSet::new()).unwrap();
        assert_eq!(m.len(), 14);
        assert_eq!(m["/dl/P/BLEACH.TYBW.S03E01.mkv"], vec![9027]);
        assert_eq!(m["/dl/P/BLEACH.TYBW.S03E14.mkv"], vec![9040]);
    }

    #[test]
    fn a_cour_pack_is_refused_whole_never_by_half() {
        let episodes: Vec<Value> = (1..=50)
            .map(|n| json!({"id": 9000 + n, "seasonNumber": 17, "episodeNumber": n}))
            .collect();
        let ok: Vec<String> = (1..=14).map(|n| format!("/dl/P/S03E{n:02}.mkv")).collect();
        let e = |r: Result<HashMap<String, Vec<i64>>, String>| r.unwrap_err();

        // un épisode visé a déjà un fichier : TOUT le pack est refusé
        let have: HashSet<i64> = [9033].into_iter().collect();
        assert!(e(cour_pack_episodes(&ok, &episodes, 17, 26, 27, 40, &have)).contains("déjà"));
        // il manque un fichier
        assert!(e(cour_pack_episodes(
            &ok[..13],
            &episodes,
            17,
            26,
            27,
            40,
            &HashSet::new()
        ))
        .contains("13 fichier"));
        // un fichier sans numéro
        let mut odd = ok[..13].to_vec();
        odd.push("/dl/P/bonus.mkv".into());
        assert!(e(cour_pack_episodes(
            &odd,
            &episodes,
            17,
            26,
            27,
            40,
            &HashSet::new()
        ))
        .contains("sans numéro"));
        // décalage incohérent avec la plage annoncée
        assert!(e(cour_pack_episodes(
            &ok,
            &episodes,
            17,
            10,
            27,
            40,
            &HashSet::new()
        ))
        .contains("décalage incohérent"));
        // épisode absent de la fiche
        let short: Vec<Value> = episodes.iter().take(30).cloned().collect();
        assert!(e(cour_pack_episodes(
            &ok,
            &short,
            17,
            26,
            27,
            40,
            &HashSet::new()
        ))
        .contains("inconnu de la fiche"));
    }

    #[test]
    fn our_mapping_wins_over_the_arr_for_a_cour_pack() {
        // L'Arr reconnaît bien la fiche (series.id == 60) mais pointe la saison 3, et rejette le
        // fichier en « Not an upgrade ». Sur ce chemin, seule notre correspondance compte.
        let cands = vec![json!({
            "path": "/dl/P/S03E01.mkv", "relativePath": "S03E01.mkv",
            "rejections": [{"reason": "Not an upgrade for existing episode file(s)"}],
            "series": {"id": 60}, "episodes": [{"id": 3001}]
        })];
        let by: HashMap<String, Vec<i64>> = [("/dl/P/S03E01.mkv".to_string(), vec![9027])]
            .into_iter()
            .collect();

        let (ours, _) = fresh_files(
            &cands,
            false,
            60,
            "H",
            &by,
            &HashSet::new(),
            EpisodeSource::OursOnly,
        );
        assert_eq!(ours.len(), 1, "le rejet de saison ne bloque pas ce chemin");
        assert_eq!(
            ours[0]["episodeIds"],
            json!([9027]),
            "notre épisode, pas 3001"
        );

        // chemin normal inchangé : le rejet bloque, et l'Arr fait foi
        let (normal, skipped) = fresh_files(
            &cands,
            false,
            60,
            "H",
            &by,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert!(normal.is_empty() && !skipped.is_empty());
    }

    #[test]
    fn falls_back_to_the_arr_episodes_only_for_the_same_series() {
        let row = |series: i64| {
            json!({"path": "/dl/T/E27.mkv", "relativePath": "E27.mkv", "rejections": [],
                   "series": {"id": series}, "episodes": [{"id": 9027}]})
        };
        let empty: HashMap<String, Vec<i64>> = HashMap::new();
        let (files, skipped) = fresh_files(
            &[row(50)],
            false,
            50,
            "H",
            &empty,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["episodeIds"], json!([9027]));
        assert!(skipped.is_empty());
        // l'Arr a reconnu une autre série : on n'importe pas ses épisodes dans notre fiche
        let (files, skipped) = fresh_files(
            &[row(77)],
            false,
            50,
            "H",
            &empty,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty() && skipped.len() == 1);
    }

    #[test]
    fn never_overwrites_and_prefers_the_arr_mapping() {
        // l'Arr a reconnu la série et rattache E01 à la saison 4 ; notre analyse du nom disait S01E01
        let row = json!({"path": "/dl/Final.Season/E01.mkv", "rejections": [],
                         "series": {"id": 50}, "episodes": [{"id": 4001}]});
        let mut by = HashMap::new();
        by.insert("/dl/Final.Season/E01.mkv".to_string(), vec![1001]);
        let s1_present: HashSet<i64> = [1001].into_iter().collect();
        let (files, _) = fresh_files(
            std::slice::from_ref(&row),
            false,
            50,
            "H",
            &by,
            &s1_present,
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files[0]["episodeIds"], json!([4001]));
        // l'Arr ne l'a pas reconnue et notre analyse vise un épisode qui a déjà un fichier : rien
        let unknown = json!({"path": "/dl/Final.Season/E01.mkv", "rejections": []});
        let (files, skipped) = fresh_files(
            &[unknown],
            false,
            50,
            "H",
            &by,
            &s1_present,
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty());
        assert!(skipped[0].contains("déjà présent"));
        // film déjà présent : pas de remplacement
        let m = json!({"path": "/dl/film.mkv", "rejections": []});
        let present: HashSet<i64> = [68].into_iter().collect();
        let (files, _) = fresh_files(
            &[m],
            true,
            68,
            "H",
            &HashMap::new(),
            &present,
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty());
    }
    /// Les 10 épisodes de la saison 1 d'Angels of Death (2021) dans Sonarr (ids 1001…1010), plus des spéciaux.
    fn angels_of_death() -> Vec<Value> {
        let mut v: Vec<Value> = (1..=10)
            .map(|n| json!({"id": 1000 + n, "seasonNumber": 1, "episodeNumber": n}))
            .collect();
        v.extend((1..=4).map(|n| json!({"id": 2000 + n, "seasonNumber": 0, "episodeNumber": n})));
        v
    }

    #[test]
    fn a_bare_episode_number_is_read_with_its_title() {
        assert_eq!(
            bare_episode("/dl/Angels of Death S01 Complet/Angels of Death - 07 (WEBRip 1920x1080 x264 AAC Rus + Eng)-NOTAG.mkv"),
            Some(("angelsofdeath".to_string(), 7))
        );
        assert_eq!(
            bare_episode("Angels of Death - 10 [Finale](WEBRip 1920x1080 x264 AAC Eng)-NOTAG.mkv"),
            Some(("angelsofdeath".to_string(), 10))
        );
        assert_eq!(bare_episode("Show - 3.mkv"), Some(("show".to_string(), 3)));
        // jamais une année, une résolution, une version « v2 » collée, ni un nom sans tiret
        assert_eq!(bare_episode("Show - 2021.mkv"), None);
        assert_eq!(bare_episode("Show - 1080p.mkv"), None);
        assert_eq!(bare_episode("Show - 07v2.mkv"), None);
        assert_eq!(bare_episode("Show.07.mkv"), None);
        assert_eq!(bare_episode("Show.S01E07.mkv"), None);
        // un nom qui porte sa saison relève de la lecture fansub, pas de celle-ci
        assert_eq!(bare_episode("Erased S01 - 06 VOSTFR [1080p].mkv"), None);
        assert_eq!(bare_episode("Show Saison 2 - 04.mkv"), None);
    }

    #[test]
    fn bare_numbers_land_in_the_tagged_season() {
        let eps = angels_of_death();
        let found: Vec<(String, String, i64)> = (1..=10)
            .map(|n| {
                (
                    format!("/dl/AoD/Angels of Death - {n:02}.mkv"),
                    "angels of death".to_string(),
                    n,
                )
            })
            .collect();
        let m = bare_episodes(&found, 1, &eps).unwrap();
        assert_eq!(m.len(), 10);
        assert_eq!(m["/dl/AoD/Angels of Death - 07.mkv"], vec![1007]);
        // jamais dans les spéciaux, même si l'étiquette visait une autre saison
        assert!(m.values().all(|ids| ids.iter().all(|id| *id < 2000)));
    }

    #[test]
    fn bare_numbers_are_refused_as_a_whole_when_unsure() {
        let eps = angels_of_death();
        let f = |p: &str, t: &str, n: i64| (p.to_string(), t.to_string(), n);
        // une autre œuvre glissée dans le pack
        assert!(bare_episodes(
            &[f("a", "angels of death", 1), f("b", "pariah nexus", 2)],
            1,
            &eps
        )
        .is_err());
        // deux fichiers pour un même numéro (deux versions)
        assert!(bare_episodes(
            &[f("a", "angels of death", 3), f("b", "angels of death", 3)],
            1,
            &eps
        )
        .is_err());
        // numéro au-delà de la saison : numérotation absolue, on ne devine pas
        assert!(bare_episodes(&[f("a", "angels of death", 11)], 1, &eps).is_err());
        assert!(bare_episodes(&[f("a", "angels of death", 0)], 1, &eps).is_err());
        // saison inconnue de la fiche
        assert!(bare_episodes(&[f("a", "angels of death", 1)], 2, &eps).is_err());
    }

    #[test]
    fn the_tag_keeps_its_season_for_bare_numbers() {
        assert_eq!(
            homelab_target("homelab:series=104:season=1"),
            Some(TagTarget::Series {
                id: 104,
                season: Some(1)
            })
        );
        // ancienne étiquette sans saison : aucun numéro nu n'est lu
        assert_eq!(
            homelab_target("homelab:series=104"),
            Some(TagTarget::Series {
                id: 104,
                season: None
            })
        );
    }

    #[test]
    fn a_bare_mapping_imports_files_sonarr_did_not_recognise() {
        // manualimport : « Unknown Series », aucun épisode proposé (Angels of Death, 2026-10-03)
        let row = json!({"path": "/dl/AoD/Angels of Death - 07.mkv", "relativePath": "Angels of Death - 07.mkv",
                         "rejections": [{"reason": "Unknown Series"}], "series": null, "episodes": []});
        let m = bare_episodes(
            &[(
                "/dl/AoD/Angels of Death - 07.mkv".to_string(),
                "angels of death".to_string(),
                7,
            )],
            1,
            &angels_of_death(),
        )
        .unwrap();
        let (files, skipped) = fresh_files(
            std::slice::from_ref(&row),
            false,
            104,
            "H",
            &m,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 1, "{skipped:?}");
        assert_eq!(files[0]["episodeIds"], json!([1007]));
        assert_eq!(files[0]["seriesId"], json!(104));
        // l'épisode a déjà un fichier : jamais remplacé
        let (files, skipped) = fresh_files(
            &[row],
            false,
            104,
            "H",
            &m,
            &[1007].into_iter().collect(),
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty() && skipped.len() == 1);
    }

    /// Pack réel du 2026-10-08 (saison 1 d'un animé de 2006, côté seedbox) : 24 fichiers « `NN. Titre` », sans nom de
    /// série ni `SxxEyy`, dans un dossier qui ne nomme pas la saison. Chemins tels que l'Arr les voit.
    const FATE_DIR: &str = "/dl/2 - Fate Stay Night (Saber Route)";
    const FATE_TITLES: [&str; 24] = [
        "Le premier jour",
        "La Nuit fatidique",
        "Lever de rideau",
        "Le Plus puissant des adversaires",
        "Deux Magiciens (1re partie)",
        "Deux Magiciens (2e partie)",
        "Infestation",
        "Dissonances",
        "Ballet au clair de lune",
        "Paisible intermède",
        "La Forteresse de Sang",
        "Celles qui fendent les cieux",
        "Le Château hivernal",
        "Au bout de ses convictions",
        "Les Douze Travaux",
        "L'épée de la victoire promise",
        "La Marque de la sorcière",
        "Bataille décisive",
        "Le Roi d'or",
        "Souvenirs d'un rêve lointain",
        "Déchirant ciel et terre, l'étoile au commencement du monde",
        "La Fin d'un rêve",
        "Le Saint Graal",
        "Utopie lointaine",
    ];

    fn fate_path(n: usize) -> String {
        format!("{FATE_DIR}/{n:02}. {}.mkv", FATE_TITLES[n - 1])
    }

    fn fate_pack(range: std::ops::RangeInclusive<usize>) -> Vec<String> {
        range.map(fate_path).collect()
    }

    /// Fiche Sonarr de l'animé : saison 1 de 24 épisodes (ids 3001…3024, numéros absolus 1…24), 4 spéciaux.
    fn fate_episodes() -> Vec<Value> {
        let mut v: Vec<Value> = (1..=24)
            .map(|n| json!({"id": 3000 + n, "seasonNumber": 1, "episodeNumber": n, "absoluteEpisodeNumber": n}))
            .collect();
        v.extend((1..=4).map(|n| json!({"id": 4000 + n, "seasonNumber": 0, "episodeNumber": n})));
        v
    }

    #[test]
    fn a_leading_number_is_read_only_in_its_strict_form() {
        assert_eq!(numbered_episode(&fate_path(5)), Some(5));
        assert_eq!(numbered_episode(&fate_path(21)), Some(21));
        assert_eq!(numbered_episode("24. Utopie lointaine.mkv"), Some(24));
        assert_eq!(numbered_episode("007. Titre.mkv"), Some(7));
        // le dossier ne compte pas, seul le nom du fichier est lu
        assert_eq!(
            numbered_episode("/dl/Show S02/03. Titre.mkv"),
            Some(3),
            "la saison du DOSSIER n'est pas lue ici"
        );
        // un point ET une espace, au tout début ; jamais une année ; un titre avec au moins une lettre
        assert_eq!(numbered_episode("05.Titre.mkv"), None);
        assert_eq!(numbered_episode("05 - Titre.mkv"), None);
        assert_eq!(numbered_episode("Titre 05.mkv"), None);
        assert_eq!(numbered_episode("Show. 05. Titre.mkv"), None);
        assert_eq!(numbered_episode("2006. Titre.mkv"), None);
        assert_eq!(numbered_episode("05. 1080.mkv"), None);
        assert_eq!(numbered_episode("05. Titre"), None);
        // un nom qui porte sa saison, ou un second numéro, reste aux chemins existants
        assert_eq!(numbered_episode("05. Show S01E05.mkv"), None);
        assert_eq!(numbered_episode("05. Show.s01e05.mkv"), None);
        assert_eq!(numbered_episode("05. Show S01 E05.mkv"), None);
        assert_eq!(numbered_episode("05. Erased S01 - 05 VOSTFR.mkv"), None);
        assert_eq!(numbered_episode("05. Show 1x05.mkv"), None);
        assert_eq!(numbered_episode("05. Show Saison 2.mkv"), None);
        assert_eq!(numbered_episode("05. Show Season 2.mkv"), None);
        assert_eq!(numbered_episode("05. Show - 07.mkv"), None);
        // un autre nombre dans le titre : Sonarr y lit CE numéro-là (`parse` du 2026-10-08 : absolu 13, ou S01E13
        // pour « E13 »), le nom est écarté ; un ordinal collé (« 1re », « 2e ») passe
        assert_eq!(numbered_episode("01. Show 13.mkv"), None);
        assert_eq!(numbered_episode("01. Show E13.mkv"), None);
        assert_eq!(numbered_episode("01. Show #13.mkv"), None);
        assert_eq!(numbered_episode("01. Show 013 [1080p].mkv"), None);
        assert_eq!(numbered_episode("01. Show Episode 13.mkv"), None);
        assert_eq!(numbered_episode("01. Show 13v2.mkv"), None);
        assert_eq!(numbered_episode("01. Les 12 Travaux.mkv"), None);
        assert_eq!(
            numbered_episode("05. Deux Magiciens (1re partie).mkv"),
            Some(5)
        );
        assert_eq!(
            numbered_episode("06. Deux Magiciens (2e partie).mkv"),
            Some(6)
        );
        // limite du filtre de forme : « 13th » passe, alors que Sonarr y lit l'absolu 13 — c'est
        // `sonarr_read_something` qui l'écarte
        assert_eq!(numbered_episode("01. Show 13th Night.mkv"), Some(1));
    }

    /// `parse` réel d'un nom où Sonarr lit un numéro absolu (Sonarr de la seedbox, 2026-10-08).
    fn parse_absolute(n: i64) -> Value {
        json!({"parsedEpisodeInfo": {"seasonNumber": 0, "episodeNumbers": [], "absoluteEpisodeNumbers": [n],
                                     "fullSeason": false, "seriesTitle": "01  Show"},
               "series": null, "episodes": []})
    }

    /// Ligne `manualimport` réelle d'un fichier que Sonarr ne rattache à rien.
    fn unknown_row(path: &str) -> Value {
        json!({"path": path, "relativePath": path.rsplit('/').next().unwrap_or(path),
               "rejections": [{"reason": "Unknown Series"}], "series": null, "episodes": []})
    }

    #[test]
    fn sonarr_reading_anything_keeps_the_numbered_reading_out() {
        let none: HashMap<String, Vec<i64>> = HashMap::new();
        let arr = EpisodeSource::ArrFirst;
        // pack réel : `parse` ne lit rien dans les 24 noms, `manualimport` ne propose rien
        let nothing = json!({"parsedEpisodeInfo": null, "series": null, "episodes": []});
        let rows: Vec<Value> = fate_pack(1..=24).iter().map(|p| unknown_row(p)).collect();
        assert!(!sonarr_read_something(
            &rows,
            &vec![nothing; 24],
            &none,
            arr
        ));
        assert!(!sonarr_read_something(&rows, &[json!({})], &none, arr));
        // un seul nom lu suffit, même un absolu que la fiche ne connaît pas, ou une saison seule
        assert!(sonarr_read_something(
            &rows,
            &[parse_absolute(13)],
            &none,
            arr
        ));
        let season_only = json!({"parsedEpisodeInfo": {"seasonNumber": 1, "fullSeason": true, "episodeNumbers": []}});
        assert!(sonarr_read_something(&rows, &[season_only], &none, arr));
        // `manualimport` propose un épisode, quelle que soit la fiche
        let mut proposed = rows.clone();
        proposed[4] =
            json!({"path": fate_path(5), "series": {"id": 999}, "episodes": [{"id": 3017}]});
        assert!(sonarr_read_something(&proposed, &[], &none, arr));
        // un fichier déjà rangé par `map_episodes`, ou une lecture fansub
        let mapped: HashMap<String, Vec<i64>> =
            [(fate_path(1), vec![]), (fate_path(2), vec![3002])].into();
        assert!(sonarr_read_something(&rows, &[], &mapped, arr));
        assert!(sonarr_read_something(
            &rows,
            &[],
            &none,
            EpisodeSource::OursOnly
        ));
    }

    #[test]
    fn a_second_number_in_the_name_leaves_the_pack_to_sonarr() {
        // revue du 2026-10-08 : second cours publié « 01. Show 13.mkv » … « 12. Show 24.mkv », pris pour la saison 1
        // (24 épisodes, rien avant). Lu par nous, le fichier 01 irait en E01 ; Sonarr y lit l'absolu 13 → E13.
        let eps = fate_episodes();
        let pack: Vec<String> = (1..=12)
            .map(|n| format!("/dl/Show/{n:02}. Show {}.mkv", n + 12))
            .collect();
        // la forme est refusée nom par nom : rien n'est lu ainsi, même si Sonarr n'avait rien lu
        assert!(numbered_pack(Some(1), false, &pack, &eps).is_none());
        // l'ancien chemin, inchangé : le `parse` réel rangé par `map_episodes`
        let parses: Vec<Value> = (13..=24).map(parse_absolute).collect();
        let by_path: HashMap<String, Vec<i64>> = pack
            .iter()
            .zip(&parses)
            .map(|(p, x)| (p.clone(), map_episodes(x, &eps)))
            .collect();
        assert_eq!(by_path[&pack[0]], vec![3013]);
        assert!(sonarr_read_something(
            &[],
            &parses,
            &by_path,
            EpisodeSource::ArrFirst
        ));
        let rows: Vec<Value> = pack.iter().map(|p| unknown_row(p)).collect();
        // E01…E12 libres : jamais pourvus avec le second cours
        let (files, skipped) = fresh_files(
            &rows,
            false,
            110,
            "H",
            &by_path,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 12, "{skipped:?}");
        assert_eq!(files[0]["episodeIds"], json!([3013]));
        assert!(files
            .iter()
            .all(|f| f["episodeIds"][0].as_i64().is_some_and(|id| id >= 3013)));
        // E01…E12 déjà pourvus : le second cours est importé quand même, pas de « nothing_importable »
        let has_file: HashSet<i64> = (3001..=3012).collect();
        let (files, skipped) = fresh_files(
            &rows,
            false,
            110,
            "H",
            &by_path,
            &has_file,
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 12, "{skipped:?}");
        assert_eq!(files[11]["episodeIds"], json!([3024]));
    }

    #[test]
    fn a_number_sonarr_reads_past_the_form_filter_still_wins() {
        // « 13th » passe le filtre de forme, mais Sonarr y lit l'absolu 13 : même si la fiche ne connaît pas cet
        // absolu (`map_episodes` vide), Sonarr a lu quelque chose, on ne devine pas
        let pack: Vec<String> = (1..=12)
            .map(|n| format!("/dl/Show/{n:02}. Show {}th Night.mkv", n + 12))
            .collect();
        assert!(pack.iter().all(|p| numbered_episode(p).is_some()));
        let parses: Vec<Value> = (13..=24).map(parse_absolute).collect();
        let unmapped: HashMap<String, Vec<i64>> =
            pack.iter().map(|p| (p.clone(), Vec::new())).collect();
        let read = sonarr_read_something(&[], &parses, &unmapped, EpisodeSource::ArrFirst);
        assert!(read);
        assert!(numbered_pack(Some(1), read, &pack, &fate_episodes()).is_none());
    }

    #[test]
    fn the_real_numbered_pack_lands_in_the_tagged_season() {
        let eps = fate_episodes();
        let m = numbered_episodes(&fate_pack(1..=24), 1, &eps).unwrap();
        assert_eq!(m.len(), 24);
        assert_eq!(m[&fate_path(1)], vec![3001]);
        assert_eq!(m[&fate_path(5)], vec![3005]);
        assert_eq!(m[&fate_path(24)], vec![3024]);
        // jamais dans les spéciaux
        assert!(m.values().all(|ids| ids.len() == 1 && ids[0] < 4000));
    }

    #[test]
    fn a_partial_continuous_numbered_pack_is_read() {
        // second cours d'une saison de 24 épisodes, saison 1 : rien ne la précède, aucune ambiguïté
        let m = numbered_episodes(&fate_pack(13..=24), 1, &fate_episodes()).unwrap();
        assert_eq!(m.len(), 12);
        assert_eq!(m[&fate_path(13)], vec![3013]);
        assert!(!m.contains_key(&fate_path(12)));
        // un seul fichier, au milieu de la saison
        assert_eq!(
            numbered_episodes(&fate_pack(7..=7), 1, &fate_episodes()).unwrap()[&fate_path(7)],
            vec![3007]
        );
    }

    #[test]
    fn a_numbered_pack_is_refused_as_a_whole_when_unsure() {
        let eps = fate_episodes();
        // un trou : 1..12 puis 14..24
        let mut gap = fate_pack(1..=12);
        gap.extend(fate_pack(14..=24));
        assert!(numbered_episodes(&gap, 1, &eps)
            .unwrap_err()
            .contains("ne se suivent pas"));
        // un doublon : deux versions de l'épisode 5
        let mut dup = fate_pack(1..=24);
        dup.push(format!(
            "{FATE_DIR}/Extras/05. Deux Magiciens (version longue).mkv"
        ));
        assert!(numbered_episodes(&dup, 1, &eps)
            .unwrap_err()
            .contains("deux fichiers portent le numéro 5"));
        // un numéro au-delà de la saison (numérotation absolue d'une saison suivante), ou 0
        let mut over = fate_pack(1..=24);
        over.push(format!("{FATE_DIR}/25. Épilogue.mkv"));
        assert!(numbered_episodes(&over, 1, &eps)
            .unwrap_err()
            .contains("hors de la saison"));
        let mut zero = fate_pack(1..=3);
        zero.push(format!("{FATE_DIR}/00. Prologue.mkv"));
        assert!(numbered_episodes(&zero, 1, &eps).is_err());
        // un mélange de formes : un « S01E24 », un bonus sans numéro
        let mut mixed = fate_pack(1..=23);
        mixed.push(format!("{FATE_DIR}/Fate Stay Night S01E24.mkv"));
        assert!(numbered_episodes(&mixed, 1, &eps)
            .unwrap_err()
            .contains("sans numéro en tête"));
        let mut extra = fate_pack(1..=24);
        extra.push(format!("{FATE_DIR}/NCOP.mkv"));
        assert!(numbered_episodes(&extra, 1, &eps).is_err());
        // saison inconnue de la fiche
        assert!(numbered_episodes(&fate_pack(1..=24), 2, &eps).is_err());
        // rien du tout
        assert!(numbered_episodes(&[], 1, &eps).is_err());
    }

    #[test]
    fn a_numbered_pack_not_starting_at_one_is_refused_after_another_season() {
        // saison 1 de 12, saison 2 de 24 : « 13. … » à « 24. … » pris pour la saison 2 peut être le second cours
        // (S02E13…) ou une numérotation absolue (S02E01…) — on ne devine pas
        let mut eps: Vec<Value> = (1..=12)
            .map(|n| json!({"id": 100 + n, "seasonNumber": 1, "episodeNumber": n}))
            .collect();
        eps.extend((1..=24).map(|n| json!({"id": 200 + n, "seasonNumber": 2, "episodeNumber": n})));
        assert!(numbered_episodes(&fate_pack(13..=24), 2, &eps)
            .unwrap_err()
            .contains("absolue"));
        // la même saison 2 prise depuis son début : lue normalement
        let m = numbered_episodes(&fate_pack(1..=12), 2, &eps).unwrap();
        assert_eq!(m[&fate_path(1)], vec![201]);
    }

    #[test]
    fn a_numbered_pack_needs_the_season_of_the_tag() {
        let eps = fate_episodes();
        let season_of = |tag: &str| match homelab_target(tag) {
            Some(TagTarget::Series { season, .. }) => season,
            _ => None,
        };
        // l'étiquette réelle posée par series_search
        let tagged = numbered_pack(
            season_of("homelab:series=110:season=1"),
            false,
            &fate_pack(1..=24),
            &eps,
        );
        assert_eq!(tagged.unwrap().unwrap().len(), 24);
        // sans saison dans l'étiquette (ancienne étiquette, ou ajout à la main) : rien n'est lu ainsi
        assert_eq!(season_of("homelab:series=110"), None);
        assert!(numbered_pack(
            season_of("homelab:series=110"),
            false,
            &fate_pack(1..=24),
            &eps
        )
        .is_none());
        assert!(numbered_pack(None, false, &fate_pack(1..=24), &eps).is_none());
        // Sonarr a lu quelque chose : dernier recours seulement, rien n'est lu ainsi
        assert!(numbered_pack(Some(1), true, &fate_pack(1..=24), &eps).is_none());
        // aucune vidéo de cette forme : les chemins ordinaires, sans un mot
        let classic = vec!["/dl/Show.S01/Show.S01E01.mkv".to_string()];
        assert!(numbered_pack(Some(1), false, &classic, &eps).is_none());
        // forme présente mais refusée : une erreur, pour le journal
        let mut gap = fate_pack(1..=3);
        gap.push(fate_path(5));
        assert!(numbered_pack(Some(1), false, &gap, &eps).unwrap().is_err());
    }

    #[test]
    fn a_numbered_mapping_fills_what_sonarr_left_empty_and_never_overwrites() {
        let eps = fate_episodes();
        // pack réel, de bout en bout : `parse` ne lit rien, `manualimport` répond « Unknown Series » sans épisode
        let pack = fate_pack(1..=24);
        let rows: Vec<Value> = pack.iter().map(|p| unknown_row(p)).collect();
        let parses = vec![json!({"parsedEpisodeInfo": null}); 24];
        let unmapped: HashMap<String, Vec<i64>> =
            pack.iter().map(|p| (p.clone(), Vec::new())).collect();
        let read = sonarr_read_something(&rows, &parses, &unmapped, EpisodeSource::ArrFirst);
        let m = numbered_pack(Some(1), read, &pack, &eps)
            .expect("forme lue")
            .expect("correspondance certaine");
        // la source reste ArrFirst : Sonarr n'a rien proposé, rien à faire taire
        let (files, skipped) = fresh_files(
            &rows,
            false,
            110,
            "H",
            &m,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert_eq!(files.len(), 24, "{skipped:?}");
        for (n, f) in files.iter().enumerate() {
            assert_eq!(f["path"], json!(fate_path(n + 1)));
            assert_eq!(f["episodeIds"], json!([3001 + n as i64]));
            assert_eq!(f["seriesId"], json!(110));
        }
        // ses autres refus restent bloquants (extrait…)
        let sample = json!({"path": fate_path(5), "rejections": [{"reason": "Sample"}], "series": null, "episodes": []});
        let (files, skipped) = fresh_files(
            &[sample],
            false,
            110,
            "H",
            &m,
            &HashSet::new(),
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty() && skipped[0].contains("Sample"));
        // épisode déjà pourvu : jamais remplacé
        let (files, skipped) = fresh_files(
            &[unknown_row(&fate_path(5))],
            false,
            110,
            "H",
            &m,
            &[3005].into_iter().collect(),
            EpisodeSource::ArrFirst,
        );
        assert!(files.is_empty() && skipped[0].contains("déjà présent"));
    }

    #[test]
    fn a_deselected_file_counts_for_the_pack_form_but_is_never_imported() {
        // l'épisode 5 désélectionné : la forme du pack est jugée sur toutes ses vidéos, l'import n'en prend que
        // les fichiers complets (`incomplete_paths` les retire des candidats avant `fresh_files`)
        let rows: Vec<Value> = (1..=24)
            .map(|n| {
                let name = format!(
                    "2 - Fate Stay Night (Saber Route)/{n:02}. {}.mkv",
                    FATE_TITLES[n - 1]
                );
                let (progress, priority) = if n == 5 { (0.3, 0) } else { (1.0, 1) };
                json!({"name": name, "size": 10, "progress": progress, "priority": priority})
            })
            .collect();
        let files: Vec<TorrentFile> = serde_json::from_value(Value::Array(rows)).unwrap();
        let paths: Vec<String> = video_files(&files)
            .iter()
            .map(|f| format!("/dl/{}", f.name))
            .collect();
        let m = numbered_pack(Some(1), false, &paths, &fate_episodes())
            .unwrap()
            .unwrap();
        assert_eq!(m.len(), 24);
        let bad = incomplete_paths(&files, "/dl/");
        assert_eq!(bad.len(), 1);
        assert!(bad.contains(&fate_path(5)));
    }
}
