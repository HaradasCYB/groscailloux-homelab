# Recherche et indexeurs

Tâches qui cherchent les releases (séries, films, voie russe), budget de l'indexeur C411 et page `/recherche`.
Sommaire de toutes les tâches : [AUTOMATION.md](../AUTOMATION.md).

## series_search — 10 min (remplace unknown_series_grab)
Les séries se cherchent **par identifiant TMDB**, par homelabd, plus par Sonarr. Pourquoi : Sonarr interroge
C411 avec ses propres titres (« Shingeki no Kyojin », « Attack on Titan ») alors que C411 range la série sous
« L'Attaque des Titans » ; et pour un animé il envoie 3 à 4 requêtes par épisode (~90 pour une saison), ce qui
déclenche le **429** de C411 et une pause de l'indexeur qui s'allonge jusqu'à 24 h. Mesuré le 2026-09-17 sur
la saison 4 : `{TmdbId:1429}{Season:4}` → 5 releases, toutes justes ; « Attack on Titan » → 1 ; identifiant IMDb
→ 100 releases sans rapport (C411 l'ignore). **Jellyseerr est en `preventSearch`** sur les deux Sonarr : une
demande crée la fiche et les saisons suivies, sans recherche. Sonarr garde le RSS, l'import et le suivi.

Pour chaque Sonarr : `GET wanted/missing` → saisons avec épisodes diffusés manquants, hors file d'attente, pas
cherchées récemment (`state.unknown_series` : sans candidat → `retry_after_hours` 24 h, ou `release_retry_hours`
(2 h) dans la **fenêtre de sortie** (voir plus bas), pack pris → 7 j, épisodes pris → `episode_retry_mins` (5 min),
attente d'une VF (`vo_wait`) → 2 h, erreur → 1 h), sans torrent étiqueté de la saison **en cours dans qBittorrent**,
**absentes de l'autre machine** et **demandées dans
Jellyseerr** (2026-10-29, 1.30.2 : une saison suivie seulement parce qu'elle a des fichiers n'est plus cherchée, voir
plus bas) ; séries ajoutées le plus récemment d'abord. Par saison, via Prowlarr
(deux clés, indexeurs « C411 » et « C411 (2) » de Prowlarr, voir « Budget de l'indexer » plus bas) : `search?type=tvsearch&query={TmdbId:<tmdbId>}{Season:<n>}`
→ releases dont l'attribut **`tmdbId` est celui de la fiche** (le nom ne compte pas) → `GET parse` de Sonarr
(saison, pack, épisodes, qualité) → garde-fous : marqueur FR (VFF > MULTi > FRENCH > VOSTFR), qualité autorisée
par le profil et ≤ 1080p, au moins une source, épisodes manquants. Pack si la moitié de la saison manque,
sinon épisodes ; tri : langue, résolution, au moins deux sources, codec, sources, audio (voir « Codec » plus bas). **Pas de pack avant la fin de la diffusion** (2026-10-07) :
quand un pack serait pris, les épisodes de la fiche sont relus (un GET) ; s'il reste un épisode **suivi** dont la date
de diffusion est connue et à venir (`future_episodes`), le pack est écarté (« pack écarté (épisodes à venir : 2,
3) ») et les épisodes sortis sont pris un à un (`choose_episodes`). C'est la `FullSeasonSpecification` de Sonarr, qui
ne joue pas côté seedbox (la release part directement dans qBittorrent). Un épisode sans date ne bloque jamais (ils sont
lus série par série si `undated_episodes`, pour les séries dont une saison suivie a eu une diffusion depuis
`undated_window_days` jours, puisque `wanted/missing` les omet). Le
plafond de taille d'un pack est jaugé sur les manquants **datés** (`SeasonTodo.dated_missing`, `choose_sized`), plus
sur les épisodes sans date de `undated_missing` : une saison toute sans date refuse donc tout pack de plus de
`max_gb_per_episode` et part épisode par épisode ; seule une lecture du `.torrent` des packs permettrait mieux.
Black Clover S02 (03/10) : pack de 51 fichiers (S02E52-102, 14,2 Go) pris pour une saison de 13 épisodes dont 1
diffusé. Rien par identifiant (série sans `tmdbId`, releases
sans attribut) : `text_queries` noms essayés en texte libre (Jellyseerr FR et original, Sonarr, alternatifs),
titre parsé identique exigé, release d'un autre identifiant écartée.

**Fenêtre de sortie** (2026-10-30, 1.30.4) : une saison restée sans release (`none`) dont le dernier épisode
manquant est diffusé depuis **au plus** `release_window_hours` est reprise toutes les `release_retry_hours`
au lieu de `retry_after_hours` (24 h). Cas d'origine : *Dead-End Job* (« Un job mortel », Netflix, coréen),
8 épisodes diffusés le 30/10 à 04:00 UTC, demandés à 08:12, cherchés une seule fois à 08:15 (3 requêtes, rien : pas
encore sur C411), publiés en MULTi VFF le jour même et remis au lendemain 08:15. Le RSS de Sonarr ne rattrape pas
ces séries : leur nom français n'est connu ni de TMDB (Jellyseerr `fr` renvoie « 꿀알바 ») ni de Sonarr (aucun titre
alternatif), `parse` de « Un.Job.Mortel.S01… » donne `series: null`. Probablement le même cas pour *Doctor X*
(« Mafia.En.Blouse.Blanche », à mesurer : la release paraît entre +1 h et +25 h après la diffusion). La règle tient dans `season_wait_secs`, partagée par la décision
(`season_due`) et l'onglet Demandes (`record_wait_secs`). Détails :
- **Identifiant seul pendant les reprises** (`id_only_retry`) : dans la fenêtre, si un passage complet (identifiant
  puis texte libre) a eu lieu après la dernière diffusion et il y a moins de `retry_after_hours`, seule la requête
  `{TmdbId}{Season}` part (1 requête au lieu de 3). Le texte libre (cours d'animés, séries trouvées par leur seul nom)
  est refait au premier passage après une diffusion puis toutes les `retry_after_hours`, comme avant. Une série sans
  `tmdbId` garde toujours le texte. Date du dernier passage complet : `full_at` de l'enregistrement.
- **Coût** : 28 requêtes au plus par saison introuvable pendant 48 h (24 passages par identifiant, dont 2 avec le
  texte libre), toujours sous `max_queries_per_run` par passage. Une série hebdomadaire rouvre la fenêtre à chaque
  nouvel épisode manquant, puis revient à 24 h ; un épisode **sans date** suit la dernière diffusion de sa saison (`undated_missing`), sa fenêtre
  s'ouvre donc à chaque épisode daté diffusé (coût borné de même). `try_integrale` n'ajoute aucune requête dans la
  fenêtre (`integrale_min_age_days` = 14 j ; le démarrage refuse une fenêtre ≥ 14 × 24 h).
- **VOSTFR/VO fraîche** (`vo_wait_hours`, 3 h, hors animés) : une release sans audio français publiée depuis moins de
  3 h n'est pas prise, **épisode par épisode** (les autres épisodes de la saison partent) ; si rien d'autre n'est
  acceptable, le passage note `vo_wait` et repart après `release_retry_hours`. Même règle que les films : reprendre
  plus souvent ne doit pas faire prendre une VOSTFR une heure avant la VFF, et le profil (`upgradeAllowed = false`)
  ne la remplacerait jamais. Jamais pendant un secours (indexeurs publics) ; les animés gardent la VOSTFR (MULTi >
  VOSTFR) et le RSS de Sonarr la prend de toute façon. « Animé » = `seriesType = anime` dans Sonarr (`is_anime`) : un
  animé classé `standard` attend donc 3 h comme une série ordinaire. Un pack VOSTFR frais d'une saison encore en
  diffusion donne aussi `vo_wait` (reprise à 2 h, au lieu de `none`) : coût borné, la release a moins de 3 h.
- **Torrent en cours dans qBittorrent** (`seasons_in_qbit`) : une saison dont un torrent étiqueté
  `homelab:series=<id>:season=<n>` (cours compris) se télécharge de ce côté — ajouté depuis moins de
  `retry_after_hours`, ou terminé depuis moins d'une heure sans jugement de `torrent_import` — n'est pas cherchée. Ces
  torrents (seedbox, ajoutés sans catégorie) ne sont pas dans la file de l'Arr : une reprise pendant le téléchargement
  aurait pris une autre release de la même saison (doublon) ou réétiqueté le torrent. qBittorrent illisible, torrent
  arrêté depuis plus de 24 h ou date d'ajout inconnue : pas de garde-fou (comportement d'avant). Le délai d'import
  d'une heure est une constante du code (`IMPORT_GRACE_SECS`, comme `PARTIAL_RETRY_SECS` de `torrent_import`). Conséquence voulue : après
  un `grabbed_episode`, les épisodes restants (VOSTFR fraîche en attente, par exemple) ne sont recherchés qu'une fois
  les torrents déjà pris importés.
- `fallback_none` (secours public) garde sa propre règle (`fallback_retry_hours`, aussitôt si C411 revient) ;
  `grabbed`, `grabbed_episode` et `error` gardent leurs délais ; dans la fenêtre, la reprise 5 min après un
  `grabbed_episode` se fait toutefois par identifiant seul si un passage complet date de moins de 24 h.
  `release_window_hours = 0` ou `vo_wait_hours = 0` coupe le volet correspondant.

**Identifiant incomplet → texte libre en plus** (2026-09-18) : `uncovered()` compare les épisodes manquants aux
épisodes couverts par les candidats. S'il en reste, les noms de la série sont essayés **en complément** de
l'identifiant (`how = "tmdb+texte"`), avec les mêmes garde-fous. Vu sur Bleach S17 : `{TmdbId:30984}{Season:17}`
renvoie 41 releases qui couvrent E01–26 et E41–48, mais **jamais E27–40** — ce cours n'est publié que sous
`BLEACH.Thousand-Year.Blood.War.S03`. Attention, Sonarr n'analyse correctement que le premier cours : `S01` →
saison 17, mais `S02` → saison 2 et `S03` → saison 3 de Bleach. Le garde-fou d'égalité de saison les refuse, ce
qui évite d'écraser deux vraies saisons ; ce qui reste introuvable est enregistré (`uncovered` dans
`state.unknown_series`) et listé sur `/status.html` sous « Saisons sans release », pour une reprise à la main
depuis `/recherche`.

**Pack d'un cours** (2026-09-18) : si le trou persiste et que la fiche est un **animé**, `cour_pack` retient
les releases que l'Arr rattache à cette fiche mais à une autre saison ; `torrent_file::files` lit la liste de
leurs fichiers dans le `.torrent` (téléchargé chez Prowlarr, aucune annonce au tracker) et `offset_mapping`
n'accepte que la certitude : trou d'un seul tenant, autant de fichiers vidéo que d'épisodes manquants,
numérotés en suite, aucun épisode déjà pourvu. Le décalage part dans l'étiquette
`homelab:series=<id>:season=<n>:offset=<k>:eps=<from>-<to>`, que `torrent_import` applique seul
(`EpisodeSource::OursOnly` : ni les épisodes de Sonarr ni `map_episodes`, tous deux lisant la mauvaise saison).
Le pack n'est jamais confié à l'Arr. Réglages : `cour_packs`, `cour_max_files`.

**Intégrale** (2026-10-03) : un pack sans numéro de saison (`Space.Dandy.INTEGRALE.MULTI.VFF.1080p…`) n'est jamais
renvoyé par `{TmdbId}{Season}`, et le `parse` de Sonarr n'en lit rien, pas même la qualité. *Space Dandy*, demandé par
un membre, restait donc « 0 candidat » alors que C411 l'avait. Quand **rien d'autre n'est acceptable** pour une saison
(et jamais pendant une panne de C411), `try_integrale` reprend les intégrales vues dans les résultats (même `tmdbId`,
`season_less_pack` : aucune saison lisible ou pack de plusieurs saisons). S'il n'y en a pas, il fait **une** requête
`{TmdbId}` sans saison (type `tvsearch`), seulement si le dernier épisode manquant est sorti depuis
`integrale_min_age_days` (14). La qualité est lue en remplaçant le marqueur par `S01` (`integrale_quality_title`).
Le classement suit les mêmes clés que `choose`, puis le `.torrent` des deux meilleures est lu (aucune annonce).
Chaque fichier vidéo passe au `parse` de Sonarr, scene mapping compris : `02x01` → S01E14. `integrale_pick` ne garde
que les fichiers dont **tous** les épisodes manquent dans CETTE saison ; deux fichiers pour un même épisode, c'est un
refus. Le torrent est ajouté **arrêté** ; les fichiers non retenus sont désélectionnés (`file_ids`, chemin exact sous
la racine), puis il démarre avec l'étiquette `homelab:series=<id>:season=<n>`, et `torrent_import` importe. Si
l'intégrale est déjà dans qBittorrent pour une autre saison, les fichiers rejoignent sa sélection et son passage dans
`torrent_import` est effacé, pour qu'il les importe à leur tour. Une intégrale dont les fichiers ne portent pas le nom
de la série (« 01. Titre.mkv ») est écartée : l'API `parse` de Sonarr ne prend pas de chemin. Réglages :
`integrale_packs`, `integrale_max_files` (300), `integrale_min_age_days`.

**Le VPS ne cherche plus** (2026-09-18) : `[downloads] auto_sides = ["seedbox"]` retire les Arrs du VPS de la
boucle, et leur indexer C411 est en `enableRss = false`. Le VPS n'entame plus aucun téléchargement.
**Envoi** — Arr du **VPS** : `POST /api/v3/release/push` (Sonarr suit, importe, renomme) avec le lien Prowlarr
réécrit pour les conteneurs (`prowlarr_url_for_arrs` = `http://prowlarr:9696` : le lien renvoyé par Prowlarr,
`http://localhost:9696/…`, désigne le conteneur lui-même, et Sonarr accepte l'envoi puis échoue en silence
« Connection refused », vu le 2026-09-17), puis vérification que la saison (ou le film) apparaît dans sa file
**par identifiants** (le titre de la file est le nom interne du torrent). Arr de la **seedbox** (Prowlarr du VPS
injoignable), refus d'identification ou indexeur bloqué (`bypassable_rejection`), ou envoi accepté mais pas
mis en file → `.torrent` récupéré par Prowlarr et ajouté au qBittorrent du même côté avec l'étiquette
`homelab:series=<id>:season=<n>`, importé ensuite par `torrent_import`. Tout autre refus (liste noire,
taille…) est respecté. **Rythme** : au plus `max_queries_per_run` requêtes C411 par passage,
`query_gap_secs` d'écart, `max_new_per_run` en plus pour les demandes de moins de
`new_request_hours`, le tout sous le budget
par clé (« Budget de l'indexer » plus bas) ; filet de sécurité : Prowlarr coupe chaque indexeur C411 à son `queryLimit` (valeur : [CLAUDE.md § 4](../../CLAUDE.md#4-valeurs-en-vigueur)).
Historique : le 2026-09-17, avec une seule clé partagée par Prowlarr et les 4 Arrs, le 429 est tombé vers 50 requêtes
dans l'heure, pendant le rattrapage de l'arriéré ; d'où les deux clés et leurs compteurs.
Résumé : `grabbed=1 none=2 pending=13`.

**Saisons demandées seulement** (2026-10-29, 1.30.2) : `wanted/missing` ne rend que des épisodes **suivis**, et
`monitor_sync` suit toute saison qui a des fichiers. Le 28/10 (cascade Lupin), une saison 3 demandée seule, des
fichiers mal rangés en saison 2 : saison 2 suivie, cherchée et prise, puis de même pour la saison 1. Après les fiches
Sonarr et le filtre « autre machine », la tâche lit toutes les demandes Jellyseerr (`all_requests_counted`, autant
d'ids distincts que le total annoncé, sinon erreur) et, pour chaque Sonarr (`jellyseerr_sonarr_id` pour la seedbox,
`jellyseerr_vps_sonarr_id` pour le VPS), construit `id de série → saisons demandées` (`requested_here` : notion de
`monitor_sync`, statuts 3 et 4 exclus, saisons de `deletions.seasons` retirées sauf nouvelle demande). Une saison d'une
série de la table qui n'y figure pas est écartée **avant le budget et toute requête à l'indexeur** (`not_requested=<n>`
au résumé, une ligne `debug` par saison). Une série absente de la table (ajoutée à la main) est cherchée comme avant.
Les demandes d'un autre titre (`media.tvdbId` différent de la fiche : id Sonarr périmé côté Jellyseerr) sont
ignorées ; une fiche qui n'a que de telles demandes est cherchée comme avant. Jellyseerr
illisible ou lu en partie : le passage échoue sans rien chercher. Le suivi n'est pas modifié : les mises à niveau
(RSS de Sonarr) continuent sur ces saisons. Pas de réglage (garde-fou, comme le filtre « autre machine ») ; risques
restants (RSS, voie russe) : [arrs-et-indexeurs.md § 5](../runbooks/arrs-et-indexeurs.md#5-séries--saisons-packs-et-numérotations).

**Codec** (2026-09-26) : à langue, résolution et partage égaux (≥ 2 sources), le **x265** passe devant le x264 et
l'**AV1** en dernier (`codec_rank`, clé `(langue, résolution, sources ≥ 2, codec, sources)`, même clé pour
`movie_search` et `/recherche`), puis l'**audio** (`audio_rank` : AAC/E-AC3/AC3/Opus > FLAC > DTS > DTS-HD/TrueHD,
le DTS n'est lu ni par les Chromecast ni par l'appli iOS). Un x264 reste pris quand c'est la seule version française bien partagée.

## movie_search — 5 min
Recherche des films **suivis, sans fichier, sortis (ou français en retard sur leur date TMDB, voir plus bas), hors
file d'attente**, dès le passage qui suit la demande
(`missing_hours` 0, passage toutes les 5 min) : depuis le 2026-09-17, Radarr n'a plus aucune recherche (C411 en
RSS seulement) et le RSS ne ramène que les nouveautés — un film ancien ne peut venir que d'ici. Même mécanique que
`series_search` : `{TmdbId:<id>}` (type `movie`), `tmdbId` vérifié, `parse` Radarr, garde-fous, `release/push`,
sinon qBittorrent + `homelab:movie=<id>`. Au plus `max_per_run` films (3) par passage ; un échec (indexeur en
panne) est retenté après `error_retry_hours` (1), « aucune release » après `retry_after_hours` (72).

