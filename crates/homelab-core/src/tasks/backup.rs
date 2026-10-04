//! Sauvegarde de l'état (ex `scripts/backup-state.sh`) : dump MySQL guacamole, copie cohérente des bases
//! SQLite (`[backup] sqlite`), tar+zstd de `paths.base` hors médias/métriques/caches, checksum, manifeste,
//! unités systemd, compose rendu, référence images. Nécessite root (npm/homarr/grafana ont leurs propres uid) :
//! lancé par `homelabctl backup` sous sudo (`homelab-backup.service`, hors du cloisonnement de homelabd).

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::Config;
use crate::docker;

/// Dossier (relatif à `paths.base`) des copies de bases SQLite : `state/backup-snapshots/<chemin d'origine>`,
/// inclus dans l'archive puis supprimé.
pub const SNAPSHOT_DIR: &str = "state/backup-snapshots";

/// Fichiers qui accompagnent une base vivante : ils sortent de l'archive avec elle.
const LIVE_SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

pub struct BackupOutput {
    pub archive: PathBuf,
    pub files: Vec<PathBuf>,
    /// Bases copiées par l'API de sauvegarde SQLite (chemins relatifs à `paths.base`).
    pub sqlite: Vec<String>,
    /// Motifs invalides, bases absentes, bases non copiées (restées telles quelles dans l'archive).
    pub warnings: Vec<String>,
}

pub async fn run(cfg: &Config) -> Result<BackupOutput> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let out_dir = &cfg.paths.backups;
    std::fs::create_dir_all(out_dir)?;
    let mut files = Vec::new();

    let dump = out_dir.join(format!("guacdb-{ts}.sql.gz"));
    mysqldump(&cfg.backup.mysql_container, &dump).await?;
    files.push(dump);

    // Les bases SQLite des services tournent pendant la sauvegarde : un tar lirait la base, son -wal et son -shm
    // à des instants différents. Chacune est d'abord copiée par l'API de sauvegarde SQLite (instantané cohérent,
    // WAL compris), la copie entre dans l'archive et la base vivante en sort.
    let base = cfg.paths.base.clone();
    let snap_dir = base.join(SNAPSHOT_DIR);
    let (sqlite, warnings) = {
        let (base, snap_dir, patterns) =
            (base.clone(), snap_dir.clone(), cfg.backup.sqlite.clone());
        tokio::task::spawn_blocking(move || snapshot_all(&base, &patterns, &snap_dir)).await??
    };
    for w in &warnings {
        warn!(task = "backup", "{w}");
    }
    info!(
        task = "backup",
        copied = sqlite.len(),
        "SQLite snapshots ready"
    );
    let archive = out_dir.join(format!("homelab-state-{ts}.tar.zst"));
    let tarred = tar_state(&base, &cfg.backup.excludes, &sqlite, &archive).await;
    let _ = std::fs::remove_dir_all(&snap_dir);
    tarred?;
    files.push(archive.clone());

    let sha = format!("{}.sha256", archive.display());
    let digest = sh(&format!(
        "cd '{}' && sha256sum '{}'",
        out_dir.display(),
        archive.file_name().unwrap().to_string_lossy()
    ))
    .await?;
    std::fs::write(&sha, format!("{digest}\n"))?;
    files.push(PathBuf::from(&sha));

    let list = format!("{}.list.gz", archive.display());
    sh(&format!(
        "zstd -dc '{}' | tar -tvf - | gzip -6 > '{list}'",
        archive.display()
    ))
    .await?;
    files.push(PathBuf::from(&list));

    let sys = out_dir.join(format!("systemd-{ts}.tar.gz"));
    let base_s = cfg.paths.base.display();
    sh(&format!(
        "set -e; T=$(mktemp -d); mkdir -p \"$T/systemd\" \"$T/cron\"; \
         cp /etc/systemd/system/homelab*.* /etc/systemd/system/homelabd.service \"$T/systemd/\" 2>/dev/null || true; \
         crontab -u deploy -l > \"$T/cron/deploy.crontab\" 2>/dev/null || true; \
         (cd '{base_s}' && docker compose config > \"$T/compose-rendered.yml\"); \
         tar -czf '{}' -C \"$T\" .; rm -rf \"$T\"",
        sys.display()
    ))
    .await?;
    files.push(sys);

    let images = out_dir.join(format!("images-{ts}.txt"));
    std::fs::write(
        &images,
        docker::docker(&["images", "--digests", "--no-trunc"]).await?,
    )?;
    files.push(images);

    for f in &files {
        let _ = sh(&format!(
            "chmod 600 '{}' && chown 1000:1000 '{}'",
            f.display(),
            f.display()
        ))
        .await;
    }
    prune(out_dir, cfg.backup.keep_last)?;
    info!(task = "backup", archive = %archive.display(), "done");
    Ok(BackupOutput {
        archive,
        files,
        sqlite,
        warnings,
    })
}

