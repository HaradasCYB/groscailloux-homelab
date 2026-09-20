use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use crate::clients::{
    ArrClient, JellyfinClient, JellyseerrClient, PayPalClient, ProwlarrClient, QbitClient,
};
use crate::config::{Config, Secrets};
use crate::state::StateStore;
use crate::subscriptions::SubStore;

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
    /// qBittorrent de la seedbox, si `[seedbox] qbit_url` est défini.
    pub seedbox_qbit: Option<QbitClient>,
    pub qbit: QbitClient,
    pub jellyfin: JellyfinClient,
    pub jellyseerr: JellyseerrClient,
    /// Recherche en texte libre chez un indexer (titres traduits) ; absent sans `PROWLARR_API_KEY`.
    pub prowlarr: Option<ProwlarrClient>,
    /// PayPal REST, si `PAYPAL_*` est renseigné dans `.env`.
    pub paypal: Option<PayPalClient>,
    /// Fiches abonnés (`[subscriptions] db_file`).
    pub subs: Arc<SubStore>,
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
        let seedbox_qbit = if cfg.seedbox.enabled && !cfg.seedbox.qbit_url.is_empty() {
            let pw = secrets
                .seedbox_qbit_password
                .clone()
                .context("[seedbox] qbit_url défini mais SEEDBOX_QBIT_PASSWORD absent")?;
            Some(QbitClient::with_login(
                &with_slash(&cfg.seedbox.qbit_url),
                &cfg.seedbox.qbit_user,
                pw,
                http.clone(),
            )?)
        } else {
            None
        };
        Ok(Self {
            seedbox_sonarr,
            seedbox_radarr,
            seedbox_qbit,
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
            prowlarr: match secrets.prowlarr_api_key.clone() {
                Some(k) => Some(ProwlarrClient::new(&cfg.urls.prowlarr, k, http.clone())?),
                None => None,
            },
            paypal: secrets
                .paypal
                .clone()
                .map(|p| PayPalClient::new(http.clone(), p)),
            subs: Arc::new(SubStore::open(&cfg.subscriptions.db_file)?),
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

/// Une machine qui télécharge : son qBittorrent, ses Arrs, où ils rangent.
pub struct Side<'a> {
    /// `vps` ou `seedbox` (clé d'état, logs).
    pub name: &'static str,
    pub qbit: &'a QbitClient,
    pub radarr: &'a ArrClient,
    pub sonarr: &'a ArrClient,
    pub radarr_root: String,
    pub sonarr_root: String,
    pub quality_profile_id: i64,
    /// Dossier de téléchargement vu par qBittorrent → même dossier sur l'hôte, pour compter
    /// les hardlinks (VPS seulement ; `None` pour la seedbox, distante).
    pub local_downloads: Option<(String, std::path::PathBuf)>,
}

impl TaskContext {
    /// VPS puis, si la seedbox et son qBittorrent sont configurés, seedbox.
    pub fn sides(&self) -> Vec<Side<'_>> {
        let mut out = vec![Side {
            name: "vps",
            qbit: &self.qbit,
            radarr: &self.radarr,
            sonarr: &self.sonarr,
            radarr_root: self.cfg.auto_import.radarr_root.clone(),
            sonarr_root: self.cfg.auto_import.sonarr_root.clone(),
            quality_profile_id: self.secrets.quality_profile_id,
            local_downloads: Some((
                self.cfg.paths.downloads_in_container.clone(),
                self.cfg.paths.downloads.clone(),
            )),
        }];
        if let (Some(qbit), Some(radarr), Some(sonarr)) = (
            &self.seedbox_qbit,
            &self.seedbox_radarr,
            &self.seedbox_sonarr,
        ) {
            let sb = &self.cfg.seedbox;
            out.push(Side {
                name: "seedbox",
                qbit,
                radarr,
                sonarr,
                radarr_root: sb.radarr_root.clone(),
                sonarr_root: sb.sonarr_root.clone(),
                quality_profile_id: sb.quality_profile_id,
                local_downloads: None,
            });
        }
        out
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
