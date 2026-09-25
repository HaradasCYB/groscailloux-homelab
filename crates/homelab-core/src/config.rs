use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::secret::Secret;

/// Configuration non secrète, chargée depuis `homelab.toml`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
    #[serde(default)]
    pub seedbox: Seedbox,
    #[serde(default)]
    pub accounts: Accounts,
    #[serde(default)]
    pub onboard: Onboard,
    #[serde(default)]
    pub discord: Discord,
    #[serde(default)]
    pub chat: Chat,
    #[serde(default)]
    pub subscriptions: Subscriptions,
    #[serde(default)]
    pub manual_search: ManualSearch,
    #[serde(default)]
    pub indexers: Indexers,
    #[serde(default)]
    pub downloads: Downloads,
}

/// Où les tâches ont le droit de lancer de **nouveaux** téléchargements.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Downloads {
    /// Machines autorisées à récupérer du neuf : `vps`, `seedbox`. Une machine absente garde ses
    /// fiches, ses fichiers et ses imports, mais aucune tâche ne lui fera prendre une release.
    pub auto_sides: Vec<String>,
}

impl Default for Downloads {
    fn default() -> Self {
        Self {
            auto_sides: vec!["seedbox".into()],
        }
    }
}

impl Downloads {
    /// Nom d'un Arr (`sonarr`, `radarr-seedbox`…) → la machine qui le porte.
    pub fn side_of(arr: &str) -> &'static str {
        if arr.ends_with("seedbox") {
            "seedbox"
        } else {
            "vps"
        }
    }

    /// Cet Arr a-t-il le droit de prendre une nouvelle release ?
    pub fn may_grab(&self, arr: &str) -> bool {
        self.auto_sides.iter().any(|s| s == Self::side_of(arr))
    }
}

/// Règles communes à tout ce qui interroge l'indexer (tâches et page `/recherche`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Indexers {
    /// Requêtes au plus par heure glissante **et par clé** (deux clés : `C411` et `C411 (2)`).
    pub c411_max_per_hour: usize,
    /// Une clé qui répond 429 est mise de côté ce nombre de minutes ; les requêtes passent sur l'autre.
    pub cooldown_after_429_mins: i64,
    /// Requêtes de cette réserve gardées pour la page `/recherche` : les tâches de fond s'arrêtent avant.
    pub manual_reserve: usize,
    /// Prendre une release sans français (VO) quand aucune release française n'est acceptable.
    pub allow_no_french: bool,
    /// Plafonds de taille du choix automatique (la page reste libre).
    pub max_gb_per_episode: f64,
    pub max_gb_per_movie: f64,
}

impl Default for Indexers {
    fn default() -> Self {
        Self {
            c411_max_per_hour: 40,
            cooldown_after_429_mins: 15,
            manual_reserve: 10,
            allow_no_french: true,
            max_gb_per_episode: 3.0,
            max_gb_per_movie: 15.0,
        }
    }
}

/// Page `/recherche` : recherche manuelle d'une saison ou d'un film par identifiant TMDB (voir `manual_search`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct ManualSearch {
    /// Nom de l'indexer dans Prowlarr.
    pub c411_indexer: String,
    /// Recherches en texte libre (titres de la fiche) quand l'identifiant ne donne rien.
    pub text_queries: usize,
    /// Résultats gardés en mémoire.
    pub results_ttl_mins: i64,
}

impl Default for ManualSearch {
    fn default() -> Self {
        Self {
            c411_indexer: "C411".into(),
            text_queries: 2,
            results_ttl_mins: 30,
        }
    }
}

/// Tchat des membres dans Jellyfin (voir `chat`). Modérateurs : écrivent les annonces, lisent les fils
/// privés, suppriment tout message. `beta_users` non vide = tchat visible de ces comptes seulement.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Chat {
    pub enabled: bool,
    pub moderators: Vec<String>,
    pub beta_users: Vec<String>,
    pub db_file: PathBuf,
    pub max_chars: usize,
    pub min_gap_secs: u64,
    pub burst_max: usize,
    pub burst_window_secs: u64,
    /// Un membre peut supprimer son message pendant ce délai.
    pub delete_own_within_mins: i64,
    /// Au plus un mail récapitulatif (entraide + privé) par intervalle.
    pub moderator_mail_interval_mins: i64,
}

/// Abonnés et cycle premium (`homelab_core::subscriptions`, tâches `subscription_cycle` et
/// `subscription_reconcile`, page « Mon compte » dans Jellyfin). L'admin garde la main : statuts
/// « offert » et « exempt », prolongations manuelles, `cycle_dry_run` pour observer avant d'agir.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Subscriptions {
    pub enabled: bool,
    pub db_file: PathBuf,
    /// Jours couverts par un paiement quand PayPal n'annonce pas la prochaine facturation.
    pub period_days: u32,
    /// Jours de grâce après l'échéance avant suspension.
    pub grace_days: u32,
    /// Rappels envoyés N jours avant l'échéance (mail au membre, récapitulatif admin).
    pub remind_days: Vec<u32>,
    /// Essai gratuit à l'inscription publique (0 = compte à activer par l'admin, comme avant).
    pub trial_days: u32,
    /// Jours offerts au parrain et au filleul au premier paiement du filleul.
    pub referral_days: u32,
    /// Plafond de jours de parrainage par compte et par année glissante.
    pub referral_cap_days_per_year: u32,
    /// Comptes jamais suspendus par le cycle (en plus de `accounts.protected`).
    pub exempt: Vec<String>,
    /// Le cycle annonce (journal + Discord admin) sans suspendre ni envoyer de mail aux membres.
    pub cycle_dry_run: bool,
    /// Prix affiché aux membres (texte libre, ex. « 3,50 € / mois »).
    pub price_text: String,
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self {
            enabled: true,
            db_file: "/opt/homelab/state/subscriptions.db".into(),
            period_days: 31,
            grace_days: 3,
            remind_days: vec![7, 1],
            trial_days: 7,
            referral_days: 15,
            referral_cap_days_per_year: 90,
            exempt: Vec::new(),
            cycle_dry_run: true,
            price_text: "3,50 € / mois".into(),
        }
    }
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            enabled: true,
            moderators: vec!["Haradas".into(), "LeGrosCailloux".into()],
            beta_users: Vec::new(),
            db_file: "/opt/homelab/state/chat.db".into(),
            max_chars: 2000,
            min_gap_secs: 3,
            burst_max: 30,
            burst_window_secs: 600,
            delete_own_within_mins: 15,
            moderator_mail_interval_mins: 15,
        }
    }
}