**Films attendus** (2026-10-10) : C411 a souvent la WEB avant la date numérique de TMDB (*Spider-Man: Brand New Day* :
première release le 05/10 à 15:43, date numérique le 06/10, prise le 06/10 à 00:05). Sur 14 films récents, 6 sont arrivés avant leur date, jamais plus de 23 h avant.
La tâche pose à chaque passage `availabilityDelay = -release_window_days` dans les Radarr qui téléchargent
(`GET`/`PUT /api/v3/config/indexer`) : Radarr tient le film pour sorti `release_window_days` jours avant sa date (la plus proche des dates
numérique et physique, sinon salle + 90 jours : `release_date`), ce qui ouvre en même temps son RSS (15 min) et la
recherche de homelabd. À moins de `release_window_days` jours de cette date, avant comme après, « aucune release »
est retenté toutes les `release_retry_hours` au lieu de 72 h (`retry_hours` ; résumé : « N autour de leur
sortie »). L'onglet Demandes affiche « Pas encore sorti en numérique (prévu le …) : recherché toutes les 2 h dès le … ».

**VOSTFR fraîche : on attend la VF** (2026-10-10). Le RSS de Radarr prend la *première* release acceptable et le
profil FR-friendly ne remplace jamais un fichier (`upgradeAllowed = false`) : *Insidious* a eu sa VOSTFR 48 min avant
sa VFF. Une release sans audio français (`lang_rank` < 2 : VOSTFR, VO) publiée depuis moins de `vo_wait_hours`
n'est pas prise tant que rien de français n'est là : résultat `vo_wait`, refait au bout de `release_retry_hours`
(`fresh_without_french`). Côté Radarr, la tâche pose le profil de délai par défaut (`GET`/`PUT /api/v3/delayprofile`,
`delay_profile_fix`) : torrents retenus `vo_wait_hours` × 60 min, sauf score ≥ `vo_wait_bypass_score`,
protocole préféré « torrent » (sinon Radarr ignore le passe-droit au score), sans passe-droit « plus haute qualité ».
Le seuil est à 2500 parce que le format « FRENCH » reconnaît aussi « VOSTFR » : une VOSTFR monte jusqu'à ~2400, un
MULTi ou une VF part de 3500. Une release « FRENCH » seule attend aussi (rare sur un film récent). Le délai compte
depuis la publication : une vieille VOSTFR (catalogue) part tout de suite.

