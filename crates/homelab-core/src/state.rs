use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// État persistant du daemon (remplace les fichiers `.state` TSV des scripts).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    /// Items de queue Arr bloqués, clé `service:downloadId`.
    #[serde(default)]
    pub stuck: BTreeMap<String, StuckEntry>,
    /// Emails déjà traités par le poller Jellyseerr.
    #[serde(default)]
    pub onboarded: BTreeMap<String, OnboardRecord>,
    #[serde(default)]
    pub task_runs: BTreeMap<String, RunInfo>,
    /// stack_health : dernier redémarrage forcé par service (cooldown).
    #[serde(default)]
    pub restarts: BTreeMap<String, i64>,
    /// stack_health : depuis quand un service est `unhealthy`.
    #[serde(default)]
    pub unhealthy_since: BTreeMap<String, i64>,
    /// seedbox_refresh : dernier id d'historique d'import traité, par Arr.
    #[serde(default)]
    pub seedbox_history: BTreeMap<String, i64>,
    /// subtitle_sync : horodatage (secondes, heure locale de Bazarr) de la dernière ligne d'historique
    /// Bazarr traitée, par liste (`episodes`, `movies`).
    #[serde(default)]
    pub bazarr_history: BTreeMap<String, i64>,
    /// torrent_import : décision par torrent, clé `côté:hash` (`vps:…`, `seedbox:…`).
    #[serde(default)]
    pub torrent_import: BTreeMap<String, TorrentImportRecord>,
    /// series_search (ex-unknown_series_grab, nom de champ gardé pour l'historique) : dernière recherche par
    /// saison, clé `arr:série:saison`.
    #[serde(default)]
    pub unknown_series: BTreeMap<String, SeasonSearchRecord>,
    /// movie_search : dernière recherche par film, clé `arr:film`.
    #[serde(default)]
    pub movie_search: BTreeMap<String, SeasonSearchRecord>,
    /// indexer_unblock : déblocages effectués, clé `application:id indexeur` → dates (secondes).
    #[serde(default)]
    pub indexer_unblocks: BTreeMap<String, Vec<i64>>,
    /// indexer_unblock : dernier arrêt/redémarrage par application, et dernière alerte par indexeur.
    #[serde(default)]
    pub indexer_app_restarts: BTreeMap<String, i64>,
    #[serde(default)]
    pub indexer_alerts: BTreeMap<String, i64>,
    /// seedbox_health : service seedbox → premier échec constaté (absent = répond).
    #[serde(default)]
    pub seedbox_down: BTreeMap<String, i64>,
    /// seedbox_health : pannes déjà signalées (service → date de l'alerte).
    #[serde(default)]
    pub seedbox_alerted: BTreeMap<String, i64>,
    /// anime_library : classement TMDB en cache, clé `tv:<tmdb>` / `movie:<tmdb>` → (date, classe).
    #[serde(default)]
    pub anime_class: BTreeMap<String, AnimeClassRecord>,
    /// anime_library : fiches déplacées, clé `côté:movie:<id>` / `côté:series:<id>` → date ;
    /// deletion_cleanup les laisse tranquilles quelques heures.
    #[serde(default)]
    pub anime_moves: BTreeMap<String, i64>,
    /// anime_library : dernière recherche lancée dans l'Arr (RuTracker) pour une fiche russe sans fichier, clé
    /// `seedbox:movie:<id>` / `seedbox:series:<id>` → date.
    #[serde(default)]
    pub russian_searches: BTreeMap<String, i64>,
    /// russian_search : dernière recherche RuTracker par titre original et torrent pris, clé
    /// `seedbox:movie:<id>` / `seedbox:series:<id>`.
    #[serde(default)]
    pub russian_title: BTreeMap<String, RussianTitleRecord>,
    /// Dates (secondes) des requêtes envoyées à l'indexer, **par clé** : plafond horaire commun aux
    /// tâches et à la page /recherche.
    /// (Nouveau format : l'ancien champ `c411_queries`, une simple liste, est ignoré.)
    #[serde(default)]
    pub c411_key_queries: BTreeMap<String, Vec<i64>>,
    /// Clés mises de côté après un 429, jusqu'à cette date.
    #[serde(default)]
    pub c411_cooldowns: BTreeMap<String, i64>,
    /// Comptes suspendus : permissions Jellyseerr à restaurer, clé = id Jellyfin.
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountRecord>,
    /// deletion_cleanup : suppressions constatées et torrents en attente de retrait.
    #[serde(default)]
    pub deletions: Deletions,
    /// Liens de bienvenue (page où le membre définit son mot de passe), clé = SHA-256 hex du jeton.
    #[serde(default)]
    pub welcome_links: BTreeMap<String, WelcomeLink>,
    /// Dernier résultat du canari de lecture (`tasks::playback_canary`).
    #[serde(default)]
    pub canary: CanaryState,
    /// Dernier rapport « catalogue jamais regardé » (`tasks::catalogue_report`), affiché sur `/status.html`.
    #[serde(default)]
    pub catalogue: Option<CatalogueReport>,
    /// Liste rouge (`tasks::red_list`, page `/accounts/liste-rouge`, 2026-10-20) : dernier calcul, exemptions, horloges
    /// « vu sans date », travaux « Nettoyer » et étapes à rejouer.
    #[serde(default)]
    pub red_list: RedListState,
    /// identity_check : dernière tentative de renommage d'une fiche au nom de release (id Jellyfin → date),
    /// pour ne pas réessayer sans fin un titre que TMDB nomme ainsi.
    #[serde(default)]
    pub renamed_items: BTreeMap<String, i64>,
    /// original_language : dernier passage par fiche Jellyfin (id → écriture ou essai), avec la valeur d'avant et la
    /// valeur écrite pour pouvoir revenir en arrière (2026-10-08).
    #[serde(default)]
    pub original_language: BTreeMap<String, OriginalLanguageRecord>,
    /// Mode VO « Langue d'origine » (`vo_native`) : dernier changement de langue audio fait par homelabd sur un compte
    /// (id Jellyfin compact → avant, après, origine, clients hors liste vus), 2026-10-08.
    #[serde(default)]
    pub vo_native: BTreeMap<String, VoNativeRecord>,
    /// subtitle_sync : dernier essai par item Jellyfin (id → essai). Sans ça, un item que Jellyfin ne liste pas
    /// encore était repris toutes les 5 min (22/09 : 7 700 appels ssh et 6 000 rafraîchissements pour rien).
    #[serde(default)]
    pub subtitle_tries: BTreeMap<String, SubtitleTry>,
    /// Dates (secondes) des inscriptions par la page publique : plafond journalier.
    #[serde(default)]
    pub signups: Vec<i64>,
    /// Dates des renvois de lien par adresse (clé = SHA-256 hex de l'adresse) : plafond horaire.
    #[serde(default)]
    pub link_renewals: BTreeMap<String, Vec<i64>>,
    /// Seuils de capacité franchis et déjà signalés (`disque_vps`, `quota_seedbox`) → date de l'alerte ; la clé
    /// disparaît quand la valeur repasse sous le seuil (voir `alerts::crossing`) : une alerte par franchissement.
    #[serde(default)]
    pub capacity_alerts: BTreeMap<String, i64>,
    /// stack_health : dernière alerte « relance en échec » par service (au plus une par `alert_every_secs`).
    #[serde(default)]
    pub stack_failures: BTreeMap<String, i64>,
    /// Alertes admin passées par `alerts::admin` : compteurs et dernières alertes (preuve de livraison).
    #[serde(default)]
    pub alerts: AlertStats,
    /// hls_loop_watch : titres (identifiant Jellyfin compact) dont la rafale de ffmpeg est déjà signalée → date de
    /// l'alerte ; la clé disparaît quand leur compteur horaire retombe sous le seuil, la rafale suivante alertera.
    #[serde(default)]
    pub burst_alerts: BTreeMap<String, i64>,
    /// Surveillances quotidiennes (`cert_watch`, `backup_watch`, `diun_watch`) : dernière alerte partie, par tâche.
    /// Ces tâches tournent aussi à chaque démarrage de homelabd : sans cette mémoire, chaque redémarrage réalertait le
    /// même défaut (voir `alerts::watch_due`). L'entrée disparaît au retour à la normale.
    #[serde(default)]
    pub watch_alerts: BTreeMap<String, WatchAlert>,
    /// media_mail : mails « disponible » des demandes Jellyseerr, clé `r<id demande>` (film) ou
    /// `r<id demande>:s<saison>` (série) → suite donnée (2026-10-15). Une clé présente n'est jamais réévaluée, sauf
    /// en attente (`pending`) ou en échec d'envoi (`error`) : c'est ce qui empêche un second mail.
    #[serde(default)]
    pub media_mail: BTreeMap<String, MediaMailRecord>,
    /// media_mail : date du premier passage. Ce passage-là note tout ce qui était déjà disponible (`baseline`) et
    /// n'envoie rien ; absent = le prochain passage fait la référence.
    #[serde(default)]
    pub media_mail_baseline_at: Option<i64>,
    /// media_mail : dernier passage du daemon sur cet état (date, notée même si Jellyseerr ne répond pas) et envoi aux
    /// membres ou non (noté avec les nouvelles clés). Un écart trop long (état restauré, daemon longtemps arrêté) et la
    /// bascule vers l'envoi se lisent ici.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_mail_last_pass: Option<MediaMailPass>,
    /// media_mail : lignes de l'alerte admin « mails abandonnés » pas encore parties, clé(s) abandonnée(s) → ligne
    /// (2026-10-15). Notées dans la même écriture que l'abandon (`gave_up`), effacées seulement quand l'alerte est
    /// partie (`alerts::retry_later`) : un échec d'envoi, un passage interrompu ou un redémarrage ne la perdent pas.
    /// Absent tant que rien n'attend (un ancien binaire ignore le champ : aucun `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub media_mail_alerts: BTreeMap<String, String>,
}

/// Dernier passage de `media_mail` (voir `tasks::media_mail::continuity`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaMailPass {
    pub at: i64,
    /// `send_to_members` valait `true` au dernier passage qui a noté de nouvelles clés.
    pub send: bool,
    /// Date du passage de bascule vers l'envoi (premier passage avec envoi après des passages sans), gardée tant que
    /// l'envoi reste allumé (2026-10-15) : une clé disponible avant lui mais absente de sa lecture est traitée comme
    /// s'il l'avait vue (`tasks::media_mail::Continuity::switched_at`). Absente sinon (un ancien binaire ignore le
    /// champ : aucun `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_since: Option<i64>,
}

