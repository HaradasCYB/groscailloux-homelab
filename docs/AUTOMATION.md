# Automatisation : homelabd et homelabctl

`homelabd` (systemd `homelabd.service`) exécute chaque tâche à l'intervalle de `homelab.toml` ; `homelabctl` lit
son état et passe par son API. Le détail est rangé par famille dans [docs/automation/](automation/) : une ligne par
tâche ou sujet ci-dessous, vers son paragraphe. Fonctionnement général du démon (journal, état, `/health`, dry-run) :
[demon-et-commandes.md](automation/demon-et-commandes.md).

Nouvelle tâche : son paragraphe va dans le fichier de sa famille, et sa ligne ici. Les ancres de l'ancien fichier
unique sont gardées sur chaque ligne (`<a id>`) : un vieux lien `AUTOMATION.md#<ancre>` mène à la bonne ligne.

<a id="tâches-planifiées"></a>

## Recherche et indexeurs

[automation/recherche-et-indexeurs.md](automation/recherche-et-indexeurs.md)

- <a id="series_search--10-min-remplace-unknown_series_grab"></a>[series_search — 10 min (remplace unknown_series_grab)](automation/recherche-et-indexeurs.md#series_search--10-min-remplace-unknown_series_grab)
- <a id="movie_search--5-min"></a>[movie_search — 5 min](automation/recherche-et-indexeurs.md#movie_search--5-min)
- <a id="indexer_unblock--5-min"></a>[indexer_unblock — 5 min](automation/recherche-et-indexeurs.md#indexer_unblock--5-min)
- <a id="indexer--rss-dun-côté-recherches-de-lautre"></a>[Indexer : RSS d'un côté, recherches de l'autre](automation/recherche-et-indexeurs.md#indexer--rss-dun-côté-recherches-de-lautre)
- <a id="budget-de-lindexer-commun"></a>[Budget de l'indexer (commun)](automation/recherche-et-indexeurs.md#budget-de-lindexer-commun)
- <a id="russian_search--10-min"></a>[russian_search — 10 min](automation/recherche-et-indexeurs.md#russian_search--10-min)
- <a id="recherche-manuelle-recherche"></a>[Recherche manuelle (`/recherche`)](automation/recherche-et-indexeurs.md#recherche-manuelle-recherche)

## Imports, seedbox et files des Arrs

[automation/imports-et-seedbox.md](automation/imports-et-seedbox.md)

- <a id="id_match_import--5-min"></a>[id_match_import — 5 min](automation/imports-et-seedbox.md#id_match_import--5-min)
- <a id="torrent_import--2-min"></a>[torrent_import — 2 min](automation/imports-et-seedbox.md#torrent_import--2-min)
- <a id="seedbox_refresh--5-min-si-seedbox-enabled"></a>[seedbox_refresh — 5 min (si `[seedbox] enabled`)](automation/imports-et-seedbox.md#seedbox_refresh--5-min-si-seedbox-enabled)
- <a id="arrs-multi-instances"></a>[Arrs multi-instances](automation/imports-et-seedbox.md#arrs-multi-instances)
- <a id="tracker_ratio--30-min"></a>[tracker_ratio — 30 min](automation/imports-et-seedbox.md#tracker_ratio--30-min)
- <a id="stuck_handler--5-min"></a>[stuck_handler — 5 min](automation/imports-et-seedbox.md#stuck_handler--5-min)
- <a id="monitor_sync--note-2026-09-13"></a>[monitor_sync — note (2026-09-13)](automation/imports-et-seedbox.md#monitor_sync--note-2026-09-13)
- <a id="tba_bypass--5-min-désactivée--dans-tasksdisabled-depuis-episodetitlerequired--never-gardée-comme-filet"></a>[tba_bypass — 5 min (désactivée : dans `tasks.disabled` depuis `episodeTitleRequired = never`, gardée comme filet)](automation/imports-et-seedbox.md#tba_bypass--5-min-désactivée--dans-tasksdisabled-depuis-episodetitlerequired--never-gardée-comme-filet)
- <a id="monitor_sync--10-min"></a>[monitor_sync — 10 min](automation/imports-et-seedbox.md#monitor_sync--10-min)
- <a id="watcher-auto_import-continu"></a>[Watcher auto_import (continu)](automation/imports-et-seedbox.md#watcher-auto_import-continu)

## Catalogue et suppressions

[automation/catalogue-et-suppressions.md](automation/catalogue-et-suppressions.md)

- <a id="catalogue_report--24-h-2026-10-08"></a>[catalogue_report — 24 h (2026-10-08)](automation/catalogue-et-suppressions.md#catalogue_report--24-h-2026-10-08)
- <a id="red_list--24-h-2026-10-20"></a>[red_list — 24 h (2026-10-20)](automation/catalogue-et-suppressions.md#red_list--24-h-2026-10-20)
- <a id="deletion_cleanup--5-min"></a>[deletion_cleanup — 5 min](automation/catalogue-et-suppressions.md#deletion_cleanup--5-min)

## Comptes, abonnements et mails

[automation/comptes-abonnements-mails.md](automation/comptes-abonnements-mails.md)

- <a id="playback_limit--20-s"></a>[playback_limit — 20 s](automation/comptes-abonnements-mails.md#playback_limit--20-s)
- <a id="user_poller--60-s"></a>[user_poller — 60 s](automation/comptes-abonnements-mails.md#user_poller--60-s)
- <a id="media_mail--5-min-2026-10-15"></a>[media_mail — 5 min (2026-10-15)](automation/comptes-abonnements-mails.md#media_mail--5-min-2026-10-15)
- <a id="vue-densemble-des-membres-jellyfin-enhanced"></a>[Vue d'ensemble des membres (Jellyfin Enhanced)](automation/comptes-abonnements-mails.md#vue-densemble-des-membres-jellyfin-enhanced)
- <a id="onboarding"></a>[Onboarding](automation/comptes-abonnements-mails.md#onboarding)
- <a id="guide-des-nouveaux-membres"></a>[Guide des nouveaux membres](automation/comptes-abonnements-mails.md#guide-des-nouveaux-membres)
- <a id="tchat-des-membres"></a>[Tchat des membres](automation/comptes-abonnements-mails.md#tchat-des-membres)
- <a id="abonnés-et-cycle-premium-v118-2026-09-20"></a>[Abonnés et cycle premium (v1.18, 2026-09-20)](automation/comptes-abonnements-mails.md#abonnés-et-cycle-premium-v118-2026-09-20)
  - `subscription_cycle` — 1 h et `subscription_reconcile` — au démarrage puis 24 h : dans la même section.
- <a id="page-de-don"></a>[Page de don](automation/comptes-abonnements-mails.md#page-de-don)
- <a id="comptes-premium"></a>[Comptes premium](automation/comptes-abonnements-mails.md#comptes-premium)

## Surveillance et santé

[automation/surveillance-et-sante.md](automation/surveillance-et-sante.md)

- <a id="stack_health--5-min-première-passe-30-s-après-le-démarrage-de-homelabd"></a>[stack_health — 5 min (première passe ~30 s après le démarrage de homelabd)](automation/surveillance-et-sante.md#stack_health--5-min-première-passe-30-s-après-le-démarrage-de-homelabd)
- <a id="seedbox_health--5-min"></a>[seedbox_health — 5 min](automation/surveillance-et-sante.md#seedbox_health--5-min)
- <a id="chien-de-garde-de-homelabd-hors-homelabd"></a>[Chien de garde de homelabd (hors homelabd)](automation/surveillance-et-sante.md#chien-de-garde-de-homelabd-hors-homelabd)
- <a id="cert_watch--24-h-2026-10-07"></a>[cert_watch — 24 h (2026-10-07)](automation/surveillance-et-sante.md#cert_watch--24-h-2026-10-07)
- <a id="backup_watch--24-h-2026-10-07"></a>[backup_watch — 24 h (2026-10-07)](automation/surveillance-et-sante.md#backup_watch--24-h-2026-10-07)
- <a id="diun_watch--24-h-2026-10-07"></a>[diun_watch — 24 h (2026-10-07)](automation/surveillance-et-sante.md#diun_watch--24-h-2026-10-07)
- <a id="hls_loop_watch--5-min"></a>[hls_loop_watch — 5 min](automation/surveillance-et-sante.md#hls_loop_watch--5-min)
- <a id="disk_pressure--15-min"></a>[disk_pressure — 15 min](automation/surveillance-et-sante.md#disk_pressure--15-min)
- <a id="cleanup--24-h"></a>[cleanup — 24 h](automation/surveillance-et-sante.md#cleanup--24-h)
- <a id="playback_canary--15-min"></a>[playback_canary — 15 min](automation/surveillance-et-sante.md#playback_canary--15-min)
- <a id="page-status---rien-ne-bouge-"></a>[Page /status : « Rien ne bouge »](automation/surveillance-et-sante.md#page-status---rien-ne-bouge-)
- <a id="alertes-admin-2026-10-07"></a>[Alertes admin (2026-10-07)](automation/surveillance-et-sante.md#alertes-admin-2026-10-07)
- <a id="discord"></a>[Discord](automation/surveillance-et-sante.md#discord)

## Jellyfin

[automation/jellyfin.md](automation/jellyfin.md)

- <a id="anime_library--5-min"></a>[anime_library — 5 min](automation/jellyfin.md#anime_library--5-min)
- <a id="identity_check--30-min"></a>[identity_check — 30 min](automation/jellyfin.md#identity_check--30-min)
- <a id="original_language--10-min-07301130-2026-10-08"></a>[original_language — 10 min, 07:30–11:30 (2026-10-08)](automation/jellyfin.md#original_language--10-min-07301130-2026-10-08)
- <a id="trending--6-h"></a>[trending — 6 h](automation/jellyfin.md#trending--6-h)
- <a id="aide-à-la-qualité-de-lecture"></a>[Aide à la qualité de lecture](automation/jellyfin.md#aide-à-la-qualité-de-lecture)
- <a id="langue-par-membre-v119-2026-09-21"></a>[Langue par membre (v1.19, 2026-09-21)](automation/jellyfin.md#langue-par-membre-v119-2026-09-21)
- <a id="mode-vo--langue-dorigine--vo_native-préparé-le-2026-10-08-coupé-par-défaut"></a>[Mode VO « Langue d'origine » (`vo_native`, préparé le 2026-10-08, coupé par défaut)](automation/jellyfin.md#mode-vo--langue-dorigine--vo_native-préparé-le-2026-10-08-coupé-par-défaut)
- <a id="vo_native_guard--30-s"></a>[vo_native_guard — 30 s](automation/jellyfin.md#vo_native_guard--30-s)
- <a id="suivi-des-demandes-dans-longlet-demandes-v119"></a>[Suivi des demandes dans l'onglet Demandes (v1.19)](automation/jellyfin.md#suivi-des-demandes-dans-longlet-demandes-v119)
- <a id="avertissements-de-longlet-téléchargements-0910"></a>[Avertissements de l'onglet Téléchargements (09/10)](automation/jellyfin.md#avertissements-de-longlet-téléchargements-0910)
- <a id="sous-titres-seedbox"></a>[Sous-titres (seedbox)](automation/jellyfin.md#sous-titres-seedbox)
- <a id="subtitle_sync--5-min"></a>[subtitle_sync — 5 min](automation/jellyfin.md#subtitle_sync--5-min)

## Démon, commandes et clients

[automation/demon-et-commandes.md](automation/demon-et-commandes.md)

- <a id="fonctionnement-du-démon"></a>[Fonctionnement du démon](automation/demon-et-commandes.md#fonctionnement-du-démon)
- <a id="pages-dadministration-2026-09-23-portes-du-2026-10-07"></a>[Pages d'administration (2026-09-23, portes du 2026-10-07)](automation/demon-et-commandes.md#pages-dadministration-2026-09-23-portes-du-2026-10-07)
- <a id="autres-commandes"></a>[Autres commandes](automation/demon-et-commandes.md#autres-commandes)
- <a id="clients-http-2026-10-07"></a>[Clients HTTP (2026-10-07)](automation/demon-et-commandes.md#clients-http-2026-10-07)
- <a id="hook-gluetun-reste-en-shell"></a>[Hook gluetun (reste en shell)](automation/demon-et-commandes.md#hook-gluetun-reste-en-shell)
- <a id="tests"></a>[Tests](automation/demon-et-commandes.md#tests)
