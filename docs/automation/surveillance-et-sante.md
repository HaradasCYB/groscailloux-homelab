# Surveillance et santé

Auto-réparation du stack, surveillances quotidiennes, canari de lecture, alertes admin et Discord. Sommaire de toutes
les tâches : [AUTOMATION.md](../AUTOMATION.md).

## stack_health — 5 min (première passe ~30 s après le démarrage de homelabd)
Auto-réparation du stack. `docker compose config --services` donne les services attendus (moins
`ignore`), `docker compose ps -a --format json` leur état.
- absent / `exited` / `created` → `docker compose up -d <svc>` (respecte `depends_on`) ;
- `unhealthy` depuis ≥ `unhealthy_grace_secs` (2 min) → `docker compose restart <svc>` ;
- sonde applicative (`[[tasks.stack_health.probes]]`) en échec → restart, seulement si les
  services de `requires_healthy` sont healthy (sinon « waiting ») ;
- jamais deux restarts du même service en moins de `restart_cooldown_secs` (10 min).
Sonde par défaut : Guacamole, `POST /guacamole/api/tokens` avec de faux identifiants → attendu
`403` + `INVALID_CREDENTIALS` ; un `500` signifie que l'extension MySQL est morte (guacdb pas
prêt au démarrage : l'image ne l'attend jamais). Cette sonde compte comme un échec de login
côté Guacamole : garder l'intervalle ≥ 5 min (extension `ban` : 5 échecs / 300 s). Une réponse
`429` (throttling) est considérée non concluante : ni échec ni redémarrage.
Pour arrêter un service volontairement : l'ajouter à `ignore` avant, sinon il sera relancé.
Résumé : `expected=21 running=21 healthy=16 started=[] restarted=[] waiting=[] failed=[]`.
Relance **ratée** (`up`/`restart`/`recreate` en échec, ou `post_exec` raté après la relance) : message Discord admin
« Relance en échec », au plus un par service et par `alert_every_secs` (3600 ; mémoire `state.stack_failures`).
Avant le 2026-10-07, seule une relance réussie prévenait.

**Sonde qBittorrent vue de l'hôte** (2026-10-02) : `http://localhost:8080/api/v2/app/version`. qBittorrent partage le
réseau de gluetun ; quand gluetun est recréé, il reste « healthy » (sa sonde interne passe) mais injoignable. Sonde en
échec alors que gluetun est sain → `action = "recreate"` (`up -d --force-recreate --no-deps qbittorrent`), puis
`post_exec` dans gluetun : le hook `qbit-update-port.sh` repose le port transféré. Toute sonde accepte maintenant
`action` (`restart` par défaut, ou `recreate`) et `post_exec_in` / `post_exec`.

## seedbox_health — 5 min

