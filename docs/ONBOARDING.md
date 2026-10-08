# Onboarding des membres — procédure admin

Un compte = un compte Jellyfin (regarder) + le même dans Jellyseerr (demander). Le membre choisit lui-même son mot de
passe par un lien à usage unique : **aucun identifiant ni mot de passe ne part jamais par mail**. Détail technique :
`homelab_core::welcome` (liens), `homelab_core::accounts` (activation), `crates/homelabd/src/web.rs` (pages).

| Réglage | Valeur | Où |
| --- | --- | --- |
| Comptes premium (actifs) au plus | 25 | `[accounts] max_premium` |
| Lectures simultanées par compte | 2 (la 3ᵉ est arrêtée) | `[accounts] max_playbacks_per_user` |
| Nouveau compte actif d'office | non : activation à la main | `[accounts] new_accounts_premium` |
| Validité du lien de bienvenue | 60 min, une seule utilisation | `[onboard] link_ttl_mins` |
| Inscriptions publiques par jour | 10 | `[onboard] max_signups_per_day` |
| Quota de demandes Jellyseerr | 10 films + 10 saisons / 7 jours | réglage Jellyseerr (`defaultQuotas`) |

## 1. Un membre s'inscrit seul (`/inscription`)

1. Il remplit pseudo + adresse sur la page publique `https://<onboarder>/inscription`.
2. Le compte est créé **suspendu** ; il reçoit « Bienvenue : définis ton mot de passe » (lien 60 min) et toi « nouveau
   compte à activer » (mail + salon Discord admin).
3. Tu ouvres **`/accounts`** (tuile Homarr « Comptes » ; identifiant du proxy « admin-outils », pas de jeton depuis la maison) → **Activer** sur sa ligne.
4. Il reçoit « Ton compte Groscailloux est actif ». S'il n'avait pas encore choisi son mot de passe, le bouton l'y mène.

## 2. Tu crées le compte toi-même

- Tuile Homarr **« Créer un compte »** (pseudo + adresse), ou `homelabctl onboard <pseudo> <adresse>`.
- Même suite qu'au-dessus ; le compte est à activer sur `/accounts` sauf si `new_accounts_premium = true`.
- `--dry-run` montre ce qui serait fait. `--password` impose un mot de passe (à éviter : le membre ne le choisit pas).

## 3. Après le mot de passe : les premiers pas

La page `/bienvenue/<jeton>` affiche, une fois le mot de passe enregistré, le parcours **Premiers pas** (appareil →
appli à installer et connexion, langue VF/VO, première demande, « Aide et annonces » et Discord, guide). Le même contenu reste
accessible à tout moment sur `https://<onboarder>/premiers-pas`, et le guide complet sur `/guide`.

## 4. Situations courantes

| Situation | Que faire |
| --- | --- |
| Lien expiré ou perdu | `/accounts` → **Renvoyer** sur sa ligne, ou `homelabctl accounts link <pseudo>`. La page expirée propose aussi d'en recevoir un nouveau (3 par heure). |
| Mot de passe oublié | Même lien (définir = changer). Connecté, il peut aussi le faire depuis **Mon compte**. |
| Mail pas reçu | Dossier spam ; sinon « Renvoyer ». Le mail part de l'adresse `SMTP_FROM`. Tester sans toucher de compte : `homelabctl mail-test <adresse de l'expéditeur>`. |
| Connexion sur une télé | Appli **Jellyfin** du store de la télé → adresse du serveur → **Quick Connect** : un code s'affiche, le membre le valide depuis son téléphone (Jellyfin → profil → Quick Connect). Pas de clavier à taper. |
| « Lire sur » vide | Normal si aucun autre appareil n'est ouvert **avec le même compte**. Chromecast : seulement Chrome sur ordinateur et l'appli Android du Play Store. |
| Lecture qui saccade | Le bandeau « Réduire la qualité » apparaît au 3ᵉ blocage ; la qualité revient en Auto à la lecture suivante. |
| VF / VO | **Mon compte → Langue de lecture**. VO = animés en japonais sur tous les appareils, langue d'origine des films et séries sur le web, Jellyfin Desktop et iPhone ; sous-titres français toujours. |
| Titre introuvable | L'onglet **Demandes** dit où en est chaque demande (recherche, téléchargement, « pas encore sorti en VOD », « introuvable : l'administrateur est prévenu »). Recherche manuelle : `/recherche`. |
| Trop d'appareils / 3ᵉ lecture coupée | Limite de 2 lectures simultanées par compte (comptes protégés exemptés). |
| Suspendre / réactiver | `/accounts` (interrupteur) ou `homelabctl accounts off|on <pseudo>`. Suspendu = connexion refusée, historique et favoris gardés. |
| Supprimer | `/accounts` → Supprimer (confirmation dans la page) ou `homelabctl accounts delete <pseudo> --yes` (Jellyfin + Jellyseerr, définitif). |

## 5. Abonnements

- Statuts : actif, offert, exempté, **à qualifier** (compte actif sans abonnement connu : jamais suspendu
  automatiquement, à trancher sur `/accounts`), suspendu.
- `homelabctl subs list | set | extend | link | import` ; paiements PayPal suivis par webhook (`AUTOMATION.md`,
  `subscription_cycle`, `subscription_reconcile`).
- Le cycle agit réellement depuis le 27/09/2026 (`[subscriptions] cycle_dry_run = false`), mais **seulement sur les essais de
  l'inscription publique et les abonnements PayPal** : toute autre fiche (actif ou offert posé par l'admin, essai posé à la
  main, import, exempté, à qualifier) est gérée à la main depuis le 08/10 — jamais suspendue, aucun rappel ; à son
  échéance, l'admin reçoit une information et `/accounts` affiche « À gérer (échéance passée) ». PayPal est en live.

## 6. Ce qu'il ne faut pas faire

- Envoyer un identifiant ou un mot de passe par mail ou par message.
- Supprimer des appareils Jellyfin en boucle (`GET /Devices?userId=` ignore le filtre : tout le monde déconnecté).
- Inventer une adresse pour un test (rebonds = réputation du domaine d'envoi) : utiliser l'adresse de l'expéditeur.
