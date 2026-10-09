# Comptes, onboarding et abonnements

À lire avant de créer, suspendre ou supprimer un compte, de toucher aux mails, à la page `/premium`, à PayPal ou au cycle
des abonnements. Procédure admin pas à pas : [ONBOARDING.md](../ONBOARDING.md). Détail des tâches :
[AUTOMATION.md, Abonnés](../AUTOMATION.md#abonnés-et-cycle-premium-v118-2026-09-20) et
[Comptes premium](../AUTOMATION.md#comptes-premium).

## 1. Comptes

- **Premium = compte Jellyfin actif ; non-premium = `IsDisabled`** (connexion refusée, rien de supprimé). Toujours passer
  par `homelab_core::accounts` (page `/accounts`, `homelabctl accounts`), qui garde les permissions Jellyseerr ; toute
  suspension passe par `accounts::set_premium`.
- **Comptes protégés** (`[accounts] protected`, les deux comptes de l'admin) : jamais suspendus ni supprimés par la page,
  exemptés de la limite de lectures ; les autres admins sont gérés comme tout le monde.
- Plafonds (`[accounts]`) : `max_premium` 25 ; `max_playbacks_per_user` 2 (la 3ᵉ lecture est arrêtée par
  `playback_limit`).
- **`MaxActiveSessions` reste à 0** (`max_devices_per_user`) : il ne joue qu'à la connexion (403 « maximum number of
  sessions », affiché comme une erreur d'identifiants) et l'appli iOS crée une nouvelle session à chaque ouverture (des
  sessions fantômes d'un iPhone bloquaient la télé d'un membre, Quick Connect compris).
- Pas de `RemoteClientBitrateLimit` (il forcerait des transcodages).
- **Supprimer des appareils** : jamais en boucle sur `GET /Devices?userId=`, qui **ignore le filtre** et renvoie tous les
  appareils (le 15/09, tous les membres ont été déconnectés). Vérifier le nombre et le propriétaire (`LastUserId`) de
  chaque élément ; Mon compte vérifie `LastUserId` avant de déconnecter.
- Comptes ordinaires créés avant l'onboarding v2 : réalignés le 20/09 sur `non_admin_policy` (sauvegarde
  `backups/jellyfin-ui-20260920-mymedia/*-before.json`).

## 2. Onboarding et mails

- **Jamais d'identifiant ni de mot de passe dans un mail.** Le mail de bienvenue ne contient qu'un bouton vers
  `/bienvenue/<jeton>` (`homelab_core::welcome`, jeton haché SHA-256 dans `state.welcome_links`, `[onboard] link_ttl_mins`
  = 60, usage unique) où le membre choisit son mot de passe ; page expirée → renvoi par adresse (réponse neutre,
  `renew_per_hour`).
- Page publique `/inscription` (`public_signup`, champ leurre, limite par IP, `max_signups_per_day`, adresse connue →
  réponse neutre) ; l'admin reçoit « nouveau compte à activer » et le membre « ton compte est actif » à l'activation
  depuis `/accounts` (colonne « Lien », bouton Renvoyer).
- `mail.rs` construit un MIME complet (`Date`, `Message-ID`, `Reply-To`, RFC 2047, quoted-printable,
  `multipart/alternative`) ; `SMTP_FROM_NAME` = « Groscailloux ». (Avant le 20/09, le mail finissait en spam : `curl smtps`
  sans `Date`/`Message-ID`, identifiants en clair, liens DuckDNS.)
- **Les liens vivent dans l'état du daemon** : `homelabctl onboard|accounts link|mail-test` passent par `POST /onboard`,
  `/admin/link`, `/admin/mail-test` (jeton `HOMELABD_ONBOARD_TOKEN` en en-tête). Un lien écrit par la CLI dans le fichier
  d'état serait invisible du daemon et écrasé.
- **Tester sans polluer** : compte temporaire dont l'adresse est **celle de l'expéditeur** (`SMTP_FROM`, aucun rebond) ;
  jamais d'adresse inventée (rebonds = réputation Gmail). Jetons et mots de passe jamais journalisés.
- Demandes Jellyseerr validées d'office et droits « voir les demandes » posés à la création et à l'activation :
  [arrs-et-indexeurs.md](arrs-et-indexeurs.md#7-jellyseerr). Langue et saut posés à l'onboarding :
  [lecture-et-transcodage.md](lecture-et-transcodage.md#7-langue-audio-et-sous-titres-des-comptes).

## 3. Abonnements

- `homelab_core::subscriptions` (fiches SQLite `state/subscriptions.db`, décisions pures `decide` testées) +
  `subscription_ops` ; tâches `subscription_cycle` (1 h, réel depuis le 27/09 : `[subscriptions] cycle_dry_run = false`)
  et `subscription_reconcile` (au démarrage de homelabd puis toutes les 24 h : l'heure du contrôle est celle du dernier
  redémarrage) ; réglages dans `[subscriptions]`.
- **PayPal est en LIVE depuis le 20/09 à 23:30** (`PAYPAL_ENV=live` + `PAYPAL_CLIENT_ID`, `PAYPAL_SECRET`,
  `PAYPAL_PLAN_ID`, `PAYPAL_WEBHOOK_ID` dans `.env` ; même application et même plan que l'ancien bouton `DONATION_*`). Trois
  abonnements PayPal sont rattachés et relus chaque jour sans erreur (« vérifiés=3 »). `PAYPAL_SANDBOX_*` ne sert qu'aux
  essais : si l'application repassait en sandbox, `/premium` reprendrait le bouton historique et `/premium?test=1`
  montrerait le bouton sandbox.
- Webhook `/paypal/webhook` sur l'hôte public de `/premium` (hôte NPM 20, **sans** liste d'accès), signature vérifiée
  chez PayPal, id dans `PAYPAL_WEBHOOK_ID` (`homelabctl subs paypal --webhook <url>` le crée).
- **Aucun secret ni identifiant PayPal** (client, plan, application) dans le dépôt, les journaux ou les pages.
- Statut « à qualifier » = compte actif sans abonnement connu : **jamais suspendu par le cycle**, l'admin tranche sur
  `/accounts`.

### Abonnés hors PayPal : gérés à la main (décision du propriétaire, 08/10)

- Le cycle ne suit plus que deux sortes de fiches : les **essais de l'inscription publique** (`is_trial` = source `trial`
  posée par `start_trial`, au statut essai ou grâce) et les fiches **liées à un abonnement PayPal** (`paypal_sub_id`),
  actives ou arrêtées chez PayPal.
- **Toutes les autres fiches sont gérées à la main** (`managed_by_hand`) : actif ou offert posé par l'admin, avec ou sans
  échéance ; **essai posé par l'admin** (`subs set --status trial`, source `manual`) ; import ; exempté ; à qualifier ;
  ancienne grâce manuelle. Elles ne sont **jamais suspendues** et ne reçoivent **aucun rappel** (ni J-7, ni J-1, ni
  grâce, ni fin).
- À leur échéance, `decide` renvoie `ManualDue` : **une** information admin (`alerts::admin`, « Abonnés : échéance à gérer
  à la main », une ligne par compte), une seule fois par échéance (colonne `due_noted` de `state/subscriptions.db`, ajoutée
  à l'ouverture le 08/10, + événement d'historique `due_noted` caché du membre ; notée seulement si l'alerte est partie ;
  une nouvelle échéance donne une nouvelle information). Sur `/accounts` : « À gérer (échéance passée) ».
- Pour qu'un compte soit suivi par le cycle, lui rattacher un abonnement PayPal (`homelabctl subs link`).
- Un ancien binaire reste compatible avec la base (liste de colonnes explicite) mais **reprendrait les rappels et les
  suspensions des fiches manuelles**.

### Règles PayPal (revue du 07/10)

- Abonnement PayPal lié et non arrêté (`auto_renews`) : **jamais de rappel avec lien de paiement** (un clic = second
  abonnement = deux prélèvements) ; une information J-7 sans lien ; grâce à échéance + `paypal_margin_hours` (36) ;
  prélèvement raté ⇒ mail de suspension sans lien (mettre à jour le moyen de paiement chez PayPal).
- Second abonnement d'un compte dont l'abonnement lié est ACTIVE chez PayPal : ni rattaché ni compté
  (`Payment::Duplicate`), alerte admin une fois par jour, `/premium/activate?err=deja` ; le rembourser ou le résilier **à
  la main** dans PayPal (argent réel). La fiche garde `paypal_status` et `paypal_paid_at`.
- `subscription_reconcile` n'applique qu'un paiement constaté (`last_payment` > `paypal_paid_at` + 12 h). Facturation due
  depuis plus de 36 h sans paiement : alerte « Prélèvement PayPal en attente » (une par abonnement et par jour), rien n'est
  prolongé. Arrêt chez PayPal : noté, accès jusqu'à l'échéance payée.
- `/premium?compte=X` ne dit « déjà abonné » que pour un lien signé `&k=` (HMAC de `HOMELABD_ONBOARD_TOKEN` et du compte,
  16 hexa, porté par les mails) : la page publique ne révèle pas qui paie. Renouveler le jeton rend les anciennes clés
  caduques, sans risque.
- Compte Jellyfin disparu : les fiches sans rien à perdre (à qualifier, suspendu ou exempté, sans échéance ni PayPal)
  partent ; les autres, `max_orphan_removals_per_run` (3) au plus d'un coup. Au-delà, ou si Jellyfin renvoie une liste
  vide ou illisible, **rien n'est retiré**, le cycle ne touche plus ces fiches et l'admin est prévenu (`orphan_held`) :
  pour une suppression en masse voulue, relever la clé le temps d'un passage.
- Essai : pas de rappel J-7 dès l'inscription ; un parrainage ne date jamais un accès offert sans échéance.

### Reste ouvert

- Export CSV PayPal des abonnés historiques à importer (`homelabctl subs import <csv>`) ; à défaut, ces comptes restent
  gérés à la main.
- Identifiants sandbox collés dans une conversation le 20/09 : à régénérer.
- « Mon compte » affiche encore la date posée par l'admin d'une fiche gérée à la main (« Actif jusqu'au … ») et, pour une
  fiche en grâce ou un essai posé à la main, des textes du cycle (« Renouvelle pour éviter la coupure », « M'abonner ») :
  au propriétaire de dire s'il faut les masquer. `homelabctl subs list` n'affiche pas l'état « à gérer ».

## 4. Mon compte

- Script Injector « Groscailloux Mon compte » (`branding/jellyfin/gc-account-loader.js`) → `/gc-compte/app.js` (NPM hôte
  1, `location` en base **et** dans `1.conf`, sauvegarde `backups/npm-20260920-gc-compte/`) → homelabd `/compte/`.
  Identité = jeton de session Jellyfin vérifié par `/Users/Me` (cache 5 min).
- Contenu : abonnement, appareils (déconnexion vérifiée sur `LastUserId`), mot de passe (page `/bienvenue`), langue
  VF/VO, taille des sous-titres, parrainage, historique, avancement des demandes, choix de la voie russe pour les comptes
  autorisés.
- Le même script enrichit l'onglet **Téléchargements** de Jellyfin Enhanced (09/10) : un élément bloqué de la file des
  Arrs y prend un badge explicite (« Déjà disponible », « Sans source », « Non reconnu »…) et une ligne qui dit au membre
  ce qui se passe ; l'admin voit aussi le message d'origine nettoyé. Route `GET /compte/api/downloads` (cache 30 s,
  `detail` envoyé aux seuls administrateurs) ; familles et règles : [arrs-et-indexeurs.md](arrs-et-indexeurs.md#10-avertissements-de-la-file-onglet-téléchargements). Détails : [lecture-et-transcodage.md](lecture-et-transcodage.md), [jellyfin-interface.md](jellyfin-interface.md),
  [voie-russe.md](voie-russe.md).