Santé de la seedbox vue du VPS : `ping` de Sonarr et Radarr, version de qBittorrent, historique Bazarr, lecture du
montage rclone (`mount_check`). Un service injoignable depuis `alert_after_mins` (10) → **mail + Discord admin**, une
seule fois par panne ; un second message à son retour (« panne d'environ 16 h 25 »). Décision pure `decide`, testée.
La relance, elle, est faite **sur la seedbox** par son crontab : `scripts/seedbox/homelab-apps-watch.sh` (copié dans
`~/.local/bin/`), `@reboot` (toutes les applis, 2 min après le démarrage) et toutes les 5 min (une appli qui ne
répond pas deux fois de suite → `app-<x> start`, sans effet si elle tourne déjà) ; journal
`~/.local/state/homelab-apps-watch/watch.log`, `--dry-run` pour voir. Origine : le 01/10, l'hôte de la seedbox a
redémarré et Sonarr, Radarr, Bazarr, Jackett, FlareSolverr, autobrr et unpackerr sont restés arrêtés 16 h sans alerte.

**Quota** (2026-10-07) : le quota du compte seedbox (`quota.json`, écrit par la seedbox toutes les 15 min, lu par le
montage après un `vfs/refresh`, module `quota.rs` partagé avec `/status.html`) est dans le résumé (« quota 57 % ») ;
alerte admin à `quota_alert_pct`, une par franchissement (voir « Alertes admin »). Une panne n'est notée
« signalée » que si l'alerte est partie : sinon nouvel essai au passage suivant (l'état est écrit après l'envoi).

## Chien de garde de homelabd (hors homelabd)

`systemd/homelabd-watchdog.timer` (toutes les 2 min, 5 min après le démarrage) lance en root
`scripts/homelabd-watchdog.sh` : `/health` sans réponse 3 fois de suite (≈ 6 min) → `systemctl restart homelabd` et
message Discord admin (webhook lu dans `.env`) ; un second message quand il répond de nouveau ; `--check` pour voir.
homelabd est en `Restart=always` (relancé même après un arrêt « propre » imprévu) ; le chien de garde couvre le cas
d'un homelabd vivant mais figé : depuis le 2026-10-07, `/health` répond 503 quand l'ordonnanceur ne tourne plus
(20 min sans tour de boucle), d'où une relance au bout d'environ 26 min. Le script écrit par `logger -p
user.warning|notice` ; l'unité est en `LogLevelMax=notice` et sous `OnFailure=homelab-alert@%n.service`.

## cert_watch — 24 h (2026-10-07)

Un seul certificat Let's Encrypt (joker, défi DNS-01) sert tous les hôtes de NPM ; NPM le renouvelle seul dès J-30
mais ne remonte rien en cas d'échec, et le canari passe par `localhost:8096` (ni NPM ni TLS). Depuis l'hôte, vers
`connect` (`127.0.0.1:443`) avec le nom de l'hôte public de Jellyfin (`JELLYFIN_PUBLIC_URL`, jamais en dur) :
date d'expiration du certificat présenté (connexion sans vérification, pour la lire même expiré ; décodage DER
minimal de `notAfter`), `GET <hôte public><health_path>` (chaîne NPM → Jellyfin), puis la même requête avec
vérification complète (certificat reconnu pour ce nom). 3 tentatives espacées de 30 s, nouvel essai sur erreur de
connexion et sur HTTP ≥ 500 (`should_retry`), seule la dernière compte. Défaut (certificat à moins de `warn_days` = 21
jours, chaîne cassée, NPM injoignable) : un message mail + Discord qui les regroupe, répété chaque jour tant que ça
dure, **mais jamais deux fois en moins de 20 h pour le même défaut** (2026-10-08 : la tâche tourne aussi à chaque
démarrage de homelabd, et chaque redémarrage réalertait ; voir « Mémoire des surveillances quotidiennes » plus bas).
Résumé : « certificat valide encore 49 j, /health 200 ».

## backup_watch — 24 h (2026-10-07)

Âge de la dernière archive `backups/homelab-state-*.tar.zst` : plus de `max_age_days` (8) jours, ou aucune archive ⇒
alerte admin. Couvre le minuteur `homelab-backup.timer` (dimanche) qui ne tourne plus du tout ; un échec franc de
l'unité prévient déjà par `OnFailure=homelab-alert@`. `homelabctl backup` poste lui-même, en fin de sauvegarde, les
avertissements de copie SQLite à l'admin. Même mémoire que `cert_watch` : « trop ancienne » puis « aucune archive »
sont deux défauts différents, l'âge qui grandit n'en fait pas un nouveau.

## diun_watch — 24 h (2026-10-07)

Relit `diun/images.yml` avec le contrôle strict de `homelabctl check` (`homelab_core::diun`) : clé en double, clé
orpheline, entrée sans `name`, nom en double, tabulation ; chaque image du compose doit avoir son entrée `repo:tag`
(digest ignoré) ; une entrée inutilisée n'est qu'un avertissement. Listes YAML en bloc acceptées sous une clé sans
valeur en ligne. Fichier invalide ⇒ alerte admin à chaque passage, sauf si les **mêmes** erreurs (empreinte
`alerts::fingerprint`) ont déjà été signalées il y a moins de 20 h. Du 18/09 au 07/10, deux groupes de clés
orphelines (retrait de Jackett/FlareSolverr) ont rendu diun aveugle sans que rien ne le signale.

Ces trois tâches passent aussi ~30 s après chaque démarrage de homelabd : l'heure du contrôle quotidien est celle du
dernier redémarrage.

