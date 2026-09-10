//! Bascule qBittorrent derrière gluetun (profil compose `vpn`) ou en direct
//! (profil `novpn`, service `qbittorrent-direct`). Remplace `vpn-toggle.sh` qui
//! swappait des fichiers compose entiers. Étapes :
//!   1. COMPOSE_PROFILES dans .env
//!   2. IPv6 qBittorrent (conteneur arrêté avant d'éditer le .conf)
//!   3. stop/rm de l'autre variante, up de la cible
//!   4. repointage du download client Sonarr/Radarr (host gluetun ↔ qbittorrent-direct)
//!   5. proxy host NPM (server_name qbittorrent.*) + reload nginx

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::info;

use crate::context::TaskContext;
use crate::docker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Vpn,
    Direct,
}

impl Mode {
    pub fn profile(self) -> &'static str {
        match self {
            Mode::Vpn => "vpn",
            Mode::Direct => "novpn",
        }
    }
    pub fn service(self) -> &'static str {
        match self {
            Mode::Vpn => "qbittorrent",
            Mode::Direct => "qbittorrent-direct",
        }
    }
    /// Tous les services du profil (gluetun n'a de raison d'exister qu'en mode VPN).
    pub fn services(self) -> &'static [&'static str] {
        match self {
            Mode::Vpn => &["gluetun", "qbittorrent"],
            Mode::Direct => &["qbittorrent-direct"],
        }
    }
    /// Nom d'hôte que voient les Arrs et NPM.
    pub fn host(self) -> &'static str {
        match self {
            Mode::Vpn => "gluetun",
            Mode::Direct => "qbittorrent-direct",
        }
    }
    pub fn other(self) -> Mode {
        match self {
            Mode::Vpn => Mode::Direct,
            Mode::Direct => Mode::Vpn,
        }
    }
}

pub struct Status {
    pub mode: Mode,
    pub public_ip: String,
}

pub async fn status(ctx: &TaskContext) -> Result<Status> {
    let running_vpn = docker::docker(&[
        "inspect",
        "qbittorrent",
        "--format",
        "{{.State.Running}} {{.HostConfig.NetworkMode}}",
    ])
    .await
    .map(|s| s.starts_with("true") && s.contains("container:"))
    .unwrap_or(false);
    let mode = if running_vpn { Mode::Vpn } else { Mode::Direct };
    let public_ip = docker::exec_in(mode.service(), &["wget", "-qO-", "https://ifconfig.me/ip"])
        .await
        .unwrap_or_else(|_| "?".into());
    let _ = ctx;
    Ok(Status { mode, public_ip })
}

pub async fn switch(ctx: &TaskContext, target: Mode) -> Result<Status> {
    let base = &ctx.cfg.paths.base;
    if ctx.dry_run {
        info!(task = "vpn", ?target, "dry-run: would switch");
        return status(ctx).await;
    }
    set_profile(&base.join(".env"), target.profile())?;
    let other = target.other();
    let mut stop = vec!["--profile", other.profile(), "stop"];
    stop.extend_from_slice(other.services());
    let _ = docker::compose(base, &stop).await;
    let mut rm = vec!["--profile", other.profile(), "rm", "-f"];
    rm.extend_from_slice(other.services());
    let _ = docker::compose(base, &rm).await;
    let _ = docker::compose(
        base,
        &["--profile", target.profile(), "stop", target.service()],
    )
    .await;
    set_qbit_ipv6(&ctx.cfg.vpn.qbit_conf, target == Mode::Direct)?;
    let mut up = vec!["--profile", target.profile(), "up", "-d"];
    up.extend_from_slice(target.services());
    docker::compose(base, &up).await?;
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    for arr in [&ctx.sonarr, &ctx.radarr] {
        repoint(arr, target.host()).await?;
    }
    repoint_npm(
        &ctx.cfg.vpn.npm_server_name,
        target.other().host(),
        target.host(),
    )
    .await?;
    info!(task = "vpn", ?target, "switched");
    status(ctx).await
}

fn set_profile(env_path: &Path, profile: &str) -> Result<()> {
    let raw = std::fs::read_to_string(env_path)
        .with_context(|| format!("lecture {}", env_path.display()))?;
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    let line = format!("COMPOSE_PROFILES={profile}");
    match lines
        .iter_mut()
        .find(|l| l.starts_with("COMPOSE_PROFILES="))
    {
        Some(l) => *l = line,
        None => lines.push(line),
    }
    std::fs::write(env_path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Sous VPN, tun0 est IPv4-only : IPv6 doit être coupé sinon les annonces échouent.
fn set_qbit_ipv6(conf: &Path, enabled: bool) -> Result<()> {
    let raw =
        std::fs::read_to_string(conf).with_context(|| format!("lecture {}", conf.display()))?;
    let val = format!("Session\\IPv6Enabled={}", enabled);
    let mut out = Vec::new();
    let mut done = false;
    for l in raw.lines() {
        if l.starts_with("Session\\IPv6Enabled=") {
            out.push(val.clone());
            done = true;
        } else {
            out.push(l.to_string());
            if !done && l == "[BitTorrent]" {
                out.push(val.clone());
                done = true;
            }
        }
    }
    if !done {
        out.push("[BitTorrent]".into());
        out.push(val);
    }
    std::fs::write(conf, out.join("\n") + "\n")?;
    Ok(())
}

async fn repoint(arr: &crate::clients::ArrClient, host: &str) -> Result<()> {
    let clients = arr.download_clients().await?;
    let Some(mut client) = clients
        .into_iter()
        .find(|c| c.get("implementation").and_then(Value::as_str) == Some("QBittorrent"))
    else {
        anyhow::bail!("{}: aucun download client QBittorrent", arr.name);
    };
    let id = client
        .get("id")
        .and_then(Value::as_i64)
        .context("download client sans id")?;
    if let Some(fields) = client.get_mut("fields").and_then(Value::as_array_mut) {
        for f in fields.iter_mut() {
            if f.get("name").and_then(Value::as_str) == Some("host") {
                f["value"] = Value::String(host.into());
            }
        }
    }
    arr.put_download_client(id, &client).await?;
    info!(
        task = "vpn",
        service = arr.name,
        host,
        "download client repointed"
    );
    Ok(())
}

async fn repoint_npm(server_name_prefix: &str, from: &str, to: &str) -> Result<()> {
    let script = format!(
        "f=$(grep -l 'server_name {server_name_prefix}' /data/nginx/proxy_host/*.conf | head -1); \
         [ -n \"$f\" ] || exit 0; sed -i 's/set \\$server *\"{from}\";/set $server         \"{to}\";/' \"$f\" && nginx -s reload"
    );
    docker::exec_in("npm", &["sh", "-c", &script]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_line_is_replaced_or_appended() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(&p, "A=1\nCOMPOSE_PROFILES=vpn\n").unwrap();
        set_profile(&p, "novpn").unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "A=1\nCOMPOSE_PROFILES=novpn\n"
        );
        std::fs::write(&p, "A=1\n").unwrap();
        set_profile(&p, "vpn").unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "A=1\nCOMPOSE_PROFILES=vpn\n"
        );
    }

    #[test]
    fn ipv6_flag_inserted_under_bittorrent_section() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("q.conf");
        std::fs::write(&p, "[BitTorrent]\nSession\\Port=1\n").unwrap();
        set_qbit_ipv6(&p, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "[BitTorrent]\nSession\\IPv6Enabled=false\nSession\\Port=1\n"
        );
        set_qbit_ipv6(&p, true).unwrap();
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("Session\\IPv6Enabled=true"));
    }
}
