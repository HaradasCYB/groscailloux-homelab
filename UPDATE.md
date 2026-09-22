# Journal des versions de Groscailloux

Groscailloux est un serveur de streaming privé pour une petite communauté : Jellyfin pour regarder, Jellyseerr
pour demander, et une chaîne d'acquisition automatique derrière. Ce journal retrace les grandes étapes, de la
première pile Docker à la version 1.0.0 qui clôt la bêta.

Les dates antérieures au dépôt git (avril-août 2026) sont **reconstituées** à partir des configurations et des
notes d'exploitation ; à partir du 10/09/2026, chaque ligne renvoie aux commits. Depuis le 21/09/2026, les versions
publiées le même jour sont regroupées sous la dernière d'entre elles (les tags git intermédiaires restent).

## En bref

| Version | Date | Thème |
| --- | --- | --- |
| **[1.19.0](#1190--21092026--suivi-des-demandes-langue-et-canari)** | 21/09/2026 | **Suivi des demandes, langue et canari** |
| [1.18.0](#1180--20092026--abonnement-automatique--mon-compte--discord-et-bienvenue) | 20/09/2026 | Abonnement automatique, « Mon compte », Discord et bienvenue |
| [1.15.1](#1151--19092026--télé-français-partout-airplay-et--lire-sur-) | 19/09/2026 | Télé, français partout, AirPlay et « Lire sur » |
| [1.14.0](#1140--18092026--animés-complets-tout-par-la-seedbox-lecture-qui-tient-la-nuit) | 18/09/2026 | Animés complets, tout par la seedbox, lecture qui tient la nuit |
| [1.8.0](#180--17092026--recherche-par-identifiant-un-seul-indexeur-menus-anime) | 17/09/2026 | Recherche par identifiant, un seul indexeur, menus Anime |
| [1.1.1](#111--16092026--la-lecture-saide-elle-même-adaptée-aux-téléviseurs) | 16/09/2026 | La lecture s'aide elle-même, adaptée aux téléviseurs |
| [1.0.0](#100--15092026--fin-de-la-bêta--une-seule-plateforme-vps--seedbox) | 12 → 15/09/2026 | Fin de la bêta : une seule plateforme VPS + seedbox |
| [0.9.0](#090--10--11092026--la-refonte--homelabd) | 10 → 11/09/2026 | La refonte : l'automatisation réécrite en Rust (homelabd) |
| [0.3.0](#030--août-2026-reconstitué--jellyfin-enrichi) | août 2026 | Jellyfin enrichi par les plugins |
| [0.2.0](#020--fin-mai-2026-reconstitué--stabilisation) | fin mai 2026 | Stabilisation |
| [0.1.0](#010--30042026--06052026--les-fondations) | 30/04 → 06/05/2026 | Les fondations : demander un film depuis Jellyfin, tout arrive seul |

---

## 1.19.0 — 21/09/2026 — Suivi des demandes, langue et canari

- **Où en est ma demande ?** Dans l'onglet Demandes de Jellyfin, chaque demande en cours affiche une barre
  d'avancement et une estimation : recherche (prochaine tentative), téléchargement (pourcentage, temps restant),
  ajout à la médiathèque, disponible.
- **Langue de lecture** : la piste française est choisie d'office quand elle existe, sous-titres français seulement
  quand l'audio n'est pas en français. Dans « Mon compte », chacun peut passer en « Toujours en VO, sous-titres
  français ».
- **Sous-titres automatiques** : Bazarr (seedbox) complète les sous-titres français manquants des nouveautés.
- Côté serveur : un canari de lecture vérifie toutes les 15 minutes qu'un transcodage démarre vraiment et prévient
  l'administrateur avant les membres.
- Journal des versions compacté : les versions publiées le même jour sont regroupées sous la dernière d'entre elles
  (les tags git intermédiaires existent toujours), et une annonce peut être publiée sur le tchat depuis la ligne de
  commande (`homelabctl chat announce`).
- Correctif du jour : la barre d'avancement ignorait tout téléchargement lancé par la plateforme sur la seedbox (le
  chemin normal de toute nouveauté) et affichait « recherche, prochaine tentative dans 7 jours » pendant que le titre
  se téléchargeait (*Black Clover* à 46 %). Elle lit maintenant aussi les téléchargements de qBittorrent. Sur
  téléphone, la barre chevauchait la ligne « membre • date » : elle est passée en dessous, sur toute la largeur.
  L'icône « Mon compte » de l'en-tête restait invisible après un rechargement tant qu'on n'avait pas cliqué dessus.
- **Les sous-titres arrivent tout de suite sur les titres de la seedbox.** Jusqu'ici, à la première lecture d'un épisode
  en VOSTFR, Jellyfin devait extraire les sous-titres en relisant tout le fichier à distance (2 à 12 minutes) : ils
  arrivaient en retard, ou jamais si l'on changeait de piste entre-temps. Ils sont désormais extraits une fois pour
  toutes sur la seedbox, à côté de la vidéo, **dans leur format d'origine** (l'ASS des animés garde ses couleurs et ses
  panneaux ; la piste malentendants reste à part), et Jellyfin les lit instantanément. Les titres récents passent en
  premier, les nouveautés sont traitées à l'arrivée.
- **Les films retrouvent leur vrai titre.** Trente films s'affichaient sous le nom du fichier téléchargé
  (« Matrix.Reloaded.2003.MULTi.VFF.1080p… »), parce que Jellyfin préférait le titre inscrit dans le fichier par celui
  qui l'a publié. C'est corrigé pour ceux-là et pour les suivants. Très visible sur une télé, où il n'y a que les
  titres et les affiches à l'écran.
- **Trois collections sans affiche** (Anime, Tendances, Univers Marvel) en ont une, et la rangée « Récemment ajouté »
  ne répète plus les mêmes titres deux fois.
- **Guide des membres à jour** : connexion détaillée appareil par appareil (dont la connexion rapide par code sur
  téléviseur), deux nouvelles étapes « Sous-titres et langues » et « Mon compte », suivi des demandes avec barre
  d'avancement, et mot de passe en libre-service.
- **Taille des sous-titres** : dans « Mon compte », choix Normale / Grande / Très grande pour l'appareil en cours,
  appliqué **aussitôt, même en pleine lecture** (sous-titres SRT ; les sous-titres stylés des animés gardent leur
  taille propre). « Grande » par défaut.

## 1.18.0 — 20/09/2026 — Abonnement automatique, « Mon compte », Discord et bienvenue

Regroupe 1.16.0, 1.17.0, 1.17.1, 1.17.2 et 1.18.0.

### Abonnement et « Mon compte »

- **Abonnement activé tout seul** : sur la page Premium, tu indiques ton nom de compte, tu payes, et ton compte est
  actif dans la seconde ; chaque mensualité le prolonge automatiquement. Tu as payé sans passer par la page ? Un
  formulaire rattache ton abonnement avec son identifiant PayPal.
- **Fin d'abonnement en douceur** : rappels par mail 7 jours et 1 jour avant l'échéance, 3 jours de grâce, puis
  l'accès se met en pause (rien n'est supprimé) et repart dès le paiement.
- **« Mon compte » dans Jellyfin** (icône en haut à droite) : état de l'abonnement et échéance, bouton d'abonnement,
  appareils connectés avec déconnexion, changement de mot de passe, code de parrainage, historique.
- **Essai gratuit de 7 jours** à l'inscription, et **parrainage** : 15 jours offerts au parrain et au filleul au premier
  paiement du filleul.
- Côté administrateur : colonne Abonnement sur la page Comptes (offrir pour N jours ou sans limite, exempter,
  prolonger, suspendre), commande `homelabctl subs`, cycle en mode observation la première semaine.

### Les nouveautés sur Discord

- **Un salon Discord pour les membres** : chaque film ou série qui arrive (avec l'affiche, un seul message par
  téléchargement), les demandes (nouvelle, validée, refusée, disponible — avec le pseudo du demandeur) et les
  annonces de l'administrateur y sont publiés. Plus besoin d'attendre un mail.
- **Un salon privé pour l'administrateur** : alertes de lecture, indexeur bloqué ou débloqué, nouveau compte à
  activer, mail de bienvenue non parti, service relancé, santé des outils d'acquisition, récapitulatif du tchat.
  Réglé par deux webhooks dans `.env` ; `homelabctl discord apply|test|remove` configure ou retire tout.

### Bienvenue comme chez les grands

- **Un vrai mail de bienvenue, qui arrive.** Fini le mail brut avec identifiant et mot de passe en clair (que Gmail
  rangeait en spam) : un mail sobre au nom de Groscailloux, avec **un bouton** vers une page où le membre **choisit
  son mot de passe**. Le lien vaut une heure et ne sert qu'une fois ; expiré, la page en renvoie un nouveau sur
  demande — ce qui sert aussi de « mot de passe oublié ».
- **Une page d'inscription** (`/inscription`) : pseudo + adresse, et le compte est créé en attente de validation ;
  l'administrateur est prévenu, active depuis la page Comptes, et le membre reçoit « ton compte est actif ».
- Aucun mot de passe ne circule plus par mail ni ne s'affiche dans les outils d'admin.

### De la place sur les deux disques, et deux correctifs de lecture

- **Déménagement VPS → seedbox** (`scripts/move-to-seedbox.py`) : 30 titres (≈ 450 Go) copiés sur la seedbox,
  vérifiés fichier par fichier, reconnus par Sonarr/Radarr là-bas, puis seulement retirés du serveur ; Jellyfin les
  affiche depuis la seedbox sans rien perdre (historique compris). Le transfert lancé l'après-midi a gêné la lecture
  d'un membre : il ne tourne plus que le matin (08 h 30 – 12 h 30), sur un seul flux plafonné, et un titre en cours
  de lecture attend.
- **Ménage de la seedbox** (`scripts/seedbox-cleanup.py`) : 62 téléchargements jamais entrés dans la médiathèque
  (ISO, logiciels, musique, journaux, sport…) et 18 entrées de corbeille retirés, ≈ 800 Go libérés ; les torrents
  trop récents pour le tracker sont retirés automatiquement au 7ᵉ jour.
- **Sauvegarde corrigée** : la sauvegarde de nuit embarquait le cache de lecture de la seedbox (149 Go d'archive au
  lieu de 3) et avait rempli le disque à 92 %. Archive supprimée, cache exclu.
- **« Mes médias » en haut de l'accueil**, sur tous les appareils et tous les comptes (sur téléphone, il fallait tout
  faire défiler pour voir la rangée des bibliothèques) ; les deux entrées « Découvrir » fantômes ont disparu.
- **La lecture restait sur « chargement »** pour tout ce qui devait être converti à la volée : l'espace temporaire de
  conversion était plein de restes de la veille. Vidé, purge automatique toutes les minutes (alerte à
  l'administrateur si ça sature), espace doublé à 4 Go.

## 1.15.1 — 19/09/2026 — Télé, français partout, AirPlay et « Lire sur »

Regroupe 1.14.1, 1.14.2, 1.14.3, 1.14.4, 1.15.0 et 1.15.1.

### Une interface pensée pour la télé

Sur un téléviseur (appli LG, Samsung… qui affichent le site), l'interface est désormais différente de celle du PC,
du téléphone et de la tablette — qui, eux, ne changent pas d'un pixel.

- **Accueil allégé** : plus de grand bandeau animé ni de bandes-annonces YouTube (ce qui pesait le plus sur une
  télé) ; les rangées se limitent à l'essentiel — Reprendre, À suivre, Films et Séries ajoutés, Anime, Collections,
  Ma médiathèque. Les rangées Tendances, Genres, Mieux notés, Découvrir, Mes demandes… restent sur les autres appareils.
- **Télécommande** : en-tête fixé en haut de l'écran (il défilait hors de l'écran dès que l'on parcourait les
  rangées) avec seulement Notifications, Rechercher et Profil ; onglets resserrés, focus plus visible.
- **Lecture d'abord** : sur la fiche d'un titre, « Lire » est plus grand et les blocs secondaires (Plus comme ça,
  genres, tags, studios, services de streaming, langues) sont cachés ; la distribution reste.
- **Pas de tchat sur télé** : sans clavier il était inutilisable ; il n'est plus chargé du tout.

### En français sur tous les appareils

- **L'interface est en français même sur un appareil réglé en anglais.** Jellyfin prenait la langue de l'appareil
  tant que le membre n'avait pas choisi la sienne : un PC en anglais affichait « Home », « Favorites », « Ends at
  11:09 PM ». Le français est posé d'office à la première ouverture (la page se recharge une fois). Un membre qui a
  choisi lui-même une autre langue la garde.

### « Lire sur » et AirPlay

- **Le menu « Lire sur » propose de nouveau tous vos appareils connectés**, quel que soit le réseau. Depuis le
  16/09, il n'affichait que ceux partageant l'adresse de votre box ; un iPhone protégé par le Relais privé iCloud
  (ou un VPN) ne la partage pas toujours, et sa propre TV disparaissait. Les appareils des autres membres restent
  invisibles ; quand aucun autre appareil de votre compte n'est connecté, le menu le dit au lieu de rester vide.
- **Sur iPhone, iPad et Mac, ce menu propose AirPlay**, et l'entrée fait tout le travail : elle ferme le menu, lance
  le titre affiché (depuis sa fiche ou depuis le bandeau de l'accueil) et ouvre le sélecteur d'écran d'Apple dès que
  la vidéo est prête ; si une lecture est déjà en cours, le sélecteur s'ouvre directement. La mention « Google Cast
  non pris en charge » (Cast n'existe que dans Chrome et sur Android) disparaît partout où Cast n'est pas disponible.

### Films demandés sous 5 minutes, et correctifs

- **Un film demandé est cherché dans les 5 minutes**, comme une série : Radarr ne cherche plus lui-même et le flux
  RSS ne ramène que les nouveautés, un film ancien (*Matrix*) attendait le rattrapage du lendemain.
- **qBittorrent du VPS de nouveau joignable** : la mise à jour de nuit l'avait laissé attaché à l'ancien conteneur
  VPN (imports côté VPS en pause pendant 11 h). Recréé, port VPN reposé, procédure documentée.
- **Homarr affiche enfin les téléchargements de la seedbox** (le widget ne recevait que 10 torrents, tous déjà
  terminés donc masqués) ; sur la page d'état, une saison notée « sans release » disparaît une fois complétée.

## 1.14.0 — 18/09/2026 — Animés complets, tout par la seedbox, lecture qui tient la nuit

Regroupe 1.9.0, 1.10.0, 1.11.0, 1.12.0, 1.13.0, 1.13.1 et 1.14.0. Journée marquée par un audit des quatre
applications de téléchargement (croisé avec le guide du tracker), un test en conditions réelles d'une demande
d'animé à 50 épisodes (18 minutes entre la demande et la disponibilité) et un audit complet de la chaîne de lecture
(14 jours de mesures).

### Lecture

- **L'avance rapide saute 10 secondes au lieu de 30**, par le bouton du lecteur comme par les flèches du clavier,
  dans les deux sens. Appliqué à tous les comptes et posé d'office sur les nouveaux (recharger l'application si elle
  était déjà ouverte).
- **Ce qui a été regardé la veille reste prêt le lendemain.** Une tâche de fond (les vignettes de défilement)
  tournait six heures chaque matin sans jamais finir et rapatriait plus d'un téraoctet, ce qui vidait la mémoire
  tampon des médias distants. Elle ne tourne plus que le dimanche, et la mémoire tampon passe de 20 à 120 Go.
- **Démarrage et sauts plus vifs sur les médias distants** (blocs de lecture plus petits), **le retour arrière ne
  relance plus l'encodage** avant cinq minutes, le serveur garde trois minutes d'avance quand une connexion faiblit,
  et **deux encodages simultanés tiennent le temps réel** (plafond de cœurs levé).
- **Plafond à l'acquisition** : les prochaines prises restent sous ~15 Mbit/s en 1080p, sans toucher à l'existant ni
  imposer de limite par membre. Alerte automatique quand un lecteur tourne en boucle sur un flux.
- Les changements qui coupent brièvement la lecture s'appliquent automatiquement à 04:30, hors des heures de
  visionnage.

### Animés

- **Pour les animés, la priorité change** : d'abord les versions **MULTi** (piste japonaise et française), puis la
  **VOSTFR**, le doublage seul ensuite. Séries et films ne changent pas : le français reste prioritaire.
- **Les épisodes qui « n'existaient pas » arrivent enfin.** Beaucoup d'animés sont diffusés en plusieurs parties,
  publiées chacune sous son propre nom : une saison restait incomplète pour toujours, sans que rien ne le signale
  (Bleach s'arrêtait à 34 épisodes sur 48). La plateforme reconnaît ces parties et les range au bon endroit, sans
  jamais prendre de risque : le contenu doit combler exactement les épisodes manquants, aucun épisode présent n'est
  remplacé, et au moindre doute rien n'est fait. Bleach est passée à **48 épisodes sur 48**.
- **Les épisodes sans titre définitif s'importent**, la **numérotation japonaise** (« 367 » plutôt que « saison 18,
  épisode 7 ») est comprise (23 séries corrigées, sans toucher aux fichiers), **les sous-titres livrés à côté de la
  vidéo ne sont plus perdus** à l'import, et un nommage de fansub qui bloquait un lot entier est enfin lu (*Erased*,
  disponible en entier).
- **Plus rien ne se perd en silence** : les téléchargements terminés que rien n'a pu ranger, et les saisons
  qu'aucune version ne couvre, apparaissent dans la page d'état avec la raison.

### Acquisition

- **Une saison entière arrive en un seul passage** : tous les épisodes manquants sont pris d'un coup à partir de la
  même recherche (avant : un épisode toutes les deux heures, près d'une journée pour une saison). *BLACK TORCH* est
  passé de 1 à 11 épisodes en quelques minutes.
- **Quatre fois plus de recherches possibles** : deux accès distincts à la source, chacun avec son compteur, une
  part réservée aux recherches lancées à la main, et **plus de coupure quand un accès sature** (tout continue sur
  l'autre). Films : rattrapage toutes les 15 minutes au lieu d'une heure.
- **Le serveur principal ne télécharge plus rien tout seul** : tout ce qui est neuf est récupéré par la seedbox.
  Ses fiches, qui avaient été mises par erreur sur un profil préférant la version japonaise, sont revenues sur le
  profil français (24 séries, 46 films, rien retéléchargé) ; celles qui n'avaient aucune règle de langue suivent
  désormais les mêmes (français d'abord, 1080p maximum).
- **Le format vidéo ne bloque plus une recherche** : les versions HEVC (x265) étaient écartées au profit du H.264.
  Mesuré sur le serveur, les deux demandent le même travail et les appareils des membres lisent le HEVC
  directement ; les écarter revenait à refuser la seule version française disponible.
- **Recherche élargie** quand l'identifiant ne couvre qu'une partie d'une saison (titres de la série essayés en
  plus), **dossiers de saison rétablis** pour les séries créées par une demande, et **redemander un titre supprimé
  fonctionne** : ses fichiers de partage, conservés pour le tracker, sont réutilisés tels quels.
- **Nouveautés repérées plus vite** (flux relu toutes les 15 minutes, rangement 2 minutes après la fin du
  téléchargement) et **corbeille de 14 jours** sur le serveur principal.

## 1.8.0 — 17/09/2026 — Recherche par identifiant, un seul indexeur, menus Anime

Regroupe 1.2.0, 1.3.0, 1.4.0, 1.5.0, 1.6.0, 1.7.0 et 1.8.0. Point de départ : les saisons 2, 3 et 4 d'un animé
demandé ne partaient jamais. Sonarr cherchait la série sous ses noms anglais et japonais alors que l'indexeur la
classe sous son nom français, envoyait des dizaines de requêtes d'un coup jusqu'à se faire bloquer 24 h, et un
échec immobilisait la saison trois jours.

### Recherche et indexeur

- **Les séries se cherchent par identifiant**, plus par leur nom : une demande part dans les dix minutes, avec une
  seule requête par saison, que la série s'appelle *Shingeki no Kyojin*, *Attack on Titan* ou *L'Attaque des Titans*.
  Les noms français et d'origine servent de repli. Films : un film encore manquant est recherché une seconde fois.
- **Fini les « aucun indexeur disponible »** : les applications de téléchargement ne lancent plus de recherche
  elles-mêmes (elles gardent le flux d'annonces), toutes les recherches passent par la plateforme avec un budget
  horaire et une réserve pour les recherches manuelles, et deux clés distinctes séparent le flux des recherches.
- **Le ménage dans les indexeurs** : 34 indexeurs publics inutilisés retirés, il n'en reste qu'un ; les recherches
  ne dépassent plus le délai d'attente, le journal d'erreurs s'est vidé, deux services inutiles arrêtés.
- **Indexeur toujours disponible** : mis en pause par Sonarr ou Radarr, il est remis en service une heure après son
  dernier échec, l'admin prévenu ; une recherche en échec est retentée dans l'heure au lieu du lendemain.
- **Plus de souplesse sur la langue** : sans version française, la version originale est acceptée en dernier
  recours (VF, MULTi, FRENCH, VOSTFR, puis VO). **Choix plus sûr** : versions démesurées écartées (plus de 6 Go par
  épisode, 25 Go par film), une seule source passe derrière une version bien partagée, et une œuvre dérivée
  (mini-épisodes, spéciaux, OVA, parodie…) n'est plus choisie à la place de la série (*Smoking Behind the
  Supermarket with You* avait été pris en mini-épisodes de 12 minutes).
- **Recherche manuelle** (administration) : une page dédiée cherche une saison ou un film par identifiant, chez
  l'indexeur principal et chez un indexeur d'animés, et affiche toutes les releases avec leurs écarts. Un clic lance
  le téléchargement, une seule requête est envoyée.
- **Demande supprimée = titre supprimé** : retirer une demande efface la fiche et ses fichiers, libère le torrent et
  rend le titre à nouveau demandable.

### Menus Anime et Films d'animation

- **Deux nouveaux menus** dans Jellyfin : **Anime** (séries) et **Films d'animation**, pour l'animation japonaise.
  Les autres dessins animés et les séries japonaises en prises de vues réelles restent dans Séries et Films.
- **Rangement automatique** d'après la fiche TMDB de chaque titre (genre Animation et origine japonaise) : un animé
  demandé arrive au bon endroit, ce qui passe à travers est rangé dans la demi-heure, jamais pendant qu'on le
  regarde ; l'admin peut forcer avec un tag. **27 titres déjà présents rangés** sans perte (historique conservé).

### Tout le monde voit tout

- **Demandes et Calendrier pour tout le monde** : chaque membre voit les demandes de tous (avec leur auteur) et le
  calendrier complet des sorties, comme l'administrateur, qui reste le seul à valider ou refuser.

### Identification et imports

- **Les séries ne partent plus sur leur spin-off** : Jellyfin confondait *The Walking Dead* avec *Dead City*,
  *L'Attaque des Titans* avec un dérivé, *Tomb Raider* avec *Lara Croft*. Chaque fiche est comparée à la référence
  des applications de téléchargement et corrigée seule ; les nouveaux dossiers portent l'identifiant du titre.
- **Imports plus sûrs** : un import ne remplace plus jamais un épisode déjà présent, le rattachement suit celui de
  Sonarr, un torrent au titre japonais ou anglais retrouve la fiche existante, et une série tout juste téléchargée
  s'affiche complète (le dossier est relu en entier avant de prévenir Jellyfin).

## 1.1.1 — 16/09/2026 — La lecture s'aide elle-même, adaptée aux téléviseurs

Regroupe 1.1.0 et 1.1.1. Un membre a vu sa série saccader alors que le serveur ne peinait pas : sa connexion était
descendue sous le débit du film. Jellyfin ne choisit sa qualité qu'au démarrage et relance le flux en boucle quand
le tampon se vide.

- **Bandeau d'aide dans le lecteur** : quand l'image se fige trois fois en trois minutes, un bandeau propose de
  réduire la qualité. Un clic et le film repart au même endroit ; le choix est retenu pour cet appareil. Mesuré sur
  une connexion bridée à 2 Mbit/s : 57 s d'image par minute au lieu de 15.
- **Guide complété** : une section « Ça saccade ? » explique « Auto », les 5 Mbit/s d'une lecture directe et comment
  fixer la qualité à la main. Aucun plafond de débit n'est imposé côté serveur.
- **Téléviseurs** : les bandeaux (annonce, message de l'admin, aide à la qualité) se ferment avec la touche
  **Retour** et s'effacent seuls au bout de douze secondes ; le tchat tourne au ralenti sur ces appareils. Mesuré :
  nos scripts coûtent environ deux points de processeur sur l'accueil, rien pendant la lecture.
- Administration : diagnostic reconstitué à partir des journaux du proxy, de ffmpeg et du cache rclone ; métrique
  réseau de Telegraf corrigée (elle mesurait son propre conteneur) ; banc d'essai reproductible sans écriture sur la
  production.

## 1.0.0 — 15/09/2026 — Fin de la bêta : une seule plateforme VPS + seedbox

La version 1.0.0 réunit le VPS et la seedbox en **une seule plateforme** : un seul Jellyfin, une seule
bibliothèque, des demandes qui partent seules et des membres qui ont enfin un guide, un tchat et des règles
claires. C'est la fin de la bêta.

### Plateforme unifiée VPS + seedbox (12 → 13/09)

- Les médias de la seedbox sont lus **par le Jellyfin du VPS**, via un montage rclone (cache limité, reprise
  après coupure) ([`882949b`][882949b], [`ae84d87`][ae84d87], [`26b4fad`][26b4fad]).
- Plus de bibliothèques « (Seedbox) » : **Films** et **Séries** ont chacune un dossier VPS et un dossier
  seedbox, invisibles pour les membres.
- Les nouvelles demandes partent vers la seedbox ; aucun titre n'est importé deux fois d'une machine à
  l'autre, ni aucune saison en double ([`27c39c8`][27c39c8]).
- Un nouvel import sur la seedbox apparaît dans Jellyfin en quelques minutes (tâche `seedbox_refresh`).
- Schémas de l'infrastructure dans `docs/INFRA.md` ([`be5b771`][be5b771]).

### Acquisition automatique (12 → 14/09)

- **C411 seul en automatique** sur les quatre Radarr/Sonarr ; les autres indexers en recherche manuelle.
  Profils français : 1080p maximum, VF d'abord, VOSTFR en dernier recours ([`a6760a4`][a6760a4]).
- Imports débloqués automatiquement :
  - titres reconnus seulement par leur identifiant (`id_match_import`) ([`cd245e8`][cd245e8]) ;
  - releases françaises rejetées en « Unknown Series » (`unknown_series_grab`) ([`a8e1493`][a8e1493]) ;
  - torrents ajoutés à la main dans qBittorrent (`torrent_import`) ([`a6760a4`][a6760a4]).
- Catégories anime de C411 et limite d'API documentées ([`04bc345`][04bc345]) ; C411 reconnu sur ses deux
  domaines d'annonce (53 torrents relancés) ([`6b6d071`][6b6d071]).
- **Supprimer un film dans Jellyfin nettoie tout** : fichiers, fiche Radarr/Sonarr, demande Jellyseerr et
  torrent (après son partage minimal) ([`748cd53`][748cd53], [`4d5d18a`][4d5d18a]).

### Lecture fluide (12/09, 15/09)

- **98 % de lecture directe** : tâches lourdes (trickplay, analyses) entre 5 h et 13 h seulement, aucune
  lecture de vidéo à l'ajout d'un titre, priorité CPU à Jellyfin ([`a6760a4`][a6760a4], [`a2a7a62`][a2a7a62]).
- **2 écrans par compte** : une 3e lecture simultanée est arrêtée avec un message à l'écran ; plus de limite
  d'appareils connectés, qui bloquait les iPhone ([`3eb05e9`][3eb05e9]).
- **« Lire sur »** ne propose que ses propres appareils, sur le même Wi-Fi ([`b060a08`][b060a08]).

### Comptes et accès (14 → 15/09)

- **Comptes premium** : un compte non premium est suspendu, jamais supprimé ; plafonds de capacité ; page
  « Comptes » pour activer, suspendre ou supprimer ([`a99b7e2`][a99b7e2], [`597415e`][597415e],
  [`748cd53`][748cd53]).
- **Demandes validées automatiquement**, avec un quota de 10 films et 10 saisons par semaine
  ([`4d5d18a`][4d5d18a]).
- Écran de connexion sans liste de comptes, titre en français ([`b8f239f`][b8f239f]) ; page d'inscription qui
  retient son jeton ([`3b646ad`][3b646ad]).
- Page de soutien facultative, sans aucun lien avec l'accès au service ([`7161370`][7161370],
  [`676ae96`][676ae96]).

### Interface « Groscailloux TV » (14/09)

- Thème ElegantFin épinglé + calque maison bleu, logo Groscailloux TV, bannière avec bande-annonce en fondu
  ([`4b167cb`][4b167cb], [`6b6d071`][6b6d071]).
- Accueil façon Netflix : 16 rangées, collections françaises, rangée **« Tendances cette semaine »** calculée
  chaque jour d'après ce que regardent les membres (`trending`) ([`4b167cb`][4b167cb], [`2e8ccbb`][2e8ccbb]).
- Onglets en français : Découvrir, Demandes, Calendrier ; plugins inutiles retirés.

### Communauté : guide et tchat (15/09)

- **Guide des nouveaux membres**, en ligne (`/guide`) et en PDF, lié depuis le mail de bienvenue
  ([`cccd582`][cccd582]).
- **Tchat intégré à Jellyfin** : bulle en haut à droite, salons Annonces, Entraide et Discussion, conversation
  privée avec l'admin, bannière des annonces, récapitulatifs par mail ([`a9bcf2a`][a9bcf2a]).
- **Messages ciblés** : l'admin écrit en privé à un ou plusieurs membres (« ta période d'essai se termine
  samedi »), avec mail facultatif ([`f9982de`][f9982de]).
- Suppression de message confirmée dans le message lui-même, pour marcher dans les applis mobiles
  ([`5208a40`][5208a40]).

### Sécurité et administration (12 → 15/09)

- Homarr en deux tableaux (public pour les membres, privé pour l'admin) ; outils d'admin derrière
  l'authentification de Nginx Proxy Manager ; page d'état protégée ; qBittorrent plus jamais ouvert sans mot
  de passe ([`4367c22`][4367c22], [`b8ab624`][b8ab624], [`4e1612b`][4e1612b], [`954308c`][954308c]).
- Jellyfin voit enfin l'adresse réelle des clients (proxy de confiance corrigé) ([`b060a08`][b060a08]).
- Rotation de la clé Jellyseerr en une commande ; clé de la seedbox limitée à la lecture et à la suppression
  ([`b8f239f`][b8f239f], [`4d5d18a`][4d5d18a]).
- Mémoire de Homarr relevée après un arrêt ([`5374920`][5374920]).
- Règle de travail : **tout changement se teste dans un environnement de test** (comptes temporaires, instance
  séparée), jamais sur la production.

### Incidents et leçons retenues

- **10 → 12/09** : le retrait de Jackett et FlareSolverr a coupé les indexers publics pendant deux jours.
  Rétablis ([`822946d`][822946d]) ; on vérifie désormais qui utilise un service avant de le retirer.
- **14/09** : un bannissement dans qBittorrent a bloqué l'accès web de tout le monde (NPM vu comme un seul
  client) ; corrigé par la reconnaissance du proxy ([`954308c`][954308c]).
- **15/09** : une suppression d'appareils mal filtrée a déconnecté tous les membres. Plus aucune suppression
  en masse sans vérifier chaque élément.
- **15/09** : un film au titre ambigu mal identifié par Jellyfin restait « en cours » dans Jellyseerr ; la
  correction est documentée.

### Commits

[`822946d`][822946d] [`882949b`][882949b] [`cd245e8`][cd245e8] [`be5b771`][be5b771] [`ae84d87`][ae84d87]
[`a6760a4`][a6760a4] [`a2a7a62`][a2a7a62] [`26b4fad`][26b4fad] [`a8e1493`][a8e1493] [`4e1612b`][4e1612b]
[`4367c22`][4367c22] [`b8ab624`][b8ab624] [`04bc345`][04bc345] [`27c39c8`][27c39c8] [`954308c`][954308c]
[`a99b7e2`][a99b7e2] [`597415e`][597415e] [`5374920`][5374920] [`7161370`][7161370] [`676ae96`][676ae96]
[`748cd53`][748cd53] [`4b167cb`][4b167cb] [`2e8ccbb`][2e8ccbb] [`6b6d071`][6b6d071] [`3b646ad`][3b646ad]
[`4d5d18a`][4d5d18a] [`cccd582`][cccd582] [`b8f239f`][b8f239f] [`a9bcf2a`][a9bcf2a] [`b060a08`][b060a08]
[`f9982de`][f9982de] [`3eb05e9`][3eb05e9] [`5208a40`][5208a40]

---

## 0.9.0 — 10 → 11/09/2026 — La refonte : homelabd

Le serveur tournait avec une dizaine de scripts bash et des réglages faits à la main. Tout est repris dans le
dépôt, proprement.

- **homelabd**, un démon en Rust, remplace tous les scripts ; `homelabctl` le pilote en ligne de commande
  (état, simulation, lancement d'une tâche) ([`36a2506`][36a2506], [`488e66a`][488e66a]).
- **Secrets sortis du code** vers un fichier `.env` ; configuration versionnée dans `homelab.toml`
  ([`e820f61`][e820f61]).
- **Docker durci** : images épinglées par empreinte, contrôles de santé, limites de ressources, alertes de
  nouvelles versions (Diun) ([`2795fce`][2795fce]).
- **Auto-réparation** : la tâche `stack_health` relance tout service arrêté ou malade ; démarrage de
  Guacamole fiabilisé ([`e666986`][e666986]).
- Page d'inscription servie par homelabd ; script d'installation, documentation et intégration continue
  ([`42143c3`][42143c3], [`b513331`][b513331], [`aca132d`][aca132d], [`fa3acc8`][fa3acc8],
  [`35c4829`][35c4829]).

## 0.3.0 — août 2026 (reconstitué) — Jellyfin enrichi

Configurations des plugins datées de mi-août 2026.

- Demandes Jellyseerr directement dans Jellyfin (SeerrFin, onglets Films / Séries / Demandes).
- Bannière vedette (Media Bar), aperçu des épisodes dans le lecteur, notifications de nouveautés (NotifySync).
- Mode cinéma, génériques d'animés, synchronisation avec les listes d'animés.
- « Reprendre » sans doublons, limite de flux, thème Jellyfish.

## 0.2.0 — fin mai 2026 (reconstitué) — Stabilisation

- **Passer l'intro** avec Intro Skipper.
- Dashdot ne consomme plus 200 % de CPU (test de débit en boucle coupé).
- Grand ménage des connexions mortes dans Guacamole.

## 0.1.0 — 30/04/2026 → 06/05/2026 — Les fondations

Tag git [`v0.1.0`][v0.1.0].

- **Pile Docker complète** :
  - streaming et demandes : Jellyfin, Jellyseerr ;
  - acquisition : Sonarr, Radarr, Prowlarr, qBittorrent, pyLoad ;
  - accès : Nginx Proxy Manager, DuckDNS, Homarr ;
  - supervision : Grafana, InfluxDB, Telegraf, Dashdot ;
  - bureau à distance : Guacamole.
- **Un bouton « Demander » dans Jellyfin** (Jellyfin Enhanced + Jellyseerr) : on cherche un film, on le
  demande, **il arrive tout seul** dans la bibliothèque.
- **Ajout automatique** : tout fichier déposé est reconnu, rangé et importé sans intervention (`auto-import`).
- **Création de compte en une fois** : Jellyfin + Jellyseerr + mail de bienvenue, avec sa page web ; les comptes
  créés depuis Jellyseerr sont redirigés vers ce parcours.
- **VPN ProtonVPN** avec redirection de port pour partager correctement (après NordVPN), bascule VPN / direct.
- **Torrents gérés seuls** :
  - torrents bloqués remplacés ;
  - disque protégé quand il sature ;
  - ratio de partage adapté à chaque tracker.
- **Animés et séries** :
  - épisodes tout juste sortis importés sans attendre leur titre (TBA) ;
  - seules les saisons demandées sont suivies.
- **Français d'abord** : profils VF, VOSTFR en dernier recours ; H.264 préféré au HEVC (serveur sans carte
  graphique) ; indexers publics relancés.
- Commits : [`f4bb55f`][f4bb55f] [`994892c`][994892c] [`2cd0f84`][2cd0f84] [`8399207`][8399207]
  [`c9d8fb9`][c9d8fb9].

---

## Numérotation pour la suite

- **1.0.x** : corrections ;
- **1.x.0** : nouveautés ;
- **2.0.0** : changement majeur d'architecture.

Chaque version ajoute sa section en haut de ce fichier, avec ses commits.

<!-- liens des commits -->
[v0.1.0]: https://github.com/HaradasCYB/groscailloux-homelab/tree/v0.1.0
[f4bb55f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f4bb55f
[994892c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/994892c
[2cd0f84]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2cd0f84
[8399207]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8399207
[c9d8fb9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/c9d8fb9
[e820f61]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e820f61
[2795fce]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2795fce
[36a2506]: https://github.com/HaradasCYB/groscailloux-homelab/commit/36a2506
[42143c3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/42143c3
[b513331]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b513331
[488e66a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/488e66a
[aca132d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/aca132d
[e666986]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e666986
[fa3acc8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/fa3acc8
[35c4829]: https://github.com/HaradasCYB/groscailloux-homelab/commit/35c4829
[822946d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/822946d
[882949b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/882949b
[cd245e8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cd245e8
[be5b771]: https://github.com/HaradasCYB/groscailloux-homelab/commit/be5b771
[ae84d87]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ae84d87
[a6760a4]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a6760a4
[a2a7a62]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a2a7a62
[26b4fad]: https://github.com/HaradasCYB/groscailloux-homelab/commit/26b4fad
[a8e1493]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a8e1493
[4e1612b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4e1612b
[4367c22]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4367c22
[b8ab624]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b8ab624
[04bc345]: https://github.com/HaradasCYB/groscailloux-homelab/commit/04bc345
[27c39c8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/27c39c8
[954308c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/954308c
[a99b7e2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a99b7e2
[597415e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/597415e
[5374920]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5374920
[7161370]: https://github.com/HaradasCYB/groscailloux-homelab/commit/7161370
[676ae96]: https://github.com/HaradasCYB/groscailloux-homelab/commit/676ae96
[748cd53]: https://github.com/HaradasCYB/groscailloux-homelab/commit/748cd53
[4b167cb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4b167cb
[2e8ccbb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2e8ccbb
[6b6d071]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6b6d071
[3b646ad]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3b646ad
[4d5d18a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4d5d18a
[cccd582]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cccd582
[b8f239f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b8f239f
[a9bcf2a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a9bcf2a
[b060a08]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b060a08
[f9982de]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f9982de
[3eb05e9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3eb05e9
[5208a40]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5208a40