## hls_loop_watch — 5 min
Boucle HLS = un client qui redemande sans fin le même segment d'un flux transcodé, ce que le membre voit comme
« ça charge » (le 13/09/2026, une TV webOS a redemandé deux segments ~950 fois en 6 min, tous servis en 200 avec
des tailles différentes : le segment était régénéré sous elle). NPM voit chaque requête. La tâche relit les
`tail_bytes` derniers octets de `npm_access_log` (horodatages **UTC**), garde les requêtes
`/videos/<id>/hls1/…/<n>.(ts|mp4|m4s)` de la fenêtre `window_secs`, et compte par (client, média, segment) en
fenêtre glissante ; `≥ threshold` ⇒ `warn!` + mail admin, une seule fois par (client, média) tant que la boucle dure.
**Rafales de ffmpeg** (2026-10-04) : un lecteur peut aussi faire relancer son flux en boucle sans redemander le même
segment — télé Samsung, 116 remux en une heure le 30/09 ; Chromecast, 42 lancements en 22 min le 04/10 — et la règle
des segments ne le voit pas. Chaque lancement laisse un `FFmpeg.<Transcode|Remux|DirectStream>-<date>_<heure>_<id>_<n>.log`
(heure locale) dans `cleanup.jellyfin_log_dir` : la tâche compte les lancements des 60 dernières minutes par titre, sans
les titres du canari (`state.canary.items`), et alerte au-delà de `max_jobs_per_item_hour` (lecture normale :
17 au plus). Depuis le 2026-10-07, **une alerte par rafale**, mémorisée dans `state.burst_alerts` (id compact → date)
et réarmée quand le compteur horaire du titre retombe sous le seuil : un redémarrage ne répète plus l'alerte, une
seconde rafale le même jour alerte. Une rafale n'est notée signalée que si l'alerte est partie (ou sans canal).
Les alertes (rafales et boucles de segments) donnent le **titre** Jellyfin (« Série S17E03 — Titre », « Film
(année) »), l'identifiant restant pour le journal ; repli sur « média <id> » si Jellyfin ne répond pas. Le
dédoublonnage des boucles de segments (client, média) reste en mémoire : il contient l'adresse d'un membre. L'ancien
repère « non-keyframe breaks » a disparu en 12.x (et ne comptait que le canari, qui demandait `BreakOnNonKeyFrames`).
Le résumé porte le plus grand nombre de lancements d'un titre dans l'heure et les 5xx sur segments. Rien n'est modifié.
La ligne « passage » du journal n'est en `info` que s'il y a une boucle, une rafale ou des 5xx (`is_noteworthy`, 2026-10-08) :
sinon `debug` (111 lignes en 9 h pour « rien à signaler »). De même la ligne « ok » de `disk_pressure`.

## disk_pressure — 15 min
`statvfs` sur `paths.base`, arrondi comme `df`. À `alert_pct` (2026-10-07) : alerte admin, une par franchissement
(réarmée `capacity_rearm_pts` sous le seuil), bien avant `hard_pct` qui supprime sans prévenir. Sous `hard_pct` : rien. De `hard_pct` à `crit_pct` : supprime avec
fichiers (`POST /api/v2/torrents/delete`, `deleteFiles=true`) jusqu'à 5 torrents en état
`stoppedUP`, les plus anciens d'abord — les hardlinks de `library/media` survivent.
≥ `crit_pct` : log d'erreur, aucune action automatique.

## cleanup — 24 h
Section `[cleanup]` : fichiers de `transcodes_dir` plus vieux que `transcodes_max_age_days` (vide en production : les
transcodages sont dans un tmpfs purgé chaque minute par `systemd/jellyfin-transcodes-purge.timer`) ; dossiers vides de
`paths.downloads` (profondeur ≤ 3) plus vieux que `empty_download_dirs_max_age_days` ; fichiers des `recycle_dirs`
(`.recycle` Sonarr/Radarr, aussi corbeille de la liste rouge côté VPS) plus vieux que `recycle_max_age_days` ; journaux
de `jellyfin_log_dir` plus vieux que `jellyfin_log_max_age_days`.

## playback_canary — 15 min

