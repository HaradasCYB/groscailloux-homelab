//! Gabarit des mails aux membres (2026-10-15) : thème sombre « bleu galet » de Groscailloux TV, un document et
//! ses composants, repris à l'identique des prototypes validés (affiche, fond, puces, un seul gros bouton).
//!
//! Un même contenu ([`MailContent`] : objet, préheader, blocs) donne le HTML **et** une vraie version texte
//! (Gmail se méfie d'un mail sans texte ou fait d'images). Règles :
//! - tout texte passe par [`fr_typo`] puis [`crate::html::esc`] ; une URL par `esc` seulement ; un lien autre que
//!   `https://`, `http://` ou `mailto:` devient `#` ; une URL `http(s)://` écrite par un modérateur (carte de
//!   message) devient un lien, même règle que le tchat ;
//! - le `text/plain` garde le texte brut (apostrophes droites, espaces ordinaires) ;
//! - une image est soit distante ([`Img::Remote`], URL publique sans clé), soit jointe ([`Img::Inline`],
//!   `src="cid:…"`), et le rendu rassemble les images jointes une seule fois dans [`RenderedMail::images`] ;
//! - mise en page en tables, styles en ligne, trois blocs `<style>` (remises à zéro, téléphone, Gmail ; Gmail jette
//!   un bloc entier qu'il refuse, d'où des blocs séparés) ; sans aucun style, le mail tient à 390 px.
//!
//! **Mode sombre de Gmail** (iOS surtout) : il inverse même un mail déjà sombre, en réécrivant chaque couleur de
//! texte, de fond et de bordure (styles en ligne comme blocs `<style>`), mais ni les `background-image` ni les
//! images. Le troisième bloc (`STYLE_GMAIL`, sélecteurs `u + .body .gc-…`, qui ne valent que chez Gmail) porte
//! les deux moitiés de la parade, pour qu'elles tombent ensemble (compte non Google, mail transféré : on retombe
//! alors sur la version claire de Gmail, lisible) :
//! 1. **fonds** : chaque surface garde son `bgcolor` en ligne et reçoit une classe `gc-k*`/`gc-t*`/`gc-b*`
//!    (`SURFACES`) qui pose la même couleur en dégradé, que Gmail ne réécrit pas ;
//! 2. **encre** : le texte est placé dans deux enveloppes `background:#000; mix-blend-mode:difference`
//!    (`ink`, `ink_inline` ; `screen` puis `difference` pour le bouton, `ink_button`), qui défont l'inversion.
//!
//! Non prouvé par les sources : qu'un dégradé posé par un bloc `<style>` (et non en ligne) survive lui aussi. Premier
//! contrôle sur un iPhone en mode sombre : la toile reste sombre ; sinon, retirer l'encre du bouton (voir
//! `docs/runbooks/comptes-et-abonnements.md`).
//!
//! Règles de structure (vérifiées par `checks::assert_gmail_dark_safe` sur chaque mail des tests) :
//! - aucun `linear-gradient` ni `mix-blend-mode` en ligne, aucun élément `<u>` (le sélecteur en dépend) ;
//! - toute surface (fond de cellule, de puce, filet, bordure de carte) est protégée ; une bordure devient une
//!   cellule d'1 px protégée, sauf celle du bouton (bleu moyen) ;
//! - l'encre ne contient que du texte (ni image, ni table, ni fond, ni bordure) et ce texte est neutre dans Gmail
//!   (le calcul retourne la teinte : un lien bleu deviendrait orange) : `gc-n` (blanc) ou `gc-m` (gris de notice)
//!   sur un texte coloré ; un lien y prend la couleur de son paragraphe (`.gc-j a`), y compris ceux que Gmail crée
//!   lui-même dans le texte (adresse, domaine nu, `www.`) ;
//! - hors encre, seuls l'eyebrow, l'URL de secours (`#5aa6ff`), l'étoile (`#d99a1e`) et le texte de remplacement
//!   des images (`#5aa6ff` ou `#a3a7af`) : des tons moyens, lisibles sur la toile sombre comme sur sa version
//!   inversée ;
//! - les notices gardent leur fond teinté ailleurs, mais sont neutres dans Gmail (le ton passe par la bordure et
//!   la barre).
//!
//! Jamais d'identifiant, de mot de passe ni de `token=` dans un mail : le gabarit ne fait que mettre en page ce
//! qu'on lui donne, c'est à l'appelant de n'y mettre que des liens sûrs.
use std::sync::OnceLock;
use std::time::Duration;

use crate::html::esc;
use crate::mail::InlineImage;

// ---------------------------------------------------------------------------------------------
// Jetons (identité « bleu galet », jamais le rouge d'un autre service)
// ---------------------------------------------------------------------------------------------

const FONT: &str =
    "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif";
const CANVAS: &str = "#08090b";
const SURFACE: &str = "#121418";
const RAISED: &str = "#1f2228";
const LINE: &str = "#2a2d33";
const DIVIDER: &str = "#1d1f24";
const TEXT: &str = "#e6e7ea";
const STRONG: &str = "#ffffff";
const DIM: &str = "#a3a7af";
/// Pied, sous-titres d'affiche (était `#7d828c` : 2,8:1 dans la version claire de repli de Gmail).
const FAINT: &str = "#9a9fa8";
const CHIP_TEXT: &str = "#d6d8dc";
/// Texte d'une notice dans Gmail (classe `gc-m`) et des notices neutres partout.
const NOTICE_TEXT: &str = "#c9ccd2";
const ACCENT: &str = "#2f8fff";
/// Eyebrow, liens, URL de secours : ton moyen, lisible sur la toile sombre comme sur sa version inversée.
const ACCENT_TEXT: &str = "#5aa6ff";
const ACCENT_FILL: &str = "#1b6fd8";
/// Étoile de la note, hors encre : ambre moyen (était `#f0b429`, trop clair une fois inversé).
const STAR: &str = "#d99a1e";

/// Surfaces protégées dans Gmail : classe et couleur, posée par [`STYLE_GMAIL`] en dégradé d'une seule couleur
/// (les tests vérifient le bloc contre cette table). Chaque élément à fond porte la classe de sa couleur (fond
/// teinté d'une notice : `gc-k1`, neutre dans Gmail).
#[cfg(test)]
const SURFACES: [(&str, &str); 14] = [
    ("gc-k0", CANVAS),
    ("gc-k1", SURFACE),
    ("gc-k2", RAISED),
    ("gc-kl", LINE),
    ("gc-kd", DIVIDER),
    ("gc-kb", ACCENT_FILL),
    ("gc-ka", ACCENT),
    ("gc-ti", "#4a4f58"),
    ("gc-ts", "#34c77b"),
    ("gc-tw", "#f0b429"),
    ("gc-te", "#ef4444"),
    ("gc-bs", "#1f7a45"),
    ("gc-bw", "#7a5a1a"),
    ("gc-be", "#7a2a2a"),
];

/// Carte d'informations : une valeur plus longue s'empile sous son libellé, dès le rendu (sans media query).
const STACK_OVER: usize = 24;
/// Rangée d'affiches du récapitulatif : toujours 3 cases égales (les cases en trop restent vides).
pub const POSTER_SLOTS: usize = 3;
/// Budget du HTML : Gmail coupe un mail vers 102 Ko ; les prototypes font 7 à 13 Ko.
pub const HTML_BUDGET: usize = 40_000;

/// Nom affiché dans les mails (texte de remplacement du logo, signature du texte brut).
pub const BRAND: &str = "Groscailloux TV";

const LOGO_PNG: &[u8] = include_bytes!("../assets/mail/logo.png");
const WELCOME_BANNER_JPG: &[u8] = include_bytes!("../assets/mail/bienvenue.jpg");

/// Logo du haut des mails (badge 496×76 affiché 248×38, fond `#08090b` intégré), joint au mail.
pub fn logo() -> Img {
    static LOGO: OnceLock<InlineImage> = OnceLock::new();
    Img::Inline(
        LOGO.get_or_init(|| InlineImage::from_bytes("logo", LOGO_PNG).expect("logo PNG intégré"))
            .clone(),
    )
}

/// Bandeau de marque 1200×400 affiché 600×200 (bienvenue, activation, abonnement actif), joint au mail.
pub fn welcome_banner() -> Img {
    static BANNER: OnceLock<InlineImage> = OnceLock::new();
    Img::Inline(
        BANNER
            .get_or_init(|| {
                InlineImage::from_bytes("bienvenue", WELCOME_BANNER_JPG)
                    .expect("bandeau JPEG intégré")
            })
            .clone(),
    )
}

// ---------------------------------------------------------------------------------------------
// Contenu
// ---------------------------------------------------------------------------------------------

/// Image d'un composant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Img {
    /// URL publique (TMDB, Jellyfin public) : jamais de clé d'API dans l'URL.
    Remote(String),
    /// Image jointe au mail, affichée par `cid:` (voir [`fetch_image`]).
    Inline(InlineImage),
}

impl From<InlineImage> for Img {
    fn from(i: InlineImage) -> Self {
        Img::Inline(i)
    }
}

/// Ton d'une `notice` (et d'une valeur de carte d'informations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Neutre : le bleu reste réservé au bouton, aux liens et à l'eyebrow.
    Info,
    Success,
    Warning,
    Error,
}

/// Couleurs d'une `notice` (le titre reste blanc). Dans Gmail, le fond devient `SURFACE` (classe `gc-k1`) et le
/// texte `NOTICE_TEXT` (classe `gc-m`) : le ton ne passe plus que par la bordure et la barre, protégées.
struct NoticeColors {
    bg: &'static str,
    border: &'static str,
    border_k: &'static str,
    bar: &'static str,
    bar_k: &'static str,
    text: &'static str,
}

impl Tone {
    fn notice(self) -> NoticeColors {
        let (bg, border, border_k, bar, bar_k, text) = match self {
            Tone::Info => (SURFACE, LINE, "gc-kl", "#4a4f58", "gc-ti", NOTICE_TEXT),
            Tone::Success => ("#0d241a", "#1f7a45", "gc-bs", "#34c77b", "gc-ts", "#c9f7dc"),
            Tone::Warning => ("#2a2108", "#7a5a1a", "gc-bw", "#f0b429", "gc-tw", "#ffe7b3"),
            Tone::Error => ("#2a1212", "#7a2a2a", "gc-be", "#ef4444", "gc-te", "#ffd0d0"),
        };
        NoticeColors {
            bg,
            border,
            border_k,
            bar,
            bar_k,
            text,
        }
    }

    fn value_color(self) -> &'static str {
        match self {
            Tone::Info => TEXT,
            Tone::Success => "#7ee0a2",
            Tone::Warning => "#f0b429",
            Tone::Error => "#ffd0d0",
        }
    }
}

/// Sorte de puce : même style, la note porte une étoile, la langue est en gras.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipKind {
    Plain,
    Rating,
    Lang,
}

/// Puce du titre (`2022`, `Film`, `1 h 46`, `★ 7,2`, `VF · VO`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub kind: ChipKind,
    pub text: String,
    /// Forme du texte brut, si elle diffère (`note TMDB 7,2/10`).
    pub plain: Option<String>,
}

impl Chip {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            kind: ChipKind::Plain,
            text: text.into(),
            plain: None,
        }
    }
    /// Note à une décimale avec virgule (`7,2`) ; texte brut par défaut `note 7,2/10`.
    pub fn rating(text: impl Into<String>) -> Self {
        Self {
            kind: ChipKind::Rating,
            text: text.into(),
            plain: None,
        }
    }
    pub fn lang(text: impl Into<String>) -> Self {
        Self {
            kind: ChipKind::Lang,
            text: text.into(),
            plain: None,
        }
    }
    /// Forme du texte brut.
    pub fn with_plain(mut self, plain: impl Into<String>) -> Self {
        self.plain = Some(plain.into());
        self
    }

    fn html(&self) -> String {
        let weight = if self.kind == ChipKind::Lang {
            "700"
        } else {
            "600"
        };
        // l'étoile reste hors encre (ton moyen) ; sans bordure, 1 px de marge intérieure en plus garde la taille
        let mut inner = ink_inline(&t(&self.text));
        if self.kind == ChipKind::Rating {
            inner = format!(r#"<span style="color:{STAR};">&#9733;</span>&nbsp;{inner}"#);
        }
        format!(
            r#"<span class="gc-k2" style="display:inline-block;margin:0 6px 6px 0;padding:6px 11px;background-color:{RAISED};border-radius:999px;font-size:12px;line-height:16px;font-weight:{weight};color:{CHIP_TEXT};">{inner}</span>"#
        )
    }

    fn plain_text(&self) -> String {
        match (&self.plain, self.kind) {
            (Some(p), _) => p.clone(),
            (None, ChipKind::Rating) => format!("note {}/10", self.text),
            (None, _) => self.text.clone(),
        }
    }
}

/// Morceau de paragraphe : texte, ou lien en ligne.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Link { label: String, href: String },
}

/// Paragraphe d'un bloc [`Block::Text`] ; `"texte".into()` pour un paragraphe sans lien.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Para(pub Vec<Span>);

impl Para {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn text(mut self, s: impl Into<String>) -> Self {
        self.0.push(Span::Text(s.into()));
        self
    }
    pub fn link(mut self, label: impl Into<String>, href: impl Into<String>) -> Self {
        self.0.push(Span::Link {
            label: label.into(),
            href: href.into(),
        });
        self
    }

    fn html(&self) -> String {
        self.0
            .iter()
            .map(|s| match s {
                Span::Text(x) => t(x),
                // dans l'encre : couleur neutre du paragraphe dans Gmail (`.gc-j a`, `gc-n`), bleu ailleurs
                Span::Link { label, href } => format!(
                    r#"<a class="gc-n" href="{}" style="color:{ACCENT_TEXT};text-decoration:underline;">{}</a>"#,
                    link(href),
                    t(label)
                ),
            })
            .collect()
    }

    /// Texte brut : un lien devient `libellé <URL>` (les chevrons délimitent l'URL pour les lecteurs).
    fn plain_text(&self) -> String {
        self.0
            .iter()
            .map(|s| match s {
                Span::Text(x) => x.clone(),
                Span::Link { label, href } => format!("{label} <{href}>"),
            })
            .collect()
    }
}

impl From<&str> for Para {
    fn from(s: &str) -> Self {
        Para::new().text(s)
    }
}

impl From<String> for Para {
    fn from(s: String) -> Self {
        Para::new().text(s)
    }
}

/// Taille d'un bloc de texte : courant (16/25 `#e6e7ea`) ou petit (14/21 atténué).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    Body,
    Small,
}

/// Ligne d'une carte d'informations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoRow {
    pub label: String,
    pub value: String,
    /// Couleur de la valeur (succès, avertissement, erreur) ; `None` = texte courant.
    pub tone: Option<Tone>,
}