/// Comptes Jellyfin : « premium » = compte actif, sinon suspendu (`IsDisabled`, rien n'est
/// supprimé). Plafonds dimensionnés pour 6 vCPU sans GPU et le lien seedbox (~190 Mbit/s).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Accounts {
    /// Comptes premium au plus (admin exclus) ; vérifié à l'activation.
    pub max_premium: usize,
    /// Appareils connectés par compte (`MaxActiveSessions` de Jellyfin, 0 = illimité). Jellyfin ne
    /// l'applique qu'à la connexion et compte les sessions fantômes de l'appli iOS : laissé à 0.
    pub max_devices_per_user: u32,
    /// Lectures simultanées par compte (tâche `playback_limit`, 0 = illimité ; comptes protégés exemptés).
    pub max_playbacks_per_user: usize,
    /// Un compte créé par l'onboarding est-il premium d'office ?
    pub new_accounts_premium: bool,
    /// Comptes jamais suspendus ni supprimés par la page (noms Jellyfin, casse ignorée).
    pub protected: Vec<String>,
    /// Demandes Jellyseerr validées sans l'admin (bit 128) : donné à la création et à l'activation.
    pub jellyseerr_auto_approve: bool,
    /// Voir toutes les demandes dans Jellyseerr et l'onglet Demandes de Jellyfin (bit 16384, lecture seule).
    pub jellyseerr_view_requests: bool,
    /// Saut du lecteur Jellyfin, en millisecondes : bouton d'avance rapide **et** flèches gauche/droite.
    /// Jellyfin met 30 s en avant par défaut, ce qui fait rater une réplique à chaque clic.
    pub skip_forward_ms: i64,
    pub skip_back_ms: i64,
    /// Langue audio préférée posée sur chaque compte (`AudioLanguagePreference`, ISO 639-2) : Jellyfin choisit
    /// la piste française d'un MULTi au lieu de la piste « par défaut » du fichier (souvent la VO).
    pub audio_language: String,
    /// Langue de sous-titres préférée (`SubtitleLanguagePreference`).
    pub subtitle_language: String,
    /// `Smart` = sous-titres seulement quand l'audio n'est pas dans la langue préférée ; `Always`, `OnlyForced`,
    /// `Default`, `None`.
    pub subtitle_mode: String,
    /// Langue audio du mode « VO » de Mon compte (japonais : les animés ; films et séries basculés côté client).
    pub vo_audio_language: String,
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            max_premium: 25,
            max_devices_per_user: 0,
            max_playbacks_per_user: 2,
            new_accounts_premium: false,
            protected: vec!["Haradas".into(), "LeGrosCailloux".into()],
            jellyseerr_auto_approve: true,
            jellyseerr_view_requests: true,
            skip_forward_ms: 10_000,
            skip_back_ms: 10_000,
            audio_language: "fre".into(),
            subtitle_language: "fre".into(),
            subtitle_mode: "Smart".into(),
            vo_audio_language: "jpn".into(),
        }
    }
}

/// Seedbox distante : Radarr/Sonarr qui y rangent les médias, montés en lecture seule sur le
/// VPS (rclone) et lus par Jellyfin. `enabled = false` ⇒ tout ce qui touche la seedbox est ignoré.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Seedbox {
    pub enabled: bool,
    pub radarr_url: String,
    pub sonarr_url: String,
    /// Dossier que le Sonarr de la seedbox scanne pour le contournement TBA.
    pub sonarr_downloads: String,
    /// Racine des médias côté seedbox (préfixe des chemins d'import des Arrs).
    pub media_root: String,
    /// Même arborescence montée sur le VPS (rclone).
    pub mount_point: PathBuf,
    /// Même arborescence vue par le conteneur Jellyfin.
    pub jellyfin_root: String,
    /// API rc de rclone mount, pour invalider le cache de répertoires après un import.
    pub rclone_rc: String,
    /// Id Jellyseerr du serveur Sonarr de la seedbox (`media.serviceId` des demandes).
    pub jellyseerr_sonarr_id: i64,
    /// Id Jellyseerr du serveur Sonarr du VPS.
    pub jellyseerr_vps_sonarr_id: i64,
    /// WebUI qBittorrent de la seedbox (proxy HTTPS de l'hébergeur), vide = non utilisé.
    /// Mot de passe : `SEEDBOX_QBIT_PASSWORD`.
    pub qbit_url: String,
    pub qbit_user: String,
    /// Dossiers racines des Radarr/Sonarr de la seedbox (fiches ajoutées par torrent_import).
    pub radarr_root: String,
    pub sonarr_root: String,
    /// Profil de qualité des fiches ajoutées sur la seedbox.
    pub quality_profile_id: i64,
    /// Bazarr de la seedbox (proxy HTTPS de l'hébergeur), vide = pas de `subtitle_sync`.
    /// Clé : `SEEDBOX_BAZARR_API_KEY`.
    pub bazarr_url: String,
}

