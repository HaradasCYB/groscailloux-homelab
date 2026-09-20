# Journal des versions de Groscailloux

Groscailloux est un serveur de streaming privé pour une petite communauté : Jellyfin pour regarder, Jellyseerr
pour demander, et une chaîne d'acquisition automatique derrière. Ce journal retrace les grandes étapes, de la
première pile Docker à la version 1.0.0 qui clôt la bêta.

Les dates antérieures au dépôt git (avril-août 2026) sont **reconstituées** à partir des configurations et des
notes d'exploitation ; à partir du 10/09/2026, chaque ligne renvoie aux commits.

## En bref

| Version | Date | Thème |
| --- | --- | --- |
| **[1.17.1](#1171--20092026--sauvegarde-corrigée)** | 20/09/2026 | **Sauvegarde corrigée** |
| [1.17.0](#1170--20092026--les-nouveautés-sur-discord) | 20/09/2026 | Les nouveautés sur Discord |
| [1.16.0](#1160--20092026--bienvenue-comme-chez-les-grands) | 20/09/2026 | Bienvenue comme chez les grands |
| [1.15.1](#1151--19092026--en-français-sur-tous-les-appareils) | 19/09/2026 | En français sur tous les appareils |
| [1.15.0](#1150--19092026--une-interface-pensée-pour-la-télé) | 19/09/2026 | Une interface pensée pour la télé |
| [1.14.4](#1144--19092026--films-demandés-sous-5-minutes) | 19/09/2026 | Films demandés sous 5 minutes |
| [1.14.3](#1143--19092026--airplay-lance-vraiment-la-lecture) | 19/09/2026 | AirPlay lance vraiment la lecture |
| [1.14.2](#1142--19092026--airplay-dans-le-menu-lire-sur) | 19/09/2026 | AirPlay dans le menu « Lire sur » |
| [1.14.1](#1141--19092026--lire-sur-retrouve-vos-appareils) | 19/09/2026 | « Lire sur » retrouve vos appareils |
| [1.14.0](#1140--18092026--lecture--le-cache-tient-la-nuit) | 18/09/2026 | Lecture : le cache tient la nuit |
| [1.13.1](#1131--18092026--avance-rapide-de-10-secondes) | 18/09/2026 | Avance rapide de 10 secondes |
| [1.13.0](#1130--18092026--version-originale-privilégiée-pour-les-animés) | 18/09/2026 | Version originale privilégiée pour les animés |
| [1.12.0](#1120--18092026--les-saisons-dannimé-se-complètent-seules) | 18/09/2026 | Les saisons d'animé se complètent seules |
| [1.11.0](#1110--18092026--tout-passe-par-la-seedbox) | 18/09/2026 | Tout passe par la seedbox |
| [1.10.0](#1100--18092026--les-animés-sans-accroc) | 18/09/2026 | Les animés sans accroc |
| [1.9.0](#190--18092026--une-saison-complète-du-premier-coup) | 18/09/2026 | Une saison complète du premier coup |
| [1.8.0](#180--17092026--lindexeur-ne-tombe-plus) | 17/09/2026 | L'indexeur ne tombe plus |
| [1.7.0](#170--17092026--plus-de-confusion-avec-les-spin-offs) | 17/09/2026 | Plus de confusion avec les spin-offs |
| [1.6.0](#160--17092026--un-seul-indexeur-et-une-recherche-plus-souple) | 17/09/2026 | Un seul indexeur, et une recherche plus souple |
| [1.5.0](#150--17092026--tout-le-monde-voit-tout-et-une-vraie-recherche-manuelle) | 17/09/2026 | Tout le monde voit tout, et une vraie recherche manuelle |
| [1.4.0](#140--17092026--les-menus-anime-et-films-danimation) | 17/09/2026 | Les menus Anime et Films d'animation |
| [1.3.0](#130--17092026--recherche-par-identifiant-et-indexeurs-toujours-disponibles) | 17/09/2026 | Recherche par identifiant et indexeurs toujours disponibles |
| [1.2.0](#120--17092026--les-séries-aux-titres-traduits) | 17/09/2026 | Les séries aux titres traduits |
| [1.1.1](#111--16092026--adapté-aux-téléviseurs) | 16/09/2026 | Adapté aux téléviseurs |
| [1.1.0](#110--16092026--la-lecture-saide-elle-même) | 16/09/2026 | La lecture s'aide elle-même |
| [1.0.0](#100--15092026--fin-de-la-bêta--une-seule-plateforme-vps--seedbox) | 12 → 15/09/2026 | Fin de la bêta : une seule plateforme VPS + seedbox |
| [0.9.0](#090--10--11092026--la-refonte--homelabd) | 10 → 11/09/2026 | La refonte : l'automatisation réécrite en Rust (homelabd) |
| [0.3.0](#030--août-2026-reconstitué--jellyfin-enrichi) | août 2026 | Jellyfin enrichi par les plugins |
| [0.2.0](#020--fin-mai-2026-reconstitué--stabilisation) | fin mai 2026 | Stabilisation |
| [0.1.0](#010--30042026--06052026--les-fondations) | 30/04 → 06/05/2026 | Les fondations : demander un film depuis Jellyfin, tout arrive seul |

---

## 1.17.1 — 20/09/2026 — Sauvegarde corrigée

- La sauvegarde de nuit embarquait le cache de lecture de la seedbox (149 Go d'archive au lieu de 3) et avait rempli
  le disque du serveur à 92 %. Archive supprimée, cache exclu des sauvegardes.

## 1.17.0 — 20/09/2026 — Les nouveautés sur Discord

- **Un salon Discord pour les membres** : chaque film ou série qui arrive (avec l'affiche, un seul message par
  téléchargement), les demandes (nouvelle, validée, refusée, disponible — avec le pseudo du demandeur) et les
  annonces de l'administrateur y sont publiés. Plus besoin d'attendre un mail.
- **Un salon privé pour l'administrateur** : alertes de lecture (boucles), indexeur bloqué ou débloqué, nouveau
  compte à activer, mail de bienvenue non parti, service relancé, santé des outils d'acquisition, récapitulatif du
  tchat. Les mails d'alerte continuent en parallèle.
- Réglé par deux webhooks dans `.env` ; `homelabctl discord apply|test|remove` configure ou retire tout.

## 1.16.0 — 20/09/2026 — Bienvenue comme chez les grands

- **Un vrai mail de bienvenue, qui arrive.** Fini le mail brut avec identifiant et mot de passe en clair (que Gmail
  rangeait en spam) : un mail sobre, texte et HTML, au nom de Groscailloux, avec **un bouton**. Il mène à une page où
  le membre **choisit son mot de passe** et retrouve ses accès (regarder, demander, guide). Le lien vaut une heure et ne
  sert qu'une fois ; expiré, la page en renvoie un nouveau sur demande — ce qui sert aussi de « mot de passe oublié ».
- **Un second mail à l'activation** : quand l'administrateur active un compte en attente, le membre reçoit « ton
  compte est actif » avec un bouton.
- **Une page d'inscription** (`/inscription`) : pseudo + adresse, et le compte est créé en attente de validation ;
  l'administrateur est prévenu par mail et active depuis la page Comptes, qui affiche aussi l'état du lien de chaque
  membre et permet de le renvoyer.
- Aucun mot de passe ne circule plus par mail ni ne s'affiche dans les outils d'admin.

## 1.15.1 — 19/09/2026 — En français sur tous les appareils

- **L'interface est en français même sur un appareil réglé en anglais.** Jellyfin prenait la langue de l'appareil
  tant que le membre n'avait pas choisi la sienne dans ses réglages : un PC en anglais affichait « Home »,
  « Favorites », « Ends at 11:09 PM ». Le français est maintenant posé d'office à la première ouverture (la page se
  recharge une fois à la première connexion sur un appareil). Un membre qui a choisi lui-même une autre langue la garde.

## 1.15.0 — 19/09/2026 — Une interface pensée pour la télé

Sur un téléviseur (appli LG, Samsung… qui affichent le site), l'interface est désormais différente de celle du PC,
du téléphone et de la tablette — qui, eux, ne changent pas d'un pixel.

- **Accueil allégé** : plus de grand bandeau animé ni de bandes-annonces YouTube (ce qui pesait le plus sur une
  télé) ; les rangées se limitent à l'essentiel — Reprendre, À suivre, Films et Séries ajoutés, Anime, Collections,
  Ma médiathèque. Les rangées Tendances, Genres, Mieux notés, Découvrir, Mes demandes… restent sur les autres appareils.
- **Télécommande** : en-tête fixe avec seulement Notifications, Rechercher et Profil (les boutons Aléatoire,
  SyncPlay et « Lire sur » disparaissent sur télé), focus plus visible sur les cartes et les boutons.
- **Lecture d'abord** : sur la fiche d'un titre, « Lire » est plus grand et les blocs secondaires (Plus comme ça,
  genres, tags, studios, services de streaming, langues) sont cachés ; la distribution reste.
- **Pas de tchat sur télé** : sans clavier il était inutilisable ; il n'est plus chargé du tout.

## 1.14.4 — 19/09/2026 — Films demandés sous 5 minutes

- **Un film demandé est cherché dans les 5 minutes**, comme une série. Depuis le 17/09, Radarr ne cherche plus
  lui-même (pour ménager l'indexeur) et le flux RSS ne ramène que les nouveautés : un film ancien (*Matrix*)
  attendait le rattrapage du lendemain. La recherche par identifiant passe maintenant toutes les 5 minutes.
- **qBittorrent du VPS de nouveau joignable** : la mise à jour de nuit du 19/09 l'avait laissé attaché à l'ancien
  conteneur VPN (tuile Homarr rouge, imports côté VPS en pause pendant 11 h). Recréé, port VPN reposé, et la
  procédure documentée pour que ça ne se reproduise pas.
- **Homarr affiche enfin les téléchargements de la seedbox** : le widget ne recevait que 10 torrents par client, tous
  déjà terminés, donc masqués. Limite relevée.
- Sur la page d'état, une saison notée « sans release » disparaît de la liste une fois complétée.
- **Téléviseurs (LG, Samsung) : les boutons du haut restent accessibles.** L'en-tête (Rechercher, Tchat, Notifications,
  Profil) défilait hors de l'écran dès que l'on parcourait les rangées, et les onglets chevauchaient la cloche des
  notifications. L'en-tête est maintenant fixé en haut de l'écran sur TV et les onglets resserrés.

## 1.14.3 — 19/09/2026 — AirPlay lance vraiment la lecture

- **L'entrée AirPlay du menu « Lire sur » fait maintenant tout le travail.** Sur iPhone, iPad et Mac, un appui
  ferme le menu, lance le titre affiché et ouvre le sélecteur d'écran d'Apple dès que la vidéo est prête ; si une
  lecture est déjà en cours, le sélecteur s'ouvre directement. Le rappel « touchez l'icône AirPlay du lecteur »
  n'apparaît plus qu'en dernier recours, si l'appareil refuse d'ouvrir le sélecteur tout seul. La première version
  laissait le menu ouvert et se contentait d'afficher ce rappel.
- **Depuis l'accueil aussi.** Le bouton « Lire » du bandeau d'accueil ne fait rien dans l'appli iPhone (il envoie
  une commande à distance à sa propre session) ; l'entrée AirPlay passe désormais par la fiche du titre affiché
  dans le bandeau, puis lance la lecture. Sur une page sans titre (recherche, bibliothèque), le menu se ferme et
  une ligne explique d'ouvrir un film ou une série d'abord.

## 1.14.2 — 19/09/2026 — AirPlay dans le menu « Lire sur »

- **Sur iPhone, iPad et Mac, le menu « Lire sur » propose AirPlay.** Pendant une lecture, il ouvre directement
  le sélecteur d'écran d'Apple ; sinon il lance le titre affiché et rappelle où se trouve l'icône AirPlay dans le
  lecteur. La mention « Google Cast non pris en charge » — qui n'était qu'une note, Google Cast n'existant que
  dans Chrome et sur Android — disparaît partout où Cast n'est pas disponible.
- Quand aucun autre appareil de votre compte n'est connecté, le menu le dit clairement au lieu de rester vide.

## 1.14.1 — 19/09/2026 — « Lire sur » retrouve vos appareils

- **Le menu « Lire sur » propose de nouveau tous vos appareils connectés**, quel que soit le réseau. Depuis le
  16/09, il n'affichait que ceux partageant l'adresse de votre box ; un iPhone protégé par le Relais privé
  iCloud (ou un VPN) ne la partage pas toujours, et sa propre TV disparaissait. Les appareils des autres
  membres restent invisibles. Rappel : un appareil n'apparaît que s'il est allumé et connecté avec votre compte ;
  sur iPhone, « Google Cast non pris en charge » est normal — utilisez AirPlay depuis le lecteur.

## 1.14.0 — 18/09/2026 — Lecture : le cache tient la nuit

Audit complet de la chaîne de lecture (14 jours de mesures). 84 % des lectures sont déjà en lecture directe ;
le problème était ailleurs.

- **Ce qui a été regardé la veille reste prêt le lendemain.** Une tâche de fond (les vignettes de défilement)
  tournait six heures chaque matin sans jamais finir et rapatriait plus d'un téraoctet, ce qui vidait la
  mémoire tampon des médias distants : chaque soir, tout repartait de zéro. Elle ne tourne plus que le dimanche,
  et la mémoire tampon passe de 20 à 120 Go, avec une garde qui la fait reculer avant que le disque manque.
- **Démarrage et sauts plus vifs sur les médias distants** : blocs de lecture plus petits (premier morceau
  attendu ~0,35 s au lieu de 0,67 s).
- **Le retour arrière ne relance plus l'encodage** avant cinq minutes (deux auparavant), et le serveur garde
  trois minutes d'avance au lieu d'une et demie quand une connexion faiblit.
- **Deux encodages simultanés tiennent le temps réel** : le plafond de quatre cœurs sur six est levé.
- **Plafond à l'acquisition** : les prochaines prises restent sous ~15 Mbit/s en 1080p, sans toucher à
  l'existant ni imposer de limite par membre.
- **Alerte automatique** quand un lecteur tourne en boucle sur un flux (mail à l'admin), et mesures du cache
  distant dans Grafana.

Les changements qui coupent brièvement la lecture (redémarrage du montage distant et de quelques services)
s'appliquent automatiquement à 04:30, hors des heures de visionnage.

## 1.13.1 — 18/09/2026 — Avance rapide de 10 secondes

- **L'avance rapide saute maintenant 10 secondes au lieu de 30.** Que ce soit par le bouton du lecteur ou par
  les flèches gauche/droite du clavier, et dans les deux sens. Trente secondes faisaient systématiquement rater
  une réplique quand on revenait en arrière d'un pas de trop. Appliqué à tous les comptes, et posé d'office sur
  les nouveaux. Si l'application était déjà ouverte, il faut la recharger pour que le changement prenne.

## 1.13.0 — 18/09/2026 — Version originale privilégiée pour les animés

- **Pour les animés, la priorité change** : d'abord les versions **MULTi** (qui contiennent à la fois la piste
  japonaise et la française), puis la **VOSTFR**. Le doublage seul ne passe qu'ensuite. Les séries et les films
  ne changent pas : le français reste prioritaire. La règle s'applique partout, y compris aux récupérations
  automatiques.
- **Un type de nommage qui bloquait tout est enfin compris** : certaines publications numérotent leurs
  épisodes d'une façon que le gestionnaire ne sait pas lire, et refusait alors le lot entier — le titre restait
  « en cours » indéfiniment alors que les fichiers étaient déjà là. C'est le cas qui bloquait *Erased*,
  maintenant disponible en entier.
- **Plus rien ne se perd en silence** : les téléchargements terminés que rien n'a pu ranger apparaissent
  désormais dans la page d'état, avec la raison.

## 1.12.0 — 18/09/2026 — Les saisons d'animé se complètent seules

- **Les épisodes qui « n'existaient pas » arrivent enfin.** Beaucoup d'animés sont diffusés en plusieurs
  parties, publiées chacune sous son propre nom. Résultat : une saison restait incomplète pour toujours, sans
  que rien ne le signale — Bleach s'arrêtait à 34 épisodes sur 48. La plateforme reconnaît maintenant ces
  parties toute seule et les range au bon endroit. Bleach est passée à **48 épisodes sur 48**.
- **Sans jamais prendre de risque.** Avant d'engager quoi que ce soit, le contenu de la publication est
  vérifié : il doit combler exactement les épisodes manquants, ni plus ni moins, et aucun épisode déjà
  présent n'est remplacé. Au moindre doute, rien n'est fait et le cas reste affiché dans la page d'état.
  Cela ne s'applique qu'aux animés.

## 1.11.0 — 18/09/2026 — Tout passe par la seedbox

Test en conditions réelles d'une demande d'animé à 50 épisodes (18 minutes entre la demande et la
disponibilité), qui a mis au jour trois défauts.

- **Le serveur principal ne télécharge plus rien tout seul** : tout ce qui est neuf est récupéré par la
  seedbox, qui est faite pour ça. Le serveur principal continue de servir ce qu'il a déjà.
- **Correction d'une erreur de la version précédente** : les fiches du serveur principal avaient été mises sur
  un profil de qualité qui **préférait la version japonaise sous-titrée à la version française**, et pouvait
  refuser une version française. Toutes les fiches (24 séries, 46 films) sont revenues sur le profil français.
  Aucun fichier n'a été retéléchargé ni remplacé.
- **Les saisons que l'indexeur ne peut pas servir sont enfin visibles** : quand il manque des épisodes
  qu'aucune version ne couvre, la recherche repartait tous les jours pour rien, en silence. Ces saisons
  apparaissent maintenant dans la page d'état, avec la liste des épisodes concernés.
- **Recherche élargie** : quand la recherche par identifiant ne couvre qu'une partie d'une saison, les titres
  de la série sont aussi essayés — un animé diffusé en plusieurs parties est souvent publié sous le nom de la
  partie, jamais sous celui de la série. La page de recherche manuelle fait de même et signale les versions
  qui portent une autre numérotation de saison, sans jamais les prendre automatiquement.
- **Dossiers de saison rétablis** : les séries créées par une demande rangeaient tous leurs épisodes à plat.
- **Le format vidéo ne bloque plus une recherche** : les versions en HEVC (x265) étaient écartées au profit du
  H.264. Mesures faites sur le serveur : les deux demandent exactement le même travail, et les appareils des
  membres lisent le HEVC directement — plus de la moitié de la médiathèque est déjà dans ce format. Les
  écarter revenait à refuser la seule version française disponible, ce qui arrive souvent pour les animés.
  Seules la langue, la qualité d'image et le nombre de sources comptent désormais.
- **Redemander un titre supprimé fonctionne, et va vite** : les fichiers de partage d'un titre supprimé sont
  conservés un temps pour honorer les règles du tracker. Résultat, redemander ce titre ne donnait plus rien du
  tout. Ces fichiers sont maintenant réutilisés tels quels : le titre revient en quelques minutes, sans
  retéléchargement.
- **Une saison entière arrive en un seul passage** : la recherche s'arrêtait à 20 épisodes par tour et
  attendait un quart d'heure avant de reprendre — près d'une heure pour une saison d'animé de 50 épisodes,
  alors que la recherche elle-même ne coûte qu'une seule interrogation. Elle prend maintenant tous les
  épisodes manquants d'un coup, et reprend au bout de 5 minutes s'il en reste.

## 1.10.0 — 18/09/2026 — Les animés sans accroc

Audit complet des quatre applications de téléchargement, croisé avec le guide du tracker.

- **Les épisodes sans titre définitif s'importent enfin** : un animé qui vient de sortir n'a souvent pas encore
  de titre d'épisode, ce qui bloquait son import. C'était la cause d'une bonne partie des erreurs.
- **Numérotation japonaise comprise** : les séries d'animation sont déclarées comme telles, donc les versions
  numérotées à la japonaise (« 367 » plutôt que « saison 18, épisode 7 ») se rangent au bon endroit. 23 séries
  corrigées, sans toucher aux fichiers.
- **Les sous-titres ne sont plus perdus** : les fichiers de sous-titres livrés à côté de la vidéo (VOSTFR)
  étaient supprimés à l'import ; ils sont maintenant conservés.
- **Préférence française partout** : sur le serveur principal, presque toutes les fiches étaient restées sur un
  profil sans aucune règle de langue. Elles suivent désormais les mêmes règles que le reste (français d'abord,
  H.264, 1080p maximum).
- **Nouveautés repérées plus vite** : le flux d'annonces est relu toutes les 15 minutes, et un téléchargement
  terminé est rangé dans les 2 minutes au lieu de 10.
- **Filet de sécurité** : une corbeille est désormais active sur le serveur principal (14 jours), et la page
  d'administration liste les téléchargements terminés que personne ne réclame.

## 1.9.0 — 18/09/2026 — Une saison complète du premier coup

- **Une série demandée arrive en entier** : quand aucun pack de saison n'existe, la plateforme prend
  maintenant **tous les épisodes manquants d'un coup**, à partir de la même recherche. Avant, c'était un
  épisode toutes les deux heures — près d'une journée pour une saison. *BLACK TORCH* est passé de 1 à 11
  épisodes en quelques minutes.
- **Quatre fois plus de recherches possibles** : deux accès distincts à la source, chacun avec son propre
  compteur (80 recherches par heure au total, contre 20), et une part réservée aux recherches lancées à la
  main pour qu'elles passent toujours.
- **Plus de coupure quand un accès sature** : l'accès concerné est mis de côté quelques minutes et tout
  continue sur l'autre, sans que personne ne s'en aperçoive.
- **Les films aussi** : rattrapage toutes les 15 minutes au lieu d'une heure, trois titres par passage.

## 1.8.0 — 17/09/2026 — L'indexeur ne tombe plus

- **Fini les « aucun indexeur disponible »** : une recherche de saison lancée depuis l'application de
  téléchargement interrogeait la source une fois par épisode et se faisait couper l'accès pendant une heure —
  plus rien ne se téléchargeait pendant ce temps. Les applications ne lancent plus de recherche : elles gardent
  le flux d'annonces, et toutes les recherches passent par la plateforme, qui tient un budget horaire.
- **Deux accès à la source** : le flux d'annonces et les recherches utilisent désormais deux clés distinctes,
  donc une recherche intensive ne peut plus interrompre les nouveautés.
- **Reprise en quelques minutes** si l'accès est malgré tout coupé, au lieu d'une heure.
- **Plus de mini-séries à la place de la série** : une version dont le nom trahit une œuvre dérivée
  (mini-épisodes, spéciaux, OVA, parodie…) n'est plus choisie automatiquement. *Smoking Behind the Supermarket
  with You* avait été téléchargé en mini-épisodes de 12 minutes ; la vraie saison (24 minutes par épisode) l'a
  remplacée.

## 1.7.0 — 17/09/2026 — Plus de confusion avec les spin-offs

- **Les séries ne partent plus sur leur spin-off** : Jellyfin reconnaissait un titre d'après le nom de son
  dossier, et confondait *The Walking Dead* avec *Dead City*, *L'Attaque des Titans* avec un dérivé, ou
  *Tomb Raider* avec *Lara Croft*. La plateforme compare maintenant chaque fiche à la référence des
  applications de téléchargement et corrige toute seule, en moins de trente minutes. Six titres ont été
  remis d'aplomb au premier passage.
- **Les nouveaux dossiers portent l'identifiant du titre** : la confusion ne peut plus se produire à l'ajout.
- **Une série tout juste téléchargée s'affiche complète** : le dossier est relu en entier avant de prévenir
  Jellyfin, au lieu d'apparaître vide (c'est ce qui rendait *Game of Thrones* illisible ce soir).

## 1.6.0 — 17/09/2026 — Un seul indexeur, et une recherche plus souple

- **Le ménage dans les indexeurs** : 34 indexeurs publics inutilisés ont été retirés des quatre applications de
  téléchargement ; il n'en reste qu'un, celui qui sert vraiment. Les recherches manuelles ne dépassent plus le
  délai d'attente et le journal d'erreurs s'est vidé. Deux services devenus inutiles ont été arrêtés.
- **Un seul compteur de requêtes** au lieu de trois qui s'ignoraient, avec une réserve pour les recherches
  lancées à la main : elles passent toujours.
- **Plus de souplesse sur la langue** : quand aucune version française n'existe, la version originale est
  acceptée en dernier recours (l'ordre reste VF, MULTi, FRENCH, VOSTFR, puis VO). Des titres qui ne partaient
  jamais se téléchargent enfin.
- **Choix plus sûr** : les versions démesurées (plus de 6 Go par épisode, 25 Go par film) sont écartées du choix
  automatique, et une version à une seule source passe derrière une version bien partagée.
- **Reprise plus rapide** : quand l'indexeur est momentanément en pause, la recherche est retentée dans l'heure
  au lieu du lendemain — c'est ce qui immobilisait deux séries depuis la veille.
- **Demande supprimée = titre supprimé** : retirer une demande efface la fiche et ses fichiers, libère le
  torrent et rend le titre à nouveau demandable, au lieu de le laisser « en cours » pour toujours.

## 1.5.0 — 17/09/2026 — Tout le monde voit tout, et une vraie recherche manuelle

- **Demandes et Calendrier pour tout le monde** : chaque membre voit désormais les demandes de tous (avec leur
  auteur) et le calendrier complet des sorties à venir, comme l'administrateur. Jusqu'ici, chacun ne voyait que
  ses propres demandes, et le calendrier ignorait la moitié des séries. Les nouveaux comptes en profitent
  d'office. Personne d'autre que l'administrateur ne peut valider ou refuser une demande.
- **Recherche manuelle** (administration) : une page dédiée cherche une saison ou un film par identifiant, chez
  l'indexeur principal et chez un indexeur d'animés, et affiche toutes les releases avec leurs écarts (VOSTFR,
  qualité hors profil, autre saison…). Un clic lance le téléchargement. La recherche manuelle des animés depuis
  Sonarr ou Radarr échouait (« timed out ») et bloquait l'indexeur principal pendant une heure ; ici, une saison
  d'animé sort en quelques secondes et une seule requête est envoyée.

## 1.4.0 — 17/09/2026 — Les menus Anime et Films d'animation

Beaucoup d'animés vont arriver : ils avaient besoin de leur propre place, sans se mélanger aux autres séries.

- **Deux nouveaux menus** dans Jellyfin : **Anime** (séries) et **Films d'animation**, pour l'animation
  japonaise. Les autres dessins animés (américains, chinois, français) et les séries japonaises en prises de vues
  réelles restent dans Séries et Films.
- **Rangement automatique et sans erreur** : le classement s'appuie sur la fiche TMDB de chaque titre (genre
  Animation et origine japonaise), pas sur le type « anime » de Sonarr, qui manquait la moitié des animés. Un
  animé demandé arrive directement au bon endroit ; ce qui passe à travers est rangé dans la demi-heure, jamais
  pendant qu'on le regarde. L'admin peut forcer un titre dans un sens ou dans l'autre avec un tag.
- **27 titres déjà présents rangés** (23 séries, 4 films), sans perte de fichier : l'historique, les épisodes vus
  et les reprises sont conservés. Deux séries rattachées à la mauvaise fiche par Jellyfin après le déplacement
  ont été ré-identifiées.

## 1.3.0 — 17/09/2026 — Recherche par identifiant et indexeurs toujours disponibles

La saison 4 d'un animé demandé ne partait toujours pas. Deux causes qui s'entretenaient : Sonarr cherchait la
série sous ses noms anglais et japonais alors que l'indexeur la classe sous son nom français, et pour un animé
il envoyait des dizaines de requêtes d'un coup, jusqu'à se faire bloquer l'indexeur pour 24 h.

- **Les séries se cherchent par identifiant**, plus par leur nom : une demande part dans les dix minutes, avec
  une seule requête par saison, que la série s'appelle *Shingeki no Kyojin*, *Attack on Titan* ou *L'Attaque
  des Titans*. Les mêmes règles s'appliquent (français d'abord, 1080p maximum).
- **Films** : la recherche de Radarr fonctionnait déjà par identifiant ; un film encore manquant 24 h après sa
  demande est maintenant recherché une seconde fois de la même façon.
- **Indexeurs toujours disponibles** : un indexeur mis en pause par Sonarr ou Radarr est remis en service
  automatiquement une heure après son dernier échec (l'application concernée redémarre en une vingtaine de
  secondes), et l'admin est prévenu par mail. Quatre indexeurs bloqués ont été débloqués au premier passage.
- **Imports plus sûrs** : un import ne remplace plus jamais un épisode déjà présent, et le rattachement des
  épisodes suit celui de Sonarr. Un import de ce matin avait écrasé la saison 1 d'une série avec des
  épisodes de sa saison 4 : elle a été restaurée à partir de ses fichiers d'origine.

## 1.2.0 — 17/09/2026 — Les séries aux titres traduits

Les saisons 2 et 3 d'un animé demandé ne partaient jamais. En remontant la chaîne : la recherche de saison
dépassait le délai de la seedbox, un échec bloquait la saison trois jours, et surtout C411 — le seul indexer en
automatique — classe la série sous son titre français, que Sonarr ne cherche jamais.

- **Recherche par titre traduit** : quand Sonarr ne trouve rien, la plateforme interroge C411 directement avec
  les noms français et d'origine de la série, garde les mêmes règles (français, 1080p, sources), et confie la
  release à Sonarr. Débloque aussi d'autres séries (une série policière culte, une série de zombies).
- **Animés** : recherche épisode par épisode, au rythme que la seedbox supporte ; une erreur est retentée dans
  l'heure au lieu de trois jours.
- **Import des téléchargements manuels** : un torrent au titre japonais ou anglais retrouve la fiche existante
  par ses titres alternatifs, et l'import ne prend plus que les fichiers du torrent (il réimportait en silence
  une saison déjà présente).
- Correction ponctuelle : une série rattachée par Jellyfin à son spin-off a été ré-identifiée.

## 1.1.1 — 16/09/2026 — Adapté aux téléviseurs

Sur une TV, on n'a pas de souris : un membre s'est retrouvé avec une notification qu'il ne pouvait pas fermer.

- Les bandeaux (annonce, message de l'admin, aide à la qualité) se ferment maintenant avec la touche
  **Retour** de la télécommande et s'effacent seuls au bout de douze secondes.
- Le tchat tourne au ralenti sur ces appareils : surveillance trois fois moins fréquente, sondages espacés,
  ni ombre portée ni animation.
- Mesure faite sur un téléviseur simulé (processeur bridé six fois) : nos scripts coûtent environ deux points
  de processeur sur l'accueil et rien de mesurable pendant la lecture. La lenteur de l'application vient du
  client webOS lui-même, pas de ce qu'on y ajoute.

## 1.1.0 — 16/09/2026 — La lecture s'aide elle-même

Un membre a vu sa série saccader alors que le serveur ne peinait pas : sa connexion était descendue sous le
débit du film. Jellyfin ne sait pas s'adapter en cours de lecture — il ne choisit sa qualité qu'au démarrage —
et relance le flux en boucle quand le tampon se vide. La plateforme propose désormais la solution elle-même.

### Pour les membres

- **Bandeau d'aide dans le lecteur** : quand l'image se fige trois fois en trois minutes, un bandeau propose de
  réduire la qualité. Un clic et le film repart au même endroit, sans rechargement ; le choix est retenu pour
  cet appareil. Mesuré sur une connexion bridée à 2 Mbit/s : 4,98 → 1,56 Mbit/s, et 57 s d'image par minute au
  lieu de 15.
- **Guide complété** : une section « Ça saccade ? » explique ce que fait « Auto », pourquoi une lecture directe
  réclame environ 5 Mbit/s en continu, et comment fixer la qualité à la main. PDF régénéré.
- Aucun plafond de débit n'est imposé côté serveur : la lecture directe et la qualité maximale restent la règle.

### Pour l'administration

- **Diagnostic outillé** : la panne a été reconstituée à partir de la taille et de la cadence des segments dans
  les journaux du proxy, des journaux ffmpeg et de la croissance du cache rclone — méthode notée dans CLAUDE.md.
- **Métrique réseau corrigée** : Telegraf mesurait le trafic de son propre conteneur (~20 ko/30 s) et non celui
  du serveur ; il tourne désormais dans l'espace réseau de l'hôte et les compteurs correspondent au système.
- Banc d'essai reproductible : compte ordinaire temporaire, navigateur jetable et connexion bridée, sans aucune
  écriture sur la production.

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