Un vrai transcodage HLS (PlaybackInfo forcé h264/aac 2 Mbit/s, appareil `gc-canary`, compte admin) sur un petit
épisode, en alternance VPS puis seedbox : `master.m3u8` → `main.m3u8` → deux segments. Échec si un segment est
vide ou tronqué (tmpfs plein), si le premier segment dépasse `max_first_segment_secs` (20 s), ou si Jellyfin refuse
le transcodage. Alerte admin (mail + Discord) au premier échec, message de retour à la normale, état dans
`state.canary` et ligne « Canari de lecture » sur `/status.html`. Sauté si `skip_if_transcodes_at_least` (2)
transcodages de membres sont déjà en cours. Le transcodage est arrêté proprement (`DELETE /Videos/ActiveEncodings`).
Résumé d'un passage réussi : **`lecture OK`**, toujours identique (2026-10-08) : le côté et la latence changeaient à chaque
passage, aucun n'était « calme » et une ligne `run_done` partait en `info` toutes les 15 min ; ils restent dans
`state.canary.last_detail` (/status.html) et dans le journal en `debug` (le retour après un échec reste en `info`).

## Page /status : « Rien ne bouge »
Les torrents terminés qu'aucune fiche n'a voulus (`no_match` et `nothing_importable` de `torrent_import`, 15 au plus,
les plus récents d'abord) sont listés sous le tableau des tâches : sans ça, un téléchargement fini restait invisible.
Depuis le 2026-10-21, les imports partiels (`partial`) y figurent aussi, avec « N vidéo(s) non rangée(s) ».
Autres sections : « Saisons sans release » (`uncovered` de `series_search`), « Canari de lecture », « Catalogue jamais
regardé » (`catalogue_report`).
Depuis le 2026-10-07 : « Dernière erreur » sous chaque tâche (même règle que la ligne ↳ de `homelabctl status`),
section « Alertes admin · N livrée(s), M non livrée(s) » (dernières alertes, canaux), et tout texte venu d'ailleurs
(nom de torrent, côté, titre de série, objet d'alerte) échappé au rendu.

## Alertes admin (2026-10-07)

Un seul point d'entrée, `alerts::admin` : mail (`CHAT_ADMIN_EMAIL`, repli `GUIDE_CONTACT_EMAIL`) **et** salon Discord
admin (`[discord] admin_alerts`). Chaque envoi laisse une ligne « alerte envoyée » (objet et canaux seulement, jamais
l'adresse, l'URL du webhook ni le corps), ou « alerte NON livrée » / « alerte sans canal », et une entrée dans
`state.alerts` (compteurs livrées/non livrées, 20 dernières). Rien en dry-run. Une alerte envoyée par `homelabctl`
(sauvegarde) n'y figure pas : la CLI n'écrit pas l'état.

- **Règle de réessai** : un appelant qui note « déjà signalé » ne le fait que si au moins un canal a pris l'alerte
  **ou si aucun n'est configuré** (`!alerts::retry_later(sent, alerts::configured(ctx))`) : sans canal, réessayer à chaque
  passage ne ferait qu'écrire un avertissement de plus. Appliquée par `scheduler::settle`, `hls_loop_watch`,
  `indexer_unblock` (alerte « toujours en panne » et « C411 en panne »), `seedbox_health` et `alerts::capacity`
  (alignement du 2026-10-08 : les trois derniers ne regardaient que `delivered`, et l'alerte « C411 en panne » ignorait
  le résultat de l'envoi), puis `torrent_import` (import partiel, `alert_pending`, 2026-10-21).
- **Mémoire des surveillances quotidiennes** (2026-10-08) : `cert_watch`, `backup_watch` et `diun_watch` passent aussi à
  chaque démarrage de homelabd. `alerts::watch` garde dans `state.watch_alerts` (par tâche : date + clé du défaut,
  `#[serde(default)]`, compatible avec l'état d'avant) la dernière alerte **partie** ; il retient la suivante tant que
  c'est le même défaut (`watch_due`) et qu'il s'est écoulé moins de `WATCH_REPEAT_SECS` (20 h, donc le passage
  planifié du lendemain passe toujours). Un défaut différent (autre clé : `cert_watch` = ses `Problem::kind` triés,
  `backup_watch` = `stale`/`missing`, `diun_watch` = empreinte des erreurs) alerte aussitôt ; un passage sain
  (`watch_clear`) efface la mémoire, une rechute alerte normalement. Une alerte non livrée alors qu'un canal existe
  n'est pas notée : la tâche réessaie à son passage suivant.
- **Tâche en échec répété** (`[alerts]`) : une alerte quand une tâche échoue `fail_streak` fois de suite **et**
  depuis au moins `fail_minutes` minutes (une tâche toutes les 20 s échoue 6 fois en 2 min sans que ce soit une
  panne), puis un message « Tâche rétablie » à son retour. Les erreurs isolées ne préviennent pas. Au plus
  `fail_alerts_max` (3) alertes de ce type par `fail_alerts_window_mins` (10) : au-delà, la série n'est pas marquée
  signalée et repart au prochain échec. La place est **réservée dans la même mise à jour d'état que la décision**
  (`AlertStats::reserve_streak`, 2026-10-08) : `alerts::admin` n'enregistre l'alerte qu'après l'envoi (SMTP puis Discord),
  et deux tâches au seuil à quelques secondes d'écart voyaient toutes deux de la place. La réservation (`reserved`, jamais
  écrite sur disque) est rendue dès l'envoi fini (`release_streak`) : l'alerte enregistrée, livrée ou non, compte à sa place ;
  une réservation orpheline (envoi interrompu) sort de la fenêtre d'elle-même. Mémoire dans `RunInfo` (`fail_streak`, `fail_since`, `streak_alerted`,
  `last_error`, `last_error_at`, `errors_by_day` sur 14 jours).