impl Default for Seedbox {
    fn default() -> Self {
        Self {
            enabled: false,
            radarr_url: String::new(),
            sonarr_url: String::new(),
            sonarr_downloads: String::new(),
            media_root: String::new(),
            mount_point: "/mnt/seedbox/media".into(),
            jellyfin_root: "/seedbox/media".into(),
            rclone_rc: "http://127.0.0.1:5572".into(),
            jellyseerr_sonarr_id: 1,
            jellyseerr_vps_sonarr_id: 0,
            qbit_url: String::new(),
            qbit_user: String::new(),
            radarr_root: String::new(),
            sonarr_root: String::new(),
            quality_profile_id: 7,
            bazarr_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Urls {
    pub sonarr: String,
    pub radarr: String,
    pub prowlarr: String,
    pub qbittorrent: String,
    pub jellyfin: String,
    pub jellyseerr: String,
}

/// Discord (webhooks `DISCORD_WEBHOOK_MEMBERS` / `DISCORD_WEBHOOK_ADMIN` dans `.env`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Discord {
    /// Les annonces du tchat sont aussi postées sur le salon des membres.
    pub announcements: bool,
    /// Les alertes admin (boucles HLS, indexeur, inscriptions, relances) vont aussi sur le salon admin.
    pub admin_alerts: bool,
}

impl Default for Discord {
    fn default() -> Self {
        Self {
            announcements: true,
            admin_alerts: true,
        }
    }
}

/// Onboarding des membres : lien de bienvenue (définir son mot de passe) et page publique d'inscription.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Onboard {
    /// Durée de validité d'un lien de bienvenue (minutes) ; renouvelable depuis la page elle-même.
    pub link_ttl_mins: u64,
    /// Page publique `/inscription` ouverte (le compte reste à activer par l'admin).
    pub public_signup: bool,
    /// Inscriptions acceptées par 24 h sur la page publique, toutes adresses confondues.
    pub max_signups_per_day: usize,
    /// Renvois de lien acceptés par heure pour une même adresse.
    pub renew_per_hour: usize,
}

impl Default for Onboard {
    fn default() -> Self {
        Self {
            link_ttl_mins: 60,
            public_signup: true,
            max_signups_per_day: 10,
            renew_per_hour: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Web {
    pub listen: String,
    pub rate_limit_secs: u64,
}

impl Default for Web {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8766".into(),
            rate_limit_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tasks {
    /// Noms de tâches à ne pas planifier (restent invocables via `homelabctl run`).
    pub disabled: Vec<String>,
    pub tracker_ratio: TrackerRatio,
    pub stuck_handler: StuckHandler,
    pub disk_pressure: DiskPressure,
    #[serde(default)]
    pub hls_loop_watch: HlsLoopWatch,
    pub tba_bypass: Interval300,
    pub monitor_sync: Interval600,
    pub user_poller: UserPoller,
    pub stack_health: StackHealth,
    pub seedbox_refresh: Interval300,
    pub id_match_import: Interval300,
    pub torrent_import: TorrentImport,
    pub series_search: SeriesSearch,
    pub movie_search: MovieSearch,
    pub indexer_unblock: IndexerUnblock,
    pub anime_library: AnimeLibrary,
    pub identity_check: IdentityCheck,
    pub deletion_cleanup: DeletionCleanup,
    pub trending: Trending,
    pub playback_limit: PlaybackLimit,
    #[serde(default)]
    pub playback_canary: PlaybackCanary,
    #[serde(default)]
    pub subscription_cycle: Interval3600,
    #[serde(default)]
    pub subscription_reconcile: Interval86400,
    #[serde(default)]
    pub subtitle_sync: SubtitleSync,
}

/// Sous-titres extraits par le Bazarr de la seedbox → rafraîchissement des fiches Jellyfin
/// (voir `tasks::subtitle_sync`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct SubtitleSync {
    pub interval_secs: u64,
    /// Items (épisodes/films) traités au plus par passage — un ffmpeg à la fois sur la seedbox.
    pub max_per_run: usize,
    /// Budget d'un passage : la boucle s'arrête au-delà et reprend au suivant (le planificateur coupe à 600 s).
    pub max_seconds: u64,
    /// Script d'extraction **sur la seedbox** (`scripts/seedbox/gc-extract-sub.sh`), vide = tâche inactive.
    pub extract_script: String,
    /// Un item déjà traité n'est revu qu'après ce délai.
    pub retry_hours: u64,
    /// Item sans piste extractible (code 3 du script) : nouvel essai après ce délai.
    pub failed_retry_days: u64,
    /// Au-delà de cette taille, l'ASS complet n'est pas marqué `.default` (le SRT dérivé l'est).
    pub max_default_ass_mb: u64,
}

impl Default for SubtitleSync {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            max_per_run: 40,
            max_seconds: 420,
            extract_script: "~/bin/gc-extract-sub.sh".into(),
            retry_hours: 6,
            failed_retry_days: 7,
            max_default_ass_mb: 8,
        }
    }
}

/// Canari de lecture : un vrai transcodage de quelques secondes, alerte admin au premier échec
/// (voir `tasks::playback_canary`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct PlaybackCanary {
    pub interval_secs: u64,
    /// Au-delà, le premier segment est jugé trop lent (lecture qui « charge sans fin »).
    pub max_first_segment_secs: u64,
    /// Passage sauté si au moins autant de transcodages sont déjà en cours (ne pas gêner les membres).
    pub skip_if_transcodes_at_least: usize,
}

impl Default for PlaybackCanary {
    fn default() -> Self {
        Self {
            interval_secs: 900,
            max_first_segment_secs: 20,
            skip_if_transcodes_at_least: 2,
        }
    }
}

/// Lectures simultanées par compte : arrêt des lectures en trop (voir `tasks::playback_limit`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct PlaybackLimit {
    pub interval_secs: u64,
    /// Une lecture en trop n'est arrêtée qu'après ce délai (le temps de changer d'appareil).
    pub grace_secs: i64,
    pub max_actions_per_run: usize,
}

