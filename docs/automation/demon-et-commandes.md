# Démon, commandes et clients

Fonctionnement de homelabd et homelabctl, pages d'administration, commandes diverses, clients HTTP, hook gluetun,
tests. Sommaire de toutes les tâches : [AUTOMATION.md](../AUTOMATION.md).

## Fonctionnement du démon

`homelabd` (systemd `homelabd.service`, user `deploy`) exécute chaque tâche dans sa propre
boucle tokio, à l'intervalle de `homelab.toml`, avec un jitter initial de 0–30 s et un
timeout de 10 min par passage. Un mutex sérialise toute mutation de qBittorrent, un autre les
onboardings. `HOMELABD_DRY_RUN=1` (ou `--dry-run`) : aucune écriture, les tâches loggent ce
qu'elles feraient. Les logs vont dans le journal : `journalctl -u homelabd -f`.

**Journal et état (2026-10-07)** : `run_done`, `run_failed` et `run_timeout` donnent la durée (`ms`). Un passage
**calme** (réussi, sans action, résumé identique au passage réussi précédent) écrit son `run_done` en `debug` :
c'étaient 88 % des lignes (4 344 lignes entre 01:15 et 10:25 le 07/10, 312 le 08/10) ; `RUST_LOG=debug` pour tout
voir. L'état est sérialisé en mémoire puis écrit en un appel par un écrivain unique ; la simple tenue (début d'un
passage, fin d'un passage calme) part avec la sauvegarde suivante (`update_lazy`), au plus tard au tour suivant de
`StateStore::run_lazy_flusher` (60 s, lancé par homelabd), ou à l'arrêt. `/health` répond 503 `scheduler_stale` quand
plus aucune boucle de tâche n'a tourné depuis 20 min (max(2 × 600 s, plus petit intervalle + 600 s)) et donne la version
(`git describe`, voir `build.rs`).