**Films français sans date numérique** (`awaiting_vod`, 2026-09-25) : sans date numérique, Radarr croit un film
disponible 90 jours après la salle ; en France la VOD arrive 4 mois après (chronologie des médias) et C411 n'a rien
avant. Un film en langue originale française, sans `digitalRelease` ni `physicalRelease`, sorti en salle depuis
moins de `min_days_after_cinema` (110) jours n'est pas cherché (résumé : « N en attente de la VOD ») ; l'onglet
Demandes affiche « En salle depuis le … · VOD vers le …, récupéré dès sa sortie ». Le RSS reste actif.

**Films français en retard sur leur date TMDB** (`vod_overdue`, 2026-10-30) : pour un film français, TMDB n'a
souvent que la date du DVD, ou une date numérique tardive, alors que la VOD sort environ 4 mois après la salle.
*De Gaulle : Résistance* : salle le 03/06, aucune date numérique, DVD le 04/11 → Radarr `isAvailable = false`
jusqu'au 02/11, alors que le film était sur C411 le 30/10. Un film en langue originale française que Radarr ne tient
pas pour sorti, en salle depuis **au moins** `min_days_after_cinema` (110) jours, et dont la date de `release_date`
est encore à venir, est donc cherché quand même (résumé : « N français en salle depuis plus de 110 jours »), au
rythme habituel (`retry_after_hours`, puis `release_retry_hours` autour de la date TMDB). C'est l'autre côté du
même seuil qu'`awaiting_vod` (avant : sans date, on attend ; après : daté plus tard, on cherche), les deux cas sont
disjoints. L'onglet Demandes affiche « En salle depuis le … : la VOD française est sans doute sortie, recherché tous
les 3 jours (date TMDB : …) ». Le RSS de Radarr, lui, reste fermé jusqu'à la date (moins `release_window_days`).
Les films étrangers ne sont pas concernés (date numérique fiable, releases précoces = CAM/TS ; *The Odyssey* reste
« Pas encore sorti »). Côté seedbox, la release va directement au qBittorrent (`homelab:movie=<id>`) et
`torrent_import` l'importe (`ManualImport`, copie) : la disponibilité de Radarr ne joue pas. Côté VPS (aujourd'hui
hors `auto_sides`), Radarr refuserait probablement le `release/push` d'un film non disponible (résultat `none`).

