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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub last_start: i64,
    pub last_end: Option<i64>,
    pub last_ok: Option<bool>,
    pub last_summary: String,
    pub runs: u64,
    pub errors: u64,
}

/// Une mutation différée (`update_lazy`) n'entraîne pas d'écriture si l'état a été sérialisé depuis moins que ça :
/// elle part avec l'écriture suivante, au plus tard à la première mutation qui suit ce délai, ou à l'arrêt.
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
    /// en mémoire, écrite avec la sauvegarde suivante — au plus tard `LAZY_MAX_AGE` après la précédente, ou à
    /// l'arrêt (`flush`). Une coupure brutale peut en perdre une minute, jamais une mutation faite par `update`.
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
