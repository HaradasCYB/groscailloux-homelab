//! Mail « ta demande est disponible » (2026-10-15) : un film, une ou plusieurs saisons d'une série, ou plusieurs
//! titres arrivés ensemble pour le même membre (récapitulatif). Rendu par une fonction pure ([`render`]) à partir de
//! données typées ([`AvailableMail`]) ; la tâche `media_mail` rassemble ces données (Jellyseerr, Jellyfin vu par le
//! membre, images jointes) et décide quand envoyer.
//!
//! Gabarit [`crate::mail_layout`], dans l'ordre des prototypes validés : logo, fond 16:9, affiche + puces, accroche,
//! un seul bouton « Regarder… » vers la fiche Jellyfin, synopsis, note télé, pied. Objets : `« Le Menu » est
//! disponible`, `La saison 8 de « Dexter » est disponible`, `Les saisons 1 à 10 de « X-Files » sont disponibles`,
//! `« Le Menu » et 3 autres demandes sont disponibles`. La rangée d'affiches n'existe que dans le récapitulatif (une
//! grille de titres fait passer un mail dans l'onglet « Promotions » de Gmail).

use crate::mail_layout::{
    cut, date_long_fr, day_month_fr, rating_fr, runtime_fr, show_original, Block, Chip, Img,
    MailContent, PosterItem, RenderedMail, TitleCard, POSTER_SLOTS,
};

const REASON_ONE: &str = "Tu reçois ce mail parce que tu as demandé ce titre sur Groscailloux TV, le service de streaming privé entre amis.";
const REASON_MANY: &str = "Tu reçois ce mail parce que tu as demandé ces titres sur Groscailloux TV, le service de streaming privé entre amis.";
const CONTACT: &str = "Une question, ou plus envie de recevoir ces mails quand une demande arrive ? Réponds simplement à ce mail.";
const CREDIT: &str = "Visuels : TMDB.";
const TV_NOTE: &str =
    "Sur la télé ou dans l'appli Jellyfin : sur l'accueil, dans les derniers ajouts.";
const ALSO: &str = "Aussi disponibles pour toi";
/// Synopsis du titre mis en avant : seul, ou dans un récapitulatif.
const SYNOPSIS_MAX: usize = 240;
const SYNOPSIS_DIGEST_MAX: usize = 160;
/// Titre dans un objet, dans le préheader.
const SUBJECT_TITLE_MAX: usize = 50;
const NAME_MAX: usize = 28;
const PREHEADER_MAX: usize = 110;

/// Film ou série.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleKind {
    Movie,
    Series,
}

/// Un titre arrivé pour le membre (textes bruts : le gabarit échappe et met la typographie).
#[derive(Clone, Debug, PartialEq)]
pub struct Title {
    pub kind: TitleKind,
    /// Titre français (Jellyseerr `?language=fr`).
    pub title: String,
    /// Titre original, affiché seulement s'il diffère vraiment ([`show_original`]).
    pub original: Option<String>,
    /// `2022` (film) ; `2006–2013`, `2013` ou `depuis 2019` (série).
    pub years: Option<String>,
    /// Série : saisons arrivées (sans la saison 0), triées.
    pub seasons: Vec<u32>,
    /// Série d'une seule saison, arrivée : présentée comme un tout (« la série »).
    pub whole_series: bool,
    /// Série : épisodes des saisons arrivées.
    pub episodes: Option<u32>,
    /// Film : durée en minutes.
    pub runtime_mins: Option<u32>,
    /// Note TMDB sur 10.
    pub rating: Option<f64>,
    /// Puce de langue : `VF · VO`, `VF`, `VOSTFR`, `VO`.
    pub lang: Option<String>,
    /// Genres en français (3 au plus sont montrés).
    pub genres: Vec<String>,
    pub synopsis: String,
    /// Affiche (celle de la saison s'il n'y en a qu'une) et fond 16:9, jointes au mail ou distantes.
    pub poster: Option<Img>,
    pub backdrop: Option<Img>,
    /// Fiche Jellyfin : `<JELLYFIN_PUBLIC_URL>/web/#/details?id=<id>` (film, saison, ou série).
    pub watch_url: String,
    /// Date de la demande (la plus ancienne si le titre en réunit plusieurs).
    pub requested_at: Option<i64>,
}

