//! Sauvegarde de l'état (ex `scripts/backup-state.sh`) : dump MySQL guacamole,
//! tar+zstd de `paths.base` hors médias/métriques/caches, checksum, manifeste,
//! unités systemd, compose rendu, référence images. Nécessite root (npm/homarr/
//! grafana ont leurs propres uid) : lancé par `homelabctl backup` sous sudo.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tracing::info;

use crate::config::Config;
use crate::docker;

pub struct BackupOutput {
    pub archive: PathBuf,
    pub files: Vec<PathBuf>,
}

pub async fn run(cfg: &Config) -> Result<BackupOutput> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let out_dir = &cfg.paths.backups;
    std::fs::create_dir_all(out_dir)?;
    let mut files = Vec::new();

    let dump = out_dir.join(format!("guacdb-{ts}.sql.gz"));
    mysqldump(&cfg.backup.mysql_container, &dump).await?;
    files.push(dump);

    let archive = out_dir.join(format!("homelab-state-{ts}.tar.zst"));
    tar_state(&cfg.paths.base, &cfg.backup.excludes, &archive).await?;
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
    let base = cfg.paths.base.display();
    sh(&format!(
        "set -e; T=$(mktemp -d); mkdir -p \"$T/systemd\" \"$T/cron\"; \
         cp /etc/systemd/system/homelab*.* /etc/systemd/system/homelabd.service \"$T/systemd/\" 2>/dev/null || true; \
         crontab -u deploy -l > \"$T/cron/deploy.crontab\" 2>/dev/null || true; \
         (cd '{base}' && docker compose config > \"$T/compose-rendered.yml\"); \
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
    Ok(BackupOutput { archive, files })
}

async fn mysqldump(container: &str, out: &Path) -> Result<()> {
    let pw = docker::exec_in(container, &["printenv", "MYSQL_ROOT_PASSWORD"]).await?;
    let cmd = format!(
        "docker exec -e MYSQL_PWD='{pw}' {container} mysqldump --all-databases --single-transaction --routines --triggers | gzip -6 > '{}'",
        out.display()
    );
    sh(&cmd).await.map(|_| ())
}

async fn tar_state(base: &Path, excludes: &[String], out: &Path) -> Result<()> {
    let parent = base.parent().context("paths.base sans parent")?;
    let name = base
        .file_name()
        .context("paths.base sans nom")?
        .to_string_lossy();
    let mut ex = String::new();
    for e in excludes {
        ex.push_str(&format!(" --exclude='{name}/{e}'"));
    }
    // tar rc 1 = "file changed as we read it" sur un système vivant : toléré ; rc 2 = fatal
    let cmd = format!(
        "set -o pipefail; tar --numeric-owner --warning=no-file-changed -cpf - -C '{}'{ex} '{name}' | zstd -T4 -3 -q -f -o '{}'; rc=${{PIPESTATUS[0]}}; [ \"$rc\" -le 1 ]",
        parent.display(),
        out.display()
    );
    sh(&cmd).await.map(|_| ())
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
