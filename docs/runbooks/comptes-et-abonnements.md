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
- **Habillage des mails aux membres (15/10)** : un seul gabarit sombre aux couleurs de Groscailloux TV
  (`homelab_core::mail_layout`) pour la bienvenue, l'activation, l'abonnement, « Aide et annonces » et « disponible » :
  logo, un seul bouton, vraie version texte à côté (`multipart/alternative`), HTML sous 40 Ko (Gmail coupe vers 102 Ko).
  Logo, bandeau, affiches et fonds voyagent **dans le mail** (`multipart/related`, `cid:`), lus dans Jellyfin
  (`format=Jpg`) ou chez TMDB en secours. Jamais le nom, le logo ni le rouge d'un service commercial. Chaque mail est
  rendu par une fonction pure (`welcome`, `subscriptions`, `chat`, `available_mail`) ; les mails d'admin restent en texte.
- **Voir chaque mail avant de le valider** : `homelabctl mail-test <adresse> --all` fait envoyer par homelabd un exemple de
  **chaque** mail qu'un membre peut recevoir (17 types : `homelabctl mail-test --help`), compte fictif « Exemple »,
  objet « [Exemple] … », à 2 s d'écart (une à deux minutes) ; `--kind <type>` (répétable) pour un seul. Pour le
  placement Gmail (Principale, Promotions ou spam ; « Afficher l'original » : SPF, DKIM, DMARC), l'adresse est une boîte
  Gmail de l'admin, donnée sur la ligne de commande et jamais écrite dans le dépôt. Les mails « disponible » prennent de
  vrais titres arrivés (Jellyseerr, images comprises) ; Jellyseerr injoignable : titres d'exemple sans image. Aucun compte
  ni abonnement touché ; seul un lien de démonstration est émis (comme `mail-test` sans option, qui envoie toujours le
  mail de bienvenue d'exemple). **Sans rien envoyer** : `homelabctl mail-test --dump <dossier>` (sans homelabd, état
  jamais écrit, lien de bienvenue factice) écrit `<type>.html` (images dans `img/`, à ouvrir dans un navigateur),
  `<type>.eml` (message complet) et `<type>.txt`. `--all`/`--kind` demandent un homelabd installé avec la même version.
- **Gmail sombre sur iPhone : à contrôler en premier**. La toile du mail doit rester sombre : c'est la preuve que les
  dégradés du bloc `<style>` réservé à Gmail (`STYLE_GMAIL` de `mail_layout`) survivent au mode sombre (les sources
  n'ont testé que des dégradés en ligne). Si elle devient claire, retirer l'encre du bouton principal (`ink_button`,
  classe `gc-o`) : son libellé blanc serait alors simplement inversé par Gmail, sombre sur bleu clair (5,5:1), au lieu
  de rester clair sur un bleu devenu clair.
- **Mails « disponible »** (tâche `media_mail`, 2026-10-15) : remplacent ceux de Jellyseerr. Livrés **sans envoi aux
  membres** (`[tasks.media_mail] send_to_members = false`, bloc commenté dans `homelab.toml` jusqu'à l'installation de
  homelabd **et** homelabctl) : journal, état `logged`, copie à `MEDIA_MAIL_PREVIEW_TO` si `.env` la donne. Bascule,
  référence du premier passage et garde-fous : [AUTOMATION.md](../AUTOMATION.md#media_mail--5-min-2026-10-15). Un membre
  qui répond « stop » (ou clique « Se désabonner » dans Gmail, qui envoie un mail depuis son adresse) : son adresse,
  son pseudo ou son id Jellyfin dans `MEDIA_MAIL_OPTOUT` (`.env`), puis redémarrage de homelabd.
- **Bascule, une fois les exemples validés** (dans une fenêtre d'installation) :
  1. `homelab.toml` : décommenter **tout** le bloc `[tasks.media_mail]` et y passer `send_to_members = true` ; même
     valeur pour le défaut du code (`MediaMail::default` dans `config.rs`) : une fois le bloc décommenté,
     `homelab_toml_equals_code_defaults` compare tout le fichier aux défauts du code, et `cargo test` doit passer ;
  2. installer **homelabd et homelabctl ensemble**, depuis l'arbre de travail de `/opt/homelab` (`deny_unknown_fields` :
     un binaire d'avant refuse le bloc), redémarrer homelabd, puis les contrôles d'après redémarrage
     ([homelabd.md](homelabd.md)) et `homelabctl run media_mail --dry-run`. Si la tâche n'avait jamais tourné, son
     premier passage est la référence (tout ce qui est déjà disponible est noté sans mail) ; sinon il note sans mail
     ce qui attendait encore (déjà annoncé par Seerr) ;
  3. **dans la même minute**, arrêter les mails « Média disponible » de Seerr. **À trancher avant la bascule** : Seerr
     3.5 n'a **pas** de types de notification pour son agent e-mail (vérifié dans le code de la 3.5.0 le 15/10 ;
     Discord, Slack… en ont, l'e-mail non), donc pas de case « Média disponible » à décocher pour tout le monde dans
     Paramètres → Notifications → E-mail. Deux voies :
     - **couper l'agent e-mail** (même écran, décocher l'activation), **voie par défaut** : marche avec le code actuel
       et ne change rien à la connexion ni au mot de passe des membres. Leurs comptes sont des comptes Jellyfin : le
       « Mot de passe oublié ? » de Seerr, qui disparaît avec l'agent, ne leur a jamais servi (mot de passe oublié =
       lien de bienvenue, ou « Mon compte »). Côté admin, « Add User » dans Seerr demande alors un mot de passe : en
       saisir un quelconque, `user_poller` supprime ce compte local et lance l'onboarding (lien de bienvenue). Seerr
       n'envoie plus aucun mail (demande approuvée, refusée, en échec, mot de passe généré ; mails de demandes à
       l'admin, que le salon Discord admin garde). Les membres ne voient plus l'onglet E-mail de leurs notifications ;
       le choix déjà enregistré reste lu par `media_mail`, et `MEDIA_MAIL_OPTOUT` prend le relais pour un « stop » ;
     - **garder l'agent**, seulement si le propriétaire tient aux mails de statut de Seerr (approuvée, refusée, en
       échec), et décocher « Média disponible » **compte par compte** (`notificationTypes.email` de chaque membre) :
       bien plus risqué, demande d'abord un changement de `media_mail`, qui lit aujourd'hui cette case comme un refus
       du membre (`opted_out` : plus aucun mail « disponible » pour personne), le report dans `MEDIA_MAIL_OPTOUT` des
       membres qui l'avaient décochée eux-mêmes, et l'onboarding (Seerr coche tout pour un compte neuf) ; écriture
       compte par compte, comptée et vérifiée avant et après.

  Au pire un titre reçu deux fois, pour ce qui arrive dans les 5 min qui précèdent l'arrêt des mails de Seerr. Tant
  que Seerr n'est pas arrêté, il envoie toujours son propre mail (anglais mêlé de français).
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