/// Une demande (film) ou une saison demandée, devenue disponible, et la suite donnée (`tasks::media_mail`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaMailRecord {
    /// Dernier changement de `outcome`.
    pub at: i64,
    /// Premier passage qui l'a vue disponible (horloge de homelabd) : sert au regroupement.
    pub seen_at: i64,
    /// Passage à « disponible » selon Jellyseerr (fin de la demande, ou de la saison).
    pub ready_at: i64,
    /// Demandeur : id du compte Jellyseerr.
    pub user: i64,
    /// `movie` ou `tv`.
    pub kind: String,
    pub tmdb: i64,
    /// En cours : `pending` (à envoyer), `error` (envoi en échec, nouvel essai à `next_at`), `sending` (envoi parti,
    /// réponse pas encore notée). Définitifs : `baseline` (déjà disponible au premier passage), `stale` (disponible
    /// depuis plus de `max_age_hours`), `logged` (envoi aux membres coupé : noté, jamais envoyé), `sent`,
    /// `sent_uncertain` (passage interrompu pendant l'envoi, ou en attente avant un écart de l'état : jamais renvoyé),
    /// `no_email`, `no_account`, `suspended`, `opted_out`, `not_visible` (le compte ne voit pas le titre dans
    /// Jellyfin), `gone` (demande retirée), `gave_up` (échecs d'envoi répétés, ou plus de `max_age_hours` sans pouvoir partir : alerte admin
    /// gardée dans `State::media_mail_alerts` jusqu'à ce qu'elle parte).
    pub outcome: String,
    /// Échecs d'envoi.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<i64>,
}

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Dernière alerte d'une surveillance quotidienne : date et empreinte du défaut signalé (`alerts::fingerprint`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchAlert {
    pub at: i64,
    pub key: String,
}

/// Langue audio d'un compte en mode VO posée par homelabd (`vo_native`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoNativeRecord {
    pub at: i64,
    pub name: String,
    /// `migration` (`homelabctl accounts vo-native`), `retour` (`vo-classic`), `compte` (Mon compte), `onboarding`,
    /// `garde` (tâche `vo_native_guard`).
    pub source: String,
    /// `native` (« Langue d'origine »), `held` (gardé sur `vo_audio_language` : client hors liste), `classic` (retour).
    pub outcome: String,
    /// `AudioLanguagePreference` avant et après.
    #[serde(default)]
    pub old: String,
    #[serde(default)]
    pub new: String,
    /// Clients hors de `vo_native_clients` qui ont décidé `held`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clients: Vec<String>,
}

/// Passage de `original_language` sur une fiche.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginalLanguageRecord {
    pub at: i64,
    /// `movie` ou `series`.
    pub kind: String,
    pub name: String,
    pub tmdb: i64,
    /// `written` (écrite et relue), `unknown` (TMDB sans langue utilisable), `locked` (fiche verrouillée),
    /// `children_differ` (série dont une saison ou un épisode a sa propre classification, que Jellyfin effacerait),
    /// `error` (lecture ou écriture en échec). `written` n'est **jamais** réessayé d'office (une fiche remise à vide
    /// par un retour arrière le reste), ni `error` quand l'écriture est partie (`posted`) ; les autres le sont après
    /// `retry_hours` (2026-10-08).
    pub outcome: String,
    /// OriginalLanguage avant écriture (vide = aucune) et valeur écrite : revenir en arrière = réécrire `old`.
    #[serde(default)]
    pub old: String,
    #[serde(default)]
    pub new: String,
    /// La requête d'écriture est partie et Jellyfin a pu l'appliquer (réponse reçue sans refus net, délai dépassé,
    /// coupure en route) : vrai pour toute fiche `written`, et pour un `error` dont on ne sait pas s'il a été appliqué.
    /// Un tel `error` passe en `written` dès qu'un passage relit la langue écrite, et n'est jamais réessayé d'office.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub posted: bool,
    /// Série : saisons et épisodes non verrouillés dont la classification était vide avant l'écriture (ids
    /// compacts) ; Jellyfin leur a donné celle de la série (`children_rating`). Revenir en arrière = remettre leur
    /// classification à vide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children_unrated: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub children_rating: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeClassRecord {
    pub at: i64,
    /// `anime`, `not_anime` ou `unknown`.
    pub class: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Deletions {
    /// Première absence constatée d'un fichier, clé `côté:chemin Arr`.
    #[serde(default)]
    pub first_seen: BTreeMap<String, i64>,
    /// Saisons supprimées, clé `côté:tvdb:saison` → date : monitor_sync ne les re-surveille pas
    /// (sauf demande Jellyseerr plus récente).
    #[serde(default)]
    pub seasons: BTreeMap<String, i64>,
    /// Torrents C411 à retirer une fois le seuil de seed atteint, clé `côté:hash`.
    #[serde(default)]
    pub pending_torrents: BTreeMap<String, PendingTorrent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTorrent {
    pub at: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub at: i64,
    pub jellyseerr_id: i64,
    /// Permissions Jellyseerr d'avant la suspension (remises à 0 pendant la suspension).
    pub jellyseerr_permissions: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeasonSearchRecord {
    pub at: i64,
    /// `grabbed`, `grabbed_episode`, `none`, `vo_wait`, `error`, `fallback_none`.
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    /// Titre de la série, pour l'afficher sur `/status.html` sans réinterroger l'Arr.
    #[serde(default)]
    pub title: String,
    /// Épisodes manquants qu'aucune release ne couvre, au dernier passage. Une saison qui en garde
    /// est un blocage durable : l'indexer n'a rien, la recherche repartira pour rien.
    #[serde(default)]
    pub uncovered: Vec<i64>,
    /// Diffusion du dernier épisode manquant au moment du passage (`series_search`, RFC 3339) : l'onglet Demandes
    /// en déduit la fenêtre de sortie (reprise rapide). Vide pour un film ou un état d'avant 1.30.4 ; un ancien
    /// binaire ignore le champ (aucun `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_air: String,
    /// Dernier passage **complet** (identifiant puis texte libre) : pendant la fenêtre de sortie, les reprises qui
    /// suivent un passage complet postérieur à la diffusion n'interrogent que l'identifiant. 0 = inconnu.
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub full_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TorrentImportRecord {
    pub at: i64,
    pub name: String,
    /// `imported`, `arr_managed`, `already_linked`, `no_video`, `no_match`, `dup_other_side`,
    /// `nothing_importable`, `error` (définitifs) ; `retry` (nouvel essai au passage suivant) ; `partial` (des vidéos
    /// laissées de côté, réexaminé toutes les 6 h tant que `attempts < max_attempts`, puis définitif).
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub attempts: u32,
    /// Vidéos laissées de côté d'un import `partial` (« fichier: raison », 40 au plus), hors extraits, bonus et
    /// épisodes déjà présents. Absent de l'état quand la liste est vide : un ancien binaire relit l'état sans peine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<String>,
    /// Alerte « import partiel » pas encore partie : redonnée à chaque passage tant qu'aucun canal ne l'a prise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alert_pending: bool,
}

impl TorrentImportRecord {
    pub fn is_final(&self) -> bool {
        self.outcome != "retry"
    }
}

/// Voie russe : recherche par titre original et suite donnée.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RussianTitleRecord {
    pub at: i64,
    /// `none`, `grabbed`, `imported`, `error`.
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    /// Torrent pris (minuscules), en attente d'import tant que `outcome = grabbed`.
    #[serde(default)]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StuckEntry {
    pub service: String,
    pub download_id: String,
    pub first_seen: i64,
    pub title: String,
}

/// Un lien de bienvenue : `welcome` (définir son mot de passe), `activated` (compte activé, même page si le
/// mot de passe n'est pas encore défini), `demo` (mail de test : page d'exemple, aucun compte).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WelcomeLink {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub kind: String,
    pub created_at: i64,
    pub expires_at: i64,
    #[serde(default)]
    pub used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardRecord {
    pub at: i64,
    pub outcome: String,
}

/// Dernier essai d'extraction de sous-titres pour un item (`tasks::subtitle_sync`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubtitleTry {
    pub at: i64,
    /// Aucune piste extractible (code 3 du script) : on ne retente qu'une fois par `failed_retry_days`.
    #[serde(default)]
    pub no_track: bool,
}

/// État du canari de lecture.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanaryState {
    pub last_run: i64,
    pub last_ok: Option<bool>,
    pub last_ok_at: i64,
    pub failures: u32,
    /// Ce qui a été testé (`vps` / `seedbox`) et le temps du premier segment, ou l'erreur.
    pub last_detail: String,
    /// Passages effectués (alternance VPS / seedbox).
    #[serde(default)]
    pub runs: u64,
    /// Items Jellyfin choisis une fois pour toutes (petits fichiers), par côté.
    #[serde(default)]
    pub items: BTreeMap<String, String>,
}

/// Nombre de titres et volume (octets) d'un groupe, pour le rapport « catalogue jamais regardé ».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueBucket {
    #[serde(default)]
    pub titles: u32,
    #[serde(default)]
    pub bytes: u64,
}

/// Un titre du rapport (jamais de pseudo : le demandeur n'est qu'une sorte).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueEntry {
    /// Texte brut tiré de l'Arr (échappé à l'affichage), 60 caractères au plus.
    pub title: String,
    /// `film`, `série` ou `animé`.
    pub kind: String,
    /// `vps` ou `seedbox`.
    pub side: String,
    pub bytes: u64,
    /// Arrivée d'après l'Arr (secondes).
    pub added: i64,
    /// `membre`, `admin`, `aucune` (aucune demande) ou `inconnu` (Jellyseerr injoignable, ou fiche sans identifiant TMDB).
    pub requester: String,
    /// Épisodes vus sur épisodes présents (0 et 0 pour un film).
    #[serde(default)]
    pub seen: u32,
    #[serde(default)]
    pub total: u32,
}

/// Séries commencées mais pas finies : l'arriéré normal des séries en cours, à ne pas confondre avec du poids mort.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueBacklog {
    #[serde(default)]
    pub series: u32,
    #[serde(default)]
    pub unseen_episodes: u32,
    /// Volume **estimé** des épisodes non vus (taille de la série au prorata).
    #[serde(default)]
    pub bytes: u64,
}

/// Voie russe : ce qui est disponible et ce qui a été lu (un titre russe n'est évalué par l'âge qu'après `min_age_days`,
/// cette ligne le montre dès l'arrivée).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueRussian {
    #[serde(default)]
    pub series: u32,
    #[serde(default)]
    pub movies: u32,
    #[serde(default)]
    pub episodes: u32,
    #[serde(default)]
    pub episodes_seen: u32,
    #[serde(default)]
    pub movies_seen: u32,
    #[serde(default)]
    pub bytes: u64,
}

/// Répartition d'un ensemble de titres jamais commencés : par sorte (`film`, `série`, `animé`), côté (`vps`, `seedbox`)
/// et sorte de demandeur (`membre`, `admin`, `aucune`, `inconnu`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueSplit {
    #[serde(default)]
    pub by_kind: BTreeMap<String, CatalogueBucket>,
    #[serde(default)]
    pub by_side: BTreeMap<String, CatalogueBucket>,
    #[serde(default)]
    pub by_requester: BTreeMap<String, CatalogueBucket>,
}

impl CatalogueSplit {
    pub fn add(&mut self, kind: &str, side: &str, requester: &str, bytes: u64) {
        for (map, key) in [
            (&mut self.by_kind, kind),
            (&mut self.by_side, side),
            (&mut self.by_requester, requester),
        ] {
            let b = map.entry(key.to_string()).or_default();
            b.titles += 1;
            b.bytes += bytes;
        }
    }
}

