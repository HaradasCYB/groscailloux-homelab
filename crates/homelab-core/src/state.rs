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
    /// anime_library : classement TMDB en cache, clé `tv:<tmdb>` / `movie:<tmdb>` → (date, classe).
    #[serde(default)]
    pub anime_class: BTreeMap<String, AnimeClassRecord>,
    /// anime_library : fiches déplacées, clé `côté:movie:<id>` / `côté:series:<id>` → date ;
    /// deletion_cleanup les laisse tranquilles quelques heures.
    #[serde(default)]
    pub anime_moves: BTreeMap<String, i64>,
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
    /// Dates (secondes) des inscriptions par la page publique : plafond journalier.
    #[serde(default)]
    pub signups: Vec<i64>,
    /// Dates des renvois de lien par adresse (clé = SHA-256 hex de l'adresse) : plafond horaire.
    #[serde(default)]
    pub link_renewals: BTreeMap<String, Vec<i64>>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub last_start: i64,
    pub last_end: Option<i64>,
    pub last_ok: Option<bool>,
    pub last_summary: String,
    pub runs: u64,
    pub errors: u64,
}

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    inner: Arc<Mutex<State>>,
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
        })
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
        let dir = self
            .path
            .parent()
            .context("state_file sans dossier parent")?;
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, state)?;
        tmp.write_all(b"\n")?;
        tmp.persist(&self.path)
            .map_err(|e| anyhow::anyhow!("écriture {} : {}", self.path.display(), e.error))?;
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
}