impl Default for PlaybackLimit {
    fn default() -> Self {
        Self {
            interval_secs: 20,
            grace_secs: 30,
            max_actions_per_run: 5,
        }
    }
}

/// Rangée « Tendances » de l'accueil Jellyfin : collection tenue à jour depuis Playback Reporting.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Trending {
    pub interval_secs: u64,
    /// Fenêtre de classement (jours) ; complétée par `fallback_days` si trop peu de titres.
    pub days: i64,
    pub fallback_days: i64,
    pub size: usize,
    /// Visionnage minimal d'un spectateur sur un titre pour qu'il compte (évite les clics par erreur).
    pub min_minutes: i64,
    pub collection_name: String,
}

impl Default for Trending {
    fn default() -> Self {
        Self {
            interval_secs: 21600,
            days: 7,
            fallback_days: 30,
            size: 10,
            min_minutes: 10,
            collection_name: "Tendances cette semaine".into(),
        }
    }
}

/// Nettoyage après une suppression dans Jellyfin : fiche Arr, Jellyseerr, torrent.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct DeletionCleanup {
    pub interval_secs: u64,
    /// Titres (film ou série) nettoyés au plus par passage et par machine.
    pub max_titles_per_run: usize,
    /// Plus de titres manquants que ça d'un coup sur une machine ⇒ on n'agit pas (disque, montage).
    pub abort_if_missing_titles_over: usize,
    /// Un fichier absent n'est traité qu'après ce délai depuis sa première absence constatée.
    pub confirm_after_secs: i64,
    /// Fichiers importés depuis moins longtemps ignorés (cache du montage seedbox, scan Jellyfin).
    pub min_file_age_mins: i64,
    /// Torrent C411 (ou tracker inconnu) : retiré seulement à ce ratio…
    pub c411_min_ratio: f64,
    /// …ou après ce temps de seed.
    pub c411_min_seed_days: i64,
    /// VPS : [racine vue par l'Arr, même dossier sur l'hôte, même dossier vu par Jellyfin].
    pub vps_paths: Vec<[String; 3]>,
}

impl Default for DeletionCleanup {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            max_titles_per_run: 5,
            abort_if_missing_titles_over: 10,
            confirm_after_secs: 240,
            min_file_age_mins: 60,
            c411_min_ratio: 1.0,
            c411_min_seed_days: 7,
            vps_paths: vec![
                [
                    "/movies".into(),
                    "/opt/homelab/library/media/movies".into(),
                    "/media/movies".into(),
                ],
                [
                    "/tv".into(),
                    "/opt/homelab/library/media/tvshows".into(),
                    "/media/tvshows".into(),
                ],
                [
                    "/anime-films".into(),
                    "/opt/homelab/library/media/anime-films".into(),
                    "/media/anime-films".into(),
                ],
                [
                    "/anime".into(),
                    "/opt/homelab/library/media/anime".into(),
                    "/media/anime".into(),
                ],
            ],
        }
    }
}

/// Recherche des saisons manquantes par identifiant TMDB chez un seul indexer (C411), via Prowlarr.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct SeriesSearch {
    pub interval_secs: u64,
    /// Requêtes envoyées à l'indexer au plus par passage (identifiant et texte libre confondus).
    pub max_queries_per_run: usize,
    /// Écart minimal entre deux requêtes à l'indexer (limite d'API de C411).
    pub query_gap_secs: u64,
    /// Nouvelle recherche d'une saison sans candidat après N heures.
    pub retry_after_hours: i64,
    /// Nouvelle recherche d'une saison déjà prise après N heures (si elle manque encore).
    pub grabbed_retry_hours: i64,
    /// Nouvelle tentative après une erreur (indexer indisponible, délai dépassé).
    pub error_retry_hours: i64,
    /// Nom (préfixe, insensible à la casse) de l'indexer interrogé, dans Prowlarr.
    pub indexer: String,
    /// Noms de la série essayés en texte libre quand la recherche par identifiant ne donne rien.
    pub text_queries: usize,
    /// Épisodes envoyés au plus pour une saison sans pack, dans le même passage.
    pub max_grabs_per_season: usize,
    /// Requêtes en plus (au-delà de `max_queries_per_run`) réservées aux demandes fraîches.
    pub max_new_per_run: usize,
    /// Une fiche ajoutée depuis moins de N heures est une demande fraîche.
    pub new_request_hours: i64,
    /// Délai (minutes) avant de reprendre une saison dont on vient de prendre des épisodes.
    pub episode_retry_mins: i64,
    /// Adresse de Prowlarr vue depuis les conteneurs Sonarr/Radarr du VPS (lien de téléchargement envoyé).
    pub prowlarr_url_for_arrs: String,
    /// Récupérer les cours d'animés publiés sous leur propre titre (interrupteur : coupe tout le chemin).
    pub cour_packs: bool,
    /// Au-delà de ce nombre de fichiers vidéo, ce n'est plus un cours : on n'y touche pas.
    pub cour_max_files: usize,
    /// Chercher aussi les épisodes suivis sans date de diffusion d'une saison déjà commencée (retard TheTVDB).
    pub undated_episodes: bool,
    /// Seules les saisons dont la dernière diffusion date de moins de N jours sont examinées.
    pub undated_window_days: i64,
}

