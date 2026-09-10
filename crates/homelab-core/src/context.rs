use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
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
        Ok(Self {
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
