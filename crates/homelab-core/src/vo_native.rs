//! Mode « VO » des comptes et bascule vers la préférence audio native de Jellyfin 12 « Langue d'origine »
//! (2026-10-08, lot 4 : préparée sur décision du propriétaire, à activer une fois la langue d'origine des fiches remplie).
//!
//! **Aujourd'hui** (`[accounts] vo_native = false`) : le mode VO de Mon compte pose `vo_audio_language` (`jpn`) — les
//! animés partent en japonais sur toutes les applis — et le script de Mon compte bascule les autres titres sur leur piste
//! d'origine dans les clients web (`SetAudioStreamIndex`, après le démarrage).
//!
//! **Avec `vo_native = true`** : le mode VO pose `AudioLanguagePreference = "OriginalLanguage"` (option « Langue
//! d'origine » de jellyfin-web 12.1). Le serveur prend la piste de la langue d'origine du titre (métadonnée
//! `OriginalLanguage`, héritée de la série par les épisodes, remplie par `tasks::original_language`), et jellyfin-web la
//! demande dès le premier flux : plus de second démarrage, sous-titres complets dès le départ. Sans métadonnée, le
//! serveur prend la piste par défaut (VF) et le script de Mon compte reste le filet.
//!
//! **Le trou** (essai du 08/10 ; code 12.1 identique à master) : quand la piste d'origine porte le drapeau Matroska
//! « Original » sans être la piste par défaut, `MediaSourceManager.SetDefaultAudioStreamIndex` sort sans poser
//! `DefaultAudioIndexSource`. Un client qui n'impose pas d'index dans `PlaybackInfo` (appli Jellyfin Android TV, donc
//! Fire TV) reçoit alors la première piste compatible : la VF, là où `jpn` lui donne le japonais aujourd'hui. Une
//! préférence vaut pour **tout** le compte : aucun réglage ne donne « Langue d'origine » à jellyfin-web et `jpn` à
//! l'appli Android TV d'un même membre.
//!
//! **Règle sans régression** : un compte en VO ne passe en « Langue d'origine » que si aucun client hors de
//! `vo_native_clients` (jellyfin-web et les applis qui l'embarquent : même piste qu'aujourd'hui pour un animé dont la
//! fiche porte sa langue d'origine, la VO des autres titres en plus, et le filet de Mon compte dans tous les cas)
//! n'apparaît pour lui, à trois endroits : ses lectures depuis `vo_native_days` jours (Playback Reporting), ses sessions
//! ouvertes et ses appareils enregistrés (`GET /Devices`, `LastUserId`). Sinon il est **gardé** sur
//! `vo_audio_language`, exactement comme aujourd'hui.
//!
//! Toute session compte, qu'elle déclare des capacités ou non : l'appli Android TV 0.19.10 ne les envoie
//! (`PlayableMediaTypes`) qu'à son démarrage à froid, et Jellyfin ne les garde qu'en mémoire. Après un redémarrage de
//! Jellyfin, une appli reprise de l'arrière-plan a donc une session sans capacités (2 sessions Android TV sur 2 en prod
//! le 09/10). Seuls les services connus qui ne lisent jamais (`vo_native_ignored_clients` : Seerr) sont écartés, par
//! leur nom.
//!
//! La tâche `vo_native_guard` ramène sur `vo_audio_language` un compte en « Langue d'origine » qui a ensuite une session
//! ou un appareil sur un autre client : en général avant sa première lecture sur ce client, sinon dès cette lecture
//! (sondage toutes les `interval_secs`). Reste un trou étroit : la première lecture d'un appareil neuf lancée moins d'un
//! intervalle après sa connexion, sur un titre touché par le bogue ou sans langue d'origine. Compromis possible, au
//! choix du propriétaire et jamais par défaut : ajouter `Jellyfin Android TV` à la liste (VF sur les titres touchés par
//! le bogue, que `exposure` compte : 0 au 09/10, le drapeau n'étant en base que pour les fichiers sondés depuis la 12.1).
//!
//! Limite côté jellyfin-web (bundle 12.1 relu le 09/10) : `PlaybackInfo` ne porte `AudioStreamIndex` que si la lecture en
//! fournit un (bouton Lire de la fiche, qui le lit dans la fiche du compte) ; lancée d'une carte (accueil, reprise), elle
//! n'en porte pas, et un titre touché par le bogue partirait en VF avant que le filet de Mon compte ne rebascule (un
//! second démarrage, là où `jpn` part directement en japonais).
//!
//! Décisions pures et testées ici ; les appels (Jellyfin, Playback Reporting, état, sauvegarde) restent minces.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::clients::JellyfinClient;
use crate::config::Accounts;
use crate::state::{now, StateStore, VoNativeRecord};

/// Valeur littérale de l'option « Langue d'origine » de jellyfin-web 12.1 (`UserConfiguration.AudioLanguagePreference`).
pub const ORIGINAL_LANGUAGE: &str = "OriginalLanguage";
/// Client sans nom dans l'historique : jamais compté comme sûr.
pub const UNKNOWN_CLIENT: &str = "(client inconnu)";
/// Historique de lecture illisible : le compte est gardé, jamais passé en « Langue d'origine » à l'aveugle.
pub const UNREADABLE: &str = "(historique illisible)";

/// Mode de lecture choisi dans Mon compte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Piste française d'abord, sous-titres seulement quand l'audio n'est pas en français.
    Fr,
    /// Audio d'origine, sous-titres français toujours.
    Vo,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Fr => "fr",
            Mode::Vo => "vo",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fr" => Some(Mode::Fr),
            "vo" => Some(Mode::Vo),
            _ => None,
        }
    }
}

/// `true` pour la préférence « Langue d'origine ».
pub fn is_native(audio: &str) -> bool {
    audio.trim().eq_ignore_ascii_case(ORIGINAL_LANGUAGE)
}

/// Mode d'un compte d'après ses préférences : VO = sous-titres `Always` et audio vide (ancien mode, avant le
/// 2026-09-25), `vo_audio_language` (`jpn`) ou « Langue d'origine ». Tout le reste est `fr`, y compris un compte réglé à
/// la main sur une autre langue (`eng`) : il n'est jamais touché par une migration.
pub fn mode_of(audio: &str, subtitle_mode: &str, vo_audio_language: &str) -> Mode {
    let audio = audio.trim();
    let vo_audio =
        audio.is_empty() || audio.eq_ignore_ascii_case(vo_audio_language) || is_native(audio);
    if subtitle_mode == "Always" && vo_audio {
        Mode::Vo
    } else {
        Mode::Fr
    }
}

/// Champ texte de la `Configuration` d'un compte Jellyfin (`GET /Users`), vide s'il manque.
pub fn config_str<'a>(user: &'a Value, key: &str) -> &'a str {
    user.get("Configuration")
        .and_then(|c| c.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Mode d'un compte Jellyfin (`GET /Users` ou `GET /Users/{id}`).
pub fn mode_of_user(user: &Value, vo_audio_language: &str) -> Mode {
    mode_of(
        config_str(user, "AudioLanguagePreference"),
        config_str(user, "SubtitleMode"),
        vo_audio_language,
    )
}

/// Préférences de langue posées sur un compte (`set_language_prefs`, `PlayDefaultAudioTrack` toujours `false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub audio: String,
    pub subtitles: String,
    pub subtitle_mode: String,
}

/// Préférences d'un mode ; `native` : le compte en VO passe en « Langue d'origine » (voir `vo_audio`).
pub fn prefs_for(mode: Mode, cfg: &Accounts, native: bool) -> Prefs {
    match mode {
        Mode::Fr => Prefs {
            audio: cfg.audio_language.clone(),
            subtitles: cfg.subtitle_language.clone(),
            subtitle_mode: cfg.subtitle_mode.clone(),
        },
        Mode::Vo => Prefs {
            audio: if native {
                ORIGINAL_LANGUAGE.to_string()
            } else {
                cfg.vo_audio_language.clone()
            },
            subtitles: cfg.subtitle_language.clone(),
            subtitle_mode: "Always".into(),
        },
    }
}

/// Mode d'un compte neuf : celui que décrivent `audio_language` et `subtitle_mode` (`fre` + `Smart` = `fr`). Si l'admin
/// règle l'onboarding sur la VO (`vo_audio_language` + `Always`), le compte neuf suit les règles du mode VO.
pub fn onboarding_mode(cfg: &Accounts) -> Mode {
    mode_of(
        &cfg.audio_language,
        &cfg.subtitle_mode,
        &cfg.vo_audio_language,
    )
}

