# Nginx Proxy Manager (NPM)

À lire avant de toucher à un hôte NPM, à sa configuration avancée, à une liste d'accès ou au site par défaut. NPM est la
seule entrée web (80/443) ; voir aussi [ARCHITECTURE.md, Sécurité des accès web](../ARCHITECTURE.md#sécurité-des-accès-web).

## 1. Règles

- **Pas d'identifiants admin NPM ici.** Le port 81 (admin NPM) est limité à l'IP de l'admin en amont + identifiants :
  **on n'y touche pas** (décision du propriétaire). **Les listes d'accès existantes ne se modifient pas** (décision de
  l'audit du 23/09) ; un nouvel outil d'admin exposé se met derrière la liste existante « admin-outils », avec l'accord
  du propriétaire.
- **Toujours éditer la base ET le fichier ensemble** (`npm/data/database.sqlite` et `npm/data/nginx/proxy_host/<id>.conf`),
  sauvegarde avant. Ne jamais afficher les colonnes `password` des tables NPM. `npm/data/database.sqlite.bak-*`
  (mots de passe des listes d'accès en clair) en 600 root ; `npm/data` reste en 755 (homelabd lit ses journaux).
- Pour imiter NPM, **rendre son gabarit avec son moteur** (`docker exec -w /app npm node --input-type=module`,
  `./lib/utils.js`, `./internal/nginx.js`) plutôt que l'écrire à la main ; exemple
  `backups/lot1-20261007/npm/apply_lot1_npm.py`.
- Juste après un `nginx -s reload`, la première requête peut encore être servie par un ancien worker : contrôler une
  seconde après, jamais sur la seule première requête.
- La liste « admin-outils » (id 2, utilisateur HTTP de l'admin, mot de passe `NPM_ADMIN_TOOLS_PASSWORD`) a été écrite en
  imitant NPM : base + `npm/data/access/2` + bloc dans `location /` de chaque site (sauvegarde `backups/npm-20260912-212419/`).
- L'interface d'onboarding de homelabd est sur l'hôte : NPM cible `172.18.0.1:8766`, pas un conteneur (avec ufw :
  `ufw allow from 172.18.0.0/16 to any port 8766 proto tcp`).
- Les journaux NPM sont en **UTC** ; ceux des Arrs contiennent leur clé API (`access_token=` des websockets de leur
  interface) : normal, journaux en 750. Les bancs portent l'agent `GcBanc/1`.

## 2. Hôte 1 (Jellyfin) : configuration avancée

Tout ce qui suit est dans la configuration avancée de l'hôte 1, **en base et dans `1.conf`** :

- **SyncPlay** : websocket, délais 3600 s, tampons coupés (jusqu'au 15/09 ils n'étaient que dans le fichier).
- **Tchat** : `location ^~ /gc-chat/` → `172.18.0.1:8766/chat/` (le `^~` est obligatoire, voir [tchat.md](tchat.md)) ;
  sauvegardes `backups/npm-*-chat`.
- **Mon compte** : `/gc-compte/` → homelabd `/compte/` (sauvegarde `backups/npm-20260920-gc-compte/`).
- **Compression** (05/10, sauvegarde `backups/npm-20261005-gzip/`) : `gzip_types` js/css/json/svg (+ `gzip_proxied any`,
  `gzip_vary on`) : le `gzip on` de NPM ne visait que le HTML et Jellyfin ne compresse plus derrière
  `X-Forwarded-Proto: https`. Plus les listes HLS (`application/vnd.apple.mpegurl`, `application/x-mpegurl`, 07/10) :
  un `main.m3u8` de 1,4 Mo → 44 Ko, seulement si le client envoie `Accept-Encoding: gzip`, segments jamais compressés.
  **À valider en lecture réelle** (iPhone, Chromecast, Tizen/webOS, AirPlay d'une télé LG).

### Chromecast

- Bloc `location ~* ^/videos/[^/]+/master\.m3u8$` (sauvegarde `backups/npm-20260927-chromecast/`), pour l'agent `CrKey`
  seulement ; le pourquoi est dans [lecture-et-transcodage.md](lecture-et-transcodage.md#5-chromecast) :
  - `AudioCodec=aac` + `aac-audiochannels=2` sur la liste maîtresse (main.m3u8 et segments en héritent) ;
  - Chromecast 1080p (`MaxWidth` ≠ 3840) : `MaxWidth=1280`, `MaxHeight=720`, `VideoBitrate` ≤ 4 000 000, H.264 ;
  - profil `high10` → `high` (`$gc_pf`, captures `gcpp`/`gcps`, tous les `CrKey`).
- `proxy.conf` passe `$request_uri`, d'où un `proxy_pass …$uri?$gc_args` propre au bloc.
- **Captures nommées obligatoires** dans ces `set` (`${1}` → « unknown "1" variable »). La ligne AAC (`$1`/`$2` dans un
  `if`/`set`) fonctionne encore ; la passer en captures nommées au prochain passage sur l'hôte 1 (base ET `1.conf`).
- Contrôles : `curl -A '…CrKey/1.56…'` sur un `master.m3u8` → `CODECS="…,mp4a.40.2"` (sans l'agent → `ac-3`) ; avec
  `MaxWidth=1920` → `RESOLUTION=…x720`, `BANDWIDTH` ≈ 4,3 M ; `h264-profile=high10&h264-level=42&MaxWidth=1920` →
  `CODECS="avc1.640029,mp4a.40.2"`.

### Garde de Home Screen Sections

- Faille de Home Screen Sections 3.0.2 (issue amont #298, sans correctif publié) : toute session pouvait enregistrer ou
  remplacer une rangée d'accueil de tout le monde (`POST /HomeScreen/RegisterSection`) et lire les rangées d'un autre
  compte en passant son `userId`. Bloc fermé le 05/10 (après `gzip_vary on;`) :
  - `RegisterSection` et `/CollectionSections/` → 403 (aucune extension ne passe par HTTP) ;
  - `/HomeScreen/Sections`, `/HomeScreen/Section/*`, `/ModularHomeViews/UserSettings` passent d'abord par `auth_request`
    sur `/UserViews/GroupingOptions?<même chaîne>` : Jellyfin juge lui-même le `userId` (le sien ou session admin) →
    autre compte 403, sans session 401 ; écritures (hors GET/HEAD) réservées aux admins ;
  - une chaîne de requête entièrement encodée n'est décodée ni par la garde ni par HSS (400 « userId requis ») ;
  - `assets.conf` passe avant : la garde suppose qu'aucune rangée n'a d'id finissant par une extension d'asset.
- Outils et sauvegarde : `backups/npm-20261005-014624-hss/` (`apply_npm_hss.py --check|--apply|--remove`,
  `test_hss_guard.sh` : 19/20, l'écart étant le cas encodé). **À retirer quand l'auteur publiera un correctif.**

## 3. Autres hôtes

- **Hôte 20** (page publique `/premium` et webhook PayPal) : **sans liste d'accès** (PayPal doit joindre
  `/paypal/webhook`).
- **Page de don** (sous-domaine `don.`) : la configuration avancée ne laisse passer que `/don` (`/` redirige, le reste
  404).
- **`/accounts` et `/recherche`** (hôte d'onboarding) : liste « admin-outils » dans la configuration avancée
  (`location /accounts`…) **et** session d'administration homelabd.
- Hôtes 5 (qBittorrent : en-têtes Referer/Origin) et 14 (File Browser : délais 3600 s, `proxy_buffering off`, cache,
  HSTS `includeSubDomains`) : leur fichier diverge de la base (la base porte `proxy_max_temp_file_size 0`). **Ne pas les
  enregistrer dans l'interface NPM ni lancer `regenerate-config`** avant d'avoir reporté ces lignes en base, sinon elles
  sont perdues.

## 4. Site par défaut

- **Site par défaut = « 404 Page »** (07/10) : setting `default-site` = `404` et `/data/nginx/default_host/site.conf` rendu
  par le moteur de NPM (gabarit `/app/templates/default.conf`). L'ancien serveur « Congratulations » incluait
  `assets.conf` (`proxy_pass` vers 127.0.0.1:80) : chaque `.js/.css/.ico` demandé par l'IP nue bouclait jusqu'à « 512
  worker_connections are not enough » (13 épisodes du 27/09 au 05/10, des robots). **Ne jamais le remettre.** Retour :
  `backups/lot1-20261007/npm/LISEZMOI.txt`.

## 5. NPM 2.16 (lot 3, 09/10)

- **NPM 2.16.0** (OpenResty 1.31.1.1) depuis le 09/10 : CVE-2026-42945, -8711, -9256 et la RCE de l'interface corrigées.
  Préparation, retour arrière et sauvegarde : `backups/lot3-20261008/npm/`.
- **Jeton DuckDNS plus gardé sur le disque** : NPM l'écrit depuis sa base le temps d'un certbot, puis l'efface. Un
  renouvellement à la main doit faire de même (recette dans `backups/lot3-20261008/npm/apply.sh`).
- **Vers le 27/10** : vérifier le premier renouvellement sous 2.16 (`npm/data/logs/letsencrypt.log`, nouvelle échéance
  après le 26/11) et que `credentials-1` a disparu ensuite ; supprimer alors `backups/lot3-20261008/npm/run-*`.
- **Contrôle complet** : `backups/lot3-20261008/npm/check.sh` (production par défaut ; `--ct`, `--ports`, `--data` pour
  une instance d'essai) : 15 hôtes, garde HSS, Chromecast, gzip, tchat, Mon compte, websocket, site par défaut, journal lu
  par `hls_loop_watch`. Il remplace `hosts_check.sh` + `hls_check.sh` + `test_hss_guard.sh`.
- La ligne Chromecast AAC de `1.conf` (`$1`/`$2` dans un `if`/`set`) marche toujours sous nginx 1.31 ; la passer en
  captures nommées reste une simple cohérence, en base **et** dans `1.conf`, au prochain passage sur l'hôte 1.
