//! Moteur « Nettoyer » de la liste rouge (2026-10-20, décidé par l'admin) : supprime **proprement partout** les titres
//! rouges confirmés sur `/accounts/liste-rouge`. Un seul travail à la fois ; tout est revérifié au moment d'agir.
//!
//! **Début** : quota de la seedbox relevé, puis **nouveau calcul complet** de la liste ([`red_list::compute`]) ; refus si
//! un côté Arr est injoignable ou si le calcul est incomplet (Playback Reporting muet, compte illisible). Ne sont traités
//! que les titres à la fois confirmés par l'admin (clés lues dans l'état par la page, jamais dans le formulaire), rouges
//! maintenant et non exemptés, `max_titles_per_clean` au plus, les plus gros d'abord.
//!
//! **Pour chaque titre** :
//! 1. revérifications : exemption, lecture en cours (`playing_paths`), rangement récent par `anime_library`, fiche
//!    relue dont l'identifiant TMDB/TVDB et le dossier correspondent toujours ;
//! 2. sauvegarde de chaque fiche (JSON, 600) dans `<backups>/liste-rouge-<date>/` (dossier en 700) ;
//! 3. relevés avant suppression : torrents d'origine (`source_hashes` : l'historique part avec la fiche), taille
//!    exclusive selon les liens physiques (`nlink` sur le VPS ; script python **fixe** par ssh sur la seedbox, chemin
//!    par l'entrée standard) ;
//! 4. `DELETE` de la fiche avec ses fichiers (`deleteFiles`), jamais rejoué ; erreur ou délai dépassé : fiche relue, un
//!    404 vaut réussite, sinon ce côté s'arrête là (ni torrent, ni corbeille, ni Seerr) ;
//! 5. corbeille de l'Arr (sans quoi rien n'est libéré avant 14 j) : cible `<corbeille>/<dossier du titre>`, refusée si
//!    elle contient `..`, vise une racine ou sort des corbeilles connues (`[cleanup] recycle_dirs` sur le VPS, racine des
//!    médias sur la seedbox) ; vidée une fois que l'Arr y a déplacé le dossier ; échec : étape à rejouer ;
//! 6. torrents d'origine qui ne servent plus : retirés, ou mis en attente pour C411 (`deletions.pending_torrents`, que
//!    `deletion_cleanup` traite déjà). **Après** la corbeille, à la différence de l'ordre du plan : sur le VPS, le fichier
//!    déplacé dans la corbeille garde son lien physique et `still_used` jugerait le torrent encore utilisé ;
//! 7. montage seedbox : `vfs/forget` du dossier, `vfs/refresh` de ses parents ;
//! 8. Seerr : si le titre n'a plus de fichiers nulle part, son média est retiré (un membre peut le redemander) ;
//!    échec : étape à rejouer ;
//! 9. canari : vidé s'il lisait ce titre (sinon fausse alerte de `playback_canary`) ;
//! 10. résultat enregistré dans l'état (écriture durable), titre par titre.
//!
//! **Fin** : un seul `Library/Media/Updated` en `Deleted` pour tous les dossiers (analyse ciblée, jamais
//! `DELETE /Items`), titres supprimés retirés du dernier calcul, alerte admin récapitulative (mail et Discord).
//!
//! **Simulation** (`clean_dry_run` pour tout travail de plus d'un titre, `--dry-run` toujours) : aucune écriture ailleurs
//! que dans l'état, où le travail est noté « simulation » ; chaque action prévue est journalisée « dry-run: would … ».
//! Les lectures (fiches, historique, tailles, corbeille, média Seerr) ont lieu comme pour de vrai.
//!
//! **Reprise** : relancer reprend ce qui est encore rouge ; un travail coupé par un redémarrage est marqué
//! « interrompu » au passage suivant de la tâche `red_list`, qui rejoue aussi les étapes en attente
//! ([`replay_pending`], [`PENDING_PER_RUN`] par passage, abandonnées après 14 j).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::alerts::{self, Level};
use crate::clients::{ArrClient, Torrent};
use crate::context::{Side, TaskContext};
use crate::docker;
use crate::quota::{self, SeedboxQuota};
use crate::state::{
    now, CleanJob, CleanResult, Exemption, PendingStep, QuotaReading, RedEntry, RedListReport,
    RedSide,
};
use crate::tasks::anime_library::MOVE_GRACE_SECS;
use crate::tasks::catalogue_report::{human_size, pick_admin};
use crate::tasks::deletion_cleanup::{self as dc, map_path, side_maps, PathMap};
use crate::tasks::red_list::{self, parse_key};
use crate::tasks::subtitle_sync::sh_quote;

const TASK: &str = "red_list";

/// Étapes en attente (corbeille, Seerr) rejouées par passage de la tâche `red_list`.
pub const PENDING_PER_RUN: usize = 5;
/// Au-delà, une étape en attente est abandonnée (journalisée) : l'Arr a vidé sa corbeille de lui-même.
pub const PENDING_MAX_AGE_SECS: i64 = 14 * 86_400;
/// Intervalle entre deux regards sur le dossier d'origine pendant que l'Arr le déplace dans sa corbeille.
const RECYCLE_POLL: Duration = Duration::from_secs(3);
/// Relevé des tailles d'un dossier (série : quelques centaines de fichiers, par ssh pour la seedbox).
const MEASURE_TIMEOUT: Duration = Duration::from_secs(120);
/// Suppression d'un dossier de la corbeille (série entière).
const REMOVE_TIMEOUT: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------------------------
// Un seul travail à la fois
// ---------------------------------------------------------------------------------------------

fn slot() -> &'static Arc<tokio::sync::Mutex<()>> {
    static SLOT: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

/// Identifiant du travail en cours dans ce processus : un travail « en cours » de l'état qui n'est pas celui-ci a été
/// coupé par un redémarrage.
fn current() -> &'static std::sync::Mutex<Option<String>> {
    static CURRENT: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();
    CURRENT.get_or_init(|| std::sync::Mutex::new(None))
}

