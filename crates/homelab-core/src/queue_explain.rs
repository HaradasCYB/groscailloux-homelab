//! Avertissements de la file des Arrs, expliqués aux membres (2026-10-09).
//!
//! La page « Téléchargements » de Jellyfin Enhanced (visible de tous les membres) affiche la file de Sonarr et
//! Radarr ; un élément bloqué n'y portait qu'un badge « AVERTISSEMENT », sans rien dire (cas réel : Carrie S01E02,
//! REPACK refusée en « Not a Custom Format upgrade »). Ce module traduit l'état d'un élément de file en une note :
//! - `badge` : court (≤ 18 caractères, mis en majuscules par le CSS de Jellyfin Enhanced) ;
//! - `text` : une ou deux phrases pour un membre, qui ne promettent une action automatique que si homelabd (ou
//!   l'Arr) la fait vraiment ; sinon « l'admin doit… » ;
//! - `detail` : le message d'origine nettoyé (nom de fichier sans dossier, ni lien, ni hôte, ni clé), réservé à
//!   l'admin.
//!
//! Fonctions pures et testées ; la collecte (files des 4 Arrs, cache) est dans `homelabd::subs_api`.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

/// Arr d'où vient l'élément (Jellyfin Enhanced affiche l'icône correspondante sur la carte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Source {
    Sonarr,
    Radarr,
}

/// Un message d'état de la file (`statusMessages[]`) : `title` est en général le nom du fichier concerné ;
/// quand `messages` est vide, c'est `title` qui porte le message (« One or more episodes expected… »).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusMessage {
    pub title: String,
    pub messages: Vec<String>,
}

/// Ce que l'on sait d'un élément de file.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub source: Source,
    /// `status` : `downloading`, `completed`, `warning`, `failed`, `queued`, `paused`, `delay`,
    /// `downloadClientUnavailable`…
    pub status: &'a str,
    /// `trackedDownloadState` : `downloading`, `importPending`, `importBlocked`, `importing`, `imported`,
    /// `failedPending`, `failed`, `ignored`.
    pub tracked_state: &'a str,
    /// `trackedDownloadStatus` : `ok`, `warning`, `error`.
    pub tracked_status: &'a str,
    pub messages: &'a [StatusMessage],
    pub error_message: Option<&'a str>,
    /// L'épisode (ou le film) a-t-il déjà un fichier ? (`episodeHasFile`, `episode.hasFile`, `movie.hasFile`)
    pub has_file: Option<bool>,
}

/// Ce que homelabd fait réellement de lui-même (lu dans la configuration en vigueur) : la note ne promet une
/// action automatique que si elle est là.
#[derive(Debug, Clone, Copy, Default)]
pub struct Automation<'a> {
    /// `id_match_import` active : intervalle de passage en minutes.
    pub id_match_every_mins: Option<u64>,
    /// `stuck_handler` actif : son motif (`pattern`, insensible à la casse) et son délai en heures.
    pub stuck: Option<(&'a Regex, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Le nom de la release ne correspond pas à la fiche, mais l'historique du grab désigne la bonne
    /// (« matched to … by ID ») : `id_match_import`.
    IdMatch,
    /// Pas une amélioration du fichier déjà en place (qualité, révision, formats personnalisés), ou déjà importé.
    AlreadyAvailable,
    /// Titre d'épisode encore « TBA ».
    TitlePending,
    /// Aucun fichier vidéo utilisable (vide, extrait, archive, extension).
    NoVideo,
    /// Épisode inattendu, nom illisible, série ou film inconnu.
    Unrecognized,
    /// Plus de place pour ranger le fichier.
    DiskFull,
    /// Fichier verrouillé ou accès refusé.
    Locked,
    /// Aucune source (torrent à l'arrêt ou métadonnées introuvables).
    Stalled,
    /// Client de téléchargement injoignable.
    ClientDown,
    /// Téléchargement en échec.
    Failed,
    /// Pack où il manque des épisodes (ou qui n'ont pas pu être importés).
    PackIncomplete,
    /// Tout autre avertissement.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    pub family: Family,
    pub badge: &'static str,
    pub text: String,
    /// Message d'origine nettoyé (`clean_detail`), vide s'il n'y en a pas. Réservé à l'admin.
    pub detail: String,
}