`homelabctl run <tâche> [--dry-run]` exécute la même implémentation une fois ;
`homelabctl status` lit `state/homelabd.json` (dernier passage, erreurs, items suivis) : la tenue d'un passage sans
rien de neuf y arrive dans la minute, d'où `running? depuis HH:MM` (début récent sans fin enregistrée : en cours ou fini
depuis moins d'une minute) et `interrompu? JJ/MM HH:MM` (début plus vieux que `RUN_TIMEOUT`, 600 s : daemon arrêté en
plein passage) ; `/status.html` lit la mémoire du démon. Sous chaque tâche, une ligne ↳ donne les erreurs récentes
(« 2 aujourd'hui, 5 sur 7 j · dernière le JJ/MM HH:MM : message »), seulement si la dernière date de moins de 7 jours
ou si la tâche échoue en ce moment : `errors` est un total depuis l'origine.

Configuration : `homelab.toml` (toute clé inconnue est refusée ; `paths.downloads` doit
exister sinon le daemon refuse de démarrer — plus jamais un watcher mort en silence) ; `paths.downloads_in_container`
est ce même dossier vu des conteneurs qBittorrent, Sonarr et Radarr du VPS (chemin donné aux Arrs par `auto_import`,
`torrent_import` et `tba_bypass`).
Secrets : `.env` via `EnvironmentFile`.

## Pages d'administration (2026-09-23, portes du 2026-10-07)

`crates/homelabd/src/admin_auth.rs` + `client_addr.rs`. Session par cookie (`/connexion`, voir [runbooks/homelabd.md](../runbooks/homelabd.md#5-pages-dadministration-crateshomelabdsrcadmin_authrs-client_addrrs)) ; en plus :

- **Adresse du client** : dernier saut de `X-Forwarded-For`, seulement si le pair TCP est dans `[web] trusted_proxies`
  (172.18.0.0/16) et n'est pas une adresse de l'hôte (127.0.0.1, 172.18.0.1 : `bind` UDP d'essai, en cache). Avec
  `[web] trusted_proxy_container = "npm"`, seul le pair à l'adresse actuelle de ce conteneur est « confirmé » :
  adresse relue en tâche de fond (`docker inspect --type container`, toutes les 60 s, et 10 s après un pair inconnu du
  réseau, par exemple NPM recréé) ; les requêtes lisent un instantané et n'attendent jamais Docker (sauf la toute
  première, 4 s au plus). Docker muet ou conteneur vide : le réseau seul décide, pour les limites par adresse.
- **IP de la maison** (`HOMELABD_ADMIN_TRUSTED_IPS`) : session d'office seulement depuis un NPM **confirmé** ; Docker
  muet ⇒ formulaire `/connexion` (le cookie d'un an couvre l'entre-deux), une seule ligne « docker inspect en échec »
  par panne.
- **Hôte** : les chemins d'administration (`/`, `/accounts*`, `/recherche*`, `/status`, `/status.html`, `/onboard`,
  `/connexion`, `/admin`, `/admin/*`) répondent 404 si `Host` n'est ni l'hôte de `ONBOARD_PUBLIC_URL` ni `localhost`
  ou une adresse locale ou privée ; sans `ONBOARD_PUBLIC_URL`, aucun filtre (avertissement au démarrage). Les pages
  publiques (`/premium*`, `/inscription`, `/bienvenue`, `/premiers-pas`, `/guide`, `/paypal/webhook`, `/don`, `/chat`,
  `/compte`, `/health`) restent servies partout.
- **API de la CLI** (`/admin/*`) : pair local (127.0.0.1, ::1) sans `X-Forwarded-For` seulement, sinon 404 ; un refus
  = une ligne `warn` par adresse et par 15 min (100 adresses au plus), les suivants en `debug` ; jeton incorrect =
  `warn`. `homelabctl` appelle `http://127.0.0.1:<port de listen>` avec `x-onboard-token`.
- **Échecs** : jetons comparés à temps constant partout (`admin_auth::token_matches`) ; un POST sans session sur un
  chemin d'administration compte ses 401 comme `/connexion` (10 par IP et par 15 min, puis 429) ; le plafond global
  (100) ne bloque jamais un appel local. `POST /onboard` est fermé si aucun jeton n'est configuré.
- **Jeton absent du HTML** : la couche remplace le jeton par un jeton de formulaire (HMAC du jeton) dans les pages, et
  fait l'inverse dans un POST qui a une session. Contrôle : le code source de `/accounts` ne contient plus le jeton.

## Autres commandes

- `homelabctl vpn status|on|off` : voir [DEPLOY.md](../DEPLOY.md) (profils compose).
- `sudo homelabctl backup` : voir [DEPLOY.md](../DEPLOY.md) ; aussi `homelab-backup.timer`.
- `homelabctl check` : ping Sonarr, Radarr, qBittorrent, Jellyfin, Jellyseerr ; présence SMTP,
  token, dossier surveillé ; Arrs et qBittorrent de la seedbox, montage ; `diun/images.yml` en contrôle strict (✗ et
  code de sortie ≠ 0 si invalide).
- `sudo homelabctl install` : copie `systemd/*` dans `/etc/systemd/system`, enable ; pose aussi
  `systemd/journald-homelab.conf` (puis `sudo systemctl restart systemd-journald`, à lancer soi-même).

## Clients HTTP (2026-10-07)

- **GET coupé rejoué une fois** (`clients::SendRetry::send_retry`) : seulement un GET, seulement sur une coupure de
  transport (`is_request() || is_connect()`, jamais un délai dépassé), une seule fois ; ligne `info` « GET coupé :
  nouvelle tentative (une seule) » avec le chemin (sans requête) et la chaîne de causes, sans l'URL. Appliqué aux
  clients Arr, Bazarr, Jellyfin, Jellyseerr, Prowlarr et qBittorrent, **sauf** les GET à effet : recherches,
  téléchargements et magnets Prowlarr (quota C411), `release` des Arrs, `get_bytes` de Jellyfin (la playlist du canari
  démarre une conversion). PayPal n'est pas concerné. Vu le 08/10 : un GET de Radarr seedbox rejoué après
  « connection closed before message completed ».
- **Jellyseerr** (Node, keep-alive 5 s) a son propre client avec `pool_idle_timeout` 4 s ; pas de délai global (4 s
  imposerait une poignée de main TLS vers la seedbox à presque chaque requête).
- **qBittorrent 5.2** : le cookie de session `QBT_SID_<port>` est accepté comme `SID` (nom renvoyé tel quel).
- **Erreurs** : journalisées avec toute leur chaîne (`format!("{e:#}")`) ; une URL qui porte un secret (webhook
  Discord, Jackett `apikey=`, liens Prowlarr, playlists de conversion Jellyfin `ApiKey=`) est retirée avant le contexte
  (`reqwest::Error::without_url`).

## Hook gluetun (reste en shell)

`hooks/qbit-update-port.sh` tourne **dans** le conteneur gluetun (busybox, pas de curl), appelé
par `VPN_PORT_FORWARDING_UP_COMMAND` avec le port forwardé : attend que qBittorrent réponde
sur 127.0.0.1:8080 puis `setPreferences {listen_port}`. Log sur stdout de gluetun.

## Tests

`cargo test` (unitaires : classification, politiques de tracker, décisions stuck/disk, filtre
TBA, règle monitor_sync, candidats du poller, validateurs d'onboarding, état, config).
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. CI : `.github/workflows/rust.yml`.
