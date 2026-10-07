use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// identity_check : dernière tentative de renommage d'une fiche au nom de release (id Jellyfin → date),
    /// pour ne pas réessayer sans fin un titre que TMDB nomme ainsi.
    #[serde(default)]
    pub renamed_items: BTreeMap<String, i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeasonSearchRecord {
    pub at: i64,
    /// `grabbed`, `none`, `error`.
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TorrentImportRecord {
    pub at: i64,
    pub name: String,
    /// `imported`, `arr_managed`, `already_linked`, `no_video`, `no_match`, `dup_other_side`,
    /// `nothing_importable`, `error` (définitifs) ; `retry` (nouvel essai au passage suivant).
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub attempts: u32,
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

impl RunInfo {
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

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    inner: Arc<Mutex<State>>,
    /// Copie de `homelabctl` : jamais écrite. Le daemon est seul propriétaire du fichier (une CLI qui l'écrivait
    /// écrasait l'état du daemon, qui l'écrasait en retour à sa sauvegarde suivante — audit du 2026-09-23, E9).
    read_only: bool,
}

impl StateStore {
    pub fn load(path: &Path) -> Result<Self> {
        let state = if path.is_file() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("lecture de {}", path.display()))?;
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?
        } else {
            State::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            inner: Arc::new(Mutex::new(state)),
            read_only: false,
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
        f(&guard)
    }

    /// Applique une mutation puis persiste atomiquement (tempfile + rename).
    pub async fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        let mut guard = self.inner.lock().await;
        let out = f(&mut guard);
        self.persist(&guard)?;
        Ok(out)
    }

    fn persist(&self, state: &State) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        let dir = self
            .path
            .parent()
            .context("state_file sans dossier parent")?;
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, state)?;
        tmp.write_all(b"\n")?;
        // sur disque AVANT le renommage : sinon une coupure peut laisser un fichier d'état vide
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path)
            .map_err(|e| anyhow::anyhow!("écriture {} : {}", self.path.display(), e.error))?;
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all(); // le renommage lui-même
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_persists_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");
        let store = StateStore::load(&path).unwrap();
        store
            .update(|s| {
                s.stuck.insert(
                    "sonarr:abc".into(),
                    StuckEntry {
                        service: "sonarr".into(),
                        download_id: "abc".into(),
                        first_seen: 1,
                        title: "t".into(),
                    },
                );
            })
            .await
            .unwrap();
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
        assert_eq!(
            ro.read(|s| s.renamed_items.len()).await,
            1,
            "visible en mémoire"
        );
        assert!(!path.exists(), "rien d'écrit sur disque");
    }
}