/// `statusMessages` d'un élément brut de file.
pub fn status_messages(record: &Value) -> Vec<StatusMessage> {
    record
        .get("statusMessages")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| StatusMessage {
                    title: m
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    messages: m
                        .get("messages")
                        .and_then(Value::as_array)
                        .map(|ms| {
                            ms.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Les raisons proprement dites (le titre d'un message n'en est une que si le message n'a pas de lignes), en
/// minuscules, plus `errorMessage`.
fn reasons(f: &Facts) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in f.messages {
        if m.messages.is_empty() {
            if !m.title.trim().is_empty() {
                out.push(m.title.to_lowercase());
            }
        } else {
            out.extend(m.messages.iter().map(|s| s.to_lowercase()));
        }
    }
    if let Some(e) = f.error_message.filter(|e| !e.trim().is_empty()) {
        out.push(e.to_lowercase());
    }
    out
}

const PACK_HEADER: &str = "one or more episodes expected";

fn any(reasons: &[String], needles: &[&str]) -> bool {
    reasons
        .iter()
        .any(|r| needles.iter().any(|n| r.contains(n)))
}

const ID_MATCH: &[&str] = &["was matched to series by id", "was matched to movie by id"];
const NOT_UPGRADE: &[&str] = &[
    "not an upgrade for existing",
    "not a custom format upgrade",
    "not a quality upgrade",
    "not a revision upgrade",
    "not a quality revision upgrade",
    "existing file on disk is of equal or higher",
    "existing file on disk has a equal or higher",
    "existing file on disk has an equal or higher",
    "file already imported at",
];
const TBA: &[&str] = &["has a tba title", "does not have a title"];
const NO_VIDEO: &[&str] = &[
    "no files found are eligible for import",
    "no video files",
    "found archive file",
    "unsupported extension",
    "invalid video file",
    "is a sample",
    "unable to determine if file is a sample",
];
const UNRECOGNIZED: &[&str] = &[
    "unknown series",
    "unknown movie",
    "unknown episode",
    "unable to parse",
    "unable to identify",
    "title mismatch",
    "was unexpected",
    "invalid season or episode",
    "couldn't find similar",
    "could not find similar",
    "not found in the grabbed release",
    "single episode file contains all episodes",
    "episode does not match",
    "doesn't match",
    "does not match",
];
const DISK_FULL: &[&str] = &[
    "not enough free space",
    "not enough disk space",
    "no space left",
    "disk is full",
    "insufficient space",
    "insufficient free space",
];
const LOCKED: &[&str] = &[
    "access to the path",
    "access denied",
    "is denied",
    "permission denied",
    "being used by another process",
    "read-only file system",
    "unauthorizedaccess",
    "file is locked",
];
const STALLED: &[&str] = &["stalled", "no connections", "downloading metadata"];

fn has_file_word(source: Source) -> &'static str {
    match source {
        Source::Sonarr => "Cet épisode",
        Source::Radarr => "Ce film",
    }
}

/// Explication d'un élément de file, ou `None` pour un élément ordinaire (téléchargement qui avance, import normal
/// en attente, file d'attente, pause).
pub fn explain(f: &Facts, auto: &Automation) -> Option<Note> {
    let r = reasons(f);
    let detail = clean_detail(&detail_of(f));
    let note = |family: Family, badge: &'static str, text: String| {
        Some(Note {
            family,
            badge,
            text,
            detail: detail.clone(),
        })
    };
    let state = f.tracked_state.to_ascii_lowercase();
    let status = f.status.to_ascii_lowercase();
    let tstatus = f.tracked_status.to_ascii_lowercase();

    if state == "failedpending" || state == "failed" || status == "failed" {
        return note(
            Family::Failed,
            "Échec",
            "Le téléchargement a échoué. L'admin doit le retirer pour qu'une autre version soit cherchée."
                .into(),
        );
    }
    if any(&r, ID_MATCH) {
        return match auto.id_match_every_mins {
            Some(m) => note(
                Family::IdMatch,
                "Import imminent",
                format!(
                    "Le nom de cette version ne correspond pas tout à fait à la fiche, mais c'est bien le bon titre : \
                     l'import est relancé automatiquement (toutes les {m} min). Si ça dure, l'admin doit l'importer \
                     à la main."
                ),
            ),
            None => note(
                Family::IdMatch,
                "Import manuel",
                "Le nom de cette version ne correspond pas tout à fait à la fiche : l'admin doit l'importer à la \
                 main."
                    .into(),
            ),
        };
    }
    // « déjà disponible » seulement si l'épisode ou le film a vraiment un fichier (dans un pack, l'épisode de
    // CETTE ligne peut manquer alors qu'un autre fichier du pack n'était pas une amélioration)
    if any(&r, NOT_UPGRADE) && f.has_file != Some(false) {
        return note(
            Family::AlreadyAvailable,
            "Déjà disponible",
            format!(
                "{} est déjà disponible dans une version au moins aussi bonne : ce téléchargement n'est pas importé. \
                 Tu peux regarder la version en place ; l'admin doit retirer ce téléchargement de la file.",
                has_file_word(f.source)
            ),
        );
    }
    if any(&r, TBA) {
        return note(
            Family::TitlePending,
            "Titre attendu",
            "Le titre de cet épisode n'est pas encore publié. L'import est retenté régulièrement et devrait se faire \
             une fois le titre connu ; sinon l'admin doit l'importer à la main."
                .into(),
        );
    }
    if any(&r, NO_VIDEO) || r.iter().any(|x| x.trim() == "sample") {
        return note(
            Family::NoVideo,
            "Pas de vidéo",
            "Ce téléchargement ne contient aucun fichier vidéo utilisable (vide, simple extrait ou archive). L'admin \
             doit le retirer pour qu'une autre version soit cherchée."
                .into(),
        );
    }
    if any(&r, UNRECOGNIZED) {
        return note(
            Family::Unrecognized,
            "Non reconnu",
            "Le fichier reçu ne correspond pas à ce qui était attendu (autre épisode, autre titre ou nom illisible) : \
             il n'est pas importé. L'admin doit l'importer à la main ou le retirer."
                .into(),
        );
    }
    if any(&r, DISK_FULL) {
        return note(
            Family::DiskFull,
            "Disque plein",
            "Plus assez de place pour ranger ce fichier. L'admin doit libérer de l'espace pour que l'import se fasse."
                .into(),
        );
    }
    if any(&r, LOCKED) {
        return note(
            Family::Locked,
            "Fichier bloqué",
            "Le fichier n'a pas pu être rangé (fichier occupé ou accès refusé). L'admin doit vérifier.".into(),
        );
    }
    if any(&r, STALLED) {
        // promesse seulement si `stuck_handler` reconnaît CE message (même motif, même champ)
        let auto_replace = auto
            .stuck
            .filter(|(re, _)| f.error_message.is_some_and(|e| re.is_match(e)));
        let text = match auto_replace {
            Some((_, hours)) => format!(
                "Aucune source ne partage ce fichier pour l'instant : le téléchargement est à l'arrêt. S'il l'est \
                 encore au bout de {hours} h, il est retiré automatiquement et une autre version sera cherchée."
            ),
            None => "Aucune source ne partage ce fichier pour l'instant : le téléchargement est à l'arrêt. Si rien \
                     ne bouge, l'admin doit le retirer pour qu'une autre version soit cherchée."
                .into(),
        };
        return note(Family::Stalled, "Sans source", text);
    }
    if status == "downloadclientunavailable" {
        return note(
            Family::ClientDown,
            "Hors ligne",
            "Le serveur de téléchargement ne répond pas en ce moment. Le téléchargement reprendra quand il sera de \
             retour."
                .into(),
        );
    }
    if f.source == Source::Sonarr && any(&r, &[PACK_HEADER]) {
        return note(
            Family::PackIncomplete,
            "Pack incomplet",
            "Il manque des épisodes dans ce pack, ou certains n'ont pas pu être importés. Les épisodes déjà importés \
             restent disponibles ; l'admin doit vérifier le reste."
                .into(),
        );
    }
    let flagged = tstatus == "warning"
        || tstatus == "error"
        || status == "warning"
        || state == "importblocked";
    if flagged {
        return note(
            Family::Other,
            "À vérifier",
            "Ce téléchargement est bloqué pour une raison que l'admin doit examiner.".into(),
        );
    }
    None
}

/// Message d'origine assemblé : « fichier : raison » pour chaque raison, puis `errorMessage`, sans doublon.
fn detail_of(f: &Facts) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |s: String| {
        let s = s.trim().to_string();
        if !s.is_empty() && !parts.contains(&s) {
            parts.push(s);
        }
    };
    for m in f.messages {
        if m.messages.is_empty() {
            push(m.title.clone());
        } else {
            for x in &m.messages {
                if m.title.trim().is_empty() {
                    push(x.clone());
                } else {
                    push(format!("{} : {x}", m.title.trim()));
                }
            }
        }
    }
    if let Some(e) = f.error_message {
        push(e.to_string());
    }
    parts.join(" · ")
}

/// Longueur maximale de `detail` (caractères).
const DETAIL_MAX: usize = 600;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("motif valide"))
}