/// La place de l'unique travail ; rendue quand elle tombe (fin, erreur ou panique), le travail courant oublié avec.
struct Running {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        *current().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn reserve(job_id: &str) -> Option<Running> {
    let guard = slot().clone().try_lock_owned().ok()?;
    *current().lock().unwrap_or_else(|e| e.into_inner()) = Some(job_id.to_string());
    Some(Running { _guard: guard })
}

/// Un « Nettoyer » (ou un « Recalculer » de la page) tient la place dans ce processus : la page refuse d'en lancer un
/// second.
pub fn busy() -> bool {
    slot().try_lock().is_err()
}

/// La place de l'unique travail tenue par un « Recalculer » de la page : jamais pendant un « Nettoyer », ni l'inverse.
/// Ce n'est pas un travail : aucun « en cours » dans l'état, rien à marquer « interrompu ». Rendue quand elle tombe.
pub struct Slot {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Réserve la place pour un recalcul ; `None` si un « Nettoyer » ou un autre recalcul la tient.
pub fn reserve_recompute() -> Option<Slot> {
    let guard = slot().clone().try_lock_owned().ok()?;
    Some(Slot { _guard: guard })
}

/// Identifiant d'un nouveau travail : date locale et suffixe aléatoire (`20261021-101500-3fa2`).
pub fn new_job_id() -> String {
    format!(
        "{}-{:04x}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        rand::random::<u16>()
    )
}

/// Simulation ou non : demandée par l'appelant, `--dry-run` ou `clean_dry_run`. Aucune exception, pas même pour un
/// titre seul (`…/nettoyer?key=`) : l'essai réel exige `clean_dry_run = false`.
pub fn effective_dry(requested: bool, clean_dry_run: bool, global_dry: bool) -> bool {
    requested || global_dry || clean_dry_run
}

// ---------------------------------------------------------------------------------------------
// Confirmation : empreinte et sélection
// ---------------------------------------------------------------------------------------------

/// Empreinte de la liste confirmée : SHA-256 (hexadécimal) de la date du calcul et des triplets (clé, `côté:id`,
/// octets) triés. Ne dépend ni de l'ordre des clés ni de celui des copies ; change si le calcul, un titre, une copie ou
/// une taille change. Une clé absente des rouges du rapport (exemptée, plus rouge, disparue) compte `(clé, absent, 0)`.
pub fn snap(report: &RedListReport, keys: &[String]) -> String {
    let keys: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
    let mut lines: Vec<String> = Vec::new();
    for k in keys {
        match report.entries.iter().find(|e| e.key == k) {
            Some(e) if !e.sides.is_empty() => lines.extend(
                e.sides
                    .iter()
                    .map(|s| format!("{k}\t{}:{}\t{}", s.side, s.arr_id, s.bytes)),
            ),
            Some(e) => lines.push(format!("{k}\taucun\t{}", e.bytes)),
            None => lines.push(format!("{k}\tabsent\t0")),
        }
    }
    lines.sort();
    let mut h = Sha256::new();
    h.update(report.at.to_string().as_bytes());
    h.update(b"\n");
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Ce que « Nettoyer » propose (page de confirmation) : les `max` premiers rouges non exemptés du dernier calcul, dans
/// son ordre (les plus gros d'abord).
pub fn proposed<'a>(
    report: &'a RedListReport,
    exemptions: &BTreeMap<String, Exemption>,
    max: usize,
) -> Vec<&'a RedEntry> {
    report
        .entries
        .iter()
        .filter(|e| !e.key.is_empty() && !exemptions.contains_key(&e.key))
        .take(max)
        .collect()
}

/// Titres qu'un travail traite : l'intersection des clés confirmées, des rouges du nouveau calcul et des titres non
/// exemptés, dans l'ordre du calcul, `max` au plus. Rend aussi chaque clé confirmée écartée et pourquoi.
pub fn select<'a>(
    confirmed: &[String],
    report: &'a RedListReport,
    exemptions: &BTreeMap<String, Exemption>,
    max: usize,
) -> (Vec<&'a RedEntry>, Vec<(String, &'static str)>) {
    let wanted: BTreeSet<&str> = confirmed.iter().map(String::as_str).collect();
    let mut picked: Vec<&RedEntry> = Vec::new();
    let mut skipped: Vec<(String, &'static str)> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for e in &report.entries {
        if !wanted.contains(e.key.as_str()) || !seen.insert(e.key.as_str()) {
            continue;
        }
        if exemptions.contains_key(&e.key) {
            skipped.push((e.key.clone(), "exempté"));
        } else if picked.len() >= max {
            skipped.push((e.key.clone(), "au-delà du plafond par clic"));
        } else {
            picked.push(e);
        }
    }
    let mut gone: Vec<(String, &'static str)> = confirmed
        .iter()
        .filter(|k| !seen.contains(k.as_str()))
        .map(|k| (k.clone(), "plus rouge au nouveau calcul"))
        .collect();
    gone.sort();
    gone.dedup();
    skipped.extend(gone);
    (picked, skipped)
}

/// La fiche relue est-elle toujours celle du titre confirmé : même identifiant d'Arr, même identifiant TMDB/TVDB que
/// la clé, même dossier ?
pub fn card_matches(e: &RedEntry, side: &RedSide, fiche: &Value) -> bool {
    let id = |k: &str| fiche.get(k).and_then(Value::as_i64).unwrap_or(0);
    let ids = match parse_key(&e.key) {
        Some((true, "tmdb", n)) => e.movie && id("tmdbId") == n,
        Some((false, "tvdb", n)) => !e.movie && id("tvdbId") == n,
        Some((false, "tmdb", n)) => !e.movie && id("tmdbId") == n,
        _ => false,
    };
    ids && id("id") == side.arr_id
        && fiche.get("path").and_then(Value::as_str) == Some(side.folder.as_str())
}

/// Fiche rangée par `anime_library` il y a moins de [`MOVE_GRACE_SECS`] (clé `côté:movie|series:id`).
fn recently_moved(
    moves: &BTreeMap<String, i64>,
    side: &str,
    movie: bool,
    id: i64,
    at: i64,
) -> bool {
    let kind = if movie { "movie" } else { "series" };
    moves
        .get(&format!("{side}:{kind}:{id}"))
        .is_some_and(|t| at - t < MOVE_GRACE_SECS)
}

/// Une lecture en cours sous ce dossier (chemins Jellyfin).
fn is_playing(playing: &[String], jf_dir: &str) -> bool {
    let prefix = format!("{}/", jf_dir.trim_end_matches('/'));
    playing.iter().any(|p| p.starts_with(&prefix))
}

fn norm(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

// ---------------------------------------------------------------------------------------------
// Corbeille : gardes
// ---------------------------------------------------------------------------------------------

/// Chemin absolu sans `.` ni `..` (doubles barres et barre finale retirées) ; `None` sinon.
fn clean_abs(p: &str) -> Option<PathBuf> {
    let path = Path::new(p);
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => out.push(n),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Strictement sous `root` (composant par composant), sans `..` d'un côté ni de l'autre.
fn strictly_under(path: &str, root: &str) -> bool {
    match (clean_abs(path), clean_abs(root)) {
        (Some(p), Some(r)) => r.components().count() > 1 && p != r && p.starts_with(&r),
        _ => false,
    }
}

/// Nom du dossier d'un titre (`/movies/Titre (2020)` → `Titre (2020)`) : refusé s'il est vide, `.`, `..` ou à la
/// racine.
fn folder_name(folder: &str) -> Result<String, String> {
    let p = clean_abs(folder).ok_or_else(|| format!("dossier du titre invalide : {folder}"))?;
    if p.components().count() < 3 {
        return Err(format!("dossier du titre trop haut : {folder}"));
    }
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .ok_or_else(|| format!("dossier du titre sans nom : {folder}"))
}

/// Corbeille du VPS sur l'hôte : celle de `[cleanup] recycle_dirs` qui correspond à la corbeille de l'Arr. Par
/// `vps_paths` d'abord ; à défaut par la fin du chemin : les Arrs du VPS voient la bibliothèque sous `/data` (leurs
/// racines `/movies`, `/tv` n'en sont que des liens), d'où `/data/media/movies/.recycle` ↔
/// `…/library/media/movies/.recycle`. Au moins deux composants communs (racine et corbeille) et un seul candidat.
pub fn vps_recycle_dir(
    arr_bin: &str,
    maps: &[PathMap],
    recycle_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let bin = clean_abs(arr_bin)?;
    let known: Vec<PathBuf> = recycle_dirs
        .iter()
        .filter_map(|d| clean_abs(&d.to_string_lossy()))
        .filter(|d| d.components().count() > 2)
        .collect();
    if let Some((host, _)) = map_path(maps, &bin.to_string_lossy()) {
        let host = clean_abs(&host.to_string_lossy())?;
        return known.into_iter().find(|d| *d == host);
    }
    let tail = |d: &Path| {
        bin.components()
            .rev()
            .zip(d.components().rev())
            .take_while(|(a, b)| a == b && matches!(a, Component::Normal(_)))
            .count()
    };
    let mut found = known.into_iter().filter(|d| tail(d) >= 2);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Cible VPS : (corbeille sur l'hôte, `<corbeille>/<dossier du titre>`), strictement sous une corbeille connue.
pub fn vps_recycle_target(
    arr_bin: &str,
    folder: &str,
    maps: &[PathMap],
    recycle_dirs: &[PathBuf],
) -> Result<(PathBuf, PathBuf), String> {
    let dir = vps_recycle_dir(arr_bin, maps, recycle_dirs)
        .ok_or_else(|| format!("corbeille de l'Arr ({arr_bin}) hors de [cleanup] recycle_dirs"))?;
    let target = dir.join(folder_name(folder)?);
    if target.parent() != Some(dir.as_path()) {
        return Err("cible hors de la corbeille".into());
    }
    Ok((dir, target))
}

/// Corbeille d'une cible VPS à rejouer : son parent, s'il est l'une des corbeilles connues.
fn vps_replay_bin(target: &str, recycle_dirs: &[PathBuf]) -> Option<PathBuf> {
    let t = clean_abs(target)?;
    let parent = t.parent()?.to_path_buf();
    folder_name(target).ok()?;
    recycle_dirs
        .iter()
        .filter_map(|d| clean_abs(&d.to_string_lossy()))
        .find(|d| *d == parent && d.components().count() > 2)
}

/// Cible seedbox : (corbeille, `<corbeille>/<dossier du titre>`), la corbeille strictement sous la racine des médias.
pub fn seedbox_recycle_target(
    arr_bin: &str,
    folder: &str,
    media_root: &str,
) -> Result<(String, String), String> {
    if !strictly_under(arr_bin, media_root) {
        return Err(format!(
            "corbeille de l'Arr ({arr_bin}) hors de la racine des médias"
        ));
    }
    let bin = clean_abs(arr_bin).ok_or("corbeille invalide")?;
    let target = bin.join(folder_name(folder)?);
    Ok((
        bin.to_string_lossy().to_string(),
        target.to_string_lossy().to_string(),
    ))
}

/// Corbeille d'une cible seedbox à rejouer : son parent, strictement sous la racine des médias.
fn seedbox_replay_bin(target: &str, media_root: &str) -> Option<String> {
    let t = clean_abs(target)?;
    folder_name(target).ok()?;
    let bin = t.parent()?.to_string_lossy().to_string();
    strictly_under(&bin, media_root).then_some(bin)
}

// ---------------------------------------------------------------------------------------------
// Seedbox : scripts python fixes, chemins par l'entrée standard
// ---------------------------------------------------------------------------------------------

/// Tailles d'un dossier selon les liens physiques. Entrée : `{"dir": …}`.
pub const MEASURE_PY: &str = r#"import json, os, stat, sys
d = json.load(sys.stdin)
root = d["dir"]
out = {"exclusive": 0, "linked": 0, "files": 0, "missing": os.path.islink(root) or not os.path.isdir(root)}
if not out["missing"]:
    for top, dirs, files in os.walk(root):
        for f in files:
            try:
                st = os.lstat(os.path.join(top, f))
            except OSError:
                continue
            if stat.S_ISREG(st.st_mode):
                out["files"] += 1
                out["exclusive" if st.st_nlink == 1 else "linked"] += st.st_size
print(json.dumps(out))
"#;

/// Vide `target` de la corbeille, une fois `origin` disparu (attente de `wait` secondes au plus). Entrée :
/// `{"base", "recycle", "target", "origin", "wait"}`. Gardes rejouées sur la seedbox (`realpath`) : corbeille sous
/// `base`, cible enfant direct de la corbeille, jamais un lien symbolique. Sortie : `removed`, `absent`, `waiting` ou
/// `refused:<motif>`.
pub const RECYCLE_PY: &str = r#"import json, os, shutil, sys, time
d = json.load(sys.stdin)
def done(s):
    print(s)
    sys.exit(0)
base = os.path.realpath(d["base"])
rb = os.path.realpath(d["recycle"])
target = d["target"]
origin = d.get("origin") or ""
if not rb.startswith(base + os.sep):
    done("refused:recycle")
if os.path.dirname(target.rstrip("/")) != d["recycle"].rstrip("/"):
    done("refused:target")
if origin and not os.path.realpath(origin).startswith(base + os.sep):
    done("refused:origin")
deadline = time.time() + float(d.get("wait") or 0)
while origin and os.path.lexists(origin) and time.time() < deadline:
    time.sleep(3)
if origin and os.path.lexists(origin):
    done("waiting")
if not os.path.lexists(target):
    done("absent")
if os.path.islink(target) or not os.path.isdir(target):
    done("refused:target")
rt = os.path.realpath(target)
if os.path.dirname(rt) != rb or rt == rb:
    done("refused:target")
shutil.rmtree(rt)
done("removed")
"#;

/// Arguments de `ssh` pour un script python **fixe** : seuls des options, l'alias et le script (constant) sont sur la
/// ligne de commande ; les chemins passent par l'entrée standard (JSON), jamais par la ligne recollée par ssh.
pub fn ssh_args(host: &str, script: &str) -> Vec<String> {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=20",
        // connexion morte détectée en une minute
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=4",
        host,
    ]
    .into_iter()
    .map(str::to_string)
    .chain(std::iter::once(format!("python3 -c {}", sh_quote(script))))
    .collect()
}

async fn ssh_python(
    ctx: &TaskContext,
    script: &str,
    input: &Value,
    limit: Duration,
) -> Result<String> {
    let args = ssh_args(&ctx.cfg.tasks.red_list.ssh_host, script);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    tokio::time::timeout(
        limit,
        docker::run_stdin("ssh", &args, input.to_string().as_bytes(), None),
    )
    .await
    .context("ssh seedbox : délai dépassé")?
}

// ---------------------------------------------------------------------------------------------
// Tailles et comptes
// ---------------------------------------------------------------------------------------------

/// Tailles d'un dossier selon ses liens physiques.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Sizes {
    /// Fichiers sans autre lien : libérés dès que le dossier part (corbeille comprise).
    pub exclusive: u64,
    /// Fichiers liés ailleurs (le plus souvent au torrent d'origine) : libérés avec leur dernier lien.
    pub linked: u64,
    pub files: u64,
    /// Dossier absent (ou lien symbolique : jamais suivi).
    #[serde(default)]
    pub missing: bool,
}

/// Parcourt un dossier local sans suivre les liens symboliques.
pub fn tally_dir(dir: &Path) -> Sizes {
    use std::os::unix::fs::MetadataExt;
    let mut s = Sizes::default();
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        s.missing = true;
        return s;
    }
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(m) = entry.metadata() else {
            continue;
        };
        s.files += 1;
        if m.nlink() > 1 {
            s.linked += m.len();
        } else {
            s.exclusive += m.len();
        }
    }
    s
}

/// Répartition d'une copie supprimée : (libéré tout de suite, plus tard, partagé). Le non-lié part avec le dossier, ou
/// plus tard s'il reste dans la corbeille ; le lié part avec ses torrents : tous retirés (et corbeille vidée) → tout de
/// suite, retirés ou en attente C411 → plus tard, sinon (torrent gardé, en erreur ou introuvable) → partagé.
pub fn account(sizes: Sizes, recycle_done: bool, torrents: &[&str]) -> (u64, u64, u64) {
    let (mut now, mut later, mut shared) = (0, 0, 0);
    if recycle_done {
        now += sizes.exclusive;
    } else {
        later += sizes.exclusive;
    }
    let any = !torrents.is_empty();
    let all_deleted = any && torrents.iter().all(|n| *n == "torrent_deleted");
    let all_leaving = any
        && torrents
            .iter()
            .all(|n| matches!(*n, "torrent_deleted" | "torrent_pending"));
    if all_deleted && recycle_done {
        now += sizes.linked;
    } else if all_leaving {
        later += sizes.linked;
    } else {
        shared += sizes.linked;
    }
    (now, later, shared)
}

// ---------------------------------------------------------------------------------------------
// Étapes à rejouer, travaux
// ---------------------------------------------------------------------------------------------

/// Clé d'une étape en attente : `<étape>:<côté>:<clé du titre>` (côté vide pour `seerr`).
pub fn pending_key(p: &PendingStep) -> String {
    format!("{}:{}:{}", p.step, p.side, p.key)
}

/// Ajoute une étape à rejouer ; idempotent : une étape déjà en attente garde sa date de départ et son compte d'essais.
/// `true` si elle est nouvelle.
pub fn add_pending(map: &mut BTreeMap<String, PendingStep>, step: PendingStep) -> bool {
    match map.entry(pending_key(&step)) {
        std::collections::btree_map::Entry::Vacant(v) => {
            v.insert(step);
            true
        }
        std::collections::btree_map::Entry::Occupied(_) => false,
    }
}

/// Étapes à rejouer à ce passage (les moins récemment tentées d'abord, `max` au plus) et étapes à abandonner (en
/// attente depuis `max_age` ou plus).
pub fn replay_plan(
    pending: &BTreeMap<String, PendingStep>,
    now: i64,
    max: usize,
    max_age: i64,
) -> (Vec<String>, Vec<String>) {
    let mut expired = Vec::new();
    let mut due: Vec<(i64, &String)> = Vec::new();
    for (k, p) in pending {
        if now - p.first_at >= max_age {
            expired.push(k.clone());
        } else {
            due.push((p.last_try, k));
        }
    }
    due.sort();
    (
        due.into_iter().take(max).map(|(_, k)| k.clone()).collect(),
        expired,
    )
}

/// Remplace (ou ajoute) un travail par son identifiant et ne garde que les `kept` plus récents.
pub fn upsert_job(jobs: &mut Vec<CleanJob>, job: &CleanJob, kept: usize) {
    match jobs.iter_mut().find(|j| j.id == job.id) {
        Some(j) => *j = job.clone(),
        None => jobs.push(job.clone()),
    }
    let kept = kept.max(1);
    if jobs.len() > kept {
        let extra = jobs.len() - kept;
        jobs.drain(..extra);
    }
}

/// Marque « interrompu » chaque travail resté en cours qui n'est pas `current`. Rend leur nombre.
pub fn interrupt_stale(jobs: &mut [CleanJob], current: Option<&str>, at: i64) -> usize {
    let mut n = 0;
    for j in jobs
        .iter_mut()
        .filter(|j| j.status == "running" && Some(j.id.as_str()) != current)
    {
        j.status = "interrupted".into();
        j.ended = Some(at);
        j.detail = "homelabd redémarré en plein travail : relancer « Nettoyer » reprend ce qui reste rouge".into();
        n += 1;
    }
    n
}

/// Octets d'un travail : (libérés tout de suite, plus tard, partagés).
pub fn totals(results: &[CleanResult]) -> (u64, u64, u64) {
    results.iter().fold((0, 0, 0), |(a, b, c), r| {
        (a + r.freed_now, b + r.freed_later, c + r.shared)
    })
}

fn count(results: &[CleanResult], outcome: &str) -> usize {
    results.iter().filter(|r| r.outcome == outcome).count()
}

/// Résumé d'un travail terminé (une ligne, sur la page et dans `detail`).
pub fn job_summary(job: &CleanJob) -> String {
    let (now_b, later_b, shared_b) = totals(&job.results);
    let mut parts = Vec::new();
    for (outcome, label) in [
        ("deleted", "supprimé(s)"),
        ("simulated", "simulé(s)"),
        ("partial", "en partie"),
        ("error", "en erreur"),
        ("skipped", "écarté(s)"),
    ] {
        let n = count(&job.results, outcome);
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    if parts.is_empty() {
        parts.push("aucun titre".into());
    }
    format!(
        "{} · {} tout de suite, {} plus tard, {} partagé(s)",
        parts.join(", "),
        human_size(now_b),
        human_size(later_b),
        human_size(shared_b)
    )
}

/// Objet, corps et niveau (avertissement ?) de l'alerte de fin de travail : titres seulement, jamais de pseudo. `None`
/// si rien n'a été tenté (tout écarté, ou simulation).
pub fn alert_text(job: &CleanJob) -> Option<(String, String, bool)> {
    let acted: Vec<&CleanResult> = job
        .results
        .iter()
        .filter(|r| matches!(r.outcome.as_str(), "deleted" | "partial" | "error"))
        .collect();
    if acted.is_empty() || job.dry_run {
        return None;
    }
    let (now_b, later_b, shared_b) = totals(&job.results);
    let warn = acted.iter().any(|r| r.outcome != "deleted");
    let subject = format!(
        "Liste rouge : {} titre(s) supprimé(s), {} libérés",
        count(&job.results, "deleted"),
        human_size(now_b)
    );
    let quota = job.quota_before.map_or_else(
        || "inconnu".to_string(),
        |q| {
            SeedboxQuota {
                used_kb: q.used_kb,
                quota_kb: q.quota_kb,
                at: q.at,
            }
            .describe()
        },
    );
    let mut body = format!(
        "Nettoyage de la liste rouge (travail {}).\n\
         Libéré tout de suite : {}.\n\
         Libéré plus tard : {} (torrents C411 qui doivent encore partager, corbeille à vider).\n\
         Encore lié ailleurs : {}.\n\
         Quota de la seedbox avant : {quota}.\n\n",
        job.id,
        human_size(now_b),
        human_size(later_b),
        human_size(shared_b),
    );
    for r in &acted {
        let what = match r.outcome.as_str() {
            "deleted" => "supprimé",
            "partial" => "supprimé en partie",
            _ => "erreur",
        };
        body.push_str(&format!(
            "- {} : {what}, {} libérés",
            r.title,
            human_size(r.freed_now)
        ));
        if r.freed_later > 0 {
            body.push_str(&format!(", {} plus tard", human_size(r.freed_later)));
        }
        if !r.detail.is_empty() {
            body.push_str(&format!(" ({})", r.detail));
        }
        body.push('\n');
    }
    let skipped = count(&job.results, "skipped");
    if skipped > 0 {
        body.push_str(&format!(
            "\n{skipped} titre(s) écarté(s) à la revérification (détail sur la page Liste rouge).\n"
        ));
    }
    Some((subject, body, warn))
}

// ---------------------------------------------------------------------------------------------
// Sauvegarde des fiches
// ---------------------------------------------------------------------------------------------

/// Dossier des sauvegardes d'un travail : `<backups>/liste-rouge-<AAAAMMJJ>` (date locale du début).
pub fn backup_dir(backups: &Path, started: i64) -> PathBuf {
    let day = chrono::DateTime::from_timestamp(started, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y%m%d").to_string())
        .unwrap_or_else(|| "inconnu".into());
    backups.join(format!("liste-rouge-{day}"))
}

/// Écrit une fiche (JSON) en 600 dans un dossier en 700 : `<côté>-<movie|series>-<id>.json`.
pub fn save_card(
    dir: &Path,
    side: &str,
    movie: bool,
    arr_id: i64,
    fiche: &Value,
) -> Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("création de {}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let kind = if movie { "movie" } else { "series" };
    let path = dir.join(format!("{side}-{kind}-{arr_id}.json"));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("écriture de {}", path.display()))?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(serde_json::to_string_pretty(fiche)?.as_bytes())?;
    f.sync_all()?;
    Ok(path)
}

// ---------------------------------------------------------------------------------------------
// Suppression d'une fiche, corbeille
// ---------------------------------------------------------------------------------------------

/// Ce que la suppression d'une fiche a donné.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deletion {
    /// L'Arr a répondu.
    Deleted,
    /// Erreur ou délai dépassé, mais la fiche relue n'existe plus (404).
    Gone,
}

/// `DELETE` d'une fiche avec ses fichiers, jamais rejoué ; en cas d'erreur ou de délai dépassé, la fiche est relue : un
/// 404 vaut réussite, une fiche toujours là (ou illisible) arrête ce côté (`Err`, motif en clair).
pub async fn delete_card(
    arr: &ArrClient,
    path: &str,
    query: &[(&str, &str)],
    timeout: Duration,
) -> std::result::Result<Deletion, String> {
    let Err(e) = arr.delete_timeout(path, query, timeout).await else {
        return Ok(Deletion::Deleted);
    };
    match arr.get_opt(path).await {
        Ok(None) => Ok(Deletion::Gone),
        Ok(Some(_)) => Err(format!("fiche toujours là après l'erreur ({e:#})")),
        Err(e2) => Err(format!(
            "fiche dans un état inconnu ({e:#} ; relecture : {e2:#})"
        )),
    }
}

/// Requête de suppression d'une fiche : (Arr, chemin, paramètres).
fn delete_request<'a>(
    side: &Side<'a>,
    movie: bool,
    id: i64,
    exclusion: bool,
) -> (&'a ArrClient, String, [(&'static str, &'static str); 2]) {
    let excl = if exclusion { "true" } else { "false" };
    if movie {
        (
            side.radarr,
            format!("api/v3/movie/{id}"),
            [("deleteFiles", "true"), ("addImportExclusion", excl)],
        )
    } else {
        (
            side.sonarr,
            format!("api/v3/series/{id}"),
            [("deleteFiles", "true"), ("addImportListExclusion", excl)],
        )
    }
}