- **Capacité** : `alerts::crossing` (pure) + `alerts::capacity` : une alerte par franchissement de
  `tasks.disk_pressure.alert_pct` ou `tasks.seedbox_health.quota_alert_pct` (85 ; 0 = désactivé), réarmée
  `capacity_rearm_pts` (3) points sous le seuil, mémoire `state.capacity_alerts`.
- **Unités systemd** : `systemd/homelab-alert@.service` + `scripts/homelab-alert.sh` (webhook lu dans `.env`, jamais
  affiché ; titre « <unité> en échec », résultat et 20 dernières lignes du journal ; au plus un message par unité et
  par heure, le suivant dit combien d'échecs ont été tus ; `--dry-run`). `OnFailure=homelab-alert@%n.service` sur
  `homelab-backup`, `homelabd-watchdog`, `seedbox-mount-watch` et `jellyfin-transcodes-purge`. Les minuteurs à la
  minute sont en `LogLevelMax=notice` : seules les lignes préfixées `<5>`/`<4>` par leurs scripts restent au journal
  (purge silencieuse quand il n'y a rien à faire, `find -ignore_readdir_race`, conteneur arrêté = sortie propre,
  `SuccessExitStatus=2` pour « saturé après purge », déjà signalé par le script). `homelab-seedbox-mount.service` :
  `SuccessExitStatus=143` (un arrêt voulu n'est plus un échec ; un SIGTERM externe ne relance plus rclone).
- **Journal système** : `systemd/journald-homelab.conf` (`SystemMaxUse=2G`, `MaxFileSec=1day`, `MaxRetentionSec=45day`)
  posé par `homelabctl install` dans `/etc/systemd/journald.conf.d/homelab.conf` (réécrit seulement s'il diffère),
  puis `sudo systemctl restart systemd-journald` (à lancer soi-même, ne coupe aucun service). Avant : 500 Mo, et le
  fichier `user-1000` (homelabd, rclone) ne tournait presque jamais, d'où 1 à 10 jours d'historique en dents de scie.

## Discord

Webhooks (aucun bot) : `DISCORD_WEBHOOK_MEMBERS` (salon des membres) et `DISCORD_WEBHOOK_ADMIN` (salon privé) dans
`.env` ; `[discord] announcements` / `admin_alerts` dans `homelab.toml`. `homelab_core::discord` : embeds avec limites
de l'API, reprise sur 429, URL masquée dans les journaux. `homelabctl discord apply` crée ou met à jour, dans les 4
Arrs, « Discord membres » (imports : Radarr `onDownload`/`onUpgrade`, Sonarr `onImportComplete`/`onUpgrade`) et
« Discord admin » (`onHealthIssue`/`onHealthRestored`/`onManualInteractionRequired`), et active l'agent Discord de
Jellyseerr (demande, validée d'office, refusée, disponible) ; `test` poste un message d'essai sur chaque salon et
déclenche le test Jellyseerr ; `remove` retire tout. Ce que homelabd poste lui-même : annonces du tchat (membres),
alertes admin via `alerts::admin` (mail + Discord, voir « Alertes admin »), relances de `stack_health`. L'URL d'un
webhook est un secret : retirée des erreurs (`without_url`) et masquée dans les journaux.