/// Message d'origine rendu montrable (dépôt public, page vue par l'admin sur l'adresse de Jellyfin) : liens
/// retirés, chemins réduits au nom du fichier (donc aucun `/home/<compte>`), adresses IP, noms d'hôte, paramètres
/// secrets et longues chaînes hexadécimales masqués, espaces resserrés, longueur bornée.
pub fn clean_detail(s: &str) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    static SECRET_PARAM: OnceLock<Regex> = OnceLock::new();
    static HOME: OnceLock<Regex> = OnceLock::new();
    static IPV4: OnceLock<Regex> = OnceLock::new();
    static HOST_PORT: OnceLock<Regex> = OnceLock::new();
    static DOMAIN: OnceLock<Regex> = OnceLock::new();
    static HEX: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();

    let t = re(&URL, r"(?i)\b(?:[a-z][a-z0-9+.-]*://|magnet:\?)\S+").replace_all(s, "[lien]");
    let t = re(
        &SECRET_PARAM,
        r"(?i)\b(api_?key|apitoken|token|passkey|password|passwd|secret|auth)\s*[=:]\s*[^\s&,;'\x22]+",
    )
    .replace_all(&t, "$1=[masqué]");
    let t = shorten_paths(&t);
    let t = re(&HOME, r"(?i)\bhome/[^/\s'\x22\])]+").replace_all(&t, "home/…");
    let t = re(&IPV4, r"\b\d{1,3}(?:\.\d{1,3}){3}(?::\d{1,5})?\b").replace_all(&t, "[adresse]");
    let t = re(&HOST_PORT, r"\b[A-Za-z][A-Za-z0-9.-]*:\d{2,5}\b").replace_all(&t, "[hôte]");
    let t = re(
        &DOMAIN,
        r"\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|net|org|io|me|tw|fr|eu|xyz|cc|info|lan|local|internal|invalid|home|arpa)\b",
    )
    .replace_all(&t, "[hôte]");
    let t = re(&HEX, r"\b[0-9A-Fa-f]{24,}\b").replace_all(&t, "[masqué]");
    let t = re(&SPACES, r"\s+").replace_all(&t, " ");
    let t = t.trim();
    if t.chars().count() > DETAIL_MAX {
        let cut: String = t.chars().take(DETAIL_MAX - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        t.to_string()
    }
}

/// Chemins absolus réduits au dernier élément (nom de fichier ou de dossier). Un chemin commence par `/` ou `~/`
/// en début de texte ou après un espace, un guillemet, une parenthèse, `:`, `=`, `,` ou `;`, et doit être suivi
/// d'un caractère visible (« VF / French » n'est pas un chemin). Entre guillemets, il va jusqu'au guillemet
/// fermant ; sinon jusqu'à une parenthèse, un crochet, un guillemet, « , », « ; », un saut de ligne ou la fin.
fn shorten_paths(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        let prev = if i == 0 { None } else { Some(c[i - 1]) };
        let lead = match (c[i], c.get(i + 1)) {
            ('/', _) => 1,
            ('~', Some('/')) => 2,
            _ => 0,
        };
        let opens = prev.is_none_or(|p| p.is_whitespace() || "'\"([{<:=,;".contains(p));
        let visible_next = c
            .get(i + lead)
            .is_some_and(|n| !n.is_whitespace() && *n != '/');
        if lead == 0 || !opens || !visible_next {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let quote = prev.filter(|p| *p == '\'' || *p == '"');
        let mut j = i;
        while j < c.len() {
            let ch = c[j];
            let stop = match quote {
                Some(q) => ch == q || ch == '\n',
                None => {
                    "'\"()[]{}<>\n".contains(ch)
                        || ((ch == ',' || ch == ';')
                            && c.get(j + 1).is_none_or(|n| n.is_whitespace()))
                }
            };
            if stop {
                break;
            }
            j += 1;
        }
        let span: String = c[i..j].iter().collect();
        let body = span.trim_end_matches(|ch: char| ch.is_whitespace() || ch == '.' || ch == ':');
        let tail = &span[body.len()..];
        let name = body
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim();
        out.push_str(if name.is_empty() || name == "~" {
            "…"
        } else {
            name
        });
        out.push_str(tail);
        i = j;
    }
    out
}

/// Clé d'appariement d'un titre : minuscules, lettres accentuées ramenées à leur base, seulement lettres et
/// chiffres, espaces resserrés (la même que `dlNorm` du script de Mon compte).
pub fn title_key(s: &str) -> String {
    let spaced: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(crate::matching::fold)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// « S01E02 » (deux chiffres au moins, comme Jellyfin Enhanced).
pub fn episode_code(season: i64, episode: i64) -> String {
    format!("S{season:02}E{episode:02}")
}

/// Réglages en vigueur qui décident des promesses (`Automation`), lus dans la configuration.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    id_match_every_mins: Option<u64>,
    stuck: Option<(Regex, u64)>,
}