impl Default for SeriesSearch {
    fn default() -> Self {
        Self {
            interval_secs: 600,
            max_queries_per_run: 6,
            query_gap_secs: 5,
            retry_after_hours: 24,
            grabbed_retry_hours: 168,
            error_retry_hours: 1,
            indexer: "C411".into(),
            text_queries: 2,
            max_grabs_per_season: 60,
            max_new_per_run: 3,
            new_request_hours: 1,
            episode_retry_mins: 5,
            prowlarr_url_for_arrs: "http://prowlarr:9696".into(),
            cour_packs: true,
            cour_max_files: 30,
            undated_episodes: true,
            undated_window_days: 730,
        }
    }
}

/// Rattrapage des films suivis et manquants par identifiant TMDB (la recherche de Radarr reste la voie normale).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct MovieSearch {
    pub interval_secs: u64,
    /// Films cherchés au plus par passage.
    pub max_per_run: usize,
    /// Un film n'est rattrapé que s'il manque depuis au moins N heures après son ajout.
    pub missing_hours: i64,
    /// Nouvelle recherche d'un film sans candidat après N heures.
    pub retry_after_hours: i64,
    pub error_retry_hours: i64,
    pub query_gap_secs: u64,
    pub indexer: String,
    /// Film français sans date numérique : pas de recherche avant N jours après la salle (VOD à 4 mois en France).
    pub min_days_after_cinema: i64,
}

impl Default for MovieSearch {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            max_per_run: 3,
            missing_hours: 0,
            retry_after_hours: 72,
            error_retry_hours: 1,
            query_gap_secs: 5,
            indexer: "C411".into(),
            min_days_after_cinema: 110,
        }
    }
}

/// Rangement de l'animation japonaise dans les dossiers « Anime » et « Films d'animation » (voir `tasks::anime_library`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnimeLibrary {
    pub interval_secs: u64,
    /// Fiches déplacées au plus par passage (tous Arrs confondus).
    pub max_moves_per_run: usize,
    /// Fiches TMDB lues au plus par passage (le reste attend le passage suivant).
    pub max_lookups_per_run: usize,
    /// Classement mis en cache, revu après N jours (1 jour pour une fiche introuvable).
    pub recheck_days: i64,
    /// Dossiers racines vus par les Arrs du VPS et de la seedbox.
    pub vps_series_root: String,
    pub vps_movies_root: String,
    pub seedbox_series_root: String,
    pub seedbox_movies_root: String,
    /// Essai : seuls ces identifiants TMDB sont traités (vide = tous).
    pub only_tmdb: Vec<i64>,
}

impl Default for AnimeLibrary {
    fn default() -> Self {
        Self {
            interval_secs: 1800,
            max_moves_per_run: 5,
            max_lookups_per_run: 150,
            recheck_days: 30,
            vps_series_root: "/anime".into(),
            vps_movies_root: "/anime-films".into(),
            seedbox_series_root: "/home/kakaouette/media/Anime".into(),
            seedbox_movies_root: "/home/kakaouette/media/Anime Movies".into(),
            only_tmdb: Vec::new(),
        }
    }
}

/// Contrôle des identifications de Jellyfin (voir `tasks::identity_check`).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct IdentityCheck {
    pub interval_secs: u64,
    /// Corrections au plus par passage (chacune relance les métadonnées du titre).
    pub max_fixes_per_run: usize,
    /// Ré-identifier aussi les fiches affichées sous un nom de release (« Matrix.Reloaded.2003.MULTi… »).
    pub fix_release_names: bool,
}

impl Default for IdentityCheck {
    fn default() -> Self {
        Self {
            interval_secs: 1800,
            max_fixes_per_run: 3,
            fix_release_names: true,
        }
    }
}

/// Remise en service des indexeurs mis en pause par Sonarr/Radarr après des échecs.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct IndexerUnblock {
    pub interval_secs: u64,
    /// Un indexeur n'est débloqué que si son dernier échec date d'au moins N minutes (fenêtre de C411 : 1 h).
    pub quiet_mins: i64,
    /// Au plus un arrêt/redémarrage par application pendant cette durée.
    pub app_cooldown_mins: i64,
    /// Au-delà de N déblocages en 24 h pour un même indexeur, on ne débloque plus : alerte seulement.
    pub max_unblocks_per_day: u32,
    /// Hôte `ssh` de la seedbox (clé d'admin) et dossier de ses applications (`<dossier>/sonarr/sonarr.db`).
    pub ssh_host: String,
    pub seedbox_apps_dir: String,
}

impl Default for IndexerUnblock {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            quiet_mins: 15,
            app_cooldown_mins: 60,
            max_unblocks_per_day: 10,
            ssh_host: "seedbox".into(),
            seedbox_apps_dir: "/home/kakaouette/.apps".into(),
        }
    }
}

/// Import des torrents ajoutés à la main dans qBittorrent (VPS et seedbox).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct TorrentImport {
    pub interval_secs: u64,
    /// Torrents examinés (parse, recherche, import) au plus par passage, tous côtés confondus.
    pub max_per_run: usize,
    /// Tentatives avant d'abandonner un torrent sur erreur (Arr injoignable…).
    pub max_attempts: u32,
    /// Attente maximale des épisodes d'une série qui vient d'être ajoutée.
    pub series_ready_secs: u64,
}