/// Ce qu'est devenu le dossier d'un titre dans la corbeille.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecycleOutcome {
    /// Retiré de la corbeille.
    Removed,
    /// Rien à ce nom dans la corbeille (l'Arr a supprimé pour de bon).
    Absent,
    /// Le dossier d'origine est encore là : l'Arr n'a pas fini de le déplacer.
    Waiting,
    /// Refusé par une garde (jamais rejoué).
    Refused(String),
}

/// Dernière ligne de la sortie de [`RECYCLE_PY`].
pub fn parse_recycle(out: &str) -> Option<RecycleOutcome> {
    let last = out.lines().last()?.trim();
    match last {
        "removed" => Some(RecycleOutcome::Removed),
        "absent" => Some(RecycleOutcome::Absent),
        "waiting" => Some(RecycleOutcome::Waiting),
        _ => last
            .strip_prefix("refused:")
            .map(|why| RecycleOutcome::Refused(why.to_string())),
    }
}

/// Retire `target`, dossier réel enfant direct de la corbeille `bin` ; jamais un lien symbolique.
pub fn remove_recycled(target: &Path, bin: &Path) -> Result<RecycleOutcome> {
    let meta = match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RecycleOutcome::Absent),
        Err(e) => return Err(e).context("lecture du dossier de la corbeille"),
        Ok(m) => m,
    };
    if !meta.is_dir() {
        return Ok(RecycleOutcome::Refused(
            "pas un dossier (ou lien symbolique)".into(),
        ));
    }
    let real = std::fs::canonicalize(target)?;
    let real_bin = std::fs::canonicalize(bin)?;
    if real.parent() != Some(real_bin.as_path()) {
        return Ok(RecycleOutcome::Refused("hors de la corbeille".into()));
    }
    std::fs::remove_dir_all(&real).with_context(|| format!("suppression de {}", real.display()))?;
    Ok(RecycleOutcome::Removed)
}

/// VPS : attend (`wait` au plus) que le dossier d'origine ait disparu, puis vide sa place dans la corbeille.
async fn vps_empty(
    origin: Option<PathBuf>,
    target: PathBuf,
    bin: PathBuf,
    wait: Duration,
) -> Result<RecycleOutcome> {
    let started = std::time::Instant::now();
    if let Some(o) = &origin {
        while std::fs::symlink_metadata(o).is_ok() {
            if started.elapsed() >= wait {
                return Ok(RecycleOutcome::Waiting);
            }
            tokio::time::sleep(RECYCLE_POLL).await;
        }
    }
    let job = tokio::task::spawn_blocking(move || remove_recycled(&target, &bin));
    tokio::time::timeout(REMOVE_TIMEOUT, job)
        .await
        .context("vidage de la corbeille trop long")?
        .context("vidage de la corbeille interrompu")?
}

/// Seedbox : même chose par ssh ([`RECYCLE_PY`]), chemins par l'entrée standard.
async fn seedbox_empty(
    ctx: &TaskContext,
    origin: &str,
    target: &str,
    bin: &str,
    wait: Duration,
) -> Result<RecycleOutcome> {
    let input = json!({
        "base": ctx.cfg.seedbox.media_root,
        "recycle": bin,
        "target": target,
        "origin": origin,
        "wait": wait.as_secs(),
    });
    let out = ssh_python(ctx, RECYCLE_PY, &input, wait + REMOVE_TIMEOUT).await?;
    parse_recycle(&out).with_context(|| format!("réponse inattendue du script : {out}"))
}

// ---------------------------------------------------------------------------------------------
// Le travail
// ---------------------------------------------------------------------------------------------

/// Une copie revérifiée, prête à être supprimée.
struct Copy<'s> {
    side: &'s Side<'s>,
    red: RedSide,
    title: dc::Title,
    fiche: Value,
    /// Dossier sur l'hôte (VPS) ou dans le montage (seedbox), et tel que Jellyfin le voit.
    host: PathBuf,
    jf: String,
}

/// Relevés d'une copie avant sa suppression.
struct Measured {
    torrents: Vec<Torrent>,
    hashes: BTreeSet<String>,
    /// `None` : relevé impossible (les octets de l'Arr sont alors tenus pour liés).
    sizes: Option<Sizes>,
}

/// Lu une fois par travail.
#[derive(Default)]
struct JobCache {
    /// Corbeille de chaque Arr (par nom).
    bins: HashMap<&'static str, Option<String>>,
    /// Éléments Jellyfin du canari et leur série (identifiants compacts) ; `None` si illisibles.
    canary: Option<HashSet<String>>,
    /// Un titre supprimé portait (ou pouvait porter) un élément du canari.
    canary_hit: bool,
}

/// Un « Nettoyer » : `keys` = clés confirmées sur la page (lues dans l'état, `max_titles_per_clean` au plus), `snap` =
/// leur empreinte ([`snap`]), `dry` = simulation demandée (voir [`effective_dry`]). Un seul travail à la fois : un
/// second est noté « refusé ». Le travail est enregistré dans `state.red_list.jobs` dès le début (« en cours »), puis
/// après chaque titre : la page suit sa progression là. Rend le travail tel qu'enregistré à la fin.
pub async fn clean(
    ctx: &TaskContext,
    job_id: &str,
    keys: &[String],
    snap: &str,
    dry: bool,
) -> Result<CleanJob> {
    let cfg = &ctx.cfg.tasks.red_list;
    let mut wanted: Vec<String> = Vec::new();
    for k in keys {
        if parse_key(k).is_some() && !wanted.contains(k) {
            wanted.push(k.clone());
        }
    }
    wanted.truncate(cfg.max_titles_per_clean);
    let dry = effective_dry(dry, cfg.clean_dry_run, ctx.dry_run);
    let mut job = CleanJob {
        id: job_id.to_string(),
        started: now(),
        dry_run: dry,
        snap: snap.to_string(),
        status: "running".into(),
        keys: wanted,
        ..Default::default()
    };
    let Some(_running) = reserve(job_id) else {
        job.status = "refused".into();
        job.detail = "un autre « Nettoyer » ou un recalcul est en cours".into();
        job.ended = Some(now());
        record_job(ctx, &job).await?;
        return Ok(job);
    };
    record_job(ctx, &job).await?;
    info!(
        task = TASK,
        job = job_id,
        titles = job.keys.len(),
        dry_run = dry,
        "clean started"
    );
    if let Err(e) = run_job(ctx, &mut job, dry).await {
        warn!(
            task = TASK,
            job = job_id,
            error = format!("{e:#}"),
            "clean failed"
        );
        job.status = "failed".into();
        job.detail = format!("{e:#}");
    }
    job.ended = Some(now());
    record_job(ctx, &job).await?;
    info!(task = TASK, job = job_id, status = %job.status, summary = %job.detail, "clean finished");
    Ok(job)
}

async fn record_job(ctx: &TaskContext, job: &CleanJob) -> Result<()> {
    let kept = ctx.cfg.tasks.red_list.jobs_kept;
    ctx.state
        .update(|s| upsert_job(&mut s.red_list.jobs, job, kept))
        .await
}

