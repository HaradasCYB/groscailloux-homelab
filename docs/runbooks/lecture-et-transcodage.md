# Lecture, transcodage, langues et sous-titres

À lire avant de toucher aux réglages de lecture de Jellyfin, au tmpfs de transcodage, aux tâches planifiées de
Jellyfin, aux règles Chromecast de NPM, aux langues des comptes ou aux sous-titres, et pour diagnostiquer une lecture
qui saccade ou « charge à l'infini ». Tâches liées : [AUTOMATION.md](../AUTOMATION.md) (`playback_canary`,
`hls_loop_watch`, `subtitle_sync`, `original_language`, `playback_limit`).

## 1. Ce qui limite vraiment (mesures)

- **Un seul transcodage 1080p à la fois** (pas de GPU ; réglages Jellyfin `superfast`, 4 threads) : 1 flux 1,42× ;
  2 flux 0,82×/0,85× ; 3 flux 0,60×/0,69×. Dès deux transcodages simultanés, tout le monde passe sous le temps réel.
- **Le codec source n'est pas un critère de charge** (18/09) : un transcodage 1080p tourne à 1,85× que la source soit
  H.264 ou HEVC ; le coût est dans l'**encodage x264** (décodage seul : H.264 6,25×, HEVC 6,74×). Sur 7 jours, les 19
  transcodages venaient tous de sources H.264, alors que le HEVC faisait 53 % des épisodes : les clients lisent le HEVC
  en direct. Sur 30 jours (26/09) : 13 % des lectures HEVC réencodées contre 20 % des H.264 (plafonds de débit
  Chromecast/iOS ; Safari réencode un HEVC en MKV, les vieux Chromecast aussi). D'où l'ordre x265 > x264 > AV1 du choix
  des releases ([arrs-et-indexeurs.md](arrs-et-indexeurs.md#4-choix-dune-release-choose-best_movie_release-recherche)).
- **Lecture directe : 65 % des lectures et des heures** (mesuré le 04/10 sur 30 j, lectures ≥ 60 s ; l'ancien « 98 % »
  était faux, la baisse date du 14 au 26/09) : remux 18 % du temps, son seul 7,5 %, vidéo 9,4 %. Le Chromecast fait 63 %
  des conversions vidéo (sources HEVC + règle NPM 720p) ; l'appli iOS remuxe presque tout (MKV) ou convertit le son
  (DTS) ; Jellyfin Desktop, Android TV et JellyWatch lisent en direct.
- Pas de plafond de débit par utilisateur ni de `RemoteClientBitrateLimit` : ils forceraient des transcodages. Le
  plafond est à l'acquisition (110 Mo/min, `max_gb_per_*`).

## 2. Fenêtre des tâches lourdes et options interdites

- **Aucune tâche lourde (trickplay, analyse de segments, analyse complète, extraction) entre 13 h et 05 h**. Fenêtre en
  semaine : analyse de la médiathèque 05:00, Intro Skipper 05:30 (`ProcessThreads 2`, `MaxParallelism 1`,
  `ScanCommercial false`), images de chapitre 06:30 en `P480` (extraction désactivée par bibliothèque : tâche vide),
  `LibraryScanFanoutConcurrency 2`, normalisation audio 07:00, `original_language` 07:30–11:30 ; tout est fini vers 08:30.
- **Trickplay** : plus généré (déclencheur retiré le 25/09 : tout arrive sur la seedbox, la tâche relisait chaque titre
  par le lien et s'arrêtait à 6 h sans finir) ; les vignettes existantes restent affichées. Retour : `POST
  /ScheduledTasks/<id>/Triggers` avec `backups/jellyfin-trickplay-20260925/triggers-before.json`. Jamais pendant une
  analyse (il lit tout le fichier).
- **Aucune option qui lit la vidéo à l'ajout d'un titre** : Intro Skipper `AutoDetectIntros`, source d'images « Screen
  Grabber », `SaveLocalMetadata`/NFO. Sur les dossiers seedbox, chaque lecture passe par le lien et fait attendre les
  spectateurs.
- **Médias en écriture pour Jellyfin** (`/media` et `/seedbox` sans `:ro`, rclone sans `--read-only`) **seulement** pour
  le bouton « Supprimer ». Aucune option qui écrit dans les dossiers médias : `SaveLocalMetadata`,
  `SaveSubtitlesWithMedia`, trickplay à côté du média, tous à `false` ; `MetadataSavers` vide sur Films et Séries (sinon
  des `.nfo`). Une suppression dans Jellyfin est suivie par `deletion_cleanup`.
- `cpu_shares` : Jellyfin et NPM 2048, services de fond 512 (duckdns 256), homarr `cpus: 1`. **Garder 512 sur tout
  nouveau service de fond.** Pas de quota `cpus:` sur Jellyfin (il plafonnait à 4 vCPU : deux transcodages tombaient à
  0,82×).
- Bandes-annonces de HoverTrailer et Media Bar : YouTube (zéro CPU serveur) ; seul `EnableThemeVideoFallback` touchait
  Jellyfin, mis à `false`.

## 3. Conteneur Jellyfin et tmpfs de transcodage

- Source de vérité : `docker-compose.yml`, service `jellyfin` : tmpfs `/cache/transcodes` de **4 Go**, `mem_limit 6g`
  (4 Go + le tmpfs), healthcheck explicite `start_period 180s` (sinon une migration de base au démarrage était
  redémarrée par `stack_health`), port publié sur `127.0.0.1:8096` seulement.
- Réglages d'encodage : `ThrottleDelaySeconds 180`, `SegmentKeepSeconds 300` (retour arrière de 5 min sans relance de
  ffmpeg ; 720 envisagé, palier B), `EnableSegmentDeletion`.
- **Tmpfs plein = « chargement infini »** : Jellyfin n'efface pas tous les segments en fin de job ; plein, ffmpeg écrit des
  segments **vides** servis en 200 (`[Length 0]` dans le journal NPM), le lecteur les télécharge à toute vitesse et
  n'affiche rien (la lecture directe passe, tout remux ou transcodage échoue). Le tmpfs se remplit **par paliers à la fin
  de chaque job** (~400 Mo laissés par un 1080p) ; un job actif occupe ~480 s × son débit. Un job actif plus gros que le
  tmpfs ne se purge pas (remux 4K 60 Mbit/s ≈ 3,6 Go), d'où 4 Go.
  - Diagnostic **d'abord** : `docker exec jellyfin df -h /cache/transcodes` ou `scripts/jellyfin-transcodes-purge.sh --check`.
  - Purge : `scripts/jellyfin-transcodes-purge.sh` par les unités versionnées
    `systemd/jellyfin-transcodes-purge.{service,timer}` (**chaque minute**, installées par `homelabctl install`) ;
    routine = jobs sans ffmpeg actif vieux de plus de 2 min (un job terminé n'est jamais réutilisé) ; **urgence**
    automatique dès 85 % (tout ce qui n'a pas de ffmpeg actif, puis les segments de plus de 10 min des jobs actifs) +
    message Discord admin ; `--urgent` à la main ; `--force` refusé tant qu'un ffmpeg tourne.
- **Mise en pause des conversions (throttling) : elle marche** (10.11 et 12.1, vérifié le 04/10). Piège de lecture : le
  patch de pause de jellyfin-ffmpeg exclut le temps de pause de `speed=` et `elapsed=` et n'écrit rien pendant une pause ;
  un job bridé ressemble à une conversion continue à 6×. Chaque « Transcoding is paused » = une pause. Avance réelle =
  dernier segment écrit (`Opening … N.mp4` du journal FFmpeg) − dernier `hls1/main/N.mp4` demandé (journal NPM, UTC) :
  60 à 72 segments de 3 s attendus (seuil 180 s). Un job inactif reste vivant tant que le client envoie
  `/Sessions/Playing/Ping`.
- **Compter les ffmpeg** : `pgrep -fc '[j]ellyfin-ffmpeg/ffmpeg'` (avec crochets ; sans eux, `pgrep -f` compte aussi le
  shell qui porte le motif).

## 4. Diagnostiquer une lecture qui saccade

- Jellyfin choisit **une seule qualité par session** (pas d'ABR). En « Auto », un 1080p part tel quel (~5 Mbit/s) ; si le
  débit du membre baisse, la lecture cale et jellyfin-web relance le flux toutes les ~30 s, ce qui aggrave le retard
  (en 12.x, un journal `FFmpeg.*` par relance). À 1,5 Mbit/s, la même lecture tient.
- Où regarder : taille et cadence des segments dans `npm/data/logs/proxy-host-1_access.log` (horodatage **UTC** ; les
  bancs ont l'agent `GcBanc/1` : `grep -v 'GcBanc/'`), journaux ffmpeg `jellyfin/config/log/FFmpeg.*`, croissance du
  cache dans `journalctl -u homelab-seedbox-mount`, `docker_container_net` et `rclone_vfs`/`rclone_core` (Telegraf lit
  l'API RC de rclone) dans InfluxDB.
- Alertes : `hls_loop_watch` (même segment redemandé ≥ `threshold` 20 fois en 5 min, ou plus de
  `max_jobs_per_item_hour` 30 lancements de ffmpeg pour un titre dans l'heure) ; `playback_canary` (transcodage réel de
  2 segments toutes les 15 min, alerte au premier échec, `state.canary`).
- **Aide à la qualité** : `branding/jellyfin/gc-quality-helper.js` (« Groscailloux Qualité ») surveille la progression de
  l'image (les événements `waiting` ne suffisent pas, hls.js les absorbe) et propose au 3ᵉ blocage le palier sous
  2 Mbit/s **par le menu du lecteur** (la commande `SetMaxStreamingBitrate` n'est pas gérée par le client web ; écrire
  `maxbitrate-Video-*` ne change pas la lecture en cours). **Ce palier ne vaut que pour la lecture en cours** :
  jellyfin-web le mémorise par appareil (`maxbitrate-Video-<réseau>` + `enableautobitratebitrate-Video-<réseau>` à
  `false`) ; le script note ce qu'il a posé (`gc-quality-lowered`) et remet « Auto » à la sortie du lecteur, sauf si le
  membre a changé la qualité entre-temps. Sous 12.1 : un saut ou un rechargement ouvre 8 s de calme (`QUIET_MS`) ;
  bandeau en haut sur PC, tablette et téléphone ; la note du palier attend que jellyfin-web l'ait écrit (≤ 10 s).
  Bancs : `backups/quality-tests-20260916/` (phases 5–6), `backups/jellyfin12-test-20261003/t8_quality.js`.
- **Plusieurs appuis rapprochés sur l'avance** = autant de relances de ffmpeg : avancer d'un geste.

## 5. Chromecast

- Google Cast n'existe que dans **Chrome sur ordinateur** et l'**appli Android du Play Store** (pas celle de F-Droid,
  pas Chrome sur Android). Récepteur du serveur : `CastReceiverApplications` = Stable `F007D354`, posé sur les comptes.
- Règles NPM (bloc `master.m3u8` de l'hôte 1, agent `CrKey` seulement, détail dans [npm.md](npm.md#chromecast)) :
  - **son AAC stéréo imposé** : le récepteur déclare l'AC3/E-AC3, Jellyfin copiait le son et la télé restait sur
    « Ready to cast » après le premier segment ;
  - **720p, H.264, 4 Mbit/s au plus** pour un Chromecast 1080p (`MaxWidth` ≠ 3840 ; choix du propriétaire le 29/09) : en
    1080p niveau 4.2 à 18 Mbit/s, il chargeait 1 à 2 segments puis restait figé ; en 1080p la conversion tournait à
    ~1,3× et chaque avance coûtait 10 à 15 s. Un Chromecast 4K n'est pas touché ;
  - **profil `high10` → `high`** (07/10, tous les `CrKey`, 4K compris) : le récepteur demande `high10` en premier et
    Jellyfin annonçait alors `avc1.4240xx` (Baseline) alors que ffmpeg encode en High. Rien ne prouve encore que ce soit la
    cause des gels : **à valider en séance réelle**.
- « Transcode Nag » : `ExcludedClientPatterns = ["Chromecast"]` (sauvegarde `backups/transcode-nag-20260929/`) ; ce n'était
  pas la cause des blocages, gardé car le message n'a pas de sens sur une télé.
- Un Chromecast figé peut ne plus rien demander au serveur : le débrancher 10 s.

## 6. AirPlay, télés LG, applis TV natives

- **Sous-titres en AirPlay** : jellyfin-web ne déclare que des sous-titres `External`, dessinés par la page ; AirPlay
  n'envoie que le flux HLS. `gc-airplay.js` (appareils Apple seulement) ajoute `{Format: vtt, Method: Hls}` **en tête**
  des `SubtitleProfiles` de chaque `PlaybackInfo` (XHR du SDK et fetch) : en lecture HLS (remux ou conversion), le
  serveur met les sous-titres dans le flux (`#EXT-X-MEDIA TYPE=SUBTITLES`). Lecture directe : non couverte. Banc
  `backups/lg-tv-20260929/sub_airplay.js`. **Limite** : le récepteur AirPlay intégré à une télé LG télécharge
  `subtitles.m3u8` et `stream.vtt` mais n'affiche rien : sur une LG, utiliser l'appli Jellyfin de la télé.
- **Télés LG (webOS) : changement de piste audio** : jellyfin-web (webOS ≥ 4) bascule la piste dans le lecteur de la télé
  sans rien demander au serveur, et sur les LG ça ne fait rien. `gc-tv.js`, sur agent webOS seulement : `audioTracks`
  masqué sur la **vidéo insérée dans la page** (jamais sur le prototype : sinon Jellyfin remuxe aussi la VF par défaut),
  et toute `PlaybackInfo` pour une autre piste que celle par défaut part avec `EnableDirectPlay=false` → remux, image et
  son copiés. Bancs `backups/lg-tv-20260929/` (`run.sh lg_audio.js "tv:0"`, télé émulée ; le Chromium du banc ne décode
  pas le HEVC : seules les décisions du serveur sont mesurées).
- **Applis TV natives** (Android TV, Fire TV Stick, JellyWatch) : 2ᵉ usage de la maison, **100 % en lecture directe** ;
  elles ne chargent **pas** jellyfin-web (ni calque CSS, ni script injecté, ni rangées Home Screen Sections). Tout ce qui
  les améliore passe par les **métadonnées et les réglages de compte**. Leur disposition d'accueil n'est pas stockée côté
  serveur (`TvHome` vide). Les segments d'Intro Skipper leur donnent « Passer l'intro » ; inutile d'ajouter TheIntroDB.

## 7. Langue audio et sous-titres des comptes

- **À l'onboarding** (`[accounts] audio_language = "fre"`, `subtitle_language = "fre"`, `subtitle_mode = "Smart"`) et
  `PlayDefaultAudioTrack = false` **dans les deux modes** : à `true`, Jellyfin prend la piste « par défaut » du fichier
  AVANT la langue préférée (un animé en VF en mode VO). Rattrapage des comptes existants le 21/09 (sauvegarde
  `backups/jellyfin-language-20260921/`).
- **Mode « VO » de Mon compte** (`POST /compte/api/language`) : audio = `[accounts] vo_audio_language` (`jpn` : les animés
  sur tous les appareils, applis TV comprises) + sous-titres `Always` ; dans les clients web, le script Mon compte
  bascule au démarrage d'un titre parti en français sur la piste de la **langue d'origine** (TMDB via Jellyseerr,
  `GET /compte/api/original`, anglais si inconnue ; rien pour une origine française ni l'audiodescription) par
  `SetAudioStreamIndex` envoyé à sa propre session, une fois par titre. Banc `backups/jellyfin-vo-20260925/`.
- Choisir un mode coupe aussi `RememberAudioSelections`/`RememberSubtitleSelections` du compte : une piste retenue pour
  un titre (`UserData.AudioStreamIndex`, invisible par l'API) passait avant la langue du compte.
- Un membre qui bascule l'audio en VO **en cours de lecture** garde la piste de sous-titres choisie au départ (« Forcé »
  en mode Smart) : c'est jellyfin-web. « Toujours en VO » sélectionne la piste complète d'office.
- **Depuis Jellyfin 12.1, les sous-titres externes ont l'index 0** et les flux internes sont renumérotés : un index de
  piste lu avant la 12.1, ou calculé d'après ffprobe, ne correspond plus.

### Langue d'origine (`original_language`, lot 4, 08/10) et préférence native

- Tâche `original_language` (10 min, fenêtre `07:30–11:30`, `max_movies_per_run` 4 ou `max_series_per_run` 1) : écrit la
  langue d'origine TMDB (lue par Jellyseerr) sur les **films**, puis sur les séries si `[tasks.original_language]
  series = true`. **Décision en attente du propriétaire : `series = false`** tant qu'il n'a pas accepté que le POST
  d'une série réécrive la classification de toutes ses saisons et épisodes (~1 458 épisodes et 130 saisons recevraient
  celle de leur série, valeur qu'ils héritaient déjà). Une fois accepté : `series = true` + redémarrage de homelabd.
- Règles : `POST /Items/{id}` avec le seul corps `update_body`, jamais de Refresh, jamais une fiche en lecture, rien
  pendant une analyse de la médiathèque ; les épisodes héritent de leur série et ne sont jamais écrits ; une fiche écrite
  (ou une écriture partie sans réponse nette, `posted`) n'est **jamais** réécrite d'office ; chaque écriture (avant,
  après, enfants dont la classification vide a reçu celle de la série) est dans `state.original_language`.
- Pièges Jellyfin 12.1 (vérifiés dans le code et sur une instance d'essai) : renvoyer la fiche entière n'est pas neutre
  (ordre TMDB des personnes perdu ; 500 sur une fiche avec vignettes Trickplay) ; sur une série, UpdateItem réécrit chaque
  saison et chaque épisode rangé dans une saison (`OfficialRating` sauf verrou, `CustomRating` sans exception), d'où le
  refus `children_differ` ; `GET /Items` avec un compte regroupe par `PresentationUniqueKey` (une série dans deux
  dossiers n'est listée qu'une fois ; lire ses enfants avec ET sans compte puis filtrer sur `SeriesId`).
- Retour arrière : `backups/original-language-20261008/rollback.py` (complet : `enabled = false`, redémarrage, `--dry-run`
  puis sans option ; `--only <id>` tâche active). Une écriture « non confirmée » se complète à la main (éditeur de
  métadonnées). Banc : `prepare-ol-test.sh`, `seed-fix.py`, `mk-e2e.py`, `e2efix-check.py` dans le même dossier.
- **Préférence audio native « Langue d'origine »** (essai du 08/10 sur une instance 12.1 jetable, pas encore sur de vrais
  appareils) : `UserConfiguration.AudioLanguagePreference = "OriginalLanguage"` (option « Langue d'origine » de
  jellyfin-web 12.1), avec `PlayDefaultAudioTrack = false` et `SubtitleMode = Always`. Le serveur prend alors la piste de
  la langue d'origine pour tous les clients (jpn pour un animé, eng/ita pour un MULTi, VF principale et jamais l'AD pour un
  film français) ; sans métadonnée, la piste par défaut (VF). **Piège serveur** (12.1 = master) : une piste d'origine
  marquée « Original » (drapeau Matroska) mais pas par défaut est perdue par les clients qui n'imposent pas d'index
  (Android TV, Fire TV : VF) ; jellyfin-web n'est pas touché, `gc-tv.js` (LG) le gère tel quel. Plan, chaque étape sur
  décision du propriétaire : remplir OriginalLanguage (Anime d'abord) → signaler le bogue amont
  (`MediaSourceManager.SetDefaultAudioStreamIndex`) → basculer le mode VO (`vo_audio_language = "OriginalLanguage"`,
  `language_mode` reconnaissant `jpn` et `OriginalLanguage`, libellé « VO (langue d'origine) ») → garder le script VO de
  Mon compte comme filet quelques semaines.

## 8. Sous-titres

- **Sous-titres incrustés d'un fichier seedbox = 2 à 12 minutes d'extraction par le lien** : Jellyfin relit tout le
  fichier ; le lecteur attend ou abandonne (499). Réglé par `subtitle_sync` : extraction **sur la seedbox** à codec
  identique (`.fr.default.ass`, `.fr.forced.ass`, `.fr.hi.ass`, `.fr.srt`, SRT sans panneaux dérivé de l'ASS), puis
  FullRefresh de la fiche (seul moyen de voir un fichier annexe). Détail :
  [AUTOMATION.md, subtitle_sync](../AUTOMATION.md#subtitle_sync--5-min).
- **Ne jamais convertir l'ASS en SRT pour l'usage principal** : les lignes de panneaux (titre d'épisode, « PROCHAIN
  ÉPISODE ») se retrouvent en bas de l'image comme des mentions malentendants.
- **Bazarr** = celui de la seedbox (profil « Français (+anglais) », `subsync` off, `use_embedded_subs = true`), rien sur
  le VPS ; compte OpenSubtitles saisi dans son interface.
- **Taille des sous-titres SRT** : réglage jellyfin-web **par appareil** (`<userId>-localplayersubtitleappearance3`,
  `textSize` : smaller .8em, small inherit, normal 1.36em, large 1.72em, larger 2em, extralarge 2.2em), posé « large »
  par défaut par Mon compte et modifiable dans Mon compte ; l'ASS a ses propres styles. jellyfin-web pose la taille en
  style **inline** sur `.videoSubtitlesInner` une seule fois, à la création de l'élément : le script pose une règle
  `#gc-sub-size` en `!important` sur `.videoSubtitlesInner`, `.videoSecondarySubtitlesInner` et `video::cue`, relue à
  chaque tour de boucle.
- **Jellyfin Enhanced impose sa taille en `vw`** (1.2vw par défaut, ~5 px sur iPhone en portrait) par une règle
  `video.htmlvideoplayer::cue` et un style inline `!important`. **Décision du propriétaire (04/10) : sur téléphone et
  télé, la taille de Mon compte l'emporte** (`applySubtitleSize` de `compte/app.js` : règle
  `video.htmlvideoplayer.htmlvideoplayer::cue` et style inline remis par un `MutationObserver`) ; téléphone et tablette
  = `clamp(14px, 4.2vmin, 22px)` × coefficient ; **télé = `4.8vh` × coefficient** (52 / 65 / 84 px en 1080p ; banc
  `backups/lg-tv-20260929/t6_run.sh t6_subs.js "tv:0"`). Sur PC, Jellyfin Enhanced garde la main.
