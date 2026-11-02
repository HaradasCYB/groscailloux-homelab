---
version: 1.0
name: Groscailloux TV
description: "Identité visuelle des pages servies par homelabd, des mails et de l'habillage de Jellyfin : une toile presque noire (#08090b), du texte gris clair (#e6e7ea), un seul accent chromatique, le bleu galet (#2f8fff), et un bouton principal blanc à texte noir. Rendu sobre et dense, inspiré des services de streaming et des interfaces système d'Apple : police système, titres grands et serrés, cartes anthracite à filet fin, aucune ressource externe."

colors:
  bg: "#08090b"
  surface: "#121418"
  raised: "#1f2228"
  line: "#2a2d33"
  divider: "#1d1f24"
  field-line: "#60656f"
  field-hover: "#7a7f89"
  text: "#e6e7ea"
  strong: "#ffffff"
  dim: "#a3a7af"
  accent: "#2f8fff"
  accent-hover: "#5aa6ff"
  accent-fill: "#1b6fd8"
  accent-press: "#1760c4"
  ok: "#34c77b"
  warn: "#f0b429"
  err: "#ef4444"
  err-text: "#ff8a8a"
  btn-bg: "#ffffff"
  btn-text: "#0b0b0c"
  btn-hover: "#d9dbe0"

typography:
  family-text: '-apple-system, BlinkMacSystemFont, "SF Pro Text", Inter, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif'
  family-display: '-apple-system, BlinkMacSystemFont, "SF Pro Display", Inter, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif'
  family-mono: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
  h1:
    fontFamily: display
    fontSize: clamp(1.9rem, 1.2rem + 2.6vw, 2.75rem)
    fontWeight: 800
    lineHeight: 1.12
    letterSpacing: -0.035em
  h2:
    fontFamily: display
    fontSize: clamp(1.3rem, 1.1rem + 0.9vw, 1.6rem)
    fontWeight: 700
    lineHeight: 1.12
    letterSpacing: -0.025em
  h3:
    fontFamily: display
    fontSize: 1.1rem
    fontWeight: 700
    lineHeight: 1.12
    letterSpacing: -0.015em
  lead:
    fontFamily: text
    fontSize: 1.125rem
    fontWeight: 400
    lineHeight: 1.55
  body:
    fontFamily: text
    fontSize: 16px
    fontWeight: 400
    lineHeight: 1.55
  small:
    fontFamily: text
    fontSize: 0.875rem
    fontWeight: 400
    lineHeight: 1.55
  label:
    fontFamily: text
    fontSize: 0.95rem
    fontWeight: 600
  button:
    fontFamily: text
    fontSize: 1rem
    fontWeight: 700
    lineHeight: 1.2
  table-head:
    fontFamily: text
    fontSize: 0.8rem
    fontWeight: 600
    letterSpacing: 0.04em
    textTransform: uppercase
  pill:
    fontFamily: text
    fontSize: 0.78rem
    fontWeight: 600
    lineHeight: 1.5
  brand:
    fontFamily: display
    fontSize: 1.05rem
    fontWeight: 800
    letterSpacing: -0.02em

rounded:
  sm: .3em
  md: .45em
  lg: .6em
  pill: 999px

spacing:
  xxs: 6px
  xs: 10px
  sm: 12px
  md: 16px
  lg: 24px
  xl: 32px
  xxl: 64px
  gutter: 16px
  measure: 34rem
  wide: 72rem

components:
  header:
    backgroundColor: "rgba(8, 9, 11, .86)"
    borderBottom: "1px solid {colors.divider}"
    minHeight: 56px
  card:
    backgroundColor: "{colors.surface}"
    border: "1px solid {colors.line}"
    rounded: "{rounded.lg}"
    padding: 24px
  input:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.text}"
    border: "1px solid {colors.field-line}"
    rounded: "{rounded.md}"
    padding: 11px 14px
    minHeight: 44px
  button-primary:
    backgroundColor: "{colors.btn-bg}"
    textColor: "{colors.btn-text}"
    typography: "{typography.button}"
    rounded: "{rounded.md}"
    padding: 10px 20px
    minHeight: 44px
  button-primary-hover:
    backgroundColor: "{colors.btn-hover}"
    textColor: "{colors.btn-text}"
  button-secondary:
    backgroundColor: "rgba(255, 255, 255, .08)"
    textColor: "{colors.strong}"
    border: "1px solid {colors.line}"
  button-accent:
    backgroundColor: "{colors.accent-fill}"
    textColor: "{colors.strong}"
  button-accent-hover:
    backgroundColor: "{colors.accent-press}"
  button-danger:
    backgroundColor: transparent
    textColor: "{colors.err-text}"
    border: "1px solid rgba(239, 68, 68, .55)"
  notice:
    backgroundColor: "{colors.surface}"
    borderLeft: "4px solid (gris #4a4f58, ou accent / ok / warn / err)"
    rounded: "{rounded.md}"
    padding: 12px 16px
  table-head:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.dim}"
    typography: "{typography.table-head}"
  pill:
    backgroundColor: "{colors.raised} (ou teinte à 10 % du ton)"
    textColor: "#d6d8dc (ou le ton)"
    rounded: "{rounded.pill}"
    padding: 2px 9px
