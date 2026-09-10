use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

/// Exécute `docker <args>` et renvoie stdout (trim). Échoue sur code ≠ 0.
pub async fn docker(args: &[&str]) -> Result<String> {
    run("docker", args, None).await
}

pub async fn compose(base: &Path, args: &[&str]) -> Result<String> {
    let mut full = vec!["compose"];
    full.extend_from_slice(args);
    run("docker", &full, Some(base)).await
}

pub async fn exec_in(container: &str, cmd: &[&str]) -> Result<String> {
    let mut args = vec!["exec", container];
    args.extend_from_slice(cmd);
    docker(&args).await
}

pub async fn run(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut c = Command::new(program);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c
        .output()
        .await
        .with_context(|| format!("lancement de {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} → {} : {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