/// Client de la liste sûre (`vo_native_clients`, casse et espaces de bord ignorés) ; un nom vide ne l'est jamais.
pub fn is_safe(client: &str, safe: &[String]) -> bool {
    let c = client.trim();
    !c.is_empty() && safe.iter().any(|s| s.trim().eq_ignore_ascii_case(c))
}

/// Service connu qui se connecte au nom d'un membre sans jamais lire (`vo_native_ignored_clients` : Seerr), même
/// comparaison que `is_safe` ; un nom vide ne l'est jamais.
pub fn is_ignored(client: &str, ignored: &[String]) -> bool {
    is_safe(client, ignored)
}

/// Client qui garde un compte sur `vo_audio_language` : ni dans la liste sûre, ni un service ignoré. Un nom vide
/// (client inconnu) en est un.
pub fn is_unsafe(client: &str, cfg: &Accounts) -> bool {
    !is_safe(client, &cfg.vo_native_clients) && !is_ignored(client, &cfg.vo_native_ignored_clients)
}

/// Clients utilisés qui gardent le compte (`is_unsafe`), triés et sans doublon (nom vide → `UNKNOWN_CLIENT`).
pub fn unsafe_clients<'a>(
    used: impl IntoIterator<Item = &'a String>,
    cfg: &Accounts,
) -> Vec<String> {
    used.into_iter()
        .filter(|c| is_unsafe(c, cfg))
        .map(|c| {
            let c = c.trim();
            if c.is_empty() {
                UNKNOWN_CLIENT.to_string()
            } else {
                c.to_string()
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Issue d'une décision sur un compte en VO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// « Langue d'origine ».
    Native,
    /// Gardé sur `vo_audio_language` : un client hors liste (ou un historique illisible).
    Held,
    /// `vo_audio_language`, bascule coupée ou retour arrière.
    Classic,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Native => "native",
            Outcome::Held => "held",
            Outcome::Classic => "classic",
        }
    }
}

/// Cible d'un compte en VO quand la bascule est engagée (migration, garde) : « Langue d'origine » si aucun client hors
/// liste, sinon `vo_audio_language` (« gardé »).
pub fn native_target(cfg: &Accounts, unsafe_found: &[String]) -> (String, Outcome) {
    if unsafe_found.is_empty() {
        (ORIGINAL_LANGUAGE.to_string(), Outcome::Native)
    } else {
        (cfg.vo_audio_language.clone(), Outcome::Held)
    }
}

/// Langue audio du mode VO pour un compte (Mon compte, onboarding) : `vo_audio_language` tant que la bascule est coupée
/// (`vo_native = false`, rien ne change), sinon `native_target`.
pub fn vo_audio(cfg: &Accounts, unsafe_found: &[String]) -> (String, Outcome) {
    if cfg.vo_native {
        native_target(cfg, unsafe_found)
    } else {
        (cfg.vo_audio_language.clone(), Outcome::Classic)
    }
}

/// Identifiant Jellyfin compact (minuscules, sans tirets) : `GET /Users`, sessions et Playback Reporting n'écrivent pas
/// tous les identifiants pareil.
pub fn compact(id: &str) -> String {
    crate::chat::normalize_id(id)
}

/// Requête Playback Reporting : clients de lecture par compte sur `days` jours (`days` est un entier : rien d'injecté).
pub fn playback_sql(days: u32) -> String {
    format!(
        "SELECT UserId, ClientName FROM PlaybackActivity \
         WHERE DateCreated > datetime('now', '-{days} day') GROUP BY UserId, ClientName"
    )
}

/// Clients de lecture par compte (id compact) depuis la réponse de `playback_sql`. Colonnes absentes = erreur : un
/// historique illisible ne doit jamais passer pour « aucun client » (le compte passerait en « Langue d'origine » sans
/// contrôle).
pub fn clients_from_rows(
    cols: &[String],
    rows: &[Vec<String>],
) -> Result<HashMap<String, BTreeSet<String>>> {
    let idx = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
    let (u, c) = (
        idx("UserId").context("Playback Reporting : colonne UserId absente")?,
        idx("ClientName").context("Playback Reporting : colonne ClientName absente")?,
    );
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    for r in rows {
        let Some(user) = r.get(u).map(|s| compact(s)).filter(|s| !s.is_empty()) else {
            continue;
        };
        let client = r.get(c).map(|s| s.trim()).unwrap_or("");
        // une cellule NULL revient en texte « null » : client inconnu, donc pas sûr
        let client = if client.eq_ignore_ascii_case("null") {
            ""
        } else {
            client
        };
        out.entry(user).or_default().insert(client.to_string());
    }
    Ok(out)
}

/// (id compact du compte, client) de chaque élément qui a un compte : `user_key` et `client_key` nomment les champs.
fn pairs(items: &[Value], user_key: &str, client_key: &str) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|s| {
            let user = compact(s.get(user_key).and_then(Value::as_str)?);
            if user.is_empty() {
                return None;
            }
            let client = s.get(client_key).and_then(Value::as_str).unwrap_or("");
            Some((user, client.trim().to_string()))
        })
        .collect()
}

/// Sessions ouvertes des membres : (id compact du compte, client). **Toutes** comptent, capacités déclarées ou non :
/// l'appli Android TV reprise après un redémarrage de Jellyfin a `PlayableMediaTypes = []` jusqu'à sa première lecture
/// (2 sur 2 en prod le 09/10) ; les services qui ne lisent pas sont écartés ensuite par leur nom (`is_unsafe`). Une
/// session sans compte (clé d'API : homelabd, Homarr) n'y est pas.
pub fn session_clients(sessions: &[Value]) -> Vec<(String, String)> {
    pairs(sessions, "UserId", "Client")
}

/// Appareils enregistrés (`GET /Devices`, liste complète : son filtre `?userId=` est ignoré) : (id compact du dernier
/// compte, appli). Un appareil connecté reste là jusqu'à sa déconnexion : l'appli Android TV d'un compte est vue sans
/// attendre qu'elle soit ouverte ou qu'elle lise. Limite : un appareil partagé n'indique que son dernier compte (les
/// autres sont vus par leurs lectures et leurs sessions).
pub fn device_clients(devices: &[Value]) -> Vec<(String, String)> {
    pairs(devices, "LastUserId", "AppName")
}

/// Sens d'une migration des comptes en mode VO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// `homelabctl accounts vo-native` : vers « Langue d'origine » (ou gardé).
    Native,
    /// `homelabctl accounts vo-classic` : retour à `vo_audio_language`.
    Classic,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Native => "native",
            Direction::Classic => "classic",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "native" => Some(Direction::Native),
            "classic" => Some(Direction::Classic),
            _ => None,
        }
    }
}

/// Un compte en mode VO et sa langue audio cible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub user_id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub outcome: Outcome,
    /// Clients hors liste qui gardent le compte sur `vo_audio_language`.
    pub clients: Vec<String>,
}

impl Step {
    pub fn changes(&self) -> bool {
        !self.from.trim().eq_ignore_ascii_case(self.to.trim())
    }
}

/// Plan d'une migration : chaque compte en mode VO (les comptes `fr` ne sont jamais touchés), trié par nom.
/// - `Native` : « Langue d'origine » si aucun client hors liste dans `clients` (id compact → clients de ses lectures,
///   sessions et appareils : `clients_by_user`), sinon
///   `vo_audio_language` — y compris pour un compte déjà en « Langue d'origine » qui lit maintenant par un client hors
///   liste ;
/// - `Classic` : un compte en « Langue d'origine » revient à `vo_audio_language` ; les autres ne bougent pas.
pub fn plan(
    users: &[Value],
    clients: &HashMap<String, BTreeSet<String>>,
    cfg: &Accounts,
    dir: Direction,
) -> Vec<Step> {
    let mut out: Vec<Step> = users
        .iter()
        .filter(|u| mode_of_user(u, &cfg.vo_audio_language) == Mode::Vo)
        .filter_map(|u| {
            let id = u.get("Id").and_then(Value::as_str)?.to_string();
            let name = u.get("Name").and_then(Value::as_str)?.to_string();
            let from = config_str(u, "AudioLanguagePreference").to_string();
            let (to, outcome, found) = match dir {
                Direction::Native => {
                    let found =
                        unsafe_clients(clients.get(&compact(&id)).into_iter().flatten(), cfg);
                    let (to, outcome) = native_target(cfg, &found);
                    (to, outcome, found)
                }
                Direction::Classic => {
                    let to = if is_native(&from) {
                        cfg.vo_audio_language.clone()
                    } else {
                        from.clone()
                    };
                    (to, Outcome::Classic, Vec::new())
                }
            };
            Some(Step {
                user_id: id,
                name,
                from,
                to,
                outcome,
                clients: found,
            })
        })
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

/// Un compte en « Langue d'origine » à ramener sur `vo_audio_language` (tâche `vo_native_guard`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub user_id: String,
    pub name: String,
    pub clients: Vec<String>,
}