async fn run_job(ctx: &TaskContext, job: &mut CleanJob, dry: bool) -> Result<()> {
    let cfg = &ctx.cfg.tasks.red_list;
    job.quota_before = quota::read(ctx).await.map(|q| QuotaReading {
        used_kb: q.used_kb,
        quota_kb: q.quota_kb,
        at: q.at,
    });
    // la liste montrée à la confirmation : une copie apparue ou changée depuis n'a pas été vue par l'admin
    let shown = ctx.state.read(|s| s.red_list.report.clone()).await;
    // nouveau calcul complet : on ne supprime que ce qui est rouge maintenant
    let Some(computed) = red_list::compute(ctx).await? else {
        job.status = "refused".into();
        job.detail = "un côté Arr est injoignable : rien n'a été supprimé".into();
        return Ok(());
    };
    let report = if ctx.dry_run {
        computed.report
    } else {
        red_list::store(ctx, computed).await?
    };
    if !report.complete {
        job.status = "refused".into();
        job.detail = format!(
            "calcul incomplet ({}) : rien n'a été supprimé",
            report.missing.join(", ")
        );
        return Ok(());
    }
    let exemptions = ctx.state.read(|s| s.red_list.exemptions.clone()).await;
    let (picked, mut skipped) = select(&job.keys, &report, &exemptions, cfg.max_titles_per_clean);
    let picked: Vec<&RedEntry> = picked
        .into_iter()
        .filter(|e| {
            let same = same_copies(shown.as_ref(), e);
            if !same {
                skipped.push((
                    e.key.clone(),
                    "copies changées depuis la confirmation : à reconfirmer",
                ));
            }
            same
        })
        .collect();
    let at = now();
    for (key, why) in skipped {
        let title = report
            .entries
            .iter()
            .chain(&report.soon)
            .chain(&report.exempted)
            .find(|e| e.key == key)
            .map(|e| e.title.clone())
            .unwrap_or_default();
        info!(task = TASK, key = %key, reason = why, "title skipped");
        job.results.push(CleanResult {
            key,
            title,
            at,
            outcome: "skipped".into(),
            detail: why.to_string(),
            ..Default::default()
        });
    }
    record_job(ctx, job).await?;
    let sides = ctx.sides();
    let backups = backup_dir(&ctx.cfg.paths.backups, job.started);
    let mut cache = JobCache {
        canary: canary_ids(ctx).await,
        ..Default::default()
    };
    let mut jf_deleted: Vec<String> = Vec::new();
    let mut deleted_keys: BTreeSet<String> = BTreeSet::new();
    for e in picked {
        let (res, jf) = clean_entry(ctx, &sides, &mut cache, &backups, e, dry).await;
        if res.outcome == "deleted" {
            deleted_keys.insert(res.key.clone());
        }
        jf_deleted.extend(jf);
        job.results.push(res);
        record_job(ctx, job).await?;
    }
    // fin : une seule analyse ciblée pour tous les dossiers
    if !jf_deleted.is_empty() {
        if dry {
            info!(task = TASK, folders = ?jf_deleted, "dry-run: would tell Jellyfin these folders are deleted");
        } else if let Err(e) = ctx.jellyfin.media_updated_as(&jf_deleted, "Deleted").await {
            warn!(
                task = TASK,
                error = format!("{e:#}"),
                "Jellyfin not told (its next library scan will catch up)"
            );
        }
    }
    if !dry && (cache.canary_hit || !deleted_keys.is_empty()) {
        let hit = cache.canary_hit;
        ctx.state
            .update(|s| {
                if hit {
                    s.canary.items.clear(); // le canari choisira un autre petit fichier
                }
                if let Some(r) = s.red_list.report.as_mut() {
                    r.entries.retain(|e| !deleted_keys.contains(&e.key));
                }
            })
            .await?;
    }
    job.status = "done".into();
    job.detail = job_summary(job);
    if let Some((subject, body, warn)) = alert_text(job) {
        let level = if warn { Level::Warn } else { Level::Info };
        alerts::admin(ctx, level, &subject, &body).await;
    }
    Ok(())
}

/// Éléments Jellyfin que lit le canari, avec leur série.
async fn canary_ids(ctx: &TaskContext) -> Option<HashSet<String>> {
    let items: Vec<String> = ctx
        .state
        .read(|s| s.canary.items.values().cloned().collect())
        .await;
    if items.is_empty() {
        return Some(HashSet::new());
    }
    let users = ctx.jellyfin.users().await.ok()?;
    let admin = pick_admin(&users)?;
    let found = ctx.jellyfin.items_by_ids_as(&admin, &items).await.ok()?;
    let mut ids: HashSet<String> = items.iter().map(|i| norm(i)).collect();
    ids.extend(
        found
            .iter()
            .filter_map(|it| it.get("SeriesId").and_then(Value::as_str))
            .map(norm),
    );
    Some(ids)
}

/// Revérifications d'un titre juste avant d'y toucher. `Err(motif)` : le titre est écarté. Une copie dont la fiche
/// n'existe plus (404) est déjà partie : elle n'est pas rendue.
async fn recheck<'s>(
    ctx: &TaskContext,
    sides: &'s [Side<'s>],
    e: &RedEntry,
) -> std::result::Result<Vec<Copy<'s>>, String> {
    let at = now();
    let (exempt, moves) = ctx
        .state
        .read(|s| {
            (
                s.red_list.exemptions.contains_key(&e.key),
                s.anime_moves.clone(),
            )
        })
        .await;
    if exempt {
        return Err("exempté entre-temps".into());
    }
    let playing = ctx
        .jellyfin
        .playing_paths()
        .await
        .map_err(|err| format!("sessions Jellyfin illisibles ({err:#})"))?;
    let mut out = Vec::new();
    for rs in &e.sides {
        let side = sides
            .iter()
            .find(|s| s.name == rs.side)
            .ok_or_else(|| format!("côté {} non configuré", rs.side))?;
        if recently_moved(&moves, &rs.side, e.movie, rs.arr_id, at) {
            return Err(format!(
                "{} : rangé par anime_library il y a moins de 6 h",
                rs.side
            ));
        }
        let (host, jf) = map_path(&side_maps(ctx, side.name), &rs.folder)
            .ok_or_else(|| format!("{} : dossier hors des racines connues", rs.side))?;
        if is_playing(&playing, &jf) {
            return Err("en cours de lecture".into());
        }
        match dc::arr_title_by_id(ctx, side, e.movie, rs.arr_id).await {
            Err(err) => return Err(format!("{} : fiche illisible ({err:#})", rs.side)),
            Ok(None) => {
                info!(task = TASK, side = side.name, title = %e.title, "card already gone on this side")
            }
            Ok(Some((title, fiche))) => {
                if !card_matches(e, rs, &fiche) {
                    return Err(format!(
                        "{} : fiche changée (identifiant ou dossier)",
                        rs.side
                    ));
                }
                out.push(Copy {
                    side,
                    red: rs.clone(),
                    title,
                    fiche,
                    host,
                    jf,
                });
            }
        }
    }
    Ok(out)
}

/// Tailles d'une copie avant suppression : `nlink` sur le VPS, script fixe par ssh sur la seedbox.
async fn measure(ctx: &TaskContext, c: &Copy<'_>) -> Result<Sizes> {
    if c.side.name == "vps" {
        let dir = c.host.clone();
        let job = tokio::task::spawn_blocking(move || tally_dir(&dir));
        return tokio::time::timeout(MEASURE_TIMEOUT, job)
            .await
            .context("relevé des tailles trop long")?
            .context("relevé des tailles interrompu");
    }
    if !strictly_under(&c.red.folder, &ctx.cfg.seedbox.media_root) {
        bail!("dossier hors de la racine des médias de la seedbox");
    }
    let out = ssh_python(
        ctx,
        MEASURE_PY,
        &json!({ "dir": c.red.folder }),
        MEASURE_TIMEOUT,
    )
    .await?;
    serde_json::from_str(out.lines().last().unwrap_or_default())
        .with_context(|| format!("relevé illisible : {out}"))
}

/// Le vidage de la corbeille pour une copie supprimée.
struct RecycleStep {
    /// Plus rien du titre dans la corbeille (vidée, ou l'Arr supprime pour de bon).
    done: bool,
    note: Option<String>,
    pending: Option<PendingStep>,
}

impl RecycleStep {
    fn done() -> Self {
        Self {
            done: true,
            note: None,
            pending: None,
        }
    }

    fn later(note: String, pending: Option<PendingStep>) -> Self {
        Self {
            done: false,
            note: Some(note),
            pending,
        }
    }
}

fn pending_step(step: &str, e: &RedEntry, side: &str, at: i64, error: String) -> PendingStep {
    PendingStep {
        step: step.into(),
        key: e.key.clone(),
        title: e.title.clone(),
        side: side.into(),
        media_type: if e.movie { "movie" } else { "tv" }.into(),
        tmdb: e.tmdb,
        first_at: at,
        last_try: at,
        tries: 1,
        last_error: error,
        ..Default::default()
    }
}

async fn empty_recycle(
    ctx: &TaskContext,
    cache: &mut JobCache,
    c: &Copy<'_>,
    e: &RedEntry,
    dry: bool,
) -> RecycleStep {
    let cfg = &ctx.cfg.tasks.red_list;
    let arr = if e.movie {
        c.side.radarr
    } else {
        c.side.sonarr
    };
    let bin = match cache.bins.get(arr.name) {
        Some(b) => b.clone(),
        None => match arr.recycle_bin().await {
            Ok(b) => {
                cache.bins.insert(arr.name, b.clone());
                b
            }
            Err(err) => {
                warn!(
                    task = TASK,
                    arr = arr.name,
                    error = format!("{err:#}"),
                    "recycle bin setting unreadable"
                );
                return RecycleStep::later(format!("corbeille de {} illisible", arr.name), None);
            }
        },
    };
    let Some(bin) = bin else {
        return RecycleStep::done(); // aucune corbeille : l'Arr supprime pour de bon
    };
    if !cfg.empty_recycle {
        return RecycleStep::later("corbeille laissée à l'Arr (14 j)".into(), None);
    }
    let wait = Duration::from_secs(cfg.recycle_wait_secs);
    let (outcome, target, origin, used_bin) = if c.side.name == "vps" {
        let maps = side_maps(ctx, "vps");
        let (dir, target) = match vps_recycle_target(
            &bin,
            &c.red.folder,
            &maps,
            &ctx.cfg.cleanup.recycle_dirs,
        ) {
            Ok(t) => t,
            Err(why) => {
                warn!(task = TASK, side = "vps", title = %e.title, reason = %why, "recycle target refused");
                return RecycleStep::later(format!("corbeille : {why}"), None);
            }
        };
        if dry {
            info!(task = TASK, side = "vps", title = %e.title, target = %target.display(), "dry-run: would empty the arr recycle bin folder");
            return RecycleStep::done();
        }
        let out = vps_empty(Some(c.host.clone()), target.clone(), dir, wait).await;
        (
            out,
            target.to_string_lossy().to_string(),
            c.host.to_string_lossy().to_string(),
            String::new(),
        )
    } else {
        let (bin, target) = match seedbox_recycle_target(
            &bin,
            &c.red.folder,
            &ctx.cfg.seedbox.media_root,
        ) {
            Ok(t) => t,
            Err(why) => {
                warn!(task = TASK, side = "seedbox", title = %e.title, reason = %why, "recycle target refused");
                return RecycleStep::later(format!("corbeille : {why}"), None);
            }
        };
        if dry {
            info!(task = TASK, side = "seedbox", title = %e.title, target = %target, "dry-run: would empty the arr recycle bin folder");
            return RecycleStep::done();
        }
        let out = seedbox_empty(ctx, &c.red.folder, &target, &bin, wait).await;
        (out, target, c.red.folder.clone(), bin)
    };
    let at = now();
    let to_replay = |why: String| {
        let mut p = pending_step("recycle", e, c.side.name, at, why.clone());
        p.target = target.clone();
        p.origin = origin.clone();
        p.bin = used_bin.clone();
        RecycleStep::later(format!("corbeille à vider plus tard ({why})"), Some(p))
    };
    match outcome {
        Ok(RecycleOutcome::Removed) | Ok(RecycleOutcome::Absent) => {
            info!(task = TASK, side = c.side.name, title = %e.title, "arr recycle bin folder emptied");
            RecycleStep::done()
        }
        Ok(RecycleOutcome::Waiting) => {
            to_replay("l'Arr n'a pas fini d'y déplacer le dossier".into())
        }
        Ok(RecycleOutcome::Refused(why)) => {
            warn!(task = TASK, side = c.side.name, title = %e.title, reason = %why, "recycle target refused");
            RecycleStep::later(format!("corbeille : cible refusée ({why})"), None)
        }
        Err(err) => {
            warn!(task = TASK, side = c.side.name, title = %e.title, error = format!("{err:#}"), "recycle bin not emptied");
            to_replay(format!("{err:#}"))
        }
    }
}

