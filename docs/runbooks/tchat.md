# « Aide et annonces » (tchat des membres)

À lire avant de toucher au tchat (`homelab_core::chat`, API `crates/homelabd/src/chat_api.rs`, client
`crates/homelabd/assets/chat/app.js`), à sa route NPM ou à ses mails. Détail de l'API et des mails :
[AUTOMATION.md, Tchat des membres](../AUTOMATION.md#tchat-des-membres). Réglages : `homelab.toml` `[chat]`.

## 1. Chemin et identité

- Servi sous `/gc-chat/` **sur l'adresse de Jellyfin** : NPM hôte 1, `location ^~ /gc-chat/` → `172.18.0.1:8766/chat/`.
  Le `^~` est **obligatoire** (sinon la règle de cache des `.js` de NPM envoie `app.js` à Jellyfin). Éditer la base NPM
  **et** `1.conf` ensemble ([npm.md](npm.md)).
- Chargé par JavaScript Injector (script « Groscailloux Tchat » = `branding/jellyfin/gc-chat-loader.js`, privé). **Une
  mise à jour du tchat = rebuild de homelabd**, pas le plugin.
- Identité = jeton de session Jellyfin vérifié par `/Users/Me`, cache mémoire 5 min (une suspension prend effet sous
  5 min), jamais écrit ni journalisé.
- Base `state/chat.db` (sauvegardée). Modérateurs et phase de test (`beta_users`) dans `[chat]`.
- Mails : récapitulatif à `CHAT_ADMIN_EMAIL` (repli `GUIDE_CONTACT_EMAIL`) ; annonces aux comptes actifs ayant une adresse
  **valide** dans Jellyseerr (le compte principal de l'admin n'y a pas d'adresse valide).
- Annonce depuis la CLI : `homelabctl chat announce [fichier] [--author <modérateur>] [--mail] [--no-discord]`.

## 2. Salons et nom (lot 4, 08/10)

- La bulle et le panneau s'appellent **« Aide et annonces »** (title et aria-label de la bulle, titre du panneau, guide
  public, page de premiers pas, trois mails : récapitulatif admin, annonce, message privé).
- Salons : **Annonces** (seuls les modérateurs écrivent), **Entraide** (tout le monde) et le fil privé membre ↔ admin.
  L'ancien salon **Discussion a été fusionné dans Entraide** (`merge_discussion`, transaction unique à chaque ouverture de
  `chat.db`, sans effet s'il ne reste rien ; messages gardés tels quels, repères de lecture fusionnés par MAX : un message
  d'Entraide plus ancien que le repère Discussion d'un membre passe pour lu). `Channel::parse` accepte encore
  « discussion » comme synonyme d'`entraide` (une page restée ouverte avant la mise à jour ne casse pas ; à retirer plus
  tard). Une restauration d'une base d'avant la fusion est rejouée par la même migration, idempotente.
- Onglets Annonces / Entraide / Écrire à l'admin (Privé pour un modérateur) sur **une** ligne, même à 320 px. À 600 px ou
  moins, le panneau prend tout l'écran.

## 3. Non-lus, sondages, clavier

- `/messages` et `/read` renvoient l'état des non-lus (clé `me`, comme `/me`) ; le client jette une réponse dont la
  requête est partie avant une réponse déjà appliquée.
- **Première visite d'un compte** : `ChatStore::init_reads` (appelé par `svc_me`) marque comme lus, salon public par salon
  public où il n'a aucun repère, les messages de plus de `[chat] new_member_read_days` (14) jours, une fois pour toutes ;
  fils privés exclus ; 0 = tout lu.
- **Sondages** : panneau ouvert, messages toutes les 5 s (10 s sur télé), `/me` en filet à 120 s ; sans salon ni fil à
  interroger (liste des fils d'un modérateur, rédaction d'un message privé : `watching()` faux), `/me` reprend 20 s
  (30 s télé, `ME_BLIND`). Panneau fermé : 60 s, aucun appel pendant une lecture, 3 min si rien n'a été touché depuis
  5 min (reprise immédiate), une tentative par 15 s si le serveur répond mal.
- **Ouverture, focus, Échap, Retour** : l'ouverture donne le focus à l'onglet actif (sinon la croix) ; Échap ferme le
  panneau où que soit le focus (écouteur sur `document`, en capture, posé à l'ouverture), le focus revient à la bulle ;
  Mon compte, ouvert par-dessus, se ferme d'abord. Panneau fermé = `visibility:hidden`. Retour des télécommandes (461,
  10009, GoBack) ferme aussi ; Backspace hors saisie seulement sur appareil « télé ».
- **Annonces** : l'ouverture défile jusqu'au **début** de la dernière annonce (une annonce découpée par `homelabctl chat
  announce` = même auteur, ≤ 10 s d'écart, compte pour une). **Bandeau d'annonce** : reste affiché tant que l'annonce
  n'est pas lue sur ordinateur, téléphone et télé avec pointeur ; la fermeture automatique (12 s) ne dépend que de
  `NO_POINTER` (`(hover: none) and (pointer: none)`), plus du nombre de cœurs.
- Messages privés de l'admin : onglet Privé → « Nouveau message privé », un ou plusieurs membres, chacun reçoit le message
  séparément. La liste des fils d'un modérateur n'est pas rafraîchie seule (seul le badge l'est).

## 4. Appareils

- Pas de tchat dans les applis natives (Android TV, Swiftfin) **ni sur les téléviseurs** : agent webOS/Tizen… ou classe
  `layout-tv` → `gc-chat-loader.js` n'insère pas `app.js`, et `app.js` se retire s'il est chargé.
- Nouvelle interface 12.1 : la bulle vise la barre visible (`headerBox()`) ; bandeau à `z-index` 1050
  ([jellyfin-interface.md](jellyfin-interface.md#4-nouvelle-interface-121-gardée-et-adaptée)).

## 5. Tests

- Jamais sur la production : une **seconde instance de homelabd** (binaire de la branche, `HOMELABD_DRY_RUN=1`, toutes les
  tâches dans `tasks.disabled`, `auto_import` coupé, port `127.0.0.1:18766`, `state_file` et `chat.db_file` à part,
  modérateurs = comptes de test) et le navigateur de test qui redirige `/gc-chat/*` vers elle. Bancs :
  `backups/chat-tests-20260915/` (dont `chatshots.js` : réponses d'API simulées dans le navigateur) et
  `backups/chat-tests-20261008/` (`all.sh`, comptes `zz_chat_*` supprimés par `teardown.sh`).
- Captures du guide public (section « Aide et annonces », ancre `#tchat` gardée) : `backups/chat-tests-20261008/`
  (`SEED=guide ./setup.sh`, `run.sh guide.js PASS=desk`, re-semer, `PASS=phone`, puis insertion dans `guide.html` ;
  viewport 720 px pour la capture « ordinateur »). La règle `img` du guide a `height: auto`.
- Aucun test unitaire JavaScript dans le dépôt : le client n'est couvert que par les bancs ; les règles de l'API le sont
  par des tests Rust (annonces réservées aux modérateurs, fils privés, suppression, fusion des salons, compte neuf).

## 6. Couper

`[chat] enabled = false` (+ restart homelabd) et désactiver le script dans JavaScript Injector.