/// Dernier rapport de `tasks::catalogue_report` (2026-10-08, lecture seule : l'admin tranche, rien n'est supprimé).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueReport {
    /// Date du calcul (secondes).
    pub at: i64,
    pub min_age_days: i64,
    /// Tout ce qui a des fichiers et a pu être rapproché de Jellyfin.
    pub catalogue: CatalogueBucket,
    /// Fiches Arr avec fichiers que Jellyfin ne montre pas sous le même chemin ni le même identifiant : non évaluées.
    pub unmatched: CatalogueBucket,
    /// Jamais commencés, quel que soit leur âge.
    pub never_all: CatalogueBucket,
    /// Jamais commencés et arrivés depuis au moins `min_age_days` : la liste de décision.
    pub never_aged: CatalogueBucket,
    /// Répartition de `never_aged` (la liste de décision) et de `never_all` (tous âges : c'est elle qui renseigne tant que
    /// peu de titres ont atteint le seuil).
    #[serde(default)]
    pub aged: CatalogueSplit,
    #[serde(default)]
    pub all: CatalogueSplit,
    #[serde(default)]
    pub backlog: CatalogueBacklog,
    /// Quand le prochain titre jamais vu atteindra `min_age_days` (rien si tous l'ont atteint).
    #[serde(default)]
    pub next_aged_at: Option<i64>,
    /// Playback Reporting a répondu (sinon seuls les marqueurs « vu » et « en cours » des comptes comptent).
    #[serde(default)]
    pub playback_reporting: bool,
    #[serde(default)]
    pub russian: CatalogueRussian,
    /// Les plus gros titres de `never_aged`, bornés par `max_listed`.
    #[serde(default)]
    pub listed: Vec<CatalogueEntry>,
    /// Les séries commencées qui ont le plus d'épisodes non vus (volume estimé), bornées par `max_backlog_listed`.
    #[serde(default)]
    pub backlog_listed: Vec<CatalogueEntry>,
}

/// Liste rouge (`tasks::red_list`, 2026-10-20). Tout est en `#[serde(default)]` : un état écrit avant elle se relit, et
/// un ancien binaire ignore le champ (aucun `deny_unknown_fields`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedListState {
    /// Dernier calcul (tâche quotidienne, « Recalculer », ou calcul complet au début d'un « Nettoyer »).
    #[serde(default)]
    pub report: Option<RedListReport>,
    /// Titres exemptés par l'admin, par clé stable : permanents, jusqu'à « Retirer l'exemption ».
    #[serde(default)]
    pub exemptions: BTreeMap<String, Exemption>,
    /// Vu sans date (marqué vu à la main : ni `LastPlayedDate` ni ligne datée de Playback Reporting), clé stable →
    /// première observation. L'horloge tient lieu de date de lecture ; la clé disparaît dès qu'une date apparaît ou que
    /// le marqueur s'en va.
    #[serde(default)]
    pub undated_since: BTreeMap<String, i64>,
    /// Travaux « Nettoyer », du plus ancien au plus récent (`jobs_kept` au plus).
    #[serde(default)]
    pub jobs: Vec<CleanJob>,
    /// Étapes d'un nettoyage à rejouer (corbeille de l'Arr, média Seerr), clé `<étape>:<côté>:<clé du titre>`.
    #[serde(default)]
    pub pending: BTreeMap<String, PendingStep>,
}

/// Exemption d'un titre de la liste rouge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exemption {
    pub at: i64,
    /// Titre au moment de l'exemption (texte brut, échappé à l'affichage) : listé même si le titre a disparu.
    pub title: String,
}

/// Un calcul de la liste rouge (`tasks::red_list::build_red_list`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedListReport {
    /// Date du calcul (secondes) : un titre dont `due <= at` est rouge.
    pub at: i64,
    pub min_idle_days: i64,
    #[serde(default)]
    pub soon_days: i64,
    /// Playback Reporting a répondu et chaque compte a été lu. Sinon une lecture a pu échapper au calcul :
    /// « Nettoyer » est refusé.
    pub complete: bool,
    /// Ce qui a manqué (`complete = false`), en clair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Titres évalués (rapprochés de Jellyfin, avec identifiant et date d'arrivée).
    #[serde(default)]
    pub evaluated: u32,
    /// Rouges et non exemptés, les plus gros d'abord.
    #[serde(default)]
    pub entries: Vec<RedEntry>,
    /// Pas encore rouges, mais le deviendront dans `soon_days` jours au plus (le plus proche d'abord).
    #[serde(default)]
    pub soon: Vec<RedEntry>,
    /// Exemptés présents au catalogue, rouges ou non (par titre).
    #[serde(default)]
    pub exempted: Vec<RedEntry>,
    /// Non évalués, jamais rouges : absents de Jellyfin, sans identifiant TMDB/TVDB, sans date d'arrivée lisible,
    /// fichiers d'épisodes illisibles (motif dans `reason`).
    #[serde(default)]
    pub unmatched: Vec<RedEntry>,
}

/// Un titre de la liste rouge (jamais de pseudo : le demandeur n'est qu'une sorte).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedEntry {
    /// Clé stable : `movie:tmdb:<id>`, `series:tvdb:<id>`, à défaut `series:tmdb:<id>` ; vide pour un titre sans
    /// identifiant (jamais rouge).
    pub key: String,
    /// Texte brut tiré de l'Arr (échappé à l'affichage), 60 caractères au plus.
    pub title: String,
    /// `film`, `série` ou `animé`.
    pub kind: String,
    pub movie: bool,
    #[serde(default)]
    pub tmdb: i64,
    #[serde(default)]
    pub tvdb: i64,
    /// Voie russe : étiquette affichée.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub russian: bool,
    /// Une copie par fiche d'Arr : un titre présent sur le VPS et la seedbox est une seule entrée à deux côtés.
    pub sides: Vec<RedSide>,
    /// Somme des copies.
    pub bytes: u64,
    /// Arrivée la plus tardive des copies (série : fiche, diffusions et import du dernier épisode compris).
    #[serde(default)]
    pub arrived: Option<i64>,
    /// Dernière lecture datée, tous comptes et Playback Reporting confondus (série : maximum sur ses épisodes).
    #[serde(default)]
    pub last_played: Option<i64>,
    /// Vu sans date : début de l'horloge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undated_since: Option<i64>,
    /// Date où le titre devient (ou est devenu) rouge : dernière activité + `min_idle_days`. Absente si non évalué.
    #[serde(default)]
    pub due: Option<i64>,
    /// Motif en clair (« jamais lu, arrivé le 12/09/2026 »), texte brut échappé à l'affichage.
    pub reason: String,
    /// `membre`, `admin`, `aucune` ou `inconnu`.
    pub requester: String,
}

/// Une copie d'un titre : son côté et sa fiche d'Arr.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedSide {
    /// `vps` ou `seedbox`.
    pub side: String,
    /// Identifiant de la fiche dans le Radarr ou le Sonarr de ce côté.
    pub arr_id: i64,
    pub bytes: u64,
    /// Dossier de la fiche tel que l'Arr le voit.
    pub folder: String,
    /// Élément Jellyfin de cette copie (identifiant compact) ; vide si non rapproché.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jf_id: String,
    /// Arrivée de cette copie.
    #[serde(default)]
    pub arrived: Option<i64>,
}

/// Un clic sur « Nettoyer » (`red_list_clean`), gardé dans [`RedListState::jobs`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanJob {
    pub id: String,
    pub started: i64,
    #[serde(default)]
    pub ended: Option<i64>,
    /// Simulation (`clean_dry_run`) : rien n'a été supprimé.
    pub dry_run: bool,
    /// Empreinte `snap` de la liste confirmée par l'admin.
    pub snap: String,
    /// `running`, `done`, `interrupted` (homelabd redémarré en plein travail), `refused` (calcul incomplet, liste
    /// changée), `failed`.
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Clés confirmées, `max_titles_per_clean` au plus.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Quota de la seedbox au début du travail (`quota.json`).
    #[serde(default)]
    pub quota_before: Option<QuotaReading>,
    /// Résultat titre par titre, dans l'ordre de traitement.
    #[serde(default)]
    pub results: Vec<CleanResult>,
}

/// Relevé de `quota.json` de la seedbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaReading {
    pub used_kb: u64,
    pub quota_kb: u64,
    pub at: i64,
}

/// Ce qu'un « Nettoyer » a fait d'un titre.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanResult {
    pub key: String,
    pub title: String,
    pub at: i64,
    /// `deleted`, `partial` (un côté resté : fiche toujours là), `skipped` (revérification : exempté, en lecture, plus
    /// rouge, fiche changée, déplacé récemment), `simulated`, `error`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Octets libérés tout de suite (taille exclusive : aucun autre lien physique).
    #[serde(default)]
    pub freed_now: u64,
    /// Libérés plus tard : torrent C411 qui doit encore partager, corbeille à rejouer.
    #[serde(default)]
    pub freed_later: u64,
    /// Encore liés ailleurs (autre lien physique hors du titre) : rien de libéré.
    #[serde(default)]
    pub shared: u64,
    /// Étapes restées en attente (`recycle:<côté>`, `seerr`, `torrent:<côté>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
}

/// Étape d'un nettoyage à rejouer (quelques-unes par passage, abandonnée au bout de 14 j).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingStep {
    /// `recycle` (dossier du titre dans la corbeille de l'Arr) ou `seerr` (média Jellyseerr à retirer).
    pub step: String,
    /// Clé stable du titre.
    pub key: String,
    pub title: String,
    /// `vps` ou `seedbox` (vide pour `seerr`).
    #[serde(default)]
    pub side: String,
    /// `recycle` : dossier vérifié à vider ; vide pour `seerr`.
    #[serde(default)]
    pub target: String,
    /// `recycle` : dossier d'origine du titre (tant qu'il existe, l'Arr n'a pas fini de le déplacer dans sa corbeille).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// `seerr` : `movie` ou `tv`, et identifiant TMDB.
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub tmdb: i64,
    /// `recycle` seedbox : corbeille de l'Arr au moment de la suppression ; la reprise exige la même, relue alors.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bin: String,
    /// `recycle` VPS : torrents gardés parce que le fichier dans la corbeille tenait encore leur lien physique ; revus
    /// une fois la corbeille vidée. `arr_id` : fiche supprimée (historique des autres titres).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub arr_id: i64,
    /// `seerr` : dossiers du titre (hôte ou montage) ; le média n'est retiré qu'une fois tous disparus.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<String>,
    pub first_at: i64,
    #[serde(default)]
    pub last_try: i64,
    #[serde(default)]
    pub tries: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_error: String,
}

/// Dernier passage d'une tâche. Les champs ajoutés le 2026-10-07 sont tous en `#[serde(default)]` : un état écrit par
/// l'ancien binaire se relit, et l'ancien binaire relit un état écrit par le nouveau (aucun `deny_unknown_fields`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunInfo {
    pub last_start: i64,
    pub last_end: Option<i64>,
    pub last_ok: Option<bool>,
    pub last_summary: String,
    pub runs: u64,
    /// Total **depuis l'origine** de l'état (jamais remis à zéro) : pour « combien et quand », voir `errors_by_day`.
    pub errors: u64,
    /// Dernière erreur, chaîne complète des causes coupée à [`LAST_ERROR_MAX`] caractères. Le résumé
    /// (`last_summary`) est écrasé par le passage réussi suivant ; celle-ci survit.
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub last_error_at: Option<i64>,
    /// Erreurs par jour local (`AAAA-MM-JJ`), [`ERROR_DAYS_KEPT`] jours au plus.
    #[serde(default)]
    pub errors_by_day: BTreeMap<String, u32>,
    /// Échecs consécutifs ; remis à 0 au premier succès.
    #[serde(default)]
    pub fail_streak: u32,
    /// Début de la série d'échecs en cours (date du premier échec).
    #[serde(default)]
    pub fail_since: Option<i64>,
    /// Une alerte admin est partie pour la série en cours (un message au retour, puis plus rien).
    #[serde(default)]
    pub streak_alerted: bool,
}

