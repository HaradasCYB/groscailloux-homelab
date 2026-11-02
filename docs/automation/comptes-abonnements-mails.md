# Comptes, abonnements et mails

Comptes premium, onboarding, abonnés, mails aux membres, tchat et guide. Sommaire de toutes les tâches :
[AUTOMATION.md](../AUTOMATION.md).

## playback_limit — 20 s
Lectures simultanées par compte (`accounts.max_playbacks_per_user`, 2 ; comptes `accounts.protected`
exemptés ; 0 = illimité). `GET /Sessions` : pour chaque compte au-delà du maximum, les lectures les plus
récentes (première apparition la plus tardive, à égalité la moins avancée) qui durent depuis `grace_secs`
(30 s, le temps de passer d'un appareil à l'autre) reçoivent un message à l'écran (« Ce compte regarde déjà
sur 2 écrans… ») puis un ordre d'arrêt (`Sessions/{id}/Playing/Stop`) ; 3 tentatives au plus par lecture,
`max_actions_per_run` arrêts par passage. Remplace `MaxActiveSessions`, laissé à 0 (voir [runbooks/comptes-et-abonnements.md](../runbooks/comptes-et-abonnements.md#1-comptes)).

## user_poller — 60 s
`GET /api/v1/user?take=200` : users `userType == 2` (locaux), non admin, email valide, créés
depuis < 10 min, email pas encore traité (`state.onboarded`). Si un user Jellyfin du même nom
existe → marqué et ignoré. Sinon `DELETE /api/v1/user/{id}` puis onboarding unifié.

## media_mail — 5 min (2026-10-15)
Mails « ta demande est disponible » aux membres, en français, à la place de ceux de Jellyseerr (anglais mêlé de français,
logo SVG cassé dans Gmail, bouton vers Jellyseerr). **Envoi aux membres allumé le 15/10**, agent e-mail de Seerr coupé
le même jour : Seerr n'envoie plus aucun mail. Rendu : `available_mail::render` (gabarit `mail_layout`, fond,
affiche, puces, bouton « Regarder… » vers `<JELLYFIN_PUBLIC_URL>/web/#/details?id=<fiche>`), objets
`« Le Menu » est disponible`, `La saison 8 de « Dexter » est disponible`, `Les saisons 1 à 10 de « … » sont
disponibles`, `« Le Menu » et 3 autres demandes sont disponibles`.
- **Détection** : `GET /api/v1/request?filter=all` et `GET /api/v1/media` complets à chaque passage (la fin d'une
  saison ne change pas la date de sa demande ; la liste des demandes ne porte pas l'état des saisons). Film : média
  disponible, clé `r<id demande>`. Série : chaque saison demandée dont la demande de saison est `COMPLETED` **et** dont
  la saison du média est disponible, clé `r<id>:s<n>` : une saison arrivée part sans attendre les autres. Jellyseerr
  3.5 termine aussi la demande d'une saison **supprimée** (`MediaSubscriber` : saison `AVAILABLE` ou `DELETED`), ce
  que fait sa synchro de disponibilité pour une saison incomplète introuvable dans Sonarr et Jellyfin : sans le
  contrôle de la saison du média, elle serait annoncée, et sa vraie arrivée plus tard jamais. Demande de série en
  échec (`FAILED`) : ses saisons arrivées comptent (Jellyseerr ne termine la demande qu'avec la dernière). Saison 0,
  4K, demandes en attente ou refusées, films en échec, médias supprimés : ignorés.
- **Jamais deux fois, jamais l'arriéré** (`state.media_mail`, une suite par clé) : le premier passage est une
  référence (`baseline`, rien d'envoyé, `media_mail_baseline_at`) ; une clé notée n'est plus réévaluée, sauf
  `pending` et `error` ; disponible depuis plus de `max_age_hours` (daemon arrêté) → `stale` ; plus de `max_gap_mins`
  sans passage noté (`media_mail_last_pass` : état restauré d'une sauvegarde, daemon longtemps arrêté) → ce qui est
  arrivé pendant l'écart est `stale` aussi (l'état perdu l'avait peut-être envoyé), et ce que l'état restauré tenait
  encore en attente (`pending`, `error`) passe en `sent_uncertain`, sans mail ni alerte d'abandon (l'état perdu l'a
  très probablement envoyé : au plus un mail) ; clé notée pour un autre titre que sa
  demande actuelle (base Jellyseerr restaurée : ses ids repartent en arrière) → nouvelle clé ; clé notée `sending`
  sur disque **avant** `curl`, retrouvée au passage suivant → `sent_uncertain`, jamais renvoyée ; demande absente de
  la liste → `gone` seulement sur un 404 de `GET /api/v1/request/{id}`.
- **Regroupement par membre** : ses titres attendent `settle_mins` sans nouvelle arrivée (`max_hold_mins` au plus).
  Un titre (film, ou saisons d'une série réunies) = mail du titre ; plusieurs = récapitulatif (le dernier arrivé en
  avant, 3 affiches en rangée, les autres en liste dans le texte). `max_mails_per_run` par passage. Demande
  réattribuée dans Jellyseerr : le titre suit le nouveau demandeur. Un titre illisible (fiche TMDB retirée, panne)
  reste en attente sans bloquer les autres titres du membre.
- **Destinataire** : adresse Jellyseerr du demandeur (sinon `no_email`) ; pas de mail s'il a décoché « disponible »
  dans ses notifications par mail de Jellyseerr ou s'il est listé dans `MEDIA_MAIL_OPTOUT` par son adresse, son pseudo
  ou son id (`opted_out`), si son compte Jellyfin est suspendu (`suspended`) ou absent (`no_account`). Chaque titre est
  relu **comme le membre** par son id (`GET /Items/{id}?userId=`, 404 = invisible ; la liste `GET /Items?Ids=&UserId=`
  renvoie aussi les éléments des bibliothèques non partagées, Jellyfin 12.1 : ne pas y revenir ; saisons et premier
  épisode par `/Shows/{id}/…?UserId=`) : invisible pour lui, il attend `max_hold_mins` puis `not_visible`. Images lues
  dans Jellyfin (`format=Jpg`, anonymes), TMDB en secours, jointes en `cid:` ; le mail part sans celle qui manque.
- **Envoi** : `send_to_members = true` (défaut, depuis le 15/10) : `List-Unsubscribe` en `mailto:` vers l'adresse
  d'envoi, pas de `Reply-To` ; échec net → nouvel essai après `retry_mins` (15, 60, 240), puis `gave_up` ; `curl` en
  échec après avoir passé le message entier (délai dépassé ou connexion coupée en attendant la réponse) →
  `sent_uncertain`, jamais renvoyé. `false` (observation jusqu'au 15/10, retour arrière) = tout le chemin tourne, le
  mail est journalisé (« envoi aux membres coupé ») et ses clés notées `logged`, jamais envoyées ;
  `MEDIA_MAIL_PREVIEW_TO` (`.env`) reçoit alors une copie de chaque mail (essai de délivrabilité). Journal : id
  Jellyseerr et nom du membre, clés, objet ; jamais l'adresse.
- **Alerte d'abandon** (`gave_up` : échecs d'envoi répétés, ou titre en attente depuis plus de `max_age_hours`) : sa
  ligne est notée dans l'état (`media_mail_alerts`) **dans la même écriture** que l'abandon, et n'en sort que quand
  l'alerte est partie (`alerts::retry_later` : au moins un canal a pris l'alerte, ou aucun canal n'est configuré).
  **Une** alerte par passage, qui reprend tout ce qui attend, passages précédents compris, et part même quand
  Jellyseerr ne répond pas ou que le passage échoue ; une ligne n'y entre jamais deux fois.
- Réglages `[tasks.media_mail]` (bloc actif dans `homelab.toml`, toutes les clés, valeurs = défauts du code : test
  `media_mail_block_is_live_and_complete` ; `send_to_members` reste à `false` dans le fichier jusqu'à la minute de la
  bascule, ces deux tests échouent d'ici là) :
  `interval_secs` (300), `send_to_members` (`true`), `max_mails_per_run` (5), `settle_mins` (10), `max_hold_mins`
  (60), `max_age_hours` (48), `max_gap_mins` (120), `retry_mins` ([15, 60, 240]), `keep_days` (90).
  `max_gap_mins` doit rester bien au-dessus de `interval_secs` : homelabd refuse de démarrer si `max_gap_mins` × 60 ne
  dépasse pas `interval_secs` + 600 s (un passage au plafond), sinon chaque passage serait un écart et plus aucun mail
  ne partirait.
- **Ménage** : sur une liste complète seulement, clés terminées d'une demande disparue, après `keep_days`.
- **Bascule du 15/10** (décision du propriétaire, après validation des exemples) : binaires construits d'abord, puis
  dans la même minute `send_to_members = true` dans `homelab.toml` (même défaut dans le code), **agent e-mail de Seerr
  coupé**, homelabd **et** homelabctl installés, redémarrage. La ligne ne passe jamais à `true` à l'avance : homelabd
  ne lit `homelab.toml` qu'au démarrage, et un redémarrage du binaire d'avant (chien de garde, plantage) enverrait
  pendant que Seerr envoie encore. **Seerr 3.5 n'a pas de types de notification pour l'agent e-mail** (vérifié dans
  son code le 15/10) : « Média disponible » ne se décoche que compte par compte, et `media_mail` lit justement cette
  case comme un refus du membre (`opted_out`) ; d'où la coupure de tout l'agent (par l'écran : par l'API, `POST`
  remplace tout l'objet et `GET` renvoie le mot de passe SMTP). Procédure, conséquences côté Seerr et retour arrière :
  [comptes-et-abonnements.md § 2](../runbooks/comptes-et-abonnements.md#2-onboarding-et-mails).
  **Premier passage avec envoi** (dans les 30 s qui suivent le démarrage : décalage initial de l'ordonnanceur ; un
  passage sans Jellyseerr ne compte pas, le suivant fait la bascule) : il note `logged` ce qui attendait encore (le
  regroupement retient un titre jusqu'à `max_hold_mins`) **et** ce qui est apparu depuis le dernier passage sans
  envoi, **si Jellyseerr, encore actif, l'a annoncé** (`Continuity::switch_on`, `Ready::seerr_announced`). Jellyseerr
  n'envoie son mail qu'au passage de la demande à `COMPLETED`, et pour une série seulement si toutes les saisons
  demandées sont alors disponibles (`notifyAvailableSeries`) : une saison arrivée d'une demande dont d'autres saisons
  manquent (ou d'une demande pas encore terminée) n'a eu aucun mail de lui et n'en aura plus ; elle reste à homelabd
  (en attente, puis un mail, au besoin dès ce passage). Les clés déjà notées (`baseline`, `logged`…) ne sont jamais
  réévaluées. La date de ce passage est notée dans l'état (`media_mail_last_pass.send_since`, gardée tant que l'envoi
  reste allumé) : une nouvelle clé disponible avant elle mais absente de sa lecture (une demande supprimée pendant la
  lecture paginée fait sauter un id à la frontière d'une page, sans que la liste paraisse incomplète) suit la même
  règle au passage suivant (`Continuity::switched_at` ; sans effet au-delà de `max_age_hours`). Seul ce qui devient
  disponible après ce passage part, au plus une fois. Au plus un mail, jamais deux ; au pire un titre sans mail :
  arrivé entre la coupure de l'agent de Seerr et ce premier passage (moins d'une minute quand l'agent est coupé juste
  avant le redémarrage), ou saison d'une demande pas encore entière déjà notée `logged` pendant l'observation (Seerr
  attendait la dernière saison ; aucune avant la bascule du 15/10). Si la tâche n'a jamais tourné, ce premier passage
  est la référence (rien d'envoyé non plus).

## Vue d'ensemble des membres (Jellyfin Enhanced)

Chaque membre voit les mêmes onglets que l'admin : **Demandes** (toutes, avec leur demandeur) et **Calendrier**
(VPS et seedbox). Réglages, tous globaux (les nouveaux comptes en héritent) :
`DownloadsFilterByUserRequests = false`, `CalendarFilterByLibraryAccess = false`, `SonarrInstances` et
`RadarrInstances` avec les deux machines. Côté Jellyseerr, le droit **« voir les demandes » (bit 16384)** est posé
sur les comptes actifs, dans `defaultPermissions`, et à chaque activation (`accounts::granted_bits`, réglage
`[accounts] jellyseerr_view_requests`) : sans lui, Jellyseerr ne renvoie que les demandes du membre. Ce droit est
en lecture seule — ni validation ni refus. Le plugin garde 30 min en cache le lien compte Jellyfin ↔ compte
Jellyseerr : un changement de droits met ce temps à se voir.

## Onboarding

Trois entrées, un seul flux (`onboard::run`) : `homelabctl onboard <user> <email> [--password]` (passe par l'API
du daemon), le formulaire admin `/` (session d'administration, voir « Pages d'administration »), et la **page publique
`/inscription`** (`[onboard] public_signup`) ; `user_poller` reprend aussi les « Add User » de Jellyseerr.
Le compte Jellyfin est créé avec un mot de passe aléatoire **jamais communiqué**, importé dans Jellyseerr
(e-mail, droits), puis suspendu sauf `accounts.new_accounts_premium`.

**Lien de bienvenue** (`homelab_core::welcome`, 2026-09-20) : le mail ne contient qu'un bouton vers
`/bienvenue/<jeton>` (jeton de 32 octets aléatoires, stocké **haché** dans `state.welcome_links`, valable
`[onboard] link_ttl_mins`, un seul usage, un seul lien actif par compte). La page fait choisir le mot de passe
(8 caractères min, sans le pseudo ; `POST /Users/{id}/Password` en admin, Jellyseerr suit), consomme le lien et
affiche les accès (Jellyfin, Jellyseerr, guide). Lien expiré ou consommé → page « recevoir un nouveau lien »
(`POST /bienvenue/renouveler`, réponse identique que l'adresse soit connue ou non, `renew_per_hour` par adresse) :
c'est aussi le « mot de passe oublié ». À l'**activation** depuis `/accounts`, mail « ton compte est actif » : bouton
vers Jellyfin si le dernier lien du compte a servi (mot de passe choisi, aucun lien émis), sinon nouveau lien
`activated` vers la même page. `/accounts` montre l'état du lien et permet
de le renvoyer ; `homelabctl accounts link <compte>` idem ; `homelabctl mail-test <adresse>` envoie le mail avec
un lien de démonstration (page en lecture seule ; `--all` : tous les mails aux membres, voir **Mails**). Les liens vivent dans l'état **du daemon** : la CLI passe par
`POST /admin/link` et `/admin/mail-test` (jeton en en-tête). `cleanup` purge les liens finis depuis > 7 j.

**Inscription publique** : pseudo + e-mail + case à cocher + champ leurre ; rate limit IP
(`web.rate_limit_secs`), plafond `max_signups_per_day` (toutes adresses), adresse déjà connue → réponse neutre sans
création (même page qu'une vraie inscription, plafond premium compris), pseudo pris → message. Le compte est créé
suspendu, puis l'essai est décidé **avant les mails** (1.30.5, `subscription_ops::start_trial` sous `onboard_lock`) :
essai démarré → compte actif, encadré « essai » dans le mail de bienvenue, mail admin « Nouveau membre en essai » (fin
d'essai, parrain, rien à faire) ; essai impossible (plafond premium, `trial_days = 0`, abonnements coupés, erreur) →
compte suspendu, fiche « suspendue », encadré « activation », mail admin « Nouveau compte à activer » avec la raison
(`CHAT_ADMIN_EMAIL`, repli `GUIDE_CONTACT_EMAIL`). Lien de bienvenue non parti ou erreur : le mail admin le dit, en
avertissement. Détail : [comptes-et-abonnements.md](../runbooks/comptes-et-abonnements.md#2-onboarding-et-mails).

**Mails** (`mail.rs`) : `curl smtps` vers le SMTP de `.env`, message MIME construit ici — `Date`, `Message-ID`,
`Reply-To` (contact, mails de lien seulement), sujet/noms RFC 2047, corps quoted-printable UTF-8 ; texte seul pour
l'admin. **Mails aux membres** (2026-10-15) : gabarit sombre « bleu galet » `mail_layout.rs`, `multipart/alternative`
= vraie version texte + `multipart/related` (HTML, logo et bandeau joints en `cid:`). Chacun est rendu par une
fonction pure : `welcome::welcome_mail` / `activated_mail` (bienvenue, démo, activation),
`subscriptions::reminder_mail` / `renewal_mail` / `suspended_mail` / `payment_failed_mail` / `activated_mail`
(prix tiré de `[subscriptions] price_text`), `chat::direct_mail` / `announcement_mail`, `available_mail::render`
(« disponible », tâche `media_mail`). Jamais d'identifiant, de mot de passe ni de `token=` dans un mail ; aucun lien
dans l'information de renouvellement ni dans le prélèvement échoué. **Exemples** (`mail_samples.rs`) : `homelabctl
mail-test <adresse> --all | --kind <type>` (`POST /admin/mail-test` `{to, all}` ou `{to, kinds}`, réponse une ligne
par type : envoyé ou échec) envoie un exemple de chaque mail aux membres, rendu par ces mêmes fonctions pour un compte
fictif « Exemple » (objet « [Exemple] … », mêmes en-têtes, 2 s d'écart ; « disponible » sur les derniers vrais titres
arrivés, relus par un compte qui voit toutes les bibliothèques) ; `--dump <dossier>` les écrit (`.html` + `img/`,
`.eml`, `.txt`) sans homelabd ni envoi. Seul état écrit : le lien de démonstration des mails de bienvenue.

## Guide des nouveaux membres

`GET /guide` (public, sans jeton, sur l'onboarder) : `crates/homelabd/assets/guide.html`, captures
intégrées, affiches floutées. Le fichier ne contient ni adresse ni contact (dépôt public) : `guide.rs`
remplace `{{JELLYFIN_URL}}`, `{{JELLYSEERR_URL}}` (et `_HOST`) et `{{CONTACT}}` depuis `.env`
(`JELLYFIN_PUBLIC_URL`, `JELLYSEERR_PUBLIC_URL`, `GUIDE_CONTACT_EMAIL`, `GUIDE_CONTACT_DISCORD`). Pour le
modifier : sources et captures dans `backups/guide-draft-20260915/` (`guide.final.tmpl.html`,
`build_final.py <asset> <aperçu>`, script de captures `shots.js` avec un compte ordinaire temporaire,
aucune demande envoyée), puis rebuild de homelabd. Validé par l'utilisateur le 2026-09-15.

## Tchat des membres

« Aide et annonces » (nom de la bulle et du panneau depuis le 2026-10-08) : bulle en haut à droite de l'interface web de
Jellyfin (navigateur, Jellyfin Desktop, applis Android/iPhone ; pas sur les télés ni les applis natives). Salons
`annonces` (modérateurs seulement), `entraide` (tout le monde ; l'ancien salon `discussion` y a été fusionné le 08/10 par
`merge_discussion`, à l'ouverture de `chat.db`, et `Channel::parse` accepte encore « discussion » comme synonyme), et un
fil privé `prive:<id Jellyfin>` par membre, lisible par lui et par les modérateurs (`[chat] moderators`). Un compte neuf
voit comme lus les messages publics de plus de `[chat] new_member_read_days` (14) jours (`ChatStore::init_reads`, une
fois). Règles côté client (sondages, focus, Échap, bandeau) : [runbooks/tchat.md](../runbooks/tchat.md).

- **Chemin** : script chargé par JavaScript Injector (`branding/jellyfin/gc-chat-loader.js`) →
  `/gc-chat/app.js` → API `/gc-chat/api/*`, publiées par NPM (hôte Jellyfin) vers homelabd `/chat/*`.
- **API** : `GET /me` (salons, non-lus, dernière annonce non lue), `GET|POST /messages`,
  `DELETE /messages/{id}`, `POST /read`, `GET /private` (modérateurs). 401 sans session valide, 403 hors
  droits ou hors `beta_users`, 429 au-delà du débit (1 message / 3 s, 30 / 10 min).
- **Règles** (`homelab_core::chat`, testées) : texte nettoyé, 2 000 caractères ; suppression de son message
  pendant 15 min, de tout message pour un modérateur (message conservé, marqué supprimé).
- **Mails** (nommés « Aide et annonces ») : toutes les minutes, s'il y a de nouveaux messages d'entraide ou privés de
  membres et que le dernier récapitulatif a plus de `moderator_mail_interval_mins` (15), un mail à `CHAT_ADMIN_EMAIL`. Annonce avec « envoyer aussi par
  mail » : un mail par compte actif ayant une adresse valide dans Jellyseerr (2 s d'écart), sauf l'auteur.
- **Messages privés de l'admin** : onglet Privé → « Nouveau message privé », un ou plusieurs membres actifs
  (`GET /members`, `POST /direct`, modérateurs seulement) ; chacun reçoit le message **séparément** dans son
  fil privé (personne ne voit les autres destinataires), avec mail facultatif (`direct_mail`, adresse
  Jellyseerr). Côté membre : badge et bannière « Message de l'admin » sur l'accueil (prioritaire sur
  l'annonce), qui disparaît une fois le message lu ou quand il répond.
- **Client** : messages toutes les 5 s panneau ouvert (10 s sur télé), `/me` en filet à 120 s (20 s quand aucun salon ni
  fil n'est interrogé) ; panneau fermé : 60 s, rien pendant une lecture, 3 min après 5 min sans activité ; `/messages` et
  `/read` renvoient l'état des non-lus ; bulle masquée pendant la lecture ; bannière de la dernière annonce non lue sur
  l'accueil ; aucun HTML de message interprété.
- **Annonce depuis la ligne de commande** : `homelabctl chat announce [fichier] [--author <modérateur>] [--mail]
  [--no-discord]` (texte lu dans le fichier ou sur l'entrée standard) → `POST /admin/chat/announce` (jeton
  `HOMELABD_ONBOARD_TOKEN`, hors du préfixe `/gc-chat/` publié par NPM). L'auteur doit être dans `[chat] moderators`
  et exister dans Jellyfin ; le texte est nettoyé comme un message ordinaire puis **découpé** en messages ≤ `max_chars`
  entre deux paragraphes (`chat::split_parts`, testé) ; Discord (salon des membres) et le mail facultatif reçoivent le
  texte entier, une fois. `--dry-run` affiche le nombre de messages sans rien publier. **Aperçu** :
  `homelabctl mail-test <adresse> --announce <fichier> [--author <modérateur>]` (`POST /admin/mail-test/announce`
  `{to, author, body}`) envoie à cette adresse seulement le mail que `--mail` enverrait aux membres
  (`chat::announcement_author`, `announcement_text`, `announcement_mail` partagés avec `/admin/chat/announce`), objet
  « [Aperçu] … » ; rien dans le tchat ni sur Discord ; `--dump <dossier>` le rend sans homelabd. Une URL `http(s)://`
  du texte devient un lien dans la carte du mail (annonce et message privé, même règle que le tchat).
- **Couper** : `[chat] enabled = false` (+ restart homelabd) et désactiver le script dans JavaScript Injector.

## Abonnés et cycle premium (v1.18, 2026-09-20)

Une fiche par compte dans `state/subscriptions.db` (SQLite, sauvegardée) : statut (`essai`, `actif`, `échéance
dépassée`, `suspendu`, `offert`, `exempt`, `à qualifier`), échéance, source (`paypal`, `manual`, `trial`, `import`),
abonnement PayPal lié, code de parrainage, historique horodaté. Module `homelab_core::subscriptions` (décisions pures,
testées) + `subscription_ops` (ce qui touche Jellyfin/PayPal/mails, toujours via `accounts::set_premium`).

- **Page `/premium`** : le membre saisit son nom de compte, PayPal reçoit ce nom en `custom_id` ; à l'approbation, la
  page poste `/premium/lier` qui relit l'abonnement chez PayPal, le rattache et **active tout de suite** (page
  `/premium/merci`). `/premium/activate` : rattachement par identifiant `I-…` (ou demande à l'admin, comme avant).
- **Webhook `POST /paypal/webhook`** (hôte public de `/premium`, sans liste d'accès) : signature vérifiée par
  `verify-webhook-signature` (`PAYPAL_WEBHOOK_ID`), chaque événement traité une fois (`paypal_events`).
  `ACTIVATED`/`RE-ACTIVATED`/`PAYMENT.SALE.COMPLETED` → paiement (échéance = prochaine facturation PayPal, sinon
  `period_days`) ; `CANCELLED`/`SUSPENDED`/`EXPIRED`/`PAYMENT.FAILED` → note dans l'historique, le compte va au bout
  de sa période ; remboursement → alerte admin. 5xx = PayPal réessaie.
- **`subscription_cycle` — 1 h** : fiches créées pour les comptes qui n'en ont pas (actifs → « à qualifier », jamais
  suspendus tant que l'admin n'a pas tranché). `decide` (décision pure, testée) ne suit que deux sortes de fiches
  (décision du propriétaire, 2026-10-08) :
  - **essai de l'inscription publique** (`is_trial` : source `trial` posée par `start_trial`, statut essai ou grâce) :
    rappel J-1 (jamais un palier égal à la durée de l'essai), grâce de `grace_days` à l'échéance, puis suspension ;
  - **fiche liée à PayPal** (`paypal_sub_id`) : qui prélève (ACTIVE) → une information de renouvellement sans lien au
    palier le plus lointain, marge `paypal_margin_hours`, puis grâce et suspension ; arrêtée chez PayPal → rappels J-7/J-1
    avec lien, grâce, suspension ;
  - **toute autre fiche est gérée à la main** (`managed_by_hand` : actif ou offert posé par l'admin, essai posé par
    l'admin avec `subs set --status trial`, import, exempté, à qualifier, ancienne grâce manuelle) : ni rappel ni
    suspension ; à l'échéance `decide` renvoie `ManualDue`, `run_cycle` regroupe les `ManualDue` d'un passage en **une**
    alerte admin (« Abonnés : échéance à gérer à la main ») et note `due_noted` (colonne de `state/subscriptions.db` +
    événement caché du membre) seulement si l'alerte est partie ; `/accounts` affiche « À gérer (échéance passée) ». En
    `cycle_dry_run`, une ligne dans le récapitulatif, sans note.
  Suspension = `set_premium(false)`, mail « accès en pause », rien de supprimé. Comptes protégés, `[subscriptions] exempt`
  et fiches gardées (compte Jellyfin absent) toujours sautés. Récapitulatif Discord admin à chaque passage qui a agi.
  `cycle_dry_run = true` : tout est annoncé, rien n'est appliqué. Depuis le 2026-10-07 :
  - un essai ne reçoit pas un palier au moins égal à sa durée (pas de J-7 pour un essai de 7 jours ; le J-1 part) ;
  - **abonnement PayPal automatique** (lié et pas arrêté chez PayPal, `auto_renews` ; statut inconnu = actif) : une
    seule information J-7 **sans lien** (« ton abonnement se renouvelle le JJ/MM », `Action::RenewalNotice`), grâce
    seulement `paypal_margin_hours` après la date de facturation ; à la suspension, mail « prélèvement non
    passé, mets à jour ton moyen de paiement dans PayPal » sans lien (un lien = second abonnement) ;
  - **fiches orphelines** (compte Jellyfin disparu) : celles sans rien à perdre (à qualifier, suspendu ou exempté,
    sans échéance ni PayPal : comptes de banc) sont retirées ; les autres, `max_orphan_removals_per_run` (3) au plus
    par passage. Au-delà, ou si Jellyfin renvoie une liste vide ou autre chose qu'une liste (`users()` échoue), aucune
    n'est retirée, le cycle les ignore (ni mail ni suspension), l'admin reçoit une alerte par fiche (événement
    `orphan_held`) et doit relever la clé le temps d'un passage ;
  - un parrainage ne prolonge jamais une fiche offerte, exemptée ou à qualifier sans échéance (ligne d'historique,
    rien compté dans le plafond annuel).
- **`subscription_reconcile` — au démarrage puis 24 h** (l'heure du contrôle est celle du dernier redémarrage de
  homelabd) : relit chaque abonnement PayPal lié ; décision pure `reconcile_decision` :
  - `Apply` : paiement constaté (`last_payment.time` > `paypal_paid_at` + 12 h), jamais déduit de `next_billing_time` ;
    fiche en retard ou importée par CSV : appliqué comme avant ;
  - `Baseline` : fiche d'avant ce suivi (sans `paypal_paid_at`) déjà alignée sur la date PayPal : le dernier paiement
    devient la référence, sans ligne d'historique ;
  - `PendingCharge` : prochaine facturation absente, facturation passée depuis plus de `paypal_margin_hours`, échéance
    de la fiche + marge dépassée sans paiement, ou fiche sans référence dont la facturation a été repoussée de plus de
    `period_days` + 3 j ⇒ alerte « Prélèvement PayPal en attente », une par abonnement et par jour UTC
    (`paypal_events` `pending:<id>:<jour>`), rien n'est prolongé ;
  - `Stopped` : CANCELLED, SUSPENDED, EXPIRED ou inconnu de PayPal (404) noté sur la fiche (`paypal_status`), ligne
    d'historique « abonnement annulé chez PayPal — accès jusqu'au JJ/MM » et information à l'admin au changement ;
    rien n'est suspendu avant l'échéance payée (le cycle suit, sans marge).
  Résumé : « vérifiés=3 corrigés=0 en attente=0 arrêtés=0 ». Un échec de `on_payment` ne bloque plus le passage.
- **Second abonnement** : si le compte trouvé (par `custom_id` ou adresse) a déjà un AUTRE abonnement ACTIVE chez
  PayPal (relu par `find_subscription` ; 404 ne bloque pas), rien n'est rattaché ni prolongé (`Payment::Duplicate`),
  l'admin est alerté une fois par jour et par abonnement, `/premium/lier` renvoie vers `/premium/activate?err=deja`.
  Le remboursement ou la résiliation se font à la main dans PayPal (argent réel). Changer d'abonnement remet
  `paypal_status`/`paypal_paid_at` à zéro ; les webhooks les tiennent à jour.
- **Essai et parrainage** : `/inscription` active le compte en « essai » `trial_days` jours (mail de fin d'essai par
  le cycle) ; code de parrainage facultatif à l'inscription ; au premier paiement du filleul, `referral_days` offerts
  aux deux, plafond `referral_cap_days_per_year`.
- **« Mon compte » dans Jellyfin** : script JavaScript Injector « Groscailloux Mon compte »
  (`branding/jellyfin/gc-account-loader.js`) → `/gc-compte/app.js` (NPM hôte 1 → homelabd `/compte/`) : statut,
  échéance, bouton d'abonnement pré-rempli (masqué tant que l'abonnement PayPal se renouvelle seul ; lien court sans
  clé), appareils connectés (déconnexion vérifiée sur `LastUserId`), lien de
  changement de mot de passe (page `/bienvenue`), code de parrainage, historique. Identité = jeton Jellyfin
  (`/Users/Me`, cache 5 min). Téléviseurs : lecture seule, Retour ferme.
- **Lien signé** : les mails (rappels, suspension) mènent à `/premium?compte=X&k=…`, `k` = HMAC-SHA256 de
  `HOMELABD_ONBOARD_TOKEN` sur `gc-premium-v1|<compte en minuscules>`, 16 hexa, comparé à temps constant. Seul un lien
  signé affiche « déjà un abonnement PayPal actif » à la place du bouton (fiche locale, aucun appel PayPal) : sans
  `k` ou avec une clé fausse, page normale, et la page publique ne dit plus qui paie. Renouveler le jeton rend les
  clés des anciens mails caduques, sans risque (le serveur refuse toujours un second abonnement).
- **Admin** : colonne « Abonnement » sur `/accounts` (statut, échéance, source) et décision par compte (prolonger de
  N jours, actif, offert, exempté, à qualifier, suspendu) ; `homelabctl subs list | set <compte> --status … [--days N]
  | extend <compte> --days N | link <compte> --sub I-… | import <csv PayPal> | paypal [--webhook <url>]`.
- **Secrets** : `PAYPAL_ENV` (sandbox|live), `PAYPAL_CLIENT_ID`/`PAYPAL_SECRET`/`PAYPAL_PLAN_ID`/`PAYPAL_WEBHOOK_ID`
  (ou `PAYPAL_SANDBOX_*`), `PREMIUM_PUBLIC_URL`. En service : **live** ; `/premium` montre le bouton de l'application
  REST. Si l'application repassait en sandbox, la page publique reprendrait le bouton Live historique (`DONATION_*`)
  et `/premium?test=1` montrerait le bouton sandbox.
- **Admin** (`/accounts`) : la source indique aussi « paypal · PayPal annulé/suspendu/expiré/introuvable ».

## Page de don

`GET /don` (public, sous-domaine `don.`) : redirection permanente (301) vers `/premium` ; l'ancienne page
statique `assets/don.html` n'est plus servie et a été supprimée (refonte de la DA). Les identifiants
`DONATION_PAYPAL_CLIENT_ID` et `DONATION_PAYPAL_PLAN_ID` ne servent plus qu'au bouton de repli de `/premium`
(voir ci-dessus). Pour le bouton PayPal de `/premium` : le conteneur ne doit pas avoir `id="paypal"`
(l'élément masquerait `window.paypal` et le SDK plante) et le cadre reste en `color-scheme: light` (sinon
fond blanc opaque en thème sombre). Avatar : `npm/data/don/avatar.jpg` (hors git), servi par NPM en `/avatar.jpg`.

## Comptes premium

Premium = compte Jellyfin actif ; non-premium = `Policy.IsDisabled = true` (fonction native de
Jellyfin : connexion refusée, jetons existants rejetés, rien n'est supprimé). La politique Jellyfin est
la seule source de vérité ; les comptes admin ne sont jamais listés ni modifiés.

- **Page** `onboarder.<domaine>/accounts` (session `/connexion`, voir « Pages d'administration »), derrière la
  connexion NPM « admin-outils » : un interrupteur par compte, compteur « N premium / max ». Formulaires POST sans
  JavaScript, jeton en champ caché (anti-CSRF). Lien « Comptes » dans le tableau Homarr Opérations.
- **CLI** : `homelabctl accounts list | on <compte> | off <compte> | limits` (`--dry-run` respecté).
- **Suspension** : politique relue puis seuls `IsDisabled` et `MaxActiveSessions` changent (bibliothèques
  intactes) ; lectures en cours arrêtées ; permissions Jellyseerr sauvegardées dans `state.accounts` puis
  mises à 0 (une session Jellyseerr ouverte survit à la suspension Jellyfin).
- **Activation** : refusée au-delà de `accounts.max_premium` ; permissions Jellyseerr restaurées (à
  défaut de sauvegarde, celles par défaut de Jellyseerr).
- **Comptes protégés** (`[accounts] protected` : les deux comptes de l'admin) : affichés avec un badge,
  sans interrupteur ni suppression, hors plafond. Les autres admins sont gérés normalement.
- **Suppression** : bouton « Supprimer » → page de confirmation (`GET /accounts/delete`) → `POST
  /accounts/delete` : compte Jellyfin puis compte Jellyseerr (ses demandes partent avec). CLI :
  `homelabctl accounts delete <compte> --yes`.
- **Plafonds** (`[accounts]`) : `max_premium` comptes premium, `max_playbacks_per_user` lectures simultanées par compte. Dimensionnés
  pour 6 vCPU sans GPU (un seul transcodage 1080p en temps réel) et le lien seedbox (~8–10 Mo/s par connexion, mesuré le
  20/09) ; pic mesuré le 2026-09-14 : 4 lectures simultanées pour 13 comptes. Lecture directe : 65 % des lectures (30 j
  au 04/10). Pas de limite de débit par utilisateur : elle forcerait des transcodages.