---

# Groscailloux TV : identité visuelle

Référence unique de la DA. Les valeurs ci-dessus sont celles de `crates/homelabd/src/ui.rs` (`TOKENS`, `BASE_CSS`) ;
en cas d'écart, **le code a raison et ce document se corrige**.

## 1. Où vivent les jetons

| Surface | Fichier | Remarque |
| --- | --- | --- |
| Pages servies par homelabd (inscription, premium, guide, admin…) | `crates/homelabd/src/ui.rs` : `TOKENS`, `BASE_CSS`, `page()` | variables CSS `--bg`, `--accent`… ; une page n'ajoute que son `extra_css` |
| Mails | `crates/homelab-core/src/mail_layout.rs` (constantes `CANVAS`, `SURFACE`, `ACCENT`…) | styles en ligne, contraintes des clients mail (§ 6) |
| Jellyfin (web) | `branding/jellyfin/groscailloux-tv.css` (`--gc-accent`, `--darkerGradientPoint`, rayons…) | calque sur ElegantFin, appliqué par `scripts/jellyfin-branding-apply.sh` |
| Tchat et « Mon compte » (injectés dans Jellyfin) | `crates/homelabd/assets/chat/app.js`, `crates/homelabd/assets/compte/app.js` | couleurs en dur dans le script, à garder égales aux jetons |
| Logo et images | `branding/jellyfin/logo/` ; mails : `crates/homelab-core/assets/mail/` (`logo.png`, `bienvenue.jpg`) | pages : `GET /assets/logo.png` (`ui::logo`) ; mails : images jointes au message ; aucune autre image de marque |

Pages encore dans l'ancienne DA claire (`crates/homelabd/assets/premium*.html`, `don.html`, accent `#0a6fb0`) : migrées
par les lots 2 de la refonte ; ne pas les prendre pour modèle.

**Règle : changer un jeton = changer les trois (`ui.rs`, `mail_layout.rs`, `groscailloux-tv.css`) + ce document**, dans
le même commit, avec les tests de `ui.rs` (`page_carries_the_tokens`) et de `mail_layout.rs` mis à jour. Un nouveau
jeton se justifie par un rôle, pas par une page.

## 2. Vue d'ensemble

- **Noir, pas gris** : la toile `bg` est presque noire (jamais `#000`), les surfaces montent par paliers
  `bg → surface → raised`. La profondeur vient de ces paliers et d'un filet `line`, pas d'ombres.
- **Un seul accent** : le bleu galet. Il sert aux liens, au focus, aux états « info » et au bouton bleu secondaire.
  Aucun deuxième accent décoratif (pas de violet, rose ni indigo, anciennes DA abandonnées).
- **Le bouton principal est blanc**, texte presque noir, graisse 700 : l'action évidente d'une page, comme « Lire » dans
  Jellyfin. Une seule par écran.
- **Police système** : SF sur les appareils Apple, Segoe UI ou Roboto ailleurs. Aucune police externe, aucun appel tiers.
- **Sobre et lisible** : titres grands et serrés, texte de 16 px au moins, largeur de lecture limitée (`measure`).

## 3. Couleurs

### Toile et surfaces

| Jeton | Valeur | Rôle |
| --- | --- | --- |
| `bg` | `#08090b` | fond de page, `theme-color` |
| `surface` | `#121418` | cartes, notices, en-têtes de tableau |
| `raised` | `#1f2228` | champs, `code`, pastille neutre |
| `line` | `#2a2d33` | filets de cartes, `hr`, bord des boutons secondaires (décoratif : 1,4:1) |
| `divider` | `#1d1f24` | séparations de lignes de tableau, bas de l'en-tête |
| `field-line` / `field-hover` | `#60656f` / `#7a7f89` | bord des champs (3,4:1 sur `bg`, 3,15:1 sur `surface` : WCAG 1.4.11) |

### Texte

| Jeton | Valeur | Rôle | Contraste |
| --- | --- | --- | --- |
| `text` | `#e6e7ea` | texte courant | 16,1:1 sur `bg`, 12,9:1 sur `raised` |
| `strong` | `#ffffff` | titres, texte sur bleu | — |
| `dim` | `#a3a7af` | texte secondaire, aides, en-têtes de tableau | 8,3:1 sur `bg`, 6,6:1 sur `raised` |

### Accent

