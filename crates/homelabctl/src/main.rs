//! `homelabctl` — opérations manuelles : lancer une tâche, onboarder un user,
//! basculer le VPN, sauvegarder, vérifier la config et la connectivité.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use homelab_core::tasks::{self, backup, onboard, vpn};
use homelab_core::{Config, Secret, Secrets, TaskContext};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(version, about = "Homelab control CLI")]
struct Args {
    #[arg(long, global = true, default_value = "/opt/homelab/homelab.toml")]
    config: PathBuf,
    #[arg(long, global = true, default_value = "/opt/homelab/.env")]
    env_file: PathBuf,
    /// Aucune écriture, on affiche ce qui serait fait.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Exécute une tâche une fois (voir `list`)
    Run { task: String },
    /// Liste les tâches planifiables
    List,
    /// Crée un compte Jellyfin + Jellyseerr et envoie le mail de bienvenue
    Onboard {
        username: String,
        email: String,
        #[arg(long)]
        password: Option<String>,
    },
    /// qBittorrent derrière gluetun (on) ou en direct (off)
    Vpn {
        #[arg(value_parser = ["on", "off", "status"])]
        mode: String,
    },
    /// Sauvegarde de l'état dans paths.backups (à lancer avec sudo)
    Backup,
    /// Dernier état des tâches du daemon
    Status,
    /// Valide config + secrets et teste chaque service
    Check,
    /// Installe les unités systemd depuis <base>/systemd (root)
    Install,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .without_time()
        .init();

    let cfg = Config::load(&args.config)?;
    if let Cmd::List = args.cmd {
        for n in tasks::names() {
            println!(
                "{n}{}",
                if cfg.task_enabled(n) {
                    ""
                } else {
                    "  (disabled)"
                }
            );
        }
        return Ok(());
    }
    if let Cmd::Install = args.cmd {
        return install(&cfg).await;
    }
    let secrets = Secrets::load(Some(&args.env_file)).context("chargement des secrets")?;
    let ctx = TaskContext::new(cfg, secrets, args.dry_run)?;

