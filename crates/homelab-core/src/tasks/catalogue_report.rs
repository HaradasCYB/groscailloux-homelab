//! Rapport « catalogue jamais regardé » (2026-10-08, revue Kaizen `mbr-catalogue-jamais-vu`, décidé par le propriétaire).
//!
//! La revue du 07/10 avait trouvé que 69 % du volume du catalogue n'avait jamais été regardé par aucun compte. La contre-
//! vérification a corrigé le titre : 0,72 To de ce volume sont l'arriéré normal de séries **déjà commencées**, le vrai poids
//! mort est ce qui n'a **jamais été commencé** (~0,87 To, 38 %). Cette tâche tient ce relevé à jour sur `/status.html`.
//! **Lecture seule** : elle ne supprime rien, ne propose aucun bouton, n'écrit ni chez Jellyfin ni chez les Arrs ;
//! l'admin tranche, comme pour le ménage du 25/09.
//!
//! Une fois par jour, une cinquantaine de requêtes API (aucun fichier n'est lu ni analysé) :
//! - **catalogue** : fiches Radarr/Sonarr du VPS et de la seedbox qui ont des fichiers (taille et date `added` de
//!   l'Arr — celle de Jellyfin n'est pas fiable, les déménagements recréent les éléments). Un film arrive avec son
//!   fichier (`movieFile.dateAdded`), une série avec sa première diffusion (`firstAired`) si elle est postérieure à la
//!   fiche : un titre demandé avant sa sortie ne doit pas paraître vieux ;
//! - **vu** : pour chaque compte Jellyfin (désactivés compris), les films et épisodes `IsPlayed` et `IsResumable`, lus
//!   **avec `UserId`** (`GET /Items` sans compte renvoie une liste incomplète), plus toute ligne de Playback Reporting
//!   d'au moins 60 s si le plugin répond ;
//! - **demandeur** : la sorte seulement (`membre`, `admin`, `aucune`), d'après Jellyseerr — jamais un pseudo. `inconnu`
//!   si Jellyseerr est muet ou si la fiche n'a pas d'identifiant TMDB (la demande est introuvable : ce n'est pas « aucune »).
//!
//! Une fiche d'Arr est rapprochée de Jellyfin par le **dossier** (même correspondance que `identity_check`), à défaut
//! par un identifiant TMDB/TVDB sans ambiguïté. Ce qui n'est pas rapproché n'est pas évalué (« je ne sais pas » n'est
//! pas « jamais vu ») et reste compté à part.
//!
//! Trois groupes :
//! - **jamais commencé** : un film sans lecture, une série dont aucun épisode n'a été vu. Ceux arrivés depuis au moins
//!   `min_age_days` jours forment la liste de décision (`never_aged`) ; les autres sont comptés (`never_all`) sans
//!   conclure, avec la date où le prochain atteindra le seuil ;
//! - **série en cours** : commencée mais pas finie. À part : c'est l'arriéré normal d'une longue série, pas du poids
//!   mort. Le volume non vu est une estimation (taille de la série au prorata des épisodes) ;
//! - **voie russe** : épisodes disponibles et lus, dès l'arrivée (le seuil d'âge ne s'y applique pas).
//!
//! Limite connue : les fiches Arr de la seedbox datent de leur création (12/09/2026) ; un titre déménagé du VPS y
//! reparaît « arrivé » ce jour-là. Elles n'atteignent donc le seuil de 60 jours qu'à partir du 11/11.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::deletion_cleanup::{map_path, side_maps};
use super::identity_check::{item_folder, provider_id};
use super::{side_or_skip, Report, Task};
use crate::accounts::JS_ADMIN;
use crate::clients::ArrClient;
use crate::config::{AnimeLibrary, CatalogueReport as Cfg, Config};
use crate::context::TaskContext;
use crate::state::{now, CatalogueBacklog, CatalogueBucket, CatalogueEntry, CatalogueReport};

pub struct CatalogueReportTask;

const FILM: &str = "film";
const SERIES: &str = "série";
const ANIME: &str = "animé";

/// Une lecture d'au moins ce nombre de secondes dans Playback Reporting compte comme « vu » (comme la revue).
const MIN_PLAY_SECS: f64 = 60.0;
/// Longueur maximale d'un titre gardé dans l'état.
const TITLE_MAX: usize = 60;

// ---------------------------------------------------------------------------------------------
// Fiches d'Arr
// ---------------------------------------------------------------------------------------------

/// Dossiers qui font la sorte d'un titre : l'animation (`anime_library`) et la voie russe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    anime: Vec<String>,
    russian: Vec<String>,
}

impl Roots {
    pub fn from_cfg(c: &AnimeLibrary) -> Self {
        let keep = |v: [&String; 2]| {
            v.into_iter()
                .filter(|s| !s.trim_matches('/').is_empty())
                .cloned()
                .collect::<Vec<_>>()
        };
        Self {
            anime: keep([&c.vps_series_root, &c.vps_movies_root])
                .into_iter()
                .chain(keep([&c.seedbox_series_root, &c.seedbox_movies_root]))
                .collect(),
            russian: keep([&c.seedbox_ru_series_root, &c.seedbox_ru_movies_root]),
        }
    }
}

