# Migration Jellyfin 10.11 → 12.1

**Pourquoi** : Jellyfin 12.1 corrige côté serveur le blocage SyncPlay après un saut, avec deux changements :
- les « prêt » envoyés avec une position périmée sont corrigés (PR #17797) ;
- l'attente d'un membre silencieux est désormais limitée.

La 10.11.11 est la dernière 10.11, et la prod tourne en 10.11.8. Le contournement en place est le script
`branding/jellyfin/gc-syncplay.js` (voir `CLAUDE.md`, « SyncPlay »).

**État** : répétition faite le 03/10 sur une instance d'essai, avec une copie de la config de prod. **La bascule de la
prod n'est pas faite** : elle doit être validée par l'admin.

## Ce que la répétition a montré

| Point | Résultat sur 12.1 |
| --- | --- |
| Migration de la base (79 Mo) | 36 étapes, environ 20 s, sans erreur |
| Authentification historique | **Coupée par la migration** (`EnableLegacyAuthorization` passe à `false`) : `X-Emby-Token` et `?api_key=` → 401 |
| `Authorization: MediaBrowser Token="…"` | Accepté par la 12.1 **et** par la 10.11 |
| Extensions en version 10.11 | Toutes se chargent, sauf Intro Skipper (`NotSupported`). **L'accueil ne s'affiche jamais** avec Home Screen Sections 3.0.0 |
| Extensions en version 12.x | Les 23 actives, accueil affiché en 18 s, fiches et lecture correctes |
| SyncPlay (navigateurs) | Saut puis reprise du groupe sans intervention |
| `gc-syncplay.js` | Retrouve encore SyncPlay dans le jellyfin-web 12.1 |
| Habillage | Logo et onglets corrects. À retoucher : titre de Media Bar 3.0 qui déborde, quelques libellés restés en anglais (« Play », « Favorites », onglets de Jellyfin Enhanced) |

**Versions 12.x installées pendant l'essai** : Auto Collections 0.0.9, File Transformation 3.0.1, Home Screen Sections
3.0.2, HoverTrailer 0.4.1, InPlayerEpisodePreview 2.4.0.3, Intro Skipper 12.0.4, Jellyfin Enhanced 12.10, Media Bar 3.0.0,
Playback Reporting 19.0.0, Plugin Pages 3.0.1. JavaScript Injector 4.0.0 est déjà compatible.

**Sans version 12.x publiée, mais actives sur 12.1** : Collection Sections, Continue Watching Deduplicator, GetAvatar,
Jellysleep, NotifySync, Transcode Nag. Elles sont à surveiller : NotifySync a montré une erreur de récupération côté
client.

## Avant la bascule (sur la 10.11, sans coupure)
1. **Authentification** : passer de `X-Emby-Token` / `api_key` à `Authorization: MediaBrowser Token="…"`.
   - Cela touche 43 lignes dans 14 fichiers : le client Jellyfin de homelabd, `scripts/`, et les scripts de
     Mon compte, du tchat et de `branding/`.
   - Le tout marche déjà en 10.11.
   - Vérifier aussi Jellyseerr, Homarr et les applis des membres. Tant que ce n'est pas fait, garder
     `EnableLegacyAuthorization = true` après la migration (étape 3 de la bascule).
2. **Répéter** avec `backups/jellyfin12-test-20261003/prepare.sh` (nouvelle copie) puis `run12.sh ui` et
   `run12.sh syncplay 0|1` : bancs d'essai de l'instance de test. Repasser aussi les bancs de l'interface (`tv_home_lite.js`,
   `airplay_test.js`, `lg_audio.js`, `lang_test.js`) avec `JF_URL` sur l'instance d'essai.
3. **Retoucher le calque CSS** pour Media Bar 3.0 et les nouveaux identifiants d'onglets, puis vérifier sur des captures.

## Bascule (heure creuse, aucune lecture en cours)
1. **Sauvegarde** : `jellyfin/config` sans `data/trickplay` ni `metadata/` (la migration ne les modifie pas), avec la
   base copiée par l'API de sauvegarde de SQLite. Noter l'image actuelle : `jellyfin/jellyfin:10.11.8@sha256:1694ff06…`.
2. **Compose** : image `jellyfin/jellyfin:12.1@sha256:78d3ea1207d1322471fcac39a614f004f2ccf7e878f95ab2977d752f07e4dd7e`
   (mettre `diun/images.yml` en cohérence), puis `docker compose config --quiet` et `docker compose up -d jellyfin`.
3. **Authentification** : dès que la 12.1 répond, remettre `EnableLegacyAuthorization` à `true` dans
   `config/system.xml` puis redémarrer, sauf si l'étape « Authentification » d'avant la bascule est entièrement faite.
4. **Extensions** : installer les versions 12.x listées plus haut par le catalogue (`POST /Packages/Installed/<nom>`),
   puis redémarrer.
5. **Vérifications** :
   - `/health` ;
   - toutes les extensions « Active » ;
   - accueil, fiche et lecture, en directe comme en conversion ;
   - Mon compte, le tchat, AirPlay ;
   - `homelabctl check` et un passage de chaque tâche qui parle à Jellyfin ;
   - le journal (aucune `ERR` nouvelle).

## Retour en arrière
Remettre l'image 10.11.8 dans le compose et restaurer la sauvegarde de `jellyfin/config`. La base migrée en 12.x n'est
pas relisible par la 10.11 : la sauvegarde de l'étape 1 est indispensable.