/// Taille maximale de [`RunInfo::last_error`].
pub const LAST_ERROR_MAX: usize = 300;
/// Nombre de jours gardés dans [`RunInfo::errors_by_day`].
pub const ERROR_DAYS_KEPT: usize = 14;

/// Jour local d'une date (clé de `errors_by_day`).
pub fn day_key(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

/// Date lisible « 04/10 02:30 » (heure locale) pour les messages et les pages d'état.
pub fn short_date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

/// Retour à la normale d'une tâche qui avait déclenché une alerte d'échecs répétés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub failures: u32,
    pub since: i64,
}

/// Série d'échecs qui vient de dépasser les seuils (voir [`RunInfo::take_streak_alert`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreakAlert {
    pub failures: u32,
    pub since: i64,
    pub last_error: String,
}

/// Durée au bout de laquelle le daemon interrompt un passage (`scheduler::run_once`). Au-delà, un début sans fin ne
/// peut plus être un passage en cours.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// Où en est une tâche d'après ce qu'un lecteur du fichier d'état en voit (voir [`RunInfo::phase`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    /// Dernier passage terminé à cette date.
    Finished(i64),
    /// Début à cette date, pas de fin enregistrée : un passage en cours, **ou** fini depuis moins d'une minute dont
    /// la fin (`update_lazy`) n'est pas encore sur disque.
    Running(i64),
    /// Début à cette date, pas de fin, et plus ancien que [`RUN_TIMEOUT`] : le passage ne tourne plus (daemon arrêté
    /// ou tué en plein passage). Rien ne le dira avant le passage suivant de la tâche, des heures pour une rare.
    Interrupted(i64),
}

impl RunPhase {
    /// Texte court pour `homelabctl status`. Un « ? » après `running` : le fichier ne sait pas dire si le passage
    /// tourne encore ou vient de finir.
    pub fn label(&self) -> String {
        let local = |t: i64, fmt: &str| {
            chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.with_timezone(&chrono::Local).format(fmt).to_string())
                .unwrap_or_else(|| "?".into())
        };
        match *self {
            RunPhase::Finished(t) => local(t, "%Y-%m-%d %H:%M:%S"),
            RunPhase::Running(t) => format!("running? depuis {}", local(t, "%H:%M")),
            RunPhase::Interrupted(t) => format!("interrompu? {}", local(t, "%d/%m %H:%M")),
        }
    }
}

impl RunInfo {
    /// Un début sans fin (`last_end` vide, ou plus ancienne que le début) n'est « en cours » que tant qu'un passage
    /// peut encore durer. Avant le 2026-10-08, `homelabctl status` affichait « running » dès que `last_end` était
    /// vide, donc aussi pour une tâche coupée en plein passage (tuée avec le daemon) — jusqu'à son passage suivant.
    pub fn phase(&self, now: i64) -> RunPhase {
        match self.last_end {
            Some(end) if end >= self.last_start => RunPhase::Finished(end),
            _ if now.saturating_sub(self.last_start) <= RUN_TIMEOUT.as_secs() as i64 => {
                RunPhase::Running(self.last_start)
            }
            _ => RunPhase::Interrupted(self.last_start),
        }
    }

    /// Enregistre la fin d'un passage. `summary` est ce que la tâche a rendu (ou « error: … » / « timeout » en cas
    /// d'échec). Succès : remet la série à zéro et rend le retour à la normale si une alerte était partie. Échec :
    /// compte l'erreur (total, jour, série) et garde son texte.
    pub fn record_outcome(&mut self, ts: i64, ok: bool, summary: &str) -> Option<Recovered> {
        self.last_end = Some(ts);
        self.last_ok = Some(ok);
        self.last_summary = summary.to_string();
        if ok {
            let back = self.streak_alerted.then(|| Recovered {
                failures: self.fail_streak,
                since: self.fail_since.unwrap_or(ts),
            });
            self.fail_streak = 0;
            self.fail_since = None;
            self.streak_alerted = false;
            return back;
        }
        self.errors += 1;
        self.last_error = truncate_error(summary.strip_prefix("error: ").unwrap_or(summary));
        self.last_error_at = Some(ts);
        *self.errors_by_day.entry(day_key(ts)).or_insert(0) += 1;
        while self.errors_by_day.len() > ERROR_DAYS_KEPT {
            self.errors_by_day.pop_first();
        }
        self.fail_streak = self.fail_streak.saturating_add(1);
        self.fail_since.get_or_insert(ts);
        None
    }

    /// La série d'échecs en cours a-t-elle dépassé les seuils sans avoir encore été signalée ? (Sans rien changer.)
    pub fn streak_due(&self, ts: i64, min_failures: u32, min_mins: i64) -> bool {
        match self.fail_since {
            Some(since) => {
                !self.streak_alerted
                    && self.last_ok == Some(false)
                    && self.fail_streak >= min_failures
                    && ts - since >= min_mins * 60
            }
            None => false,
        }
    }

    /// La série d'échecs en cours doit-elle déclencher l'alerte ? Oui, une seule fois, quand elle compte au moins
    /// `min_failures` échecs de suite **et** dure depuis au moins `min_mins` minutes (une tâche toutes les 20 s
    /// échoue six fois en deux minutes sans que ce soit une panne). Marque la série comme signalée.
    pub fn take_streak_alert(
        &mut self,
        ts: i64,
        min_failures: u32,
        min_mins: i64,
    ) -> Option<StreakAlert> {
        if !self.streak_due(ts, min_failures, min_mins) {
            return None;
        }
        let since = self.fail_since?;
        self.streak_alerted = true;
        Some(StreakAlert {
            failures: self.fail_streak,
            since,
            last_error: self.last_error.clone(),
        })
    }

    /// Erreurs des `days` derniers jours locaux (aujourd'hui compris).
    pub fn errors_last_days(&self, ts: i64, days: u32) -> u32 {
        let first = day_key(ts - (days.saturating_sub(1) as i64) * 86_400);
        self.errors_by_day
            .range(first..)
            .map(|(_, n)| *n)
            .fold(0u32, u32::saturating_add)
    }

    /// « 2 aujourd'hui, 5 sur 7 j · dernière le 04/10 02:30 : message » ; `None` si aucune erreur n'est connue.
    pub fn error_line(&self, ts: i64) -> Option<String> {
        let at = self.last_error_at?;
        let mut line = format!(
            "{} aujourd'hui, {} sur 7 j · dernière le {}",
            self.errors_last_days(ts, 1),
            self.errors_last_days(ts, 7),
            short_date(at)
        );
        if !self.last_error.is_empty() {
            line.push_str(" : ");
            line.push_str(&self.last_error);
        }
        Some(line)
    }

    /// Note à afficher sous une tâche (`homelabctl status`, `/status.html`) : combien d'erreurs récentes et la
    /// dernière, avec sa date. Seulement si la dernière erreur date de moins de 7 jours ou si la tâche échoue en ce
    /// moment : le compteur cumulé `errors` ne dit pas « quand » et faisait passer d'anciennes pannes pour des
    /// erreurs actuelles (229 pour `seedbox_refresh`, toutes antérieures au 04/10).
    pub fn error_note(&self, now: i64) -> Option<String> {
        let at = self.last_error_at?;
        if self.last_ok != Some(false) && now - at > 7 * 86_400 {
            return None;
        }
        let mut note = String::new();
        if self.fail_streak >= 2 {
            note.push_str(&format!("{} échecs de suite · ", self.fail_streak));
        }
        note.push_str(&self.error_line(now)?);
        Some(note.chars().take(260).collect())
    }
}

/// Texte d'erreur sur une ligne, coupé à [`LAST_ERROR_MAX`] caractères.
fn truncate_error(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() <= LAST_ERROR_MAX {
        return flat;
    }
    let mut out: String = flat.chars().take(LAST_ERROR_MAX - 1).collect();
    out.push('…');
    out
}

/// Nombre d'alertes gardées dans [`AlertStats::recent`].
pub const ALERTS_KEPT: usize = 20;

/// Une alerte admin passée par `alerts::admin` : ni adresse, ni URL, ni corps du message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertRecord {
    pub at: i64,
    pub subject: String,
    pub mailed: bool,
    pub posted: bool,
}

impl AlertRecord {
    /// « mail + Discord », « Discord », « mail » ou « non livrée ».
    pub fn channels(&self) -> &'static str {
        match (self.mailed, self.posted) {
            (true, true) => "mail + Discord",
            (true, false) => "mail",
            (false, true) => "Discord",
            (false, false) => "non livrée",
        }
    }
}

/// Livraison des alertes admin : sans cette trace, on ne savait pas lesquelles étaient parties (le journal
/// ne gardait que les échecs d'envoi, et seulement 3 jours).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertStats {
    /// Alertes livrées par au moins un canal / par aucun (canal configuré mais en échec).
    #[serde(default)]
    pub delivered: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub last_delivered_at: Option<i64>,
    /// Les [`ALERTS_KEPT`] dernières, la plus récente en dernier.
    #[serde(default)]
    pub recent: Vec<AlertRecord>,
    /// Places prises par des alertes d'échecs répétés **en cours d'envoi** (dates, secondes). Jamais écrites sur
    /// disque : une réservation ne vit que le temps de l'envoi (voir [`AlertStats::reserve_streak`]).
    #[serde(skip)]
    pub reserved: Vec<i64>,
}

impl AlertStats {
    /// Alertes dont l'objet commence par `prefix` et qui datent de moins de `window_secs` avant `ts` : le plafond
    /// des alertes d'échecs répétés (voir `[alerts] fail_alerts_max`). Ne regarde que les [`ALERTS_KEPT`] dernières.
    pub fn count_recent(&self, prefix: &str, ts: i64, window_secs: i64) -> usize {
        self.recent
            .iter()
            .filter(|a| a.subject.starts_with(prefix) && ts - a.at < window_secs)
            .count()
    }

    /// Prend une place pour une alerte d'échecs répétés **dans la même mise à jour d'état que la décision**
    /// (2026-10-08). Le plafond ne comptait que les alertes déjà enregistrées, or `alerts::admin` n'enregistre
    /// qu'après l'envoi (SMTP puis Discord, plusieurs secondes) : deux tâches arrivées au seuil à quelques secondes
    /// d'écart voyaient toutes deux de la place et partaient toutes deux. Ici les envois en cours comptent aussi.
    /// `false` : plafond atteint, rien n'est réservé. Une réservation orpheline (envoi interrompu) sort de la
    /// fenêtre d'elle-même.
    pub fn reserve_streak(&mut self, prefix: &str, ts: i64, window_secs: i64, cap: usize) -> bool {
        self.reserved.retain(|r| ts - *r < window_secs);
        if self.count_recent(prefix, ts, window_secs) + self.reserved.len() >= cap {
            return false;
        }
        self.reserved.push(ts);
        true
    }

    /// Rend la place prise par [`reserve_streak`](Self::reserve_streak) une fois l'envoi terminé : `alerts::admin`
    /// a alors enregistré l'alerte (livrée ou non), qui compte à sa place. Même quand rien n'a été enregistré
    /// (dry-run), la place n'a plus de raison d'être retenue.
    pub fn release_streak(&mut self, ts: i64) {
        if let Some(i) = self.reserved.iter().position(|r| *r == ts) {
            self.reserved.remove(i);
        }
    }