/// Un titre, de la revérification au résultat. Rend le résultat et les dossiers Jellyfin supprimés.
async fn clean_entry(
    ctx: &TaskContext,
    sides: &[Side<'_>],
    cache: &mut JobCache,
    backups: &Path,
    e: &RedEntry,
    dry: bool,
) -> (CleanResult, Vec<String>) {
    let cfg = &ctx.cfg.tasks.red_list;
    let mut res = CleanResult {
        key: e.key.clone(),
        title: e.title.clone(),
        at: now(),
        ..Default::default()
    };
    let skip = |mut res: CleanResult, why: String| {
        info!(task = TASK, title = %e.title, reason = %why, "title skipped");
        res.outcome = "skipped".into();
        res.detail = why;
        (res, Vec::new())
    };
    // 1. revérifications
    let copies = match recheck(ctx, sides, e).await {
        Ok(c) if c.is_empty() => return skip(res, "plus aucune fiche".into()),
        Ok(c) => c,
        Err(why) => return skip(res, why),
    };
    // 2. sauvegarde des fiches
    for c in &copies {
        if dry {
            info!(task = TASK, side = c.side.name, title = %e.title, dir = %backups.display(), "dry-run: would save the arr card");
            continue;
        }
        if let Err(err) = save_card(backups, c.side.name, e.movie, c.red.arr_id, &c.fiche) {
            warn!(task = TASK, title = %e.title, error = format!("{err:#}"), "card backup failed: nothing deleted");
            res.outcome = "error".into();
            res.detail = format!("sauvegarde de la fiche impossible ({err:#})");
            return (res, Vec::new());
        }
    }
    // 3. relevés : torrents d'origine (l'historique part avec la fiche), tailles
    let mut measured = Vec::new();
    for c in &copies {
        let torrents = match c.side.qbit.torrents().await {
            Ok(t) => t,
            Err(err) => {
                return skip(
                    res,
                    format!("{} : qBittorrent injoignable ({err:#})", c.side.name),
                )
            }
        };
        let hashes = dc::source_hashes(c.side, &c.title, None, &torrents, TASK).await;
        let sizes = match measure(ctx, c).await {
            Ok(s) => Some(s),
            Err(err) => {
                warn!(task = TASK, side = c.side.name, title = %e.title, error = format!("{err:#}"), "sizes unreadable");
                None
            }
        };
        info!(
            task = TASK,
            side = c.side.name,
            title = %e.title,
            hashes = ?hashes,
            exclusive = sizes.map(|s| s.exclusive),
            linked = sizes.map(|s| s.linked),
            "measured before deletion"
        );
        measured.push(Measured {
            torrents,
            hashes,
            sizes,
        });
    }
    let mut notes: Vec<String> = Vec::new();
    let mut pending: Vec<PendingStep> = Vec::new();
    let (mut deleted, mut failed) = (0usize, 0usize);
    let mut jf_deleted = Vec::new();
    let mut origins: Vec<PathBuf> = Vec::new();
    for (c, m) in copies.iter().zip(&measured) {
        // 4. la fiche et ses fichiers
        let (arr, path, query) =
            delete_request(c.side, e.movie, c.red.arr_id, cfg.add_import_exclusion);
        if dry {
            info!(task = TASK, side = c.side.name, title = %e.title, card = %path, "dry-run: would delete the arr card with its files");
        } else {
            let timeout = Duration::from_secs(cfg.arr_delete_timeout_secs);
            match delete_card(arr, &path, &query, timeout).await {
                Ok(d) => {
                    info!(task = TASK, side = c.side.name, title = %e.title, after_error = d == Deletion::Gone, "arr card deleted with its files")
                }
                Err(why) => {
                    warn!(task = TASK, side = c.side.name, title = %e.title, reason = %why, "arr card not deleted: side stopped");
                    notes.push(format!("{} : {why}", c.side.name));
                    failed += 1;
                    continue;
                }
            }
        }
        deleted += 1;
        // 5. corbeille de l'Arr (avant les torrents : un fichier dans la corbeille garde son lien physique)
        let rec = empty_recycle(ctx, cache, c, e, dry).await;
        if let Some(n) = rec.note {
            notes.push(format!("{} : {n}", c.side.name));
        }
        let mut rec_pending = rec.pending;
        // 6. torrents d'origine
        let mut tnotes: Vec<&'static str> = Vec::new();
        let mut kept: Vec<String> = Vec::new();
        for h in &m.hashes {
            let Some(tor) = m.torrents.iter().find(|x| x.hash.eq_ignore_ascii_case(h)) else {
                continue;
            };
            // simulation : le dossier du titre compte comme supprimé si la vraie suppression le ferait disparaître
            // (corbeille vidée ou absente) ; sinon le relevé jugerait le torrent lié à ce titre même
            let gone = (dry && rec.done).then_some(c.host.as_path());
            if dc::still_used(c.side, &c.title, h, &BTreeSet::new(), gone).await {
                info!(task = TASK, side = c.side.name, torrent = %tor.name, "torrent still used by other library files: kept");
                tnotes.push("torrent_kept");
                kept.push(h.clone());
                continue;
            }
            tnotes.push(dc::handle_torrent(ctx, c.side, tor, dry, TASK).await);
        }
        // VPS, corbeille pas encore vidée : le fichier qui y attend tient le lien des torrents gardés ; la reprise les
        // revoit une fois la corbeille vidée (sinon ils resteraient pour toujours, la fiche n'existant plus)
        if let Some(mut p) = rec_pending.take() {
            if c.side.name == "vps" {
                p.hashes = kept.clone();
                p.arr_id = c.red.arr_id;
            }
            res.pending.push(format!("recycle:{}", c.side.name));
            pending.push(p);
        }
        if tnotes.contains(&"torrent_pending") {
            res.pending.push(format!("torrent:{}", c.side.name));
        }
        if tnotes.contains(&"torrent_error") {
            notes.push(format!("{} : torrent non retiré", c.side.name));
        }
        // 7. montage seedbox
        if c.side.name == "seedbox" {
            if dry {
                info!(task = TASK, title = %e.title, dir = %c.host.display(), "dry-run: would forget the folder in the rclone cache");
            } else {
                dc::rclone_forget(ctx, &c.host).await;
                dc::rclone_refresh(ctx, &c.host).await;
            }
        }
        let sizes = m.sizes.unwrap_or(Sizes {
            linked: c.red.bytes,
            ..Default::default()
        });
        let (a, b, s) = account(sizes, rec.done, &tnotes);
        res.freed_now += a;
        res.freed_later += b;
        res.shared += s;
        jf_deleted.push(c.jf.clone());
        origins.push(c.host.clone());
        // 9. canari
        if cache
            .canary
            .as_ref()
            .is_none_or(|ids| ids.contains(&norm(&c.red.jf_id)))
        {
            cache.canary_hit = true;
        }
    }
    // 8. Seerr : seulement si le titre n'a plus de fichiers nulle part
    if failed == 0 && deleted > 0 {
        let at = now();
        let left = if dry {
            Vec::new()
        } else {
            folders_left(&origins)
        };
        let why = if left.is_empty() {
            release_seerr(ctx, sides, e, &copies[0].title, dry, &mut notes).await
        } else {
            // l'Arr n'a pas (encore) retiré le dossier : retirer le média laisserait un orphelin redemandable
            warn!(task = TASK, title = %e.title, folders = ?left, "title folder still there: seerr media kept for now");
            notes.push("dossier encore présent : Jellyseerr libéré plus tard".into());
            Some("dossier du titre encore présent".into())
        };
        if let Some(p) = why {
            let mut p = pending_step("seerr", e, "", at, p);
            p.origins = origins
                .iter()
                .map(|o| o.to_string_lossy().to_string())
                .collect();
            res.pending.push("seerr".into());
            pending.push(p);
        }
    }
    if !pending.is_empty() && !dry {
        if let Err(err) = ctx
            .state
            .update(|s| {
                for p in pending {
                    add_pending(&mut s.red_list.pending, p);
                }
            })
            .await
        {
            warn!(
                task = TASK,
                error = format!("{err:#}"),
                "pending steps not recorded"
            );
        }
    }
    res.outcome = if dry {
        "simulated"
    } else if failed == 0 {
        "deleted"
    } else if deleted > 0 {
        "partial"
    } else {
        "error"
    }
    .into();
    res.detail = notes.join(" ; ");
    res.at = now();
    (res, jf_deleted)
}

/// Retire le média Seerr d'un titre qui n'a plus de fichiers nulle part. Rend le motif d'une étape à rejouer.
async fn release_seerr(
    ctx: &TaskContext,
    sides: &[Side<'_>],
    e: &RedEntry,
    t: &dc::Title,
    dry: bool,
    notes: &mut Vec<String>,
) -> Option<String> {
    if e.tmdb <= 0 {
        notes.push("Seerr : sans identifiant TMDB".into());
        return None;
    }
    // un côté sans copie du titre peut encore avoir ses fichiers (fiche non rouge) : Seerr reste alors
    let names: Vec<&str> = e.sides.iter().map(|s| s.side.as_str()).collect();
    let others = sides.iter().filter(|s| !names.contains(&s.name)).count();
    let elsewhere = match (others, names.as_slice()) {
        (0, _) => false,
        (_, [one]) => dc::present_elsewhere(ctx, one, t).await,
        _ => true,
    };
    if elsewhere {
        notes.push("Seerr : titre présent ailleurs, média gardé".into());
        return None;
    }
    let kind = if e.movie { "movie" } else { "tv" };
    match ctx.jellyseerr.media_id(kind, e.tmdb).await {
        Ok(None) => None,
        Ok(Some(mid)) if dry => {
            info!(task = TASK, title = %e.title, media = mid, "dry-run: would release the seerr media");
            None
        }
        Ok(Some(mid)) => match ctx.jellyseerr.delete_media(mid).await {
            Ok(()) => {
                info!(task = TASK, title = %e.title, "seerr media released");
                None
            }
            Err(err) => {
                warn!(task = TASK, title = %e.title, error = format!("{err:#}"), "seerr media not released: will retry");
                Some(format!("{err:#}"))
            }
        },
        Err(err) if dry => {
            warn!(task = TASK, title = %e.title, error = format!("{err:#}"), "seerr lookup failed");
            None
        }
        Err(err) => {
            warn!(task = TASK, title = %e.title, error = format!("{err:#}"), "seerr lookup failed: will retry");
            Some(format!("{err:#}"))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reprise (appelée par la tâche `red_list`)
// ---------------------------------------------------------------------------------------------

/// Marque « interrompu » tout travail resté « en cours » qui n'est pas celui de ce processus (homelabd redémarré en
/// plein travail). Appelé à chaque passage de la tâche `red_list`, dont le premier suit de peu le démarrage. Rend le
/// nombre de travaux marqués ; rien n'est écrit en `--dry-run`.
pub async fn mark_interrupted(ctx: &TaskContext) -> Result<usize> {
    let stale = ctx
        .state
        .read(|s| s.red_list.jobs.iter().any(|j| j.status == "running"))
        .await;
    if !stale || ctx.dry_run {
        return Ok(0);
    }
    // le travail courant est relu sous le verrou de l'état : un travail qui démarre s'inscrit après s'être réservé
    ctx.state
        .update(|s| {
            let current = current().lock().unwrap_or_else(|e| e.into_inner()).clone();
            interrupt_stale(&mut s.red_list.jobs, current.as_deref(), now())
        })
        .await
}

/// Le titre a-t-il de nouveau une fiche dans un Arr (redemandé depuis) ? Clé du titre : `tvdb` ou `tmdb`.
async fn arr_knows(ctx: &TaskContext, key: &str) -> Result<bool> {
    let Some((movie, provider, id)) = parse_key(key) else {
        return Ok(false);
    };
    let field = if provider == "tvdb" {
        "tvdbId"
    } else {
        "tmdbId"
    };
    for side in ctx.sides() {
        let list = if movie {
            side.radarr.movies().await?
        } else {
            side.sonarr.series().await?
        };
        if list
            .iter()
            .any(|v| v.get(field).and_then(Value::as_i64) == Some(id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Les copies d'un titre (côté, fiche, dossier) sont-elles celles de la liste montrée à la confirmation ?
pub fn same_copies(shown: Option<&RedListReport>, e: &RedEntry) -> bool {
    let copies = |x: &RedEntry| {
        let mut v: Vec<(String, i64, String)> = x
            .sides
            .iter()
            .map(|c| (c.side.clone(), c.arr_id, c.folder.clone()))
            .collect();
        v.sort();
        v
    };
    shown
        .and_then(|r| r.entries.iter().find(|x| x.key == e.key))
        .is_some_and(|old| copies(old) == copies(e))
}

/// Dossiers d'un titre qui ne sont pas (sûrement) partis : encore là, ou indécidable (parent absent : montage tombé).
pub fn folders_left(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| {
            let parent_ok = p.parent().is_some_and(|d| d.is_dir());
            !parent_ok || std::fs::symlink_metadata(p).is_ok()
        })
        .cloned()
        .collect()
}

/// Corbeille seedbox à la reprise : notée à la suppression, identique à celle que l'Arr déclare maintenant, et parent de
/// la cible.
pub fn same_bin(noted: &str, now: Option<&str>, target_parent: &str) -> bool {
    let norm = |x: &str| x.trim_end_matches('/').to_string();
    !noted.is_empty()
        && now.is_some_and(|n| norm(n) == norm(noted))
        && norm(target_parent) == norm(noted)
}

/// Après une corbeille du VPS vidée : les torrents qu'elle retenait sont revus (encore utilisés ailleurs : gardés ;
/// sinon retirés, ou mis en attente pour C411). Rend une note pour le journal.
async fn release_kept_torrents(ctx: &TaskContext, p: &PendingStep) -> String {
    if p.side != "vps" || p.hashes.is_empty() {
        return String::new();
    }
    let Some((movie, provider, id)) = parse_key(&p.key) else {
        return String::new();
    };
    let sides = ctx.sides();
    let Some(side) = sides.iter().find(|x| x.name == "vps") else {
        return String::new();
    };
    let torrents = match side.qbit.torrents().await {
        Ok(t) => t,
        Err(e) => {
            warn!(task = TASK, title = %p.title, error = format!("{e:#}"), "qbittorrent unreachable: kept torrents not reviewed");
            return " ; torrents non revus (qBittorrent injoignable)".into();
        }
    };
    let title = dc::Title {
        kind: if movie {
            dc::Kind::Movie
        } else {
            dc::Kind::Series {
                tvdb: if provider == "tvdb" { id } else { 0 },
                total_files: 0,
            }
        },
        id: p.arr_id,
        tmdb: p.tmdb,
        name: p.title.clone(),
        files: Vec::new(),
    };
    let mut out = Vec::new();
    for h in &p.hashes {
        let Some(tor) = torrents.iter().find(|x| x.hash.eq_ignore_ascii_case(h)) else {
            continue;
        };
        if dc::still_used(side, &title, h, &BTreeSet::new(), None).await {
            info!(task = TASK, torrent = %tor.name, "torrent still used by other library files: kept");
            out.push("torrent_kept");
            continue;
        }
        out.push(dc::handle_torrent(ctx, side, tor, false, TASK).await);
    }
    if out.is_empty() {
        String::new()
    } else {
        format!(" ; torrents : {}", out.join(", "))
    }
}

/// Rejoue une étape. `Ok(note)` : terminée (ou sans objet) ; `Err(motif)` : à retenter.
async fn replay_one(ctx: &TaskContext, p: &PendingStep) -> std::result::Result<String, String> {
    match p.step.as_str() {
        "recycle" => {
            let outcome = if p.side == "vps" {
                let Some(bin) = vps_replay_bin(&p.target, &ctx.cfg.cleanup.recycle_dirs) else {
                    return Ok("cible refusée, abandonnée".into());
                };
                let origin = (!p.origin.is_empty()).then(|| PathBuf::from(&p.origin));
                vps_empty(origin, PathBuf::from(&p.target), bin, Duration::ZERO).await
            } else {
                let Some(bin) = seedbox_replay_bin(&p.target, &ctx.cfg.seedbox.media_root) else {
                    return Ok("cible refusée, abandonnée".into());
                };
                // même corbeille qu'à la suppression, relue maintenant dans l'Arr : jamais un dossier de bibliothèque
                let movie = parse_key(&p.key).map(|(m, _, _)| m).unwrap_or(true);
                let sides = ctx.sides();
                let Some(side) = sides.iter().find(|x| x.name == "seedbox") else {
                    return Ok("seedbox absente, abandonnée".into());
                };
                let arr = if movie { side.radarr } else { side.sonarr };
                let now_bin = arr
                    .recycle_bin()
                    .await
                    .map_err(|e| format!("corbeille illisible : {e:#}"))?;
                if !same_bin(&p.bin, now_bin.as_deref(), &bin) {
                    return Ok("corbeille changée depuis la suppression, abandonnée".into());
                }
                seedbox_empty(ctx, &p.origin, &p.target, &bin, Duration::ZERO).await
            };
            match outcome {
                Ok(RecycleOutcome::Removed) | Ok(RecycleOutcome::Absent) => {
                    let torrents = release_kept_torrents(ctx, p).await;
                    Ok(format!("corbeille vidée{torrents}"))
                }
                Ok(RecycleOutcome::Refused(why)) => {
                    Ok(format!("cible refusée ({why}), abandonnée"))
                }
                Ok(RecycleOutcome::Waiting) => {
                    Err("l'Arr n'a pas fini d'y déplacer le dossier".into())
                }
                Err(e) => Err(format!("{e:#}")),
            }
        }
        "seerr" => {
            let paths: Vec<PathBuf> = p.origins.iter().map(PathBuf::from).collect();
            if !folders_left(&paths).is_empty() {
                return Err("dossier du titre encore présent".into());
            }
            // redemandé entre-temps : la fiche est revenue, retirer le média effacerait la nouvelle demande
            match arr_knows(ctx, &p.key).await {
                Err(e) => return Err(format!("{e:#}")),
                Ok(true) => return Ok("titre de nouveau dans un Arr : média gardé".into()),
                Ok(false) => {}
            }
            match ctx.jellyseerr.media_id(&p.media_type, p.tmdb).await {
                Ok(None) => Ok("média déjà retiré".into()),
                Ok(Some(mid)) => ctx
                    .jellyseerr
                    .delete_media(mid)
                    .await
                    .map(|()| "média Seerr retiré".to_string())
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(format!("{e:#}")),
            }
        }
        other => Ok(format!("étape inconnue ({other}), abandonnée")),
    }
}

/// Rejoue les étapes en attente : [`PENDING_PER_RUN`] au plus, les moins récemment tentées d'abord ; abandonne celles
/// qui attendent depuis 14 j. Rend un résumé (vide s'il n'y avait rien) ; rien n'est fait en `--dry-run`.
pub async fn replay_pending(ctx: &TaskContext) -> Result<String> {
    let pending = ctx.state.read(|s| s.red_list.pending.clone()).await;
    if pending.is_empty() {
        return Ok(String::new());
    }
    let (due, expired) = replay_plan(&pending, now(), PENDING_PER_RUN, PENDING_MAX_AGE_SECS);
    if ctx.dry_run {
        info!(task = TASK, due = ?due, expired = ?expired, "dry-run: would replay pending steps");
        return Ok(format!("{} étape(s) en attente", pending.len()));
    }
    for k in &expired {
        warn!(task = TASK, step = %k, "pending step abandoned after 14 days");
    }
    if !expired.is_empty() {
        ctx.state
            .update(|s| {
                for k in &expired {
                    s.red_list.pending.remove(k);
                }
            })
            .await?;
    }
    let (mut done, mut again) = (0u32, 0u32);
    for k in due {
        let p = &pending[&k];
        let outcome = replay_one(ctx, p).await;
        let at = now();
        match &outcome {
            Ok(note) => {
                info!(task = TASK, step = %k, title = %p.title, note = %note, "pending step done");
                done += 1;
            }
            Err(why) => {
                warn!(task = TASK, step = %k, title = %p.title, error = %why, "pending step failed: will retry");
                again += 1;
            }
        }
        ctx.state
            .update(|s| match outcome {
                Ok(_) => {
                    s.red_list.pending.remove(&k);
                }
                Err(why) => {
                    if let Some(x) = s.red_list.pending.get_mut(&k) {
                        x.tries += 1;
                        x.last_try = at;
                        x.last_error = why;
                    }
                }
            })
            .await?;
    }
    let left = pending.len() - expired.len() - done as usize;
    Ok(format!(
        "étapes rejouées : {done} faite(s), {again} à retenter, {} abandonnée(s), {left} en attente",
        expired.len()
    ))
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn side(name: &str, id: i64, bytes: u64) -> RedSide {
        RedSide {
            side: name.into(),
            arr_id: id,
            bytes,
            folder: format!("/arr/{name}/T{id}"),
            ..Default::default()
        }
    }

    fn entry(key: &str, sides: Vec<RedSide>) -> RedEntry {
        RedEntry {
            key: key.into(),
            title: format!("Titre {key}"),
            movie: key.starts_with("movie:"),
            bytes: sides.iter().map(|s| s.bytes).sum(),
            sides,
            ..Default::default()
        }
    }

    fn report(entries: Vec<RedEntry>) -> RedListReport {
        RedListReport {
            at: 1_790_000_000,
            complete: true,
            entries,
            ..Default::default()
        }
    }

    /// Revue du 22/10 : une copie apparue, retirée ou changée entre la confirmation et le travail n'a pas été vue par
    /// l'admin ; le titre est écarté (taille changée seule : gardé, les octets ne désignent pas une autre fiche).
    #[test]
    fn copies_changees_depuis_la_confirmation_ecartees() {
        let e = entry("movie:tmdb:1", vec![side("vps", 45, 10)]);
        let shown = report(vec![entry("movie:tmdb:1", vec![side("vps", 45, 99)])]);
        assert!(same_copies(Some(&shown), &e));
        let two = entry(
            "movie:tmdb:1",
            vec![side("vps", 45, 10), side("seedbox", 7, 5)],
        );
        assert!(!same_copies(Some(&shown), &two));
        let other_id = entry("movie:tmdb:1", vec![side("vps", 46, 10)]);
        assert!(!same_copies(Some(&shown), &other_id));
        let mut moved = e.clone();
        moved.sides[0].folder = "/arr/vps/ailleurs".into();
        assert!(!same_copies(Some(&shown), &moved));
        assert!(!same_copies(Some(&report(vec![])), &e));
        assert!(!same_copies(None, &e));
        // l'ordre des copies ne compte pas
        let shown2 = report(vec![entry(
            "movie:tmdb:1",
            vec![side("seedbox", 7, 5), side("vps", 45, 10)],
        )]);
        assert!(same_copies(Some(&shown2), &two));
    }

    /// Revue du 22/10 : Jellyseerr n'est libéré qu'une fois les dossiers du titre partis ; montage tombé (parent
    /// absent) = indécidable, donc attendu.
    #[test]
    fn seerr_attend_que_les_dossiers_soient_partis() {
        let tmp = tempfile::tempdir().unwrap();
        let there = tmp.path().join("Film (2019)");
        std::fs::create_dir(&there).unwrap();
        let gone = tmp.path().join("Parti (2018)");
        let unmounted = tmp.path().join("montage/absent/Film (2020)");
        let left = folders_left(&[there.clone(), gone.clone(), unmounted.clone()]);
        assert_eq!(left, vec![there.clone(), unmounted]);
        std::fs::remove_dir(&there).unwrap();
        assert!(folders_left(&[there, gone]).is_empty());
    }

    /// Revue du 22/10 : la reprise d'une corbeille seedbox exige la corbeille notée, toujours déclarée par l'Arr, et
    /// parente de la cible.
    #[test]
    fn reprise_seedbox_exige_la_meme_corbeille() {
        let b = "/home/u/media/.recycle";
        assert!(same_bin(b, Some("/home/u/media/.recycle/"), b));
        assert!(
            !same_bin("", Some(b), b),
            "étape ancienne sans corbeille notée"
        );
        assert!(!same_bin(b, None, b), "corbeille désactivée depuis");
        assert!(
            !same_bin(b, Some("/home/u/media/movies"), b),
            "corbeille changée"
        );
        assert!(
            !same_bin(b, Some(b), "/home/u/media/movies"),
            "cible hors de la corbeille"
        );
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn exempt(k: &str) -> BTreeMap<String, Exemption> {
        BTreeMap::from([(k.to_string(), Exemption::default())])
    }

    #[test]
    fn the_snap_is_stable_whatever_the_order_and_changes_on_any_gap() {
        let a = entry(
            "movie:tmdb:1",
            vec![side("vps", 10, 5), side("seedbox", 20, 7)],
        );
        let b = entry("series:tvdb:2", vec![side("seedbox", 30, 9)]);
        let r = report(vec![a.clone(), b.clone()]);
        let base = snap(&r, &keys(&["movie:tmdb:1", "series:tvdb:2"]));
        assert_eq!(base.len(), 64, "SHA-256 en hexadécimal");
        // ordre des clés, des entrées et des copies sans effet
        let mut a2 = a.clone();
        a2.sides.reverse();
        let r2 = report(vec![b.clone(), a2]);
        assert_eq!(snap(&r2, &keys(&["series:tvdb:2", "movie:tmdb:1"])), base);
        assert_eq!(
            snap(
                &r,
                &keys(&["series:tvdb:2", "movie:tmdb:1", "series:tvdb:2"])
            ),
            base,
            "doublon sans effet"
        );
        // tout écart la change
        let mut later = r.clone();
        later.at += 1;
        let mut bigger = r.clone();
        bigger.entries[0].sides[1].bytes += 1;
        let mut other_card = r.clone();
        other_card.entries[1].sides[0].arr_id = 31;
        let mut other_side = r.clone();
        other_side.entries[1].sides[0].side = "vps".into();
        let mut one_copy_less = r.clone();
        one_copy_less.entries[0].sides.pop();
        let mut exempted = r.clone();
        exempted.entries.remove(1);
        for (what, other) in [
            (
                "date du calcul",
                snap(&later, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            (
                "taille",
                snap(&bigger, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            (
                "fiche",
                snap(&other_card, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            (
                "côté",
                snap(&other_side, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            (
                "copie en moins",
                snap(&one_copy_less, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            (
                "titre sorti de la liste",
                snap(&exempted, &keys(&["movie:tmdb:1", "series:tvdb:2"])),
            ),
            ("clé en moins", snap(&r, &keys(&["movie:tmdb:1"]))),
            (
                "clé en plus",
                snap(
                    &r,
                    &keys(&["movie:tmdb:1", "series:tvdb:2", "movie:tmdb:3"]),
                ),
            ),
        ] {
            assert_ne!(other, base, "{what} : empreinte inchangée");
        }
    }

    #[test]
    fn only_confirmed_red_and_not_exempted_titles_are_cleaned_up_to_the_cap() {
        let r = report(vec![
            entry("movie:tmdb:1", vec![side("seedbox", 1, 900)]),
            entry("movie:tmdb:2", vec![side("seedbox", 2, 800)]),
            entry("series:tvdb:3", vec![side("vps", 3, 700)]),
            entry("movie:tmdb:4", vec![side("seedbox", 4, 600)]),
        ]);
        // confirmé mais plus rouge (5), confirmé et exempté depuis (2), non confirmé (4)
        let confirmed = keys(&[
            "movie:tmdb:5",
            "series:tvdb:3",
            "movie:tmdb:2",
            "movie:tmdb:1",
        ]);
        let (picked, skipped) = select(&confirmed, &r, &exempt("movie:tmdb:2"), 50);
        let picked: Vec<&str> = picked.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(picked, ["movie:tmdb:1", "series:tvdb:3"], "ordre du calcul");
        assert_eq!(
            skipped,
            [
                ("movie:tmdb:2".to_string(), "exempté"),
                ("movie:tmdb:5".to_string(), "plus rouge au nouveau calcul"),
            ]
        );
        // plafond : les plus gros d'abord, le reste écarté et dit
        let all = keys(&[
            "movie:tmdb:1",
            "movie:tmdb:2",
            "series:tvdb:3",
            "movie:tmdb:4",
        ]);
        let (picked, skipped) = select(&all, &r, &BTreeMap::new(), 2);
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[1].key, "movie:tmdb:2");
        assert_eq!(
            skipped.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["series:tvdb:3", "movie:tmdb:4"]
        );
        assert!(skipped
            .iter()
            .all(|(_, why)| *why == "au-delà du plafond par clic"));
        // aucune clé confirmée : rien
        assert!(select(&[], &r, &BTreeMap::new(), 50).0.is_empty());
    }

    #[test]
    fn the_proposed_list_skips_exemptions_and_follows_the_cap() {
        let r = report(vec![
            entry("movie:tmdb:1", vec![side("seedbox", 1, 900)]),
            entry("movie:tmdb:2", vec![side("seedbox", 2, 800)]),
            entry("movie:tmdb:3", vec![side("seedbox", 3, 700)]),
        ]);
        let p: Vec<&str> = proposed(&r, &exempt("movie:tmdb:1"), 1)
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        assert_eq!(p, ["movie:tmdb:2"]);
        assert_eq!(proposed(&r, &BTreeMap::new(), 50).len(), 3);
    }

    #[test]
    fn simulation_follows_clean_dry_run_even_for_one_title() {
        // --dry-run l'emporte toujours
        assert!(effective_dry(false, false, true));
        // clean_dry_run : tout travail est simulé, un titre seul compris, même demandé en réel
        assert!(effective_dry(false, true, false));
        assert!(effective_dry(true, false, false), "simulation demandée");
        // après la mise en service
        assert!(!effective_dry(false, false, false));
    }

    #[test]
    fn a_card_must_still_be_the_confirmed_title() {
        let rs = RedSide {
            side: "seedbox".into(),
            arr_id: 12,
            folder: "/home/u/media/Movies/A (2020)".into(),
            ..Default::default()
        };
        let e = entry("movie:tmdb:100", vec![rs.clone()]);
        let ok = json!({"id": 12, "tmdbId": 100, "path": "/home/u/media/Movies/A (2020)"});
        assert!(card_matches(&e, &rs, &ok));
        for bad in [
            json!({"id": 13, "tmdbId": 100, "path": "/home/u/media/Movies/A (2020)"}),
            json!({"id": 12, "tmdbId": 101, "path": "/home/u/media/Movies/A (2020)"}),
            json!({"id": 12, "tmdbId": 100, "path": "/home/u/media/Anime/A (2020)"}),
            json!({"id": 12, "tmdbId": 100}),
        ] {
            assert!(!card_matches(&e, &rs, &bad), "{bad}");
        }
        let mut s = entry("series:tvdb:7", vec![rs.clone()]);
        s.movie = false;
        let series = json!({"id": 12, "tvdbId": 7, "tmdbId": 70, "path": rs.folder});
        assert!(card_matches(&s, &rs, &series));
        s.key = "series:tmdb:70".into();
        assert!(card_matches(&s, &rs, &series), "clé TMDB à défaut de TVDB");
        s.key = "series:tvdb:8".into();
        assert!(!card_matches(&s, &rs, &series));
        s.key = "movie:tmdb:70".into();
        assert!(
            !card_matches(&s, &rs, &series),
            "un film n'est pas une série"
        );
    }

    #[test]
    fn moves_and_playbacks_hold_a_title_back() {
        let at = 1_000_000;
        let moves = BTreeMap::from([
            ("seedbox:series:7".to_string(), at - 60),
            ("vps:movie:3".to_string(), at - 7 * 3600),
        ]);
        assert!(recently_moved(&moves, "seedbox", false, 7, at));
        assert!(
            !recently_moved(&moves, "seedbox", true, 7, at),
            "un film de même numéro"
        );
        assert!(!recently_moved(&moves, "vps", true, 3, at), "plus de 6 h");
        let playing = vec!["/seedbox/media/Movies/A (2020)/a.mkv".to_string()];
        assert!(is_playing(&playing, "/seedbox/media/Movies/A (2020)"));
        assert!(
            !is_playing(&playing, "/seedbox/media/Movies/A"),
            "préfixe d'un autre dossier"
        );
        assert!(!is_playing(&[], "/seedbox/media/Movies/A (2020)"));
    }

    fn vps_maps() -> Vec<PathMap> {
        vec![
            (
                "/movies".into(),
                "/opt/h/library/media/movies".into(),
                "/media/movies".into(),
            ),
            (
                "/tv".into(),
                "/opt/h/library/media/tvshows".into(),
                "/media/tvshows".into(),
            ),
        ]
    }

    fn recycle_dirs() -> Vec<PathBuf> {
        vec![
            "/opt/h/library/media/movies/.recycle".into(),
            "/opt/h/library/media/tvshows/.recycle".into(),
        ]
    }

    #[test]
    fn the_recycle_target_is_guarded() {
        // VPS : corbeille vue sous /data par l'Arr, rapprochée de [cleanup] recycle_dirs par la fin du chemin
        let (dir, target) = vps_recycle_target(
            "/data/media/movies/.recycle",
            "/movies/A (2020)",
            &vps_maps(),
            &recycle_dirs(),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/opt/h/library/media/movies/.recycle"));
        assert_eq!(
            target,
            PathBuf::from("/opt/h/library/media/movies/.recycle/A (2020)")
        );
        // corbeille sous une racine de vps_paths
        let (dir, _) =
            vps_recycle_target("/tv/.recycle", "/tv/B", &vps_maps(), &recycle_dirs()).unwrap();
        assert_eq!(dir, PathBuf::from("/opt/h/library/media/tvshows/.recycle"));
        for (bin, folder, why) in [
            (
                "/data/media/movies/../../etc",
                "/movies/A",
                "`..` dans la corbeille",
            ),
            (
                "/data/media/movies/.recycle",
                "/movies/..",
                "`..` comme dossier",
            ),
            (
                "/data/media/movies/.recycle",
                "/movies/A/../..",
                "`..` dans le dossier",
            ),
            ("/data/media/movies/.recycle", "/A", "dossier à la racine"),
            ("/data/media/movies/.recycle", "/", "racine"),
            ("/data/media/movies/.recycle", "movies/A", "chemin relatif"),
            (
                "/data/media/other/.trash",
                "/movies/A",
                "hors des corbeilles connues",
            ),
            ("/", "/movies/A", "corbeille à la racine"),
            ("/movies", "/movies/A", "corbeille = racine des films"),
        ] {
            assert!(
                vps_recycle_target(bin, folder, &vps_maps(), &recycle_dirs()).is_err(),
                "{why} accepté"
            );
        }
        // deux corbeilles connues aussi proches : on ne devine pas
        let twins: Vec<PathBuf> = vec![
            "/a/media/movies/.recycle".into(),
            "/b/media/movies/.recycle".into(),
        ];
        assert!(vps_recycle_dir("/data/media/movies/.recycle", &[], &twins).is_none());
        // une corbeille connue à la racine n'est jamais retenue
        assert!(vps_recycle_dir("/x/.recycle", &[], &["/.recycle".into()]).is_none());
        // seedbox : corbeille strictement sous la racine des médias
        let root = "/home/u/media";
        let (bin, target) = seedbox_recycle_target(
            "/home/u/media/Movies/.recycle",
            "/home/u/media/Movies/A (2020)",
            root,
        )
        .unwrap();
        assert_eq!(bin, "/home/u/media/Movies/.recycle");
        assert_eq!(target, "/home/u/media/Movies/.recycle/A (2020)");
        for (bin, folder) in [
            ("/home/u/media", "/home/u/media/Movies/A"),
            ("/home/u/.recycle", "/home/u/media/Movies/A"),
            ("/home/u/media/../.recycle", "/home/u/media/Movies/A"),
            ("/home/u/mediax/.recycle", "/home/u/media/Movies/A"),
            ("/home/u/media/Movies/.recycle", "/home/u/media/Movies/.."),
        ] {
            assert!(
                seedbox_recycle_target(bin, folder, root).is_err(),
                "{bin} {folder}"
            );
        }
        // reprise : la cible doit toujours être l'enfant direct d'une corbeille connue
        assert_eq!(
            vps_replay_bin(
                "/opt/h/library/media/movies/.recycle/A (2020)",
                &recycle_dirs()
            ),
            Some(PathBuf::from("/opt/h/library/media/movies/.recycle"))
        );
        assert_eq!(
            vps_replay_bin("/opt/h/library/media/movies/A (2020)", &recycle_dirs()),
            None
        );
        assert_eq!(
            vps_replay_bin("/opt/h/library/media/movies/.recycle/..", &recycle_dirs()),
            None
        );
        assert_eq!(
            seedbox_replay_bin("/home/u/media/Movies/.recycle/A", root).as_deref(),
            Some("/home/u/media/Movies/.recycle")
        );
        assert_eq!(
            seedbox_replay_bin("/home/u/media/A", root),
            None,
            "corbeille = racine"
        );
        assert_eq!(seedbox_replay_bin("/home/u/A/B", root), None);
    }

    #[test]
    fn local_recycle_removal_never_follows_a_link_nor_leaves_the_bin() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join(".recycle");
        let target = bin.join("A (2020)");
        std::fs::create_dir_all(target.join("Sub")).unwrap();
        std::fs::write(target.join("Sub/a.mkv"), b"x").unwrap();
        assert_eq!(
            remove_recycled(&target, &bin).unwrap(),
            RecycleOutcome::Removed
        );
        assert!(!target.exists());
        assert_eq!(
            remove_recycled(&target, &bin).unwrap(),
            RecycleOutcome::Absent
        );
        // un lien symbolique dans la corbeille : jamais suivi
        let outside = tmp.path().join("library");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.mkv"), b"x").unwrap();
        let link = bin.join("B");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(matches!(
            remove_recycled(&link, &bin).unwrap(),
            RecycleOutcome::Refused(_)
        ));
        assert!(
            outside.join("keep.mkv").exists(),
            "la cible du lien est intacte"
        );
        // un dossier qui n'est pas dans la corbeille
        assert!(matches!(
            remove_recycled(&outside, &bin).unwrap(),
            RecycleOutcome::Refused(_)
        ));
        assert!(outside.exists());
    }

    #[tokio::test]
    async fn the_vps_bin_is_emptied_only_once_the_origin_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join(".recycle");
        let target = bin.join("A");
        std::fs::create_dir_all(&target).unwrap();
        let origin = tmp.path().join("movies/A");
        std::fs::create_dir_all(&origin).unwrap();
        let out = vps_empty(
            Some(origin.clone()),
            target.clone(),
            bin.clone(),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            RecycleOutcome::Waiting,
            "l'Arr n'a pas fini de déplacer"
        );
        assert!(target.exists());
        std::fs::remove_dir(&origin).unwrap();
        let out = vps_empty(Some(origin), target.clone(), bin, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(out, RecycleOutcome::Removed);
        assert!(!target.exists());
    }

    #[test]
    fn ssh_paths_go_through_stdin_never_the_command_line() {
        let path = "/home/u/media/Movies/L'été ; rm -rf ~ (2020)";
        let args = ssh_args("seedbox", RECYCLE_PY);
        assert_eq!(args[args.len() - 2], "seedbox");
        assert!(args[..args.len() - 2]
            .iter()
            .all(|a| a.starts_with('-') || a.contains('=')));
        assert!(args
            .iter()
            .all(|a| !a.contains(path) && !a.contains("Movies")));
        assert!(
            args.last().unwrap().starts_with("python3 -c '"),
            "script entre apostrophes"
        );
        // le script ne change jamais : même ligne de commande quel que soit le titre
        assert_eq!(
            ssh_args("seedbox", MEASURE_PY),
            ssh_args("seedbox", MEASURE_PY)
        );
        let input = json!({ "dir": path }).to_string();
        assert!(
            input.contains("L'été ; rm -rf ~ (2020)"),
            "le chemin passe tel quel par l'entrée standard"
        );
        for script in [MEASURE_PY, RECYCLE_PY] {
            assert!(
                !script.contains('\''),
                "aucune apostrophe dans un script fixe"
            );
            assert!(script.contains("json.load(sys.stdin)"));
        }
    }

    #[test]
    fn recycle_script_answers_are_parsed() {
        assert_eq!(parse_recycle("removed\n"), Some(RecycleOutcome::Removed));
        assert_eq!(
            parse_recycle("bavard\nabsent"),
            Some(RecycleOutcome::Absent)
        );
        assert_eq!(parse_recycle("waiting"), Some(RecycleOutcome::Waiting));
        assert_eq!(
            parse_recycle("refused:target"),
            Some(RecycleOutcome::Refused("target".into()))
        );
        assert_eq!(parse_recycle("Traceback"), None);
        assert_eq!(parse_recycle(""), None);
    }

    #[test]
    fn hard_links_decide_what_is_freed_and_when() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("A");
        std::fs::create_dir_all(dir.join("Subs")).unwrap();
        std::fs::write(dir.join("a.nfo"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("a.mkv"), vec![0u8; 1000]).unwrap();
        std::fs::hard_link(dir.join("a.mkv"), tmp.path().join("torrent.mkv")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("torrent.mkv"), dir.join("Subs/lien")).unwrap();
        let s = tally_dir(&dir);
        assert_eq!(
            s,
            Sizes {
                exclusive: 10,
                linked: 1000,
                files: 2,
                missing: false
            },
            "lien symbolique ignoré"
        );
        assert!(tally_dir(&tmp.path().join("absent")).missing);
        // corbeille vidée, torrent retiré : tout de suite
        assert_eq!(account(s, true, &["torrent_deleted"]), (1010, 0, 0));
        // torrent C411 qui partage encore : le lié plus tard
        assert_eq!(account(s, true, &["torrent_pending"]), (10, 1000, 0));
        // corbeille à vider plus tard
        assert_eq!(account(s, false, &["torrent_deleted"]), (0, 1010, 0));
        // torrent gardé (pack) ou introuvable : partagé
        assert_eq!(
            account(s, true, &["torrent_deleted", "torrent_kept"]),
            (10, 0, 1000)
        );
        assert_eq!(account(s, true, &[]), (10, 0, 1000));
        assert_eq!(account(s, true, &["torrent_error"]), (10, 0, 1000));
    }

    fn step(kind: &str, side: &str, key: &str, first_at: i64, last_try: i64) -> PendingStep {
        PendingStep {
            step: kind.into(),
            key: key.into(),
            side: side.into(),
            first_at,
            last_try,
            tries: 1,
            ..Default::default()
        }
    }

    #[test]
    fn pending_steps_are_idempotent_capped_and_abandoned_after_14_days() {
        let mut map = BTreeMap::new();
        assert!(add_pending(
            &mut map,
            step("recycle", "seedbox", "movie:tmdb:1", 100, 100)
        ));
        // même étape rajoutée (relance) : la première garde sa date et ses essais
        let mut again = step("recycle", "seedbox", "movie:tmdb:1", 500, 500);
        again.tries = 9;
        assert!(!add_pending(&mut map, again));
        assert_eq!(map.len(), 1);
        let p = &map["recycle:seedbox:movie:tmdb:1"];
        assert_eq!((p.first_at, p.tries), (100, 1));
        // autre côté, autre étape : distinctes
        assert!(add_pending(
            &mut map,
            step("recycle", "vps", "movie:tmdb:1", 100, 100)
        ));
        assert!(add_pending(
            &mut map,
            step("seerr", "", "movie:tmdb:1", 100, 100)
        ));
        assert!(map.contains_key("seerr::movie:tmdb:1"));
        // plan de reprise : les moins récemment tentées d'abord, au plus `max`, les vieilles abandonnées
        let now = 100 + PENDING_MAX_AGE_SECS;
        let mut map = BTreeMap::new();
        for (i, last) in [(1, 900), (2, 300), (3, 600), (4, 50)] {
            add_pending(
                &mut map,
                step("seerr", "", &format!("movie:tmdb:{i}"), 200, last),
            );
        }
        add_pending(&mut map, step("recycle", "vps", "movie:tmdb:9", 100, 100));
        let (due, expired) = replay_plan(&map, now, 2, PENDING_MAX_AGE_SECS);
        assert_eq!(due, ["seerr::movie:tmdb:4", "seerr::movie:tmdb:2"]);
        assert_eq!(expired, ["recycle:vps:movie:tmdb:9"]);
        let (due, _) = replay_plan(&map, now, PENDING_PER_RUN, PENDING_MAX_AGE_SECS);
        assert_eq!(due.len(), 4, "5 au plus par passage");
    }

    fn job(id: &str, status: &str) -> CleanJob {
        CleanJob {
            id: id.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn jobs_are_kept_updated_in_place_and_interrupted_after_a_restart() {
        let mut jobs = vec![job("a", "done"), job("b", "done")];
        let mut c = job("c", "running");
        upsert_job(&mut jobs, &c, 2);
        assert_eq!(
            jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        c.status = "done".into();
        upsert_job(&mut jobs, &c, 2);
        assert_eq!(jobs.len(), 2, "mis à jour sur place");
        assert_eq!(jobs[1].status, "done");
        let mut jobs = vec![job("x", "running"), job("y", "running"), job("z", "done")];
        assert_eq!(interrupt_stale(&mut jobs, Some("y"), 42), 1);
        assert_eq!(jobs[0].status, "interrupted");
        assert_eq!(jobs[0].ended, Some(42));
        assert_eq!(
            jobs[1].status, "running",
            "le travail de ce processus continue"
        );
        assert_eq!(jobs[2].status, "done");
        assert_eq!(interrupt_stale(&mut jobs, None, 43), 1);
    }

    #[test]
    fn the_alert_names_titles_sizes_and_the_quota_before() {
        let mut j = job("20261021-101500-00ff", "done");
        j.quota_before = Some(QuotaReading {
            used_kb: 2_700_000_000,
            quota_kb: 3_725_000_000,
            at: 1,
        });
        j.results = vec![
            CleanResult {
                title: "Gros <film>".into(),
                outcome: "deleted".into(),
                freed_now: 4_000_000_000,
                ..Default::default()
            },
            CleanResult {
                title: "Série".into(),
                outcome: "partial".into(),
                freed_later: 2_000_000_000,
                detail: "vps : fiche toujours là".into(),
                ..Default::default()
            },
            CleanResult {
                outcome: "skipped".into(),
                ..Default::default()
            },
        ];
        let (subject, body, warn) = alert_text(&j).unwrap();
        assert!(subject.contains("1 titre(s) supprimé(s)"), "{subject}");
        assert!(body.contains("Gros <film> : supprimé"), "{body}");
        assert!(body.contains("Série : supprimé en partie"), "{body}");
        assert!(body.contains("2700 Go utilisés sur 3,7 To"), "{body}");
        assert!(body.contains("1 titre(s) écarté(s)"), "{body}");
        assert!(warn, "un titre en partie : avertissement");
        assert!(job_summary(&j).starts_with("1 supprimé(s), 1 en partie, 1 écarté(s)"));
        // simulation, ou tout écarté : pas d'alerte
        j.dry_run = true;
        assert!(alert_text(&j).is_none());
        let mut skipped = job("s", "done");
        skipped.results = vec![CleanResult {
            outcome: "skipped".into(),
            ..Default::default()
        }];
        assert!(alert_text(&skipped).is_none());
    }

    #[test]
    fn cards_are_saved_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = backup_dir(tmp.path(), 1_790_000_000);
        assert!(dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("liste-rouge-20"));
        let p = save_card(&dir, "seedbox", true, 12, &json!({"id": 12, "title": "A"})).unwrap();
        assert_eq!(p.file_name().unwrap(), "seedbox-movie-12.json");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back["title"], "A");
        // relance le même jour : réécrite, toujours privée
        save_card(&dir, "seedbox", true, 12, &json!({"id": 12})).unwrap();
        assert_eq!(mode(&p), 0o600);
    }

    #[tokio::test]
    async fn a_failed_delete_counts_only_if_the_card_is_gone() {
        use crate::secret::Secret;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        // 1 : supprimée ; 2 : erreur mais relue 404 ; 3 : erreur et toujours là ; 4 : erreur puis relecture en panne
        Mock::given(method("DELETE"))
            .and(path("/api/v3/movie/1"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        for id in [2, 3, 4] {
            Mock::given(method("DELETE"))
                .and(path(format!("/api/v3/movie/{id}")))
                .respond_with(ResponseTemplate::new(504))
                .mount(&server)
                .await;
        }
        for (id, code) in [(2, 404), (3, 200), (4, 500)] {
            Mock::given(method("GET"))
                .and(path(format!("/api/v3/movie/{id}")))
                .respond_with(ResponseTemplate::new(code).set_body_json(json!({"id": id})))
                .mount(&server)
                .await;
        }
        let arr = ArrClient::new(
            "radarr",
            &format!("{}/", server.uri()),
            Secret::new("k"),
            reqwest::Client::new(),
        )
        .unwrap();
        let q = [("deleteFiles", "true")];
        let t = Duration::from_secs(5);
        assert_eq!(
            delete_card(&arr, "api/v3/movie/1", &q, t).await,
            Ok(Deletion::Deleted)
        );
        assert_eq!(
            delete_card(&arr, "api/v3/movie/2", &q, t).await,
            Ok(Deletion::Gone)
        );
        let still = delete_card(&arr, "api/v3/movie/3", &q, t)
            .await
            .unwrap_err();
        assert!(still.contains("toujours là"), "{still}");
        let unknown = delete_card(&arr, "api/v3/movie/4", &q, t)
            .await
            .unwrap_err();
        assert!(unknown.contains("état inconnu"), "{unknown}");
        let deletes = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "DELETE")
            .count();
        assert_eq!(deletes, 4, "aucun DELETE rejoué");
    }

    #[test]
    fn one_job_at_a_time() {
        let first = reserve("t1").expect("place libre");
        assert!(busy());
        assert!(reserve("t2").is_none(), "second travail refusé");
        assert_eq!(current().lock().unwrap().as_deref(), Some("t1"));
        assert!(
            reserve_recompute().is_none(),
            "pas de recalcul pendant un travail"
        );
        drop(first);
        assert!(!busy());
        assert_eq!(current().lock().unwrap().as_deref(), None);
        // un « Recalculer » tient la même place, sans se faire passer pour un travail
        let recompute = reserve_recompute().expect("place libre");
        assert!(busy());
        assert!(
            reserve("t3").is_none(),
            "pas de « Nettoyer » pendant un recalcul"
        );
        assert!(reserve_recompute().is_none(), "un seul recalcul");
        assert_eq!(current().lock().unwrap().as_deref(), None);
        drop(recompute);
        assert!(!busy());
        let id = new_job_id();
        assert_eq!(id.len(), "20261021-101500-3fa2".len(), "{id}");
    }

    /// Les scripts envoyés à la seedbox, joués ici par `python3 -c` comme là-bas (entrée standard en JSON).
    async fn py(script: &str, input: Value) -> String {
        docker::run_stdin(
            "python3",
            &["-c", script],
            input.to_string().as_bytes(),
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_seedbox_scripts_keep_their_guards() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("python3 absent : scripts de la seedbox non joués");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("media");
        let bin = base.join("Movies/.recycle");
        let s = |p: &Path| p.to_string_lossy().to_string();
        // relevé : lié et non lié, dossier absent
        let title = base.join("Movies/A (2020)");
        std::fs::create_dir_all(&title).unwrap();
        std::fs::write(title.join("a.nfo"), vec![0u8; 10]).unwrap();
        std::fs::write(title.join("a.mkv"), vec![0u8; 1000]).unwrap();
        std::fs::hard_link(title.join("a.mkv"), tmp.path().join("t.mkv")).unwrap();
        let sizes: Sizes =
            serde_json::from_str(&py(MEASURE_PY, json!({ "dir": s(&title) })).await).unwrap();
        assert_eq!((sizes.exclusive, sizes.linked, sizes.files), (10, 1000, 2));
        let none: Sizes =
            serde_json::from_str(&py(MEASURE_PY, json!({ "dir": s(&base.join("x")) })).await)
                .unwrap();
        assert!(none.missing);
        // corbeille : rien tant que le dossier d'origine est là
        let target = bin.join("A (2020)");
        std::fs::create_dir_all(&target).unwrap();
        let input = |target: &Path, origin: &Path| json!({"base": s(&base), "recycle": s(&bin), "target": s(target), "origin": s(origin), "wait": 0});
        assert_eq!(py(RECYCLE_PY, input(&target, &title)).await, "waiting");
        assert!(target.exists());
        std::fs::remove_dir_all(&title).unwrap();
        assert_eq!(py(RECYCLE_PY, input(&target, &title)).await, "removed");
        assert!(!target.exists());
        assert_eq!(py(RECYCLE_PY, input(&target, &title)).await, "absent");
        // lien symbolique dans la corbeille : jamais suivi
        let keep = tmp.path().join("garder");
        std::fs::create_dir_all(&keep).unwrap();
        std::os::unix::fs::symlink(&keep, bin.join("B")).unwrap();
        assert_eq!(
            py(RECYCLE_PY, input(&bin.join("B"), &title)).await,
            "refused:target"
        );
        assert!(keep.exists());
        // cible qui n'est pas l'enfant direct de la corbeille
        std::fs::create_dir_all(bin.join("C/D")).unwrap();
        assert_eq!(
            py(RECYCLE_PY, input(&bin.join("C/D"), &title)).await,
            "refused:target"
        );
        // corbeille hors de la racine des médias
        let out = py(
            RECYCLE_PY,
            json!({"base": s(&base.join("Movies/A")), "recycle": s(&bin), "target": s(&bin.join("C")), "origin": "", "wait": 0}),
        )
        .await;
        assert_eq!(out, "refused:recycle");
        assert!(bin.join("C/D").exists());
    }
}
