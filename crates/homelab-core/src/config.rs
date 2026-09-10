use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::secret::Secret;

/// Configuration non secrète, chargée depuis `homelab.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub paths: Paths,
    pub urls: Urls,
    #[serde(default)]
    pub web: Web,
    #[serde(default)]
    pub tasks: Tasks,
    #[serde(default)]
    pub auto_import: AutoImport,
    #[serde(default)]
    pub cleanup: Cleanup,
    #[serde(default)]
    pub backup: Backup,
    #[serde(default)]
    pub vpn: Vpn,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    /// Racine du homelab (compose, .env, dossiers d'état).
    pub base: PathBuf,
    /// Dossier de téléchargement côté hôte (ce que surveille auto_import).
    pub downloads: PathBuf,
    /// Le même dossier tel que monté dans Sonarr/Radarr/qBittorrent.
    #[serde(default = "d_downloads_in_container")]
    pub downloads_in_container: String,
    #[serde(default = "d_state_file")]
    pub state_file: PathBuf,
    #[serde(default = "d_backups")]
    pub backups: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Urls {
    pub sonarr: String,
    pub radarr: String,
    pub prowlarr: String,
    pub qbittorrent: String,
    pub jellyfin: String,
    pub jellyseerr: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Web {
    pub listen: String,
    pub rate_limit_secs: u64,
}

impl Default for Web {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8765".into(),
            rate_limit_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Tasks {
    /// Noms de tâches à ne pas planifier (restent invocables via `homelabctl run`).
    pub disabled: Vec<String>,
    pub tracker_ratio: TrackerRatio,
    pub stuck_handler: StuckHandler,
    pub disk_pressure: DiskPressure,
    pub tba_bypass: Interval300,
    pub monitor_sync: Interval600,
    pub user_poller: UserPoller,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TrackerRatio {
    pub interval_secs: u64,
    /// Trackers privés prioritaires : seed illimité.
    pub unlimited: Vec<String>,
    /// Trackers privés secondaires + publics : ratio/temps "public".
    pub secondary: Vec<String>,
    pub public: Vec<String>,
    pub public_ratio: f64,
    pub public_time_min: i64,
    pub default_ratio: f64,
    pub default_time_min: i64,
}

impl Default for TrackerRatio {
    fn default() -> Self {
        Self {
            interval_secs: 1800,
            unlimited: vec!["c411.org".into()],
            secondary: vec!["yggleak".into(), "u2p".into(), "ygg.gratis".into()],
            public: [
                "opentrackr",
                "demonii",
                "exodus.desync",
                "open.stealth",
                "tracker.torrent.eu",
                "tracker.theoks",
                "explodie.org",
                "leet-tracker",
                "tracker.dler",
                "tracker.filemail",
                "tracker.opentrackr",
                "tracker.alaskantf",
                "tracker-udp.gbitt",
                "overflow.biz",
                "open.dstud",
                "tracker.srv00",
                "tracker1.myporn",
                "durukanbal",
                "encrypt.net",
                "corpscorp",
                "6ahddutb",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            public_ratio: 2.0,
            public_time_min: 20160,
            default_ratio: 1.0,
            default_time_min: 10080,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StuckHandler {
    pub interval_secs: u64,
    /// Durée pendant laquelle un item doit rester bloqué avant remplacement.
    pub stall_secs: i64,
    pub max_actions_per_run: usize,
    /// Regex (insensible à la casse) sur `errorMessage` de la queue Arr.
    pub pattern: String,
}

impl Default for StuckHandler {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            stall_secs: 8 * 3600,
            max_actions_per_run: 5,
            pattern: "stalled|metadata|no connections".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DiskPressure {
    pub interval_secs: u64,
    pub hard_pct: u8,
    pub crit_pct: u8,
    pub max_actions_per_run: usize,
}

impl Default for DiskPressure {
    fn default() -> Self {
        Self {
            interval_secs: 900,
            hard_pct: 95,
            crit_pct: 98,
            max_actions_per_run: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval300 {
    pub interval_secs: u64,
}
impl Default for Interval300 {
    fn default() -> Self {
        Self { interval_secs: 300 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval600 {
    pub interval_secs: u64,
}
impl Default for Interval600 {
    fn default() -> Self {
        Self { interval_secs: 600 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UserPoller {
    pub interval_secs: u64,
    /// Ne considère que les users Jellyseerr créés depuis moins de N secondes.
    pub max_age_secs: i64,
    pub admin_user_id: i64,
}

impl Default for UserPoller {
    fn default() -> Self {
        Self {
            interval_secs: 60,
            max_age_secs: 600,
            admin_user_id: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AutoImport {
    pub enabled: bool,
    /// Secondes d'attente après détection d'une vidéo avant scan.
    pub settle_secs: u64,
    /// Nombre de relevés de taille identiques consécutifs pour une archive.
    pub archive_stable_checks: u32,
    pub archive_max_wait_secs: u64,
    pub radarr_root: String,
    pub sonarr_root: String,
}

impl Default for AutoImport {
    fn default() -> Self {
        Self {
            enabled: true,
            settle_secs: 5,
            archive_stable_checks: 3,
            archive_max_wait_secs: 1800,
            radarr_root: "/movies".into(),
            sonarr_root: "/tv".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Cleanup {
    pub interval_secs: u64,
    pub transcodes_dir: PathBuf,
    pub transcodes_max_age_days: u64,
    pub empty_download_dirs_max_age_days: u64,
    pub recycle_dirs: Vec<PathBuf>,
    pub recycle_max_age_days: u64,
    pub jellyfin_log_dir: PathBuf,
    pub jellyfin_log_max_age_days: u64,
}

impl Default for Cleanup {
    fn default() -> Self {
        Self {
            interval_secs: 86400,
            transcodes_dir: "/opt/homelab/jellyfin/cache/transcodes".into(),
            transcodes_max_age_days: 1,
            empty_download_dirs_max_age_days: 2,
            recycle_dirs: vec![
                "/opt/homelab/library/media/movies/.recycle".into(),
                "/opt/homelab/library/media/tvshows/.recycle".into(),
            ],
            recycle_max_age_days: 14,
            jellyfin_log_dir: "/opt/homelab/jellyfin/config/log".into(),
            jellyfin_log_max_age_days: 7,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Backup {
    /// Sous-chemins de `paths.base` exclus de l'archive d'état.
    pub excludes: Vec<String>,
    pub keep_last: usize,
    pub mysql_container: String,
}

impl Default for Backup {
    fn default() -> Self {
        Self {
            excludes: [
                "library",
                "backups",
                "influxdb",
                "jellyfin/cache",
                "jellyfin/config/log",
                "target",
                "logs",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            keep_last: 4,
            mysql_container: "guacdb".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Vpn {
    pub qbit_conf: PathBuf,
    /// server_name NPM du proxy host qBittorrent (pour repointer gluetun ↔ direct).
    pub npm_server_name: String,
}

impl Default for Vpn {
    fn default() -> Self {
        Self {
            qbit_conf: "/opt/homelab/qbittorrent/config/qBittorrent/qBittorrent.conf".into(),
            npm_server_name: "qbittorrent.".into(),
        }
    }
}

fn d_downloads_in_container() -> String {
    "/downloads".into()
}
fn d_state_file() -> PathBuf {
    "/opt/homelab/state/homelabd.json".into()
}
fn d_backups() -> PathBuf {
    "/opt/homelab/backups".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Refuse de démarrer sur une config incohérente : mieux qu'un exit 0 silencieux.
    pub fn validate(&self) -> Result<()> {
        if !self.paths.base.is_dir() {
            bail!("paths.base n'existe pas : {}", self.paths.base.display());
        }
        if self.auto_import.enabled && !self.paths.downloads.is_dir() {
            bail!(
                "paths.downloads n'existe pas : {} (auto_import.enabled=true)",
                self.paths.downloads.display()
            );
        }
        if self.tasks.disk_pressure.hard_pct >= self.tasks.disk_pressure.crit_pct {
            bail!("tasks.disk_pressure : hard_pct doit être < crit_pct");
        }
        for url in [
            &self.urls.sonarr,
            &self.urls.radarr,
            &self.urls.prowlarr,
            &self.urls.qbittorrent,
            &self.urls.jellyfin,
            &self.urls.jellyseerr,
        ] {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                bail!("URL invalide dans [urls] : {url}");
            }
        }
        Ok(())
    }

    pub fn task_enabled(&self, name: &str) -> bool {
        !self.tasks.disabled.iter().any(|d| d == name)
    }
}

/// Secrets et valeurs spécifiques à l'hôte, lus depuis l'environnement (`.env`).
#[derive(Debug, Clone)]
pub struct Secrets {
    pub sonarr_api_key: Secret,
    pub radarr_api_key: Secret,
    pub prowlarr_api_key: Option<Secret>,
    pub jellyfin_api_key: Secret,
    pub jellyseerr_api_key: Secret,
    pub jellyfin_lib_films: String,
    pub jellyfin_lib_series: String,
    pub jellyfin_public_url: String,
    pub jellyseerr_public_url: String,
    pub quality_profile_id: i64,
    pub onboard_token: Option<Secret>,
    pub smtp: Option<Smtp>,
}

#[derive(Debug, Clone)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: Secret,
    pub from: String,
    pub from_name: String,
}

impl Secrets {
    /// Charge `.env` (si présent) puis lit les variables. Les variables déjà
    /// exportées (EnvironmentFile systemd) ont priorité sur le fichier.
    pub fn load(env_file: Option<&Path>) -> Result<Self> {
        if let Some(p) = env_file {
            if p.is_file() {
                dotenvy::from_path(p).with_context(|| format!("lecture de {}", p.display()))?;
            }
        }
        Ok(Self {
            sonarr_api_key: req_secret("SONARR_API_KEY")?,
            radarr_api_key: req_secret("RADARR_API_KEY")?,
            prowlarr_api_key: opt("PROWLARR_API_KEY").map(Secret::new),
            jellyfin_api_key: req_secret("JELLYFIN_API_KEY")?,
            jellyseerr_api_key: req_secret("JELLYSEERR_API_KEY")?,
            jellyfin_lib_films: req("JELLYFIN_LIB_FILMS")?,
            jellyfin_lib_series: req("JELLYFIN_LIB_SERIES")?,
            jellyfin_public_url: req("JELLYFIN_PUBLIC_URL")?,
            jellyseerr_public_url: req("JELLYSEERR_PUBLIC_URL")?,
            quality_profile_id: opt("QUALITY_PROFILE_ID")
                .map(|v| v.parse::<i64>().context("QUALITY_PROFILE_ID non numérique"))
                .transpose()?
                .unwrap_or(6),
            onboard_token: opt("HOMELABD_ONBOARD_TOKEN").map(Secret::new),
            smtp: match (opt("SMTP_HOST"), opt("SMTP_USER"), opt("SMTP_PASS")) {
                (Some(host), Some(user), Some(pass)) => Some(Smtp {
                    port: opt("SMTP_PORT").and_then(|p| p.parse().ok()).unwrap_or(465),
                    from: opt("SMTP_FROM").unwrap_or_else(|| user.clone()),
                    from_name: opt("SMTP_FROM_NAME").unwrap_or_else(|| "Homelab".into()),
                    host,
                    user,
                    pass: Secret::new(pass),
                }),
                _ => None,
            },
        })
    }
}

fn opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn req(key: &str) -> Result<String> {
    opt(key).with_context(|| format!("variable d'environnement manquante : {key}"))
}

fn req_secret(key: &str) -> Result<Secret> {
    req(key).map(Secret::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_toml_parses_with_defaults() {
        let raw = r#"
[paths]
base = "/tmp"
downloads = "/tmp"
[urls]
sonarr = "http://s"
radarr = "http://r"
prowlarr = "http://p"
qbittorrent = "http://q"
jellyfin = "http://j"
jellyseerr = "http://js"
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.tasks.stuck_handler.stall_secs, 28800);
        assert_eq!(cfg.tasks.tracker_ratio.public_time_min, 20160);
        assert!(cfg.task_enabled("tba_bypass"));
    }

    #[test]
    fn rejects_unknown_keys() {
        let raw = r#"
[paths]
base = "/tmp"
downloads = "/tmp"
typo = 1
[urls]
sonarr = "http://s"
radarr = "http://r"
prowlarr = "http://p"
qbittorrent = "http://q"
jellyfin = "http://j"
jellyseerr = "http://js"
"#;
        assert!(toml::from_str::<Config>(raw).is_err());
    }
}