/// Le mail d'un membre : un titre mis en avant, et les autres titres arrivés dans le même passage.
#[derive(Clone, Debug, PartialEq)]
pub struct AvailableMail {
    /// Nom affiché du membre (salutation), vide = « Salut, ».
    pub name: String,
    pub hero: Title,
    /// Récapitulatif : 3 en rangée d'affiches ([`POSTER_SLOTS`]), les suivants en liste dans la version texte.
    pub others: Vec<Title>,
    /// Accueil Jellyfin (`JELLYFIN_PUBLIC_URL`) : lien du logo.
    pub home_url: String,
    /// Date du mail : une demande d'une autre année porte son année (« le 3 décembre 2025 »).
    pub at: i64,
}

/// Forme d'un titre : film, série entière, une saison, plusieurs saisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape<'a> {
    Movie,
    Whole,
    One(u32),
    Many(&'a [u32]),
}

fn shape(t: &Title) -> Shape<'_> {
    match (t.kind, t.seasons.as_slice()) {
        (TitleKind::Movie, _) => Shape::Movie,
        (TitleKind::Series, _) if t.whole_series => Shape::Whole,
        (TitleKind::Series, []) => Shape::Whole,
        (TitleKind::Series, [n]) => Shape::One(*n),
        (TitleKind::Series, s) => Shape::Many(s),
    }
}

/// `a`, `a et b`, `a, b et c`.
fn join_fr(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} et {last}", rest.join(", ")),
    }
}

/// Saisons en français : `8`, `1 et 2`, `1 à 10`, `1, 3 et 5`, `1 à 3, 5 et 6` (suites de 3 ou plus en « a à b »).
pub fn seasons_fr(seasons: &[u32]) -> String {
    let mut s = seasons.to_vec();
    s.sort_unstable();
    s.dedup();
    let mut items = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let mut j = i;
        while j + 1 < s.len() && s[j + 1] == s[j] + 1 {
            j += 1;
        }
        if j - i >= 2 {
            items.push(format!("{} à {}", s[i], s[j]));
        } else {
            items.extend(s[i..=j].iter().map(u32::to_string));
        }
        i = j + 1;
    }
    join_fr(&items)
}

fn quoted(t: &Title, max: usize) -> String {
    format!("« {} »", cut(&t.title, max))
}

/// Objet : le titre en tête, un seul motif par forme (jamais deux deux-points).
pub fn subject(m: &AvailableMail) -> String {
    let hero = quoted(&m.hero, SUBJECT_TITLE_MAX);
    match m.others.as_slice() {
        [] => match shape(&m.hero) {
            Shape::Movie | Shape::Whole => format!("{hero} est disponible"),
            Shape::One(n) => format!("La saison {n} de {hero} est disponible"),
            Shape::Many(s) => format!("Les saisons {} de {hero} sont disponibles", seasons_fr(s)),
        },
        [other] => format!(
            "{hero} et {} sont disponibles",
            quoted(other, SUBJECT_TITLE_MAX)
        ),
        others => format!(
            "{hero} et {} autres demandes sont disponibles",
            others.len()
        ),
    }
}

/// ` le 9 octobre` (` le 3 décembre 2025` pour une autre année que celle du mail), rien sans date.
fn asked_on(t: &Title, at: i64) -> String {
    use chrono::Datelike;
    let Some(r) = t.requested_at.filter(|r| *r > 0) else {
        return String::new();
    };
    let year = |x: i64| {
        chrono::DateTime::from_timestamp(x, 0).map(|d| d.with_timezone(&chrono::Local).year())
    };
    if year(r) == year(at) {
        format!(" le {}", day_month_fr(r))
    } else {
        format!(" le {}", date_long_fr(r))
    }
}

fn hello(name: &str) -> String {
    match name.trim() {
        "" => "Salut".into(),
        n => format!("Salut {n}"),
    }
}

fn eyebrow(m: &AvailableMail) -> String {
    if !m.others.is_empty() {
        return format!("{} demandes disponibles", m.others.len() + 1);
    }
    match shape(&m.hero) {
        Shape::Movie | Shape::Whole => "Ta demande est disponible".into(),
        Shape::One(n) => format!("Saison {n} disponible"),
        Shape::Many(s) => format!("Saisons {} disponibles", seasons_fr(s)),
    }
}