async fn mysqldump(container: &str, out: &Path) -> Result<()> {
    // Le mot de passe reste dans le conteneur (sa propre variable d'environnement) : il n'apparaît plus dans
    // la liste des processus de l'hôte, et une apostrophe dans sa valeur ne casse plus la commande.
    let cmd = format!(
        "set -o pipefail; docker exec {container} sh -c 'export MYSQL_PWD=\"$MYSQL_ROOT_PASSWORD\"; exec mysqldump -uroot --all-databases --single-transaction --routines --triggers' | gzip -6 > '{}'",
        out.display()
    );
    sh(&cmd).await.map(|_| ())
}

/// Motif de `[backup] sqlite` acceptable : chemin relatif sans `.`/`..`, `*` dans le seul nom de fichier, hors du
/// dossier des copies, sans caractère qui changerait de sens dans la liste d'exclusion de tar.
fn valid_pattern(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && !p.ends_with('/')
        && !p.contains(['\n', '\\'])
        && path.is_relative()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && p.rsplit_once('/').is_none_or(|(dir, _)| !dir.contains('*'))
        && !path.starts_with(SNAPSHOT_DIR)
}

/// `*` = n'importe quelle suite de caractères (vide comprise) ; tout le reste est littéral.
fn wildcard(pat: &str, name: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    let [first, .., last] = parts.as_slice() else {
        return pat == name;
    };
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// En-tête « SQLite format 3\0 » : un `*.db` qui n'est pas une base (BoltDB, fichier vide…) reste au tar.
fn is_sqlite(path: &Path) -> bool {
    let mut head = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == b"SQLite format 3\0"
}

/// Bases à copier (chemins relatifs à `base`, triés, sans doublon) et avertissements. Un motif à `*` qui ne
/// trouve rien, ou qui tombe sur un fichier qui n'est pas une base, n'est pas une erreur ; un chemin explicite
/// absent ou qui n'est pas une base est signalé.
pub(crate) fn resolve_sqlite(base: &Path, patterns: &[String]) -> (Vec<String>, Vec<String>) {
    let mut found = BTreeSet::new();
    let mut warnings = Vec::new();
    for p in patterns {
        if !valid_pattern(p) {
            warnings.push(format!(
                "{p} : motif ignoré (chemin relatif, `*` dans le nom de fichier seulement, hors {SNAPSHOT_DIR})"
            ));
            continue;
        }
        let (dir, name) = p.rsplit_once('/').unwrap_or(("", p.as_str()));
        if !name.contains('*') {
            let path = base.join(p);
            if !path.is_file() {
                warnings.push(format!("{p} : absente, rien à copier"));
            } else if !is_sqlite(&path) {
                warnings.push(format!(
                    "{p} : pas une base SQLite, laissée telle quelle dans l'archive"
                ));
            } else {
                found.insert(p.clone());
            }
            continue;
        }
        let Ok(entries) = std::fs::read_dir(base.join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let file_name = e.file_name();
            let Some(n) = file_name.to_str() else {
                continue;
            };
            let rel = if dir.is_empty() {
                n.to_string()
            } else {
                format!("{dir}/{n}")
            };
            if wildcard(name, n)
                && valid_pattern(&rel)
                && e.file_type().is_ok_and(|t| t.is_file())
                && is_sqlite(&e.path())
            {
                found.insert(rel);
            }
        }
    }
    (found.into_iter().collect(), warnings)
}

/// Lignes de la liste d'exclusion de tar (`--anchored --no-wildcards` : chemin exact) : chaque base copiée et
/// ses fichiers d'accompagnement, sous le nom de premier niveau de l'archive (`homelab/…`).
fn live_excludes(top: &str, copied: &[String]) -> Vec<String> {
    copied
        .iter()
        .flat_map(|rel| LIVE_SUFFIXES.map(|s| format!("{top}/{rel}{s}")))
        .collect()
}

/// Copie de chaque base de `patterns` dans `dest/<chemin>`. Une base qui ne se copie pas est signalée et reste
/// dans l'archive telle quelle (comme avant) : la sauvegarde ne s'arrête pas pour elle.
fn snapshot_all(
    base: &Path,
    patterns: &[String],
    dest: &Path,
) -> Result<(Vec<String>, Vec<String>)> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).with_context(|| format!("création de {}", dest.display()))?;
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o700))?;
    let (dbs, mut warnings) = resolve_sqlite(base, patterns);
    let mut copied = Vec::new();
    for rel in dbs {
        let out = dest.join(&rel);
        match snapshot_one(&base.join(&rel), &out) {
            Ok(()) => copied.push(rel),
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                warnings.push(format!(
                    "{rel} : copie impossible, base laissée telle quelle dans l'archive ({e:#})"
                ));
            }
        }
    }
    Ok((copied, warnings))
}