**Qualités d'avant-sortie jamais prises** (`PRE_RELEASE_QUALITIES`, 2026-10-30) : `best_movie_release` écarte
toujours les qualités Radarr inconnue (0), WORKPRINT, CAM, TELESYNC, TELECINE, DVDSCR et REGIONAL (24–29, constantes
du code de Radarr), même si le profil les autorise ou s'il est illisible (`allowed` vide = tout accepté).

## indexer_unblock — 5 min
Après des échecs (429, délais), Sonarr et Radarr mettent un indexeur en pause, jusqu'à 24 h ; aucune API ne
lève la pause (`indexerstatus` → 404). Lecture seule de la table `IndexerStatus` des 4 Arrs (VPS : `rusqlite`
sur `sonarr|radarr/config/*.db` ; seedbox : `ssh seedbox` + `sqlite3 -readonly` sur
`<seedbox_apps_dir>/<app>/<app>.db`). Un indexeur en pause dont le **dernier échec date d'au moins
`quiet_mins`** (15 : fenêtre de limite de C411 ; plus tôt il se rebloquerait) est remis en service :
application arrêtée (`docker compose stop` / `app-<app> stop`), base copiée (`backups/arr-db-<app>-<date>.db`
ou `<app>.db.homelab-<date>` sur la seedbox, mode 600, gardées 7 jours), `UPDATE IndexerStatus SET
DisabledTill = NULL, EscalationLevel = 0, …`, application relancée et vérifiée (`system/status`). Au plus un
arrêt par application par `app_cooldown_mins` (60), même après un échec. Un indexeur débloqué
`max_unblocks_per_day` fois (10) en 24 h n'est plus touché : alerte admin (site mort, Cloudflare…), notée
« déjà signalée » (24 h) seulement si elle est partie. Chaque déblocage est signalé (mail + Discord admin).