/// Compte en mode VO réglé sur « Langue d'origine » (ceux que surveille la garde). Un compte en mode `fr` qui a choisi
/// « Langue d'origine » lui-même dans jellyfin-web n'en est pas (son choix).
pub fn is_native_vo(user: &Value, cfg: &Accounts) -> bool {
    mode_of_user(user, &cfg.vo_audio_language) == Mode::Vo
        && is_native(config_str(user, "AudioLanguagePreference"))
}

/// Comptes en mode VO réglés sur « Langue d'origine » qui ont une session ouverte (capacités déclarées ou non) ou un
/// appareil enregistré sur un client hors liste (`is_unsafe`).
pub fn guard_holds(
    sessions: &[Value],
    devices: &[Value],
    users: &[Value],
    cfg: &Accounts,
) -> Vec<Hold> {
    let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (user, client) in session_clients(sessions)
        .into_iter()
        .chain(device_clients(devices))
    {
        if is_unsafe(&client, cfg) {
            seen.entry(user).or_default().insert(client);
        }
    }
    let mut out: Vec<Hold> = users
        .iter()
        .filter(|u| is_native_vo(u, cfg))
        .filter_map(|u| {
            let id = u.get("Id").and_then(Value::as_str)?;
            let found = seen.get(&compact(id))?;
            Some(Hold {
                user_id: id.to_string(),
                name: u
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
                clients: unsafe_clients(found.iter(), cfg),
            })
        })
        .collect();
    out.sort_by_key(|h| h.name.to_lowercase());
    out
}

/// Sauvegarde d'avant migration : la `Configuration` complète de chaque compte qui va changer.
pub fn backup_body(users: &[Value], steps: &[&Step], dir: Direction, at: i64) -> Value {
    let accounts: Vec<Value> = steps
        .iter()
        .filter_map(|s| {
            let u = users
                .iter()
                .find(|u| u.get("Id").and_then(Value::as_str) == Some(s.user_id.as_str()))?;
            Some(json!({
                "Id": s.user_id,
                "Name": s.name,
                "Configuration": u.get("Configuration").cloned().unwrap_or(Value::Null),
                "planned": { "from": s.from, "to": s.to, "outcome": s.outcome },
            }))
        })
        .collect();
    json!({ "direction": dir, "at": at, "accounts": accounts })
}

/// Écrit la sauvegarde dans `dir` (`[paths] backups`) : fichier neuf en 0600, jamais écrasé.
pub fn write_backup(dir: &Path, body: &Value, stamp: &str, d: Direction) -> Result<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(format!("vo-native-{stamp}-{}-avant.json", d.as_str()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("sauvegarde {}", path.display()))?;
    f.write_all(serde_json::to_string_pretty(body)?.as_bytes())
        .and_then(|_| f.sync_all())
        .with_context(|| format!("écriture de {}", path.display()))?;
    Ok(path)
}

/// Formes ISO 639-2 d'une langue d'origine ISO 639-1 (celle des fiches et de TMDB) ; un code inconnu reste tel quel.
/// Même table que le script de Mon compte (`ISO2` de `assets/compte/app.js`).
pub fn iso639_2(lang: &str) -> Vec<String> {
    let l = lang.trim().to_ascii_lowercase();
    let forms: &[&str] = match l.as_str() {
        "fr" => &["fre", "fra"],
        "en" => &["eng"],
        "ja" => &["jpn"],
        "ko" => &["kor"],
        "es" => &["spa"],
        "de" => &["ger", "deu"],
        "it" => &["ita"],
        "zh" | "cn" => &["chi", "zho"],
        "pt" => &["por"],
        "ru" => &["rus"],
        "hi" => &["hin"],
        "nl" => &["dut", "nld"],
        "sv" => &["swe"],
        "da" => &["dan"],
        "no" => &["nor", "nob"],
        "pl" => &["pol"],
        "tr" => &["tur"],
        "ar" => &["ara"],
        "th" => &["tha"],
        "fi" => &["fin"],
        "he" => &["heb"],
        "id" => &["ind"],
        _ => &[],
    };
    let mut out: Vec<String> = forms.iter().map(|s| s.to_string()).collect();
    out.push(l);
    out
}

/// Ce qu'un passage en « Langue d'origine » changerait pour les clients qui n'imposent pas d'index (appli Android TV,
/// Fire TV) et pour le filet de Mon compte, mesuré sur les films et épisodes (`homelabctl accounts vo-native --dry-run`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Exposure {
    /// Films et épisodes lus.
    pub videos: usize,
    /// Langue d'origine connue (fiche, ou série pour un épisode).
    pub with_original_language: usize,
    /// Sans langue d'origine mais avec une piste japonaise : le serveur servirait la piste par défaut (VF) là où `jpn`
    /// donne le japonais — à remplir avant l'activation (`original_language`).
    pub jpn_without_original_language: usize,
    /// Piste d'origine marquée « Original » (drapeau Matroska) sans être la piste par défaut ni la première : le bogue de
    /// Jellyfin 12.1 la fait perdre aux clients qui n'imposent pas d'index.
    pub flagged: usize,
    /// … dont piste japonaise : ces titres partiraient en VF sur l'appli Android TV d'un compte en « Langue d'origine »
    /// (ou un second démarrage dans jellyfin-web lancé hors de la fiche, rattrapé par le filet de Mon compte).
    pub flagged_jpn: usize,
    /// Quelques titres de chaque catégorie, pour vérifier à la main.
    pub examples_jpn_without_original_language: Vec<String>,
    pub examples_flagged: Vec<String>,
}

const EXAMPLES: usize = 8;

/// Mesure `Exposure` sur des films et épisodes (`GET /Items`, `Fields=MediaStreams`) ; `series_ol` : langue d'origine
/// des séries (id → code). Reproduit `MediaSourceManager.SetDefaultAudioStreamIndex` de Jellyfin 12.1 avec
/// `PlayDefaultAudioTrack = false` : la branche fautive est prise quand une piste audio porte `IsOriginal` et que la
/// langue d'origine est vide ou est celle de cette piste ; elle se voit quand cette piste n'est ni par défaut ni la
/// première (StreamBuilder sert alors la première compatible).
pub fn exposure(videos: &[Value], series_ol: &HashMap<String, String>) -> Exposure {
    let mut e = Exposure {
        videos: videos.len(),
        ..Default::default()
    };
    for v in videos {
        let own = v
            .get("OriginalLanguage")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let inherited = v
            .get("SeriesId")
            .and_then(Value::as_str)
            .and_then(|s| series_ol.get(&compact(s)))
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty());
        let ol = own.or(inherited).unwrap_or("");
        let audio: Vec<&Value> = v
            .get("MediaStreams")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|s| s.get("Type").and_then(Value::as_str) == Some("Audio"))
                    .collect()
            })
            .unwrap_or_default();
        let lang = |s: &Value| {
            s.get("Language")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        };
        let flag = |s: &Value, k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        // exemple : la série pour un épisode (une fois), le titre pour un film
        let title = || {
            v.get("SeriesName")
                .and_then(Value::as_str)
                .or_else(|| v.get("Name").and_then(Value::as_str))
                .unwrap_or("?")
                .to_string()
        };
        let add = |list: &mut Vec<String>, t: String| {
            if list.len() < EXAMPLES && !list.contains(&t) {
                list.push(t);
            }
        };
        let has_jpn = audio.iter().any(|s| lang(s) == "jpn");
        if ol.is_empty() {
            if has_jpn {
                e.jpn_without_original_language += 1;
                add(&mut e.examples_jpn_without_original_language, title());
            }
        } else {
            e.with_original_language += 1;
        }
        let Some(pos) = audio.iter().position(|s| flag(s, "IsOriginal")) else {
            continue;
        };
        let orig = audio[pos];
        let branch = ol.is_empty() || iso639_2(ol).contains(&lang(orig));
        if branch && pos > 0 && !flag(orig, "IsDefault") {
            e.flagged += 1;
            if lang(orig) == "jpn" {
                e.flagged_jpn += 1;
            }
            add(
                &mut e.examples_flagged,
                format!("{} [{}]", title(), lang(orig)),
            );
        }
    }
    e
}