    pub fn record(&mut self, at: i64, subject: &str, mailed: bool, posted: bool) {
        if mailed || posted {
            self.delivered += 1;
            self.last_delivered_at = Some(at);
        } else {
            self.failed += 1;
        }
        self.recent.push(AlertRecord {
            at,
            subject: subject.chars().take(120).collect(),
            mailed,
            posted,
        });
        let extra = self.recent.len().saturating_sub(ALERTS_KEPT);
        self.recent.drain(..extra);
    }
}

/// Une mutation différée (`update_lazy`) n'entraîne pas d'écriture si l'état a été sérialisé depuis moins que ça :
/// elle part avec l'écriture suivante, ou au plus tard au tour suivant de [`StateStore::run_lazy_flusher`] (un tour
/// de ce délai), ou à l'arrêt. Jusqu'au 2026-10-08 le drapeau `dirty` n'était lu par personne et rien ne forçait
/// l'écriture : une mutation différée attendait la première mutation suivante, des heures pour une tâche rare (vu
/// en production sur `deletion_cleanup`).
pub const LAZY_MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    inner: Arc<Mutex<Inner>>,
    disk: Arc<Disk>,
    /// Copie de `homelabctl` : jamais écrite. Le daemon est seul propriétaire du fichier (une CLI qui l'écrivait
    /// écrasait l'état du daemon, qui l'écrasait en retour à sa sauvegarde suivante — audit du 2026-09-23, E9).
    read_only: bool,
    lazy_max_age: Duration,
}

struct Inner {
    state: State,
    /// Numéro du dernier instantané sérialisé. Attribué sous ce verrou : l'ordre des numéros est celui des mutations.
    version: u64,
    /// Mutations différées (`update_lazy`) pas encore sérialisées.
    dirty: bool,
    /// Dernière sérialisation : rythme des écritures différées.
    encoded_at: Option<Instant>,
}

/// Côté disque : un seul écrivain à la fois, qui écrit toujours l'instantané le plus récent.
///
/// Jusqu'au 2026-10-07, chaque `update` écrivait sous le verrou de l'état, avec serde_json branché directement sur
/// le fichier, sans tampon : ~40 000 appels `write` de 4 octets et ~150 ms de verrou par sauvegarde, 66 % du temps
/// processeur du daemon dans le noyau, 4,2 Go écrits par jour. L'état est maintenant sérialisé en mémoire sous le
/// verrou (quelques ms), puis écrit en un appel, hors du verrou, dans un fil bloquant.
struct Disk {
    /// Instantané le plus récent qu'aucun écrivain n'a encore pris. Un plus ancien est remplacé : inutile d'écrire
    /// un état déjà dépassé (il est contenu dans le suivant).
    pending: std::sync::Mutex<Option<Snapshot>>,
    /// Tenu pendant toute une écriture, même si l'appelant est annulé (la garde part dans `spawn_blocking`) : deux
    /// écritures ne se croisent jamais, et un état plus ancien ne peut pas être renommé après un plus récent.
    written: Arc<Mutex<Written>>,
    /// Écritures effectives sur disque (diagnostic, tests).
    writes: AtomicU64,
}

struct Snapshot {
    version: u64,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Written {
    /// Dernière version sur disque (ou identique octet pour octet à ce qui y est).
    version: u64,
    /// Ses octets : une sérialisation identique n'est pas réécrite.
    bytes: Vec<u8>,
}

impl Disk {
    fn new(on_disk: Vec<u8>) -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
            written: Arc::new(Mutex::new(Written {
                version: 0,
                bytes: on_disk,
            })),
            writes: AtomicU64::new(0),
        }
    }

    /// Propose un instantané : il ne remplace celui en attente que s'il est plus récent.
    fn offer(&self, snap: Snapshot) {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if p.as_ref().is_none_or(|q| q.version < snap.version) {
            *p = Some(snap);
        }
    }

    fn take(&self) -> Option<Snapshot> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