**Site de C411 en panne** (2026-10-07) : avant de lever la pause d'un indexeur C411 (nom commençant par
`[manual_search] c411_indexer`), le site est sondé (`indexer::c411_reachable` : `caps` par Prowlarr, sans clé ni
quota ; pas `c411_up`, dont la branche `indexerstatus` reflète la clé de recherche et bloquerait la levée de la clé RSS
sur un 429). Site en panne (503, page HTML en 200, injoignable) ⇒ `Decision::Outage` : aucun arrêt d'Arr, aucune
copie de base, résumé « C411 en panne : pause laissée (n) ». Une alerte au début de l'incident (marqueur `c411_outage`
dans `state.indexer_alerts`, posé **seulement si l'alerte est partie** — ou si aucun canal n'est configuré —, sinon le
passage suivant la retente) ; tant qu'il existe, le site est sondé à chaque passage. Il n'est retiré qu'après
**6 h sans panne constatée** (2026-10-08, `OUTAGE_HOLD_SECS`) : chaque passage en panne rafraîchit la date du marqueur
(`OutageStep::Hold`, mutation différée), et une sonde réussie sur un marqueur plus récent ne fait rien (`Quiet`). Un C411 qui
clignote (503, 200, 503…) ne réalerte donc pas à chaque aller-retour ; la panne suivante, 6 h après le retour, est
signalée. Prowlarr absent ou injoignable = inconnu : comportement d'avant. Les autres
indexeurs ne sont pas concernés. Du 30/09 au 02/10 : 34 arrêts inutiles des Arrs seedbox. **Purge** des copies
`*.homelab-*` de plus de 7 jours de la seedbox : une fois par 24 h (un seul ssh, marqueur `purge:seedbox`, antidaté de
23 h en cas d'échec pour réessayer dans l'heure), plus seulement après un déblocage. `indexer_alerts` porte donc trois
sortes de clés : `application:id`, `c411_outage`, `purge:seedbox`. homelabd est cloisonné (`ProtectSystem=strict`) :
`backups/`, `sonarr/config` et `radarr/config` sont dans `ReadWritePaths` de `systemd/homelabd.service`.
Premier passage le 2026-09-17 : C411 (Sonarr seedbox, niveau 9, bloqué jusqu'à 21 h 33) et U2P, WorldTorrent,
JK-nortorrent (Sonarr VPS) remis en service.

## Indexer : RSS d'un côté, recherches de l'autre
Les 4 Arrs gardent C411 en **RSS seulement** (avec `C411_RSS_API_KEY`) : ils ne lancent plus aucune recherche,
donc ils ne peuvent plus déclencher la limite d'API (une recherche de saison d'animé = une requête par épisode).
Les recherches passent par Prowlarr, avec l'autre clé, sous le budget commun ci-dessous. `indexer_unblock` passe
toutes les 5 min et lève une pause 15 min après le dernier échec (10 fois par jour au plus).

