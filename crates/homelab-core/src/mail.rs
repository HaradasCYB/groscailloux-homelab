use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Smtp;

/// Envoie un mail texte via `curl smtps://` (même mécanique que l'ancien script :
/// éprouvée avec Gmail, aucune dépendance TLS côté Rust).
pub async fn send_plain(
    smtp: &Smtp,
    to_name: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    let message = format!(
        "From: {from_name} <{from}>\r\nTo: {to_name} <{to}>\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body}",
        from_name = smtp.from_name,
        from = smtp.from,
    );
    let url = format!("smtps://{}:{}", smtp.host, smtp.port);
    let userpass = format!("{}:{}", smtp.user, smtp.pass.expose());
    let mut child = Command::new("curl")
        .args([
            "-fsS",
            "--url",
            &url,
            "--ssl-reqd",
            "--user",
            &userpass,
            "--mail-from",
            &smtp.from,
            "--mail-rcpt",
            to,
            "--upload-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("lancement de curl")?;
    let mut stdin = child.stdin.take().context("stdin curl")?;
    stdin.write_all(message.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!(
            "curl smtp {} : {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
