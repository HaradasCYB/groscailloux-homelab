//! `homelabctl` — opérations manuelles : lancer une tâche, onboarder un user,
//! basculer le VPN, sauvegarder, vérifier la config et la connectivité.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use homelab_core::accounts::{self, Outcome};
use homelab_core::tasks::{self, backup, vpn};
use homelab_core::{Config, Secrets, TaskContext};
use serde_json::{json, Value};
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
        /// Imposer un mot de passe (sinon le membre le définit via le lien de bienvenue)
        #[arg(long)]
        password: Option<String>,
    },
    /// Discord : apply (configure les 4 Arrs et Jellyseerr d'après .env) ; test (message d'essai) ; remove
    Discord {
        #[arg(value_parser = ["apply", "test", "remove"])]
        action: String,
    },
    /// Envoie le mail de bienvenue en exemple (lien de démonstration, aucun compte touché)
    MailTest {
        /// Adresse destinataire
        to: String,
    },
    /// Comptes Jellyfin : list ; on|off <compte> (premium ou suspendu) ; delete <compte> --yes ;
    /// limits (applique `max_devices_per_user`, appareils connectés par compte ; 0 = illimité) ;
    /// link <compte> (renvoie un lien de bienvenue : définir ou changer son mot de passe)
    Accounts {
        #[arg(value_parser = ["list", "on", "off", "delete", "limits", "link"])]
        action: String,
        /// Nom ou id Jellyfin (pour on/off/delete)
        who: Option<String>,
        /// Confirme une suppression (définitive : Jellyfin + Jellyseerr)
        #[arg(long)]
        yes: bool,
    },
    /// Abonnés : list ; set <compte> --status active|offered|exempt|unknown|suspended [--days N] ;
    /// extend <compte> --days N ; link <compte> --sub I-… ; import <csv PayPal> ; paypal (webhook, plan)
    Subs {
        #[arg(value_parser = ["list", "set", "extend", "link", "import", "paypal"])]
        action: String,
        /// Compte Jellyfin (set/extend/link) ou fichier CSV (import)
        who: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        days: Option<i64>,
        /// Identifiant d'abonnement PayPal (I-…)
        #[arg(long)]
        sub: Option<String>,
        /// paypal : crée le webhook sur cette URL (https://…/paypal/webhook)
        #[arg(long)]
        webhook: Option<String>,
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
            // le lien de bienvenue vit dans l'état du daemon : tout passe par son API (jeton)
            let v = daemon_post(
                &ctx,
                "/onboard",
                json!({ "username": username, "email": email, "password": password }),
            )
            .await?;
            if v.get("dry_run").and_then(Value::as_bool) == Some(true) {
                println!("DRY-RUN : pré-contrôles OK pour {username} <{email}>");
                return Ok(());
            }
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("—").to_string();
            let mail_sent = v.get("mail_sent").and_then(Value::as_bool).unwrap_or(false);
            println!(
                "\n✓ Onboarding terminé pour {u}\n\n  Username      : {u}\n  Email         : {e}\n  Mot de passe  : {p}\n  Jellyfin Id   : {jf}\n  Jellyseerr id : {js}\n  Mail          : {m}\n  Lien          : {l}\n  Premium       : {pr}\n\n  Streaming : {ju}\n  Requêtes  : {su}\n",
                u = s("username"),
                e = s("email"),
                p = v.get("password").and_then(Value::as_str).unwrap_or("à définir par le membre (lien)"),
                jf = s("jellyfin_id"),
                js = v.get("jellyseerr_id").and_then(Value::as_i64).unwrap_or(0),
                m = if mail_sent { "envoyé" } else { "NON envoyé — transmettre le lien à la main" },
                l = s("link_url"),
                pr = if v.get("premium").and_then(Value::as_bool).unwrap_or(false) { "oui" } else { "non — à activer (homelabctl accounts on <compte>)" },
                ju = s("jellyfin_url"),
                su = s("jellyseerr_url"),
            );
        }
        Cmd::Discord { action } => {
            let r = match action.as_str() {
                "apply" => homelab_core::discord::apply(&ctx).await?,
                "remove" => homelab_core::discord::remove(&ctx).await?,
                _ => homelab_core::discord::test(&ctx).await?,
            };
            let dry = if ctx.dry_run { "DRY-RUN : " } else { "" };
            for l in r.lines {
                println!("{dry}{l}");
            }
        }
        Cmd::MailTest { to } => {
            let v = daemon_post(&ctx, "/admin/mail-test", json!({ "to": to })).await?;
            println!(
                "mail de bienvenue (démo) {} → {to}\n  lien : {}",
                if v.get("mail_sent").and_then(Value::as_bool).unwrap_or(false) {
                    "envoyé"
                } else {
                    "NON envoyé (SMTP ?)"
                },
                v.get("url").and_then(Value::as_str).unwrap_or("—")
            );
        }
        Cmd::Subs {
            action,
            who,
            status,
            days,
            sub,
            webhook,
        } => {
            subs_cmd(&ctx, &action, who, status, days, sub, webhook).await?;
        }
        Cmd::Accounts { action, who, yes } => match action.as_str() {
            "link" => {
                let who = who.context("préciser le compte : homelabctl accounts link <compte>")?;
                let a = accounts::resolve(&ctx, &who).await?;
                let v = daemon_post(&ctx, "/admin/link", json!({ "user_id": a.id })).await?;
                println!(
                    "lien de bienvenue pour {} {}\n  lien : {}",
                    a.name,
                    if v.get("mail_sent").and_then(Value::as_bool).unwrap_or(false) {
                        "envoyé"
                    } else {
                        "NON envoyé — transmettre à la main"
                    },
                    v.get("url").and_then(Value::as_str).unwrap_or("—")
                );
            }
            "limits" => {
                let changed = accounts::apply_device_limit(&ctx).await?;
                println!(
                    "{}{} compte(s) → appareils connectés au plus : {} (0 = illimité) : {}",
                    if ctx.dry_run { "DRY-RUN : " } else { "" },
                    changed.len(),
                    ctx.cfg.accounts.max_devices_per_user,
                    changed.join(", ")
                );
            }
            "delete" => {
                let who =
                    who.context("préciser le compte : homelabctl accounts delete <compte> --yes")?;
                let a = accounts::resolve(&ctx, &who).await?;
                if !yes && !ctx.dry_run {
                    bail!("suppression définitive de {} (Jellyfin + Jellyseerr) : relancer avec --yes", a.name);
                }
                let d = accounts::delete(&ctx, &a.id).await?;
                println!(
                    "{}{} supprimé{}",
                    if ctx.dry_run { "DRY-RUN : " } else { "" },
                    d.name,
                    if d.jellyseerr {
                        " (Jellyfin + Jellyseerr)"
                    } else {
                        " (Jellyfin)"
                    }
                );
            }
            on_off => {
                let who =
                    who.context("préciser le compte : homelabctl accounts on|off <compte>")?;
                let a = accounts::resolve(&ctx, &who).await?;
                let dry = if ctx.dry_run { "DRY-RUN : " } else { "" };
                match accounts::set_premium(&ctx, &a.id, on_off == "on").await? {
                    Outcome::Activated => println!("{dry}{} activé (premium)", a.name),
                    Outcome::Suspended => println!("{dry}{} suspendu", a.name),
                    Outcome::Unchanged => println!("{} déjà dans cet état", a.name),
                    Outcome::CapReached { premium, max } => {
                        bail!("plafond atteint ({premium}/{max} premium) : suspendre un compte ou relever accounts.max_premium")
                    }
                }
            }
        },
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
            let sb = &ctx.cfg.seedbox;
            if sb.enabled {
                for arr in [&ctx.seedbox_sonarr, &ctx.seedbox_radarr]
                    .into_iter()
                    .flatten()
                {
                    match arr.get("api/v3/system/status", &[]).await {
                        Ok(v) => println!(
                            "✓ {:<14} {}",
                            arr.name,
                            v.get("version").and_then(|x| x.as_str()).unwrap_or("?")
                        ),
                        Err(e) => {
                            ok = false;
                            println!("✗ {:<14} {e}", arr.name);
                        }
                    }
                }
                if let Some(q) = &ctx.seedbox_qbit {
                    match q.version().await {
                        Ok(v) => println!("✓ {:<14} {v}", "qbit-seedbox"),
                        Err(e) => {
                            ok = false;
                            println!("✗ {:<14} {e}", "qbit-seedbox");
                        }
                    }
                }
                let mounted = std::fs::read_dir(&sb.mount_point)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
                if mounted {
                    println!("✓ seedbox mount {}", sb.mount_point.display());
                } else {
                    ok = false;
                    println!(
                        "✗ seedbox mount {} vide ou absent (systemctl status homelab-seedbox-mount)",
                        sb.mount_point.display()
                    );
                }
            }
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
        let seedbox_mount = n == "homelab-seedbox-mount.service" && cfg.seedbox.enabled;
        if n.ends_with(".timer")
            || n == "homelabd.service"
            || n == "homelab-stack.service"
            || seedbox_mount
        {
            homelab_core::docker::run("systemctl", &["enable", n], None).await?;
            println!("enabled {n}");
        }
    }
    println!("→ systemctl start homelabd.service (ou restart après mise à jour du binaire)");
    Ok(())
}