/// Lit films, épisodes (pistes) et séries (langue d'origine) avec le compte `admin` (`GET /Items` sans compte renvoie
/// une liste incomplète ; les séries sont lues avec ET sans compte, Jellyfin regroupant une série présente dans deux
/// dossiers). Lecture seule, ~15 Mo pour toute la médiathèque : à lancer hors soirée.
pub async fn exposure_scan(jf: &JellyfinClient, admin: &str) -> Result<Exposure> {
    let videos = jf
        .items_paged(
            &[
                ("UserId", admin),
                ("Recursive", "true"),
                ("IncludeItemTypes", "Movie,Episode"),
                ("Fields", "MediaStreams"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ],
            300,
        )
        .await
        .context("films et épisodes")?;
    let mut series_ol = HashMap::new();
    for with_user in [true, false] {
        let mut q = vec![
            ("Recursive", "true"),
            ("IncludeItemTypes", "Series"),
            ("EnableImages", "false"),
            ("EnableUserData", "false"),
        ];
        if with_user {
            q.push(("UserId", admin));
        }
        for s in jf.items(&q).await.context("séries")? {
            if let (Some(id), Some(ol)) = (
                s.get("Id").and_then(Value::as_str),
                s.get("OriginalLanguage").and_then(Value::as_str),
            ) {
                series_ol.insert(compact(id), ol.to_string());
            }
        }
    }
    Ok(exposure(&videos, &series_ol))
}

/// Clients par compte : lectures de Playback Reporting sur `days` jours, sessions ouvertes (toutes) et appareils
/// enregistrés. Une des trois sources illisible = erreur (le compte est gardé, jamais décidé sur une vue partielle).
pub async fn clients_by_user(
    jf: &JellyfinClient,
    days: u32,
) -> Result<HashMap<String, BTreeSet<String>>> {
    let (cols, rows) = jf
        .playback_query(&playback_sql(days))
        .await
        .context("Playback Reporting illisible")?;
    let mut out = clients_from_rows(&cols, &rows)?;
    let sessions = jf.sessions().await.context("sessions Jellyfin")?;
    let devices = jf.devices().await.context("appareils Jellyfin")?;
    for (user, client) in session_clients(&sessions)
        .into_iter()
        .chain(device_clients(&devices))
    {
        out.entry(user).or_default().insert(client);
    }
    Ok(out)
}

/// Décision pour UN compte qui choisit un mode (Mon compte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub prefs: Prefs,
    /// `None` : mode `fr`, ou VO avec la bascule coupée (rien de neuf à noter).
    pub outcome: Option<Outcome>,
    pub clients: Vec<String>,
}

/// Préférences à poser quand un compte choisit `mode`. Bascule active : clients du compte lus (Playback Reporting,
/// sessions, appareils) ; s'ils sont illisibles, le compte est gardé (`UNREADABLE`), jamais passé en « Langue
/// d'origine » à l'aveugle.
pub async fn decide(jf: &JellyfinClient, cfg: &Accounts, user_id: &str, mode: Mode) -> Decision {
    if mode == Mode::Fr || !cfg.vo_native {
        return Decision {
            prefs: prefs_for(mode, cfg, false),
            outcome: None,
            clients: Vec::new(),
        };
    }
    let found = match clients_by_user(jf, cfg.vo_native_days).await {
        Ok(m) => unsafe_clients(m.get(&compact(user_id)).into_iter().flatten(), cfg),
        Err(e) => {
            warn!(
                task = "vo_native",
                error = format!("{e:#}"),
                "historique de lecture illisible : compte gardé"
            );
            vec![UNREADABLE.to_string()]
        }
    };
    let (audio, outcome) = vo_audio(cfg, &found);
    Decision {
        prefs: Prefs {
            audio,
            ..prefs_for(Mode::Vo, cfg, false)
        },
        outcome: Some(outcome),
        clients: found,
    }
}

/// Note un changement dans `state.vo_native` (id compact).
pub async fn record(state: &StateStore, user_id: &str, rec: VoNativeRecord) -> Result<()> {
    let key = compact(user_id);
    state
        .update(|s| {
            s.vo_native.insert(key, rec);
        })
        .await
}

/// Résultat d'une migration (renvoyé tel quel par `POST /admin/vo-native`).
#[derive(Debug, Clone, Serialize)]
pub struct Migration {
    pub direction: Direction,
    pub dry_run: bool,
    /// Bascule (`[accounts] vo_native`) au moment de la migration.
    pub vo_native: bool,
    pub steps: Vec<Step>,
    /// Sauvegarde écrite avant le premier changement.
    pub backup: Option<String>,
    pub applied: Vec<String>,
    pub errors: Vec<String>,
    pub warning: Option<String>,
    /// À blanc vers « Langue d'origine » : condition d'activation (langue d'origine remplie) et titres touchés par le
    /// bogue de Jellyfin 12.1. `None` ailleurs, ou si la lecture a échoué (`exposure_error`).
    pub exposure: Option<Exposure>,
    pub exposure_error: Option<String>,
}