    match args.cmd {
        Cmd::Run { task } => {
            let t = tasks::find(&task)
                .with_context(|| format!("tâche inconnue : {task} (voir `homelabctl list`)"))?;
            let rep = t.run(&ctx).await?;
            println!("{}: {} (actions={})", t.name(), rep.summary, rep.actions);
        }
        Cmd::Onboard {
            username,
            email,
            password,
        } => {
            let r = onboard::run(
                &ctx,
                onboard::OnboardRequest {
                    username,
                    email,
                    password: password.map(Secret::new),
                },
            )
            .await?;
            if r.dry_run {
                println!(
                    "DRY-RUN : pré-contrôles OK pour {} <{}>",
                    r.username, r.email
                );
                return Ok(());
            }
            println!(
                "\n✓ Onboarding terminé pour {u}\n\n  Username      : {u}\n  Email         : {e}\n  Password      : {p}\n  Jellyfin Id   : {jf}\n  Jellyseerr id : {js}\n  Mail          : {m}\n\n  Streaming : {ju}\n  Requêtes  : {su}\n",
                u = r.username,
                e = r.email,
                p = r.password.expose(),
                jf = r.jellyfin_id,
                js = r.jellyseerr_id,
                m = if r.mail_sent { "envoyé" } else { "NON envoyé — transmettre à la main" },
                ju = r.jellyfin_url,
                su = r.jellyseerr_url
            );
        }
        Cmd::Vpn { mode } => {
            let st = match mode.as_str() {
                "on" => vpn::switch(&ctx, vpn::Mode::Vpn).await?,
                "off" => vpn::switch(&ctx, vpn::Mode::Direct).await?,
                _ => vpn::status(&ctx).await?,
            };
            match st.mode {
                vpn::Mode::Vpn => println!("VPN ON  — qBittorrent derrière gluetun"),
                vpn::Mode::Direct => println!("VPN OFF — qBittorrent direct"),
            }
            println!("IP publique qBittorrent : {}", st.public_ip);
        }
        Cmd::Backup => {
            if !nix_is_root() {
                bail!("backup doit tourner en root (sudo homelabctl backup) : npm/homarr/grafana ont leurs propres uid");
            }
            let out = backup::run(&ctx.cfg).await?;
            for f in out.files {
                println!("{}", f.display());
            }
        }
        Cmd::Status => {
            let runs = ctx.state.read(|s| s.task_runs.clone()).await;
            println!(
                "{:<16} {:<8} {:<20} {:<6} {:<6} summary",
                "task", "last_ok", "last_end", "runs", "errors"
            );
            for (name, r) in runs {
                let end = r
                    .last_end
                    .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| "running".into());
                println!(
                    "{:<16} {:<8} {:<20} {:<6} {:<6} {}",
                    name,
                    r.last_ok
                        .map(|b| if b { "ok" } else { "FAIL" })
                        .unwrap_or("-"),
                    end,
                    r.runs,
                    r.errors,
                    r.last_summary
                );
            }
            let stuck = ctx.state.read(|s| s.stuck.len()).await;
            println!(
                "\nstuck tracked: {stuck}  state: {}",
                ctx.state.path().display()
            );
        }
        Cmd::Check => {
            let mut ok = true;
            for (name, res) in [
                (
                    "sonarr",
                    ctx.sonarr.ping().await.map(|_| "pong".to_string()),
                ),
                (
                    "radarr",
                    ctx.radarr.ping().await.map(|_| "pong".to_string()),
                ),
                ("qbittorrent", ctx.qbit.version().await),
                (
                    "jellyfin",
                    ctx.jellyfin.system_info().await.map(|v| {
                        v.get("Version")
                            .and_then(|x| x.as_str())
                            .unwrap_or("?")
                            .to_string()
                    }),
                ),
                (
                    "jellyseerr",
                    ctx.jellyseerr.status().await.map(|v| {
                        v.get("version")
                            .and_then(|x| x.as_str())
                            .unwrap_or("?")
                            .to_string()
                    }),
                ),
            ] {
                match res {
                    Ok(v) => println!("✓ {name:<12} {v}"),
                    Err(e) => {
                        ok = false;
                        println!("✗ {name:<12} {e}");
                    }
                }
            }
            println!(
                "{} smtp {}",
                if ctx.secrets.smtp.is_some() {
                    "✓"
                } else {
                    "!"
                },
                if ctx.secrets.smtp.is_some() {
                    "configuré"
                } else {
                    "absent (pas de mail d'onboarding)"
                }
            );
            println!(
                "{} onboard token {}",
                if ctx.secrets.onboard_token.is_some() {
                    "✓"
                } else {
                    "!"
                },
                if ctx.secrets.onboard_token.is_some() {
                    "défini"
                } else {
                    "absent : POST /onboard sans auth"
                }
            );
            println!("✓ watch dir   {}", ctx.cfg.paths.downloads.display());
            if !ok {
                bail!("au moins un service injoignable");
            }
        }
        Cmd::List | Cmd::Install => unreachable!(),
    }
    Ok(())
}

fn nix_is_root() -> bool {
    std::fs::metadata("/proc/self")
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid() == 0
        })
        .unwrap_or(false)
}

async fn install(cfg: &Config) -> Result<()> {
    if !nix_is_root() {
        bail!("install doit tourner en root");
    }
    let src = cfg.paths.base.join("systemd");
    let mut names = Vec::new();
    for e in std::fs::read_dir(&src)
        .with_context(|| format!("lecture {}", src.display()))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".service") || name.ends_with(".timer") {
            std::fs::copy(e.path(), format!("/etc/systemd/system/{name}"))?;
            println!("installed /etc/systemd/system/{name}");
            names.push(name);
        }
    }
    homelab_core::docker::run("systemctl", &["daemon-reload"], None).await?;
    for n in &names {
        if n.ends_with(".timer") || n == "homelabd.service" || n == "homelab-stack.service" {
            homelab_core::docker::run("systemctl", &["enable", n], None).await?;
            println!("enabled {n}");
        }
    }
    println!("→ systemctl start homelabd.service (ou restart après mise à jour du binaire)");
    Ok(())
}
