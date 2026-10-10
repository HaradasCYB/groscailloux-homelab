# Jellyfin : interface « Groscailloux TV » et scripts injectés

À lire avant de toucher au calque CSS, à un script de `branding/jellyfin/`, aux scripts servis par homelabd dans
Jellyfin (tchat, Mon compte) ou à un réglage d'affichage. Serveur et extensions :
[jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md). Bancs : [outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md).

## 1. Règles de l'interface

- **Un élément se contrôle par sa taille à l'écran (`getBoundingClientRect`, balayage `elementFromPoint`), jamais par sa
  seule présence** : le 03/10, un banc disait « tchat présent » alors que le bouton était dans l'en-tête caché de la 12.1.
- **Jamais retirer un élément dessiné par React** (nouvelle interface 12.1) : React le retirerait lui-même ensuite et
  planterait. On cache, on déplace **nos** éléments, on change des textes, sans jamais remplacer un nœud.
- **Jamais de `window.confirm/alert/prompt`** dans un script injecté : la WebView de l'appli iPhone et Jellyfin Desktop
  les ignorent (réponse « non » sans rien afficher). Confirmer dans la page.
- **Tout ce qui est propre à la télé est sous `.layout-tv` ou gardé par l'agent utilisateur, jamais par le nombre de
  cœurs** (un vieux portable y passerait).