impl StateStore {
    pub fn load(path: &Path) -> Result<Self> {
        let (state, on_disk) = if path.is_file() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("lecture de {}", path.display()))?;
            let state =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
            (state, raw.into_bytes())
        } else {
            (State::default(), Vec::new())
        };
        Ok(Self {
            path: path.to_path_buf(),
            inner: Arc::new(Mutex::new(Inner {
                state,
                version: 0,
                dirty: false,
                encoded_at: None,
            })),
            disk: Arc::new(Disk::new(on_disk)),
            read_only: false,
            lazy_max_age: LAZY_MAX_AGE,
        })
    }

    /// Même lecture, mais les mutations restent en mémoire : pour `homelabctl`, qui passe par le daemon pour
    /// tout changement durable.
    pub fn load_read_only(path: &Path) -> Result<Self> {
        let mut s = Self::load(path)?;
        s.read_only = true;
        Ok(s)
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub async fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        let guard = self.inner.lock().await;
        f(&guard.state)
    }

    /// Applique une mutation puis attend qu'elle soit sur disque (écriture atomique : fichier temporaire + rename).
    pub async fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        self.apply(false, f).await
    }

    /// Mutation de simple tenue (début d'un passage, fin d'un passage qui ne change rien) : appliquée tout de suite
    /// en mémoire, écrite avec la sauvegarde suivante — au plus tard au tour suivant de `run_lazy_flusher`
    /// (`LAZY_MAX_AGE`), ou à l'arrêt (`flush`). Une coupure brutale peut en perdre une minute, jamais une mutation
    /// faite par `update`.
    pub async fn update_lazy<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        self.apply(true, f).await
    }

    /// `update_lazy` si `lazy`, sinon `update`.
    pub async fn update_lazy_if<R>(
        &self,
        lazy: bool,
        f: impl FnOnce(&mut State) -> R,
    ) -> Result<R> {
        self.apply(lazy, f).await
    }

    /// Écrit les mutations différées qui attendent encore, et rien du tout si aucune n'attend (ni sérialisation ni
    /// écriture). Rend `true` si une écriture a été demandée. En cas d'échec d'écriture le drapeau est remis : le
    /// tour suivant réessaie (l'instantané resté en attente n'aurait sinon plus personne pour l'écrire).
    pub async fn flush_if_dirty(&self) -> Result<bool> {
        if self.read_only {
            return Ok(false);
        }
        let version = {
            let mut g = self.inner.lock().await;
            if !g.dirty {
                return Ok(false);
            }
            self.snapshot(&mut g)?
        };
        if let Err(e) = self.write_up_to(version).await {
            self.inner.lock().await.dirty = true;
            return Err(e);
        }
        Ok(true)
    }

    /// Tient la promesse de [`LAZY_MAX_AGE`] : toutes les `every`, écrit ce que `update_lazy` a laissé en mémoire.
    /// Ne rend jamais la main (à lancer dans une tâche, arrêtée avec le daemon ; `flush` écrit le reste).
    pub async fn run_lazy_flusher(&self, every: Duration) {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await; // le premier tour part tout de suite : rien à écrire au démarrage
        loop {
            tick.tick().await;
            if let Err(e) = self.flush_if_dirty().await {
                tracing::warn!(
                    error = format!("{e:#}"),
                    "écriture différée de l'état échouée"
                );
            }
        }
    }

    /// Écrit ce qui ne l'est pas encore (mutations différées) : à l'arrêt du daemon. Rien si le disque est à jour.
    pub async fn flush(&self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        let version = {
            let mut g = self.inner.lock().await;
            self.snapshot(&mut g)?
        };
        self.write_up_to(version).await
    }

    async fn apply<R>(&self, lazy: bool, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        let (out, version) = {
            let mut g = self.inner.lock().await;
            let out = f(&mut g.state);
            if self.read_only {
                return Ok(out);
            }
            if lazy
                && g.encoded_at
                    .is_some_and(|t| t.elapsed() < self.lazy_max_age)
            {
                g.dirty = true;
                return Ok(out);
            }
            let version = self.snapshot(&mut g)?;
            (out, version)
        };
        // verrou de l'état relâché : lectures et mutations continuent pendant l'écriture
        self.write_up_to(version).await?;
        Ok(out)
    }

    /// Sérialise l'état (sous son verrou, donc cohérent) et le propose à l'écrivain. Rend son numéro.
    fn snapshot(&self, g: &mut Inner) -> Result<u64> {
        let bytes = match encode(&g.state) {
            Ok(b) => b,
            Err(e) => {
                g.dirty = true;
                return Err(e);
            }
        };
        g.version += 1;
        g.dirty = false;
        g.encoded_at = Some(Instant::now());
        self.disk.offer(Snapshot {
            version: g.version,
            bytes,
        });
        Ok(g.version)
    }

    /// Rend la main quand une version au moins aussi récente que `version` est sur disque (l'écrit si besoin).
    async fn write_up_to(&self, version: u64) -> Result<()> {
        let mut w = self.disk.written.clone().lock_owned().await;
        if w.version >= version {
            // un écrivain précédent a déjà posé un état au moins aussi récent, qui contient cette mutation
            return Ok(());
        }
        let Some(snap) = self.disk.take() else {
            // seulement si une écriture a paniqué : l'instantané suivant contiendra tout
            anyhow::bail!(
                "écriture {} : instantané {version} perdu",
                self.path.display()
            );
        };
        if snap.version <= w.version {
            return Ok(()); // déjà dépassé (ne devrait pas arriver : les numéros ne font que croître)
        }
        if snap.bytes == w.bytes {
            w.version = snap.version; // identique à ce qui est sur disque : rien à réécrire
            return Ok(());
        }
        let path = self.path.clone();
        let disk = self.disk.clone();
        tokio::task::spawn_blocking(move || match write_atomic(&path, &snap.bytes) {
            Ok(()) => {
                disk.writes.fetch_add(1, Ordering::Relaxed);
                w.version = snap.version;
                w.bytes = snap.bytes;
                Ok(())
            }
            Err(e) => {
                // repris par l'écriture suivante, sauf si un instantané plus récent attend déjà
                disk.offer(snap);
                Err(e)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("écriture {} interrompue : {e}", self.path.display()))?
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    fn disk_writes(&self) -> u64 {
        self.disk.writes.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn with_lazy_max_age(mut self, d: Duration) -> Self {
        self.lazy_max_age = d;
        self
    }
}

/// État → JSON indenté (lisible avec jq ou less), en mémoire : aucun appel système.
fn encode(state: &State) -> Result<Vec<u8>> {
    let mut buf = serde_json::to_vec_pretty(state)?;
    buf.push(b'\n');
    Ok(buf)
}

/// Écriture atomique : fichier temporaire du même dossier, **un seul** `write`, fsync, renommage, fsync du dossier.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("state_file sans dossier parent")?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    // sur disque AVANT le renommage : sinon une coupure peut laisser un fichier d'état vide
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("écriture {} : {}", path.display(), e.error))?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all(); // le renommage lui-même
    }
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck(s: &mut State, k: &str) {
        s.stuck.insert(
            k.into(),
            StuckEntry {
                service: "sonarr".into(),
                download_id: k.into(),
                first_seen: 1,
                title: "t".into(),
            },
        );
    }

    fn on_disk(path: &Path) -> State {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// État de la taille de celui de la production (~180 Ko de JSON indenté).
    fn big_state() -> State {
        let mut s = State::default();
        for i in 0..600 {
            s.torrent_import.insert(
                format!("seedbox:{i:040x}"),
                TorrentImportRecord {
                    at: 1_791_000_000 + i,
                    name: format!("Une.Serie.Quelconque.S01E{i:03}.MULTi.1080p.WEB.H265-GRP"),
                    outcome: "imported".into(),
                    detail: "3 fichier(s) importé(s) en lien physique vers la fiche 1234".into(),
                    attempts: 1,
                    ..Default::default()
                },
            );
        }
        s
    }

    #[tokio::test]
    async fn roundtrip_persists_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");
        let store = StateStore::load(&path).unwrap();
        store.update(|s| stuck(s, "sonarr:abc")).await.unwrap();
        let reloaded = StateStore::load(&path).unwrap();
        assert_eq!(reloaded.read(|s| s.stuck.len()).await, 1);
        assert!(std::fs::read_dir(path.parent().unwrap()).unwrap().count() == 1);
    }

    /// Midi UTC d'un jour : loin de toute frontière de jour local, quel que soit le fuseau du poste de test.
    const NOON: i64 = 1_790_000_000 - 1_790_000_000 % 86_400 + 43_200;

    #[test]
    fn a_failure_keeps_its_text_date_and_day_count() {
        let mut r = RunInfo::default();
        assert_eq!(
            r.record_outcome(NOON, false, "error: sonarr: délai dépassé"),
            None
        );
        assert_eq!(r.last_ok, Some(false));
        assert_eq!(r.errors, 1);
        assert_eq!(
            r.last_error, "sonarr: délai dépassé",
            "le préfixe « error: » n'est pas gardé"
        );
        assert_eq!(r.last_error_at, Some(NOON));
        assert_eq!(r.errors_by_day.get(&day_key(NOON)), Some(&1));
        r.record_outcome(NOON + 60, false, "timeout");
        assert_eq!(r.errors_by_day.get(&day_key(NOON)), Some(&2));
        assert_eq!((r.fail_streak, r.fail_since), (2, Some(NOON)));
        // le succès suivant écrase le résumé, pas l'erreur
        r.record_outcome(NOON + 120, true, "ok");
        assert_eq!(r.last_summary, "ok");
        assert_eq!(r.last_error, "timeout");
        assert_eq!((r.fail_streak, r.fail_since), (0, None));
        assert_eq!(r.errors, 2, "le total ne baisse jamais");
    }

    #[test]
    fn the_error_text_is_one_line_of_300_chars_at_most() {
        let mut r = RunInfo::default();
        let long = format!("error: a\nb\t{}", "é".repeat(500));
        r.record_outcome(NOON, false, &long);
        assert_eq!(r.last_error.chars().count(), LAST_ERROR_MAX);
        assert!(r.last_error.ends_with('…'));
        assert!(!r.last_error.contains('\n') && !r.last_error.contains('\t'));
        assert!(r.last_error.starts_with("a b "));
    }

    #[test]
    fn only_the_last_14_days_are_kept() {
        let mut r = RunInfo::default();
        for d in 0..20 {
            r.record_outcome(NOON + d * 86_400, false, "error: x");
        }
        assert_eq!(r.errors_by_day.len(), ERROR_DAYS_KEPT);
        assert_eq!(r.errors, 20, "le total cumulé reste");
        let last = NOON + 19 * 86_400;
        assert_eq!(r.errors_last_days(last, 1), 1);
        assert_eq!(r.errors_last_days(last, 7), 7);
        assert_eq!(r.errors_last_days(last, 30), 14);
        assert!(
            !r.errors_by_day.contains_key(&day_key(NOON)),
            "le plus ancien est parti"
        );
    }

    #[test]
    fn the_streak_alert_needs_both_the_count_and_the_duration() {
        let mut r = RunInfo::default();
        // 6 échecs en 2 minutes : trop court
        for i in 0..6 {
            r.record_outcome(NOON + i * 20, false, "error: x");
        }
        assert_eq!(r.take_streak_alert(NOON + 120, 6, 30), None);
        // 30 minutes plus tard, encore en échec : l'alerte part
        r.record_outcome(NOON + 1800, false, "error: y");
        let a = r.take_streak_alert(NOON + 1800, 6, 30).expect("alerte");
        assert_eq!((a.failures, a.since, a.last_error.as_str()), (7, NOON, "y"));
        // une seule fois par série
        r.record_outcome(NOON + 2100, false, "error: y");
        assert_eq!(r.take_streak_alert(NOON + 2100, 6, 30), None);
        // retour : message de retour, puis le compteur repart de zéro
        let back = r.record_outcome(NOON + 2400, true, "ok").expect("retour");
        assert_eq!(
            back,
            Recovered {
                failures: 8,
                since: NOON
            }
        );
        assert_eq!(r.record_outcome(NOON + 2700, true, "ok"), None);
        r.record_outcome(NOON + 3000, false, "error: z");
        assert_eq!(r.take_streak_alert(NOON + 3000, 6, 30), None);
    }

    #[test]
    fn streak_due_changes_nothing_and_matches_the_alert() {
        let mut r = RunInfo::default();
        for i in 0..7 {
            r.record_outcome(NOON + i * 300, false, "error: x");
        }
        let ts = NOON + 6 * 300;
        // 7 échecs en 30 min : dû, et le constat ne marque rien
        assert!(r.streak_due(ts, 6, 30));
        assert!(r.streak_due(ts, 6, 30), "lecture seule : toujours dû");
        assert!(!r.streak_alerted);
        // trop court, ou trop peu d'échecs : pas dû
        assert!(!r.streak_due(ts, 6, 31));
        assert!(!r.streak_due(ts, 8, 30));
        // la prise marque la série, après quoi plus rien n'est dû
        assert!(r.take_streak_alert(ts, 6, 30).is_some());
        assert!(!r.streak_due(ts, 6, 30));
        // un succès : plus de série
        r.record_outcome(ts + 60, true, "ok");
        assert!(!r.streak_due(ts + 60, 1, 0));
    }

    #[test]
    fn intermittent_errors_never_alert() {
        let mut r = RunInfo::default();
        for i in 0..40 {
            let ok = i % 3 == 2; // jamais plus de deux échecs de suite
            r.record_outcome(NOON + i * 600, ok, if ok { "ok" } else { "error: x" });
            assert_eq!(r.take_streak_alert(NOON + i * 600, 6, 30), None);
        }
    }

    #[test]
    fn a_recovery_without_alert_is_silent() {
        let mut r = RunInfo::default();
        r.record_outcome(NOON, false, "error: x");
        assert_eq!(r.record_outcome(NOON + 60, true, "ok"), None);
    }

    #[test]
    fn the_old_state_format_still_loads() {
        // état d'avant le 2026-10-07 : ni last_error, ni errors_by_day, ni fail_streak
        let old = r#"{"last_start":1,"last_end":2,"last_ok":true,"last_summary":"s","runs":3,"errors":4}"#;
        let r: RunInfo = serde_json::from_str(old).unwrap();
        assert_eq!((r.runs, r.errors, r.fail_streak), (3, 4, 0));
        assert!(r.last_error.is_empty() && r.last_error_at.is_none() && r.errors_by_day.is_empty());
        assert_eq!(r.error_line(NOON), None, "aucune erreur datée connue");
        // l'inverse (retour arrière du binaire) : champs inconnus ignorés par l'ancienne structure
        let new = serde_json::to_string(&r).unwrap();
        assert!(new.contains("fail_streak"));
    }

    #[test]
    fn a_state_without_watch_memory_still_loads() {
        // état d'avant le 2026-10-08 : pas de `watch_alerts`
        let s: State = serde_json::from_str(r#"{"alerts":{"delivered":2}}"#).unwrap();
        assert!(s.watch_alerts.is_empty());
        assert_eq!(s.alerts.delivered, 2);
        // et l'aller-retour garde la date et l'empreinte de la dernière alerte
        let mut s = State::default();
        s.watch_alerts.insert(
            "backup_watch".into(),
            WatchAlert {
                at: 1_000,
                key: "stale".into(),
            },
        );
        let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.watch_alerts["backup_watch"].key, "stale");
        assert_eq!(back.watch_alerts["backup_watch"].at, 1_000);
    }

    #[test]
    fn a_state_without_red_list_still_loads_and_the_red_list_survives_both_ways() {
        // état d'avant le 2026-10-20 : pas de `red_list`
        let old: State = serde_json::from_str(r#"{"alerts":{"delivered":2}}"#).unwrap();
        assert_eq!(old.red_list, RedListState::default());
        // un `red_list` partiel (champs ajoutés plus tard absents) se relit aussi
        let partial: State = serde_json::from_str(
            r#"{"red_list":{"exemptions":{"movie:tmdb:1":{"at":5,"title":"X"}}}}"#,
        )
        .unwrap();
        assert_eq!(partial.red_list.exemptions["movie:tmdb:1"].title, "X");
        assert!(partial.red_list.report.is_none() && partial.red_list.jobs.is_empty());
        // aller-retour complet
        let side = RedSide {
            side: "seedbox".into(),
            arr_id: 12,
            bytes: 3_000,
            folder: "/media/TV Shows/Serie".into(),
            jf_id: "abcd".into(),
            arrived: Some(100),
        };
        let entry = RedEntry {
            key: "series:tvdb:7".into(),
            title: "Série <b>".into(),
            kind: "série".into(),
            movie: false,
            tmdb: 70,
            tvdb: 7,
            russian: true,
            sides: vec![side],
            bytes: 3_000,
            arrived: Some(100),
            last_played: None,
            undated_since: Some(150),
            due: Some(5_284_150),
            reason: "vu sans date".into(),
            requester: "membre".into(),
        };
        let red_list = RedListState {
            report: Some(RedListReport {
                at: 6_000_000,
                min_idle_days: 60,
                soon_days: 14,
                complete: false,
                missing: vec!["Playback Reporting muet".into()],
                evaluated: 1,
                entries: vec![entry.clone()],
                soon: vec![],
                exempted: vec![],
                unmatched: vec![],
            }),
            exemptions: BTreeMap::from([(
                "movie:tmdb:1".into(),
                Exemption {
                    at: 5,
                    title: "X".into(),
                },
            )]),
            undated_since: BTreeMap::from([("series:tvdb:7".into(), 150)]),
            jobs: vec![CleanJob {
                id: "j1".into(),
                started: 10,
                ended: Some(20),
                dry_run: true,
                snap: "ab".into(),
                status: "done".into(),
                detail: String::new(),
                keys: vec!["series:tvdb:7".into()],
                quota_before: Some(QuotaReading {
                    used_kb: 1,
                    quota_kb: 2,
                    at: 3,
                }),
                results: vec![CleanResult {
                    key: "series:tvdb:7".into(),
                    title: "Série <b>".into(),
                    at: 15,
                    outcome: "simulated".into(),
                    freed_later: 3_000,
                    pending: vec!["seerr".into()],
                    ..Default::default()
                }],
            }],
            pending: BTreeMap::from([(
                "seerr::series:tvdb:7".into(),
                PendingStep {
                    step: "seerr".into(),
                    key: "series:tvdb:7".into(),
                    media_type: "tv".into(),
                    tmdb: 70,
                    first_at: 15,
                    ..Default::default()
                },
            )]),
        };
        let s = State {
            red_list,
            ..Default::default()
        };
        let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(
            back.red_list, s.red_list,
            "la liste rouge doit survivre à l'état"
        );
    }

    #[test]
    fn old_torrent_import_records_still_load_and_new_fields_are_omitted() {
        // enregistrement d'avant le 2026-10-21 (One Piece S23, 18/10) : ni `left_out` ni `alert_pending`
        let old = r#"{"at":1792000000,"name":"One.Piece.S23.VOSTFR.1080p.WEB.CR.AAC.2.0.x264-TsundereRaws","outcome":"imported","detail":"One Piece (1999) : 7 fichier(s)","attempts":0}"#;
        let r: TorrentImportRecord = serde_json::from_str(old).unwrap();
        assert_eq!(r.outcome, "imported");
        assert!(r.left_out.is_empty() && !r.alert_pending);
        // sans vidéo de côté ni alerte en attente, l'état écrit garde la forme d'avant (retour arrière sans surprise)
        let back = serde_json::to_string(&r).unwrap();
        assert!(
            !back.contains("left_out") && !back.contains("alert_pending"),
            "champs vides omis : {back}"
        );
        // un partiel garde sa liste et son alerte en attente d'un passage à l'autre
        let partial = TorrentImportRecord {
            outcome: "partial".into(),
            left_out: vec!["One Piece S01E1156 VOSTFR.mkv: Invalid season or episode".into()],
            alert_pending: true,
            ..r
        };
        let back: TorrentImportRecord =
            serde_json::from_str(&serde_json::to_string(&partial).unwrap()).unwrap();
        assert_eq!(back.left_out.len(), 1);
        assert!(
            back.alert_pending,
            "l'alerte en attente survit au rechargement"
        );
    }

    #[test]
    fn error_line_reads_well() {
        let mut r = RunInfo::default();
        r.record_outcome(NOON, false, "error: jellyfin Items: operation timed out");
        r.record_outcome(
            NOON + 60,
            false,
            "error: jellyfin Items: operation timed out",
        );
        let l = r.error_line(NOON + 3600).unwrap();
        assert!(
            l.starts_with("2 aujourd'hui, 2 sur 7 j · dernière le "),
            "{l}"
        );
        assert!(l.ends_with(": jellyfin Items: operation timed out"), "{l}");
    }

    #[test]
    fn alert_stats_count_and_keep_the_last_20() {
        let mut a = AlertStats::default();
        a.record(10, "livrée par mail", true, false);
        a.record(20, "échec", false, false);
        assert_eq!(
            (a.delivered, a.failed, a.last_delivered_at),
            (1, 1, Some(10))
        );
        assert_eq!(a.recent[1].channels(), "non livrée");
        for i in 0..30 {
            a.record(100 + i, &"x".repeat(200), false, true);
        }
        assert_eq!(a.recent.len(), ALERTS_KEPT);
        assert_eq!(a.recent.last().unwrap().at, 129);
        assert_eq!(a.recent[0].subject.chars().count(), 120);
        assert_eq!(a.recent[0].channels(), "Discord");
    }

    #[test]
    fn the_streak_alert_cap_counts_only_recent_matching_subjects() {
        let mut a = AlertStats::default();
        let p = "Tâche en échec répété";
        a.record(1_000, &format!("{p} : Seedbox"), true, true);
        a.record(1_100, "Disque VPS : 90 %", true, false);
        a.record(1_200, &format!("{p} : Jellyfin"), false, false);
        a.record(1_300, "Tâche rétablie : Seedbox", true, true);
        // fenêtre de 10 min = 600 s : les deux alertes d'échecs comptent, ni le disque ni le retour
        assert_eq!(a.count_recent(p, 1_400, 600), 2);
        // plus tard, la première sort de la fenêtre, puis la seconde
        assert_eq!(a.count_recent(p, 1_650, 600), 1);
        assert_eq!(a.count_recent(p, 1_900, 600), 0);
        // une alerte non livrée compte aussi : elle a bien été tentée
        assert_eq!(a.count_recent("Tâche rétablie", 1_400, 600), 1);
        assert_eq!(AlertStats::default().count_recent(p, 0, 600), 0);
    }

    #[test]
    fn two_streak_alerts_at_the_cap_cannot_both_pass() {
        let p = "Tâche en échec répété";
        let (window, cap) = (600, 3);
        let mut a = AlertStats::default();
        a.record(1_000, &format!("{p} : A"), true, true);
        a.record(1_100, &format!("{p} : B"), true, true);
        // deux tâches au seuil à quelques secondes d'écart, la première n'a pas fini d'envoyer (rien d'enregistré)
        assert!(
            a.reserve_streak(p, 1_200, window, cap),
            "il restait une place"
        );
        assert!(
            !a.reserve_streak(p, 1_203, window, cap),
            "la place est prise par l'envoi en cours"
        );
        // la première a fini : l'alerte enregistrée remplace la réservation, le compte reste à 3
        a.record(1_207, &format!("{p} : C"), true, true);
        a.release_streak(1_200);
        assert!(a.reserved.is_empty());
        assert!(!a.reserve_streak(p, 1_210, window, cap));
        // la fenêtre passe : de la place de nouveau
        assert!(a.reserve_streak(p, 1_000 + window + 1, window, cap));
    }

    #[test]
    fn a_released_reservation_without_record_frees_its_place() {
        let p = "Tâche en échec répété";
        let mut a = AlertStats::default();
        assert!(a.reserve_streak(p, 500, 600, 1));
        assert!(!a.reserve_streak(p, 501, 600, 1));
        // envoi sans trace (dry-run) : on rend la place
        a.release_streak(500);
        assert!(a.reserve_streak(p, 502, 600, 1));
        // libérer une date inconnue ne touche à rien
        a.release_streak(9_999);
        assert_eq!(a.reserved, vec![502]);
    }

    #[test]
    fn an_orphan_reservation_expires_with_the_window() {
        let p = "Tâche en échec répété";
        let mut a = AlertStats::default();
        assert!(a.reserve_streak(p, 100, 600, 1));
        // l'envoi a été interrompu (arrêt du daemon, annulation) : rien n'a été libéré, la fenêtre le fait
        assert!(!a.reserve_streak(p, 699, 600, 1));
        assert!(a.reserve_streak(p, 700, 600, 1));
        assert_eq!(a.reserved, vec![700], "l'orpheline est purgée");
    }

    #[test]
    fn reservations_are_never_written_to_disk() {
        let mut a = AlertStats::default();
        a.reserved.push(42);
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("reserved"), "{json}");
        let back: AlertStats = serde_json::from_str(&json).unwrap();
        assert!(back.reserved.is_empty());
    }

    #[test]
    fn a_start_without_an_end_is_running_only_while_a_pass_can_still_last() {
        let run = |start: i64, end: Option<i64>| RunInfo {
            last_start: start,
            last_end: end,
            ..Default::default()
        };
        let t = RUN_TIMEOUT.as_secs() as i64;
        // passage fini : sa fin
        assert_eq!(
            run(1_000, Some(1_005)).phase(9_999),
            RunPhase::Finished(1_005)
        );
        assert_eq!(
            run(1_000, Some(1_000)).phase(9_999),
            RunPhase::Finished(1_000),
            "début et fin dans la même seconde"
        );
        // vraie tâche en cours : début récent, pas de fin
        assert_eq!(run(1_000, None).phase(1_030), RunPhase::Running(1_000));
        assert_eq!(run(1_000, None).phase(1_000 + t), RunPhase::Running(1_000));
        // une fin plus ancienne que le début : un nouveau passage a commencé
        assert_eq!(
            run(2_000, Some(1_500)).phase(2_010),
            RunPhase::Running(2_000)
        );
        // plus vieux que le plafond d'un passage : le passage ne tourne plus (daemon arrêté en plein passage)
        assert_eq!(
            run(1_000, None).phase(1_000 + t + 1),
            RunPhase::Interrupted(1_000)
        );
        assert_eq!(
            run(2_000, Some(1_500)).phase(2_000 + 3_600),
            RunPhase::Interrupted(2_000)
        );
        // une horloge qui recule ne fait pas plus que « en cours »
        assert_eq!(run(5_000, None).phase(4_000), RunPhase::Running(5_000));
    }

    #[test]
    fn the_status_label_tells_a_guess_from_a_certainty() {
        assert!(RunPhase::Finished(1_700_000_000).label().len() == 19);
        let running = RunPhase::Running(1_700_000_000).label();
        assert!(
            running.starts_with("running? depuis "),
            "{running}: le fichier ne sait pas si c'est fini"
        );
        let cut = RunPhase::Interrupted(1_700_000_000).label();
        assert!(cut.starts_with("interrompu? "), "{cut}");
        assert!(!cut.starts_with("running"));
    }

    #[tokio::test]
    async fn read_only_store_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let ro = StateStore::load_read_only(&path).unwrap();
        ro.update(|s| {
            s.renamed_items.insert("x".into(), 1);
        })
        .await
        .unwrap();
        ro.update_lazy(|s| {
            s.renamed_items.insert("y".into(), 1);
        })
        .await
        .unwrap();
        ro.flush().await.unwrap();
        assert_eq!(
            ro.read(|s| s.renamed_items.len()).await,
            2,
            "visible en mémoire"
        );
        assert!(!path.exists(), "rien d'écrit sur disque");
    }

    /// Micro-banc avant/après (2026-10-07) : appels `write` pour sauvegarder un état de ~180 Ko.
    #[test]
    fn one_write_call_per_save() {
        #[derive(Default)]
        struct Counting {
            calls: usize,
            bytes: usize,
        }
        impl Write for Counting {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                self.bytes += buf.len();
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let state = big_state();
        // avant : serde_json branché sur le fichier, un `write` par jeton JSON
        let mut before = Counting::default();
        serde_json::to_writer_pretty(&mut before, &state).unwrap();
        // après : sérialisé en mémoire, un seul `write`
        let bytes = encode(&state).unwrap();
        let mut after = Counting::default();
        after.write_all(&bytes).unwrap();
        eprintln!(
            "état de {} octets : {} appels write avant, {} après",
            bytes.len(),
            before.calls,
            after.calls
        );
        assert!(bytes.len() > 150_000, "taille réaliste : {}", bytes.len());
        assert!(before.calls > 20_000, "{}", before.calls);
        assert_eq!(after.calls, 1);
        assert_eq!(
            before.bytes + 1,
            bytes.len(),
            "même contenu (+ saut de ligne)"
        );
    }

    /// Même mesure par le noyau (Linux) : `syscw` du fil courant pendant une vraie sauvegarde atomique.
    #[test]
    fn write_atomic_costs_one_syscall() {
        fn syscw() -> Option<u64> {
            std::fs::read_to_string("/proc/thread-self/io")
                .ok()?
                .lines()
                .find_map(|l| l.strip_prefix("syscw: ")?.trim().parse().ok())
        }
        if syscw().is_none() {
            eprintln!("/proc/thread-self/io illisible : mesure sautée");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let state = big_state();
        let old_path = dir.path().join("avant.json");
        let s0 = syscw().unwrap();
        {
            let mut tmp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            serde_json::to_writer_pretty(&mut tmp, &state).unwrap();
            tmp.write_all(b"\n").unwrap();
            tmp.as_file().sync_all().unwrap();
            tmp.persist(&old_path).unwrap();
        }
        let s1 = syscw().unwrap();
        let new_path = dir.path().join("apres.json");
        write_atomic(&new_path, &encode(&state).unwrap()).unwrap();
        let s2 = syscw().unwrap();
        eprintln!(
            "appels write (noyau) : {} avant, {} après",
            s1 - s0,
            s2 - s1
        );
        assert!(s1 - s0 > 20_000);
        assert!(s2 - s1 <= 2, "{}", s2 - s1);
        assert_eq!(
            std::fs::read(&old_path).unwrap(),
            std::fs::read(&new_path).unwrap(),
            "fichiers identiques"
        );
    }

    #[tokio::test]
    async fn update_returns_once_on_disk_and_identical_state_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path).unwrap();
        store.update(|s| stuck(s, "a")).await.unwrap();
        assert!(
            on_disk(&path).stuck.contains_key("a"),
            "écrit avant de rendre la main"
        );
        assert_eq!(store.disk_writes(), 1);
        store.update(|s| stuck(s, "a")).await.unwrap(); // même contenu
        store.update(|_| ()).await.unwrap();
        assert_eq!(store.disk_writes(), 1, "rien de changé : rien de réécrit");
        // un fichier relu tel quel n'est pas réécrit non plus
        let again = StateStore::load(&path).unwrap();
        again.update(|_| ()).await.unwrap();
        assert_eq!(again.disk_writes(), 0);
        again.update(|s| stuck(s, "b")).await.unwrap();
        assert_eq!(again.disk_writes(), 1);
        assert_eq!(on_disk(&path).stuck.len(), 2);
    }

    #[tokio::test]
    async fn lazy_mutations_ride_with_the_next_write_or_the_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path)
            .unwrap()
            .with_lazy_max_age(Duration::from_secs(3600));
        // premier passage : rien n'a encore été sérialisé, la tenue est écrite
        store
            .update_lazy(|s| {
                s.restarts.insert("a".into(), 1);
            })
            .await
            .unwrap();
        assert_eq!(on_disk(&path).restarts.len(), 1);
        // sérialisé à l'instant : la suivante attend
        store
            .update_lazy(|s| {
                s.restarts.insert("b".into(), 2);
            })
            .await
            .unwrap();
        assert_eq!(store.read(|s| s.restarts.len()).await, 2, "en mémoire");
        assert_eq!(on_disk(&path).restarts.len(), 1, "pas encore sur disque");
        // une mutation durable emporte tout
        store.update(|s| stuck(s, "x")).await.unwrap();
        let d = on_disk(&path);
        assert_eq!((d.restarts.len(), d.stuck.len()), (2, 1));
        // l'arrêt écrit ce qui reste
        store
            .update_lazy_if(true, |s| {
                s.restarts.insert("c".into(), 3);
            })
            .await
            .unwrap();
        assert_eq!(on_disk(&path).restarts.len(), 2);
        store.flush().await.unwrap();
        assert_eq!(on_disk(&path).restarts.len(), 3);
        let n = store.disk_writes();
        store.flush().await.unwrap();
        assert_eq!(store.disk_writes(), n, "rien de neuf : flush sans écriture");
        // délai écoulé : la mutation différée part aussitôt
        let eager = StateStore::load(&path)
            .unwrap()
            .with_lazy_max_age(Duration::ZERO);
        eager.update(|_| ()).await.unwrap();
        eager
            .update_lazy(|s| {
                s.restarts.insert("d".into(), 4);
            })
            .await
            .unwrap();
        assert_eq!(on_disk(&path).restarts.len(), 4);
    }

    /// 2026-10-08 : `dirty` était écrit mais jamais lu — une mutation différée attendait la suivante (des heures
    /// pour `deletion_cleanup`). `flush_if_dirty` écrit ce qui attend, et rien d'autre.
    #[tokio::test]
    async fn flush_if_dirty_writes_pending_lazy_mutations_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path)
            .unwrap()
            .with_lazy_max_age(Duration::from_secs(3600));
        assert!(!store.flush_if_dirty().await.unwrap(), "rien d'attente");
        assert_eq!(store.disk_writes(), 0);
        store
            .update_lazy(|s| {
                s.restarts.insert("a".into(), 1);
            })
            .await
            .unwrap(); // premier passage : écrit
        assert_eq!(store.disk_writes(), 1);
        assert!(
            !store.flush_if_dirty().await.unwrap(),
            "rien n'a changé depuis l'écriture"
        );
        store
            .update_lazy(|s| {
                s.restarts.insert("b".into(), 2);
            })
            .await
            .unwrap();
        assert_eq!(
            on_disk(&path).restarts.len(),
            1,
            "différée : pas sur disque"
        );
        assert!(store.flush_if_dirty().await.unwrap());
        assert_eq!(
            on_disk(&path).restarts.len(),
            2,
            "partie sans autre mutation"
        );
        assert_eq!(store.disk_writes(), 2);
        // plus rien à écrire : ni sérialisation ni écriture
        assert!(!store.flush_if_dirty().await.unwrap());
        assert_eq!(store.disk_writes(), 2);
        // une mutation durable remet le drapeau à zéro : le tour suivant n'a rien à faire
        store
            .update_lazy(|s| {
                s.restarts.insert("c".into(), 3);
            })
            .await
            .unwrap();
        store.update(|s| stuck(s, "x")).await.unwrap();
        let n = store.disk_writes();
        assert!(!store.flush_if_dirty().await.unwrap());
        assert_eq!(store.disk_writes(), n);
        assert_eq!(on_disk(&path).restarts.len(), 3);
    }

    #[tokio::test]
    async fn flush_if_dirty_keeps_retrying_after_a_failed_write() {
        let dir = tempfile::tempdir().unwrap();
        // le « dossier » de l'état est un fichier : toute écriture échoue
        let blocker = dir.path().join("bloc");
        std::fs::write(&blocker, b"x").unwrap();
        let store = StateStore::load(&blocker.join("state.json"))
            .unwrap()
            .with_lazy_max_age(Duration::from_secs(3600));
        assert!(store
            .update_lazy(|s| {
                s.restarts.insert("a".into(), 1);
            })
            .await
            .is_err());
        store
            .update_lazy(|s| {
                s.restarts.insert("b".into(), 2);
            })
            .await
            .unwrap(); // différée : en mémoire seulement
        assert!(store.flush_if_dirty().await.is_err());
        // le drapeau est remis : le tour suivant essaie encore au lieu de croire que tout est écrit
        assert!(store.flush_if_dirty().await.is_err());
        assert_eq!(store.disk_writes(), 0);
    }

    #[tokio::test]
    async fn read_only_store_never_flushes_if_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let ro = StateStore::load_read_only(&path).unwrap();
        ro.update_lazy(|s| {
            s.restarts.insert("a".into(), 1);
        })
        .await
        .unwrap();
        assert!(!ro.flush_if_dirty().await.unwrap());
        assert!(!path.exists());
    }

    /// La boucle du démon : une mutation différée est sur disque au tour suivant, sans autre mutation.
    #[tokio::test]
    async fn the_lazy_flusher_writes_within_one_tick() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path)
            .unwrap()
            .with_lazy_max_age(Duration::from_secs(3600));
        let flusher = {
            let s = store.clone();
            tokio::spawn(async move { s.run_lazy_flusher(Duration::from_millis(20)).await })
        };
        for k in ["a", "b"] {
            store
                .update_lazy(|s| {
                    s.restarts.insert(k.into(), 1);
                })
                .await
                .unwrap();
        }
        assert_eq!(on_disk(&path).restarts.len(), 1, "la 2e est différée");
        let deadline = Instant::now() + Duration::from_secs(10);
        while on_disk(&path).restarts.len() < 2 {
            assert!(Instant::now() < deadline, "jamais écrite par la boucle");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        flusher.abort();
    }

    #[test]
    fn an_older_snapshot_never_replaces_a_newer_one() {
        let disk = Disk::new(Vec::new());
        let snap = |v: u64| Snapshot {
            version: v,
            bytes: vec![v as u8],
        };
        disk.offer(snap(2));
        disk.offer(snap(1)); // arrivé en retard
        assert_eq!(disk.take().map(|s| s.version), Some(2));
        disk.offer(snap(3));
        disk.offer(snap(2)); // écriture ratée d'un plus ancien, remise en attente : refusée
        assert_eq!(disk.take().map(|s| s.version), Some(3));
        assert!(disk.take().is_none());
    }

    #[tokio::test]
    async fn coalesced_writes_follow_mutation_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path).unwrap();
        // trois mutations sérialisées avant toute écriture : la première écriture prend la plus récente
        let (v1, v2, v3) = {
            let mut g = store.inner.lock().await;
            stuck(&mut g.state, "1");
            let v1 = store.snapshot(&mut g).unwrap();
            stuck(&mut g.state, "2");
            let v2 = store.snapshot(&mut g).unwrap();
            stuck(&mut g.state, "3");
            (v1, v2, store.snapshot(&mut g).unwrap())
        };
        store.write_up_to(v1).await.unwrap();
        assert_eq!(on_disk(&path).stuck.len(), 3, "le plus récent, directement");
        store.write_up_to(v3).await.unwrap();
        store.write_up_to(v2).await.unwrap();
        assert_eq!(store.disk_writes(), 1, "les autres sont déjà couverts");
        assert_eq!(on_disk(&path).stuck.len(), 3, "jamais un état plus ancien");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_updates_never_leave_a_partial_or_older_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = StateStore::load(&path).unwrap();
        store.update(|s| *s = big_state()).await.unwrap();
        // lecteur indépendant : à tout instant le fichier est un état complet, et jamais plus ancien que le précédent lu
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let (mut reads, mut last) = (0u32, 0usize);
                while !stop.load(Ordering::Relaxed) {
                    let raw = std::fs::read(&path).expect("le fichier existe toujours");
                    let s: State = serde_json::from_slice(&raw).expect("jamais un état partiel");
                    assert!(s.torrent_import.len() == 600, "jamais un état tronqué");
                    assert!(s.stuck.len() >= last, "jamais un état plus ancien");
                    last = s.stuck.len();
                    reads += 1;
                }
                reads
            })
        };
        let mut tasks = Vec::new();
        for t in 0..8 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                for i in 0..25 {
                    store
                        .update(|s| stuck(s, &format!("{t}:{i}")))
                        .await
                        .unwrap();
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let reads = reader.join().unwrap();
        let d = on_disk(&path);
        assert_eq!(d.stuck.len(), 200, "toutes les mutations sur disque");
        assert_eq!(store.read(|s| s.stuck.len()).await, 200);
        assert!(store.disk_writes() <= 201);
        assert!(reads > 0);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "aucun fichier temporaire oublié"
        );
    }

    #[tokio::test]
    async fn a_failed_write_is_retried_by_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("sub");
        std::fs::write(&blocker, b"un fichier, pas un dossier").unwrap();
        let path = blocker.join("state.json");
        let store = StateStore::load(&path).unwrap();
        assert!(store.update(|s| stuck(s, "a")).await.is_err());
        assert_eq!(store.read(|s| s.stuck.len()).await, 1, "gardé en mémoire");
        std::fs::remove_file(&blocker).unwrap();
        store.update(|s| stuck(s, "b")).await.unwrap();
        assert_eq!(on_disk(&path).stuck.len(), 2, "rien de perdu");
    }
}