impl InfoRow {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone: None,
        }
    }
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }
}

/// Case de la rangée d'affiches (récapitulatif) : affiche, titre, sous-titre (`Film · 2009`), fiche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PosterItem {
    pub image: Img,
    pub title: String,
    pub sub: String,
    pub href: String,
}

/// Affiche + eyebrow + titre + puces + genres (mails « disponible »).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleCard {
    pub eyebrow: String,
    pub title: String,
    /// Titre original : affiché seulement selon [`show_original`].
    pub original: Option<String>,
    pub chips: Vec<Chip>,
    pub poster: Option<Img>,
    /// Lien de l'affiche (même fiche que le bouton).
    pub href: Option<String>,
    /// Genres déjà joints (`Comédie · Horreur`, 3 au plus).
    pub genres: Option<String>,
    pub top: u32,
}

impl TitleCard {
    pub fn new(eyebrow: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            eyebrow: eyebrow.into(),
            title: title.into(),
            original: None,
            chips: Vec::new(),
            poster: None,
            href: None,
            genres: None,
            top: 24,
        }
    }
    pub fn original(mut self, s: impl Into<String>) -> Self {
        self.original = Some(s.into());
        self
    }
    pub fn chip(mut self, c: Chip) -> Self {
        self.chips.push(c);
        self
    }
    pub fn poster(mut self, img: Img, href: Option<&str>) -> Self {
        self.poster = Some(img);
        self.href = href.map(str::to_string);
        self
    }
    pub fn genres(mut self, s: impl Into<String>) -> Self {
        self.genres = Some(s.into()).filter(|g: &String| !g.is_empty());
        self
    }

    /// Titre original retenu (différent du titre, titre court).
    fn shown_original(&self) -> Option<&str> {
        self.original
            .as_deref()
            .filter(|o| show_original(&self.title, o))
    }

    fn html(&self, images: &mut Vec<InlineImage>) -> String {
        let original = self.shown_original();
        let mut cells = String::new();
        if let Some(poster) = &self.poster {
            let mut img = format!(
                r#"<img class="gc-poster gc-k1" src="{}" width="158" height="237" alt="{}" style="display:block;width:100%;max-width:158px;height:auto;border:0;border-radius:8px;background-color:{SURFACE};font-family:{FONT};font-size:13px;line-height:18px;color:{DIM};">"#,
                src(poster, images),
                t(&format!("Affiche : {}", self.title))
            );
            if let Some(h) = &self.href {
                img = format!(
                    r#"<a href="{}" style="text-decoration:none;">{img}</a>"#,
                    link(h)
                );
            }
            cells.push_str(&format!(
                "<td class=\"gc-postercell\" width=\"30%\" valign=\"top\" style=\"width:30%;\">{img}</td>\n\
                 <td class=\"gc-gap\" width=\"24\" style=\"width:24px;font-size:0;line-height:0;\">&nbsp;</td>\n"
            ));
        }
        let h1_margin = if original.is_some() {
            "0 0 4px 0"
        } else {
            "0 0 12px 0"
        };
        let mut x = eyebrow(&self.eyebrow);
        x.push('\n');
        let mut head = format!(
            r#"<h1 class="gc-h1" style="margin:{h1_margin};font-size:30px;line-height:34px;font-weight:800;letter-spacing:-.01em;color:{STRONG};word-break:break-word;overflow-wrap:anywhere;">{}</h1>"#,
            t(&self.title)
        );
        if let Some(o) = original {
            head.push_str(&format!(
                r#"<p style="margin:0 0 12px 0;font-size:14px;line-height:20px;color:{DIM};">{}</p>"#,
                t(o)
            ));
        }
        x.push_str(&ink(&head));
        x.push('\n');
        // puces hors encre : chacune porte la sienne, sur son fond protégé
        if !self.chips.is_empty() {
            x.push_str(r#"<p style="margin:0;font-size:0;line-height:0;">"#);
            for c in &self.chips {
                x.push_str(&c.html());
            }
            x.push_str("</p>\n");
        }
        if let Some(g) = &self.genres {
            x.push_str(&ink(&format!(
                r#"<p style="margin:6px 0 0 0;font-size:14px;line-height:20px;color:{DIM};">{}</p>"#,
                t(g)
            )));
            x.push('\n');
        }
        cells.push_str(&format!(
            "<td valign=\"top\" style=\"font-family:{FONT};\">\n{x}</td>"
        ));
        format!(
            "<!-- gc:title_card -->\n<tr><td class=\"gc-px\" style=\"padding:{}px 32px 0 32px;\">\n\
             <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\n\
             {cells}\n</tr></table>\n</td></tr>",
            self.top
        )
    }

    /// Texte brut : `Titre (Titre original)`, puis les puces jointes par ` · `, puis les genres.
    fn plain_text(&self) -> String {
        let mut lines = vec![match self.shown_original() {
            Some(o) => format!("{} ({o})", self.title),
            None => self.title.clone(),
        }];
        if !self.chips.is_empty() {
            lines.push(
                self.chips
                    .iter()
                    .map(Chip::plain_text)
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        if let Some(g) = &self.genres {
            lines.push(g.clone());
        }
        lines.join("\n")
    }
}

/// Un composant du mail (une ou plusieurs lignes de la table de 600 px). `top` = marge au-dessus, en px ; les
/// constructeurs posent la valeur des prototypes, [`Block::top`] la change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Logo (lien vers `home` seulement si le mail a déjà un lien vers Jellyfin).
    Header {
        logo: Img,
        home: Option<String>,
    },
    /// Image de fond 16:9 pleine largeur, cliquable vers la fiche.
    Hero {
        image: Img,
        title: String,
        href: Option<String>,
    },
    /// Bandeau de marque décoratif 600×200.
    Banner {
        image: Img,
    },
    TitleCard(TitleCard),
    /// Eyebrow + h1 des mails sans visuel de titre.
    Intro {
        eyebrow: String,
        title: String,
        top: u32,
    },
    Text {
        paras: Vec<Para>,
        size: TextSize,
        top: u32,
    },
    /// Le bouton du mail (un seul par mail) ; `play` = ▶ devant un libellé « Regarder… ».
    Button {
        label: String,
        href: String,
        play: bool,
        top: u32,
    },
    /// Ligne 13/20 atténuée.
    Note {
        text: String,
        top: u32,
    },
    /// « Le bouton ne s'ouvre pas ? Copie ce lien… » : l'URL en lien et en texte.
    FallbackLink {
        url: String,
        top: u32,
    },
    InfoCard {
        rows: Vec<InfoRow>,
        top: u32,
    },
    /// Carte de lignes titre + texte (« Sur quel écran ? »).
    ListCard {
        items: Vec<(String, String)>,
        top: u32,
    },
    /// Étapes numérotées.
    Steps {
        items: Vec<(String, String)>,
        top: u32,
    },
    H2 {
        text: String,
        top: u32,
    },
    Notice {
        tone: Tone,
        title: String,
        body: String,
        top: u32,
    },
    /// Texte libre d'un modérateur (échappé ; ligne vide = paragraphe, retour simple = `<br>`) + signature.
    MessageCard {
        text: String,
        author: String,
        top: u32,
    },
    /// Rangée de 3 affiches (récapitulatif seulement : une grille ferait basculer le mail en « Promotions »).
    PosterRow {
        items: Vec<PosterItem>,
        top: u32,
    },
    Footer {
        reason: String,
        contact: String,
        credit: Option<String>,
    },
}

impl From<TitleCard> for Block {
    fn from(c: TitleCard) -> Self {
        Block::TitleCard(c)
    }
}

fn paras<P: Into<Para>>(ps: impl IntoIterator<Item = P>) -> Vec<Para> {
    ps.into_iter().map(Into::into).collect()
}

fn pairs<A: Into<String>, B: Into<String>>(
    items: impl IntoIterator<Item = (A, B)>,
) -> Vec<(String, String)> {
    items
        .into_iter()
        .map(|(a, b)| (a.into(), b.into()))
        .collect()
}

impl Block {
    /// Logo intégré ([`logo`]), lien vers `home` s'il est donné.
    pub fn header(home: Option<&str>) -> Self {
        Block::Header {
            logo: logo(),
            home: home.map(str::to_string),
        }
    }
    pub fn hero(image: Img, title: impl Into<String>, href: &str) -> Self {
        Block::Hero {
            image,
            title: title.into(),
            href: Some(href.to_string()),
        }
    }
    pub fn banner(image: Img) -> Self {
        Block::Banner { image }
    }
    pub fn intro(eyebrow: impl Into<String>, title: impl Into<String>) -> Self {
        Block::Intro {
            eyebrow: eyebrow.into(),
            title: title.into(),
            top: 12,
        }
    }
    /// Paragraphes de texte courant (`&str`, `String` ou [`Para`] avec liens).
    pub fn text<P: Into<Para>>(ps: impl IntoIterator<Item = P>) -> Self {
        Block::Text {
            paras: paras(ps),
            size: TextSize::Body,
            top: 16,
        }
    }
    /// Paragraphes en petit, atténués (synopsis, précisions).
    pub fn small<P: Into<Para>>(ps: impl IntoIterator<Item = P>) -> Self {
        Block::Text {
            paras: paras(ps),
            size: TextSize::Small,
            top: 16,
        }
    }
    pub fn button(label: impl Into<String>, href: impl Into<String>) -> Self {
        Block::Button {
            label: label.into(),
            href: href.into(),
            play: false,
            top: 20,
        }
    }
    /// Bouton « Regarder… » précédé de ▶.
    pub fn play_button(label: impl Into<String>, href: impl Into<String>) -> Self {
        Block::Button {
            label: label.into(),
            href: href.into(),
            play: true,
            top: 20,
        }
    }
    pub fn note(text: impl Into<String>) -> Self {
        Block::Note {
            text: text.into(),
            top: 12,
        }
    }
    pub fn fallback_link(url: impl Into<String>) -> Self {
        Block::FallbackLink {
            url: url.into(),
            top: 12,
        }
    }
    pub fn info_card(rows: impl IntoIterator<Item = InfoRow>) -> Self {
        Block::InfoCard {
            rows: rows.into_iter().collect(),
            top: 24,
        }
    }
    pub fn list_card<A: Into<String>, B: Into<String>>(
        items: impl IntoIterator<Item = (A, B)>,
    ) -> Self {
        Block::ListCard {
            items: pairs(items),
            top: 0,
        }
    }
    pub fn steps<A: Into<String>, B: Into<String>>(
        items: impl IntoIterator<Item = (A, B)>,
    ) -> Self {
        Block::Steps {
            items: pairs(items),
            top: 0,
        }
    }
    pub fn h2(text: impl Into<String>) -> Self {
        Block::H2 {
            text: text.into(),
            top: 32,
        }
    }
    pub fn notice(tone: Tone, title: impl Into<String>, body: impl Into<String>) -> Self {
        Block::Notice {
            tone,
            title: title.into(),
            body: body.into(),
            top: 24,
        }
    }
    pub fn message_card(text: impl Into<String>, author: impl Into<String>) -> Self {
        Block::MessageCard {
            text: text.into(),
            author: author.into(),
            top: 20,
        }
    }
    /// 3 affiches au plus ([`POSTER_SLOTS`]) : les suivantes sont ignorées (ajouter une `note` « Et N autres… »).
    pub fn poster_row(items: impl IntoIterator<Item = PosterItem>) -> Self {
        Block::PosterRow {
            items: items.into_iter().take(POSTER_SLOTS).collect(),
            top: 0,
        }
    }
    pub fn footer(
        reason: impl Into<String>,
        contact: impl Into<String>,
        credit: Option<&str>,
    ) -> Self {
        Block::Footer {
            reason: reason.into(),
            contact: contact.into(),
            credit: credit.map(str::to_string),
        }
    }

    /// Marge au-dessus du composant (sans effet sur `Header`, `Hero`, `Banner`, `Footer`, à marges fixes).
    pub fn top(mut self, px: u32) -> Self {
        match &mut self {
            Block::TitleCard(TitleCard { top, .. })
            | Block::Intro { top, .. }
            | Block::Text { top, .. }
            | Block::Button { top, .. }
            | Block::Note { top, .. }
            | Block::FallbackLink { top, .. }
            | Block::InfoCard { top, .. }
            | Block::ListCard { top, .. }
            | Block::Steps { top, .. }
            | Block::H2 { top, .. }
            | Block::Notice { top, .. }
            | Block::MessageCard { top, .. }
            | Block::PosterRow { top, .. } => *top = px,
            Block::Header { .. }
            | Block::Hero { .. }
            | Block::Banner { .. }
            | Block::Footer { .. } => {}
        }
        self
    }

    /// HTML du composant (lignes `<tr>` de la table principale) ; les images jointes vont dans `images`.
    fn html(&self, images: &mut Vec<InlineImage>) -> String {
        match self {
            Block::Header { logo, home } => {
                // texte de remplacement au ton moyen (hors encre) : lisible même inversé
                let mut img = format!(
                    r#"<img src="{}" width="248" height="38" alt="{BRAND}" style="display:block;width:248px;height:38px;border:0;font-family:Arial,Helvetica,sans-serif;font-size:20px;line-height:38px;font-weight:800;color:{ACCENT_TEXT};">"#,
                    src(logo, images)
                );
                if let Some(h) = home {
                    img = format!(r#"<a href="{}" style="text-decoration:none;">{img}</a>"#, link(h));
                }
                format!(
                    "<!-- gc:header -->\n<tr><td class=\"gc-hd\" align=\"left\" style=\"padding:24px 32px 20px 22px;\">{img}</td></tr>"
                )
            }
            Block::Hero { image, title, href } => {
                let mut img = format!(
                    r#"<img src="{}" width="600" height="338" alt="{}" style="display:block;width:100%;max-width:600px;height:auto;border:0;font-family:{FONT};font-size:24px;line-height:30px;font-weight:800;color:{ACCENT_TEXT};">"#,
                    src(image, images),
                    t(title)
                );
                if let Some(h) = href {
                    img = format!(
                        r#"<a href="{}" style="display:block;text-decoration:none;">{img}</a>"#,
                        link(h)
                    );
                }
                format!(
                    "<!-- gc:hero -->\n<tr><td class=\"gc-k1\" bgcolor=\"{SURFACE}\" style=\"background-color:{SURFACE};padding:0;\">{img}</td></tr>"
                )
            }
            Block::Banner { image } => format!(
                "<!-- gc:banner -->\n<tr><td class=\"gc-k0\" bgcolor=\"{CANVAS}\" style=\"background-color:{CANVAS};padding:0;\">\
                 <img src=\"{}\" width=\"600\" height=\"200\" alt=\"\" style=\"display:block;width:100%;max-width:600px;height:auto;border:0;\"></td></tr>",
                src(image, images)
            ),
            Block::TitleCard(c) => c.html(images),
            Block::Intro {
                eyebrow: e,
                title,
                top,
            } => format!(
                "<!-- gc:intro -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;font-family:{FONT};\">\n{}\n{}\n</td></tr>",
                eyebrow(e),
                ink(&format!(
                    "<h1 class=\"gc-h1\" style=\"margin:0;font-size:30px;line-height:34px;font-weight:800;letter-spacing:-.01em;color:{STRONG};word-break:break-word;overflow-wrap:anywhere;\">{}</h1>",
                    t(title)
                ))
            ),
            Block::Text { paras, size, top } => {
                let (fs, lh, color) = match size {
                    TextSize::Body => (16, 25, TEXT),
                    TextSize::Small => (14, 21, DIM),
                };
                let out: Vec<String> = paras
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let mb = if i + 1 == paras.len() { "0" } else { "12px" };
                        format!(
                            r#"<p style="margin:0 0 {mb} 0;font-size:{fs}px;line-height:{lh}px;color:{color};">{}</p>"#,
                            p.html()
                        )
                    })
                    .collect();
                format!(
                    "<!-- gc:text -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;font-family:{FONT};\">\n{}\n</td></tr>",
                    ink(&out.join("\n"))
                )
            }
            Block::Button {
                label,
                href,
                play,
                top,
            } => {
                let glyph = if *play {
                    "&#9654;&#65038;&nbsp;&nbsp;"
                } else {
                    ""
                };
                format!(
                    "<!-- gc:button -->\n<tr><td class=\"gc-px\" align=\"left\" style=\"padding:{top}px 32px 0 32px;\">\n\
                     <table role=\"presentation\" class=\"gc-btn\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
                     <td align=\"center\" class=\"gc-kb\" bgcolor=\"{ACCENT_FILL}\" style=\"background-color:{ACCENT_FILL};border-radius:10px;mso-padding-alt:16px 32px;\">\
                     <a href=\"{}\" style=\"display:inline-block;padding:16px 32px;border:1px solid {ACCENT};border-radius:10px;font-family:{FONT};font-size:16px;line-height:20px;font-weight:700;color:{STRONG};text-decoration:none;\">{}</a>\
                     </td></tr></table>\n</td></tr>",
                    link(href),
                    ink_button(&format!("{glyph}{}", t(label)))
                )
            }
            Block::Note { text, top } => format!(
                "<!-- gc:note -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;font-family:{FONT};\">{}</td></tr>",
                ink(&format!(
                    "<p style=\"margin:0;font-size:13px;line-height:20px;color:{DIM};\">{}</p>",
                    t(text)
                ))
            ),
            // la phrase dans l'encre, l'URL dehors (ton moyen, bleu partout)
            Block::FallbackLink { url, top } => format!(
                "<!-- gc:fallback_link -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;font-family:{FONT};\">{}\
                 <p style=\"margin:0;font-size:13px;line-height:20px;\">\
                 <a href=\"{}\" style=\"color:{ACCENT_TEXT};text-decoration:underline;word-break:break-all;\">{}</a></p></td></tr>",
                ink(&format!(
                    "<p style=\"margin:0;font-size:13px;line-height:20px;color:{DIM};\">{}</p>",
                    t("Le bouton ne s'ouvre pas ? Copie ce lien dans ton navigateur :")
                )),
                link(url),
                esc(url)
            ),
            Block::InfoCard { rows, top } => {
                let lab = format!("font-family:{FONT};font-size:13px;line-height:20px;color:{DIM};");
                let mut out = Vec::new();
                for (i, r) in rows.iter().enumerate() {
                    if i > 0 {
                        out.push(sep(2));
                    }
                    let color = r.tone.map_or(TEXT, Tone::value_color);
                    // valeur colorée : blanche dans Gmail (l'encre retournerait sa teinte)
                    let cls = if color == TEXT { "" } else { r#" class="gc-n""# };
                    let val = format!(
                        "font-family:{FONT};font-size:15px;line-height:20px;font-weight:600;color:{color};"
                    );
                    if r.value.chars().count() > STACK_OVER {
                        out.push(format!(
                            r#"<tr><td colspan="2" style="padding:10px 0 2px 0;{lab}">{}</td></tr>"#,
                            ink(&t(&r.label))
                        ));
                        out.push(format!(
                            r#"<tr><td{cls} colspan="2" style="padding:0 0 10px 0;{val}">{}</td></tr>"#,
                            ink(&t(&r.value))
                        ));
                    } else {
                        out.push(format!(
                            r#"<tr><td valign="top" style="padding:10px 12px 10px 0;{lab}">{}</td><td{cls} valign="top" align="right" style="padding:10px 0;{val}text-align:right;">{}</td></tr>"#,
                            ink(&t(&r.label)),
                            ink(&t(&r.value))
                        ));
                    }
                }
                format!("<!-- gc:info_card -->\n{}", card(&out.join("\n"), *top))
            }
            Block::ListCard { items, top } => {
                let mut out = Vec::new();
                for (i, (title, body)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(sep(1));
                    }
                    out.push(format!(
                        "<tr><td style=\"padding:14px 0;font-family:{FONT};\">{}</td></tr>",
                        ink(&format!(
                            "<p style=\"margin:0 0 4px 0;font-size:16px;line-height:22px;font-weight:700;color:{STRONG};\">{}</p>\
                             <p style=\"margin:0;font-size:15px;line-height:22px;color:{DIM};\">{}</p>",
                            t(title),
                            t(body)
                        ))
                    ));
                }
                format!("<!-- gc:list_card -->\n{}", card(&out.join("\n"), *top))
            }
            Block::Steps { items, top } => {
                let out: Vec<String> = items
                    .iter()
                    .enumerate()
                    .map(|(i, (title, body))| {
                        let n = i + 1;
                        let pb = if n == items.len() { "0" } else { "16px" };
                        // pastille pleine de 30 px, comme 28 + bordure (une bordure ne se protège pas dans Gmail)
                        format!(
                            "<tr><td width=\"42\" valign=\"top\" style=\"width:42px;padding:0 0 {pb} 0;\">\
                             <table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
                             <td class=\"gc-kl\" width=\"30\" height=\"30\" align=\"center\" valign=\"middle\" bgcolor=\"{LINE}\" style=\"width:30px;height:30px;border-radius:50%;background-color:{LINE};font-family:{FONT};font-size:14px;line-height:30px;font-weight:700;color:{STRONG};text-align:center;\">{}</td></tr></table></td>\
                             <td valign=\"top\" style=\"padding:3px 0 {pb} 0;font-family:{FONT};\">{}</td></tr>",
                            ink(&n.to_string()),
                            ink(&format!(
                                "<p style=\"margin:0 0 2px 0;font-size:16px;line-height:22px;font-weight:700;color:{STRONG};\">{}</p>\
                                 <p style=\"margin:0;font-size:15px;line-height:22px;color:{DIM};\">{}</p>",
                                t(title),
                                t(body)
                            ))
                        )
                    })
                    .collect();
                format!(
                    "<!-- gc:steps -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;\">\n\
                     <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\">\n{}\n</table>\n</td></tr>",
                    out.join("\n")
                )
            }
            Block::H2 { text, top } => format!(
                "<!-- gc:h2 -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 12px 32px;font-family:{FONT};\">{}</td></tr>",
                ink(&format!(
                    "<h2 style=\"margin:0;font-size:18px;line-height:24px;font-weight:800;color:{STRONG};\">{}</h2>",
                    t(text)
                ))
            ),
            Block::Notice {
                tone,
                title,
                body,
                top,
            } => {
                let c = tone.notice();
                let inner = ink(&format!(
                    "<p style=\"margin:0 0 4px 0;font-size:14px;line-height:20px;font-weight:700;color:{STRONG};\">{}</p>\
                     <p class=\"gc-m\" style=\"margin:0;font-size:14px;line-height:21px;color:{};\">{}</p>",
                    t(title),
                    c.text,
                    t(body)
                ));
                format!(
                    "<!-- gc:notice -->\n{}",
                    framed(
                        *top,
                        (c.border, c.border_k),
                        (c.bar, c.bar_k, 4),
                        c.bg,
                        "14px 16px",
                        &inner
                    )
                )
            }
            Block::MessageCard { text, author, top } => {
                let inner = ink(&format!(
                    "{}\n<p style=\"margin:14px 0 0 0;font-size:14px;line-height:20px;color:{DIM};\">— {}</p>",
                    message_html(text),
                    t(author)
                ));
                format!(
                    "<!-- gc:message_card -->\n{}",
                    framed(
                        *top,
                        (LINE, "gc-kl"),
                        (ACCENT, "gc-ka", 3),
                        SURFACE,
                        "18px 20px",
                        &inner
                    )
                )
            }
            Block::PosterRow { items, top } => {
                let mut cells = Vec::new();
                for i in 0..POSTER_SLOTS {
                    if i > 0 {
                        cells.push(
                            r#"<td width="3.5%" style="width:3.5%;font-size:0;line-height:0;">&nbsp;</td>"#
                                .to_string(),
                        );
                    }
                    let Some(it) = items.get(i) else {
                        cells.push(
                            r#"<td width="31%" valign="top" style="width:31%;">&nbsp;</td>"#
                                .to_string(),
                        );
                        continue;
                    };
                    let href = link(&it.href);
                    cells.push(format!(
                        "<td width=\"31%\" valign=\"top\" style=\"width:31%;font-family:{FONT};\">\
                         <a href=\"{href}\" style=\"display:block;text-decoration:none;\">\
                         <img class=\"gc-k1\" src=\"{}\" width=\"166\" height=\"249\" alt=\"{}\" style=\"display:block;width:100%;max-width:166px;height:auto;border:0;border-radius:6px;background-color:{SURFACE};font-family:{FONT};font-size:12px;line-height:16px;color:{DIM};\"></a>{}</td>",
                        src(&it.image, images),
                        t(&format!("Affiche : {}", it.title)),
                        ink(&format!(
                            "<p style=\"margin:8px 0 0 0;font-size:13px;line-height:17px;font-weight:600;color:{TEXT};\">\
                             <a href=\"{href}\" style=\"color:{TEXT};text-decoration:none;\">{}</a></p>\
                             <p style=\"margin:2px 0 0 0;font-size:12px;line-height:16px;color:{FAINT};\">{}</p>",
                            t(&it.title),
                            t(&it.sub)
                        ))
                    ));
                }
                format!(
                    "<!-- gc:poster_row -->\n<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;\">\n\
                     <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:100%;table-layout:fixed;\"><tr>\n{}\n</tr></table>\n</td></tr>",
                    cells.join("\n")
                )
            }
            Block::Footer {
                reason,
                contact,
                credit,
            } => {
                let lines: Vec<&String> = [Some(reason), Some(contact), credit.as_ref()]
                    .into_iter()
                    .flatten()
                    .collect();
                let ps: Vec<String> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let mb = if i + 1 == lines.len() { "0" } else { "8px" };
                        format!(
                            r#"<p style="margin:0 0 {mb} 0;font-size:12px;line-height:19px;color:{FAINT};">{}</p>"#,
                            t(l)
                        )
                    })
                    .collect();
                // filet = cellule d'1 px protégée, puis 24 px d'air (comme l'ancienne bordure haute d'une cellule de 24 px)
                format!(
                    "<!-- gc:footer -->\n<tr><td class=\"gc-px\" style=\"padding:36px 32px 40px 32px;font-family:{FONT};\">\n\
                     <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\">{}\
                     <tr><td height=\"24\" style=\"height:24px;font-size:0;line-height:0;\">&nbsp;</td></tr></table>\n{}\n</td></tr>",
                    sep(1),
                    ink(&ps.join("\n"))
                )
            }
        }
    }

    /// Texte brut par défaut du composant (vide pour les images et le lien de secours, déjà donné par le
    /// bouton). [`MailContent::push_with_text`] le remplace au besoin.
    pub fn plain_text(&self) -> String {
        let lines = |v: Vec<String>| v.join("\n");
        match self {
            Block::Header { .. }
            | Block::Hero { .. }
            | Block::Banner { .. }
            | Block::FallbackLink { .. } => String::new(),
            Block::TitleCard(c) => c.plain_text(),
            Block::Intro { title, .. } => title.clone(),
            Block::Text { paras, .. } => paras
                .iter()
                .map(Para::plain_text)
                .collect::<Vec<_>>()
                .join("\n\n"),
            Block::Button { label, href, .. } => format!("{label} :\n{href}"),
            Block::Note { text, .. } | Block::H2 { text, .. } => text.clone(),
            Block::InfoCard { rows, .. } => lines(
                rows.iter()
                    .map(|r| format!("{} : {}", r.label, r.value))
                    .collect(),
            ),
            Block::ListCard { items, .. } => lines(
                items
                    .iter()
                    .map(|(a, b)| format!("- {a} : {}", lower_first(b)))
                    .collect(),
            ),
            Block::Steps { items, .. } => lines(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, (a, b))| format!("{}. {a}. {b}", i + 1))
                    .collect(),
            ),
            Block::Notice { title, body, .. } => format!("{title} : {}", lower_first(body)),
            Block::MessageCard { text, author, .. } => {
                format!("{}\n\n— {author}", text.replace("\r\n", "\n").trim())
            }
            Block::PosterRow { items, .. } => lines(
                items
                    .iter()
                    .map(|it| format!("- {} ({}) : {}", it.title, it.sub, it.href))
                    .collect(),
            ),
            Block::Footer {
                reason,
                contact,
                credit,
            } => {
                let mut s = format!("— {BRAND}\n{reason}\n{contact}");
                if let Some(c) = credit {
                    s.push('\n');
                    s.push_str(c);
                }
                s
            }
        }
    }
}