/// Appel de l'API locale de homelabd (jeton `HOMELABD_ONBOARD_TOKEN`) : l'état des liens de bienvenue
/// appartient au daemon, la CLI ne l'écrit jamais elle-même (un lien émis ici serait invisible du daemon
/// et écrasé à sa prochaine sauvegarde).
async fn daemon_post(ctx: &TaskContext, path: &str, body: Value) -> Result<Value> {
    let token = ctx
        .secrets
        .onboard_token
        .as_ref()
        .context("HOMELABD_ONBOARD_TOKEN manquant dans .env : requis pour parler à homelabd")?;
    let port = ctx.cfg.web.listen.rsplit(':').next().unwrap_or("8766");
    let url = format!("http://127.0.0.1:{port}{path}");
    let resp = ctx
        .http
        .post(&url)
        .header("x-onboard-token", token.expose())
        .json(&body)
        .send()
        .await
        .with_context(|| format!("homelabd injoignable sur {url} (systemctl status homelabd)"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() || v.get("success").and_then(Value::as_bool) == Some(false) {
        bail!(
            "homelabd {path} → {status} : {}",
            v.get("error").and_then(Value::as_str).unwrap_or("?")
        );
    }
    Ok(v)
}

async fn subs_cmd(
    ctx: &homelab_core::TaskContext,
    action: &str,
    who: Option<String>,
    status: Option<String>,
    days: Option<i64>,
    sub: Option<String>,
    webhook: Option<String>,
) -> Result<()> {
    use homelab_core::subscription_ops as ops;
    use homelab_core::subscriptions::{self as subs, Status};
    let t = homelab_core::state::now();
    let resolve = |who: Option<String>| async move {
        let who = who.context("préciser le compte")?;
        ops::resolve_account(ctx, &who)
            .await?
            .with_context(|| format!("compte inconnu : {who}"))
    };
    match action {
        "list" => {
            let created = ops::ensure_fiches(ctx).await?;
            if created > 0 {
                println!("{created} fiche(s) créée(s) pour des comptes sans fiche");
            }
            println!(
                "{:<18} {:<18} {:<12} {:<8} paypal",
                "compte", "statut", "échéance", "source"
            );
            for s in ctx.subs.list()? {
                println!(
                    "{:<18} {:<18} {:<12} {:<8} {}",
                    s.username,
                    s.status.label(),
                    s.expires_at
                        .map(ops::date_text)
                        .unwrap_or_else(|| "—".into()),
                    s.source,
                    s.paypal_sub_id.as_deref().unwrap_or("—")
                );
            }
        }
        "set" => {
            let a = resolve(who).await?;
            let st = status
                .as_deref()
                .and_then(Status::parse)
                .context("--status active|trial|offered|exempt|unknown|suspended")?;
            let s = ops::admin_set(ctx, &a.user_id, st, days, "homelabctl").await?;
            println!(
                "{} : {}{}",
                s.username,
                s.status.label(),
                s.expires_at
                    .map(|e| format!(" jusqu'au {}", ops::date_text(e)))
                    .unwrap_or_default()
            );
        }
        "extend" => {
            let a = resolve(who).await?;
            let d = days.context("--days N")?;
            let new = ops::admin_extend(ctx, &a.user_id, d, "homelabctl").await?;
            println!("{} : prolongé jusqu'au {}", a.username, ops::date_text(new));
        }
        "link" => {
            let a = resolve(who).await?;
            let sid = sub.context("--sub I-…")?;
            if !subs::valid_paypal_sub_id(&sid) {
                bail!("identifiant d'abonnement invalide : {sid}");
            }
            let pp = ctx
                .paypal
                .as_ref()
                .context("PayPal non configuré (PAYPAL_* dans .env)")?;
            let v = pp.subscription(&sid).await?;
            let facts = homelab_core::clients::paypal::event_facts(
                &json!({ "id": format!("link:{sid}"), "event_type": "BILLING.SUBSCRIPTION.ACTIVATED", "resource": v }),
            );
            let mut facts = facts;
            facts.custom_id = Some(a.username.clone());
            ctx.subs
                .link_paypal(&a.user_id, &sid, facts.email.as_deref(), "homelabctl", t)?;
            let st = facts.status.clone().unwrap_or_default();
            if st == "ACTIVE" {
                let _ = ctx.subs.record_paypal_event(
                    &facts.event_id,
                    "LINK",
                    Some(&sid),
                    "homelabctl",
                    t,
                );
                ops::on_payment(ctx, &facts, "homelabctl").await?;
            }
            let s = ctx.subs.get(&a.user_id)?.context("fiche")?;
            println!(
                "{} ↔ {sid} (PayPal : {st}) → {}{}",
                s.username,
                s.status.label(),
                s.expires_at
                    .map(|e| format!(" jusqu'au {}", ops::date_text(e)))
                    .unwrap_or_default()
            );
        }
        "import" => {
            let path = who.context("préciser le fichier CSV exporté de PayPal")?;
            let text = std::fs::read_to_string(&path).with_context(|| format!("lecture {path}"))?;
            let rows = subs::parse_paypal_csv(&text);
            println!("{} abonnement(s) dans le fichier", rows.len());
            ops::ensure_fiches(ctx).await?;
            let all = ctx.subs.list()?;
            let mut linked = 0;
            for r in &rows {
                if ctx.subs.by_paypal_sub(&r.sub_id)?.is_some() {
                    continue;
                }
                // rapprochement : e-mail Jellyseerr du compte, sinon nom PayPal ≈ nom de compte
                let mut target: Option<String> = None;
                if r.email.contains('@') {
                    if let Ok(Some((id, _))) =
                        homelab_core::accounts::account_by_email(ctx, &r.email).await
                    {
                        target = Some(id);
                    }
                }
                if target.is_none() && !r.name.is_empty() {
                    let n = r.name.to_lowercase();
                    target = all
                        .iter()
                        .find(|s| n.contains(&s.username.to_lowercase()))
                        .map(|s| s.user_id.clone());
                }
                match target {
                    Some(uid) => {
                        ctx.subs.link_paypal(&uid, &r.sub_id, Some(&r.email), "import", t)?;
                        let name = all.iter().find(|s| s.user_id == uid).map(|s| s.username.clone()).unwrap_or(uid.clone());
                        println!("  {} ↔ {} ({})", name, r.sub_id, r.status);
                        linked += 1;
                    }
                    None => println!("  ? {} — {} {} : aucun compte trouvé, à rattacher à la main (homelabctl subs link <compte> --sub {})", r.sub_id, r.name, r.email, r.sub_id),
                }
            }
            println!("{linked} rattaché(s) ; lancer `homelabctl run subscription_reconcile` pour poser les échéances");
        }
        "paypal" => {
            let pp = ctx
                .paypal
                .as_ref()
                .context("PayPal non configuré (PAYPAL_* dans .env)")?;
            println!(
                "environnement : {}",
                if pp.cfg().sandbox { "sandbox" } else { "live" }
            );
            match pp.plan(&pp.cfg().plan_id).await {
                Ok(v) => println!(
                    "plan {} : {} ({})",
                    pp.cfg().plan_id,
                    v.get("name").and_then(Value::as_str).unwrap_or("?"),
                    v.get("status").and_then(Value::as_str).unwrap_or("?")
                ),
                Err(e) => println!("plan {} : {e}", pp.cfg().plan_id),
            }
            for w in pp.webhooks().await? {
                println!(
                    "webhook {} → {} ({} événements)",
                    w.get("id").and_then(Value::as_str).unwrap_or("?"),
                    w.get("url").and_then(Value::as_str).unwrap_or("?"),
                    w.get("event_types")
                        .and_then(Value::as_array)
                        .map(|a| a.len())
                        .unwrap_or(0)
                );
            }
            if let Some(url) = webhook {
                let id = pp.ensure_webhook(&url).await?;
                println!(
                    "webhook prêt : {id}
→ mettre PAYPAL_WEBHOOK_ID={id} (ou PAYPAL_SANDBOX_WEBHOOK_ID) dans .env puis restart homelabd"
                );
            }
        }
        _ => bail!("action inconnue"),
    }
    Ok(())
}