/// Migration des comptes en mode VO dans un sens ou dans l'autre. Ordre immuable : plan, sauvegarde de la configuration
/// complète des comptes qui changent (rien ne change si elle échoue), puis un compte à la fois, chacun noté dans l'état.
/// Vers « Langue d'origine » : refusée tant que la bascule est coupée (sinon la garde ne surveillerait pas les comptes
/// basculés). `dry_run` : plan seulement, rien d'écrit nulle part.
pub async fn migrate(
    jf: &JellyfinClient,
    cfg: &Accounts,
    backups: &Path,
    state: &StateStore,
    dir: Direction,
    dry_run: bool,
) -> Result<Migration> {
    if dir == Direction::Native && !cfg.vo_native && !dry_run {
        bail!("[accounts] vo_native = false : passer d'abord l'interrupteur à true dans homelab.toml et redémarrer homelabd (la garde doit tourner)");
    }
    let users = jf.users().await?;
    let clients = match dir {
        Direction::Native => clients_by_user(jf, cfg.vo_native_days).await?,
        Direction::Classic => HashMap::new(),
    };
    let steps = plan(&users, &clients, cfg, dir);
    let warning = match (dir, cfg.vo_native) {
        (Direction::Native, false) => Some(
            "vo_native = false : migration réelle refusée tant que l'interrupteur est coupé".to_string(),
        ),
        (Direction::Classic, true) => Some(
            "vo_native = true : un membre qui rechoisit la VO dans Mon compte repassera en « Langue d'origine » ; couper l'interrupteur (et redémarrer homelabd) puis relancer vo-classic".to_string(),
        ),
        _ => None,
    };
    let mut m = Migration {
        direction: dir,
        dry_run,
        vo_native: cfg.vo_native,
        steps,
        backup: None,
        applied: Vec::new(),
        errors: Vec::new(),
        warning,
        exposure: None,
        exposure_error: None,
    };
    if dry_run && dir == Direction::Native {
        let admin = users
            .iter()
            .find(|u| crate::accounts::is_admin(u))
            .and_then(|u| u.get("Id").and_then(Value::as_str));
        match admin {
            Some(admin) => match exposure_scan(jf, admin).await {
                Ok(e) => m.exposure = Some(e),
                Err(e) => m.exposure_error = Some(format!("{e:#}")),
            },
            None => m.exposure_error = Some("aucun compte admin Jellyfin".into()),
        }
    }
    let todo: Vec<&Step> = m.steps.iter().filter(|s| s.changes()).collect();
    if dry_run || todo.is_empty() {
        return Ok(m);
    }
    let at = now();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = write_backup(backups, &backup_body(&users, &todo, dir, at), &stamp, dir)?;
    info!(task = "vo_native", direction = dir.as_str(), accounts = todo.len(), backup = %path.display(), "réglages sauvegardés avant migration");
    m.backup = Some(path.display().to_string());
    let source = match dir {
        Direction::Native => "migration",
        Direction::Classic => "retour",
    };
    let todo: Vec<Step> = todo.into_iter().cloned().collect();
    for s in todo {
        match jf.set_vo_audio(&s.user_id, &s.to).await {
            Ok(()) => {
                info!(task = "vo_native", user = %s.name, from = %s.from, to = %s.to, outcome = s.outcome.as_str(), "langue audio du mode VO changée");
                let rec = VoNativeRecord {
                    at,
                    name: s.name.clone(),
                    source: source.into(),
                    outcome: s.outcome.as_str().into(),
                    old: s.from.clone(),
                    new: s.to.clone(),
                    clients: s.clients.clone(),
                };
                if let Err(e) = record(state, &s.user_id, rec).await {
                    warn!(task = "vo_native", user = %s.name, error = format!("{e:#}"), "changement fait mais non noté dans l'état");
                }
                m.applied.push(s.name);
            }
            Err(e) => {
                warn!(task = "vo_native", user = %s.name, error = format!("{e:#}"), "langue audio du mode VO non changée");
                m.errors.push(format!("{} : {e:#}", s.name));
            }
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Accounts {
        Accounts::default()
    }

    fn user(id: &str, name: &str, audio: Option<&str>, mode: &str) -> Value {
        let mut c = json!({ "SubtitleMode": mode, "SubtitleLanguagePreference": "fre", "PlayDefaultAudioTrack": false });
        if let Some(a) = audio {
            c["AudioLanguagePreference"] = json!(a);
        }
        json!({ "Id": id, "Name": name, "Configuration": c })
    }

    fn set(xs: &[&str]) -> BTreeSet<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mode_recognises_jpn_empty_and_original_language() {
        assert_eq!(mode_of("jpn", "Always", "jpn"), Mode::Vo);
        assert_eq!(mode_of("", "Always", "jpn"), Mode::Vo); // ancien mode (avant le 25/09)
        assert_eq!(mode_of("OriginalLanguage", "Always", "jpn"), Mode::Vo);
        assert_eq!(mode_of("originallanguage", "Always", "jpn"), Mode::Vo);
        assert_eq!(mode_of(" JPN ", "Always", "jpn"), Mode::Vo);
        // les autres : fr (un compte réglé à la main sur l'anglais n'est jamais pris pour un compte VO)
        assert_eq!(mode_of("fre", "Smart", "jpn"), Mode::Fr);
        assert_eq!(mode_of("eng", "Always", "jpn"), Mode::Fr);
        assert_eq!(mode_of("eng", "Default", "jpn"), Mode::Fr);
        assert_eq!(mode_of("jpn", "Smart", "jpn"), Mode::Fr);
        assert_eq!(mode_of("OriginalLanguage", "Smart", "jpn"), Mode::Fr);
        // une Configuration sans champ : fr
        assert_eq!(mode_of_user(&json!({ "Id": "x" }), "jpn"), Mode::Fr);
        assert_eq!(
            mode_of_user(&user("a", "A", None, "Always"), "jpn"),
            Mode::Vo
        );
    }

    #[test]
    fn modes_parse_and_print() {
        for m in [Mode::Fr, Mode::Vo] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("VO"), None);
        for d in [Direction::Native, Direction::Classic] {
            assert_eq!(Direction::parse(d.as_str()), Some(d));
        }
        assert_eq!(Direction::parse("jpn"), None);
    }

    #[test]
    fn prefs_follow_the_switch() {
        let c = cfg();
        let fr = prefs_for(Mode::Fr, &c, true);
        assert_eq!(
            (fr.audio.as_str(), fr.subtitle_mode.as_str()),
            ("fre", "Smart")
        );
        let vo = prefs_for(Mode::Vo, &c, false);
        assert_eq!(
            (vo.audio.as_str(), vo.subtitle_mode.as_str()),
            ("jpn", "Always")
        );
        assert_eq!(vo.subtitles, "fre");
        let native = prefs_for(Mode::Vo, &c, true);
        assert_eq!(native.audio, ORIGINAL_LANGUAGE);
        assert_eq!(native.subtitle_mode, "Always");
        // bascule coupée (défaut) : le mode VO reste jpn, quels que soient les clients
        assert!(!c.vo_native);
        assert_eq!(vo_audio(&c, &[]), ("jpn".to_string(), Outcome::Classic));
        let on = Accounts {
            vo_native: true,
            ..cfg()
        };
        assert_eq!(
            vo_audio(&on, &[]),
            (ORIGINAL_LANGUAGE.to_string(), Outcome::Native)
        );
        assert_eq!(
            vo_audio(&on, &["Jellyfin Android TV".into()]),
            ("jpn".to_string(), Outcome::Held)
        );
    }

    #[test]
    fn onboarding_mode_comes_from_the_defaults() {
        assert_eq!(onboarding_mode(&cfg()), Mode::Fr);
        let vo = Accounts {
            audio_language: "jpn".into(),
            subtitle_mode: "Always".into(),
            ..cfg()
        };
        assert_eq!(onboarding_mode(&vo), Mode::Vo);
    }

    #[test]
    fn safe_list_is_case_insensitive_and_unknown_is_unsafe() {
        let safe = cfg().vo_native_clients;
        assert!(is_safe("Jellyfin Web", &safe));
        assert!(is_safe(" jellyfin desktop ", &safe));
        assert!(is_safe("Jellyfin for WebOS", &safe));
        for c in [
            "Jellyfin Android TV",
            "JellyWatch TV",
            "Chromecast",
            "Swiftfin iOS",
            "",
            "  ",
        ] {
            assert!(!is_safe(c, &safe), "{c:?}");
        }
        let used = set(&[
            "Jellyfin Web",
            "Chromecast",
            "",
            "Jellyfin Android TV",
            "Seerr",
            " jellyseerr ",
        ]);
        assert_eq!(
            unsafe_clients(used.iter(), &cfg()),
            vec![
                UNKNOWN_CLIENT.to_string(),
                "Chromecast".into(),
                "Jellyfin Android TV".into()
            ]
        );
        assert!(unsafe_clients(
            set(&["Jellyfin Web", "Jellyfin iOS", "Seerr"]).iter(),
            &cfg()
        )
        .is_empty());
        // les services ignorés ne gardent pas un compte, mais ne le rendent pas « sûr » non plus : vide reste inconnu
        assert!(!is_unsafe("Seerr", &cfg()));
        assert!(is_unsafe("", &cfg()));
        assert!(!is_ignored("", &cfg().vo_native_ignored_clients));
    }

    #[test]
    fn playback_rows_are_grouped_by_compact_id_and_missing_columns_fail() {
        let cols = vec!["UserId".to_string(), "ClientName".to_string()];
        let rows = vec![
            vec!["AB-CD".into(), "Jellyfin Web".into()],
            vec!["abcd".into(), "Jellyfin Android TV".into()],
            vec!["ef".into(), "null".into()],
            vec!["".into(), "Chromecast".into()],
        ];
        let m = clients_from_rows(&cols, &rows).unwrap();
        assert_eq!(m["abcd"], set(&["Jellyfin Web", "Jellyfin Android TV"]));
        assert_eq!(m["ef"], set(&[""]));
        assert_eq!(m.len(), 2);
        // historique illisible : erreur, jamais « aucun client »
        assert!(clients_from_rows(&["ItemId".into()], &rows).is_err());
        assert!(playback_sql(60).contains("'-60 day'"));
    }

    /// Sessions au format de la prod (09/10, après un redémarrage de Jellyfin) : l'appli Android TV reprise de
    /// l'arrière-plan n'a déclaré aucune capacité (`PlayableMediaTypes: []`) et ne lit rien. Elle compte quand même.
    #[test]
    fn every_member_session_counts_whatever_its_capabilities() {
        let sessions = vec![
            json!({ "UserId": "A-1", "Client": "Jellyfin Android TV", "ApplicationVersion": "0.19.10", "PlayableMediaTypes": [] }),
            json!({ "UserId": "b2", "Client": "Seerr", "PlayableMediaTypes": [] }),
            json!({ "UserId": "c3", "Client": "Chromecast", "NowPlayingItem": { "Id": "x" } }),
            json!({ "UserId": "d4", "Client": "Jellyfin Web", "PlayableMediaTypes": ["Audio", "Video"] }),
            json!({ "UserId": "e5" }),
            // clé d'API (homelabd, Homarr) : aucune session de membre
            json!({ "Client": "Jellyfin Web", "PlayableMediaTypes": ["Video"] }),
            json!({ "UserId": "", "Client": "Jellyfin Android TV" }),
        ];
        let got = session_clients(&sessions);
        assert_eq!(
            got,
            vec![
                ("a1".to_string(), "Jellyfin Android TV".to_string()),
                ("b2".to_string(), "Seerr".to_string()),
                ("c3".to_string(), "Chromecast".to_string()),
                ("d4".to_string(), "Jellyfin Web".to_string()),
                ("e5".to_string(), String::new()),
            ]
        );
        // ce qui garde un compte : Android TV, Chromecast et le client sans nom ; ni Seerr ni jellyfin-web
        let c = cfg();
        let keep: Vec<&str> = got
            .iter()
            .filter(|(_, cl)| is_unsafe(cl, &c))
            .map(|(u, _)| u.as_str())
            .collect();
        assert_eq!(keep, vec!["a1", "c3", "e5"]);
    }

    #[test]
    fn registered_devices_are_read_by_last_user() {
        let devices = vec![
            json!({ "Id": "dev1", "AppName": "Jellyfin Android TV", "AppVersion": "0.19.10", "LastUserId": "AB-CD", "Capabilities": { "PlayableMediaTypes": [] } }),
            json!({ "Id": "dev2", "AppName": "Seerr", "LastUserId": "abcd" }),
            json!({ "Id": "dev3", "AppName": "Jellyfin Web" }),
        ];
        assert_eq!(
            device_clients(&devices),
            vec![
                ("abcd".to_string(), "Jellyfin Android TV".to_string()),
                ("abcd".to_string(), "Seerr".to_string()),
            ]
        );
    }

    /// Les 4 comptes VO de la production au 08/10 (anonymisés) + les cas de bord.
    fn library() -> (Vec<Value>, HashMap<String, BTreeSet<String>>) {
        let users = vec![
            user("u-web", "Web seul", Some("jpn"), "Always"),
            user("u-atv", "Android TV", Some("jpn"), "Always"),
            user("u-cast", "Chromecast", Some("jpn"), "Always"),
            user("u-old", "Ancien mode", Some(""), "Always"),
            user("u-new", "Sans lecture", Some("jpn"), "Always"),
            user("u-nat", "Déjà natif", Some("OriginalLanguage"), "Always"),
            user("u-fr", "Français", Some("fre"), "Smart"),
            user("u-eng", "Anglais", Some("eng"), "Default"),
            user("u-self", "Choix perso", Some("OriginalLanguage"), "Smart"),
        ];
        let mut clients = HashMap::new();
        clients.insert("uweb".into(), set(&["Jellyfin Web", "Jellyfin Desktop"]));
        clients.insert(
            "uatv".into(),
            set(&["Jellyfin Web", "Jellyfin Android TV", "Jellyfin for WebOS"]),
        );
        clients.insert("ucast".into(), set(&["Jellyfin Desktop", "Chromecast"]));
        clients.insert("uold".into(), set(&["Jellyfin iOS"]));
        clients.insert("unat".into(), set(&["Jellyfin Web", "JellyWatch TV"]));
        clients.insert("ufr".into(), set(&["Jellyfin Android TV"]));
        (users, clients)
    }

    #[test]
    fn native_plan_holds_every_account_with_an_unsafe_client() {
        let (users, clients) = library();
        let on = Accounts {
            vo_native: true,
            ..cfg()
        };
        let p = plan(&users, &clients, &on, Direction::Native);
        let by = |n: &str| p.iter().find(|s| s.name == n).unwrap().clone();
        // seuls les comptes en mode VO sont au plan
        assert_eq!(p.len(), 6);
        assert!(!p
            .iter()
            .any(|s| ["Français", "Anglais", "Choix perso"].contains(&s.name.as_str())));
        let s = by("Web seul");
        assert_eq!(
            (s.to.as_str(), s.outcome),
            (ORIGINAL_LANGUAGE, Outcome::Native)
        );
        assert!(s.changes());
        let s = by("Android TV");
        assert_eq!((s.to.as_str(), s.outcome), ("jpn", Outcome::Held));
        assert_eq!(s.clients, vec!["Jellyfin Android TV".to_string()]);
        assert!(!s.changes()); // gardé : exactement comme aujourd'hui
        assert_eq!(by("Chromecast").outcome, Outcome::Held);
        assert_eq!(by("Ancien mode").to, ORIGINAL_LANGUAGE);
        // un compte sans aucune lecture passe : la garde le rattrapera à sa première session hors liste
        assert_eq!(by("Sans lecture").outcome, Outcome::Native);
        // déjà en « Langue d'origine » mais lit maintenant par un client hors liste : retour à jpn
        let s = by("Déjà natif");
        assert_eq!(
            (s.from.as_str(), s.to.as_str(), s.outcome),
            (ORIGINAL_LANGUAGE, "jpn", Outcome::Held)
        );
        assert!(s.changes());
        // le plan ne dépend pas de l'interrupteur (c'est la migration réelle qui refuse)
        assert_eq!(plan(&users, &clients, &cfg(), Direction::Native), p);
    }

    #[test]
    fn compromise_is_a_config_choice() {
        let (users, clients) = library();
        let mut c = Accounts {
            vo_native: true,
            ..cfg()
        };
        c.vo_native_clients.push("Jellyfin Android TV".into());
        let p = plan(&users, &clients, &c, Direction::Native);
        let s = p.iter().find(|s| s.name == "Android TV").unwrap();
        assert_eq!(s.outcome, Outcome::Native);
        // le compte Chromecast reste gardé : le compromis ne vise que ce qui est ajouté à la liste
        assert_eq!(
            p.iter().find(|s| s.name == "Chromecast").unwrap().outcome,
            Outcome::Held
        );
    }

    #[test]
    fn classic_plan_reverts_only_native_accounts() {
        let (users, _) = library();
        let p = plan(&users, &HashMap::new(), &cfg(), Direction::Classic);
        let changed: Vec<_> = p.iter().filter(|s| s.changes()).collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(
            (changed[0].name.as_str(), changed[0].to.as_str()),
            ("Déjà natif", "jpn")
        );
        assert!(p.iter().all(|s| s.outcome == Outcome::Classic));
        // le compte en mode fr réglé à la main sur « Langue d'origine » n'est pas touché
        assert!(!p.iter().any(|s| s.name == "Choix perso"));
    }

    #[test]
    fn guard_holds_only_native_vo_accounts_seen_on_unsafe_clients() {
        let c = Accounts {
            vo_native: true,
            ..cfg()
        };
        let users = vec![
            user("n-1", "Natif TV", Some("OriginalLanguage"), "Always"),
            user("n-2", "Natif web", Some("OriginalLanguage"), "Always"),
            user("j-3", "Déjà jpn", Some("jpn"), "Always"),
            user("s-4", "Choix perso", Some("OriginalLanguage"), "Smart"),
            user("n-5", "Natif Seerr", Some("OriginalLanguage"), "Always"),
            user("n-6", "Natif appareil", Some("OriginalLanguage"), "Always"),
        ];
        // sessions au format de la prod : l'appli Android TV ouverte sans rien lire, sans capacités déclarées
        let sessions = vec![
            json!({ "UserId": "N1", "Client": "Jellyfin Android TV", "PlayableMediaTypes": [] }),
            json!({ "UserId": "n1", "Client": "Jellyfin Web", "PlayableMediaTypes": ["Audio", "Video"] }),
            json!({ "UserId": "n2", "Client": "Jellyfin Web", "NowPlayingItem": { "Id": "i" } }),
            json!({ "UserId": "j3", "Client": "Jellyfin Android TV", "PlayableMediaTypes": [] }),
            json!({ "UserId": "s4", "Client": "Jellyfin Android TV", "PlayableMediaTypes": [] }),
            json!({ "UserId": "n5", "Client": "Seerr", "PlayableMediaTypes": [] }),
        ];
        // n-6 : aucune session ouverte, mais l'appli Android TV est connectée sur son compte
        let devices = vec![
            json!({ "AppName": "Jellyfin Android TV", "LastUserId": "n6" }),
            json!({ "AppName": "Seerr", "LastUserId": "n5" }),
            json!({ "AppName": "Jellyfin Web", "LastUserId": "n2" }),
        ];
        let h = guard_holds(&sessions, &devices, &users, &c);
        assert_eq!(
            h,
            vec![
                Hold {
                    user_id: "n-6".into(),
                    name: "Natif appareil".into(),
                    clients: vec!["Jellyfin Android TV".into()]
                },
                Hold {
                    user_id: "n-1".into(),
                    name: "Natif TV".into(),
                    clients: vec!["Jellyfin Android TV".into()]
                },
            ]
        );
        // la même appli vue seulement par sa session (appareils illisibles ce passage) garde quand même le compte
        let h = guard_holds(&sessions, &[], &users, &c);
        assert_eq!(
            h.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            vec!["Natif TV"]
        );
        // compromis choisi par le propriétaire : l'appli Android TV dans la liste sûre, plus rien à garder
        let mut atv = c.clone();
        atv.vo_native_clients.push("Jellyfin Android TV".into());
        assert!(guard_holds(&sessions, &devices, &users, &atv).is_empty());
        assert!(is_native_vo(&users[0], &c) && !is_native_vo(&users[3], &c));
    }

    #[test]
    fn backup_holds_the_full_configuration_of_changed_accounts_only() {
        let (users, clients) = library();
        let p = plan(&users, &clients, &cfg(), Direction::Native);
        let todo: Vec<&Step> = p.iter().filter(|s| s.changes()).collect();
        let b = backup_body(&users, &todo, Direction::Native, 42);
        let acc = b["accounts"].as_array().unwrap();
        assert_eq!(acc.len(), todo.len());
        assert_eq!(b["direction"], "native");
        let web = acc.iter().find(|a| a["Name"] == "Web seul").unwrap();
        assert_eq!(web["Configuration"]["AudioLanguagePreference"], "jpn");
        assert_eq!(web["Configuration"]["SubtitleMode"], "Always");
        assert_eq!(web["planned"]["to"], ORIGINAL_LANGUAGE);
        assert_eq!(web["planned"]["outcome"], "native");
        assert!(!acc.iter().any(|a| a["Name"] == "Android TV"));

        let dir = tempfile::tempdir().unwrap();
        let path = write_backup(dir.path(), &b, "20261011-090000", Direction::Native).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back, b);
        // jamais écrasée
        assert!(write_backup(dir.path(), &b, "20261011-090000", Direction::Native).is_err());
    }

    #[test]
    fn exposure_mirrors_the_server_branch() {
        let a = |lang: &str, default: bool, original: bool| json!({ "Type": "Audio", "Language": lang, "IsDefault": default, "IsOriginal": original });
        let v = |name: &str, ol: Option<&str>, series: Option<&str>, audio: Vec<Value>| {
            let mut streams = vec![json!({ "Type": "Video" })];
            streams.extend(audio);
            json!({ "Name": name, "OriginalLanguage": ol, "SeriesId": series, "SeriesName": series.map(|_| "Série"),
                    "MediaStreams": streams })
        };
        let videos = vec![
            // animé hérité de la série, jpn marqué Original mais VF par défaut en tête : touché
            v(
                "E1",
                None,
                Some("S-1"),
                vec![a("fre", true, false), a("jpn", false, true)],
            ),
            // même fichier sans drapeau : non touché
            v(
                "E2",
                None,
                Some("S-1"),
                vec![a("fre", true, false), a("jpn", false, false)],
            ),
            // drapeau sur la piste par défaut (et première) : non touché
            v(
                "F1",
                Some("fr"),
                None,
                vec![a("fra", true, true), a("eng", false, false)],
            ),
            // anglais marqué Original, pas par défaut : touché (pas japonais)
            v(
                "F2",
                Some("en"),
                None,
                vec![a("fre", true, false), a("eng", false, true)],
            ),
            // drapeau sur une piste d'une autre langue que l'origine : le serveur passe par la langue (non touché)
            v(
                "F3",
                Some("it"),
                None,
                vec![
                    a("fre", true, false),
                    a("eng", false, true),
                    a("ita", false, false),
                ],
            ),
            // sans langue d'origine, piste japonaise sans drapeau : VF servie (à remplir)
            v(
                "A1",
                None,
                Some("S-2"),
                vec![a("fre", true, false), a("jpn", false, false)],
            ),
            // sans langue d'origine, drapeau Original non par défaut : branche fautive aussi
            v(
                "A2",
                None,
                None,
                vec![a("fre", true, false), a("jpn", false, true)],
            ),
            // origine « fr » écrite « fra » sur la piste : correspondance ISO
            v(
                "F4",
                Some("fr"),
                None,
                vec![a("eng", true, false), a("fra", false, true)],
            ),
        ];
        let mut series = HashMap::new();
        series.insert("s1".to_string(), "ja".to_string());
        series.insert("s2".to_string(), "".to_string());
        let e = exposure(&videos, &series);
        assert_eq!(e.videos, 8);
        assert_eq!(e.with_original_language, 6);
        assert_eq!(e.jpn_without_original_language, 2); // A1, A2
        assert_eq!(e.flagged, 4); // E1, F2, A2, F4
        assert_eq!(e.flagged_jpn, 2); // E1, A2
                                      // une série n'est citée qu'une fois ; un film par son titre
        assert_eq!(
            e.examples_flagged,
            vec![
                "Série [jpn]".to_string(),
                "F2 [eng]".into(),
                "A2 [jpn]".into(),
                "F4 [fra]".into()
            ]
        );
        assert_eq!(
            e.examples_jpn_without_original_language,
            vec!["Série".to_string(), "A2".to_string()]
        );
        assert_eq!(iso639_2(" DE "), vec!["ger", "deu", "de"]);
        assert_eq!(iso639_2("xx"), vec!["xx"]);
    }

    mod http {
        use super::*;
        use crate::Secret;
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, Request, ResponseTemplate};

        async fn jellyfin(server: &MockServer, users: Vec<Value>, rows: Vec<Vec<&str>>) {
            Mock::given(method("GET"))
                .and(path("/Users"))
                .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(users.clone())))
                .mount(server)
                .await;
            for u in &users {
                let id = u["Id"].as_str().unwrap();
                Mock::given(method("GET"))
                    .and(path(format!("/Users/{id}")))
                    .respond_with(ResponseTemplate::new(200).set_body_json(u.clone()))
                    .mount(server)
                    .await;
            }
            Mock::given(method("POST"))
                .and(path("/user_usage_stats/submit_custom_query"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "colums": ["UserId", "ClientName"],
                    "results": rows,
                })))
                .mount(server)
                .await;
            // format de la prod : l'appli Android TV ouverte sans rien lire n'a déclaré aucune capacité
            Mock::given(method("GET"))
                .and(path("/Sessions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                    { "UserId": "u3", "Client": "Jellyfin Android TV", "PlayableMediaTypes": [] },
                    { "UserId": "u1", "Client": "Seerr", "PlayableMediaTypes": [] }
                ])))
                .mount(server)
                .await;
            // u5 : appli Android TV connectée, jamais lue dans l'historique ni ouverte ; Seerr au nom de u1 (ignoré)
            Mock::given(method("GET"))
                .and(path("/Devices"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Items": [
                    { "Id": "d1", "AppName": "Jellyfin Android TV", "LastUserId": "U5" },
                    { "Id": "d2", "AppName": "Seerr", "LastUserId": "u1" },
                    { "Id": "d3", "AppName": "Jellyfin Web", "LastUserId": "u1" }
                ] })))
                .mount(server)
                .await;
            Mock::given(method("POST"))
                .and(path_regex(r"^/Users/[^/]+/Configuration$"))
                .respond_with(ResponseTemplate::new(204))
                .mount(server)
                .await;
        }

        fn client(server: &MockServer) -> JellyfinClient {
            JellyfinClient::new(
                &format!("{}/", server.uri()),
                Secret::new("cle-de-test".to_string()),
                reqwest::Client::new(),
            )
            .unwrap()
        }

        fn posted(reqs: &[Request]) -> Vec<(String, Value)> {
            reqs.iter()
                .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Configuration"))
                .map(|r| {
                    (
                        r.url.path().to_string(),
                        serde_json::from_slice(&r.body).unwrap(),
                    )
                })
                .collect()
        }

        fn accounts() -> Vec<Value> {
            let mut web = user("u1", "Web seul", Some("jpn"), "Always");
            // réglages du membre gardés tels quels (sauf la langue audio et la définition du mode VO)
            web["Configuration"]["SubtitleLanguagePreference"] = json!("fre");
            web["Configuration"]["RememberAudioSelections"] = json!(true);
            web["Configuration"]["DisplayMissingEpisodes"] = json!(true);
            vec![
                web,
                user("u2", "Chromecast", Some("jpn"), "Always"),
                user("u3", "Session TV", Some("jpn"), "Always"),
                user("u4", "Français", Some("fre"), "Smart"),
                user("u5", "Appareil TV", Some("jpn"), "Always"),
            ]
        }

        #[tokio::test]
        async fn migration_backs_up_then_switches_only_safe_accounts() {
            let server = MockServer::start().await;
            jellyfin(
                &server,
                accounts(),
                vec![
                    vec!["u1", "Jellyfin Web"],
                    vec!["u2", "Chromecast"],
                    vec!["u2", "Jellyfin Web"],
                    vec!["u3", "Jellyfin Web"],
                ],
            )
            .await;
            let jf = client(&server);
            let dir = tempfile::tempdir().unwrap();
            let state = StateStore::load(&dir.path().join("state.json")).unwrap();
            let on = Accounts {
                vo_native: true,
                ..Accounts::default()
            };

            // à blanc : plan seulement, rien d'écrit
            let dry = migrate(&jf, &on, dir.path(), &state, Direction::Native, true)
                .await
                .unwrap();
            assert!(dry.backup.is_none() && dry.applied.is_empty());
            assert!(posted(&server.received_requests().await.unwrap()).is_empty());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

            let m = migrate(&jf, &on, dir.path(), &state, Direction::Native, false)
                .await
                .unwrap();
            assert_eq!(m.applied, vec!["Web seul".to_string()]);
            assert!(m.errors.is_empty());
            // gardés sur jpn : u2 (Chromecast dans l'historique), u3 (appli Android TV ouverte sans capacités
            // déclarées) et u5 (appli Android TV seulement enregistrée) ; pas u1 pour Seerr
            let held: Vec<_> = m
                .steps
                .iter()
                .filter(|s| s.outcome == Outcome::Held)
                .map(|s| (s.name.as_str(), s.clients.clone()))
                .collect();
            assert_eq!(
                held,
                vec![
                    ("Appareil TV", vec!["Jellyfin Android TV".to_string()]),
                    ("Chromecast", vec!["Chromecast".to_string()]),
                    ("Session TV", vec!["Jellyfin Android TV".to_string()]),
                ]
            );
            let post = posted(&server.received_requests().await.unwrap());
            assert_eq!(post.len(), 1);
            assert_eq!(post[0].0, "/Users/u1/Configuration");
            let c = &post[0].1;
            assert_eq!(c["AudioLanguagePreference"], ORIGINAL_LANGUAGE);
            assert_eq!(c["PlayDefaultAudioTrack"], false);
            assert_eq!(c["RememberAudioSelections"], false);
            assert_eq!(c["SubtitleMode"], "Always");
            assert_eq!(c["DisplayMissingEpisodes"], true);
            // sauvegarde écrite AVANT, avec l'ancienne configuration complète
            let backup = m.backup.clone().unwrap();
            let b: Value =
                serde_json::from_str(&std::fs::read_to_string(&backup).unwrap()).unwrap();
            assert_eq!(
                b["accounts"][0]["Configuration"]["AudioLanguagePreference"],
                "jpn"
            );
            assert_eq!(
                b["accounts"][0]["Configuration"]["RememberAudioSelections"],
                true
            );
            // noté dans l'état
            let rec = state
                .read(|s| s.vo_native.get("u1").cloned())
                .await
                .unwrap();
            assert_eq!(
                (
                    rec.source.as_str(),
                    rec.outcome.as_str(),
                    rec.old.as_str(),
                    rec.new.as_str()
                ),
                ("migration", "native", "jpn", ORIGINAL_LANGUAGE)
            );
        }

        #[tokio::test]
        async fn native_migration_is_refused_while_the_switch_is_off() {
            let server = MockServer::start().await;
            jellyfin(&server, accounts(), vec![]).await;
            let jf = client(&server);
            let dir = tempfile::tempdir().unwrap();
            let state = StateStore::load(&dir.path().join("state.json")).unwrap();
            let off = Accounts::default();
            assert!(
                migrate(&jf, &off, dir.path(), &state, Direction::Native, false)
                    .await
                    .is_err()
            );
            assert!(server.received_requests().await.unwrap().is_empty());
            // à blanc, le plan se lit quand même (avec l'avertissement)
            let dry = migrate(&jf, &off, dir.path(), &state, Direction::Native, true)
                .await
                .unwrap();
            assert!(dry.warning.is_some());
        }

        #[tokio::test]
        async fn unreadable_history_aborts_the_native_migration() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/Users"))
                .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(accounts())))
                .mount(&server)
                .await;
            // Playback Reporting absent (instance sans l'extension) : 404
            Mock::given(method("POST"))
                .and(path("/user_usage_stats/submit_custom_query"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            let jf = client(&server);
            let dir = tempfile::tempdir().unwrap();
            let state = StateStore::load(&dir.path().join("state.json")).unwrap();
            let on = Accounts {
                vo_native: true,
                ..Accounts::default()
            };
            assert!(
                migrate(&jf, &on, dir.path(), &state, Direction::Native, false)
                    .await
                    .is_err()
            );
            assert!(posted(&server.received_requests().await.unwrap()).is_empty());
            // et une décision de Mon compte garde le compte
            let d = decide(&jf, &on, "u1", Mode::Vo).await;
            assert_eq!(d.outcome, Some(Outcome::Held));
            assert_eq!(d.prefs.audio, "jpn");
            assert_eq!(d.clients, vec![UNREADABLE.to_string()]);
        }

        #[tokio::test]
        async fn unreadable_devices_hold_the_account_and_abort_the_migration() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/Users"))
                .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(accounts())))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/user_usage_stats/submit_custom_query"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "colums": ["UserId", "ClientName"],
                    "results": [["u1", "Jellyfin Web"]],
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/Sessions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/Devices"))
                .respond_with(ResponseTemplate::new(403))
                .mount(&server)
                .await;
            let jf = client(&server);
            let dir = tempfile::tempdir().unwrap();
            let state = StateStore::load(&dir.path().join("state.json")).unwrap();
            let on = Accounts {
                vo_native: true,
                ..Accounts::default()
            };
            assert!(
                migrate(&jf, &on, dir.path(), &state, Direction::Native, false)
                    .await
                    .is_err()
            );
            assert!(posted(&server.received_requests().await.unwrap()).is_empty());
            let d = decide(&jf, &on, "u1", Mode::Vo).await;
            assert_eq!(
                (d.prefs.audio.as_str(), d.outcome),
                ("jpn", Some(Outcome::Held))
            );
            assert_eq!(d.clients, vec![UNREADABLE.to_string()]);
        }

        #[tokio::test]
        async fn classic_migration_reverts_native_accounts() {
            let server = MockServer::start().await;
            let mut users = accounts();
            users[0]["Configuration"]["AudioLanguagePreference"] = json!(ORIGINAL_LANGUAGE);
            jellyfin(&server, users, vec![]).await;
            let jf = client(&server);
            let dir = tempfile::tempdir().unwrap();
            let state = StateStore::load(&dir.path().join("state.json")).unwrap();
            let m = migrate(
                &jf,
                &Accounts::default(),
                dir.path(),
                &state,
                Direction::Classic,
                false,
            )
            .await
            .unwrap();
            assert_eq!(m.applied, vec!["Web seul".to_string()]);
            let post = posted(&server.received_requests().await.unwrap());
            assert_eq!(post.len(), 1);
            assert_eq!(post[0].1["AudioLanguagePreference"], "jpn");
            let rec = state
                .read(|s| s.vo_native.get("u1").cloned())
                .await
                .unwrap();
            assert_eq!(
                (rec.source.as_str(), rec.outcome.as_str()),
                ("retour", "classic")
            );
            // le retour ne lit pas l'historique de lecture
            assert!(!server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().contains("user_usage_stats")));
        }

        #[tokio::test]
        async fn decision_for_one_account() {
            let server = MockServer::start().await;
            jellyfin(
                &server,
                accounts(),
                vec![vec!["u1", "Jellyfin Web"], vec!["u2", "Chromecast"]],
            )
            .await;
            let jf = client(&server);
            let on = Accounts {
                vo_native: true,
                ..Accounts::default()
            };
            let d = decide(&jf, &on, "u1", Mode::Vo).await;
            assert_eq!(
                (d.prefs.audio.as_str(), d.outcome),
                (ORIGINAL_LANGUAGE, Some(Outcome::Native))
            );
            assert_eq!(d.prefs.subtitle_mode, "Always");
            let d = decide(&jf, &on, "u2", Mode::Vo).await;
            assert_eq!(
                (d.prefs.audio.as_str(), d.outcome),
                ("jpn", Some(Outcome::Held))
            );
            assert_eq!(d.clients, vec!["Chromecast".to_string()]);
            // ni lecture ni session : son appli Android TV enregistrée suffit à le garder
            let d = decide(&jf, &on, "u5", Mode::Vo).await;
            assert_eq!(
                (d.prefs.audio.as_str(), d.outcome, d.clients),
                (
                    "jpn",
                    Some(Outcome::Held),
                    vec!["Jellyfin Android TV".to_string()]
                )
            );
            // mode fr, ou bascule coupée : aucune lecture de l'historique, préférences d'aujourd'hui
            let before = server.received_requests().await.unwrap().len();
            let d = decide(&jf, &on, "u1", Mode::Fr).await;
            assert_eq!((d.prefs.audio.as_str(), d.outcome), ("fre", None));
            let d = decide(&jf, &Accounts::default(), "u1", Mode::Vo).await;
            assert_eq!((d.prefs.audio.as_str(), d.outcome), ("jpn", None));
            assert_eq!(server.received_requests().await.unwrap().len(), before);
        }

        #[tokio::test]
        async fn set_vo_audio_changes_only_the_vo_fields() {
            let server = MockServer::start().await;
            jellyfin(&server, accounts(), vec![]).await;
            let jf = client(&server);
            jf.set_vo_audio("u1", ORIGINAL_LANGUAGE).await.unwrap();
            let post = posted(&server.received_requests().await.unwrap());
            let mut want = accounts()[0]["Configuration"].clone();
            want["AudioLanguagePreference"] = json!(ORIGINAL_LANGUAGE);
            want["PlayDefaultAudioTrack"] = json!(false);
            want["RememberAudioSelections"] = json!(false);
            want["RememberSubtitleSelections"] = json!(false);
            assert_eq!(post, vec![("/Users/u1/Configuration".to_string(), want)]);
            // en-tête de la clé : jamais dans l'URL
            let reqs = server.received_requests().await.unwrap();
            assert!(reqs.iter().all(|r| !r.url.as_str().contains("cle-de-test")));
        }
    }
}