- **Ne pas éditer le CSS dans l'interface** : source `branding/jellyfin/groscailloux-tv.css`, appliquée par
  `scripts/jellyfin-branding-apply.sh` (sauvegarde l'ancien, refuse tout `@import` en `@main/@master/@latest`). Jellyfin
  lit ce CSS dans `Branding/Configuration` (champ `CustomCss`), plus dans `Branding/Css`.
- **Tester sur un navigateur jetable avec un compte ordinaire temporaire**, CSS ou scripts candidats remplacés dans CE
  navigateur (interception de `Branding/Configuration` et de `/JavaScriptInjector/private.js`, service worker
  contourné), jamais en production. Lanceur : `tools/bench/bench.sh` (comptes `zz_` toujours supprimés).
- Les applis TV natives (Android TV, Fire TV) ne chargent **pas** jellyfin-web : ni calque, ni script.

## 2. Scripts injectés (JavaScript Injector)

Déployés par `sudo scripts/jellyfin-js-apply.py` (sauvegarde de la configuration du plugin avant écriture). Un script
public part dans `public.js` (chargé dès l'ouverture, avant la connexion), un privé dans `private.js`.

| Nom dans le plugin | Fichier | Public | Rôle |
| --- | --- | --- | --- |
| Groscailloux Socket unique | `gc-socket.js` | oui (en tête) | une seule websocket par page en 12.x (§ 9) |
| Groscailloux TV | `gc-tv.js` | oui | interface allégée sur télé, piste audio LG (§ 5) |
| Groscailloux Langue | `gc-lang.js` | oui | langue d'affichage française (§ 6) |
| Groscailloux Tchat | `gc-chat-loader.js` | non | charge `/gc-chat/app.js` ([tchat.md](tchat.md)) |
| Groscailloux Lire sur | `gc-cast-filter.js` | non | « Lire sur » limité à ses appareils (§ 7) |
| Groscailloux Qualité | `gc-quality-helper.js` | non | aide à la qualité ([lecture-et-transcodage.md](lecture-et-transcodage.md#4-diagnostiquer-une-lecture-qui-saccade)) |
| Groscailloux AirPlay | `gc-airplay.js` | non | AirPlay, sous-titres AirPlay, messages Cast (§ 7) |
| Groscailloux Mon compte | `gc-account-loader.js` | non | charge `/gc-compte/app.js` ([comptes-et-abonnements.md](comptes-et-abonnements.md)) |
| Groscailloux SyncPlay | `gc-syncplay.js` | non | contournement du saut en SyncPlay (§ 8) |
| Groscailloux En-tête | `gc-header.js` | non | adaptation de la nouvelle interface 12.1 (§ 4) |

- Une mise à jour du tchat ou de Mon compte = rebuild de homelabd (les `app.js` sont dans `crates/homelabd/assets/`),
  pas le plugin.
- Reprise par version : chaque script porte une `VERSION` ; une version plus récente prend la main et l'ancienne
  s'arrête (`__gcAirPlayDone`, `window.__gcHeaderV`…) : le lot `private.js` peut porter les deux pendant un essai. Un banc
  qui essaie une candidate retire la version déployée (`deployed-*.txt` dans le dossier candidat).
- Jellyfin garde certaines listes en mémoire pendant la vie de la page (cibles de « Lire sur ») : un filtre doit être
  chargé avant la première ouverture du menu.

## 3. Thème, logo, accueil

- **Thème** : ElegantFin **épinglé** + calque maison. Monter ElegantFin = changer le tag, repasser le banc (captures
  bureau + téléphone), puis appliquer. Retour arrière : `scripts/jellyfin-ui-rollback.sh backups/jellyfin-ui-<date>`
  (config, extensions, préférences d'affichage ; `--with-db` seulement si Jellyfin ne démarre plus).
- **Écran de connexion** : aucun compte listé (`IsHidden = true` pour tous, et dans `non_admin_policy`) ; titre
  `--loginPageText` dans le calque (« Connecte-toi » ; plus long, il passe sur deux lignes).
- **Logo** : `branding/jellyfin/logo/` (source `logo.html`, rendu par Chromium), déposé par
  `POST /JellyfinEnhanced/UploadBrandingImage` (`banner-light.png`, `banner-dark.png`, `icon-transparent.png`,
  `favicon.ico`, `apple-touch-icon.png`). Jellyfin Enhanced sert nos images pour tout nom `banner-light.<x>.png` sous
  `/web/` (adresse fixe `/web/banner-light.gc.png` utilisée par `gc-header.js`).
- **Onglets Découvrir / Demandes / Calendrier** = pages natives de Jellyfin Enhanced, renommées en français dans le
  calque (`#je-native-tab-btn-*`) ; les anciens onglets venaient de SeerrFin (retiré). `ThemeSelectorEnabled = false`.
- **Accueil** (configurations hors git, sauvegardées dans `backups/jellyfin-ui-*`) : Home Screen Sections (16 rangées,
  ordre façon Netflix, chargement 4 par 4 ; « Séries à venir » désactivée : badge et dates en anglais incrustés), Collection
  Sections (Tendances, Anime, Les mieux notés, Films français), Auto Collections (collections françaises). « Mes médias »
  en tête. Un compte absent de Jellyseerr ne voit pas les rangées « Découvrir ».

## 4. Nouvelle interface 12.1 (gardée et adaptée)

- La 12.1 a deux interfaces : « modern » (React, barre `header.MuiAppBar-root`), celle des navigateurs sans réglage
  (`layout` absent) **et** des applis dont `NativeShell` répond `desktop`/`mobile` (Jellyfin Desktop, applis
  iPhone/Android) ; l'ancienne n'est servie que pour `desktop-legacy`, `mobile-legacy` et `tv` (clé `localStorage`
  `layout`, ou Réglages → Affichage → « Mode d'affichage » ; « Auto » = la nouvelle). Les télés restent sur l'ancienne.
- **L'ancien en-tête `.skinHeader` reste dans la page, caché** : tout ce qui s'y accroche est invisible. Le tchat et Mon
  compte visent la barre **visible** : boîte de `a[href="#/search"]` dans `header.MuiAppBar-root`, sinon
  `.skinHeader .headerRight` (`headerBox()` des deux `app.js`).
- **`gc-header.js`** (choix du propriétaire du 04/10 : garder la nouvelle interface) agit seulement quand la barre
  moderne est affichée :
  - logo `banner-light` à la place de l'icône et du nom du serveur ; sous 900 px (MUI `md`), jellyfin-web ne dessine ni
    son logo ni les bibliothèques : `a.gc-logo` inséré après ☰ s'il reste 192 px ;
  - rangée d'onglets Accueil, Favoris, Découvrir, Demandes, Calendrier ; pages de Jellyfin Enhanced = `#/home?tab=2|3|4`
    (navigation par l'adresse : son bouton ne réagit plus replié dans son menu « ⋯ ») ; sur PC, la rangée monte **dans** la
    barre (`nav.gc-tabs.gc-inline`) si elle tient avec 16 px de marge, compacte (`gc-compact`) sinon, sinon dessous ;
  - libellés de la barre traduits par table anglais → français (la barre est dessinée avant le chargement du français) ;
    libellés des extensions (Jellyfin Enhanced, langues audio `Intl.DisplayNames`, tailles « Go ») traduits dans toutes les
    interfaces, boutons de la bannière Media Bar 3.0 (« Play »,
    « Details », « Favorite ») en « Lire », « Infos », « Favori » ;
  - cloche NotifySync (`#netflix-bell`) déplacée après le tchat ; hauteur réelle de la barre dans `--gc-header-h` ;
  - **menu SyncPlay** (v5) : chaque groupe (`#app-sync-play-menu li.MuiListItem-root` dont la rangée contient « Rejoindre
    groupe ») en grille sur une ligne (avatars | nom | bouton, rangée intérieure en `display: contents`, aucun nœud React
    déplacé), menu borné à 420 px ; clic/Entrée/Espace sur la ligne = bouton d'origine (`spRowClick` : double clic et
    clics à moins de 1,5 s ignorés), `tabindex=-1` sur les lignes ; `#sync-play-active-subheader` jamais cliquable. Banc
    `backups/jellyfin12-test-20261003/syncplay_menu.sh` (`SP_PREFIX` unique ; `CLICK=1` ne clique que le groupe du banc).
- **Barre de bureau** : pas de bouton ☰ à 900 px et plus (bibliothèques dans la barre) ; il apparaît sous 900 px
  (« Ouvrir le menu », premier bouton de `.MuiToolbar-root`). Le banc `tools/bench/scenarios/header.js` exige ☰ sous
  900 px et sur téléphone ou tablette, et balaie toute sa largeur tous les 3 px (`--env WIDTH=800` pour un bureau étroit).
- **Media Bar 3.0** : son bloc `.slide-content` est ancré en haut (20 px) **et** en bas avec débordement masqué ; le
  descendre par `top` coupe les boutons : le calque le décale par `translate`. Il place lui-même les rangées
  (`.homeSectionsContainer { margin-top: … }`) : le calque **remplace** cette marge (`80vh − 8.5em` en paysage ; en
  portrait, `--slideshow-content-top − --gc-header-h`) ; un `top` ajouté par-dessus faisait un double décalage.
  `--slideshow-page-offset` est faux : `LayoutSync.update()` mesure `document.querySelector('.page')`, la première page du
  DOM, toujours cachée (`#loginPage`, page Jellyfin Enhanced), et les rangées remontaient sur la bannière : figé à 0 dans
  le calque (`:root`, `!important` de feuille contre le style inline du plugin). En portrait sur téléphone, bannière à
  `--slideshow-height`.
  Ligne `.spec-line` à 11 px. Les règles Media Bar 2.x du calque (`.plot-container`, `.info-container`…) sont à revoir.
- **Interface mobile** (revue du 04/10, 28 défauts contre-vérifiés au banc) — points durables :
  - Jellyfin Enhanced force `flex-wrap: nowrap` sur la boîte des boutons, qui rétrécit sous son contenu, et
    `justify-content: flex-end` jette le surplus **à gauche, sur ☰** (seul accès aux bibliothèques) : sous 600 px,
    boutons de Jellyfin Enhanced cachés (⋯, Aléatoire), icônes à 40 px, ☰ au-dessus, SyncPlay caché sous 360 px (380 px
    avec le bouton Retour des applis). Contrôle : balayage `elementFromPoint` de ☰ ;
  - `.gc-tabs` sert à deux scripts : règles de `gc-header.js` sur `header.MuiAppBar-root > nav.gc-tabs`, du tchat sur
    `.gc-panel .gc-tabs` ;
  - bandeau d'annonce du tchat à `z-index` 1050 (sous la barre MUI 1100, le tiroir 1200, les menus 1300) ;
  - bibliothèques : `html.gc-modern .libraryPage…{padding-top:.5em}` (ElegantFin remet le décalage de l'ancien en-tête) ;
    barre A–Z : 12 px réservés à droite (`--effectiveWidth` d'ElegantFin, à revoir après une montée) ;
  - barre opaque sur écran tactile dès que la page défile (`html.gc-scrolled`) ; roue de Media Bar cachée sur écran
    tactile (`@media (hover: none)`) ; texte blanc sur bleu `#1b6fd8` sur téléphone (contraste 4,9:1), `#2f8fff` inchangé ailleurs ;
  - bloc « Également disponible » (Elsewhere) caché sur téléphone ;
  - section « Collections » des fiches : les 4 collections techniques des rangées d'accueil sont cachées par leur id
    (`data-id`, chemin `collections/<nom> [boxset]`) : **une collection renommée ou ajoutée réapparaît**, mettre son id dans le calque.
  Sauvegardes `backups/jellyfin-mobile-ui-20261004/` ; bancs `backups/jellyfin12-test-20261003/` (`mfix_home.js`,
  `modern_ui.js`, `header_dump.js`, `airplay_flow.js`), lancés par `tools/bench/bench.sh` (l'ancien `runprod.sh` et ses variables
  `CANDIDATE_DIR`, `CANDIDATE_JS`, `CANDIDATE_CSS` sont remplacés par `--candidate`).

## 5. Téléviseurs

- **Détection** : agent webOS/Tizen/Android TV, absence de pointeur, ou classe `layout-tv`. Comportement (tchat, Mon
  compte, aide à la qualité) : boucle à 3 s au lieu de 1 s, sondages espacés, ni ombre ni animation ; les bandeaux se
  ferment à la touche **Retour** (keyCode 461 webOS, 10009 Tizen). Coût mesuré sur une télé simulée : ~2 points de
  processeur sur l'accueil, rien en lecture (la lenteur vient du client webOS lui-même).
- **Disposition TV** (`layout-tv`, appli webOS = client web du serveur) : le calque **épingle l'en-tête** (jellyfin-web le
  laissait défiler hors écran dès qu'une carte avait le focus, `.skinHeader` en `position: relative` : Rechercher, Tchat,
  Notifications et Profil inatteignables au pointeur Magic Remote) et resserre onglets et boutons. Sondes
  `backups/cast-tests-20260919/tv_*.js`.
- **`gc-tv.js`** (public, exécuté avant la connexion et avant Media Bar) : sur agent TV, neutralise Media Bar
  (`window.slideshowPure.SlideshowManager.loadSlideshowData` et `CONFIG.enableTrailers`), retire `#randomItemButton` et
  `.headerSyncButton` ; piste audio des LG ([lecture-et-transcodage.md](lecture-et-transcodage.md#6-airplay-télés-lg-applis-tv-natives)).
  Le bloc `.layout-tv` du calque cache `#slides-container`, remet `.homeSectionsContainer` à `top: 1.6em`, cache les
  rangées non essentielles (classe = SectionId : `gc-tendances`, `BecauseYouWatched…`, `gc-mieux-notes`, `Genre-…`,
  `gc-films-fr`, `DiscoverMovies/TV`, `MyJellyseerrRequests`, `WatchAgain`), les blocs secondaires des fiches
  (`#similarCollapsible`, genres/tags/studios, Elsewhere `.streaming-lookup-container`, `.audio-languages-container`) et
  `.gc-chat-btn`. Les objets neutralisés de Media Bar sont exposés en fin de `slideshowpure.js`. Vérification : `tv_home_lite.js` (DEVICE=tv|desktop|iphone) : bureau et iPhone identiques avant/après.
- **Panneau Mon compte à la télécommande** : jellyfin-web ne voit pas notre panneau ; sur TV, `app.js` capte ←↑→↓
  (écouteur `window` en capture), OK garde l'activation native, Retour ferme ; langue et taille = boutons au lieu de
  `<select>` ; panneau en 22 px. Banc `backups/lg-tv-20260929/lg_panel.js`.
- Les pages « Demandes » et fiches d'un **admin** déclenchent des rafales Jellyfin Enhanced (`arr/series-slugs` par carte,
  réservé aux admins). Media Bar charge encore `youtube.com/iframe_api` au chargement (avant tout script injecté), sans
  lecteur : négligeable.

## 6. Langue d'affichage et saut

- **Langue d'affichage** : jellyfin-web la garde **dans l'appareil** (localStorage `<userId>-language` et
  `<userId>-datetimelocale` ; `userSettings.language()` lit avec `enableOnServer = false`) ; la copie dans les
  `DisplayPreferences` du serveur n'est jamais lue. `gc-lang.js` (public)
  pose `fr` pour les comptes connus de l'appareil (`jellyfin_credentials`) avant le démarrage, et **dès la réponse du
  serveur à la connexion** (XHR et fetch de `/Users/AuthenticateByName` et `AuthenticateWithQuickConnect`) : français sans
  rechargement. Filet : un rechargement unique, jamais sur `#/login` ni avant que les identifiants soient enregistrés
  (garde `sessionStorage`).
  Une clé `language` déjà présente = choix du membre, rien n'est touché. Banc
  `backups/jellyfin12-test-20261003/lang_nr.js` (navigateur anglais, 9 cas ; son code de sortie est 0 même avec des ✗).
  Une sonde `beforeunload` voit aussi les iframes YouTube de Media Bar : ce ne sont pas des rechargements.
- **Saut du lecteur à 10 s** (`[accounts] skip_forward_ms` / `skip_back_ms`, posés à la création par
  `jellyfin::set_skip_lengths`) ; Jellyfin met 30 s par défaut. Le bouton d'avance **et** les flèches passent par
  `skipForwardLength` (`playbackManager.fastForward(skipForwardLength())` dans `playback-video.*.chunk.js`) : une seule
  valeur (`skipBackLength` pour le recul). C'est un `DisplayPreferences` **par compte** (`usersettings`, client `emby`) ; une appli déjà
  ouverte garde l'ancienne valeur jusqu'au rechargement. Comptes existants convertis le 18/09 (sauvegarde
  `backups/jellyfin-skip-20260918-155125/`).

## 7. « Lire sur », Cast et AirPlay

- **`gc-cast-filter.js`** : seulement **ses propres appareils connectés**, quel que soit le réseau (un iPhone derrière le
  Relais privé iCloud ou un VPN joint le serveur par deux adresses : la condition « même adresse » a été retirée le 19/09).
  Le menu ne liste qu'un appareil **ouvert et connecté** avec le même compte : liste vide ≠ panne. Il ne filtre que les
  sessions Jellyfin (`getSessions`), jamais les cibles Cast ou AirPlay du navigateur.
- **`gc-airplay.js`** :
  - sur iPhone/iPad/Mac, remplace la note « (Google Cast non pris en charge) » (un `<p class="actionSheetText">`, pas une
    entrée) par une entrée **AirPlay** : vidéo en cours → `webkitShowPlaybackTargetPicker()` dans le clic ; sinon la
    feuille est fermée, le bouton Lire natif de la fiche est cliqué et le sélecteur tenté dès que la vidéo est prête ;
  - sur Android, « installe l'appli Jellyfin du Play Store » (Chrome sur Android n'a pas le SDK Cast web) ; ailleurs,
    « Aucun autre appareil connecté avec ce compte » quand la liste est vide ;
  - le bouton « Lire » du bandeau Media Bar (`.slide .btnPlay`) est inopérant dans l'appli iPhone (il fait
    `POST /Sessions/{id}/Playing` vers sa propre session) : depuis l'accueil, le script clique « Détails » de la diapositive
    active (`#slides-container .slide.active[data-item-id] .detail-button`), attend le `.btnPlay` de la fiche, puis lance ;
  - sous-titres en AirPlay : [lecture-et-transcodage.md](lecture-et-transcodage.md#6-airplay-télés-lg-applis-tv-natives) ;
  - en 12.1, « Lire sur » est un **menu MUI** (`#app-remote-play-menu`) présent caché dès le chargement et recréé avec la
    barre : la v5 le surveille (`characterData`) ;
  - les dialogues jellyfin-web 10.11 (interface « legacy ») se ferment **uniquement** par `history.back()` (entrée
    `history.state.usr.dialogs[]`) ; sans entrée d'historique (WebView), le script retire la feuille lui-même ;
  - chaque appui envoie ses étapes à `ClientLog/Document` (`jellyfin/config/log/upload_*.log`, lignes « gc-airplay … »).
  - hors de nos scripts, AirPlay passe par le bouton du lecteur dans Safari et l'appli iPhone.
  Bancs : `backups/cast-tests-20260919/airplay_test.js` (22 cas, `NO_INJECT=1` après déploiement),
  `backups/jellyfin12-test-20261003/airplay_flow.js`.

## 8. SyncPlay

- Correctif de fond côté serveur : Jellyfin 12.1. **Contournement en place** tant que la 12.1 n'est pas validée en séance
  réelle dans Jellyfin Desktop : `gc-syncplay.js` (seulement dans Jellyfin Desktop : `NativeShell.AppHost.appName()` ou
  `window.jmpInfo` ; forçable par `localStorage['gc-syncplay-force'] = '1'` ; état dans `window.__gcSyncPlay`).
  - Symptôme d'origine : roue de chargement après un saut, puis pause/lecture obligatoire ; ni NPM ni la conversion. À
    chaque saut, l'appli qui a sauté (Jellyfin Desktop 1.0.0, mpv) ne répondait jamais « prêt » (`PlaybackCore.scheduleSeek`
    attend `playing` 30 s) et le serveur programmait une pause lointaine pour l'autre.
  - Le script remplace `scheduleSeek` : pause, saut, attente de l'arrivée, puis « prêt » **à la cible** (le serveur
    n'accepte un « prêt » en pause qu'à 500 ms près). Le module est retrouvé par le registre webpack
    (`self.webpackChunk`, `__webpack_require__.m`) d'après son code, jamais son numéro ; sans module trouvé, il ne fait
    rien.
  - **Arrivée (VERSION 2, 10/10)** : mpv (Jellyfin Desktop 1.x, `currentTimeAsync`) = position à 5 s de la cible ;
    lecteur web (Jellyfin Desktop 2.x, navigateur forcé) = élément `video.htmlvideoplayer` sorti du saut
    (`seeking` faux, `readyState` ≥ 3) et à la cible. **Jamais la position du lecteur web** : Chrome émet `timeupdate`
    dès le DÉBUT d'un saut, la position annonce la cible avant l'image (VERSION 1 : « prêt » en 0,3 s puis « en
    chargement » 3 s plus tard et nouvelle attente du groupe ; lecteur rechargé à 0 : attente maximale à chaque saut).
  - **Attente maximale** : 10 s (lecteur web), 5 s (mpv), au lieu de 12 s. Couper plus tôt coûte plus cher qu'attendre
    (banc du 10/10 : à 5 s, un lecteur encore en chargement a perdu son saut, 10 min de décalage jamais rattrapé ; à
    3 s, synchro après 11 s). Sauts mesurés : lecteur web en HLS, fichier froid de la seedbox 1,4–8,9 s ; mpv en
    séance 0,3–0,6 s. Chaque attente maximale atteinte part au journal client (`upload_*.log`, « gc-syncplay v2
    attente maximale » : appli, position du lecteur, état de l'élément vidéo) : les lire avant de retoucher ces valeurs.
  - Banc : `tools/bench/bench.sh --accounts 2 --prefix zz_sp --item <id H.264 du VPS> --env ITEM2=<id seedbox> --timeout
    1500 tools/bench/scenarios/syncplay.js desktop` (phases normal, seedbox, lent, muet, depart ; ~12 min ; les
    navigateurs du banc lisent en HLS remuxé). Résultats du 10/10 : `backups/bench/20261010/syncplay/`.
  - **Groupe créé pendant une lecture = bloqué 30 s** (bogue de Jellyfin 12.1, reproduit au banc dans un navigateur) :
    le serveur envoie une liste de lecture **vide** (« SyncPlay startPlayback: empty playlist »), chaque appli ignore
    ensuite les commandes (« playlist item does not match ») jusqu'à l'abandon du serveur. Consigne : créer ou
    rejoindre le groupe d'abord, puis lancer l'épisode depuis le groupe ; pour débloquer, relancer l'épisode depuis le
    groupe (lecture commune en 1 à 2 s).
- NPM de l'hôte 1 garde les réglages SyncPlay (websocket, délais 3600 s, tampons coupés) : [npm.md](npm.md).
- **Après un banc SyncPlay** : un compte supprimé garde sa session et son groupe SyncPlay (visible de tous) : fermer
  l'appareil de test précis par `DELETE /Devices?id=<id vérifié>`. **Ne jamais cliquer un vrai groupe de membre** (le
  banc du menu ne clique que le groupe du banc par son nom exact, `SP_PREFIX` unique).

## 9. Une seule connexion temps réel par page

- Le serveur 12.1 n'envoie un message destiné à une session (GroupJoined/GroupLeft et commandes SyncPlay,
  `SetAudioStreamIndex` de la bascule VO, « Lire sur », arrêts de `playback_limit`) qu'à **une** de ses websockets, la plus
  récemment active. NotifySync 5.8.4.0 (compilation 12) ouvre sa propre websocket dès que `ApiClient.isWebSocketOpen()` est
  faux (toujours en 12.x) : 3 sockets par page, SyncPlay aléatoire.
- **`gc-socket.js`** (public, en tête de `public.js`) pose un accesseur sur `window.ApiClient` et, en 12.x seulement, fait
  répondre vrai à `isWebSocketOpen` et relaie LibraryChanged/UserDataChanged du SDK (cloche toujours en temps réel ; état
  dans `window.__gcOneSocket`). NotifySync ouvrait sa websocket sur le même jeton (`/socket?ApiKey=`) ; le serveur choisit
  la plus récemment active (`MaxBy(LastActivityDate)`). Contrôle : fermetures de websockets groupées par **2** dans le journal, et chaque « created
  group » suivi de « requested Ping ». **Toute nouvelle extension qui ouvre une websocket = même piège.** Banc
  `backups/jellyfin12-test-20261003/zz_spns/zz_spns_create.sh "r"`. À signaler à NotifySync (utiliser `ApiClient.subscribe`).

## 10. Vue d'ensemble des membres

- Onglets Demandes et Calendrier de Jellyfin Enhanced ouverts à tous : `DownloadsFilterByUserRequests = false`,
  `CalendarFilterByLibraryAccess = false`, `SonarrInstances` / `RadarrInstances` = VPS **et** seedbox, plus le droit
  Jellyseerr 16384 ([arrs-et-indexeurs.md](arrs-et-indexeurs.md#7-jellyseerr)). Chaque membre voit les demandes des autres,
  avec leur pseudo. Le lien compte Jellyfin ↔ Jellyseerr est en cache 30 min dans le plugin. Sauvegardes
  `backups/jellyfin-ui-20260917-191027-overview/`.
- **Avancement des demandes** dans l'onglet Demandes : script Mon compte sur les cartes `.je-request-card` (`data-tmdb-id`),
  données de `GET /compte/api/requests` (`homelab_core::requests_progress`). Les prises côté seedbox ne passent jamais par
  la file de Sonarr/Radarr : la barre lit aussi les torrents étiquetés `homelab:` des deux qBittorrent
  (`requests_progress::homelab_tag` / `from_torrent`). Clés d'état :
  `unknown_series` = `<arr>:<seriesId>:<saison>`, `movie_search` = `<arr>:<movieId>` (`<arr>` = `sonarr`, `radarr`,
  `sonarr-seedbox`, `radarr-seedbox`) ; Jellyseerr `serviceId` 0 = VPS, 1 = seedbox, `externalServiceId` = id Arr. Détail :
  [AUTOMATION.md](../AUTOMATION.md#suivi-des-demandes-dans-longlet-demandes-v119).
- **Avertissements expliqués** dans l'onglet Téléchargements (09/10) : même script, cartes `.je-download-card` visibles
  (taille à l'écran), appariées par source (icône Sonarr/Radarr), titre et `SxxEyy` de `.je-download-subtitle` ; le
  badge de statut prend le libellé de homelabd (texte d'origine gardé dans `data-gc-orig`, rendu dès que l'élément n'est
  plus expliqué) et une ligne `.gc-dl` s'ajoute sous `.je-download-meta`. Données : `GET /compte/api/downloads`
  ([arrs-et-indexeurs.md](arrs-et-indexeurs.md#10-avertissements-de-la-file-onglet-téléchargements)).