impl Setup {
    /// `id_match_import` et `stuck_handler` actifs (`[tasks] disabled`), leur rythme et le motif de `stuck_handler`
    /// tel que la tâche le compile (`(?i)` + `pattern`).
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        let id_match_every_mins = cfg
            .task_enabled("id_match_import")
            .then(|| cfg.tasks.id_match_import.interval_secs.div_ceil(60).max(1));
        let sh = &cfg.tasks.stuck_handler;
        let stuck = cfg
            .task_enabled("stuck_handler")
            .then(|| Regex::new(&format!("(?i){}", sh.pattern)).ok())
            .flatten()
            .map(|re| (re, (sh.stall_secs.max(0) as u64).div_ceil(3600).max(1)));
        Self {
            id_match_every_mins,
            stuck,
        }
    }

    pub fn automation(&self) -> Automation<'_> {
        Automation {
            id_match_every_mins: self.id_match_every_mins,
            stuck: self.stuck.as_ref().map(|(re, h)| (re, *h)),
        }
    }
}

/// Élément de file expliqué, tel que la route `/compte/api/downloads` le renvoie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Explained {
    pub source: Source,
    /// Titre de la série ou du film (celui que Jellyfin Enhanced affiche sur la carte).
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    /// « S01E02 » pour un épisode identifié.
    pub episode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    pub badge: &'static str,
    pub text: String,
    /// Réservé à l'admin : la route le retire pour les autres comptes.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

/// Titre, année, épisode d'un élément brut de file (lu avec `queue_records_detailed`).
fn identity(source: Source, r: &Value) -> (String, Option<i64>, Option<String>, Option<String>) {
    let (obj, title_ptr) = match source {
        Source::Sonarr => ("/series", "/series/title"),
        Source::Radarr => ("/movie", "/movie/title"),
    };
    let mut title = str_at(r, title_ptr).trim().to_string();
    if title.is_empty() {
        title = str_at(r, "/title").trim().to_string();
    }
    let year = r
        .pointer(&format!("{obj}/year"))
        .and_then(Value::as_i64)
        .filter(|y| *y > 0);
    let (episode, episode_title) = match source {
        Source::Radarr => (None, None),
        Source::Sonarr => {
            let s = r.pointer("/episode/seasonNumber").and_then(Value::as_i64);
            let e = r.pointer("/episode/episodeNumber").and_then(Value::as_i64);
            match (s, e) {
                (Some(s), Some(e)) => {
                    let t = str_at(r, "/episode/title").trim();
                    (
                        Some(episode_code(s, e)),
                        (!t.is_empty()).then(|| t.to_string()),
                    )
                }
                _ => (None, None),
            }
        }
    };
    (title, year, episode, episode_title)
}

/// Note d'un élément brut de file.
pub fn explain_record(source: Source, r: &Value, auto: &Automation) -> Option<Note> {
    let messages = status_messages(r);
    let has_file = match source {
        Source::Sonarr => r
            .get("episodeHasFile")
            .and_then(Value::as_bool)
            .or_else(|| r.pointer("/episode/hasFile").and_then(Value::as_bool)),
        Source::Radarr => r.pointer("/movie/hasFile").and_then(Value::as_bool),
    };
    let f = Facts {
        source,
        status: str_at(r, "/status"),
        tracked_state: str_at(r, "/trackedDownloadState"),
        tracked_status: str_at(r, "/trackedDownloadStatus"),
        messages: &messages,
        error_message: r.get("errorMessage").and_then(Value::as_str),
        has_file,
    };
    explain(&f, auto)
}