impl Default for TorrentImport {
    fn default() -> Self {
        Self {
            interval_secs: 120,
            max_per_run: 10,
            max_attempts: 3,
            series_ready_secs: 90,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct StackHealth {
    pub interval_secs: u64,
    /// Jamais deux redémarrages du même service en moins de N secondes.
    pub restart_cooldown_secs: i64,
    /// Un service `unhealthy` depuis au moins N secondes est redémarré.
    pub unhealthy_grace_secs: i64,
    /// Services que la tâche ne touche jamais (arrêt volontaire).
    pub ignore: Vec<String>,
    /// Sondes applicatives, en plus du healthcheck Docker.
    pub probes: Vec<Probe>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub service: String,
    #[serde(default = "d_get")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "d_form")]
    pub content_type: String,
    #[serde(default = "d_200")]
    pub expect_status: u16,
    #[serde(default)]
    pub expect_body_contains: String,
    /// Ne pas redémarrer tant que ces services ne sont pas `healthy`.
    #[serde(default)]
    pub requires_healthy: Vec<String>,
}

impl Default for StackHealth {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            restart_cooldown_secs: 600,
            unhealthy_grace_secs: 120,
            ignore: vec![],
            probes: vec![Probe {
                service: "guacamole".into(),
                method: "POST".into(),
                url: "http://localhost:8081/guacamole/api/tokens".into(),
                body: "username=homelabd-probe&password=probe".into(),
                content_type: d_form(),
                expect_status: 403,
                expect_body_contains: "INVALID_CREDENTIALS".into(),
                requires_healthy: vec!["guacdb".into()],
            }],
        }
    }
}

fn d_get() -> String {
    "GET".into()
}
fn d_form() -> String {
    "application/x-www-form-urlencoded".into()
}
fn d_200() -> u16 {
    200
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
            unlimited: vec!["c411.org".into(), "c411.tw".into()],
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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

/// Boucles HLS : un client qui redemande sans fin le même segment d'un flux transcodé (journal NPM).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct HlsLoopWatch {
    pub interval_secs: u64,
    /// Journal d'accès NPM de l'hôte Jellyfin (horodatages UTC).
    pub npm_access_log: PathBuf,
    /// Octets relus en fin de journal à chaque passage.
    pub tail_bytes: u64,
    /// Fenêtre glissante (secondes) et nombre de demandes d'un même segment qui font une boucle.
    pub window_secs: i64,
    pub threshold: usize,
}

impl Default for HlsLoopWatch {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            npm_access_log: "/opt/homelab/npm/data/logs/proxy-host-1_access.log".into(),
            tail_bytes: 4 * 1024 * 1024,
            window_secs: 300,
            threshold: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval300 {
    pub interval_secs: u64,
}
impl Default for Interval300 {
    fn default() -> Self {
        Self { interval_secs: 300 }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval3600 {
    pub interval_secs: u64,
}
impl Default for Interval3600 {
    fn default() -> Self {
        Self {
            interval_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval86400 {
    pub interval_secs: u64,
}
impl Default for Interval86400 {
    fn default() -> Self {
        Self {
            interval_secs: 86_400,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Interval600 {
    pub interval_secs: u64,
}
impl Default for Interval600 {
    fn default() -> Self {
        Self { interval_secs: 600 }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
            transcodes_dir: "".into(),
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
                "cache",
                "influxdb",
                "jellyfin/cache",
                "jellyfin/config/log",
                "target",
                "logs",
                "jellyfin/config/data/trickplay",
                "jellyfin/config/metadata/People",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            keep_last: 4,
            mysql_container: "guacdb".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
        let sb = &self.seedbox;
        if sb.enabled {
            for (k, v) in [
                ("radarr_url", &sb.radarr_url),
                ("sonarr_url", &sb.sonarr_url),
                ("rclone_rc", &sb.rclone_rc),
            ] {
                if !v.starts_with("http://") && !v.starts_with("https://") {
                    bail!("[seedbox] {k} invalide : {v:?}");
                }
            }
            if !sb.media_root.starts_with('/') || sb.sonarr_downloads.is_empty() {
                bail!("[seedbox] media_root (absolu) et sonarr_downloads sont requis");
            }
            if !sb.qbit_url.is_empty() {
                if !sb.qbit_url.starts_with("http://") && !sb.qbit_url.starts_with("https://") {
                    bail!("[seedbox] qbit_url invalide : {:?}", sb.qbit_url);
                }
                if sb.qbit_user.is_empty()
                    || !sb.radarr_root.starts_with('/')
                    || !sb.sonarr_root.starts_with('/')
                {
                    bail!("[seedbox] qbit_url défini : qbit_user, radarr_root et sonarr_root (absolus) sont requis");
                }
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
    /// Bibliothèques supplémentaires données aux nouveaux comptes (JELLYFIN_LIB_EXTRA, virgules).
    pub jellyfin_lib_extra: Vec<String>,
    pub seedbox_radarr_api_key: Option<Secret>,
    pub seedbox_sonarr_api_key: Option<Secret>,
    pub seedbox_qbit_password: Option<Secret>,
    pub seedbox_bazarr_api_key: Option<Secret>,
    pub jellyfin_public_url: String,
    pub jellyseerr_public_url: String,
    /// Adresse publique de l'onboarder (`ONBOARD_PUBLIC_URL`) : lien vers `/guide` dans le mail de
    /// bienvenue. Absente = pas de lien.
    pub onboard_public_url: Option<String>,
    /// Contact affiché par le guide (`GUIDE_CONTACT_EMAIL`, `GUIDE_CONTACT_DISCORD`), hors du dépôt.
    pub guide_contact_email: Option<String>,
    pub guide_contact_discord: Option<String>,
    /// Destinataire des récapitulatifs du tchat (`CHAT_ADMIN_EMAIL`, repli `GUIDE_CONTACT_EMAIL`).
    pub chat_admin_email: Option<String>,
    /// Webhooks Discord (secrets) : salon des membres, salon admin (repli sur membres), rôle à mentionner.
    pub discord_webhook_members: Option<String>,
    pub discord_webhook_admin: Option<String>,
    pub discord_role_members: Option<String>,
    pub quality_profile_id: i64,
    pub onboard_token: Option<Secret>,
    /// Protège `/status` et `/status.html` de homelabd (session `/connexion`).
    pub status_token: Option<Secret>,
    /// Adresses de l'admin (IP de la maison) ouvertes sans connexion sur les pages d'administration
    /// (`HOMELABD_ADMIN_TRUSTED_IPS`, virgules). Dans `.env` : jamais d'IP dans le dépôt public.
    pub admin_trusted_ips: Vec<String>,
    /// Page de don `/don` (bouton PayPal) : identifiants publics mais propres au compte PayPal,
    /// gardés hors du dépôt. Absents = pas de page.
    pub donation: Option<Donation>,
    /// Destinataire des notifications de demande d'activation Premium (`DONATION_NOTIFY_EMAIL`,
    /// repli sur `ADMIN_EMAIL`). Vide = notifications désactivées.
    pub donation_notify_email: String,
    pub smtp: Option<Smtp>,
    /// API REST PayPal (abonnements) : `PAYPAL_ENV` = live|sandbox, identifiants `PAYPAL_CLIENT_ID`/
    /// `PAYPAL_SECRET` (ou `PAYPAL_SANDBOX_*`), `PAYPAL_PLAN_ID`, `PAYPAL_WEBHOOK_ID`. Absent = pas de
    /// paiement relié (fiches manuelles seulement).
    pub paypal: Option<PayPal>,
    /// Adresse publique de la page `/premium` (`PREMIUM_PUBLIC_URL`, repli `ONBOARD_PUBLIC_URL`) :
    /// liens de paiement dans les mails et « Mon compte ».
    pub premium_public_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PayPal {
    pub client_id: String,
    pub secret: Secret,
    pub plan_id: String,
    pub sandbox: bool,
    pub webhook_id: Option<String>,
}

impl PayPal {
    pub fn from_env() -> Option<Self> {
        let sandbox = opt("PAYPAL_ENV")
            .map(|e| e.eq_ignore_ascii_case("sandbox"))
            .unwrap_or(false);
        let (cid, sec, plan, wh) = if sandbox {
            (
                opt("PAYPAL_SANDBOX_CLIENT_ID"),
                opt("PAYPAL_SANDBOX_SECRET"),
                opt("PAYPAL_SANDBOX_PLAN_ID"),
                opt("PAYPAL_SANDBOX_WEBHOOK_ID"),
            )
        } else {
            (
                opt("PAYPAL_CLIENT_ID"),
                opt("PAYPAL_SECRET"),
                opt("PAYPAL_PLAN_ID"),
                opt("PAYPAL_WEBHOOK_ID"),
            )
        };
        let ok = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        match (cid, sec, plan) {
            (Some(c), Some(s), Some(p)) if ok(&c) && ok(&p) && !s.is_empty() => Some(Self {
                client_id: c,
                secret: Secret::new(s),
                plan_id: p,
                sandbox,
                webhook_id: wh.filter(|w| ok(w)),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Donation {
    pub paypal_client_id: String,
    pub paypal_plan_id: String,
}

impl Donation {
    /// Identifiants PayPal : lettres, chiffres, `-` et `_` seulement (injectés dans du JS et une URL).
    pub fn from_parts(client_id: Option<String>, plan_id: Option<String>) -> Option<Self> {
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        match (client_id, plan_id) {
            (Some(c), Some(p)) if ok(&c) && ok(&p) => Some(Self {
                paypal_client_id: c,
                paypal_plan_id: p,
            }),
            _ => None,
        }
    }
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
            jellyfin_lib_extra: opt("JELLYFIN_LIB_EXTRA")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            seedbox_radarr_api_key: opt("SEEDBOX_RADARR_API_KEY").map(Secret::new),
            seedbox_sonarr_api_key: opt("SEEDBOX_SONARR_API_KEY").map(Secret::new),
            seedbox_qbit_password: opt("SEEDBOX_QBIT_PASSWORD").map(Secret::new),
            seedbox_bazarr_api_key: opt("SEEDBOX_BAZARR_API_KEY").map(Secret::new),
            jellyfin_public_url: req("JELLYFIN_PUBLIC_URL")?,
            jellyseerr_public_url: req("JELLYSEERR_PUBLIC_URL")?,
            onboard_public_url: opt("ONBOARD_PUBLIC_URL")
                .map(|u| u.trim_end_matches('/').to_string()),
            guide_contact_email: opt("GUIDE_CONTACT_EMAIL"),
            guide_contact_discord: opt("GUIDE_CONTACT_DISCORD"),
            chat_admin_email: opt("CHAT_ADMIN_EMAIL").or_else(|| opt("GUIDE_CONTACT_EMAIL")),
            discord_webhook_members: opt("DISCORD_WEBHOOK_MEMBERS")
                .filter(|u| u.starts_with("https://")),
            discord_webhook_admin: opt("DISCORD_WEBHOOK_ADMIN")
                .filter(|u| u.starts_with("https://")),
            discord_role_members: opt("DISCORD_ROLE_MEMBERS").filter(|r| !r.is_empty()),
            quality_profile_id: opt("QUALITY_PROFILE_ID")
                .map(|v| v.parse::<i64>().context("QUALITY_PROFILE_ID non numérique"))
                .transpose()?
                .unwrap_or(6),
            onboard_token: opt("HOMELABD_ONBOARD_TOKEN").map(Secret::new),
            status_token: opt("HOMELABD_STATUS_TOKEN").map(Secret::new),
            admin_trusted_ips: opt("HOMELABD_ADMIN_TRUSTED_IPS")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            donation: Donation::from_parts(
                opt("DONATION_PAYPAL_CLIENT_ID"),
                opt("DONATION_PAYPAL_PLAN_ID"),
            ),
            donation_notify_email: opt("DONATION_NOTIFY_EMAIL")
                .or_else(|| opt("ADMIN_EMAIL"))
                .unwrap_or_default(),
            paypal: PayPal::from_env(),
            premium_public_url: opt("PREMIUM_PUBLIC_URL")
                .or_else(|| opt("ONBOARD_PUBLIC_URL"))
                .map(|u| u.trim_end_matches('/').to_string()),
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
        assert_eq!(cfg.tasks.torrent_import.interval_secs, 120);
        assert_eq!(cfg.tasks.torrent_import.max_per_run, 10);
        assert_eq!(cfg.tasks.hls_loop_watch.threshold, 20);
        assert_eq!(cfg.tasks.hls_loop_watch.window_secs, 300);
        assert_eq!(cfg.seedbox.quality_profile_id, 7);
        assert_eq!(cfg.tasks.series_search.max_queries_per_run, 6);
        assert_eq!(cfg.tasks.series_search.max_grabs_per_season, 60);
        assert_eq!(cfg.indexers.c411_max_per_hour, 40);
        // plafond à l'acquisition (audit lecture 2026-09-18) : ~17 Mbit/s
        assert_eq!(cfg.indexers.max_gb_per_episode, 3.0);
        assert_eq!(cfg.indexers.max_gb_per_movie, 15.0);
        // par défaut, seule la seedbox récupère du neuf (2026-09-18)
        assert_eq!(cfg.downloads.auto_sides, vec!["seedbox".to_string()]);
        assert!(cfg.downloads.may_grab("sonarr-seedbox"));
        assert!(cfg.downloads.may_grab("radarr-seedbox"));
        assert!(!cfg.downloads.may_grab("sonarr"));
        assert!(!cfg.downloads.may_grab("radarr"));
        assert_eq!(cfg.accounts.max_premium, 25);
        assert_eq!(cfg.accounts.max_devices_per_user, 0);
        // saut du lecteur : 10 s des deux côtés (Jellyfin met 30 s en avant par défaut)
        assert_eq!(cfg.accounts.skip_forward_ms, 10_000);
        assert_eq!(cfg.accounts.skip_back_ms, 10_000);
        assert_eq!(cfg.onboard.link_ttl_mins, 60);
        assert!(cfg.onboard.public_signup);
        assert_eq!(cfg.onboard.max_signups_per_day, 10);
        assert_eq!(cfg.onboard.renew_per_hour, 3);
        assert!(cfg.discord.announcements && cfg.discord.admin_alerts);
        assert_eq!(cfg.accounts.max_playbacks_per_user, 2);
        assert_eq!(cfg.tasks.playback_limit.grace_secs, 30);
        assert!(!cfg.accounts.new_accounts_premium);
        assert_eq!(cfg.accounts.protected.len(), 2);
        assert_eq!(cfg.tasks.deletion_cleanup.vps_paths.len(), 4);
        assert_eq!(cfg.tasks.deletion_cleanup.c411_min_seed_days, 7);
        assert_eq!(cfg.tasks.trending.size, 10);
    }

    #[test]
    fn donation_ids_are_validated() {
        let d = Donation::from_parts(Some("AbC-1_x".into()), Some("P-1TC2".into()));
        assert_eq!(d.unwrap().paypal_plan_id, "P-1TC2");
        assert!(Donation::from_parts(Some("a'b".into()), Some("P-1".into())).is_none());
        assert!(Donation::from_parts(Some("abc".into()), None).is_none());
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

/// Règle du dépôt : les valeurs par défaut du code = celles de `homelab.toml`. Ce test charge le fichier, construit
/// la configuration « tout par défaut » (seuls `paths` et `urls` sont obligatoires) et échoue au premier réglage
/// qui diffère. Le 2026-09-25 il en a trouvé trois (`tracker_ratio.unlimited` sans `c411.tw`,
/// `movie_search.query_gap_secs`, `web.listen`). Seuls les réglages propres à CETTE machine sont exemptés.
#[cfg(test)]
mod toml_matches_defaults {
    const MACHINE_SPECIFIC: &[&str] = &[".paths", ".urls", ".seedbox.", ".tasks.disabled"];

    fn walk(path: &str, f: &toml::Value, d: &toml::Value, out: &mut Vec<String>) {
        if MACHINE_SPECIFIC.iter().any(|p| path.starts_with(p)) {
            return;
        }
        match (f, d) {
            (toml::Value::Table(ft), toml::Value::Table(dt)) => {
                for (k, fv) in ft {
                    match dt.get(k) {
                        Some(dv) => walk(&format!("{path}.{k}"), fv, dv, out),
                        None => out.push(format!("{path}.{k} : absent des défauts du code")),
                    }
                }
            }
            _ if f != d => out.push(format!("{path} : homelab.toml = {f}, code = {d}")),
            _ => {}
        }
    }

    #[test]
    fn homelab_toml_equals_code_defaults() {
        let raw =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../homelab.toml"))
                .expect("homelab.toml");
        let file: toml::Value = toml::from_str(&raw).unwrap();
        let mut min = toml::map::Map::new();
        min.insert("paths".into(), file["paths"].clone());
        min.insert("urls".into(), file["urls"].clone());
        let cfg: super::Config = toml::Value::Table(min).try_into().unwrap();
        let def = toml::Value::try_from(&cfg).unwrap();
        let mut out = Vec::new();
        walk("", &file, &def, &mut out);
        assert!(
            out.is_empty(),
            "homelab.toml ≠ défauts du code :\n{}",
            out.join("\n")
        );
    }
}