/// Texte brut d'un élément.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Plain {
    Auto,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    block: Option<Block>,
    plain: Plain,
    /// Le texte brut suit le précédent après un simple retour à la ligne (sinon une ligne vide).
    glued: bool,
}

/// Un mail : objet, préheader (40 à 110 caractères, montré après l'objet dans la boîte de réception) et blocs
/// dans l'ordre. Chaque bloc donne son HTML et son texte brut ; l'ordre du texte peut différer de celui du HTML
/// avec [`MailContent::push_html_only`] + [`MailContent::push_text_only`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailContent {
    pub subject: String,
    pub preheader: String,
    items: Vec<Item>,
}

/// Mail prêt à partir ([`crate::mail::send_rendered`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedMail {
    pub subject: String,
    pub preheader: String,
    pub text: String,
    pub html: String,
    /// Images jointes (`cid:`), une fois chacune, dans l'ordre d'apparition.
    pub images: Vec<InlineImage>,
}

impl MailContent {
    pub fn new(subject: impl Into<String>, preheader: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            preheader: preheader.into(),
            items: Vec::new(),
        }
    }

    fn add(&mut self, block: Option<Block>, plain: Plain) -> &mut Self {
        self.items.push(Item {
            block,
            plain,
            glued: false,
        });
        self
    }

    /// Bloc dans le HTML et son texte brut par défaut ([`Block::plain_text`]) dans le texte.
    pub fn push(&mut self, b: impl Into<Block>) -> &mut Self {
        self.add(Some(b.into()), Plain::Auto)
    }

    /// Bloc dans le HTML, avec ce texte brut à la place du texte par défaut.
    pub fn push_with_text(&mut self, b: impl Into<Block>, text: impl Into<String>) -> &mut Self {
        self.add(Some(b.into()), Plain::Custom(text.into()))
    }

    /// Bloc dans le HTML seulement.
    pub fn push_html_only(&mut self, b: impl Into<Block>) -> &mut Self {
        self.add(Some(b.into()), Plain::Custom(String::new()))
    }

    /// Texte brut seulement (rien dans le HTML).
    pub fn push_text_only(&mut self, text: impl Into<String>) -> &mut Self {
        self.add(None, Plain::Custom(text.into()))
    }

    /// Le texte brut du dernier élément suit le précédent après un simple retour à la ligne (« Sur quel écran ? »
    /// puis la liste), au lieu d'une ligne vide.
    pub fn glue(&mut self) -> &mut Self {
        if let Some(last) = self.items.last_mut() {
            last.glued = true;
        }
        self
    }

    pub fn render(&self) -> RenderedMail {
        let mut images = Vec::new();
        let rows: Vec<String> = self
            .items
            .iter()
            .filter_map(|it| it.block.as_ref())
            .map(|b| b.html(&mut images))
            .collect();
        let html = document(&self.subject, &self.preheader, &rows.join("\n\n"));
        let mut text = String::new();
        for it in &self.items {
            let chunk = match (&it.plain, &it.block) {
                (Plain::Custom(s), _) => s.clone(),
                (Plain::Auto, Some(b)) => b.plain_text(),
                (Plain::Auto, None) => String::new(),
            };
            if chunk.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push_str(if it.glued { "\n" } else { "\n\n" });
            }
            text.push_str(chunk.trim_matches('\n'));
        }
        text.push('\n');
        RenderedMail {
            subject: self.subject.clone(),
            preheader: self.preheader.clone(),
            text,
            html,
            images,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------------------------

/// Remises à zéro (Gmail peut jeter ce bloc : `:root`, sélecteur d'attribut ; rien n'en dépend).
const STYLE_RESET: &str = "<style>
:root{color-scheme:light dark;supported-color-schemes:light dark;}
body,table,td,a{-webkit-text-size-adjust:100%;-ms-text-size-adjust:100%;}
table,td{mso-table-lspace:0pt;mso-table-rspace:0pt;}
img{-ms-interpolation-mode:bicubic;border:0;outline:none;text-decoration:none;}
a[x-apple-data-detectors]{color:inherit!important;text-decoration:none!important;font-size:inherit!important;font-family:inherit!important;font-weight:inherit!important;line-height:inherit!important;}
u + .body a{color:inherit;}
</style>";

/// Téléphone : sélecteurs de classe seulement (un seul sélecteur refusé ferait jeter tout le bloc).
const STYLE_MOBILE: &str = "<style>
@media only screen and (max-width:620px){
.gc-px{padding-left:20px!important;padding-right:20px!important;}
.gc-hd{padding:18px 20px 14px 10px!important;}
.gc-h1{font-size:24px!important;line-height:28px!important;}
.gc-card{padding:10px 14px!important;}
.gc-postercell{width:112px!important;}
.gc-poster{width:110px!important;height:auto!important;}
.gc-gap{width:16px!important;}
.gc-btn{width:100%!important;}
.gc-btn a{display:block!important;text-align:center!important;}
}
</style>";

/// Gmail seulement (`u + .body` : Gmail remplace le doctype par `<u></u>` suivi d'un `div` qui reprend la classe
/// du `<body>`) : les deux moitiés de la parade au mode sombre, dans le même bloc pour tomber ensemble (voir l'en-tête
/// du module). Fonds = dégradés d'une couleur ([`SURFACES`]) ; encre = double `difference` sur fond noir (`screen`
/// à l'extérieur pour le bouton, bleu moyen) ; `gc-n`/`gc-m` neutralisent un texte coloré pris dans l'encre ; un
/// lien dans l'encre prend la couleur de son paragraphe (`.gc-j a`, plus spécifique que `gc-n`) : les liens que Gmail
/// crée lui-même (adresse, domaine nu, `www.`) comme les nôtres, soulignés. La règle de [`STYLE_RESET`] qui le
/// faisait aussi tombe avec ce bloc-là, que Gmail jette.
const STYLE_GMAIL: &str = "<style>
u + .body .gc-k0{background-image:linear-gradient(#08090b,#08090b);}
u + .body .gc-k1{background-image:linear-gradient(#121418,#121418);}
u + .body .gc-k2{background-image:linear-gradient(#1f2228,#1f2228);}
u + .body .gc-kl{background-image:linear-gradient(#2a2d33,#2a2d33);}
u + .body .gc-kd{background-image:linear-gradient(#1d1f24,#1d1f24);}
u + .body .gc-kb{background-image:linear-gradient(#1b6fd8,#1b6fd8);}
u + .body .gc-ka{background-image:linear-gradient(#2f8fff,#2f8fff);}
u + .body .gc-ti{background-image:linear-gradient(#4a4f58,#4a4f58);}
u + .body .gc-ts{background-image:linear-gradient(#34c77b,#34c77b);}
u + .body .gc-tw{background-image:linear-gradient(#f0b429,#f0b429);}
u + .body .gc-te{background-image:linear-gradient(#ef4444,#ef4444);}
u + .body .gc-bs{background-image:linear-gradient(#1f7a45,#1f7a45);}
u + .body .gc-bw{background-image:linear-gradient(#7a5a1a,#7a5a1a);}
u + .body .gc-be{background-image:linear-gradient(#7a2a2a,#7a2a2a);}
u + .body .gc-i{background:#000;mix-blend-mode:difference;}
u + .body .gc-o{background:#000;mix-blend-mode:screen;}
u + .body .gc-j{background:#000;mix-blend-mode:difference;}
u + .body .gc-n{color:#ffffff!important;}
u + .body .gc-m{color:#c9ccd2!important;}
u + .body .gc-j a{color:inherit!important;}
</style>";

fn preheader_html(text: &str) -> String {
    format!(
        r#"<div style="display:none;max-height:0;overflow:hidden;mso-hide:all;font-size:1px;line-height:1px;color:{CANVAS};opacity:0;" aria-hidden="true">{}{}</div>"#,
        t(text),
        "&#8199;&#847;".repeat(20)
    )
}

fn document(subject: &str, preheader: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="fr" dir="ltr" xmlns="http://www.w3.org/1999/xhtml" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="X-UA-Compatible" content="IE=edge">
<meta name="x-apple-disable-message-reformatting">
<meta name="format-detection" content="telephone=no, date=no, address=no, email=no, url=no">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<title>{title}</title>
<!--[if mso]><noscript><xml><o:OfficeDocumentSettings><o:PixelsPerInch>96</o:PixelsPerInch></o:OfficeDocumentSettings></xml></noscript><![endif]-->
{STYLE_RESET}
{STYLE_MOBILE}
{STYLE_GMAIL}
</head>
<body class="body" bgcolor="{CANVAS}" style="margin:0;padding:0;width:100%;background-color:{CANVAS};">
{pre}
<div lang="fr" dir="ltr" class="gc-k0" style="background-color:{CANVAS};">
<table role="presentation" class="gc-k0" width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="{CANVAS}" style="background-color:{CANVAS};">
<tr><td align="center" style="padding:0;">
<!--[if mso]><table role="presentation" width="600" cellpadding="0" cellspacing="0" border="0" align="center"><tr><td><![endif]-->
<table role="presentation" class="gc-container gc-k0" width="100%" align="center" cellpadding="0" cellspacing="0" border="0" bgcolor="{CANVAS}" style="width:100%;max-width:600px;margin:0 auto;background-color:{CANVAS};">

{body}

</table>
<!--[if mso]></td></tr></table><![endif]-->
</td></tr>
</table>
</div>
</body>
</html>
"#,
        title = t(subject),
        pre = preheader_html(preheader),
    )
}

// ---------------------------------------------------------------------------------------------
// Morceaux communs
// ---------------------------------------------------------------------------------------------

/// Texte → HTML : typographie française puis échappement.
fn t(s: &str) -> String {
    esc(&fr_typo(s))
}

/// Lien (`href`) : échappé, jamais retouché ; un schéma autre que `https:`, `http:` ou `mailto:` devient `#`.
fn link(url: &str) -> String {
    let lower = url.trim_start().to_ascii_lowercase();
    if ["https://", "http://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        esc(url)
    } else {
        "#".into()
    }
}

/// `src` d'une image : URL distante échappée, ou `cid:` d'une image jointe (rangée une fois dans `images`).
fn src(img: &Img, images: &mut Vec<InlineImage>) -> String {
    match img {
        Img::Remote(url) => esc(url),
        Img::Inline(i) => {
            if !images.iter().any(|x| x.cid == i.cid) {
                images.push(i.clone());
            }
            esc(&format!("cid:{}", i.cid))
        }
    }
}

fn eyebrow(text: &str) -> String {
    format!(
        r#"<p style="margin:0 0 10px 0;font-size:12px;line-height:16px;font-weight:700;letter-spacing:.12em;text-transform:uppercase;color:{ACCENT_TEXT};">{}</p>"#,
        t(text)
    )
}

/// Encre (voir [`STYLE_GMAIL`]) : du texte seulement, posé directement sur une surface protégée ; jamais d'image,
/// de table, de fond ni de bordure dedans. Sans effet hors de Gmail.
fn ink(inner: &str) -> String {
    format!(r#"<div class="gc-i"><div class="gc-j">{inner}</div></div>"#)
}

/// Encre en ligne (texte d'une puce) : `inline-block` en ligne, inoffensif hors de Gmail.
fn ink_inline(inner: &str) -> String {
    format!(
        r#"<span class="gc-i" style="display:inline-block;"><span class="gc-j" style="display:inline-block;">{inner}</span></span>"#
    )
}

/// Encre du libellé du bouton : `screen` à l'extérieur (sur le bleu moyen, `difference` le rendrait orange).
fn ink_button(inner: &str) -> String {
    format!(
        r#"<span class="gc-o" style="display:inline-block;"><span class="gc-j" style="display:inline-block;">{inner}</span></span>"#
    )
}

/// Carte : la bordure est une table d'1 px de marge intérieure sur fond `LINE`, la surface une table dedans (une
/// bordure CSS deviendrait claire dans Gmail sombre ; un fond se protège).
fn card(rows_html: &str, top: u32) -> String {
    format!(
        "<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;\">\n\
         <table role=\"presentation\" class=\"gc-kl\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"{LINE}\" style=\"background-color:{LINE};border-radius:12px;border-collapse:separate;\"><tr><td style=\"padding:1px;\">\
         <table role=\"presentation\" class=\"gc-k1\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"{SURFACE}\" style=\"background-color:{SURFACE};border-radius:11px;border-collapse:separate;\">\
         <tr><td class=\"gc-card\" style=\"padding:10px 24px;\">\n\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\">\n{rows_html}\n</table>\n</td></tr></table></td></tr></table>\n</td></tr>"
    )
}

/// Encadré à barre gauche (notice, message d'un modérateur), comme `border:1px` + `border-left:<n>px` arrondis à
/// 10 px, en cellules protégées : la barre est le fond de la cellule extérieure (elle garde l'arrondi de l'encadré),
/// la bordure celui de la cellule suivante ; `bg` = fond du contenu (classe `gc-k1` : neutre dans Gmail). Les coins
/// gauches des cellules intérieures suivent la courbe extérieure (rayons elliptiques, comme le navigateur le fait
/// pour une bordure épaisse) ; un client qui les refuse garde des coins droits.
fn framed(
    top: u32,
    (border, border_k): (&str, &str),
    (bar, bar_k, bar_px): (&str, &str, u32),
    bg: &str,
    padding: &str,
    inner: &str,
) -> String {
    let rx = 10 - bar_px;
    format!(
        "<tr><td class=\"gc-px\" style=\"padding:{top}px 32px 0 32px;\">\n\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
         <td class=\"{bar_k}\" bgcolor=\"{bar}\" style=\"background-color:{bar};border-radius:10px;padding:0 0 0 {bar_px}px;\">\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
         <td class=\"{border_k}\" bgcolor=\"{border}\" style=\"background-color:{border};border-radius:0 10px 10px 0;border-top-left-radius:{rx}px 10px;border-bottom-left-radius:{rx}px 10px;padding:1px 1px 1px 0;\">\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
         <td class=\"gc-k1\" bgcolor=\"{bg}\" style=\"background-color:{bg};border-radius:0 9px 9px 0;border-top-left-radius:{rx}px 9px;border-bottom-left-radius:{rx}px 9px;padding:{padding};font-family:{FONT};\">\n{inner}\n</td>\
         </tr></table></td></tr></table></td></tr></table>\n</td></tr>"
    )
}

/// Filet : cellule d'1 px protégée (une bordure deviendrait claire dans Gmail sombre).
fn sep(colspan: usize) -> String {
    let span = if colspan > 1 {
        format!(" colspan=\"{colspan}\"")
    } else {
        String::new()
    };
    format!(
        r#"<tr><td{span} height="1" class="gc-kd" bgcolor="{DIVIDER}" style="height:1px;background-color:{DIVIDER};font-size:0;line-height:1px;">&nbsp;</td></tr>"#
    )
}

/// Morceaux d'une ligne : `(true, url)` ou `(false, texte)`. Même règle que `rich()` du tchat
/// (`homelabd/assets/chat/app.js`) : `https?://` jusqu'au premier blanc, `<`, `>` ou `"` (plus `«` et `»` ici), sans
/// la ponctuation finale `.,;:!?)]` ; un schéma seul (`https://`, `https://.`) reste du texte. Ni `www.`, ni domaine
/// nu, ni adresse mail : les messageries les relient d'elles-mêmes (dans Gmail sombre, la couleur du paragraphe :
/// `.gc-j a` de [`STYLE_GMAIL`]).
fn url_spans(line: &str) -> Vec<(bool, &str)> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"https?://[^\s<>"«»]+"#).expect("motif d'URL"));
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_iter(line) {
        let url = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
        if url
            .split_once("://")
            .is_none_or(|(_, rest)| rest.is_empty())
        {
            continue;
        }
        if m.start() > last {
            out.push((false, &line[last..m.start()]));
        }
        out.push((true, url));
        last = m.start() + url.len();
    }
    if last < line.len() {
        out.push((false, &line[last..]));
    }
    out
}

/// Une ligne d'un message : texte par [`t`], URL en lien **sans** typographie ni insécable (libellé = l'URL telle
/// quelle, échappée), coupable n'importe où (`word-break` sur le lien seul : une longue URL ne doit pas élargir la
/// carte sur un téléphone). Dans l'encre : couleur neutre du paragraphe dans Gmail (`.gc-j a`, `gc-n`), bleu ailleurs.
fn message_line(l: &str) -> String {
    url_spans(l)
        .into_iter()
        .map(|(is_url, s)| {
            if is_url {
                format!(
                    r#"<a class="gc-n" href="{}" style="color:{ACCENT_TEXT};text-decoration:underline;word-break:break-all;">{}</a>"#,
                    link(s),
                    esc(s)
                )
            } else {
                t(s)
            }
        })
        .collect()
}

/// Texte libre d'un modérateur : échappé, une ligne vide sépare deux paragraphes, un retour simple donne `<br>`,
/// une URL `http(s)://` devient un lien ([`message_line`]).
fn message_html(raw: &str) -> String {
    let raw = raw.replace("\r\n", "\n");
    let mut paras: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in raw.trim().split('\n') {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    paras
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mb = if i + 1 == paras.len() { "0" } else { "12px" };
            let body = p
                .iter()
                .enumerate()
                .map(|(j, l)| {
                    // comme `strip()` d'un paragraphe : début de la première ligne, fin de la dernière
                    let l = if j == 0 { l.trim_start() } else { l };
                    let l = if j + 1 == p.len() { l.trim_end() } else { l };
                    message_line(l)
                })
                .collect::<Vec<_>>()
                .join("<br>");
            format!(
                r#"<p style="margin:0 0 {mb} 0;font-size:16px;line-height:25px;color:{TEXT};">{body}</p>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------------------------
// Typographie et coupes (portées des prototypes, mêmes sorties sur 699 cas)
// ---------------------------------------------------------------------------------------------

const NBSP: char = '\u{a0}';
/// Une chaîne de mots collés par des espaces insécables ne dépasse pas cette longueur (en caractères) : au-delà,
/// seules les liaisons « nombre + mot » restent (« Saisons 1 à 10 disponibles » ne fait pas un seul bloc).
const GLUE_MAX: usize = 22;

/// Typographie du texte HTML (jamais d'une URL, jamais du text/plain), avant l'échappement :
/// 1. `'` entre deux lettres → `’` ;
/// 2. espace avant `: ; ! ? »` et après `«` → insécable ;
/// 3. espace nombre + mot (`9 octobre`, `3,50 €`) et mot + nombre (`saison 2`, `le 9`) → insécable, dans la
///    limite de `GLUE_MAX` par chaîne (au-delà, seules les liaisons nombre + mot restent).
pub fn fr_typo(s: &str) -> String {
    let mut cs: Vec<char> = s.chars().collect();
    let n = cs.len();
    for i in 0..n {
        let prev = if i > 0 { Some(cs[i - 1]) } else { None };
        let next = cs.get(i + 1).copied();
        match cs[i] {
            '\'' if prev.is_some_and(char::is_alphabetic)
                && next.is_some_and(char::is_alphabetic) =>
            {
                cs[i] = '\u{2019}'
            }
            ' ' if matches!(next, Some(':' | ';' | '!' | '?' | '»')) => cs[i] = NBSP,
            ' ' if prev == Some('«') => cs[i] = NBSP,
            _ => {}
        }
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Joint {
        NumberWord,
        WordNumber,
    }
    let mut kind: Vec<Option<Joint>> = vec![None; n];
    for i in 1..n.saturating_sub(1) {
        if cs[i] != ' ' {
            continue;
        }
        let (p, q) = (cs[i - 1], cs[i + 1]);
        if p.is_ascii_digit() && !q.is_ascii_digit() && q != ' ' {
            kind[i] = Some(Joint::NumberWord);
        } else if p.is_alphabetic() && q.is_ascii_digit() {
            kind[i] = Some(Joint::WordNumber);
        }
    }
    let mut i = 0;
    while i < n {
        let start = i;
        let mut joints = Vec::new();
        let mut j = i;
        while j < n {
            if cs[j] == ' ' {
                if kind[j].is_some() {
                    joints.push(j);
                    j += 1;
                    continue;
                }
                break;
            }
            j += 1;
        }
        let too_long = j - start > GLUE_MAX;
        for k in joints {
            if kind[k] == Some(Joint::NumberWord) || !too_long {
                cs[k] = NBSP;
            }
        }
        i = j + 1;
    }
    cs.into_iter().collect()
}

/// Synopsis de `n` caractères au plus : après la dernière fin de phrase (`. ! ?` suivi d'une espace) des `n`
/// premiers caractères si elle tombe à `n / 2` ou plus loin, sinon au dernier mot entier suivi de `…`.
pub fn cut(s: &str, n: usize) -> String {
    let s: Vec<char> = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect();
    if s.len() <= n {
        return s.into_iter().collect();
    }
    let head = &s[..n];
    let mut best = None;
    for (i, c) in head.iter().enumerate() {
        if matches!(c, '.' | '!' | '?') && s.get(i + 1) == Some(&' ') {
            best = Some(i);
        }
    }
    if let Some(b) = best {
        if 2 * (b + 1) >= n {
            return head[..=b].iter().collect();
        }
    }
    let end = head
        .iter()
        .rposition(|c| *c == ' ')
        .filter(|&p| p > 0)
        .unwrap_or(n);
    let kept: String = head[..end].iter().collect();
    format!(
        "{}…",
        kept.trim_end_matches(|c: char| ",;:—–- ".contains(c))
    )
}

/// Titre original affiché s'il diffère vraiment (lettres et chiffres comparés sans casse) et que le titre tient
/// sur une ligne (24 caractères au plus).
pub fn show_original(title: &str, original: &str) -> bool {
    let norm = |x: &str| -> String {
        x.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    !original.is_empty() && norm(original) != norm(title) && title.chars().count() <= 24
}

/// Première lettre en minuscule (« Rubrique… » → « rubrique… » après un deux-points).
pub fn lower_first(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_alphabetic() => c.to_lowercase().chain(it).collect(),
        _ => s.to_string(),
    }
}

/// Durée d'un film : `1 h 46`, `2 h 05`, `45 min` (rien pour 0).
pub fn runtime_fr(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, 0) => String::new(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

/// Note sur 10 à une décimale, virgule française (`7.18` → `7,2`) ; rien pour une note absente ou nulle.
pub fn rating_fr(vote: f64) -> String {
    if !vote.is_finite() || vote <= 0.0 {
        return String::new();
    }
    format!("{:.1}", vote.min(10.0)).replace('.', ",")
}

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// Jour, mois et année à l'heure locale du serveur (celle de `date_text` des pages) ; `None` si l'horodatage
/// est hors limites.
fn local_date(t: i64) -> Option<(String, &'static str, i32)> {
    use chrono::Datelike;
    let d = chrono::DateTime::from_timestamp(t, 0)?.with_timezone(&chrono::Local);
    let day = match d.day() {
        1 => "1er".to_string(),
        n => n.to_string(),
    };
    Some((day, MONTHS[d.month0() as usize], d.year()))
}

/// Date en toutes lettres pour un mail : `22 octobre 2026`, `1er novembre 2026` (« ? » si invalide).
pub fn date_long_fr(t: i64) -> String {
    local_date(t)
        .map(|(d, m, y)| format!("{d} {m} {y}"))
        .unwrap_or_else(|| "?".into())
}

/// Jour et mois seulement, pour une phrase ou un objet : `22 octobre`, `1er novembre` (« ? » si invalide).
pub fn day_month_fr(t: i64) -> String {
    local_date(t)
        .map(|(d, m, _)| format!("{d} {m}"))
        .unwrap_or_else(|| "?".into())
}

// ---------------------------------------------------------------------------------------------
// Images distantes
// ---------------------------------------------------------------------------------------------

/// Poids maximal d'une image téléchargée pour être jointe (une affiche TMDB w342 fait 25 à 80 Ko).
pub const IMAGE_MAX_BYTES: usize = 400 * 1024;
/// Délai total d'un téléchargement d'image (connexion + corps).
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(10);

/// Télécharge une image (TMDB, Jellyfin avec `format=Jpg`) pour la joindre au mail : JPEG ou PNG seulement,
/// [`IMAGE_MAX_BYTES`] au plus, [`IMAGE_TIMEOUT`] au plus. `None` au moindre souci : le mail part alors sans
/// cette image, il n'échoue jamais pour elle. L'URL n'est jamais journalisée.
pub async fn fetch_image(http: &reqwest::Client, url: &str, name: &str) -> Option<InlineImage> {
    fetch_image_limited(http, url, name, IMAGE_MAX_BYTES, IMAGE_TIMEOUT).await
}

/// [`fetch_image`] avec ses limites.
pub async fn fetch_image_limited(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Option<InlineImage> {
    let r = tokio::time::timeout(timeout, try_fetch(http, url, name, max_bytes, timeout))
        .await
        .unwrap_or_else(|_| Err("délai dépassé".into()));
    match r {
        Ok(img) => Some(img),
        Err(why) => {
            tracing::debug!(image = name, reason = %why, "image de mail ignorée");
            None
        }
    }
}

async fn try_fetch(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<InlineImage, String> {
    let url = reqwest::Url::parse(url).map_err(|_| "URL invalide".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("schéma refusé".into());
    }
    let mut resp = http
        .get(url)
        // pas de WebP (Jellyfin en sert dès que l'en-tête le permet ; Outlook ne l'affiche pas)
        .header(reqwest::header::ACCEPT, "image/jpeg, image/png")
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("{:#}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let declared = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(declared.as_str(), "image/jpeg" | "image/png") {
        return Err(format!("type {declared:?} refusé"));
    }
    if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
        return Err("image trop lourde".into());
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("{:#}", e.without_url()))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err("image trop lourde".into());
        }
        buf.extend_from_slice(&chunk);
    }
    let img = InlineImage::from_bytes(name, buf).ok_or("contenu ni JPEG ni PNG")?;
    if img.content_type != declared {
        return Err("type annoncé différent du contenu".into());
    }
    Ok(img)
}

/// Contrôles communs à tous les mails aux membres, pour les tests des modules qui les composent (bienvenue,
/// abonnement, tchat, « disponible »).
#[cfg(test)]
pub(crate) mod checks {
    use super::*;

    /// Texte visible approximatif du HTML (en-tête, commentaires, préheader et balises retirés).
    pub(crate) fn visible(html: &str) -> String {
        let body = &html[html.find("</head>").unwrap()..];
        let mut out = String::new();
        let mut in_tag = false;
        for c in body.chars() {
            match c {
                '<' => in_tag = true,
                '>' => {
                    in_tag = false;
                    out.push(' ');
                }
                c if !in_tag => out.push(c),
                _ => {}
            }
        }
        out
    }

    /// Un mail membre bien formé : document complet sous le budget, vraie version texte, aucun reste de gabarit,
    /// ni jeton en paramètre, ni clé, ni identifiant ; chaque lien est web ou mail (aucun lien neutralisé en `#`) ;
    /// chaque image jointe est appelée par son `cid` et inversement ; typographie française dans le HTML seulement.
    pub(crate) fn assert_member_mail(r: &RenderedMail) {
        let h = &r.html;
        assert!(!r.subject.trim().is_empty() && !r.subject.contains('\n'));
        assert!(h.starts_with("<!DOCTYPE html>\n<html lang=\"fr\""));
        assert!(h.len() < HTML_BUDGET, "{} octets", h.len());
        assert!(r.text.len() > 120 && r.text.ends_with('\n'), "{}", r.text);
        assert!(!r.text.contains('\u{a0}') && !r.text.contains('\u{2019}'));
        for t in [h.as_str(), r.text.as_str(), r.subject.as_str()] {
            for bad in [
                "{{",
                "token=",
                "api_key",
                "apikey",
                "Password",
                "Username :",
                "Identifiant :",
                "duckdns",
            ] {
                assert!(!t.contains(bad), "{bad} dans {t}");
            }
            assert!(!t.to_lowercase().contains("e50914"));
        }
        assert!(!h.contains("href=\"#\""), "lien neutralisé");
        for (i, _) in h.match_indices("href=\"") {
            let url = &h[i + 6..];
            assert!(
                ["https://", "http://", "mailto:"]
                    .iter()
                    .any(|p| url.starts_with(p)),
                "{}",
                &url[..url.find('"').unwrap()]
            );
        }
        let cids: Vec<&str> = h
            .match_indices("src=\"cid:")
            .map(|(i, _)| {
                let rest = &h[i + 9..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        for img in &r.images {
            assert!(
                cids.contains(&img.cid.as_str()),
                "{} jamais affichée",
                img.cid
            );
        }
        for cid in cids {
            assert!(r.images.iter().any(|i| i.cid == cid), "{cid} non jointe");
        }
        let cs: Vec<char> = visible(h).chars().collect();
        assert!(!cs
            .windows(3)
            .any(|w| w[1] == '\'' && w[0].is_alphabetic() && w[2].is_alphabetic()));
        assert_gmail_dark_safe(h);
    }

    /// Élément ouvert : nom (minuscules) et attributs bruts.
    #[derive(Clone, Debug)]
    pub(crate) struct El {
        pub name: String,
        pub attrs: String,
    }

    pub(crate) enum Node<'a> {
        Elem(&'a El),
        Text(&'a str),
    }

    const VOID: [&str; 6] = ["img", "br", "meta", "link", "hr", "input"];

    /// Parcourt `<body>…</body>` (commentaires et commentaires conditionnels d'Outlook sautés) : `f` reçoit la pile
    /// des ancêtres et chaque élément ouvert ou texte. Vérifie au passage que chaque fermeture répond à son ouverture.
    pub(crate) fn walk(html: &str, mut f: impl FnMut(&[El], Node<'_>)) {
        let start = html.find("<body").expect("<body>");
        let end = html.rfind("</body>").expect("</body>") + "</body>".len();
        let mut rest = &html[start..end];
        let mut stack: Vec<El> = Vec::new();
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("<!--") {
                rest = &r[r.find("-->").expect("commentaire fermé") + 3..];
            } else if let Some(r) = rest.strip_prefix("</") {
                let e = r.find('>').expect("balise fermée");
                let name = r[..e].trim().to_ascii_lowercase();
                let top = stack
                    .pop()
                    .unwrap_or_else(|| panic!("</{name}> sans ouverture"));
                assert_eq!(top.name, name, "</{name}> ferme <{}>", top.name);
                rest = &r[e + 1..];
            } else if rest.starts_with('<') {
                let e = rest.find('>').expect("balise fermée");
                let inner = &rest[1..e];
                let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
                let el = El {
                    name: name.to_ascii_lowercase(),
                    attrs: attrs.to_string(),
                };
                f(&stack, Node::Elem(&el));
                if !VOID.contains(&el.name.as_str()) {
                    stack.push(el);
                }
                rest = &rest[e + 1..];
            } else {
                let e = rest.find('<').unwrap_or(rest.len());
                f(&stack, Node::Text(&rest[..e]));
                rest = &rest[e..];
            }
        }
        assert!(stack.is_empty(), "non fermés : {stack:?}");
    }

    impl El {
        pub(crate) fn attr(&self, name: &str) -> Option<&str> {
            let pat = format!("{name}=\"");
            let mut from = 0;
            while let Some(i) = self.attrs[from..].find(&pat) {
                let i = from + i;
                if i == 0 || self.attrs.as_bytes()[i - 1].is_ascii_whitespace() {
                    let v = &self.attrs[i + pat.len()..];
                    return Some(&v[..v.find('"').expect("attribut fermé")]);
                }
                from = i + 1;
            }
            None
        }
        pub(crate) fn has_class(&self, c: &str) -> bool {
            self.attr("class")
                .is_some_and(|v| v.split_whitespace().any(|x| x == c))
        }
        /// Déclarations du style en ligne, dans l'ordre.
        pub(crate) fn styles(&self) -> Vec<(String, String)> {
            self.attr("style")
                .unwrap_or("")
                .split(';')
                .filter_map(|d| d.split_once(':'))
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()))
                .collect()
        }
        pub(crate) fn style(&self, prop: &str) -> Option<String> {
            self.styles()
                .into_iter()
                .rev()
                .find(|(k, _)| k == prop)
                .map(|(_, v)| v)
        }
        /// Fond déclaré (`bgcolor` ou `background-color`).
        fn background(&self) -> Option<String> {
            self.style("background-color")
                .or_else(|| self.attr("bgcolor").map(str::to_ascii_lowercase))
        }
        fn is_ink(&self) -> bool {
            self.has_class("gc-i") || self.has_class("gc-o")
        }
    }

    /// Gris neutre : canaux à 16 unités au plus les uns des autres (le calcul de Gmail garde un gris gris).
    fn neutral(color: &str) -> bool {
        let c = color.trim_start_matches('#');
        assert_eq!(c.len(), 6, "couleur {color}");
        let ch: Vec<u8> = (0..3)
            .map(|i| u8::from_str_radix(&c[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        ch.iter().max().unwrap() - ch.iter().min().unwrap() <= 0x10
    }

    /// Fonds teintés des notices : neutres dans Gmail (classe `gc-k1`).
    fn notice_tints() -> Vec<&'static str> {
        [Tone::Info, Tone::Success, Tone::Warning, Tone::Error]
            .into_iter()
            .map(|t| t.notice().bg)
            .filter(|bg| *bg != SURFACE)
            .collect()
    }

    /// Le mail reste sombre et lisible dans le mode sombre de Gmail, et lisible quand une moitié de la parade (ou
    /// tout le bloc Gmail) saute ; règles de l'en-tête du module :
    /// 1. dégradés et mélanges uniquement dans [`STYLE_GMAIL`], aucun `<u>`, `<body class="body">` ;
    /// 2. tout fond hors encre a la classe protégée de sa couleur, et toute classe protégée a son fond en ligne ;
    /// 3. aucune bordure colorée hors encre, sauf celle du bouton ;
    /// 4. l'encre (`gc-i`/`gc-o` puis `gc-j`) ne contient que du texte, jamais une autre encre, et repose sur un
    ///    fond protégé ;
    /// 5. texte hors encre : bleu moyen ou ambre seulement (texte de remplacement d'une image : bleu ou gris
    ///    moyen) ; texte coloré dans l'encre : neutralisé pour Gmail (`gc-n`, `gc-m`) ;
    /// 6. le libellé du bouton est blanc, dans l'encre du bouton, sur le bleu protégé.
    pub(crate) fn assert_gmail_dark_safe(h: &str) {
        let blocks: Vec<&str> = h.match_indices("<style>").map(|(i, _)| &h[i..]).collect();
        assert_eq!(
            blocks
                .iter()
                .filter(|b| b[..b.find("</style>").unwrap()].contains("mix-blend-mode"))
                .count(),
            1
        );
        assert_eq!(h.matches(STYLE_GMAIL).count(), 1, "bloc Gmail");
        for needle in ["linear-gradient", "mix-blend-mode", "background-image"] {
            assert_eq!(
                h.matches(needle).count(),
                STYLE_GMAIL.matches(needle).count(),
                "{needle} hors du bloc Gmail"
            );
        }
        assert!(!h.contains("<u>") && !h.contains("<u "), "élément <u>");
        assert!(h.contains("<body class=\"body\""));
        let tints = notice_tints();
        let mut inks = 0;
        walk(h, |stack, node| {
            let in_ink = stack.iter().any(El::is_ink);
            let parent = stack.last();
            match node {
                Node::Elem(el) => {
                    let what = format!("<{} {}>", el.name, el.attrs);
                    let surfaces: Vec<(&str, &str)> = SURFACES
                        .iter()
                        .copied()
                        .filter(|(k, _)| el.has_class(k))
                        .collect();
                    assert!(surfaces.len() <= 1, "deux surfaces : {what}");
                    if parent.is_some_and(El::is_ink) {
                        assert!(
                            el.has_class("gc-j") && !el.is_ink(),
                            "encre sans gc-j : {what}"
                        );
                    }
                    if el.has_class("gc-j") {
                        assert!(parent.is_some_and(El::is_ink), "gc-j seul : {what}");
                    }
                    if in_ink {
                        assert!(!el.is_ink(), "encre dans l'encre : {what}");
                        assert!(
                            !["img", "table", "td", "tr"].contains(&el.name.as_str()),
                            "{what} dans l'encre"
                        );
                        assert!(
                            el.attr("bgcolor").is_none() && surfaces.is_empty(),
                            "fond dans l'encre : {what}"
                        );
                        for (k, _) in el.styles() {
                            assert!(
                                !k.starts_with("background") && !k.starts_with("border"),
                                "{k} dans l'encre : {what}"
                            );
                        }
                    } else if el.name != "body" {
                        match (el.background(), surfaces.first()) {
                            (Some(bg), Some((k, c))) => assert!(
                                bg == *c || (*k == "gc-k1" && tints.contains(&bg.as_str())),
                                "{k} ({c}) sur un fond {bg} : {what}"
                            ),
                            (Some(bg), None) => panic!("fond {bg} non protégé : {what}"),
                            (None, Some((k, _))) => panic!("{k} sans fond en ligne : {what}"),
                            (None, None) => {}
                        }
                        for (k, v) in el.styles() {
                            if !k.starts_with("border")
                                || k.ends_with("-radius")
                                || k == "border-collapse"
                                || v == "0"
                            {
                                continue;
                            }
                            let button = el.name == "a"
                                && k == "border"
                                && v == format!("1px solid {ACCENT}")
                                && parent.is_some_and(|p| p.has_class("gc-kb"));
                            assert!(button, "bordure {k}:{v} hors encre : {what}");
                        }
                    }
                    if el.is_ink() {
                        inks += 1;
                        let ground = stack
                            .iter()
                            .rev()
                            .find(|e| e.background().is_some())
                            .expect("encre sans fond");
                        assert!(ground.name != "body", "encre posée sur <body>");
                    }
                    if el.has_class("gc-o") {
                        let a = parent.expect("bouton");
                        assert!(
                            a.name == "a" && a.style("color").as_deref() == Some(STRONG),
                            "libellé du bouton : {what}"
                        );
                        assert!(stack
                            .iter()
                            .rev()
                            .nth(1)
                            .is_some_and(|td| td.has_class("gc-kb")));
                    }
                    if el.name == "img" && el.attr("alt").is_some_and(|a| !a.is_empty()) {
                        let c = el.style("color").unwrap_or_default();
                        assert!(
                            [ACCENT_TEXT, DIM].contains(&c.as_str()),
                            "texte de remplacement {c} : {what}"
                        );
                    }
                }
                Node::Text(t) => {
                    let mut v = t.to_string();
                    for e in ["&nbsp;", "&#8199;", "&#847;"] {
                        v = v.replace(e, "");
                    }
                    let v = v.trim();
                    if v.is_empty() || stack.iter().any(|e| e.attr("aria-hidden") == Some("true")) {
                        return;
                    }
                    assert!(!parent.is_some_and(El::is_ink), "texte hors gc-j : {v}");
                    let (decl, color) = stack
                        .iter()
                        .rev()
                        .find_map(|e| e.style("color").map(|c| (e, c)))
                        .unwrap_or_else(|| panic!("texte sans couleur : {v}"));
                    if in_ink {
                        assert!(
                            neutral(&color) || decl.has_class("gc-n") || decl.has_class("gc-m"),
                            "texte {color} dans l'encre, non neutralisé pour Gmail : {v}"
                        );
                    } else {
                        assert!(
                            [ACCENT_TEXT, STAR].contains(&color.as_str()),
                            "texte {color} hors encre : {v}"
                        );
                    }
                }
            }
        });
        assert!(inks > 0, "aucune encre");
    }
}

#[cfg(test)]
mod tests {
    use super::checks::visible;
    use super::*;

    const JF: &str = "https://tv.example.test";

    fn details(id: &str) -> String {
        format!("{JF}/web/#/details?id={id}")
    }

    /// Mail « disponible » d'un film, comme le prototype phare (`media_movie`).
    fn media_movie() -> RenderedMail {
        let url = details("cadee972da3998273e8cc31e49fbef7e");
        let lead =
            "Salut Nina, le film que tu as demandé le 9 octobre vient d'arriver. Bonne séance !";
        let mut m = MailContent::new(
            "Le Menu est disponible",
            "Ta demande du 9 octobre est arrivée : lance-le quand tu veux, sur la télé, le téléphone ou l'ordinateur.",
        );
        m.push(Block::header(Some(JF)))
            .push(Block::hero(
                Img::Remote("https://image.example.test/w780/fond.jpg".into()),
                "Le Menu",
                &url,
            ))
            .push_text_only(lead)
            .push(
                TitleCard::new("Ta demande est disponible", "Le Menu")
                    .original("The Menu")
                    .chip(Chip::plain("2022"))
                    .chip(Chip::plain("Film"))
                    .chip(Chip::plain(runtime_fr(106)))
                    .chip(Chip::rating(rating_fr(7.18)).with_plain("note TMDB 7,2/10"))
                    .chip(Chip::lang("VF · VO"))
                    .poster(
                        Img::Remote("https://image.example.test/w342/affiche.jpg".into()),
                        Some(&url),
                    )
                    .genres("Comédie · Horreur"),
            )
            .push_html_only(Block::text([lead]).top(20))
            .push(Block::play_button("Regarder maintenant", &url))
            .push(Block::small([cut(
                "Sur une île isolée, un jeune couple pénètre dans un restaurant gastronomique ayant une très bonne réputation. Le chef se lance alors dans un menu somptueux assorti à quelques surprises stupéfiantes.",
                240,
            )]).top(24))
            .push(Block::note("Sur la télé ou dans l'appli Jellyfin : sur l'accueil, dans les derniers ajouts.").top(16))
            .push(Block::footer(
                "Tu reçois ce mail parce que tu as demandé ce titre sur Groscailloux TV, le service de streaming privé entre amis.",
                "Une question, ou plus envie de recevoir ces mails quand une demande arrive ? Réponds simplement à ce mail.",
                Some("Visuels : TMDB."),
            ));
        m.render()
    }

    #[test]
    fn flagship_media_mail_matches_the_prototype_structure() {
        let r = media_movie();
        let h = &r.html;
        assert!(h.starts_with("<!DOCTYPE html>\n<html lang=\"fr\" dir=\"ltr\""));
        assert!(h.contains("<title>Le Menu est disponible</title>"));
        // composants dans l'ordre du prototype
        let order = [
            "gc:header",
            "gc:hero",
            "gc:title_card",
            "gc:text",
            "gc:button",
            "gc:text",
            "gc:note",
            "gc:footer",
        ];
        let found: Vec<&str> = h
            .match_indices("<!-- gc:")
            .map(|(i, _)| {
                let rest = &h[i + 5..];
                &rest[..rest.find(' ').unwrap()]
            })
            .collect();
        assert_eq!(found, order);
        // logo joint une fois, visuels distants
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.images[0].content_type, "image/png");
        assert!(h.contains(&format!("src=\"cid:{}\"", r.images[0].cid)));
        assert!(h.contains(r#"src="https://image.example.test/w780/fond.jpg""#));
        // typographie dans le HTML, pas dans le texte
        assert!(h.contains("vient d\u{2019}arriver. Bonne séance\u{a0}!"));
        // titre et titre original dans la même encre ; l'étoile hors encre, la note dans la sienne
        assert!(h.contains("Le Menu</h1><p style=\"margin:0 0 12px 0;font-size:14px;line-height:20px;color:#a3a7af;\">The Menu</p></div></div>"));
        assert!(h.contains("<span class=\"gc-o\" style=\"display:inline-block;\"><span class=\"gc-j\" style=\"display:inline-block;\">&#9654;&#65038;&nbsp;&nbsp;Regarder maintenant</span></span></a>"));
        assert!(h.contains("<span style=\"color:#d99a1e;\">&#9733;</span>&nbsp;<span class=\"gc-i\" style=\"display:inline-block;\"><span class=\"gc-j\" style=\"display:inline-block;\">7,2</span></span></span>"));
        checks::assert_gmail_dark_safe(h);
        assert!(!h.contains("{{") && !h.contains("token=") && !h.to_lowercase().contains("e50914"));
        assert!(h.len() < HTML_BUDGET, "{} octets", h.len());
        // second bloc de style : sélecteurs de classe seulement
        let mq = &STYLE_MOBILE[STYLE_MOBILE.find('{').unwrap() + 1..];
        for sel in mq
            .split('}')
            .filter_map(|r| r.split_once('{'))
            .map(|(s, _)| s)
        {
            for part in sel.split(',') {
                assert!(part.trim().starts_with('.'), "{part}");
            }
        }
        // aucune apostrophe droite entre deux lettres dans le texte visible
        let vis = visible(h);
        let cs: Vec<char> = vis.chars().collect();
        assert!(!cs
            .windows(3)
            .any(|w| w[1] == '\'' && w[0].is_alphabetic() && w[2].is_alphabetic()));
        let expected_text = "Salut Nina, le film que tu as demandé le 9 octobre vient d'arriver. Bonne séance !\n\n\
            Le Menu (The Menu)\n2022 · Film · 1 h 46 · note TMDB 7,2/10 · VF · VO\nComédie · Horreur\n\n\
            Regarder maintenant :\nhttps://tv.example.test/web/#/details?id=cadee972da3998273e8cc31e49fbef7e\n\n\
            Sur une île isolée, un jeune couple pénètre dans un restaurant gastronomique ayant une très bonne réputation. Le chef se lance alors dans un menu somptueux assorti à quelques surprises stupéfiantes.\n\n\
            Sur la télé ou dans l'appli Jellyfin : sur l'accueil, dans les derniers ajouts.\n\n\
            — Groscailloux TV\n\
            Tu reçois ce mail parce que tu as demandé ce titre sur Groscailloux TV, le service de streaming privé entre amis.\n\
            Une question, ou plus envie de recevoir ces mails quand une demande arrive ? Réponds simplement à ce mail.\n\
            Visuels : TMDB.\n";
        assert_eq!(r.text, expected_text);
        assert!(!r.text.contains('\u{a0}') && !r.text.contains('\u{2019}'));
    }

    #[test]
    fn every_dynamic_string_is_escaped() {
        let evil = r#"<script>alert("x")</script> & 'co'"#;
        let mut m = MailContent::new(evil, evil);
        m.push(Block::header(Some(evil)))
            .push(Block::hero(
                Img::Remote(format!("https://i.example.test/a.jpg?\"{evil}")),
                evil,
                "https://x.example.test/\"><script>",
            ))
            .push(
                TitleCard::new(evil, evil)
                    .original(evil)
                    .chip(Chip::plain(evil))
                    .chip(Chip::rating(evil))
                    .chip(Chip::lang(evil))
                    .genres(evil),
            )
            .push(Block::intro(evil, evil))
            .push(Block::text([Para::new()
                .text(evil)
                .link(evil, "https://x.example.test/?a=1&b=\"2\"")]))
            .push(Block::button(evil, "https://x.example.test/"))
            .push(Block::note(evil))
            .push(Block::fallback_link("https://x.example.test/\"<b>"))
            .push(Block::info_card([
                InfoRow::new(evil, evil),
                InfoRow::new(evil, "court"),
            ]))
            .push(Block::list_card([(evil, evil)]))
            .push(Block::steps([(evil, evil)]))
            .push(Block::h2(evil))
            .push(Block::notice(Tone::Warning, evil, evil))
            .push(Block::message_card(
                format!("{evil}\nligne 2\n\n<b>gras</b>"),
                evil,
            ))
            .push(Block::poster_row([PosterItem {
                image: Img::Remote("https://i.example.test/p.jpg".into()),
                title: evil.into(),
                sub: evil.into(),
                href: "https://x.example.test/".into(),
            }]))
            .push(Block::footer(evil, evil, Some(evil)));
        let r = m.render();
        assert!(!r.html.contains("<script>") && !r.html.contains("<b>"));
        assert!(!r.html.contains(r#""x")"#));
        assert!(r
            .html
            .contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;co&#39;"));
        assert!(r
            .html
            .contains("href=\"https://x.example.test/?a=1&amp;b=&quot;2&quot;\""));
        assert!(r.html.contains("ligne 2</p>") || r.html.contains("<br>ligne"));
        assert!(r.html.contains("&lt;b&gt;gras&lt;/b&gt;"));
        // le texte brut garde le texte tel quel (pas de HTML à échapper)
        assert!(r.text.contains(evil));
        // les enveloppes de Gmail n'échappent rien deux fois et tiennent avec des textes hostiles
        assert!(!r.html.contains("&amp;lt;") && !r.html.contains("&amp;amp;"));
        assert!(r.html.contains(
            "<div class=\"gc-i\"><div class=\"gc-j\"><h2 style=\"margin:0;font-size:18px;line-height:24px;font-weight:800;color:#ffffff;\">&lt;script&gt;"
        ));
        checks::assert_gmail_dark_safe(&r.html);
    }

    /// Un mail avec chaque composant, chaque ton (notice et valeur), les deux dispositions de la carte
    /// d'informations, une affiche jointe et un lien dans un paragraphe.
    fn every_block() -> RenderedMail {
        let poster = InlineImage::from_bytes("affiche", vec![0xFF, 0xD8, 0xFF, 1, 2, 3]).unwrap();
        let url = details("x");
        let mut m = MailContent::new(
            "Tout",
            "Chaque composant du gabarit, pour les contrôles du mode sombre.",
        );
        m.push(Block::header(Some(JF)))
            .push(Block::hero(
                Img::Remote("https://image.example.test/f.jpg".into()),
                "Fond",
                &url,
            ))
            .push(Block::banner(welcome_banner()))
            .push(
                TitleCard::new("Disponible", "Le Menu")
                    .original("The Menu")
                    .chip(Chip::plain("2022"))
                    .chip(Chip::rating("7,2"))
                    .chip(Chip::lang("VF · VO"))
                    .poster(Img::Inline(poster.clone()), Some(&url))
                    .genres("Comédie · Horreur"),
            )
            .push(Block::intro("Abonnement", "Ton accès"))
            .push(Block::text([Para::new()
                .text("Tout est expliqué : ")
                .link("les premiers pas", "https://o.example.test/premiers-pas")
                .text(".")]))
            .push(Block::small(["Petit texte atténué."]))
            .push(Block::play_button("Regarder", &url))
            .push(Block::note("Une note."))
            .push(Block::fallback_link("https://o.example.test/bienvenue/x"))
            .push(Block::info_card([
                InfoRow::new("Accès jusqu'au", "22 octobre 2026"),
                InfoRow::new("Info", "Neutre").tone(Tone::Info),
                InfoRow::new("Succès", "Actif").tone(Tone::Success),
                InfoRow::new("Avertissement", "Arrêté").tone(Tone::Warning),
                InfoRow::new("Erreur", "Refusé").tone(Tone::Error),
                InfoRow::new("Long", "une valeur bien trop longue pour tenir à droite")
                    .tone(Tone::Error),
            ]))
            .push(Block::h2("Sur quel écran ?"))
            .push(Block::list_card([
                ("Télé", "L'appli."),
                ("Ordinateur", "Le navigateur."),
            ]))
            .push(Block::steps([("Un", "Premier."), ("Deux", "Second.")]))
            .push(Block::notice(Tone::Info, "Info", "Neutre."))
            .push(Block::notice(Tone::Success, "Succès", "Vert."))
            .push(Block::notice(Tone::Warning, "Attention", "Ambre."))
            .push(Block::notice(Tone::Error, "Erreur", "Rouge."))
            .push(Block::message_card("Bonjour\n\nà samedi", "L'admin"))
            .push(Block::poster_row([PosterItem {
                image: Img::Inline(poster),
                title: "Avatar".into(),
                sub: "Film · 2009".into(),
                href: details("a"),
            }]))
            .push(Block::footer(
                "Pourquoi ce mail.",
                "Réponds à ce mail.",
                Some("Visuels : TMDB."),
            ));
        m.render()
    }

    #[test]
    fn gmail_block_holds_both_halves() {
        // un bloc à part, sélecteurs Gmail seulement : rien que Gmail refuse (sinon il jette tout le bloc)
        let body = STYLE_GMAIL
            .strip_prefix("<style>\n")
            .and_then(|s| s.strip_suffix("\n</style>"))
            .unwrap();
        for bad in [":root", "[", "@", "<u", "url("] {
            assert!(!body.contains(bad), "{bad}");
        }
        let rules: Vec<&str> = body.lines().collect();
        for r in &rules {
            assert!(r.starts_with("u + .body .gc-") && r.ends_with(";}"), "{r}");
        }
        // une surface = un dégradé d'une seule couleur, la même que son fond en ligne
        assert_eq!(body.matches("linear-gradient(").count(), SURFACES.len());
        for (k, c) in SURFACES {
            assert!(
                rules.contains(
                    &format!("u + .body .{k}{{background-image:linear-gradient({c},{c});}}")
                        .as_str()
                ),
                "{k}"
            );
        }
        // l'encre et la neutralisation des textes colorés, dans le même bloc
        for r in [
            "u + .body .gc-i{background:#000;mix-blend-mode:difference;}",
            "u + .body .gc-o{background:#000;mix-blend-mode:screen;}",
            "u + .body .gc-j{background:#000;mix-blend-mode:difference;}",
            &format!("u + .body .gc-n{{color:{STRONG}!important;}}"),
            &format!("u + .body .gc-m{{color:{NOTICE_TEXT}!important;}}"),
            // liens dans l'encre, dont ceux que Gmail crée lui-même dans le texte
            "u + .body .gc-j a{color:inherit!important;}",
        ] {
            assert!(rules.contains(&r), "{r}");
        }
        assert_eq!(rules.len(), SURFACES.len() + 6);
        // les autres blocs n'en portent aucune moitié
        for other in [STYLE_RESET, STYLE_MOBILE] {
            assert!(
                !other.contains("gradient") && !other.contains("blend") && !other.contains(".gc-k")
            );
        }
        assert!(STYLE_GMAIL.len() < 2_000, "{} octets", STYLE_GMAIL.len());
        // le bloc Gmail suit celui du téléphone, dans <head>
        let h = every_block().html;
        let (head, _) = h.split_once("</head>").unwrap();
        assert!(head.ends_with(&format!("{STYLE_MOBILE}\n{STYLE_GMAIL}\n")));
    }

    #[test]
    fn every_block_and_tone_stays_dark_and_readable_in_gmail() {
        let r = every_block();
        let h = &r.html;
        checks::assert_member_mail(&r);
        // chaque classe protégée sert, sauf aucune
        for (k, _) in SURFACES {
            assert!(
                h.contains(&format!("class=\"{k}\"")) || h.contains(&format!(" {k}\"")),
                "{k} jamais posée"
            );
        }
        // notices : fond teinté en ligne (Apple Mail, Outlook…), neutre dans Gmail ; texte neutralisé
        for bg in ["#0d241a", "#2a2108", "#2a1212"] {
            assert!(h.contains(&format!(
                "<td class=\"gc-k1\" bgcolor=\"{bg}\" style=\"background-color:{bg};"
            )));
        }
        assert_eq!(h.matches("<p class=\"gc-m\"").count(), 4);
        // valeurs colorées blanches dans Gmail, la valeur neutre telle quelle
        assert_eq!(h.matches("<td class=\"gc-n\"").count(), 4);
        // lien d'un paragraphe : blanc dans Gmail, bleu ailleurs
        assert!(h.contains(
            "<a class=\"gc-n\" href=\"https://o.example.test/premiers-pas\" style=\"color:#5aa6ff;"
        ));
        // le bouton : blanc sur le bleu protégé, libellé en screen puis difference
        assert!(h.contains("<td align=\"center\" class=\"gc-kb\" bgcolor=\"#1b6fd8\""));
        assert_eq!(h.matches("class=\"gc-o\"").count(), 1);
        // plus aucune bordure CSS colorée, sauf celle du bouton
        assert_eq!(h.matches("border:1px solid").count(), 1);
        assert!(!h.contains("border-top:") && !h.contains("border-left:"));
        // poids : chaque composant à la fois tient encore sous le budget (les vrais mails : < 20 Ko, voir
        // `mail_samples`)
        assert!(h.len() < HTML_BUDGET, "{} octets", h.len());
    }

    #[test]
    fn the_gmail_checker_catches_each_broken_rule() {
        let h = every_block().html;
        checks::assert_gmail_dark_safe(&h);
        // chaque règle cassée, et la raison attendue
        let broken = [
            // fond sans sa classe protégée
            (h.replacen("<span class=\"gc-k2\" ", "<span ", 1), "non protégé"),
            // bordure CSS (deviendrait claire)
            (
                h.replacen("padding:6px 11px;", "padding:6px 11px;border:1px solid #2a2d33;", 1),
                "bordure border:1px solid #2a2d33 hors encre",
            ),
            // texte clair hors encre (deviendrait sombre sur fond sombre)
            (
                h.replacen(
                    "text-transform:uppercase;color:#5aa6ff;",
                    "text-transform:uppercase;color:#e6e7ea;",
                    1,
                ),
                "texte #e6e7ea hors encre",
            ),
            // lien bleu dans l'encre sans neutralisation (deviendrait orange)
            (h.replacen("<a class=\"gc-n\" href", "<a href", 1), "non neutralisé pour Gmail"),
            // image dans l'encre (serait inversée)
            (
                h.replacen(
                    "<div class=\"gc-j\">",
                    "<div class=\"gc-j\"><img src=\"https://i.example.test/x.png\" alt=\"\">",
                    1,
                ),
                "dans l'encre",
            ),
            // dégradé en ligne (hors de la condition commune, et abîmé par Gmail Android)
            (
                h.replacen(
                    "style=\"background-color:#2a2d33;",
                    "style=\"background-image:linear-gradient(#2a2d33,#2a2d33);background-color:#2a2d33;",
                    1,
                ),
                "linear-gradient hors du bloc Gmail",
            ),
            // élément <u> (casserait le sélecteur)
            (h.replacen("Une note.", "Une <u>note</u>.", 1), "élément <u>"),
            // libellé du bouton hors de son encre
            (h.replacen("class=\"gc-o\"", "class=\"gc-x\"", 1), "gc-j seul"),
        ];
        for (i, (b, why)) in broken.iter().enumerate() {
            assert_ne!(b, &h, "cas {i} : rien remplacé");
            let err = std::panic::catch_unwind(|| checks::assert_gmail_dark_safe(b))
                .expect_err(&format!("cas {i} non détecté"));
            let msg = err
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| err.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(
                msg.contains(why),
                "cas {i} : « {msg} » au lieu de « {why} »"
            );
        }
    }

    #[test]
    fn links_other_than_web_or_mail_are_neutralised() {
        let mut m = MailContent::new("s", "p");
        m.push(Block::button("Clique", "javascript:alert(1)"))
            .push(Block::text([
                Para::new().link("ici", " JavaScript:alert(1)")
            ]))
            .push(Block::text([
                Para::new().link("écrire", "mailto:contact@example.test")
            ]))
            .push(Block::fallback_link("data:text/html,x"));
        let h = m.render().html;
        assert!(!h.to_lowercase().contains("href=\"javascript") && !h.contains("href=\"data:"));
        assert_eq!(h.matches("href=\"#\"").count(), 3);
        assert!(h.contains("href=\"mailto:contact@example.test\""));
    }

    /// Une URL écrite par un modérateur devient un lien (même règle que le tchat) coupable n'importe où ; la
    /// ponctuation qui la suit reste du texte ; le texte brut garde le message tel quel.
    #[test]
    fn message_card_links_web_addresses() {
        let text = "Le guide : https://tv.example.test/guide#a.\n(https://b.example.test/x), « https://c.example.test » !\nhttps://d.example.test/<script>\n\nMerci de lire ce message jusqu'au bout : il explique tout ce qui change pour toi.";
        let mut m = MailContent::new("Annonce", "p");
        m.push(Block::message_card(text, "L'admin"));
        let r = m.render();
        checks::assert_member_mail(&r);
        let h = &r.html;
        assert!(h.contains(
            "<a class=\"gc-n\" href=\"https://tv.example.test/guide#a\" style=\"color:#5aa6ff;text-decoration:underline;word-break:break-all;\">https://tv.example.test/guide#a</a>."
        ));
        assert!(h.contains(">https://b.example.test/x</a>),"));
        assert!(h.contains(">https://c.example.test</a>\u{a0}»\u{a0}!"));
        assert!(h.contains(">https://d.example.test/</a>&lt;script&gt;"));
        let card = &h[h.find("<!-- gc:message_card -->").unwrap()..];
        assert_eq!(card.matches("<a ").count(), 4);
        assert_eq!(card.matches("word-break:break-all;").count(), 4);
        assert!(r.text.starts_with(text) && !r.text.contains("<a"));
    }

    #[test]
    fn message_card_links_only_web_urls_without_typography() {
        let none = message_html(
            "javascript:alert(1) ftp://x.example.test www.example.test a@example.test https:// et https://.",
        );
        assert!(!none.contains("<a "), "{none}");
        // dans une URL : ni apostrophe courbe ni insécable, `&` et `'` échappés ; « : » qui suit garde son insécable
        let h = message_html("https://x.example.test/l'a?b=1&c=2 : ok");
        assert!(h.contains(r#"href="https://x.example.test/l&#39;a?b=1&amp;c=2""#));
        assert!(h.contains(">https://x.example.test/l&#39;a?b=1&amp;c=2</a>\u{a0}: ok"));
        assert!(!h.contains('\u{2019}'));
        // sans URL : le rendu d'avant
        let p = |mb: &str, body: String| {
            format!(
                r#"<p style="margin:0 0 {mb} 0;font-size:16px;line-height:25px;color:{TEXT};">{body}</p>"#
            )
        };
        assert_eq!(
            message_html("  Salut,\nl'annonce : rien à faire !\n\n\nÀ+  "),
            format!(
                "{}\n{}",
                p(
                    "12px",
                    format!("{}<br>{}", t("Salut,"), t("l'annonce : rien à faire !"))
                ),
                p("0", t("À+"))
            )
        );
    }

    #[test]
    fn text_order_can_differ_from_html_and_lines_can_be_glued() {
        let mut m = MailContent::new("s", "p");
        m.push(Block::h2("Sur quel écran ?"))
            .push(Block::list_card([
                ("Télé", "L'appli « Jellyfin »."),
                ("Ordinateur", "Ton navigateur."),
            ]))
            .glue()
            .push_with_text(
                Block::small([Para::new()
                    .text("Tout est expliqué pas à pas : ")
                    .link("les premiers pas", "https://o.example.test/premiers-pas")
                    .text(".")]),
                "Tout est expliqué pas à pas : https://o.example.test/premiers-pas",
            )
            .glue()
            .push_html_only(Block::note("seulement en HTML"))
            .push_text_only("seulement en texte")
            .push(Block::steps([
                ("Ouvre ton compte", "Rubrique « Paiements automatiques »."),
                ("C'est tout", "Rien d'autre."),
            ]))
            .push(Block::notice(
                Tone::Success,
                "Rien n'est perdu",
                "Ton historique t'attend.",
            ))
            .push(Block::info_card([
                InfoRow::new("Accès jusqu'au", "22 octobre 2026"),
                InfoRow::new("Renouvellement", "Arrêté").tone(Tone::Warning),
            ]))
            .push(Block::message_card("Bonjour\r\n\r\nà samedi", "L'admin"));
        let r = m.render();
        assert_eq!(
            r.text,
            "Sur quel écran ?\n- Télé : l'appli « Jellyfin ».\n- Ordinateur : ton navigateur.\n\
             Tout est expliqué pas à pas : https://o.example.test/premiers-pas\n\n\
             seulement en texte\n\n\
             1. Ouvre ton compte. Rubrique « Paiements automatiques ».\n2. C'est tout. Rien d'autre.\n\n\
             Rien n'est perdu : ton historique t'attend.\n\n\
             Accès jusqu'au : 22 octobre 2026\nRenouvellement : Arrêté\n\n\
             Bonjour\n\nà samedi\n\n— L'admin\n"
        );
        assert!(r.html.contains("seulement en HTML") && !r.html.contains("seulement en texte"));
        assert!(r.html.contains(">les premiers pas</a>."));
        assert!(r.images.is_empty());
        // message : deux paragraphes, pas de <br>
        assert!(r.html.contains(">Bonjour</p>\n<p style=\"margin:0 0 0 0;font-size:16px;line-height:25px;color:#e6e7ea;\">à samedi</p>"));
        // ton de la valeur (blanche dans Gmail : `gc-n`)
        assert!(r.html.contains(
            "<td class=\"gc-n\" valign=\"top\" align=\"right\" style=\"padding:10px 0;font-family:"
        ));
        assert!(r.html.contains(
            "color:#f0b429;text-align:right;\"><div class=\"gc-i\"><div class=\"gc-j\">Arrêté</div></div></td>"
        ));
        checks::assert_gmail_dark_safe(&r.html);
    }

    #[test]
    fn info_card_stacks_long_values_under_their_label() {
        let mut m = MailContent::new("s", "p");
        m.push(Block::info_card([
            InfoRow::new("Abonnement", "3,50 € par mois"),
            InfoRow::new("Abonnement", "3,50 € par mois, sans engagement"),
        ]));
        let h = m.render().html;
        assert_eq!(
            h.matches("colspan=\"2\" style=\"padding:10px 0 2px 0;")
                .count(),
            1
        );
        assert_eq!(h.matches("align=\"right\"").count(), 1);
        assert_eq!(h.matches("<tr><td colspan=\"2\" height=\"1\"").count(), 1);
    }

    #[test]
    fn poster_row_keeps_three_equal_cells_and_inline_images_once() {
        let poster = InlineImage::from_bytes("affiche", vec![0xFF, 0xD8, 0xFF, 1, 2, 3]).unwrap();
        let item = |t: &str| PosterItem {
            image: Img::Inline(poster.clone()),
            title: t.into(),
            sub: "Film · 2009".into(),
            href: details(t),
        };
        let mut m = MailContent::new("s", "p");
        m.push(Block::header(None))
            .push(Block::banner(welcome_banner()))
            .push(Block::poster_row([item("a"), item("b")]))
            .push(Block::poster_row([
                item("c"),
                item("d"),
                item("e"),
                item("f"),
            ]));
        let r = m.render();
        assert_eq!(r.html.matches("<td width=\"31%\"").count(), 6);
        assert_eq!(
            r.html
                .matches("<td width=\"31%\" valign=\"top\" style=\"width:31%;\">&nbsp;</td>")
                .count(),
            1
        );
        assert!(!r.html.contains("details?id=f"), "3 cases au plus");
        assert!(!r.text.contains("details?id=f"));
        // logo, bandeau, affiche : chacun une fois
        let kinds: Vec<&str> = r.images.iter().map(|i| i.content_type).collect();
        assert_eq!(kinds, ["image/png", "image/jpeg", "image/jpeg"]);
        assert_eq!(r.html.matches(&format!("cid:{}", poster.cid)).count(), 5);
        // sans lien d'accueil, le logo n'est pas cliquable
        assert!(!r.html.contains("<a href=\"\""));
        assert!(r.html.contains("<tr><td class=\"gc-hd\" align=\"left\" style=\"padding:24px 32px 20px 22px;\"><img src=\"cid:logo."));
    }

    #[test]
    fn embedded_assets_are_clean_png_and_jpeg() {
        let Img::Inline(l) = logo() else { panic!() };
        assert_eq!(l.content_type, "image/png");
        // en-tête IHDR : 496 × 76
        assert_eq!(&l.bytes[16..24], &[0, 0, 1, 240, 0, 0, 0, 76]);
        let Img::Inline(b) = welcome_banner() else {
            panic!()
        };
        assert_eq!(b.content_type, "image/jpeg");
        for bytes in [&l.bytes, &b.bytes] {
            for needle in [&b"http"[..], b"www.", b".com"] {
                assert!(!bytes.windows(needle.len()).any(|w| w == needle));
            }
        }
        assert!(l.bytes.len() < 24 * 1024 && b.bytes.len() < 32 * 1024);
    }

    #[test]
    fn dates_are_written_in_french_words() {
        // midi UTC : la même date dans tous les fuseaux d'Europe
        assert_eq!(date_long_fr(1_792_670_400), "22 octobre 2026");
        assert_eq!(day_month_fr(1_792_670_400), "22 octobre");
        assert_eq!(date_long_fr(1_793_534_400), "1er novembre 2026");
        assert_eq!(day_month_fr(1_798_718_400), "31 décembre");
        assert_eq!(date_long_fr(i64::MAX), "?");
    }

    #[test]
    fn french_typography_matches_the_prototypes() {
        let cases = [
            ("Saison 8 disponible", "Saison\u{a0}8\u{a0}disponible"),
            ("Saisons 1 à 10 disponibles", "Saisons 1\u{a0}à 10\u{a0}disponibles"),
            (
                "Ta demande du 9 octobre est arrivée : lance-le quand tu veux !",
                "Ta demande du\u{a0}9\u{a0}octobre est arrivée\u{a0}: lance-le quand tu veux\u{a0}!",
            ),
            ("l'appli « Jellyfin »", "l\u{2019}appli «\u{a0}Jellyfin\u{a0}»"),
            ("3,50 € par mois", "3,50\u{a0}€ par mois"),
            (
                "Samedi 17 octobre, entre 7 h et 8 h",
                "Samedi\u{a0}17\u{a0}octobre, entre\u{a0}7\u{a0}h et\u{a0}8\u{a0}h",
            ),
            ("aujourd'hui ? oui", "aujourd\u{2019}hui\u{a0}? oui"),
            ("l' été", "l' été"),
        ];
        for (input, want) in cases {
            assert_eq!(fr_typo(input), want, "{input}");
        }
    }

    #[test]
    fn synopsis_cut_and_original_title_rules() {
        let dexter = "Six mois se sont écoulés depuis l’assassinat de LaGuerta, et Dexter a beaucoup de difficultés à gérer sa vie de père, de frère et de tueur en série. Pendant ce temps, Deb doit faire face aux conséquences de ses actes, et une mystérieuse femme offre à Dexter des informations sur son passé.";
        assert_eq!(
            cut(dexter, 240),
            "Six mois se sont écoulés depuis l’assassinat de LaGuerta, et Dexter a beaucoup de difficultés à gérer sa vie de père, de frère et de tueur en série."
        );
        assert_eq!(
            cut("un deux trois quatre cinq six sept", 20),
            "un deux trois…"
        );
        assert_eq!(cut("  court \n texte ", 240), "court texte");
        assert!(show_original("Le Menu", "The Menu"));
        assert!(!show_original(
            "X-Files : Aux frontières du réel",
            "The X-Files"
        ));
        assert!(!show_original("Dexter", "Dexter"));
        assert!(!show_original("Inception", ""));
        assert_eq!(lower_first("Rubrique « X »."), "rubrique « X ».");
        assert_eq!(lower_first("« Jellyfin »"), "« Jellyfin »");
        assert_eq!(runtime_fr(106), "1 h 46");
        assert_eq!(runtime_fr(125), "2 h 05");
        assert_eq!(runtime_fr(120), "2 h");
        assert_eq!(runtime_fr(45), "45 min");
        assert_eq!(runtime_fr(0), "");
        assert_eq!(rating_fr(7.18), "7,2");
        assert_eq!(rating_fr(8.0), "8,0");
        assert_eq!(rating_fr(0.0), "");
    }

    mod fetch {
        use super::super::*;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn jpeg(n: usize) -> Vec<u8> {
            let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
            v.resize(n, 7);
            v
        }

        async fn serve(p: &str, body: Vec<u8>, content_type: &str) -> MockServer {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", content_type)
                        .set_body_bytes(body),
                )
                .mount(&server)
                .await;
            server
        }

        #[tokio::test]
        async fn jpeg_and_png_are_accepted() {
            let http = reqwest::Client::new();
            let s = serve("/a.jpg", jpeg(2000), "image/jpeg").await;
            let img = fetch_image(&http, &format!("{}/a.jpg", s.uri()), "affiche")
                .await
                .unwrap();
            assert_eq!((img.content_type, img.bytes.len()), ("image/jpeg", 2000));
            assert!(img.cid.starts_with("affiche."));
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend_from_slice(&[0; 50]);
            let s = serve("/l.png", png, "image/png; charset=binary").await;
            let img = fetch_image(&http, &format!("{}/l.png", s.uri()), "logo").await;
            assert_eq!(img.unwrap().content_type, "image/png");
        }

        #[tokio::test]
        async fn anything_else_gives_none() {
            let http = reqwest::Client::new();
            // WebP annoncé
            let s = serve("/w", b"RIFF\0\0\0\0WEBPVP8 ".to_vec(), "image/webp").await;
            assert!(fetch_image(&http, &format!("{}/w", s.uri()), "x")
                .await
                .is_none());
            // JPEG annoncé, HTML servi
            let s = serve("/h", b"<html>".to_vec(), "image/jpeg").await;
            assert!(fetch_image(&http, &format!("{}/h", s.uri()), "x")
                .await
                .is_none());
            // PNG annoncé, JPEG servi
            let s = serve("/m", jpeg(100), "image/png").await;
            assert!(fetch_image(&http, &format!("{}/m", s.uri()), "x")
                .await
                .is_none());
            // trop lourde
            let s = serve("/big", jpeg(5000), "image/jpeg").await;
            let url = format!("{}/big", s.uri());
            assert!(fetch_image_limited(&http, &url, "x", 4000, IMAGE_TIMEOUT)
                .await
                .is_none());
            assert!(fetch_image_limited(&http, &url, "x", 5000, IMAGE_TIMEOUT)
                .await
                .is_some());
            // 404, schéma, URL invalide
            assert!(fetch_image(&http, &format!("{}/absent", s.uri()), "x")
                .await
                .is_none());
            assert!(fetch_image(&http, "file:///etc/hostname", "x")
                .await
                .is_none());
            assert!(fetch_image(&http, "pas une url", "x").await.is_none());
        }

        #[tokio::test]
        async fn slow_servers_give_none() {
            let http = reqwest::Client::new();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "image/jpeg")
                        .set_body_bytes(jpeg(100))
                        .set_delay(Duration::from_secs(3)),
                )
                .mount(&server)
                .await;
            let url = format!("{}/lent.jpg", server.uri());
            let r = fetch_image_limited(
                &http,
                &url,
                "x",
                IMAGE_MAX_BYTES,
                Duration::from_millis(300),
            )
            .await;
            assert!(r.is_none());
        }
    }
}