/// Les éléments expliqués de toutes les files lues. **Jamais d'appariement ambigu** : quand plusieurs éléments
/// (expliqués ou non, de n'importe quel côté) ont la même clé — source, titre normalisé, épisode — la carte de
/// Jellyfin Enhanced ne permet pas de savoir lequel elle montre ; ils ne sont gardés (une fois) que si tous portent
/// la même note, sinon aucun.
pub fn explain_queue(records: &[(Source, Value)], auto: &Automation) -> Vec<Explained> {
    use std::collections::HashMap;
    type Key = (Source, String, Option<String>);
    let mut groups: HashMap<Key, Vec<Option<Explained>>> = HashMap::new();
    let mut order: Vec<Key> = Vec::new();
    for (source, r) in records {
        let (title, year, episode, episode_title) = identity(*source, r);
        let key: Key = (*source, title_key(&title), episode.clone());
        let item = explain_record(*source, r, auto).map(|n| Explained {
            source: *source,
            title,
            year,
            episode,
            episode_title,
            badge: n.badge,
            text: n.text,
            detail: n.detail,
        });
        let g = groups.entry(key.clone()).or_default();
        if g.is_empty() {
            order.push(key);
        }
        g.push(item);
    }
    let mut out = Vec::new();
    for key in order {
        let g = &groups[&key];
        if key.1.is_empty() {
            continue;
        }
        let Some(Some(first)) = g.first() else {
            continue;
        };
        let same = g.iter().all(|x| {
            x.as_ref()
                .is_some_and(|x| x.badge == first.badge && x.text == first.text)
        });
        if same {
            let mut kept = first.clone();
            if g.len() > 1 {
                // plusieurs téléchargements derrière la même carte : le détail de l'un ne vaut pas pour l'autre
                kept.detail.clear();
            }
            out.push(kept);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(title: &str, ms: &[&str]) -> StatusMessage {
        StatusMessage {
            title: title.into(),
            messages: ms.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn facts<'a>(
        source: Source,
        status: &'a str,
        state: &'a str,
        tstatus: &'a str,
        messages: &'a [StatusMessage],
        error: Option<&'a str>,
    ) -> Facts<'a> {
        Facts {
            source,
            status,
            tracked_state: state,
            tracked_status: tstatus,
            messages,
            error_message: error,
            has_file: None,
        }
    }

    fn stuck_re() -> Regex {
        Regex::new("(?i)stalled|metadata|no connections").unwrap()
    }

    fn auto(re: &Regex) -> Automation<'_> {
        Automation {
            id_match_every_mins: Some(5),
            stuck: Some((re, 8)),
        }
    }

    fn import_blocked(source: Source, ms: &[StatusMessage]) -> Option<Note> {
        let re = stuck_re();
        explain(
            &facts(source, "completed", "importBlocked", "warning", ms, None),
            &auto(&re),
        )
    }

    const CARRIE: &str = "Not a Custom Format upgrade for existing episode file(s). New: [FRENCH, HEVC 8-bit, MULTi, VF / French] (3700) do not improve on Existing: [FRENCH, HEVC 8-bit, MULTi, VF / French, WEB-DL] (4100)";

    #[test]
    fn ordinary_items_have_no_note() {
        let re = stuck_re();
        let a = auto(&re);
        for (status, state, ts) in [
            ("downloading", "downloading", "ok"),
            ("completed", "importPending", "ok"),
            ("completed", "importing", "ok"),
            ("queued", "downloading", "ok"),
            ("paused", "downloading", "ok"),
            ("delay", "downloading", "ok"),
            ("", "", ""),
        ] {
            for s in [Source::Sonarr, Source::Radarr] {
                assert_eq!(
                    explain(&facts(s, status, state, ts, &[], None), &a),
                    None,
                    "{status}/{state}/{ts}"
                );
            }
        }
    }

    #[test]
    fn the_carrie_case_is_already_available_with_the_message_kept_for_the_admin() {
        // forme décrite le 09/10 (importPending + warning) et forme de Sonarr v4 (en-tête « pack » + fichier)
        let one = [msg(
            "Carrie.S01E02.REPACK.FRENCH.1080p.WEB.H265-GRP.mkv",
            &[CARRIE],
        )];
        let re = stuck_re();
        let n = explain(
            &facts(
                Source::Sonarr,
                "completed",
                "importPending",
                "warning",
                &one,
                None,
            ),
            &auto(&re),
        )
        .unwrap();
        assert_eq!(n.family, Family::AlreadyAvailable);
        assert_eq!(n.badge, "Déjà disponible");
        assert!(n.text.starts_with("Cet épisode est déjà disponible"));
        assert!(n.text.contains("l'admin doit retirer"));
        assert!(n.detail.contains("VF / French"), "{}", n.detail);
        assert!(n.detail.contains("(3700)") && n.detail.contains("(4100)"));
        assert!(n.detail.starts_with("Carrie.S01E02.REPACK"));

        let v4 = [
            msg(
                "One or more episodes expected in this release were not imported or missing from the release",
                &[],
            ),
            msg("Carrie.S01E02.REPACK.FRENCH.1080p.WEB.H265-GRP.mkv", &[CARRIE]),
        ];
        let n = import_blocked(Source::Sonarr, &v4).unwrap();
        assert_eq!(n.family, Family::AlreadyAvailable);
    }

    #[test]
    fn every_upgrade_refusal_of_sonarr_and_radarr_is_already_available() {
        for (s, m) in [
            (Source::Sonarr, "Not an upgrade for existing episode file(s). Existing quality: WEBDL-1080p. New Quality HDTV-1080p."),
            (Source::Sonarr, "Not a quality revision upgrade for existing episode file(s)"),
            (Source::Sonarr, "Not a revision upgrade for existing episode file(s)"),
            (Source::Sonarr, "Not a quality upgrade for existing episode file(s)"),
            (Source::Sonarr, "Episode file already imported at 10/08/2026 21:14:03"),
            (Source::Radarr, "Not an upgrade for existing movie file. Existing quality: Bluray-1080p. New Quality WEBDL-1080p."),
            (Source::Radarr, "Not a Custom Format upgrade for existing movie file(s). New: [VFF] (1000) do not improve on Existing: [VFF, WEB-DL] (1400)"),
            (Source::Radarr, "Existing file on disk is of equal or higher preference: Bluray-1080p"),
        ] {
            let n = import_blocked(s, &[msg("x.mkv", &[m])]).unwrap();
            assert_eq!(n.family, Family::AlreadyAvailable, "{m}");
            let start = if s == Source::Radarr { "Ce film" } else { "Cet épisode" };
            assert!(n.text.starts_with(start), "{m}");
        }
    }

    #[test]
    fn already_available_is_never_claimed_for_an_episode_without_file() {
        let ms = [
            msg(
                "One or more episodes expected in this release were not imported or missing from the release",
                &[],
            ),
            msg("Show.S01E03.mkv", &["Not an upgrade for existing episode file(s). Existing quality: WEBDL-1080p. New Quality WEBDL-1080p."]),
        ];
        let re = stuck_re();
        let mut f = facts(
            Source::Sonarr,
            "completed",
            "importBlocked",
            "warning",
            &ms,
            None,
        );
        f.has_file = Some(false);
        let n = explain(&f, &auto(&re)).unwrap();
        assert_eq!(n.family, Family::PackIncomplete);
        f.has_file = Some(true);
        assert_eq!(
            explain(&f, &auto(&re)).unwrap().family,
            Family::AlreadyAvailable
        );
    }

    #[test]
    fn id_match_promises_the_automatic_import_only_when_the_task_runs() {
        let son = [msg("Le.Titre.Francais.S01E01.mkv", &["Found matching series via grab history, but release was matched to series by ID. Automatic import is not possible. See the FAQ for details."])];
        let rad = [msg("Le.Titre.Francais.2024.mkv", &["Found matching movie via grab history, but release was matched to movie by ID. Manual Import required."])];
        for (s, ms) in [(Source::Sonarr, &son[..]), (Source::Radarr, &rad[..])] {
            let n = import_blocked(s, ms).unwrap();
            assert_eq!(n.family, Family::IdMatch);
            assert_eq!(n.badge, "Import imminent");
            assert!(n.text.contains("automatiquement (toutes les 5 min)"));
            let off = explain(
                &facts(s, "completed", "importBlocked", "warning", ms, None),
                &Automation::default(),
            )
            .unwrap();
            assert_eq!(off.badge, "Import manuel");
            assert!(!off.text.contains("automatiquement"));
            assert!(off.text.contains("l'admin doit"));
        }
    }

    #[test]
    fn tba_title() {
        let n = import_blocked(
            Source::Sonarr,
            &[msg(
                "Show - 1050.mkv",
                &["Episode has a TBA title and recently aired"],
            )],
        )
        .unwrap();
        assert_eq!(n.family, Family::TitlePending);
        assert_eq!(n.badge, "Titre attendu");
    }

    #[test]
    fn no_video_sample_archive() {
        for ms in [
            vec![msg(
                "No files found are eligible for import in /home/someone/downloads/Show.S01E01",
                &[],
            )],
            vec![msg("show.s01e01.sample.mkv", &["Sample"])],
            vec![msg(
                "show.s01e01.rar",
                &["Found archive file, might need to be extracted"],
            )],
            vec![msg(
                "show.s01e01.exe",
                &["Invalid video file, unsupported extension: '.exe'"],
            )],
            vec![msg(
                "movie.mkv",
                &["Unable to determine if file is a sample"],
            )],
        ] {
            for s in [Source::Sonarr, Source::Radarr] {
                let n = import_blocked(s, &ms).unwrap();
                assert_eq!(n.family, Family::NoVideo, "{ms:?}");
                assert_eq!(n.badge, "Pas de vidéo");
            }
        }
        let n = import_blocked(
            Source::Sonarr,
            &[msg(
                "No files found are eligible for import in /home/someone/downloads/Show.S01E01",
                &[],
            )],
        )
        .unwrap();
        assert_eq!(
            n.detail,
            "No files found are eligible for import in Show.S01E01"
        );
    }

    #[test]
    fn unexpected_or_unknown() {
        for (s, m) in [
            (Source::Sonarr, "Episode 5 was unexpected considering the Show.S01E04 folder name"),
            (Source::Sonarr, "Unable to parse file"),
            (Source::Sonarr, "Unknown Series"),
            (Source::Sonarr, "Invalid season or episode"),
            (Source::Sonarr, "Unable to identify correct episode(s) using release name and scene mappings"),
            (Source::Sonarr, "Series title mismatch, automatic import is not possible"),
            (Source::Sonarr, "Single episode file contains all episodes in seasons. Review file name or manually import"),
            (Source::Radarr, "Unknown Movie"),
            (Source::Radarr, "Movie title mismatch, automatic import is not possible"),
        ] {
            let n = import_blocked(s, &[msg("x.mkv", &[m])]).unwrap();
            assert_eq!(n.family, Family::Unrecognized, "{m}");
            assert_eq!(n.badge, "Non reconnu");
        }
    }

    #[test]
    fn incomplete_pack_alone() {
        let ms = [msg(
            "One or more episodes expected in this release were not imported or missing from the release",
            &[],
        )];
        let n = import_blocked(Source::Sonarr, &ms).unwrap();
        assert_eq!(n.family, Family::PackIncomplete);
        assert_eq!(n.badge, "Pack incomplet");
        assert!(n
            .text
            .contains("Les épisodes déjà importés restent disponibles"));
    }

    #[test]
    fn disk_and_locks() {
        let n = import_blocked(
            Source::Radarr,
            &[msg(
                "movie.mkv",
                &["Not enough free space to import: 1.2 GB required"],
            )],
        )
        .unwrap();
        assert_eq!(n.family, Family::DiskFull);
        for m in [
            "Access to the path '/home/someone/media/Movies/Film (2024)/film.mkv' is denied.",
            "The process cannot access the file '/data/x/film.mkv' because it is being used by another process.",
            "Permission denied",
        ] {
            let n = import_blocked(Source::Radarr, &[msg("film.mkv", &[m])]).unwrap();
            assert_eq!(n.family, Family::Locked, "{m}");
            assert_eq!(n.badge, "Fichier bloqué");
            assert!(!n.detail.contains("home"), "{}", n.detail);
            assert!(!n.detail.contains("/data"), "{}", n.detail);
        }
    }

    #[test]
    fn stalled_promises_replacement_only_when_stuck_handler_matches() {
        let re = stuck_re();
        for e in [
            "The download is stalled with no connections",
            "qBittorrent is downloading metadata",
        ] {
            let f = facts(
                Source::Sonarr,
                "warning",
                "downloading",
                "warning",
                &[],
                Some(e),
            );
            let n = explain(&f, &auto(&re)).unwrap();
            assert_eq!(n.family, Family::Stalled, "{e}");
            assert_eq!(n.badge, "Sans source");
            assert!(n
                .text
                .contains("au bout de 8 h, il est retiré automatiquement"));
            let off = explain(&f, &Automation::default()).unwrap();
            assert!(!off.text.contains("automatiquement"));
            assert!(off.text.contains("l'admin doit"));
        }
        // motif de stuck_handler qui ne reconnaît pas ce message : pas de promesse
        let narrow = Regex::new("(?i)no connections").unwrap();
        let f = facts(
            Source::Radarr,
            "warning",
            "downloading",
            "warning",
            &[],
            Some("qBittorrent is downloading metadata"),
        );
        let n = explain(
            &f,
            &Automation {
                id_match_every_mins: None,
                stuck: Some((&narrow, 8)),
            },
        )
        .unwrap();
        assert!(!n.text.contains("automatiquement"));
    }

    #[test]
    fn failed_and_client_down() {
        let re = stuck_re();
        for (status, state) in [
            ("completed", "failedPending"),
            ("failed", "downloading"),
            ("completed", "failed"),
        ] {
            let n = explain(
                &facts(
                    Source::Radarr,
                    status,
                    state,
                    "error",
                    &[],
                    Some("qBittorrent is reporting an error"),
                ),
                &auto(&re),
            )
            .unwrap();
            assert_eq!(n.family, Family::Failed, "{status}/{state}");
            assert_eq!(n.badge, "Échec");
        }
        let n = explain(
            &facts(
                Source::Sonarr,
                "downloadClientUnavailable",
                "downloading",
                "warning",
                &[],
                None,
            ),
            &auto(&re),
        )
        .unwrap();
        assert_eq!(n.family, Family::ClientDown);
    }

    #[test]
    fn anything_else_flagged_is_to_check() {
        let n = import_blocked(
            Source::Sonarr,
            &[msg("x.mkv", &["Something nobody has seen before"])],
        )
        .unwrap();
        assert_eq!(n.family, Family::Other);
        assert_eq!(n.badge, "À vérifier");
        let re = stuck_re();
        let n = explain(
            &facts(Source::Radarr, "warning", "downloading", "ok", &[], None),
            &auto(&re),
        )
        .unwrap();
        assert_eq!(n.family, Family::Other);
        assert_eq!(n.detail, "");
    }

    #[test]
    fn badges_are_short_and_texts_never_carry_raw_data() {
        let re = stuck_re();
        let samples: Vec<Vec<StatusMessage>> = vec![
            vec![msg("a.mkv", &[CARRIE])],
            vec![msg("a.mkv", &["Episode has a TBA title and recently aired"])],
            vec![msg("a.mkv", &["Unknown Series"])],
            vec![msg("a.mkv", &["Not enough free space"])],
            vec![msg("a.mkv", &["Permission denied"])],
            vec![msg("One or more episodes expected in this release were not imported or missing from the release", &[])],
            vec![msg("a.mkv", &["Found matching series via grab history, but release was matched to series by ID."])],
            vec![msg("a.mkv", &["???"])],
        ];
        for ms in &samples {
            for a in [auto(&re), Automation::default()] {
                let n = import_blocked_with(ms, &a);
                assert!(n.badge.chars().count() <= 18, "{}", n.badge);
                assert!(
                    !n.text.contains("a.mkv") && !n.text.contains('/'),
                    "{}",
                    n.text
                );
                assert!(n.text.chars().count() <= 260, "{}", n.text);
            }
        }
        let e = "The download is stalled with no connections";
        let n = explain(
            &facts(
                Source::Sonarr,
                "warning",
                "downloading",
                "warning",
                &[],
                Some(e),
            ),
            &auto(&re),
        )
        .unwrap();
        assert!(n.badge.chars().count() <= 18);
    }

    fn import_blocked_with(ms: &[StatusMessage], a: &Automation) -> Note {
        explain(
            &facts(
                Source::Sonarr,
                "completed",
                "importBlocked",
                "warning",
                ms,
                None,
            ),
            a,
        )
        .unwrap()
    }

    #[test]
    fn status_messages_are_read_from_the_raw_record() {
        let r = serde_json::json!({"statusMessages": [
            {"title": "One or more episodes expected in this release were not imported or missing from the release", "messages": []},
            {"title": "a.mkv", "messages": ["Sample", 3]}
        ]});
        let m = status_messages(&r);
        assert_eq!(m.len(), 2);
        assert!(m[0].messages.is_empty());
        assert_eq!(m[1].messages, vec!["Sample".to_string()]);
        assert!(status_messages(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn detail_cleaning() {
        // chemins : nom seul, jamais le compte ni le dossier
        assert_eq!(
            clean_detail("No files found are eligible for import in /home/someone/files/complete/Carrie S01E02 (2024) FRENCH"),
            "No files found are eligible for import in Carrie S01E02 (2024) FRENCH"
        );
        assert_eq!(
            clean_detail("Access to the path '/home/someone/x/Film (2024)/film.mkv' is denied."),
            "Access to the path 'film.mkv' is denied."
        );
        assert_eq!(
            clean_detail("Could not find file \"~/downloads/a b/c.mkv\"."),
            "Could not find file \"c.mkv\"."
        );
        assert_eq!(
            clean_detail("in /data/torrents/Show/, retrying"),
            "in Show, retrying"
        );
        // « VF / French », « 8-bit/10 », S01E02/E03 : pas des chemins
        assert_eq!(clean_detail(CARRIE), CARRIE);
        assert_eq!(
            clean_detail("S01E02/E03 HEVC 8-bit/10-bit"),
            "S01E02/E03 HEVC 8-bit/10-bit"
        );
        // reliquat relatif « home/<compte> »
        assert_eq!(clean_detail("x=home/someone/files"), "x=home/…/files");
        // liens, hôtes, adresses, clés
        assert_eq!(
            clean_detail(
                "Failed to connect to http://seedbox.example.invalid:8080/api/v2?apikey=abc"
            ),
            "Failed to connect to [lien]"
        );
        assert_eq!(clean_detail("magnet:?xt=urn:btih:ABCDEF end"), "[lien] end");
        assert_eq!(
            clean_detail("Unable to connect to 192.0.2.10:8080"),
            "Unable to connect to [adresse]"
        );
        assert_eq!(
            clean_detail("host qbit.example.invalid down"),
            "host [hôte] down"
        );
        assert_eq!(
            clean_detail("host qbittorrent:8080 down"),
            "host [hôte] down"
        );
        assert_eq!(
            clean_detail("hash 0123456789abcdef0123456789abcdef01234567 apikey=s3cr3t-value"),
            "hash [masqué] apikey=[masqué]"
        );
        // un nom de release avec des points n'est pas un hôte (casse de release, extension vidéo)
        assert_eq!(
            clean_detail("Love.Me.S01E01.FRENCH.1080p.WEB.H264-GRP.mkv"),
            "Love.Me.S01E01.FRENCH.1080p.WEB.H264-GRP.mkv"
        );
        // espaces resserrés, longueur bornée
        assert_eq!(clean_detail("  a \n\t b  "), "a b");
        let long = "x".repeat(2000);
        assert_eq!(clean_detail(&long).chars().count(), DETAIL_MAX);
    }

    #[test]
    fn detail_joins_file_and_reason_without_duplicates() {
        let ms = [
            msg("a.mkv", &["Sample", "Sample"]),
            msg("Only a title", &[]),
        ];
        let n = import_blocked(Source::Radarr, &ms).unwrap();
        assert_eq!(n.detail, "a.mkv : Sample · Only a title");
    }

    fn rec_sonarr(series: &str, s: i64, e: i64, state: &str, msgs: serde_json::Value) -> Value {
        serde_json::json!({
            "status": "completed", "trackedDownloadState": state, "trackedDownloadStatus": "warning",
            "title": "Release.Name.S01E02.mkv", "statusMessages": msgs, "episodeHasFile": true,
            "series": {"title": series, "year": 2024},
            "episode": {"seasonNumber": s, "episodeNumber": e, "title": "Foreign Language", "hasFile": true}
        })
    }

    #[test]
    fn a_raw_sonarr_record_becomes_an_item_with_its_episode() {
        let re = stuck_re();
        let r = rec_sonarr(
            "Carrie",
            1,
            2,
            "importPending",
            serde_json::json!([{"title": "Carrie.S01E02.REPACK.mkv", "messages": [CARRIE]}]),
        );
        let items = explain_queue(&[(Source::Sonarr, r)], &auto(&re));
        assert_eq!(items.len(), 1);
        let i = &items[0];
        assert_eq!(i.title, "Carrie");
        assert_eq!(i.episode.as_deref(), Some("S01E02"));
        assert_eq!(i.episode_title.as_deref(), Some("Foreign Language"));
        assert_eq!(i.badge, "Déjà disponible");
        let v = serde_json::to_value(i).unwrap();
        assert_eq!(v["source"], "Sonarr");
        assert_eq!(v["year"], 2024);
        assert!(v["detail"].as_str().unwrap().contains("(4100)"));
    }

    #[test]
    fn a_raw_radarr_record_and_ordinary_records_are_left_out() {
        let re = stuck_re();
        let film = serde_json::json!({
            "status": "warning", "trackedDownloadState": "downloading", "trackedDownloadStatus": "warning",
            "errorMessage": "The download is stalled with no connections",
            "movie": {"title": "Le Film", "year": 2023, "hasFile": false}
        });
        let ok = serde_json::json!({
            "status": "downloading", "trackedDownloadState": "downloading", "trackedDownloadStatus": "ok",
            "movie": {"title": "Autre Film", "year": 2020}
        });
        let items = explain_queue(&[(Source::Radarr, film), (Source::Radarr, ok)], &auto(&re));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Le Film");
        assert_eq!(items[0].episode, None);
        assert_eq!(items[0].badge, "Sans source");
        // sans `movie` (film inconnu de Radarr) : le titre de la release
        let unknown = serde_json::json!({"status": "completed", "trackedDownloadState": "importBlocked",
            "trackedDownloadStatus": "warning", "title": "Some.Release.2024.mkv",
            "statusMessages": [{"title": "Some.Release.2024.mkv", "messages": ["Unknown Movie"]}]});
        let items = explain_queue(&[(Source::Radarr, unknown)], &auto(&re));
        assert_eq!(items[0].title, "Some.Release.2024.mkv");
        assert_eq!(items[0].badge, "Non reconnu");
    }

    #[test]
    fn the_same_card_twice_is_never_guessed() {
        let re = stuck_re();
        let a = auto(&re);
        let blocked = serde_json::json!([{"title": "x.mkv", "messages": ["Unknown Series"]}]);
        let warn = rec_sonarr("Show", 1, 2, "importBlocked", blocked.clone());
        let fine = serde_json::json!({"status": "downloading", "trackedDownloadState": "downloading",
            "trackedDownloadStatus": "ok", "series": {"title": "Show"},
            "episode": {"seasonNumber": 1, "episodeNumber": 2}});
        // même épisode des deux côtés, un seul bloqué : aucune note (la carte ne dit pas laquelle elle montre)
        assert!(explain_queue(
            &[(Source::Sonarr, warn.clone()), (Source::Sonarr, fine)],
            &a
        )
        .is_empty());
        // deux fois la même note : gardée une fois, sans détail
        let two = explain_queue(
            &[
                (Source::Sonarr, warn.clone()),
                (Source::Sonarr, warn.clone()),
            ],
            &a,
        );
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].detail, "");
        // deux notes différentes : aucune
        let other = rec_sonarr(
            "SHOW!",
            1,
            2,
            "importBlocked",
            serde_json::json!([{"title": "y.mkv", "messages": ["Sample"]}]),
        );
        assert!(explain_queue(
            &[(Source::Sonarr, warn.clone()), (Source::Sonarr, other)],
            &a
        )
        .is_empty());
        // autre épisode, ou même titre côté Radarr : indépendants
        let next = rec_sonarr("Show", 1, 3, "importBlocked", blocked);
        assert_eq!(
            explain_queue(&[(Source::Sonarr, warn), (Source::Sonarr, next)], &a).len(),
            2
        );
    }

    #[test]
    fn setup_follows_the_configuration() {
        let mut cfg: crate::config::Config = toml::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../homelab.toml"))
                .unwrap(),
        )
        .unwrap();
        let s = Setup::from_config(&cfg);
        let a = s.automation();
        assert_eq!(a.id_match_every_mins, Some(5));
        let (re, h) = a.stuck.unwrap();
        assert_eq!(h, 8);
        assert!(re.is_match("The download is stalled with no connections"));
        cfg.tasks.disabled = vec!["stuck_handler".into(), "id_match_import".into()];
        let s = Setup::from_config(&cfg);
        assert!(s.automation().stuck.is_none());
        assert!(s.automation().id_match_every_mins.is_none());
    }

    #[test]
    fn title_keys_and_episode_codes() {
        assert_eq!(title_key("Élite"), "elite");
        assert_eq!(
            title_key("  L'Attaque des  Titans! "),
            "l attaque des titans"
        );
        assert_eq!(episode_code(1, 2), "S01E02");
        assert_eq!(episode_code(1, 1050), "S01E1050");
    }
}