| Jeton | Valeur | Rôle | Contraste |
| --- | --- | --- | --- |
| `accent` | `#2f8fff` | focus des champs, barre des notices « info », coches | 6,2:1 sur `bg` (en élément, jamais en fond sous du blanc : 3,2:1) |
| `accent-hover` | `#5aa6ff` | liens, anneau de focus, pastille « info » | 7,9:1 sur `bg`, 7,3:1 sur `surface` |
| `accent-fill` | `#1b6fd8` | fond du bouton bleu, bouton des mails | blanc dessus : 4,9:1 |
| `accent-press` | `#1760c4` | survol du bouton bleu | blanc dessus : 6:1 |

### États

| Jeton | Valeur | Rôle |
| --- | --- | --- |
| `ok` | `#34c77b` | réussite, abonnement actif (8,4:1 sur `surface`) |
| `warn` | `#f0b429` | attention, échéance proche (9,9:1 sur `surface`) |
| `err` | `#ef4444` | erreur : barres, bords, `aria-invalid` (4,9:1 sur `surface`) |
| `err-text` | `#ff8a8a` | **texte** d'erreur et bouton « danger » (8,1:1 sur `surface`) |

Les états se lisent aussi sans la couleur : un mot (« Actif », « Échu », « Erreur ») accompagne toujours la teinte.

### Bouton principal

`btn-bg #ffffff`, `btn-text #0b0b0c` (19,7:1), survol `btn-hover #d9dbe0` (14,2:1).

## 4. Typographie

- **Familles** : `--font` (texte, SF Pro Text en tête) et `--font-display` (titres, SF Pro Display) ; `--mono` pour le
  code et les identifiants. Toutes système : rien à charger.
- **Hiérarchie** : `h1` 800 avec un interlettrage de −0,035 em, taille fluide de 1,9 à 2,75 rem ; `h2` 700 (1,3 à
  1,6 rem) ; `h3` 700 en 1,1 rem ; corps 16 px / 1,55 ; `.lead` 1,125 rem atténué ; `.small` 0,875 rem.
- **Principes** : titres serrés (interlettrage négatif) et corps neutre ; graisses 400, 600, 700 et 800 seulement ;
  chiffres tabulaires dans les tableaux ; jamais de texte sous 0,78 rem (taille des pastilles).
- **Pas de serif**, pas de capitales hors en-têtes de tableau, pas de texte justifié.

## 5. Mise en page, espaces et formes

- **Espacements** : 6, 10, 12, 16, 24, 32 et 64 px (`spacing`). Écart vertical par défaut entre blocs : 16 px
  (`.stack`) ; marge sous une carte : 20 px.
- **Conteneurs** : `.gc-main` jusqu'à 72 rem (pages d'administration, tableaux) ; `.gc-narrow` 34 rem (formulaires,
  pages publiques). Marge latérale `--gutter` 16 px partout.
- **Rayons** : `.6em` cartes et tableaux, `.45em` champs, boutons et notices, `.3em` code et pastille « TV » ; `999px`
  seulement pour les pastilles. Jellyfin a en plus `--smallerRadius .25em`.
- **Profondeur** : paliers de surface et filets, pas d'ombre portée ; l'en-tête seul a un flou d'arrière-plan.

## 6. Composants (classes de `BASE_CSS`)