/// Instantané d'une base par l'API de sauvegarde SQLite : connexion en lecture seule (rien n'est écrit dans la
/// base ; en WAL, les écrivains ne sont pas bloqués), copie en une passe (`step(-1)` : une copie par petits pas
/// repartirait de zéro à chaque écriture d'un autre processus), puis `quick_check` de la copie, qui reprend
/// propriétaire et droits de l'original.
fn snapshot_one(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(dst);
    let from = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("ouverture de {}", src.display()))?;
    from.busy_timeout(Duration::from_secs(10))?;
    let mut to = Connection::open(dst).with_context(|| format!("création de {}", dst.display()))?;
    {
        let backup = Backup::new(&from, &mut to)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match backup.step(-1)? {
                StepResult::Done => break,
                _ if Instant::now() >= deadline => bail!("base occupée pendant 2 min"),
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }
    let check: String = to.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if check != "ok" {
        bail!("quick_check de la copie : {check}");
    }
    drop(to);
    drop(from);
    let meta = std::fs::metadata(src)?;
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    // Sous root (le cas réel) la copie garde le propriétaire de la base ; ailleurs (tests), sans effet.
    let _ = std::os::unix::fs::chown(dst, Some(meta.uid()), Some(meta.gid()));
    Ok(())
}

async fn tar_state(base: &Path, excludes: &[String], copied: &[String], out: &Path) -> Result<()> {
    let parent = base.parent().context("paths.base sans parent")?;
    let name = base
        .file_name()
        .context("paths.base sans nom")?
        .to_string_lossy();
    let mut ex = String::new();
    for e in excludes {
        ex.push_str(&format!(" --exclude='{name}/{e}'"));
    }
    // Bases copiées : exclues par chemin exact. `--anchored --no-wildcards` ne vaut que pour les exclusions qui
    // suivent (GNU tar) : celles de `[backup] excludes`, avant, gardent leur sens.
    let live_list = PathBuf::from(format!("{}.sqlite-excludes", out.display()));
    if !copied.is_empty() {
        let mut lines = live_excludes(&name, copied).join("\n");
        lines.push('\n');
        std::fs::write(&live_list, lines)?;
        ex.push_str(&format!(
            " --anchored --no-wildcards --exclude-from='{}'",
            live_list.display()
        ));
    }
    // tar rc 1 = "file changed as we read it" sur un système vivant : toléré ; rc 2 = fatal. zstd doit réussir
    // (jusqu'au 2026-09-23 seul tar était vérifié : un disque plein laissait une archive tronquée « réussie »),
    // puis l'archive entière est relue avant qu'on accepte de supprimer les anciennes.
    let cmd = format!(
        "tar --numeric-owner --warning=no-file-changed -cpf - -C '{}'{ex} '{name}' | zstd -T4 -3 -q -f -o '{out}'; \
         st=(\"${{PIPESTATUS[@]}}\"); [ \"${{st[0]}}\" -le 1 ] && [ \"${{st[1]}}\" -eq 0 ] && zstd -t -q '{out}'",
        parent.display(),
        out = out.display()
    );
    let res = sh(&cmd).await.map(|_| ());
    let _ = std::fs::remove_file(&live_list);
    res
}

fn prune(dir: &Path, keep: usize) -> Result<()> {
    let mut archives: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("homelab-state-") && n.ends_with(".tar.zst"))
                .unwrap_or(false)
        })
        .collect();
    archives.sort();
    if archives.len() <= keep {
        return Ok(());
    }
    for old in &archives[..archives.len() - keep] {
        let stem = old
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".tar.zst")
            .trim_start_matches("homelab-state-")
            .to_string();
        for e in std::fs::read_dir(dir)?.flatten() {
            if e.file_name().to_string_lossy().contains(&stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        info!(task = "backup", removed = %old.display(), "pruned old backup");
    }
    Ok(())
}