fn lead(m: &AvailableMail) -> String {
    let hi = hello(&m.name);
    let t = &m.hero;
    if !m.others.is_empty() {
        return format!(
            "{hi}, {} de tes demandes sont arrivées en même temps. On commence par « {} » ?",
            m.others.len() + 1,
            t.title
        );
    }
    let on = asked_on(t, m.at);
    match shape(t) {
        Shape::Movie => {
            format!("{hi}, le film que tu as demandé{on} vient d'arriver. Bonne séance !")
        }
        Shape::Whole => {
            format!("{hi}, la série que tu as demandée{on} vient d'arriver. Bon visionnage !")
        }
        Shape::One(n) => {
            format!("{hi}, la saison {n} que tu as demandée{on} vient d'arriver. Bon visionnage !")
        }
        Shape::Many(s) => format!(
            "{hi}, les saisons {} que tu as demandées{on} sont arrivées. Bon visionnage !",
            seasons_fr(s)
        ),
    }
}

fn preheader(m: &AvailableMail) -> String {
    const WHEN: &str = "quand tu veux, sur la télé, le téléphone ou l'ordinateur.";
    let t = &m.hero;
    if !m.others.is_empty() {
        let all: Vec<&Title> = std::iter::once(t).chain(m.others.iter()).collect();
        // autant de titres que le préheader en tient, les autres comptés
        for shown in (1..=all.len().min(4)).rev() {
            // entre « » comme dans l'objet : le deux-points d'un titre ne se mêle pas à celui de la phrase
            let mut names: Vec<String> = all[..shown].iter().map(|x| quoted(x, NAME_MAX)).collect();
            let left = all.len() - shown;
            if left > 0 {
                names.push(if left == 1 {
                    "un autre titre".into()
                } else {
                    format!("{left} autres titres")
                });
            }
            let p = format!("{} t'attendent : bonne séance !", join_fr(&names));
            if p.chars().count() <= PREHEADER_MAX || shown == 1 {
                return p;
            }
        }
    }
    let on = asked_on(t, m.at).replacen(" le ", " du ", 1);
    match (shape(t), t.episodes) {
        (Shape::Movie, _) => format!("Ta demande{on} est arrivée : lance-le {WHEN}"),
        (_, Some(1)) => format!("L'épisode est prêt : lance-le {WHEN}"),
        (Shape::Many(_), Some(e)) => format!("{e} épisodes t'attendent : lance-les {WHEN}"),
        (_, Some(e)) => format!("Les {e} épisodes sont prêts : lance-les {WHEN}"),
        (Shape::Many(_), None) => format!("Ta demande{on} est arrivée : lance-les {WHEN}"),
        (_, None) => format!("Ta demande{on} est arrivée : lance-la {WHEN}"),
    }
}

/// Libellé du bouton, d'après la forme du titre mis en avant (son nom est juste au-dessus, dans le h1) : jamais un
/// titre coupé, et une seule ligne sur un téléphone de 320 px.
fn button(m: &AvailableMail) -> String {
    match shape(&m.hero) {
        Shape::Movie if m.others.is_empty() => "Regarder maintenant".into(),
        Shape::Movie => "Regarder le film".into(),
        Shape::One(n) => format!("Regarder la saison {n}"),
        Shape::Whole | Shape::Many(_) => "Regarder la série".into(),
    }
}

fn episodes_text(e: u32) -> String {
    if e == 1 {
        "1 épisode".into()
    } else {
        format!("{e} épisodes")
    }
}

fn rating_text(t: &Title) -> Option<String> {
    t.rating.map(rating_fr).filter(|r| !r.is_empty())
}