## Budget de l'indexer (commun)
`homelab_core::indexer` : deux clés déclarées dans Prowlarr, un compteur horaire **par clé**
(`c411_max_per_hour`), `manual_reserve` gardées pour `/recherche`, bascule immédiate sur l'autre clé
si l'une répond 429 (mise de côté `cooldown_after_429_mins`). `search_tmdb` et `search_text` sont les deux
seules portes d'entrée : tâches et page passent par elles.

Historique : avant le 2026-09-17, chaque tâche avait son plafond (12/h, 1/h, 6/h) sans voir les autres ; un compteur
commun (20/h, réserve 6) l'a remplacé ce jour-là, puis un compteur par clé (deux clés).

## russian_search — 10 min
Voie russe : Sonarr et Radarr cherchent par leur titre (anglais), RuTracker range par le titre russe. Pour chaque
fiche de la voie russe à qui il manque des fichiers : titre original TMDB (Jellyseerr) → Jackett de la seedbox
(RuTracker) → releases d'au moins `min_seeders` sources, ≤ 1080p et sous `max_gb_per_movie` / `max_gb_per_episode`
de la section (pas ceux d'`[indexers]`) → film de la bonne année, ou release
dont la plage d'épisodes en numérotation absolue couvre le
plus d'épisodes manquants → `.torrent` ajouté au qBittorrent de la seedbox (`homelab:russe`), fichiers inutiles
désélectionnés → une fois complet, `ManualImport` copy (épisode par `SxxEyy` ou numéro absolu) et analyse complète de
Jellyfin. `max_per_run` 2, `retry_hours` 24 (1 h après une erreur), état `russian_title`. Un torrent déjà présent
(même release) est repris tel quel. Seedbox injoignable (qBittorrent, liste ou épisodes d'un Arr) : le côté est sauté,
passage réussi (« seedbox injoignable »), au lieu d'une erreur à chaque passage (2026-10-07).

## Recherche manuelle (`/recherche`)

Page de homelabd (hôte d'onboarding, liste NPM « admin-outils » + jeton) : chercher un titre des 4 Arrs, choisir
une saison ou le film, voir les releases, en télécharger une. Elle remplace la recherche de Sonarr/Radarr pour les
**animés** : celle-ci interroge chaque indexeur avec chaque titre connu, épisode par épisode (le 2026-09-17,
plusieurs minutes, délai dépassé du proxy de la seedbox — « timed out » — et 429 de C411 pour une heure).

- Logique dans `homelab_core::manual_search`, rendu dans `crates/homelabd/src/search_page.rs` (serveur, sans
  JavaScript : la page de résultats se recharge seule tant que la recherche tourne, rien ne bloque une requête HTTP).
- **C411 par identifiant TMDB** (une requête par saison ou par film, via Prowlarr), dans le budget par clé : la page
  dispose de la réserve `[indexers] manual_reserve` (par heure et par clé). Puis, si l'identifiant ne donne rien ou
  pour une saison, `[manual_search] text_queries` (2) noms de la fiche en texte libre (cours d'animés publiés sous leur
  propre titre, marqués « autre saison »). Plafond atteint, indexeur en pause ou fiche sans identifiant : la page le dit.
- **Nyaa n'est plus interrogé par la page** depuis le budget commun du 2026-09-17 (le bandeau de la page et la doc de
  `manual_search::run` le disent encore : texte périmé) ; il ne sert qu'en secours automatique pendant une panne de
  C411. Un lien **magnet** (secours) est ajouté directement au qBittorrent du même côté avec l'étiquette `homelab:`.
- **Rien n'est filtré** : toutes les releases sont montrées, triées comme le choix automatique (sans écart d'abord,
  puis bonne œuvre et bonne saison, langue, saison complète, résolution, codec, sources) et **marquées** :
  « sans français », « VOSTFR », « plus de 1080p », « hors profil », « aucune source », « autre saison »,
  « saison inconnue », « titre non reconnu ». L'admin garde la main : un pack VOSTFR reste téléchargeable.
- « Télécharger » passe par `series_search::send_release` (même chemin que les recherches automatiques) ;
  `dry_run` journalise sans rien envoyer. Résultats gardés `results_ttl_mins` (30) en mémoire.
