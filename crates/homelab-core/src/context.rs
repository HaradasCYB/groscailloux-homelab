use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use crate::clients::{ArrClient, JellyfinClient, JellyseerrClient, QbitClient};
use crate::config::{Config, Secrets};
use crate::state::StateStore;

/// Tout ce dont une tâche a besoin. Cloné par `Arc` entre les boucles du scheduler.
pub struct TaskContext {
    pub cfg: Arc<Config>,
    pub secrets: Arc<Secrets>,
    pub http: reqwest::Client,
    pub sonarr: ArrClient,
    pub radarr: ArrClient,
    /// Présents seulement si `[seedbox] enabled = true`.
    pub seedbox_sonarr: Option<ArrClient>,
    pub seedbox_radarr: Option<ArrClient>,
    pub qbit: QbitClient,
    pub jellyfin: JellyfinClient,
    pub jellyseerr: JellyseerrClient,
    /// Sérialise toute mutation de qBittorrent (stuck, disk pressure, ratio).
    pub qbit_lock: Arc<Mutex<()>>,
    /// Sérialise les onboardings (poller + web).
    pub onboard_lock: Arc<Mutex<()>>,
    pub state: StateStore,
    /// Aucune écriture nulle part : les tâches loggent ce qu'elles auraient fait.
    pub dry_run: bool,
}

impl TaskContext {
    pub fn new(cfg: Config, secrets: Secrets, dry_run: bool) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("homelabd/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let state = StateStore::load(&cfg.paths.state_file)?;
        let (seedbox_sonarr, seedbox_radarr) = if cfg.seedbox.enabled {
            let s = secrets
                .seedbox_sonarr_api_key
                .clone()
                .context("[seedbox] activé mais SEEDBOX_SONARR_API_KEY absent")?;
            let r = secrets
                .seedbox_radarr_api_key
                .clone()
                .context("[seedbox] activé mais SEEDBOX_RADARR_API_KEY absent")?;
            (
                Some(ArrClient::new(
                    "sonarr-seedbox",
                    &with_slash(&cfg.seedbox.sonarr_url),
                    s,
                    http.clone(),
                )?),
                Some(ArrClient::new(
                    "radarr-seedbox",
                    &with_slash(&cfg.seedbox.radarr_url),
                    r,
                    http.clone(),
                )?),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            seedbox_sonarr,
            seedbox_radarr,
            sonarr: ArrClient::new(
                "sonarr",
                &cfg.urls.sonarr,
                secrets.sonarr_api_key.clone(),
                http.clone(),
            )?,
            radarr: ArrClient::new(
                "radarr",
                &cfg.urls.radarr,
                secrets.radarr_api_key.clone(),
                http.clone(),
            )?,
            qbit: QbitClient::new(&cfg.urls.qbittorrent, http.clone())?,
            jellyfin: JellyfinClient::new(
                &cfg.urls.jellyfin,
                secrets.jellyfin_api_key.clone(),
                http.clone(),
            )?,
            jellyseerr: JellyseerrClient::new(
                &cfg.urls.jellyseerr,
                secrets.jellyseerr_api_key.clone(),
                http.clone(),
            )?,
            cfg: Arc::new(cfg),
            secrets: Arc::new(secrets),
            http,
            qbit_lock: Arc::new(Mutex::new(())),
            onboard_lock: Arc::new(Mutex::new(())),
            state,
            dry_run,
        })
    }
}

impl TaskContext {
    /// Sonarr du VPS puis, si activé, celui de la seedbox.
    pub fn all_sonarr(&self) -> Vec<&ArrClient> {
        std::iter::once(&self.sonarr)
            .chain(self.seedbox_sonarr.as_ref())
            .collect()
    }

    /// Tous les Arrs (VPS puis seedbox).
    pub fn all_arrs(&self) -> Vec<&ArrClient> {
        [&self.sonarr, &self.radarr]
            .into_iter()
            .chain(self.seedbox_sonarr.as_ref())
            .chain(self.seedbox_radarr.as_ref())
            .collect()
    }
}

/// `Url::join` remplace le dernier segment si la base ne finit pas par `/` (ex. `…/radarr`).
fn with_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}