/// Carte du titre mis en avant, et sa version texte : `Titre (Titre original)`, la ligne des puces, les genres.
fn card(t: &Title, eyebrow: String) -> (TitleCard, String) {
    let mut c = TitleCard::new(eyebrow, t.title.clone());
    if let Some(o) = &t.original {
        c = c.original(o.clone());
    }
    let years = t.years.clone().filter(|y| !y.is_empty());
    let runtime = t
        .runtime_mins
        .map(runtime_fr)
        .filter(|r| !r.is_empty() && t.kind == TitleKind::Movie);
    let episodes = t
        .episodes
        .filter(|e| *e > 0 && t.kind == TitleKind::Series)
        .map(episodes_text);
    let rating = rating_text(t);
    let lang = t.lang.clone().filter(|l| !l.is_empty());
    // puces : années · Film · durée · ★ note · langue (série : années · épisodes · ★ · langue)
    let mut chips: Vec<Chip> = Vec::new();
    chips.extend(years.clone().map(Chip::plain));
    if t.kind == TitleKind::Movie {
        chips.push(Chip::plain("Film"));
    }
    chips.extend(runtime.clone().map(Chip::plain));
    chips.extend(episodes.clone().map(Chip::plain));
    chips.extend(rating.clone().map(Chip::rating));
    chips.extend(lang.clone().map(Chip::lang));
    for ch in chips {
        c = c.chip(ch);
    }
    let genres: Vec<&str> = t
        .genres
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .take(3)
        .collect();
    let genres = genres.join(" · ");
    c = c.genres(genres.clone());
    if let Some(p) = &t.poster {
        c = c.poster(p.clone(), Some(&t.watch_url));
    }
    // texte : la sorte d'abord (« Film · 2022 · 1 h 46 · note TMDB 7,2/10 · VF · VO »)
    let seasons = match shape(t) {
        Shape::One(n) => Some(format!("saison {n}")),
        Shape::Many(s) => Some(format!("saisons {}", seasons_fr(s))),
        Shape::Movie | Shape::Whole => None,
    };
    let kind = match t.kind {
        TitleKind::Movie => "Film",
        TitleKind::Series => "Série",
    };
    let meta: Vec<String> = std::iter::once(kind.to_string())
        .chain(years)
        .chain(seasons)
        .chain(runtime)
        .chain(episodes)
        .chain(rating.map(|r| format!("note TMDB {r}/10")))
        .chain(lang)
        .collect();
    let head = match t.original.as_deref().filter(|o| show_original(&t.title, o)) {
        Some(o) => format!("{} ({o})", t.title),
        None => t.title.clone(),
    };
    let mut text = format!("{head}\n{}", meta.join(" · "));
    if !genres.is_empty() {
        text.push('\n');
        text.push_str(&genres);
    }
    (c, text)
}

/// Sous-titre d'un titre du récapitulatif : `Film · 2009`, `Série · saison 8`, `Série · saisons 1 à 10`.
fn sub(t: &Title) -> String {
    let with_years = |kind: &str| match t.years.as_deref().filter(|y| !y.is_empty()) {
        Some(y) => format!("{kind} · {y}"),
        None => kind.to_string(),
    };
    match shape(t) {
        Shape::Movie => with_years("Film"),
        Shape::Whole => with_years("Série"),
        Shape::One(n) => format!("Série · saison {n}"),
        Shape::Many(s) => format!("Série · saisons {}", seasons_fr(s)),
    }
}

fn list_line(t: &Title) -> String {
    format!("- {} ({}) : {}", t.title, sub(t), t.watch_url)
}

