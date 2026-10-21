use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::io::AsyncWriteExt;
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
    // une commande abandonnée (délai de 600 s du planificateur) est tuée au lieu de rester orpheline
    c.args(args)
        .kill_on_drop(true)
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

/// Comme [`run`], mais `input` est écrit sur l'entrée standard (puis fermée) : un chemin passe par là plutôt que par
/// la ligne de commande (`ssh seedbox cmd args` recolle les arguments, règle de CLAUDE.md). Les arguments ne sont pas
/// recopiés dans l'erreur : ils portent un script entier.
pub async fn run_stdin(
    program: &str,
    args: &[&str],
    input: &[u8],
    cwd: Option<&Path>,
) -> Result<String> {
    let mut c = Command::new(program);
    c.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("lancement de {program}"))?;
    let mut stdin = child.stdin.take().context("entrée standard indisponible")?;
    // écriture et lecture en même temps : une sortie abondante ne bloque pas l'écriture
    let write = async move {
        let r = stdin.write_all(input).await;
        drop(stdin); // fin de l'entrée
        r
    };
    let (written, out) = tokio::join!(write, child.wait_with_output());
    let out = out.with_context(|| format!("attente de {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} → {} : {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    written.with_context(|| format!("entrée standard de {program}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stdin_reaches_the_program_and_never_its_arguments() {
        let out = run_stdin("cat", &[], b"/chemin/avec espace\n", None)
            .await
            .unwrap();
        assert_eq!(
            out, "/chemin/avec espace",
            "sortie lue, fin de ligne retirée"
        );
        let err = run_stdin(
            "sh",
            &["-c", "cat >/dev/null; echo non >&2; exit 3"],
            b"x",
            None,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("non"), "{msg}");
        assert!(
            !msg.contains("cat >/dev/null"),
            "arguments recopiés : {msg}"
        );
    }
}