/// `path` est-il `root` ou dedans ? (Frontière de dossier : `/anime` n'est pas `/anime-films`.)
fn under(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    !root.is_empty()
        && path
            .strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Une fiche d'Arr qui a des fichiers, ramenée à ce dont le rapport a besoin.
#[derive(Debug, Clone, PartialEq)]
pub struct Title {
    /// Texte brut tiré de l'Arr, 60 caractères au plus (échappé à l'affichage).
    pub name: String,
    pub kind: &'static str,
    /// `vps` ou `seedbox`.
    pub side: String,
    pub movie: bool,
    pub bytes: u64,
    /// Arrivée d'après l'Arr (secondes) ; `None` si la date est illisible : le titre ne conclut jamais.
    pub arrived: Option<i64>,
    pub tmdb: i64,
    pub tvdb: i64,
    /// Épisodes présents (séries).
    pub episodes: u32,
    pub russian: bool,
    /// Dossier de la fiche tel que Jellyfin le voit (chemin de l'Arr passé par `map_path`).
    pub jf_path: Option<String>,
}

fn parse_date(d: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(d)
        .ok()
        .map(|d| d.timestamp())
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn number(v: &Value, ptr: &str) -> i64 {
    v.pointer(ptr).and_then(Value::as_i64).unwrap_or(0)
}

fn shorten(s: &str) -> String {
    s.trim().chars().take(TITLE_MAX).collect()
}

fn kind_of(movie: bool, series_type: &str, path: &str, roots: &Roots) -> &'static str {
    if series_type.eq_ignore_ascii_case("anime") || roots.anime.iter().any(|r| under(r, path)) {
        ANIME
    } else if movie {
        FILM
    } else {
        SERIES
    }
}

/// Fiche Radarr avec fichier. Arrivée = la plus tardive de l'ajout de la fiche et de l'import du fichier : un film
/// demandé avant sa sortie n'« arrive » qu'avec son fichier. `jf_path` : dossier vu par Jellyfin, si l'Arr le situe.
pub fn movie_title(side: &str, v: &Value, roots: &Roots, jf_path: Option<String>) -> Option<Title> {
    let bytes = number(v, "/sizeOnDisk").max(0) as u64;
    if bytes == 0 {
        return None;
    }
    let path = text(v, "path");
    let year = number(v, "/year");
    let name = if year > 0 {
        shorten(&format!("{} ({year})", text(v, "title")))
    } else {
        shorten(text(v, "title"))
    };
    let arrived = [
        parse_date(text(v, "added")),
        v.pointer("/movieFile/dateAdded")
            .and_then(Value::as_str)
            .and_then(parse_date),
    ]
    .into_iter()
    .flatten()
    .max();
    Some(Title {
        name,
        kind: kind_of(true, "", path, roots),
        side: side.to_string(),
        movie: true,
        bytes,
        arrived,
        tmdb: number(v, "/tmdbId"),
        tvdb: 0,
        episodes: 0,
        russian: roots.russian.iter().any(|r| under(r, path)),
        jf_path,
    })
}

/// Fiche Sonarr avec au moins un fichier. Arrivée = la plus tardive de l'ajout de la fiche (`added`) et de la première
/// diffusion (`firstAired`) : une série demandée avant sa diffusion a sa fiche depuis la demande, mais ses fichiers
/// n'arrivent qu'avec les épisodes (même cas que le film demandé avant sa sortie). Sonarr ne donne pas de date d'import
/// par épisode sur la fiche : un épisode ajouté plus tard ne remet pas la série à zéro (2026-10-08).
pub fn series_title(
    side: &str,
    v: &Value,
    roots: &Roots,
    jf_path: Option<String>,
) -> Option<Title> {
    let bytes = number(v, "/statistics/sizeOnDisk").max(0) as u64;
    if bytes == 0 {
        return None;
    }
    let path = text(v, "path");
    Some(Title {
        name: shorten(text(v, "title")),
        kind: kind_of(false, text(v, "seriesType"), path, roots),
        side: side.to_string(),
        movie: false,
        bytes,
        arrived: [
            parse_date(text(v, "added")),
            parse_date(text(v, "firstAired")),
        ]
        .into_iter()
        .flatten()
        .max(),
        tmdb: number(v, "/tmdbId"),
        tvdb: number(v, "/tvdbId"),
        episodes: number(v, "/statistics/episodeFileCount").max(0) as u32,
        russian: roots.russian.iter().any(|r| under(r, path)),
        jf_path,
    })
}

// ---------------------------------------------------------------------------------------------
// Jellyfin : catalogue et « vu »
// ---------------------------------------------------------------------------------------------

/// Identifiant Jellyfin en 32 caractères minuscules sans tirets (l'API et Playback Reporting n'écrivent pas pareil).
fn norm(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

/// Le compte qui lit le catalogue : un admin actif qui voit **toutes** les bibliothèques (un admin à liste explicite ne
/// verrait pas les bibliothèques russes), à défaut le premier admin actif.
pub fn pick_admin(users: &[Value]) -> Option<String> {
    let flag = |u: &Value, k: &str| {
        u.pointer(&format!("/Policy/{k}"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let admins = || {
        users
            .iter()
            .filter(|u| flag(u, "IsAdministrator") && !flag(u, "IsDisabled"))
    };
    admins()
        .find(|u| flag(u, "EnableAllFolders"))
        .or_else(|| admins().next())
        .and_then(|u| u.get("Id").and_then(Value::as_str))
        .map(norm)
}

/// Films et séries de Jellyfin, pour y retrouver une fiche d'Arr.
#[derive(Debug, Default)]
pub struct JfIndex {
    by_path: HashMap<String, String>,
    movie_tmdb: HashMap<i64, Vec<String>>,
    series_tvdb: HashMap<i64, Vec<String>>,
    series_tmdb: HashMap<i64, Vec<String>>,
}

impl JfIndex {
    pub fn from_items(items: &[Value]) -> Self {
        let mut ix = Self::default();
        for it in items {
            let Some(id) = it.get("Id").and_then(Value::as_str).map(norm) else {
                continue;
            };
            let movie = match it.get("Type").and_then(Value::as_str) {
                Some("Movie") => true,
                Some("Series") => false,
                _ => continue,
            };
            if let Some(folder) = item_folder(it) {
                ix.by_path.insert(folder, id.clone());
            }
            let (tmdb, tvdb) = (provider_id(it, "Tmdb"), provider_id(it, "Tvdb"));
            if movie {
                push_id(&mut ix.movie_tmdb, tmdb, &id);
            } else {
                push_id(&mut ix.series_tvdb, tvdb, &id);
                push_id(&mut ix.series_tmdb, tmdb, &id);
            }
        }
        ix
    }

    /// Identifiant Jellyfin de la fiche : par dossier d'abord ; à défaut par identifiant TMDB (film) ou TVDB puis TMDB
    /// (série), seulement s'il n'y a **qu'un** élément Jellyfin qui le porte (une série présente des deux côtés, VPS et
    /// seedbox, en a deux : on ne devine pas lequel).
    pub fn find(&self, t: &Title) -> Option<&str> {
        if let Some(id) = t.jf_path.as_ref().and_then(|p| self.by_path.get(p)) {
            return Some(id);
        }
        if t.movie {
            unique(&self.movie_tmdb, t.tmdb)
        } else {
            unique(&self.series_tvdb, t.tvdb).or_else(|| unique(&self.series_tmdb, t.tmdb))
        }
    }
}

/// L'élément Jellyfin qui porte l'identifiant `k`, s'il n'y en a qu'un.
fn unique(m: &HashMap<i64, Vec<String>>, k: i64) -> Option<&str> {
    let ids = m.get(&k).filter(|_| k > 0)?;
    (ids.len() == 1).then(|| ids[0].as_str())
}

fn push_id(m: &mut HashMap<i64, Vec<String>>, key: i64, id: &str) {
    if key > 0 {
        m.entry(key).or_default().push(id.to_string());
    }
}

/// Ce qu'au moins un compte a vu ou commencé : films, et épisodes regroupés par série Jellyfin.
#[derive(Debug, Default)]
pub struct Seen {
    movies: HashSet<String>,
    episodes: HashMap<String, HashSet<String>>,
    /// Tout identifiant déjà rangé (film ou épisode), pour ne résoudre que les inconnus de Playback Reporting.
    known: HashSet<String>,
}

impl Seen {
    /// Un film ou un épisode renvoyé par Jellyfin (les autres types sont ignorés ; un épisode sans série aussi).
    pub fn add_item(&mut self, it: &Value) {
        let Some(id) = it.get("Id").and_then(Value::as_str).map(norm) else {
            return;
        };
        match it.get("Type").and_then(Value::as_str) {
            Some("Movie") => {
                self.movies.insert(id.clone());
            }
            Some("Episode") => {
                let Some(series) = it.get("SeriesId").and_then(Value::as_str).map(norm) else {
                    return;
                };
                self.episodes.entry(series).or_default().insert(id.clone());
            }
            _ => return,
        }
        self.known.insert(id);
    }

    pub fn movie_seen(&self, id: &str) -> bool {
        self.movies.contains(id)
    }

    pub fn episodes_seen(&self, series_id: &str) -> usize {
        self.episodes.get(series_id).map_or(0, HashSet::len)
    }

    /// Parmi `ids` (Playback Reporting), ceux qu'aucune liste de compte n'a rangés : triés, sans doublon.
    pub fn unknown(&self, ids: &[String]) -> Vec<String> {
        let mut v: Vec<String> = ids
            .iter()
            .map(|i| norm(i))
            .filter(|i| !self.known.contains(i))
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Playback Reporting : identifiants des éléments dont une ligne dure au moins 60 s. Colonnes `ItemId` et `secs`.
pub fn long_plays(cols: &[String], rows: &[Vec<String>]) -> Result<Vec<String>> {
    let idx = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
    let (i, s) = (
        idx("ItemId").context("colonne ItemId absente")?,
        idx("secs").context("colonne secs absente")?,
    );
    Ok(rows
        .iter()
        .filter_map(|r| {
            let secs = r.get(s)?.parse::<f64>().ok()?;
            if secs < MIN_PLAY_SECS {
                return None;
            }
            Some(norm(r.get(i)?))
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Jellyseerr : sorte de demandeur
// ---------------------------------------------------------------------------------------------

const MEMBER: &str = "membre";
const ADMIN: &str = "admin";
const NONE: &str = "aucune";
const UNKNOWN: &str = "inconnu";

/// Qui a demandé chaque titre, par (film ?, identifiant TMDB) : (un membre, un admin).
#[derive(Debug, Default)]
pub struct Requests {
    available: bool,
    by: HashMap<(bool, i64), (bool, bool)>,
}

impl Requests {
    /// Jellyseerr n'a pas répondu : la sorte de demandeur est « inconnu », jamais « aucune demande ».
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// Un compte est admin par le bit 2 des permissions Jellyseerr. Une demande sans demandeur (compte supprimé) ou sans
    /// identifiant TMDB est ignorée.
    pub fn from_json(requests: &[Value]) -> Self {
        let mut by: HashMap<(bool, i64), (bool, bool)> = HashMap::new();
        for q in requests {
            let movie = match q.get("type").and_then(Value::as_str) {
                Some("movie") => true,
                Some("tv") => false,
                _ => continue,
            };
            let Some(tmdb) = q.pointer("/media/tmdbId").and_then(Value::as_i64) else {
                continue;
            };
            let Some(perm) = q
                .pointer("/requestedBy/permissions")
                .and_then(Value::as_i64)
            else {
                continue;
            };
            let e = by.entry((movie, tmdb)).or_default();
            if perm & JS_ADMIN != 0 {
                e.1 = true;
            } else {
                e.0 = true;
            }
        }
        Self {
            available: true,
            by,
        }
    }

    /// `membre` si au moins un membre l'a demandé (c'est lui que la liste d'envies concerne), sinon `admin`, sinon
    /// `aucune` ; `inconnu` si Jellyseerr n'a pas répondu **ou** si la fiche n'a pas d'identifiant TMDB (0 ou absent) :
    /// la demande ne peut alors pas être retrouvée, et « aucune demande » serait une affirmation que rien ne prouve
    /// (2026-10-08).
    pub fn kind(&self, movie: bool, tmdb: i64) -> &'static str {
        if !self.available || tmdb <= 0 {
            return UNKNOWN;
        }
        match self.by.get(&(movie, tmdb)) {
            Some((true, _)) => MEMBER,
            Some((false, true)) => ADMIN,
            _ => NONE,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Le rapport
// ---------------------------------------------------------------------------------------------

pub struct Inputs<'a> {
    pub titles: &'a [Title],
    pub jf: &'a JfIndex,
    pub seen: &'a Seen,
    pub requests: &'a Requests,
    pub now: i64,
    /// Playback Reporting a répondu à ce passage.
    pub playback_reporting: bool,
}

fn add(b: &mut CatalogueBucket, bytes: u64) {
    b.titles += 1;
    b.bytes += bytes;
}

fn entry(t: &Title, requester: &str, bytes: u64, seen: u32, total: u32) -> CatalogueEntry {
    CatalogueEntry {
        title: t.name.clone(),
        kind: t.kind.to_string(),
        side: t.side.clone(),
        bytes,
        added: t.arrived.unwrap_or(0),
        requester: requester.to_string(),
        seen,
        total,
    }
}

/// Les plus gros d'abord ; à taille égale, l'ordre alphabétique (un rapport reproductible).
fn biggest_first(v: &mut Vec<CatalogueEntry>, max: usize) {
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.title.cmp(&b.title)));
    v.truncate(max);
}

/// Le calcul, pur : même entrée, même rapport.
pub fn build_report(inp: &Inputs<'_>, cfg: &Cfg) -> CatalogueReport {
    let min_age = cfg.min_age_days.max(0) * 86_400;
    let mut r = CatalogueReport {
        at: inp.now,
        min_age_days: cfg.min_age_days,
        playback_reporting: inp.playback_reporting,
        ..Default::default()
    };
    let (mut listed, mut backlog_listed) = (Vec::new(), Vec::new());
    for t in inp.titles {
        let Some(jid) = inp.jf.find(t) else {
            add(&mut r.unmatched, t.bytes);
            continue;
        };
        add(&mut r.catalogue, t.bytes);
        let (seen, total) = if t.movie {
            (u32::from(inp.seen.movie_seen(jid)), 1)
        } else {
            // Jellyfin peut connaître plus d'épisodes vus que de fichiers (épisode remplacé, supprimé depuis)
            let n = u32::try_from(inp.seen.episodes_seen(jid)).unwrap_or(u32::MAX);
            (n.min(t.episodes), t.episodes)
        };
        let requester = inp.requests.kind(t.movie, t.tmdb);
        if t.russian {
            let ru = &mut r.russian;
            ru.bytes += t.bytes;
            if t.movie {
                ru.movies += 1;
                ru.movies_seen += seen;
            } else {
                ru.series += 1;
                ru.episodes += total;
                ru.episodes_seen += seen;
            }
        }
        if seen == 0 {
            add(&mut r.never_all, t.bytes);
            r.all.add(t.kind, &t.side, requester, t.bytes);
            let aged = t.arrived.is_some_and(|a| inp.now - a >= min_age);
            if aged {
                add(&mut r.never_aged, t.bytes);
                r.aged.add(t.kind, &t.side, requester, t.bytes);
                listed.push(entry(t, requester, t.bytes, 0, total));
            } else if let Some(a) = t.arrived {
                let due = a + min_age;
                r.next_aged_at = Some(r.next_aged_at.map_or(due, |d| d.min(due)));
            }
        } else if !t.movie && seen < total {
            // commencée, pas finie : le volume non vu est le prorata des épisodes
            let unseen = total - seen;
            let est = (u128::from(t.bytes) * u128::from(unseen) / u128::from(total)) as u64;
            r.backlog = CatalogueBacklog {
                series: r.backlog.series + 1,
                unseen_episodes: r.backlog.unseen_episodes + unseen,
                bytes: r.backlog.bytes + est,
            };
            backlog_listed.push(entry(t, requester, est, seen, total));
        }
    }
    biggest_first(&mut listed, cfg.max_listed);
    biggest_first(&mut backlog_listed, cfg.max_backlog_listed);
    r.listed = listed;
    r.backlog_listed = backlog_listed;
    r
}

/// Part de `part` dans `whole`, en pourcentage (0 si `whole` est nul).
pub fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// « 22,1 Go », « 1,59 To », « 48 Mo » (virgule décimale).
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    let s = if bytes >= 1_000_000_000_000 {
        format!("{:.2} To", b / 1e12)
    } else if bytes >= 100_000_000_000 {
        format!("{:.0} Go", b / 1e9)
    } else if bytes >= 100_000_000 {
        format!("{:.1} Go", b / 1e9)
    } else {
        format!("{:.0} Mo", b / 1e6)
    };
    s.replace('.', ",")
}

/// Une décimale, virgule : « 36,8 ».
pub fn pct_text(p: f64) -> String {
    format!("{p:.1}").replace('.', ",")
}

/// Résumé d'un passage (une ligne, affichée dans le tableau des tâches).
pub fn summary(r: &CatalogueReport) -> String {
    format!(
        "{} jamais vu(s) depuis {} j ou plus ({}, {} % du catalogue) · {} plus récent(s) · {} série(s) en cours",
        r.never_aged.titles,
        r.min_age_days,
        human_size(r.never_aged.bytes),
        pct_text(percent(r.never_aged.bytes, r.catalogue.bytes)),
        r.never_all.titles - r.never_aged.titles,
        r.backlog.series,
    )
}

// ---------------------------------------------------------------------------------------------
// Lectures (API)
// ---------------------------------------------------------------------------------------------

/// Fiches des Arrs du VPS et de la seedbox. `None` si un côté est injoignable : un rapport sans la seedbox (80 % du
/// volume) serait faux, on garde le précédent. La panne est déjà signalée par `seedbox_health`.
async fn read_titles(ctx: &TaskContext) -> Option<Vec<Title>> {
    let roots = Roots::from_cfg(&ctx.cfg.tasks.anime_library);
    let mut sides: Vec<(&'static str, &ArrClient, &ArrClient)> =
        vec![("vps", &ctx.radarr, &ctx.sonarr)];
    if let (Some(r), Some(s)) = (&ctx.seedbox_radarr, &ctx.seedbox_sonarr) {
        sides.push(("seedbox", r, s));
    }
    let mut out = Vec::new();
    for (side, radarr, sonarr) in sides {
        let maps = side_maps(ctx, side);
        let jf_path =
            |v: &Value| map_path(&maps, text(v, "path")).map(|(_, jellyfin_path)| jellyfin_path);
        let movies = side_or_skip("catalogue_report", side, radarr.movies().await)?;
        let series = side_or_skip("catalogue_report", side, sonarr.series().await)?;
        out.extend(
            movies
                .iter()
                .filter_map(|v| movie_title(side, v, &roots, jf_path(v))),
        );
        out.extend(
            series
                .iter()
                .filter_map(|v| series_title(side, v, &roots, jf_path(v))),
        );
    }
    Some(out)
}

/// Ce que les comptes ont vu ou commencé, plus Playback Reporting (≥ 60 s). `false` : Playback Reporting n'a pas répondu.
/// Une erreur sur un compte fait échouer le passage : un « vu » incomplet gonflerait le « jamais vu ».
async fn read_seen(
    ctx: &TaskContext,
    users: &[Value],
    admin: &str,
    cfg: &Cfg,
) -> Result<(Seen, bool)> {
    let mut seen = Seen::default();
    for u in users {
        let Some(uid) = u.get("Id").and_then(Value::as_str) else {
            continue;
        };
        for filter in ["IsPlayed", "IsResumable"] {
            let items = ctx
                .jellyfin
                .items_paged(
                    &[
                        ("UserId", uid),
                        ("Recursive", "true"),
                        ("IncludeItemTypes", "Movie,Episode"),
                        ("Filters", filter),
                        ("EnableImages", "false"),
                        ("EnableUserData", "false"),
                    ],
                    1000,
                )
                .await
                .with_context(|| format!("lecture des éléments {filter} d'un compte"))?;
            for it in &items {
                seen.add_item(it);
            }
        }
        tokio::time::sleep(Duration::from_millis(cfg.user_pause_ms)).await;
    }
    let sql = "SELECT ItemId, MAX(PlayDuration) AS secs FROM PlaybackActivity GROUP BY ItemId";
    let plays = match ctx.jellyfin.playback_query(sql).await {
        Ok((cols, rows)) => long_plays(&cols, &rows).map(Some).unwrap_or_else(|e| {
            warn!(
                task = "catalogue_report",
                error = format!("{e:#}"),
                "Playback Reporting illisible"
            );
            None
        }),
        Err(e) => {
            warn!(
                task = "catalogue_report",
                error = format!("{e:#}"),
                "Playback Reporting injoignable"
            );
            None
        }
    };
    let Some(plays) = plays else {
        return Ok((seen, false));
    };
    // lecture d'au moins 60 s sans « vu » ni « en cours » (< 5 % d'un épisode) : on retrouve l'élément et sa série
    let unknown = seen.unknown(&plays);
    for it in ctx.jellyfin.items_by_ids_as(admin, &unknown).await? {
        seen.add_item(&it);
    }
    Ok((seen, true))
}

#[async_trait]
impl Task for CatalogueReportTask {
    fn name(&self) -> &'static str {
        "catalogue_report"
    }

    fn label(&self) -> &'static str {
        "Rapport du catalogue"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.catalogue_report.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.catalogue_report;
        let users = ctx.jellyfin.users().await?;
        let admin = pick_admin(&users).context("aucun compte admin actif")?;
        let Some(titles) = read_titles(ctx).await else {
            return Ok(Report::new(
                "un côté Arr est injoignable : rapport précédent conservé",
                0,
            ));
        };
        // vue d'un compte (jamais sans `UserId`) ; une page de 500 : ~280 éléments aujourd'hui
        let items = ctx
            .jellyfin
            .items_paged(
                &[
                    ("UserId", admin.as_str()),
                    ("Recursive", "true"),
                    ("IncludeItemTypes", "Series,Movie"),
                    ("Fields", "ProviderIds,Path"),
                    ("EnableImages", "false"),
                    ("EnableUserData", "false"),
                ],
                500,
            )
            .await?;
        let jf = JfIndex::from_items(&items);
        let (seen, playback_reporting) = read_seen(ctx, &users, &admin, cfg).await?;
        let requests = match ctx.jellyseerr.all_requests().await {
            Ok(v) => Requests::from_json(&v),
            Err(e) => {
                warn!(
                    task = "catalogue_report",
                    error = format!("{e:#}"),
                    "Jellyseerr injoignable : demandeur inconnu"
                );
                Requests::unavailable()
            }
        };
        let report = build_report(
            &Inputs {
                titles: &titles,
                jf: &jf,
                seen: &seen,
                requests: &requests,
                now: now(),
                playback_reporting,
            },
            cfg,
        );
        let lost: Vec<&str> = titles
            .iter()
            .filter(|t| jf.find(t).is_none())
            .map(|t| t.name.as_str())
            .take(10)
            .collect();
        if !lost.is_empty() {
            info!(
                task = "catalogue_report",
                n = report.unmatched.titles,
                titles = lost.join(" ; "),
                "fiches d'Arr sans élément Jellyfin : non évaluées"
            );
        }
        let summary = summary(&report);
        if ctx.dry_run {
            info!(
                task = "catalogue_report",
                report = %serde_json::to_string(&report)?,
                "dry-run : rapport calculé, non enregistré"
            );
        } else {
            let keep = report.clone();
            ctx.state.update(move |s| s.catalogue = Some(keep)).await?;
        }
        Ok(Report::new(summary, 0))
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CatalogueRussian;
    use serde_json::json;

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_790_000_000;

    fn cfg() -> Cfg {
        Cfg::default()
    }

    fn roots() -> Roots {
        Roots::from_cfg(&AnimeLibrary::default())
    }

    /// Un titre qui existe aussi dans Jellyfin sous `id` (dossier `/media/<nom>`).
    fn title(name: &str, movie: bool, bytes: u64, age_days: i64, episodes: u32) -> Title {
        Title {
            name: name.into(),
            kind: if movie { FILM } else { SERIES },
            side: "seedbox".into(),
            movie,
            bytes,
            arrived: Some(NOW - age_days * DAY),
            tmdb: 0,
            tvdb: 0,
            episodes,
            russian: false,
            jf_path: Some(format!("/media/{name}")),
        }
    }

    fn jf_for(titles: &[Title]) -> JfIndex {
        let items: Vec<Value> = titles
            .iter()
            .map(|t| {
                let p = t.jf_path.clone().unwrap();
                if t.movie {
                    json!({"Id": jid(t), "Type": "Movie", "Path": format!("{p}/f.mkv")})
                } else {
                    json!({"Id": jid(t), "Type": "Series", "Path": p})
                }
            })
            .collect();
        JfIndex::from_items(&items)
    }

    /// Identifiant Jellyfin du titre dans les tests (32 caractères en vrai ; ici minuscules sans tiret, comme `norm`).
    fn jid(t: &Title) -> String {
        format!("{}x{}", if t.movie { "m" } else { "s" }, t.name).to_ascii_lowercase()
    }

    /// Marque `n` épisodes d'une série comme vus.
    fn watch_episodes(seen: &mut Seen, t: &Title, n: usize) {
        for i in 0..n {
            seen.add_item(&json!({
                "Id": format!("e{i}x{}", t.name),
                "Type": "Episode",
                "SeriesId": jid(t),
            }));
        }
    }

    fn report(titles: &[Title], seen: &Seen, requests: &Requests, c: &Cfg) -> CatalogueReport {
        build_report(
            &Inputs {
                titles,
                jf: &jf_for(titles),
                seen,
                requests,
                now: NOW,
                playback_reporting: true,
            },
            c,
        )
    }

    #[test]
    fn a_never_watched_title_is_listed_only_from_the_threshold() {
        let titles = [
            title("Vieux", true, 5_000_000_000, 61, 0),
            title("PileSeuil", true, 4_000_000_000, 60, 0),
            title("Presque", true, 3_000_000_000, 59, 0),
            title("Neuf", true, 2_000_000_000, 5, 0),
        ];
        let r = report(&titles, &Seen::default(), &Requests::unavailable(), &cfg());
        let names: Vec<&str> = r.listed.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            names,
            ["Vieux", "PileSeuil"],
            "60 jours pile compte, 59 non"
        );
        assert_eq!(
            r.never_aged,
            CatalogueBucket {
                titles: 2,
                bytes: 9_000_000_000
            }
        );
        // les 4 sont jamais vus, tous âges confondus : les deux récents sont comptés sans conclure
        assert_eq!(
            r.never_all,
            CatalogueBucket {
                titles: 4,
                bytes: 14_000_000_000
            }
        );
        assert_eq!(r.catalogue.titles, 4);
        // le prochain à passer le seuil : « Presque » dans 1 jour
        assert_eq!(r.next_aged_at, Some(NOW - 59 * DAY + 60 * DAY));
    }

    #[test]
    fn a_watched_or_resumed_title_is_not_a_candidate() {
        let titles = [
            title("Vu", true, 8_000_000_000, 200, 0),
            title("Intact", true, 6_000_000_000, 200, 0),
        ];
        let mut seen = Seen::default();
        // « vu » ou « en cours » sur le compte d'un membre : le même ajout (le rapport ne distingue pas)
        seen.add_item(&json!({"Id": jid(&titles[0]).to_uppercase(), "Type": "Movie"}));
        let r = report(&titles, &seen, &Requests::unavailable(), &cfg());
        assert_eq!(r.listed.len(), 1);
        assert_eq!(r.listed[0].title, "Intact");
        assert_eq!(r.catalogue.titles, 2);
    }

    #[test]
    fn a_partly_watched_series_is_backlog_not_dead_weight() {
        let t = title("Longue", false, 100_000_000_000, 300, 100);
        let mut seen = Seen::default();
        watch_episodes(&mut seen, &t, 25);
        let r = report(&[t], &seen, &Requests::unavailable(), &cfg());
        assert_eq!(r.never_all.titles, 0, "commencée : pas du poids mort");
        assert!(r.listed.is_empty());
        assert_eq!(r.backlog.series, 1);
        assert_eq!(r.backlog.unseen_episodes, 75);
        assert_eq!(
            r.backlog.bytes, 75_000_000_000,
            "prorata des épisodes non vus"
        );
        let e = &r.backlog_listed[0];
        assert_eq!((e.seen, e.total, e.bytes), (25, 100, 75_000_000_000));
    }

    #[test]
    fn a_fully_watched_series_is_neither_listed_nor_backlog() {
        let t = title("Finie", false, 10_000_000_000, 300, 12);
        let mut seen = Seen::default();
        watch_episodes(&mut seen, &t, 12);
        let r = report(&[t], &seen, &Requests::unavailable(), &cfg());
        assert_eq!(r.never_all.titles, 0);
        assert_eq!(r.backlog.series, 0);
        assert_eq!(r.catalogue.titles, 1);
    }

    #[test]
    fn more_watched_episodes_than_files_is_capped_to_the_files() {
        // un épisode vu a été remplacé depuis : Jellyfin en garde la trace, l'Arr n'a plus que 10 fichiers
        let t = title("Remplacee", false, 10_000_000_000, 300, 10);
        let mut seen = Seen::default();
        watch_episodes(&mut seen, &t, 14);
        let r = report(&[t], &seen, &Requests::unavailable(), &cfg());
        assert_eq!(r.backlog.series, 0, "vu = 10 sur 10, rien à rattraper");
    }

    #[test]
    fn the_same_episode_seen_by_two_accounts_counts_once() {
        let t = title("Deux", false, 4_000_000_000, 100, 4);
        let mut seen = Seen::default();
        for _ in 0..2 {
            watch_episodes(&mut seen, &t, 1);
        }
        assert_eq!(seen.episodes_seen(&jid(&t)), 1);
    }

    #[test]
    fn requester_is_a_kind_never_a_name() {
        let reqs = Requests::from_json(&[
            // un membre (permissions 160 = auto-approbation + demande) et un admin sur le même film
            json!({"type": "movie", "media": {"tmdbId": 1}, "requestedBy": {"permissions": 160, "username": "qui"}}),
            json!({"type": "movie", "media": {"tmdbId": 1}, "requestedBy": {"permissions": 2}}),
            // admin seul
            json!({"type": "movie", "media": {"tmdbId": 2}, "requestedBy": {"permissions": 2}}),
            // série (tv) : le même numéro TMDB n'est pas celui du film
            json!({"type": "tv", "media": {"tmdbId": 2}, "requestedBy": {"permissions": 32}}),
            // demandeur supprimé, ou sans identifiant : ignorées
            json!({"type": "movie", "media": {"tmdbId": 3}, "requestedBy": null}),
            json!({"type": "movie", "media": {}, "requestedBy": {"permissions": 32}}),
        ]);
        assert_eq!(
            reqs.kind(true, 1),
            MEMBER,
            "un membre l'emporte sur un admin"
        );
        assert_eq!(reqs.kind(true, 2), ADMIN);
        assert_eq!(reqs.kind(false, 2), MEMBER);
        assert_eq!(reqs.kind(true, 3), NONE);
        assert_eq!(reqs.kind(true, 99), NONE);
        // Jellyseerr muet : on ne prétend pas qu'il n'y a pas de demande
        assert_eq!(Requests::unavailable().kind(true, 1), UNKNOWN);
        // fiche sans identifiant TMDB (0 ou négatif) : la demande est introuvable, donc « inconnu », jamais « aucune »
        assert_eq!(reqs.kind(false, 0), UNKNOWN);
        assert_eq!(reqs.kind(true, 0), UNKNOWN);
        assert_eq!(reqs.kind(true, -1), UNKNOWN);
    }

    #[test]
    fn the_requester_kind_reaches_the_buckets_and_the_entries() {
        let mut a = title("Demande", true, 5_000_000_000, 90, 0);
        a.tmdb = 10;
        let mut b = title("Libre", true, 3_000_000_000, 90, 0);
        b.tmdb = 11;
        let reqs = Requests::from_json(&[
            json!({"type": "movie", "media": {"tmdbId": 10}, "requestedBy": {"permissions": 32}}),
        ]);
        let r = report(&[a, b], &Seen::default(), &reqs, &cfg());
        assert_eq!(
            r.aged.by_requester[MEMBER],
            CatalogueBucket {
                titles: 1,
                bytes: 5_000_000_000
            }
        );
        assert_eq!(
            r.aged.by_requester[NONE],
            CatalogueBucket {
                titles: 1,
                bytes: 3_000_000_000
            }
        );
        assert_eq!(r.listed[0].requester, MEMBER);
        assert_eq!(r.aged.by_kind[FILM].titles, 2);
        assert_eq!(r.aged.by_side["seedbox"].titles, 2);
        // tous âges : mêmes titres ici (tous ont plus de 60 jours)
        assert_eq!(r.all, r.aged);
    }

    #[test]
    fn the_list_is_capped_but_the_totals_count_everything() {
        let titles: Vec<Title> = (0..6)
            .map(|i| title(&format!("T{i}"), true, (i + 1) * 1_000_000_000, 90, 0))
            .collect();
        let c = Cfg {
            max_listed: 3,
            ..cfg()
        };
        let r = report(&titles, &Seen::default(), &Requests::unavailable(), &c);
        assert_eq!(r.listed.len(), 3);
        assert_eq!(r.never_aged.titles, 6, "les totaux ne sont pas bornés");
        assert_eq!(r.never_aged.bytes, 21_000_000_000);
        let sizes: Vec<u64> = r.listed.iter().map(|e| e.bytes).collect();
        assert_eq!(sizes, [6_000_000_000, 5_000_000_000, 4_000_000_000]);
    }

    #[test]
    fn an_unreadable_arrival_date_never_concludes() {
        let mut t = title("SansDate", true, 9_000_000_000, 0, 0);
        t.arrived = None;
        let r = report(&[t], &Seen::default(), &Requests::unavailable(), &cfg());
        assert_eq!(r.never_all.titles, 1);
        assert_eq!(r.never_aged.titles, 0);
        assert_eq!(r.next_aged_at, None);
    }

    #[test]
    fn a_title_missing_from_jellyfin_is_not_evaluated() {
        let ok = title("Present", true, 2_000_000_000, 90, 0);
        let mut lost = title("Perdu", true, 7_000_000_000, 90, 0);
        lost.jf_path = Some("/media/ailleurs".into());
        let items = [json!({"Id": "a", "Type": "Movie", "Path": "/media/Present/f.mkv"})];
        let r = build_report(
            &Inputs {
                titles: &[ok, lost],
                jf: &JfIndex::from_items(&items),
                seen: &Seen::default(),
                requests: &Requests::unavailable(),
                now: NOW,
                playback_reporting: false,
            },
            &cfg(),
        );
        assert_eq!(
            r.unmatched,
            CatalogueBucket {
                titles: 1,
                bytes: 7_000_000_000
            }
        );
        assert_eq!(r.catalogue.titles, 1);
        assert_eq!(
            r.never_aged.titles, 1,
            "« je ne sais pas » n'est pas « jamais vu »"
        );
        assert!(!r.playback_reporting);
    }

    #[test]
    fn matching_goes_by_folder_then_by_a_unique_id() {
        let items = [
            json!({"Id": "AA-11", "Type": "Movie", "Path": "/media/A/a.mkv", "ProviderIds": {"Tmdb": "5"}}),
            json!({"Id": "bb", "Type": "Movie", "Path": "/media/B/b.mkv", "ProviderIds": {"Tmdb": "7"}}),
            // une même série des deux côtés : deux éléments Jellyfin portent le même identifiant TVDB
            json!({"Id": "s1", "Type": "Series", "Path": "/media/tv/S", "ProviderIds": {"Tvdb": "9", "Tmdb": "90"}}),
            json!({"Id": "s2", "Type": "Series", "Path": "/seedbox/tv/S", "ProviderIds": {"Tvdb": "9", "Tmdb": "90"}}),
            json!({"Id": "s3", "Type": "Series", "Path": "/media/tv/Seule", "ProviderIds": {"Tvdb": "10"}}),
            json!({"Id": "x", "Type": "Season", "Path": "/media/tv/S/Season 1"}),
        ];
        let ix = JfIndex::from_items(&items);
        let mut movie = title("A", true, 1, 1, 0);
        movie.jf_path = Some("/media/A".into());
        assert_eq!(
            ix.find(&movie),
            Some("aa11"),
            "dossier, identifiant normalisé"
        );
        // dossier inconnu mais identifiant unique : retrouvé
        movie.jf_path = Some("/media/renomme".into());
        movie.tmdb = 7;
        assert_eq!(ix.find(&movie), Some("bb"));
        movie.tmdb = 0;
        assert_eq!(ix.find(&movie), None, "ni dossier ni identifiant");
        // série : par dossier, jamais l'autre côté
        let mut s = title("S", false, 1, 1, 1);
        s.jf_path = Some("/seedbox/tv/S".into());
        assert_eq!(ix.find(&s), Some("s2"));
        // dossier inconnu et identifiant porté par deux éléments : on ne devine pas
        s.jf_path = Some("/media/renomme".into());
        s.tvdb = 9;
        s.tmdb = 90;
        assert_eq!(ix.find(&s), None, "identifiant ambigu");
        s.tvdb = 10;
        assert_eq!(ix.find(&s), Some("s3"));
    }

    #[test]
    fn russian_line_counts_available_and_read_episodes() {
        let mut ru = title("Russe", false, 20_000_000_000, 14, 60);
        ru.russian = true;
        let mut other = title("Autre", false, 10_000_000_000, 14, 10);
        other.russian = false;
        let mut seen = Seen::default();
        watch_episodes(&mut seen, &ru, 2);
        let r = report(&[ru, other], &seen, &Requests::unavailable(), &cfg());
        assert_eq!(
            r.russian,
            CatalogueRussian {
                series: 1,
                movies: 0,
                episodes: 60,
                episodes_seen: 2,
                movies_seen: 0,
                bytes: 20_000_000_000
            }
        );
        // arrivé depuis 14 jours : la ligne russe ne dépend pas du seuil d'âge, la liste si
        assert!(r.listed.is_empty());
    }

    #[test]
    fn kind_and_russian_come_from_the_folders() {
        let r = roots();
        // frontière de dossier : « /animex » n'est pas « /anime » ; « /anime-films » est un dossier d'animation à part
        assert_eq!(kind_of(true, "", "/anime-films/X (2020)", &r), ANIME);
        assert_eq!(kind_of(false, "", "/anime/X", &r), ANIME);
        assert_eq!(
            kind_of(false, "", "/animex/X", &r),
            SERIES,
            "pas le dossier /anime"
        );
        assert_eq!(kind_of(false, "anime", "/tv/X", &r), ANIME, "type Sonarr");
        assert_eq!(
            kind_of(true, "", "/home/kakaouette/media/Anime Movies/X", &r),
            ANIME
        );
        assert_eq!(kind_of(true, "", "/movies/X", &r), FILM);
        assert_eq!(kind_of(false, "standard", "/tv/X", &r), SERIES);
        let v = json!({"title": "Serie russe", "path": "/home/kakaouette/media/Russian/Serie russe",
            "seriesType": "standard", "added": "2026-09-26T10:00:00Z", "tvdbId": 5, "tmdbId": 6,
            "statistics": {"sizeOnDisk": 3_000_000_000_i64, "episodeFileCount": 60}});
        let t = series_title("seedbox", &v, &r, None).unwrap();
        assert!(t.russian);
        assert_eq!((t.kind, t.episodes, t.bytes), (SERIES, 60, 3_000_000_000));
        assert_eq!(t.arrived, parse_date("2026-09-26T10:00:00Z"));
        // dossier « Russian Movies » ≠ « Russian » : chacun est un dossier russe
        let m = json!({"title": "Film russe", "year": 2020, "path": "/home/kakaouette/media/Russian Movies/Film russe", "sizeOnDisk": 5});
        assert!(movie_title("seedbox", &m, &r, None).unwrap().russian);
        let not = json!({"title": "Film", "path": "/home/kakaouette/media/Russians/Film", "sizeOnDisk": 5});
        assert!(!movie_title("seedbox", &not, &r, None).unwrap().russian);
    }

    #[test]
    fn a_title_without_files_is_not_part_of_the_catalogue() {
        let r = roots();
        assert!(movie_title("vps", &json!({"title": "X", "sizeOnDisk": 0}), &r, None).is_none());
        assert!(series_title(
            "vps",
            &json!({"title": "X", "statistics": {"sizeOnDisk": 0}}),
            &r,
            None
        )
        .is_none());
        assert!(series_title("vps", &json!({"title": "X"}), &r, None).is_none());
    }

    #[test]
    fn a_movie_arrives_with_its_file_not_with_its_card() {
        let r = roots();
        // fiche ajoutée en avril, fichier importé hier (film demandé avant sa sortie)
        let v = json!({"title": "Sorti tard", "year": 2026, "path": "/movies/Sorti tard (2026)", "sizeOnDisk": 4,
            "added": "2026-04-30T21:49:26Z", "movieFile": {"dateAdded": "2026-10-07T08:00:00Z"}});
        let t = movie_title("vps", &v, &r, None).unwrap();
        assert_eq!(t.arrived, parse_date("2026-10-07T08:00:00Z"));
        assert_eq!(t.name, "Sorti tard (2026)");
        // date illisible des deux côtés : inconnue
        let bad = json!({"title": "X", "path": "/movies/X", "sizeOnDisk": 4, "added": "hier"});
        assert_eq!(movie_title("vps", &bad, &r, None).unwrap().arrived, None);
    }

    #[test]
    fn a_series_arrives_with_its_first_episodes_not_with_its_card() {
        let r = roots();
        // fiche ajoutée en avril, série diffusée depuis septembre (demandée avant sa diffusion) : arrivée = septembre
        let v = json!({"title": "Diffusée tard", "path": "/tv/Diffusee tard", "seriesType": "standard",
            "added": "2026-04-30T21:49:26Z", "firstAired": "2026-09-10T00:00:00Z",
            "statistics": {"sizeOnDisk": 4, "episodeFileCount": 2}});
        let t = series_title("seedbox", &v, &r, None).unwrap();
        assert_eq!(t.arrived, parse_date("2026-09-10T00:00:00Z"));
        // série ancienne ajoutée récemment : la fiche fait foi
        let old = json!({"title": "Ancienne", "path": "/tv/Ancienne", "added": "2026-09-01T10:00:00Z",
            "firstAired": "2005-03-26T00:00:00Z", "statistics": {"sizeOnDisk": 4, "episodeFileCount": 2}});
        assert_eq!(
            series_title("vps", &old, &r, None).unwrap().arrived,
            parse_date("2026-09-01T10:00:00Z")
        );
        // pas de première diffusion connue : la fiche seule
        let none = json!({"title": "Sans date", "path": "/tv/X", "added": "2026-09-01T10:00:00Z",
            "statistics": {"sizeOnDisk": 4, "episodeFileCount": 1}});
        assert_eq!(
            series_title("vps", &none, &r, None).unwrap().arrived,
            parse_date("2026-09-01T10:00:00Z")
        );
        // première diffusion lisible mais `added` illisible : la diffusion seule (jamais « inconnue » à tort)
        let half = json!({"title": "Y", "path": "/tv/Y", "added": "hier", "firstAired": "2026-09-10T00:00:00Z",
            "statistics": {"sizeOnDisk": 4, "episodeFileCount": 1}});
        assert_eq!(
            series_title("vps", &half, &r, None).unwrap().arrived,
            parse_date("2026-09-10T00:00:00Z")
        );
        // aucune des deux lisible : inconnue, le titre ne conclut jamais
        let bad = json!({"title": "Z", "path": "/tv/Z", "added": "hier", "firstAired": "bientôt",
            "statistics": {"sizeOnDisk": 4, "episodeFileCount": 1}});
        assert_eq!(series_title("vps", &bad, &r, None).unwrap().arrived, None);
    }

    #[test]
    fn titles_are_shortened_for_the_state() {
        let r = roots();
        let long = "é".repeat(200);
        let v = json!({"title": long, "path": "/movies/X", "sizeOnDisk": 1});
        assert_eq!(
            movie_title("vps", &v, &r, None)
                .unwrap()
                .name
                .chars()
                .count(),
            TITLE_MAX
        );
    }

    #[test]
    fn the_admin_who_reads_the_catalogue_sees_every_library() {
        let user = |id: &str, admin: bool, all: bool, disabled: bool| json!({"Id": id, "Policy": {"IsAdministrator": admin, "EnableAllFolders": all, "IsDisabled": disabled}});
        let users = [
            user("MEMBRE", false, true, false),
            user("ADMIN-LISTE", true, false, false),
            user("ADMIN-SUSPENDU", true, true, true),
            user("ADMIN-TOUT", true, true, false),
        ];
        assert_eq!(pick_admin(&users).as_deref(), Some("admintout"));
        // aucun admin « toutes bibliothèques » actif : le premier admin actif
        assert_eq!(pick_admin(&users[..3]).as_deref(), Some("adminliste"));
        assert_eq!(pick_admin(&users[..1]), None);
    }

    #[test]
    fn playback_reporting_keeps_plays_of_a_minute_or_more() {
        let cols = vec!["ItemId".to_string(), "secs".to_string()];
        let rows = vec![
            vec!["AB-CD".into(), "60".into()],
            vec!["ee".into(), "59.9".into()],
            vec!["ff".into(), "3600".into()],
            vec!["gg".into(), "n/a".into()],
        ];
        assert_eq!(long_plays(&cols, &rows).unwrap(), ["abcd", "ff"]);
        assert!(long_plays(&["x".to_string()], &rows).is_err());
    }

    #[test]
    fn only_unknown_playback_ids_are_resolved() {
        let mut seen = Seen::default();
        seen.add_item(&json!({"Id": "AAAA", "Type": "Movie"}));
        seen.add_item(&json!({"Id": "bbbb", "Type": "Episode", "SeriesId": "s"}));
        // un épisode sans série ne peut pas être rattaché : il reste « inconnu » et sera redemandé
        seen.add_item(&json!({"Id": "cccc", "Type": "Episode"}));
        seen.add_item(&json!({"Id": "dddd", "Type": "Season"}));
        let unknown = seen.unknown(&[
            "aaaa".into(),
            "BBBB".into(),
            "cccc".into(),
            "dddd".into(),
            "dddd".into(),
        ]);
        assert_eq!(unknown, ["cccc", "dddd"]);
    }

    #[test]
    fn sizes_read_in_french() {
        assert_eq!(human_size(22_100_000_000), "22,1 Go");
        assert_eq!(human_size(843_000_000_000), "843 Go");
        assert_eq!(human_size(1_590_000_000_000), "1,59 To");
        assert_eq!(human_size(48_000_000), "48 Mo");
        assert_eq!(pct_text(36.84), "36,8");
        assert_eq!(percent(1, 0), 0.0);
        assert_eq!(percent(1, 4), 25.0);
    }

    #[test]
    fn the_summary_says_what_was_found() {
        let titles = [
            title("Vieux", true, 5_000_000_000, 90, 0),
            title("Neuf", true, 5_000_000_000, 3, 0),
        ];
        let r = report(&titles, &Seen::default(), &Requests::unavailable(), &cfg());
        let s = summary(&r);
        assert!(
            s.starts_with("1 jamais vu(s) depuis 60 j ou plus (5,0 Go, 50,0 % du catalogue)"),
            "{s}"
        );
        assert!(s.contains("1 plus récent(s)"), "{s}");
    }

    #[test]
    fn the_report_survives_the_state_file_both_ways() {
        let titles = [title("Vieux", true, 5_000_000_000, 90, 0)];
        let r = report(&titles, &Seen::default(), &Requests::unavailable(), &cfg());
        let st = crate::state::State {
            catalogue: Some(r.clone()),
            ..Default::default()
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: crate::state::State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.catalogue, Some(r));
        // un état écrit avant ce rapport se relit, sans rapport
        let old: crate::state::State = serde_json::from_str("{}").unwrap();
        assert_eq!(old.catalogue, None);
    }
}