/// Le mail « disponible » d'un membre.
pub fn render(m: &AvailableMail) -> RenderedMail {
    let hero = &m.hero;
    let digest = !m.others.is_empty();
    let mut c = MailContent::new(subject(m), preheader(m));
    let lead = lead(m);
    c.push(Block::header(Some(&m.home_url)));
    if let Some(b) = &hero.backdrop {
        c.push(Block::hero(b.clone(), hero.title.clone(), &hero.watch_url));
    }
    let (card, card_text) = card(hero, eyebrow(m));
    c.push_text_only(lead.clone())
        .push_with_text(card, card_text)
        .push_html_only(Block::text([lead.as_str()]).top(20))
        .push(Block::play_button(button(m), hero.watch_url.clone()));
    let synopsis = cut(
        &hero.synopsis,
        if digest {
            SYNOPSIS_DIGEST_MAX
        } else {
            SYNOPSIS_MAX
        },
    );
    if !synopsis.is_empty() {
        c.push(Block::small([synopsis]).top(24));
    }
    let mut pictures = hero.backdrop.is_some() || hero.poster.is_some();
    if digest {
        // rangée : les 3 premiers titres qui ont une affiche ; les autres, en liste dans le texte
        let (row, rest): (Vec<&Title>, Vec<&Title>) = {
            let mut row = Vec::new();
            let mut rest = Vec::new();
            for t in &m.others {
                if t.poster.is_some() && row.len() < POSTER_SLOTS {
                    row.push(t);
                } else {
                    rest.push(t);
                }
            }
            (row, rest)
        };
        pictures |= !row.is_empty();
        c.push_with_text(Block::h2(ALSO).top(32), format!("{ALSO} :"));
        if !row.is_empty() {
            c.push(Block::poster_row(row.iter().map(|t| PosterItem {
                image: t.poster.clone().expect("affiche vérifiée"),
                title: t.title.clone(),
                sub: sub(t),
                href: t.watch_url.clone(),
            })))
            .glue();
        }
        if !rest.is_empty() {
            let note = if row.is_empty() {
                let names: Vec<String> = rest.iter().map(|t| quoted(t, NAME_MAX)).collect();
                format!("{}.", join_fr(&names))
            } else if rest.len() == 1 {
                "Et un autre titre t'attend sur l'accueil.".to_string()
            } else {
                format!("Et {} autres titres t'attendent sur l'accueil.", rest.len())
            };
            let lines: Vec<String> = rest.iter().map(|t| list_line(t)).collect();
            let top = if row.is_empty() { 0 } else { 12 };
            c.push_with_text(Block::note(note).top(top), lines.join("\n"))
                .glue();
        }
    }
    c.push(Block::note(TV_NOTE).top(16)).push(Block::footer(
        if digest { REASON_MANY } else { REASON_ONE },
        CONTACT,
        pictures.then_some(CREDIT),
    ));
    c.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::InlineImage;
    use crate::mail_layout::checks::{assert_member_mail, visible};

    const JF: &str = "https://tv.example.test";

    fn details(id: &str) -> String {
        format!("{JF}/web/#/details?id={id}")
    }

    fn jpeg(tag: u8) -> Img {
        Img::Inline(
            InlineImage::from_bytes("affiche", vec![0xFF, 0xD8, 0xFF, 0xE0, tag, 1, 2, 3]).unwrap(),
        )
    }

    /// 2026-10-09 12:00 UTC.
    const OCT_9: i64 = 1_791_547_200;

    fn le_menu() -> Title {
        Title {
            kind: TitleKind::Movie,
            title: "Le Menu".into(),
            original: Some("The Menu".into()),
            years: Some("2022".into()),
            seasons: vec![],
            whole_series: false,
            episodes: None,
            runtime_mins: Some(106),
            rating: Some(7.18),
            lang: Some("VF · VO".into()),
            genres: vec!["Comédie".into(), "Horreur".into()],
            synopsis: "Sur une île isolée, un jeune couple pénètre dans un restaurant gastronomique ayant une très bonne réputation. Le chef se lance alors dans un menu somptueux assorti à quelques surprises stupéfiantes.".into(),
            poster: Some(jpeg(1)),
            backdrop: Some(jpeg(2)),
            watch_url: details("cadee972da3998273e8cc31e49fbef7e"),
            requested_at: Some(OCT_9),
        }
    }

    fn dexter(seasons: &[u32]) -> Title {
        Title {
            kind: TitleKind::Series,
            title: "Dexter".into(),
            original: Some("Dexter".into()),
            years: Some("2006–2013".into()),
            seasons: seasons.to_vec(),
            whole_series: false,
            episodes: Some(12 * seasons.len() as u32),
            runtime_mins: None,
            rating: Some(8.241),
            lang: Some("VF · VO".into()),
            genres: vec![
                "Crime".into(),
                "Drame".into(),
                "Mystère".into(),
                "Thriller".into(),
            ],
            synopsis: "Six mois se sont écoulés depuis l'assassinat de LaGuerta.".into(),
            poster: Some(jpeg(3)),
            backdrop: None,
            watch_url: details("9693d143b641daa88c6143cdccfeb265"),
            requested_at: Some(OCT_9 - 7 * 86_400),
        }
    }

    fn mail(hero: Title, others: Vec<Title>) -> AvailableMail {
        AvailableMail {
            name: "Nina".into(),
            hero,
            others,
            home_url: JF.into(),
            at: OCT_9 + 86_400,
        }
    }

    #[test]
    fn seasons_are_written_in_french() {
        assert_eq!(seasons_fr(&[8]), "8");
        assert_eq!(seasons_fr(&[2, 1]), "1 et 2");
        assert_eq!(seasons_fr(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), "1 à 10");
        assert_eq!(seasons_fr(&[1, 3, 5]), "1, 3 et 5");
        assert_eq!(seasons_fr(&[1, 2, 3, 5, 6]), "1 à 3, 5 et 6");
        assert_eq!(seasons_fr(&[3, 3, 4]), "3 et 4");
        assert_eq!(seasons_fr(&[]), "");
    }

    #[test]
    fn movie_mail_is_the_flagship() {
        let r = render(&mail(le_menu(), vec![]));
        assert_member_mail(&r);
        assert_eq!(r.subject, "« Le Menu » est disponible");
        assert_eq!(
            r.preheader,
            "Ta demande du 9 octobre est arrivée : lance-le quand tu veux, sur la télé, le téléphone ou l'ordinateur."
        );
        let url = details("cadee972da3998273e8cc31e49fbef7e");
        assert!(r.text.starts_with(
            "Salut Nina, le film que tu as demandé le 9 octobre vient d'arriver. Bonne séance !\n\n\
             Le Menu (The Menu)\nFilm · 2022 · 1 h 46 · note TMDB 7,2/10 · VF · VO\nComédie · Horreur\n\n\
             Regarder maintenant :\n"
        ), "{}", r.text);
        assert!(r.text.contains(&format!("{url}\n")));
        assert!(r.text.contains("Visuels : TMDB."));
        // fond, affiche et bouton mènent à la même fiche ; le logo à l'accueil
        assert_eq!(r.html.matches(&url).count(), 3, "{}", r.html);
        assert!(r.html.contains("<!-- gc:hero -->") && !r.html.contains("gc:poster_row"));
        assert_eq!(r.images.len(), 3, "logo, fond, affiche");
        let v = visible(&r.html);
        assert!(v.contains("Ta demande est disponible") && v.contains("Regarder maintenant"));
    }

    #[test]
    fn one_season_several_seasons_and_whole_series() {
        let r = render(&mail(dexter(&[8]), vec![]));
        assert_member_mail(&r);
        assert_eq!(r.subject, "La saison 8 de « Dexter » est disponible");
        assert!(r.text.starts_with(
            "Salut Nina, la saison 8 que tu as demandée le 2 octobre vient d'arriver. Bon visionnage !\n\n\
             Dexter\nSérie · 2006–2013 · saison 8 · 12 épisodes · note TMDB 8,2/10 · VF · VO\nCrime · Drame · Mystère\n\n\
             Regarder la saison 8 :\n"
        ), "{}", r.text);
        assert!(r.preheader.starts_with("Les 12 épisodes sont prêts"));
        assert!(!r.html.contains("gc:hero"), "pas de fond, pas de bloc");

        let r = render(&mail(dexter(&[1, 2, 3]), vec![]));
        assert_member_mail(&r);
        assert_eq!(
            r.subject,
            "Les saisons 1 à 3 de « Dexter » sont disponibles"
        );
        assert!(r
            .text
            .contains("les saisons 1 à 3 que tu as demandées le 2 octobre sont arrivées"));
        assert!(r.text.contains("Regarder la série :\n"));
        assert!(r.preheader.starts_with("36 épisodes t'attendent"));

        let mut mini = dexter(&[1]);
        mini.title = "Chernobyl".into();
        mini.whole_series = true;
        let r = render(&mail(mini, vec![]));
        assert_eq!(r.subject, "« Chernobyl » est disponible");
        assert!(r
            .text
            .contains("la série que tu as demandée le 2 octobre vient d'arriver"));
        assert!(r.text.contains("\nSérie · 2006–2013 · 12 épisodes ·"));
    }

    #[test]
    fn digest_puts_one_title_first_and_lists_every_other() {
        let mut ib = le_menu();
        ib.title = "Inglourious Basterds".into();
        ib.original = None;
        ib.watch_url = details("8630e89a62b5e79d3b8341622691a98e");
        ib.poster = Some(jpeg(9));
        let mut xf = dexter(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        xf.title = "X-Files : Aux frontières du réel".into();
        xf.watch_url = details("15b82381bef9f802fade3b25233e6259");
        xf.poster = None; // sans affiche : dans la note, pas dans la rangée
        let r = render(&mail(le_menu(), vec![ib.clone(), dexter(&[8]), xf.clone()]));
        assert_member_mail(&r);
        assert_eq!(
            r.subject,
            "« Le Menu » et 3 autres demandes sont disponibles"
        );
        assert!(r.html.contains("gc:poster_row"));
        let v = visible(&r.html).replace(['\u{a0}', '\u{202f}'], " ");
        assert!(v.contains("4 demandes disponibles") && v.contains("Regarder le film"));
        assert!(r.text.contains("On commence par « Le Menu » ?"));
        assert!(r.text.contains("Regarder le film :\n"));
        assert!(
            v.contains("Et un autre titre t’attend sur l’accueil."),
            "{v}"
        );
        for t in [&ib, &xf] {
            assert!(r.text.contains(&t.watch_url), "{}", t.title);
        }
        assert!(
            r.text
                .contains("Aussi disponibles pour toi :\n- Inglourious Basterds (Film · 2022) : "),
            "{}",
            r.text
        );
        assert!(r
            .text
            .contains("- X-Files : Aux frontières du réel (Série · saisons 1 à 10) : "));
        assert!(r
            .text
            .contains("Tu reçois ce mail parce que tu as demandé ces titres"));
        assert!(
            r.preheader.ends_with("t'attendent : bonne séance !"),
            "{}",
            r.preheader
        );
        assert!(r.preheader.chars().count() <= PREHEADER_MAX);

        // deux titres : les deux nommés dans l'objet
        let r = render(&mail(le_menu(), vec![dexter(&[8])]));
        assert_eq!(r.subject, "« Le Menu » et « Dexter » sont disponibles");
        assert_eq!(
            r.preheader,
            "« Le Menu » et « Dexter » t'attendent : bonne séance !"
        );
    }

    /// Titres longs à sous-titre (vrais titres du 15/10) : bouton court sur une ligne, titres délimités dans le
    /// préheader, jamais « …, ».
    #[test]
    fn digest_of_long_titles_keeps_a_short_button_and_a_readable_preheader() {
        let mut gb = le_menu();
        gb.title = "Green Book : Sur les routes du Sud".into();
        let mut xf = dexter(&[3]);
        xf.title = "X-Files : Aux frontières du réel".into();
        let mut cage = le_menu();
        cage.title = "La Cage dorée".into();
        let mut fury = le_menu();
        fury.title = "Fury".into();
        let r = render(&mail(gb, vec![xf.clone(), cage, fury]));
        assert_member_mail(&r);
        let v = visible(&r.html);
        assert!(v.contains("Regarder le film") && !v.contains("Regarder Green"));
        assert!(!r.preheader.contains("…,"), "{}", r.preheader);
        assert!(
            r.preheader.starts_with("« Green Book : Sur les routes"),
            "{}",
            r.preheader
        );
        assert!(
            r.preheader.chars().count() <= PREHEADER_MAX,
            "{}",
            r.preheader
        );
        assert!(r
            .text
            .contains("On commence par « Green Book : Sur les routes du Sud » ?"));
        // une saison en avant : le bouton dit laquelle
        let r = render(&mail(xf, vec![le_menu()]));
        assert!(r.text.contains("Regarder la saison 3 :\n"));
    }

    #[test]
    fn no_artwork_no_credit_and_user_text_is_escaped() {
        let mut t = le_menu();
        t.title = "<b>Le Menu</b>".into();
        t.poster = None;
        t.backdrop = None;
        t.requested_at = None;
        t.synopsis = "Un chef & ses <i>surprises</i>.".into();
        let mut m = mail(t, vec![]);
        m.name = "<script>Nina</script>".into();
        let r = render(&m);
        assert_member_mail(&r);
        assert!(!r.html.contains("<b>Le Menu") && r.html.contains("&lt;b&gt;Le Menu"));
        assert!(!r.html.contains("<script>") && !r.html.contains("<i>surprises"));
        assert!(!r.text.contains("Visuels : TMDB") && !r.html.contains("Visuels"));
        assert_eq!(r.images.len(), 1, "logo seul");
        assert!(r.text.contains("le film que tu as demandé vient d'arriver"));
        assert!(r.preheader.starts_with("Ta demande est arrivée"));
    }

    #[test]
    fn a_request_from_another_year_carries_its_year() {
        let mut t = le_menu();
        t.requested_at = Some(OCT_9 - 300 * 86_400);
        let m = AvailableMail {
            at: OCT_9 + 100 * 86_400,
            ..mail(t, vec![])
        };
        let r = render(&m);
        assert!(
            r.text
                .contains("que tu as demandé le 13 décembre 2025 vient"),
            "{}",
            r.text
        );
    }

    #[test]
    fn long_titles_are_cut_in_the_subject_only() {
        let mut t = le_menu();
        t.title =
            "Un titre vraiment très long qui ne tiendrait jamais dans un objet de mail".into();
        let r = render(&mail(t.clone(), vec![]));
        assert!(r.subject.chars().count() <= 70, "{}", r.subject);
        assert!(r.subject.contains('…'));
        assert!(r.text.contains(&t.title));
    }
}