async fn sh(cmd: &str) -> Result<String> {
    let out = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        bail!(
            "commande échouée ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Base en WAL dont une transaction validée est encore dans le `-wal` (aucun checkpoint) : la connexion
    /// renvoyée doit rester ouverte, comme celle d'un service en marche.
    fn live_wal_db(path: &Path, value: i64) -> Connection {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = Connection::open(path).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE m(x);")
            .unwrap();
        c.execute("INSERT INTO m VALUES (?1)", [value]).unwrap();
        c
    }

    fn rollback_db(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE t(y); INSERT INTO t VALUES ('ok');")
            .unwrap();
    }

    fn touch(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn wildcard_matches_file_names_only_as_written() {
        assert!(wildcard("*.db", "chat.db"));
        assert!(wildcard("*.db", ".db"));
        assert!(!wildcard("*.db", "chat.db-wal"));
        assert!(!wildcard("*.db", "chat.db.bak-20260525"));
        assert!(!wildcard("*.db", "db.sqlite"));
        assert!(wildcard("introskipper*.db", "introskipper-v2.db"));
        assert!(wildcard("a*b*c", "a-b-b-c"));
        assert!(!wildcard("a*b*c", "a-c"));
        assert!(!wildcard("ab*ba", "aba"));
        assert!(wildcard("db.sqlite3", "db.sqlite3"));
        assert!(!wildcard("db.sqlite3", "db.sqlite3-wal"));
    }

    #[test]
    fn invalid_patterns_are_refused() {
        for ok in ["state/*.db", "grafana/grafana.db", "x.db", "a/b/c*.sqlite"] {
            assert!(valid_pattern(ok), "{ok}");
        }
        for bad in [
            "",
            "/opt/homelab/state/chat.db",
            "../etc/x.db",
            "state/../x.db",
            "./state/x.db",
            "*/config/*.db",
            "state/",
            "state/backup-snapshots/*.db",
            "state/backup-snapshots/state/chat.db",
            "a\\b.db",
        ] {
            assert!(!valid_pattern(bad), "{bad}");
        }
    }

    #[test]
    fn resolve_picks_sqlite_files_and_reports_explicit_problems() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let _w = live_wal_db(&base.join("state/chat.db"), 1);
        rollback_db(&base.join("grafana/grafana.db"));
        rollback_db(&base.join("jellyfin/config/data/jellyfin.db"));
        rollback_db(&base.join("jellyfin/config/data/playback_reporting.db"));
        std::fs::copy(
            base.join("jellyfin/config/data/jellyfin.db"),
            base.join("jellyfin/config/data/jellyfin.db.bak-introskip-20260525"),
        )
        .unwrap();
        touch(
            &base.join("state/notes.db"),
            "pas une base (BoltDB, fichier vide…)",
        );
        touch(&base.join("state/vide.db"), "");
        touch(&base.join("diun/diun.db"), "bolt");
        rollback_db(&base.join("state/backup-snapshots/state/old.db"));
        std::fs::create_dir_all(base.join("state/dossier.db")).unwrap();

        let (dbs, warnings) = resolve_sqlite(
            base,
            &strings(&[
                "state/*.db",
                "jellyfin/config/data/*.db",
                "grafana/grafana.db",
                "grafana/grafana.db", // doublon
                "state/chat.db",      // déjà couvert par le motif
                "pyload/config/data/pyload.db",
                "diun/diun.db",
                "absent/*.db",
                "../dehors.db",
            ]),
        );
        assert_eq!(
            dbs,
            strings(&[
                "grafana/grafana.db",
                "jellyfin/config/data/jellyfin.db",
                "jellyfin/config/data/playback_reporting.db",
                "state/chat.db",
            ])
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].starts_with("pyload/config/data/pyload.db : absente"));
        assert!(warnings[1].starts_with("diun/diun.db : pas une base SQLite"));
        assert!(warnings[2].starts_with("../dehors.db : motif ignoré"));
    }

    #[test]
    fn live_files_are_excluded_by_exact_path() {
        assert_eq!(
            live_excludes(
                "homelab",
                &strings(&["state/chat.db", "npm/data/database.sqlite"])
            ),
            strings(&[
                "homelab/state/chat.db",
                "homelab/state/chat.db-wal",
                "homelab/state/chat.db-shm",
                "homelab/state/chat.db-journal",
                "homelab/npm/data/database.sqlite",
                "homelab/npm/data/database.sqlite-wal",
                "homelab/npm/data/database.sqlite-shm",
                "homelab/npm/data/database.sqlite-journal",
            ])
        );
    }

    #[test]
    fn snapshot_holds_wal_content_and_keeps_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let writer = live_wal_db(&base.join("state/chat.db"), 42);
        assert!(base.join("state/chat.db-wal").metadata().unwrap().len() > 0);
        std::fs::set_permissions(
            base.join("state/chat.db"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        let dest = base.join(SNAPSHOT_DIR);
        let (copied, warnings) = snapshot_all(base, &strings(&["state/*.db"]), &dest).unwrap();
        assert_eq!(copied, strings(&["state/chat.db"]));
        assert!(warnings.is_empty(), "{warnings:?}");
        // Le service continue d'écrire après l'instantané : la copie n'en voit rien.
        writer.execute("INSERT INTO m VALUES (43)", []).unwrap();
        let copy_path = dest.join("state/chat.db");
        let copy = Connection::open(&copy_path).unwrap();
        let rows: Vec<i64> = copy
            .prepare("SELECT x FROM m ORDER BY x")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec![42]);
        drop(copy);
        assert_eq!(copy_path.metadata().unwrap().mode() & 0o777, 0o640);
        assert_eq!(dest.metadata().unwrap().mode() & 0o777, 0o700);
        assert!(!dest.join("state/chat.db-wal").exists());
    }

    #[test]
    fn unreadable_database_stays_in_archive_with_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // En-tête SQLite valide, corps illisible : la copie échoue, la sauvegarde continue.
        let mut junk = b"SQLite format 3\0".to_vec();
        junk.extend_from_slice(&[0xffu8; 200]);
        std::fs::create_dir_all(base.join("svc")).unwrap();
        std::fs::write(base.join("svc/broken.db"), junk).unwrap();
        rollback_db(&base.join("svc/good.db"));
        let dest = base.join(SNAPSHOT_DIR);
        let (copied, warnings) = snapshot_all(base, &strings(&["svc/*.db"]), &dest).unwrap();
        assert_eq!(copied, strings(&["svc/good.db"]));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("svc/broken.db : copie impossible"));
        assert!(!dest.join("svc/broken.db").exists());
    }

    /// Chaîne complète sur une arborescence jetable : copie, tar (GNU tar + zstd, comme en production), puis
    /// lecture de la liste de l'archive et de la base restaurée.
    #[tokio::test]
    async fn archive_holds_snapshots_instead_of_live_files() {
        if std::process::Command::new("zstd")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("zstd absent : test sauté");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("homelab");
        let _writer = live_wal_db(&base.join("state/chat.db"), 7);
        rollback_db(&base.join("grafana/grafana.db"));
        touch(&base.join("state/chat.db.bak"), "copie manuelle, gardée");
        touch(&base.join("state/state.json"), "{}");
        touch(&base.join("logs/x.log"), "exclu");
        let snap = base.join(SNAPSHOT_DIR);
        let (copied, warnings) = snapshot_all(
            &base,
            &strings(&["state/*.db", "grafana/grafana.db"]),
            &snap,
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let out = tmp.path().join("out.tar.zst");
        tar_state(&base, &strings(&["logs"]), &copied, &out)
            .await
            .unwrap();
        assert!(!PathBuf::from(format!("{}.sqlite-excludes", out.display())).exists());
        let listing = sh(&format!("zstd -dc '{}' | tar -tf -", out.display()))
            .await
            .unwrap();
        let members: BTreeSet<&str> = listing.lines().collect();
        for present in [
            "homelab/state/backup-snapshots/state/chat.db",
            "homelab/state/backup-snapshots/grafana/grafana.db",
            "homelab/state/chat.db.bak",
            "homelab/state/state.json",
        ] {
            assert!(members.contains(present), "{present} manque : {listing}");
        }
        for absent in [
            "homelab/state/chat.db",
            "homelab/state/chat.db-wal",
            "homelab/state/chat.db-shm",
            "homelab/grafana/grafana.db",
            "homelab/logs/x.log",
        ] {
            assert!(!members.contains(absent), "{absent} présent : {listing}");
        }
        let restore = tmp.path().join("restore");
        std::fs::create_dir_all(&restore).unwrap();
        sh(&format!(
            "zstd -dc '{}' | tar -xf - -C '{}' homelab/state/backup-snapshots/state/chat.db",
            out.display(),
            restore.display()
        ))
        .await
        .unwrap();
        let c =
            Connection::open(restore.join("homelab/state/backup-snapshots/state/chat.db")).unwrap();
        let x: i64 = c.query_row("SELECT x FROM m", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 7);
    }
}