- **En-tête** (`gc-top`, `gc-top-in`, `gc-brand`, `gc-tv`) : bande fine et collante, fond `bg` à 86 % flouté, logo galet
  30 px + « Groscailloux » + pastille « TV » en dégradé bleu. Posé par `page()` sur chaque page ; il n'est pas un lien
  (aucune page d'accueil commune). `page()` le pose toujours ; le cas de la page d'état affichée en iframe dans Homarr se
  tranche à sa migration.
- **Carte** (`.card`) : `surface`, filet `line`, rayon `lg`, 24 px de marge interne (18 px sur téléphone). Contient un
  formulaire, un résumé, une étape.
- **Champs** (`label`, `input`, `select`, `textarea`, `.field`, `.hint`) : libellé visible au-dessus (jamais le seul
  `placeholder`), fond `raised`, bord `field-line`, 44 px de haut, aide `.hint` en `dim` sous le champ ; erreur par
  `aria-invalid="true"` (bord `err`) et message texte relié par `aria-describedby`.
- **Boutons** (`.btn`, `button`) : un `button` nu est le **bouton principal blanc**. Variantes : `.secondary` (verre
  sombre, texte blanc), `.accent` (bleu `accent-fill`, pour une seconde action forte ou un lien externe comme PayPal),
  `.danger` (contour rouge, texte `err-text`, pour supprimer), `.block` (pleine largeur), `.block-sm` (pleine largeur
  sur téléphone). 44 px de haut au moins ; état désactivé à 50 % d'opacité avec `disabled` ou `aria-disabled`.
- **Notices** (`.notice` + `.info`, `.ok`, `.warn`, `.err`) : barre gauche de 4 px colorée, fond teinté à 8–9 %, texte
  `text`. Une erreur de formulaire va dans `.notice.err` avec `role="alert"` ; une confirmation dans `.notice.ok` avec
  `role="status"`.
- **Tableaux** (`.table-wrap` + `table`) : le conteneur défile horizontalement, jamais la page ; en-têtes en capitales
  `dim` sur `surface` ; lignes séparées par `divider` ; survol à peine marqué.
- **Pastilles** (`.pill` + `.ok`, `.warn`, `.err`, `.info`) : état court d'une ligne (« actif », « échu », « rouge »),
  forme de gélule, ton à 10 % en fond et 40 % en bord.
- **Utilitaires** : `.lead`, `.muted`, `.small`, `.row` (ligne qui passe à la ligne), `.stack`, `.sr-only`.

Une page n'ajoute que ses règles propres dans `extra_css` (constante du code) ; les classes vérifiées par des tests
(`del`, `sb`, `lk`, `w`…) y restent.

### Mails

Mêmes couleurs, adaptées aux clients mail (`mail_layout.rs`) : tableaux et styles en ligne, rayon de 10 px, un seul
gros bouton, **bleu** (`accent-fill`, filet `accent`, texte blanc) et non blanc : c'est le gabarit validé, protégé
contre l'inversion du mode sombre de Gmail (encre `ink_button`, classes `gc-k*`) ; `FAINT #9a9fa8` pour le pied.
Jamais d'identifiant, de mot de passe ni de `token=` dans un mail.

### Jellyfin

Le calque garde la même toile, les mêmes surfaces et le même bleu (`--gc-accent`, `--gc-accent-hover`) ; le bouton
« Lire » y est blanc à texte `#0b0b0c` en 700, comme le bouton principal des pages. Règles propres à Jellyfin :
[runbooks/jellyfin-interface.md](runbooks/jellyfin-interface.md).

## 7. Téléphone, accessibilité, mouvement

- **Téléphone d'abord** : marge latérale de 16 px, **aucun défilement horizontal de la page** (les tableaux défilent
  dans leur `.table-wrap`), cibles tactiles de 44 px, `viewport` `width=device-width,initial-scale=1` sans blocage du
  zoom. Sous 600 px : cartes à 18 px de marge interne, boutons `.block-sm` en pleine largeur.
- **Contraste WCAG AA** : texte ≥ 4,5:1, grands titres et bords de contrôles ≥ 3:1 (tableaux du § 3). Tout nouveau
  couple couleur/fond se mesure avant d'être utilisé.
- **Focus visible** : anneau de 2 px `accent-hover` décalé de 2 px sur tout élément atteint au clavier
  (`:focus-visible`), qui ne change pas la forme du contrôle ; jamais `outline: none` sans remplaçant.
- **Sémantique** : `lang="fr"`, un seul `h1`, titres dans l'ordre, `<main id="main">`, libellés reliés aux champs, logo
  décoratif en `alt=""` (le nom est écrit à côté), liens explicites (« Ouvrir Jellyfin », pas « cliquer ici »).
- **Mouvement** : transitions courtes (0,15 s) sur la couleur seulement ; `prefers-reduced-motion: reduce` les coupe
  toutes. Aucune animation d'entrée, aucun parallaxe, aucun carrousel automatique.
- **Couleurs forcées** (`forced-colors: active`) : boutons, pastilles et notices gardent un bord visible.

## 8. À faire / à éviter

### À faire

- Passer par `ui::page()` pour toute nouvelle page de homelabd, et par les classes de `BASE_CSS` avant d'écrire du CSS.
- Une seule action principale blanche par écran ; les autres en `.secondary`.
- Écrire l'état en mots en plus de la couleur (pastilles, notices).
- Échapper tout texte venu d'un membre ou d'un service (`homelab_core::html::esc`).
- Vérifier une page au téléphone (390 px) et au bureau (1440 px), au clavier, avant de la livrer.

### À éviter

- Une couleur en dur dans une page : un jeton, ou un nouveau jeton documenté ici.
- Un second accent (violet, rose, indigo, vert « marketing »), un dégradé de fond, une ombre portée décorative.
- Une police externe (Google Fonts…), un script ou une image tiers, une URL absolue vers un autre site dans le gabarit.
- Du blanc sur `accent #2f8fff` (3,2:1) : le fond bleu sous du texte est `accent-fill`.
- `--line` comme bord de champ (1,4:1) : c'est `field-line`.
- Une serif, des titres en capitales, du texte de moins de 0,78 rem.
- `window.alert/confirm/prompt` : confirmer dans la page.
- Imiter un autre service (nom, logo, palette) : Groscailloux TV garde sa propre identité.
