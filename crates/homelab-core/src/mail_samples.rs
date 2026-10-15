//! Exemples de chaque mail qu'un membre peut recevoir (2026-10-15), pour les voir dans une vraie boîte (placement,
//! rendu) avant de les valider : `homelabctl mail-test <adresse> --all | --kind <type>` les fait envoyer par homelabd,
//! à 2 s d'écart ; `homelabctl mail-test --dump <dossier>` les écrit sans rien envoyer (`<type>.html` avec ses images
//! à côté, `<type>.eml` = le message MIME complet, `<type>.txt` = la version texte), sans homelabd.
//!
//! Chaque exemple est rendu par la **même fonction pure** que le vrai envoi, pour un compte fictif « Exemple », avec
//! des dates et un prix plausibles (réglages de `[subscriptions]`) ; objet préfixé `[Exemple] `, mêmes en-têtes que
//! le vrai mail (`Reply-To` des mails de bienvenue, `List-Unsubscribe` des mails « disponible »). Les mails
//! « disponible » prennent de **vrais titres** : derniers film, saison, série de plusieurs saisons et titres arrivés
//! selon Jellyseerr, relus dans Jellyfin par un compte qui voit toutes les bibliothèques, avec leurs images ;
//! Jellyseerr injoignable (ou rien de tel) : titres d'exemple sans image. Lectures seulement (GET).
//!
//! Rien n'est écrit dans l'état, sauf le lien de démonstration des mails de bienvenue quand homelabd les envoie
//! (comme `mail-test` l'a toujours fait : la page montre un exemple, aucun compte n'est touché) ; `--dump` met un
//! lien factice. Aucun compte, aucun abonnement touché.
//!
//! **Aperçu d'une annonce** (`homelabctl mail-test <adresse> --announce <fichier>`, [`announcement`]) : le mail exact
//! que `homelabctl chat announce <fichier> --mail` enverrait aux membres (même nettoyage, même fonction, même auteur,
//! même enveloppe), objet préfixé [`chat::PREVIEW_PREFIX`], à l'adresse donnée seulement. Rien n'est publié dans le
//! tchat ni sur Discord.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::available_mail::{self, AvailableMail, Title, TitleKind};
use crate::chat;
use crate::config::Smtp;
use crate::context::TaskContext;
use crate::mail::{self, Outgoing};
use crate::mail_layout::{RenderedMail, POSTER_SLOTS};
use crate::secret::Secret;
use crate::state::{now, MediaMailRecord};
use crate::subscription_ops;
use crate::subscriptions::{self as subs, Reminder, Status};
use crate::tasks::media_mail::{self, Ready, Recipient};
use crate::welcome::{self, ActivatedButton};

/// Compte fictif des exemples (salutation, nom du destinataire, compte des liens de paiement).
pub const NAME: &str = "Exemple";
/// Début de l'objet de chaque exemple.
pub const SUBJECT_PREFIX: &str = "[Exemple] ";
/// Écart entre deux envois.
pub const SEND_GAP: Duration = Duration::from_secs(2);
/// Auteur des exemples du tchat (le vrai mail porte le nom du modérateur).
const CHAT_AUTHOR: &str = "L'admin";
/// Jeton factice des liens de bienvenue écrits par `--dump` (la page répond « lien invalide »).
const DUMP_TOKEN: &str = "exemple-de-lien";
/// Destinataire du `.eml` de `--dump` sans adresse donnée.
pub const DUMP_TO: &str = "exemple@example.test";
const DAY: i64 = 86_400;

const DIRECT_TEXT: &str = "Petit point sur ta demande : la saison 2 arrive samedi, en VF. Je te préviens dès qu'elle est en ligne.\n\nSi tu as un souci de lecture d'ici là, dis-le-moi directement ici.";
const ANNOUNCE_TEXT: &str = "Maintenance samedi matin\nSamedi, entre 7 h et 8 h, le serveur redémarre pour une mise à jour. La lecture sera coupée quelques minutes.\n\nTout revient seul ensuite : rien à faire de ton côté. Merci pour ta patience !";

/// Catalogue : type (`--kind`) et ce que c'est, dans l'ordre des envois.
pub const KINDS: [(&str, &str); 17] = [
    ("media_movie", "film demandé disponible"),
    ("media_season", "saison demandée disponible"),
    ("media_seasons", "plusieurs saisons demandées disponibles"),
    ("media_digest", "plusieurs demandes disponibles ensemble"),
    ("welcome", "bienvenue, compte à activer"),
    ("demo", "bienvenue, compte déjà actif (nouveau lien)"),
    ("activated", "compte activé, mot de passe déjà choisi"),
    ("activated_nopass", "compte activé, mot de passe à choisir"),
    ("sub_reminder", "rappel : fin d'essai"),
    ("sub_reminder_7d", "rappel : abonnement arrêté, fin proche"),
    ("sub_renewal_notice", "renouvellement automatique à venir"),
    ("sub_suspended", "accès en pause, abonnement terminé"),
    ("sub_suspended_trial", "accès en pause, essai terminé"),
    ("sub_payment_failed", "prélèvement PayPal refusé"),
    ("sub_activated", "abonnement enregistré"),
    ("chat_direct", "message de l'admin"),
    ("chat_announcement", "annonce aux membres"),
];

/// Types qui portent un lien de bienvenue.
const WITH_LINK: [&str; 3] = ["welcome", "demo", "activated_nopass"];

pub fn ids() -> Vec<&'static str> {
    KINDS.iter().map(|(id, _)| *id).collect()
}

pub fn label(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(id, _)| *id == kind)
        .map(|(_, l)| *l)
        .unwrap_or("")
}

fn is_media(kind: &str) -> bool {
    kind.starts_with("media_")
}

/// Types demandés, dans l'ordre du catalogue et sans doublon : tous (`all`), ou ceux de `kinds` (vide = aucun).
pub fn select(all: bool, kinds: &[String]) -> Result<Vec<&'static str>> {
    if all {
        return Ok(ids());
    }
    let unknown: Vec<&str> = kinds
        .iter()
        .map(|k| k.trim())
        .filter(|k| !KINDS.iter().any(|(id, _)| id == k))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "type de mail inconnu : {} (types : {})",
            unknown.join(", "),
            ids().join(", ")
        );
    }
    Ok(ids()
        .into_iter()
        .filter(|id| kinds.iter().any(|k| k.trim() == *id))
        .collect())
}

/// Un des types a besoin d'un lien de bienvenue.
pub fn needs_link(kinds: &[&str]) -> bool {
    kinds.iter().any(|k| WITH_LINK.contains(k))
}

/// Valeurs communes des exemples : liens publics, réglages, date ; rien de secret.
#[derive(Debug, Clone)]
pub struct Env {
    /// `JELLYFIN_PUBLIC_URL`.
    pub jellyfin_url: String,
    /// Lien de bienvenue : de démonstration (homelabd) ou factice (`--dump`).
    pub link: String,
    /// Hôte de la page de bienvenue (premiers pas).
    pub onboard_base: String,
    pub ttl_mins: u64,
    /// `/premium?compte=Exemple&k=…`, comme le cycle d'abonnement le construit.
    pub pay_url: String,
    pub price_text: String,
    pub grace_days: i64,
    pub period_days: i64,
    /// Paliers de rappel : le plus lointain (J-7) et le plus proche (J-1).
    pub remind_far: i64,
    pub remind_near: i64,
    pub now: i64,
}

impl Env {
    /// Depuis la configuration ; `link` = `None` : lien factice (rien d'émis, rien d'écrit).
    pub fn from_ctx(ctx: &TaskContext, link: Option<String>) -> Self {
        let base = welcome::link_base(ctx);
        let s = &ctx.cfg.subscriptions;
        let far = s.remind_days.iter().copied().max().unwrap_or(7);
        let near = s.remind_days.iter().copied().min().unwrap_or(1);
        Self {
            jellyfin_url: ctx.secrets.jellyfin_public_url.clone(),
            link: link.unwrap_or_else(|| welcome::url(&base, DUMP_TOKEN)),
            onboard_base: base,
            ttl_mins: ctx.cfg.onboard.link_ttl_mins,
            pay_url: subscription_ops::pay_url(ctx, NAME),
            price_text: s.price_text.clone(),
            grace_days: i64::from(s.grace_days),
            period_days: i64::from(s.period_days.max(1)),
            remind_far: i64::from(far),
            remind_near: i64::from(near),
            now: now(),
        }
    }
}

/// Exemple d'un mail sans titre de la bibliothèque (tout sauf `media_*`), objet non préfixé. Fonction pure.
pub fn fixed(kind: &str, e: &Env) -> Option<RenderedMail> {
    let t = e.now;
    let steps = welcome::steps_url(&e.onboard_base);
    let reminder = |days: i64, status: Status, stopped: bool| {
        subs::reminder_mail(&Reminder {
            username: NAME,
            days,
            status,
            ends_at: t + days * DAY,
            renewal_stopped: stopped,
            price_text: &e.price_text,
            pay_url: &e.pay_url,
        })
    };
    Some(match kind {
        "welcome" => welcome::render(welcome::KIND_WELCOME, NAME, &e.link, e.ttl_mins, false),
        "demo" => welcome::render(welcome::KIND_DEMO, NAME, &e.link, e.ttl_mins, true),
        "activated" => {
            welcome::activated_mail(NAME, &steps, ActivatedButton::Jellyfin(&e.jellyfin_url))
        }
        "activated_nopass" => {
            welcome::render(welcome::KIND_ACTIVATED, NAME, &e.link, e.ttl_mins, true)
        }
        "sub_reminder" => reminder(e.remind_near, Status::Trial, false),
        "sub_reminder_7d" => reminder(e.remind_far, Status::Active, true),
        "sub_renewal_notice" => subs::renewal_mail(NAME, t + e.remind_far * DAY),
        "sub_suspended" | "sub_suspended_trial" => subs::suspended_mail(
            NAME,
            Some(t - e.grace_days * DAY),
            kind == "sub_suspended_trial",
            &e.price_text,
            &e.pay_url,
        ),
        "sub_payment_failed" => subs::payment_failed_mail(NAME),
        "sub_activated" => subs::activated_mail(NAME, t + e.period_days * DAY, &e.jellyfin_url),
        "chat_direct" => chat::direct_mail(CHAT_AUTHOR, NAME, DIRECT_TEXT, &e.jellyfin_url),
        "chat_announcement" => chat::announcement_mail(CHAT_AUTHOR, ANNOUNCE_TEXT, &e.jellyfin_url),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Mails « disponible »
// ---------------------------------------------------------------------------------------------

/// Titres d'exemple, sans image (Jellyseerr injoignable, ou rien de tel dans la bibliothèque) : le bouton mène à
/// l'accueil Jellyfin.
fn sample_title(which: &str, home: &str, t: i64) -> Title {
    let base = Title {
        kind: TitleKind::Movie,
        title: String::new(),
        original: None,
        years: None,
        seasons: Vec::new(),
        whole_series: false,
        episodes: None,
        runtime_mins: None,
        rating: None,
        lang: Some("VF · VO".into()),
        genres: Vec::new(),
        synopsis: String::new(),
        poster: None,
        backdrop: None,
        watch_url: format!("{}/web/#/home", home.trim_end_matches('/')),
        requested_at: Some(t - 2 * DAY),
    };
    let genres = |g: &[&str]| g.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match which {
        "dexter" => Title {
            kind: TitleKind::Series,
            title: "Dexter".into(),
            years: Some("2006–2013".into()),
            seasons: vec![8],
            episodes: Some(12),
            rating: Some(8.2),
            genres: genres(&["Crime", "Drame", "Mystère"]),
            synopsis: "Dernière saison : Dexter essaie de tenir ensemble sa vie de famille et ses secrets, pendant qu'une psychiatre semble en savoir beaucoup sur lui.".into(),
            ..base
        },
        "xfiles" => Title {
            kind: TitleKind::Series,
            title: "X-Files : Aux frontières du réel".into(),
            original: Some("The X-Files".into()),
            years: Some("1993–2018".into()),
            seasons: (1..=10).collect(),
            episodes: Some(218),
            rating: Some(8.4),
            genres: genres(&["Mystère", "Science-Fiction & Fantastique", "Crime"]),
            synopsis: "Deux agents du FBI enquêtent sur des affaires inexpliquées, entre phénomènes paranormaux et secrets d'État.".into(),
            ..base
        },
        "basterds" => Title {
            title: "Inglourious Basterds".into(),
            years: Some("2009".into()),
            runtime_mins: Some(153),
            rating: Some(8.2),
            genres: genres(&["Drame", "Thriller", "Guerre"]),
            synopsis: "Dans la France occupée, un commando de soldats américains et une jeune femme préparent chacun de leur côté un coup d'éclat contre l'état-major nazi.".into(),
            ..base
        },
        _ => Title {
            title: "Le Menu".into(),
            original: Some("The Menu".into()),
            years: Some("2022".into()),
            runtime_mins: Some(106),
            rating: Some(7.2),
            genres: genres(&["Comédie", "Horreur"]),
            synopsis: "Un jeune couple rejoint une île isolée pour dîner dans un restaurant très couru. Le chef y a préparé un menu raffiné, et quelques surprises.".into(),
            ..base
        },
    }
}

/// Exemple « disponible » avec des titres d'exemple, sans image. Fonction pure.
pub fn media_fallback(kind: &str, home: &str, t: i64) -> Option<RenderedMail> {
    let one = |hero: Title, others: Vec<Title>| {
        available_mail::render(&AvailableMail {
            name: NAME.into(),
            hero,
            others,
            home_url: home.to_string(),
            at: t,
        })
    };
    let title = |w: &str| sample_title(w, home, t);
    Some(match kind {
        "media_movie" => one(title("menu"), vec![]),
        "media_season" => one(title("dexter"), vec![]),
        "media_seasons" => one(title("xfiles"), vec![]),
        "media_digest" => one(
            title("menu"),
            vec![title("dexter"), title("xfiles"), title("basterds")],
        ),
        _ => return None,
    })
}

/// Clés des vrais exemples « disponible » (la plus récente arrivée d'abord) : dernier film ; dernière saison (une
/// saison 2 ou plus de préférence, pour montrer « la saison N ») ; dernière série dont au moins deux saisons sont
/// arrivées ; les `1 + POSTER_SLOTS` derniers titres distincts pour le récapitulatif. Fonction pure.
pub fn pick_media(ready: &[Ready]) -> Vec<(&'static str, Vec<String>)> {
    let mut r: Vec<&Ready> = ready.iter().collect();
    r.sort_by(|a, b| b.ready_at.cmp(&a.ready_at).then_with(|| a.key.cmp(&b.key)));
    let mut out = Vec::new();
    if let Some(m) = r.iter().find(|x| x.kind == "movie") {
        out.push(("media_movie", vec![m.key.clone()]));
    }
    let tv: Vec<&Ready> = r.iter().copied().filter(|x| x.kind == "tv").collect();
    if let Some(s) = tv
        .iter()
        .find(|x| x.season.unwrap_or(0) >= 2)
        .or_else(|| tv.first())
    {
        out.push(("media_season", vec![s.key.clone()]));
    }
    // saisons distinctes par série, dans l'ordre de la plus récente arrivée
    let mut series: Vec<(i64, Vec<&Ready>)> = Vec::new();
    for &x in &tv {
        match series.iter_mut().find(|(tmdb, _)| *tmdb == x.tmdb) {
            Some((_, v)) => {
                if !v.iter().any(|y| y.season == x.season) {
                    v.push(x);
                }
            }
            None => series.push((x.tmdb, vec![x])),
        }
    }
    if let Some((_, v)) = series.iter().find(|(_, v)| v.len() >= 2) {
        let mut v = v.clone();
        v.sort_by_key(|x| x.season);
        out.push(("media_seasons", v.iter().map(|x| x.key.clone()).collect()));
    }
    let mut titles: Vec<(&str, i64)> = Vec::new();
    let mut keys = Vec::new();
    for x in &r {
        if titles.len() > POSTER_SLOTS {
            break;
        }
        if !titles.contains(&(x.kind, x.tmdb)) {
            titles.push((x.kind, x.tmdb));
            keys.push(x.key.clone());
        }
    }
    if keys.len() >= 2 {
        out.push(("media_digest", keys));
    }
    out
}

/// Compte Jellyfin actif qui voit toutes les bibliothèques (id compact) : les exemples relisent les titres comme
/// lui, pour n'en écarter aucun. Fonction pure.
pub fn viewer_of(users: &[Value]) -> Option<String> {
    users
        .iter()
        .filter(|u| crate::accounts::is_premium(u))
        .find(|u| {
            u.pointer("/Policy/EnableAllFolders")
                .and_then(Value::as_bool)
                == Some(true)
        })
        .and_then(|u| u.get("Id").and_then(Value::as_str))
        .map(media_mail::compact_id)
}

/// Vrais exemples « disponible » : rendus par `media_mail::compose` (lectures seulement), images jointes.
async fn real_media(
    ctx: &TaskContext,
    kinds: &[&'static str],
    t: i64,
) -> Result<HashMap<&'static str, RenderedMail>> {
    ctx.jellyseerr
        .status()
        .await
        .context("Jellyseerr injoignable")?;
    let media_mail::Snapshot {
        requests, ready, ..
    } = media_mail::snapshot(ctx).await?;
    let by_id: HashMap<i64, &Value> = requests
        .iter()
        .filter_map(|r| Some((r.get("id")?.as_i64()?, r)))
        .collect();
    let records: BTreeMap<String, MediaMailRecord> =
        media_mail::baseline(&ready, t).into_iter().collect();
    let viewer = match ctx.jellyfin.users().await {
        Ok(u) => viewer_of(&u),
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                "mail-test : comptes Jellyfin illisibles"
            );
            None
        }
    };
    let who = Recipient {
        seerr_id: 0,
        email: None,
        name: NAME.into(),
        jellyfin_id: None,
    };
    let mut out = HashMap::new();
    for (kind, keys) in pick_media(&ready) {
        if !kinds.contains(&kind) {
            continue;
        }
        let (items, _) = media_mail::plan_items(&keys, &records, &by_id);
        // sans compte « toutes bibliothèques » : le demandeur du titre mis en avant
        let user = viewer.clone().or_else(|| {
            items
                .first()
                .and_then(|p| by_id.get(&p.request))
                .and_then(|r| media_mail::recipient_of(r))
                .and_then(|r| r.jellyfin_id)
        });
        let Some(user) = user else {
            continue;
        };
        match media_mail::compose(ctx, &who, &user, &items, t, true).await {
            Ok(c) => {
                if let Some(m) = c.mail {
                    out.insert(kind, m);
                }
            }
            Err(e) => warn!(
                kind,
                error = format!("{e:#}"),
                "mail-test : exemple non composé"
            ),
        }
    }
    Ok(out)
}

/// Un exemple prêt à partir ou à écrire.
#[derive(Debug, Clone)]
pub struct Sample {
    pub kind: &'static str,
    /// Objet déjà préfixé `[Exemple] `.
    pub mail: RenderedMail,
    pub reply_to: Option<String>,
    pub headers: Vec<(String, String)>,
    /// D'où viennent les données : `exemple`, `vrai titre`, ou pourquoi un titre d'exemple remplace le vrai.
    pub origin: String,
}

/// Mêmes en-têtes que le vrai envoi : `Reply-To` des mails de bienvenue (gardé par `mail::build_message` seulement
/// sur le domaine d'envoi), `List-Unsubscribe` des « disponible ».
fn envelope(kind: &str, ctx: &TaskContext) -> (Option<String>, Vec<(String, String)>) {
    if matches!(kind, "welcome" | "demo" | "activated" | "activated_nopass") {
        (ctx.secrets.guide_contact_email.clone(), Vec::new())
    } else if is_media(kind) {
        let h = ctx
            .secrets
            .smtp
            .as_ref()
            .map(|s| {
                vec![(
                    "List-Unsubscribe".to_string(),
                    media_mail::list_unsubscribe(&s.from),
                )]
            })
            .unwrap_or_default();
        (None, h)
    } else {
        (None, Vec::new())
    }
}

/// Rend les exemples demandés. `link` : lien de bienvenue émis (homelabd), `None` = lien factice.
pub async fn build(ctx: &TaskContext, kinds: &[&'static str], link: Option<String>) -> Vec<Sample> {
    let env = Env::from_ctx(ctx, link);
    let media: Vec<&'static str> = kinds.iter().copied().filter(|k| is_media(k)).collect();
    let (real, why) = if media.is_empty() {
        (HashMap::new(), String::new())
    } else {
        match real_media(ctx, &media, env.now).await {
            Ok(m) => (m, "aucun titre de ce genre lisible".to_string()),
            Err(e) => {
                warn!(
                    error = format!("{e:#}"),
                    "mail-test : titres d'exemple sans image"
                );
                (HashMap::new(), "Jellyseerr injoignable".to_string())
            }
        }
    };
    let mut out = Vec::new();
    for &kind in kinds {
        let (mail, origin) = match real.get(kind) {
            Some(m) => (Some(m.clone()), "vrai titre".to_string()),
            None if is_media(kind) => (
                media_fallback(kind, &env.jellyfin_url, env.now),
                format!("titre d'exemple sans image : {why}"),
            ),
            None => (fixed(kind, &env), "exemple".to_string()),
        };
        let Some(mut mail) = mail else {
            continue;
        };
        mail.subject = format!("{SUBJECT_PREFIX}{}", mail.subject);
        let (reply_to, headers) = envelope(kind, ctx);
        out.push(Sample {
            kind,
            mail,
            reply_to,
            headers,
            origin,
        });
    }
    out
}

/// Type (noms des fichiers de `--dump`) de l'aperçu d'une annonce.
pub const ANNOUNCE_KIND: &str = "announce";

/// Aperçu d'une annonce, partie pure : texte nettoyé par [`chat::announcement_text`] (comme
/// `POST /admin/chat/announce`), rendu par [`chat::announcement_mail`] (comme l'envoi aux membres), objet préfixé
/// [`chat::PREVIEW_PREFIX`] ; ni `Reply-To` ni autre en-tête, comme le vrai envoi. Erreur : « message vide ».
pub fn announcement_sample(
    author_name: &str,
    raw: &str,
    jellyfin_url: &str,
    max_chars: usize,
) -> std::result::Result<Sample, &'static str> {
    let text = chat::announcement_text(raw)?;
    let mut mail = chat::announcement_mail(author_name, &text, jellyfin_url);
    mail.subject = format!("{}{}", chat::PREVIEW_PREFIX, mail.subject);
    Ok(Sample {
        kind: ANNOUNCE_KIND,
        mail,
        reply_to: None,
        headers: Vec::new(),
        origin: format!(
            "annonce de {author_name}, {} message(s) dans le tchat, {} caractères",
            chat::split_parts(&text, max_chars).len(),
            text.chars().count()
        ),
    })
}

/// Aperçu d'une annonce au nom de `author` (modérateur, compte Jellyfin relu : lecture seule), pour `--dump`,
/// `--dry-run` et homelabd. Aucun auteur deviné : Jellyfin injoignable = erreur, comme le vrai envoi.
pub async fn announcement(ctx: &TaskContext, author: &str, raw: &str) -> Result<Sample> {
    let u = chat::announcement_author(ctx, author)
        .await
        .map_err(|e| match e {
            chat::AuthorError::Jellyfin(cause) => cause.context("Jellyfin injoignable"),
            other => anyhow::anyhow!("{other} : {author}"),
        })?;
    announcement_sample(
        &u.name,
        raw,
        &ctx.secrets.jellyfin_public_url,
        ctx.cfg.chat.max_chars,
    )
    .map_err(|e| anyhow::anyhow!("annonce : {e}"))
}

fn outgoing<'a>(to: &'a str, s: &'a Sample, headers: &'a [(&'a str, &'a str)]) -> Outgoing<'a> {
    Outgoing {
        reply_to: s.reply_to.as_deref(),
        headers,
        ..Outgoing::rendered(NAME, to, &s.mail)
    }
}

fn header_refs(s: &Sample) -> Vec<(&str, &str)> {
    s.headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect()
}

/// Résultat d'un envoi : `error` = `None` si parti.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Sent {
    pub kind: &'static str,
    pub subject: String,
    pub origin: String,
    pub sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Envoie les exemples à `to`, `gap` entre deux envois ; le journal ne porte jamais l'adresse.
pub async fn send(smtp: &Smtp, to: &str, samples: &[Sample], gap: Duration) -> Vec<Sent> {
    let mut out = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(gap).await;
        }
        let headers = header_refs(s);
        let r = mail::send(smtp, &outgoing(to, s, &headers)).await;
        let error = r.err().map(|e| {
            // curl ne cite pas le mot de passe ; on s'en assure quand même
            let msg = format!("{e:#}");
            let pass = smtp.pass.expose();
            if pass.is_empty() {
                msg
            } else {
                msg.replace(pass, "***")
            }
        });
        info!(
            task = "onboard",
            kind = s.kind,
            sent = error.is_none(),
            "mail-test : exemple"
        );
        out.push(Sent {
            kind: s.kind,
            subject: s.mail.subject.clone(),
            origin: s.origin.clone(),
            sent: error.is_none(),
            error,
        });
    }
    out
}

/// Expéditeur des `.eml` de `--dump` quand `.env` n'a pas de SMTP (rien n'est envoyé).
pub fn placeholder_smtp() -> Smtp {
    Smtp {
        host: "smtp.example.test".into(),
        port: 465,
        user: String::new(),
        pass: Secret::new(""),
        from: "groscailloux@example.test".into(),
        from_name: "Groscailloux TV".into(),
    }
}

/// Fichier local d'une image jointe : `img/<nom>-<12 hex>.<ext>` (même image = même fichier).
pub fn image_path(img: &mail::InlineImage) -> String {
    let stem = img
        .cid
        .split('@')
        .next()
        .unwrap_or("image")
        .replace('.', "-");
    let ext = if img.content_type == "image/png" {
        "png"
    } else {
        "jpg"
    };
    format!("img/{stem}.{ext}")
}

/// HTML lisible dans un navigateur : chaque `cid:` remplacé par le fichier de l'image.
pub fn local_html(m: &RenderedMail) -> String {
    let mut html = m.html.clone();
    for img in &m.images {
        html = html.replace(&format!("cid:{}", img.cid), &image_path(img));
    }
    html
}

/// Ce que `dump` a écrit pour un exemple.
#[derive(Debug, Clone)]
pub struct Dumped {
    pub kind: &'static str,
    pub subject: String,
    pub origin: String,
    pub html: PathBuf,
    pub html_bytes: usize,
    pub eml_bytes: usize,
    pub images: usize,
}

/// Écrit `<dir>/<type>.html` (images dans `<dir>/img/`), `<dir>/<type>.eml` (message complet, `To` = `to`) et
/// `<dir>/<type>.txt` (version texte) ; rien n'est envoyé.
pub fn dump(dir: &Path, smtp: &Smtp, to: &str, samples: &[Sample]) -> Result<Vec<Dumped>> {
    let img_dir = dir.join("img");
    std::fs::create_dir_all(&img_dir)
        .with_context(|| format!("création de {}", img_dir.display()))?;
    let mut out = Vec::new();
    for s in samples {
        for img in &s.mail.images {
            let p = dir.join(image_path(img));
            std::fs::write(&p, &img.bytes[..])
                .with_context(|| format!("écriture de {}", p.display()))?;
        }
        let html = local_html(&s.mail);
        let headers = header_refs(s);
        let eml = mail::message(smtp, &outgoing(to, s, &headers));
        let html_path = dir.join(format!("{}.html", s.kind));
        for (p, body) in [
            (html_path.clone(), html.as_str()),
            (dir.join(format!("{}.eml", s.kind)), eml.as_str()),
            (dir.join(format!("{}.txt", s.kind)), s.mail.text.as_str()),
        ] {
            std::fs::write(&p, body).with_context(|| format!("écriture de {}", p.display()))?;
        }
        out.push(Dumped {
            kind: s.kind,
            subject: s.mail.subject.clone(),
            origin: s.origin.clone(),
            html: html_path,
            html_bytes: s.mail.html.len(),
            eml_bytes: eml.len(),
            images: s.mail.images.len(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_layout::checks::assert_member_mail;
    use serde_json::json;

    fn env() -> Env {
        Env {
            jellyfin_url: "https://tv.example.test".into(),
            link:
                "https://compte.example.test/bienvenue/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    .into(),
            onboard_base: "https://compte.example.test".into(),
            ttl_mins: 60,
            pay_url: "https://compte.example.test/premium?compte=Exemple&k=0123456789abcdef".into(),
            price_text: "3,50 € / mois".into(),
            grace_days: 3,
            period_days: 30,
            remind_far: 7,
            remind_near: 1,
            // 2026-10-15 12:00 UTC
            now: 1_792_065_600,
        }
    }

    fn rendered(kind: &str) -> RenderedMail {
        let e = env();
        fixed(kind, &e)
            .or_else(|| media_fallback(kind, &e.jellyfin_url, e.now))
            .unwrap_or_else(|| panic!("{kind} sans exemple"))
    }

    #[test]
    fn every_kind_has_a_sample_rendered_by_the_real_function() {
        let mut subjects = Vec::new();
        for (kind, label) in KINDS {
            assert!(!label.is_empty());
            let r = rendered(kind);
            assert_member_mail(&r);
            // la parade au mode sombre de Gmail coûte 2 à 3 Ko : loin du budget, borne de non-régression
            assert!(r.html.len() < 20_000, "{kind} : {} octets", r.html.len());
            assert!(
                !r.text.contains("Nina") && !r.html.contains("Nina"),
                "{kind} : compte fictif seulement"
            );
            subjects.push(r.subject);
        }
        let mut uniq = subjects.clone();
        uniq.sort();
        uniq.dedup();
        // bienvenue et démo, actif avec ou sans mot de passe : même objet que dans la vraie vie
        assert_eq!(uniq.len(), KINDS.len() - 2, "{subjects:?}");
        assert!(fixed("media_movie", &env()).is_none() && fixed("autre", &env()).is_none());
        assert!(media_fallback("welcome", "https://tv.example.test", 0).is_none());
    }

    #[test]
    fn samples_follow_the_real_rules() {
        let e = env();
        // lien de bienvenue : 3 fois dans le HTML là où il existe, jamais dans « actif » avec mot de passe
        for k in WITH_LINK {
            assert_eq!(rendered(k).html.matches(&e.link).count(), 3, "{k}");
        }
        assert!(!rendered("activated").html.contains("/bienvenue/"));
        // jamais de lien de paiement dans le renouvellement automatique ni le prélèvement refusé
        for k in ["sub_renewal_notice", "sub_payment_failed"] {
            let r = rendered(k);
            assert!(
                !r.html.contains("<a ") && !r.text.contains("/premium"),
                "{k}"
            );
        }
        for k in [
            "sub_reminder",
            "sub_reminder_7d",
            "sub_suspended",
            "sub_suspended_trial",
        ] {
            assert!(rendered(k).text.contains(&e.pay_url), "{k}");
        }
        assert!(rendered("sub_reminder").subject.contains("essai"));
        assert!(rendered("sub_suspended_trial").subject.contains("essai"));
        assert!(rendered("sub_reminder_7d").subject.contains("7 jours"));
        assert!(rendered("chat_direct").text.starts_with("Salut Exemple,"));
        assert_eq!(
            rendered("chat_announcement").subject,
            "Maintenance samedi matin"
        );
        // titres d'exemple : sans image ni crédit TMDB, bouton vers l'accueil
        let m = rendered("media_digest");
        assert!(
            m.images.iter().all(|i| i.content_type == "image/png"),
            "logo seul"
        );
        assert!(!m.text.contains("Visuels : TMDB"));
        assert!(m.text.contains("https://tv.example.test/web/#/home"));
        assert!(rendered("media_seasons").subject.contains("saisons 1 à 10"));
        assert!(rendered("media_season").subject.contains("saison 8"));
    }

    #[test]
    fn kinds_are_selected_in_catalog_order() {
        assert_eq!(select(true, &[]).unwrap().len(), KINDS.len());
        assert!(select(false, &[]).unwrap().is_empty());
        assert_eq!(
            select(false, &["demo".into(), "media_movie".into(), "demo".into()]).unwrap(),
            vec!["media_movie", "demo"]
        );
        let e = select(false, &["demo".into(), "spam".into()]).unwrap_err();
        assert!(e.to_string().contains("spam"));
        assert!(needs_link(&["sub_reminder", "demo"]) && !needs_link(&["media_movie"]));
        assert_eq!(label("chat_direct"), "message de l'admin");
    }

    fn ready(key: &str, kind: &'static str, tmdb: i64, season: Option<u32>, at: i64) -> Ready {
        Ready {
            key: key.into(),
            request: key[1..].split(':').next().unwrap().parse().unwrap(),
            kind,
            tmdb,
            season,
            ready_at: at,
            user: 1,
            seerr_announced: true,
        }
    }

    #[test]
    fn media_samples_take_the_latest_titles() {
        let r = vec![
            ready("r1", "movie", 100, None, 10),
            ready("r2", "movie", 101, None, 50),
            ready("r3:s1", "tv", 200, Some(1), 60),
            ready("r4:s1", "tv", 300, Some(1), 40),
            ready("r4:s2", "tv", 300, Some(2), 45),
            ready("r5:s2", "tv", 300, Some(2), 30),
            ready("r6", "movie", 102, None, 20),
        ];
        let p: HashMap<&str, Vec<String>> = pick_media(&r).into_iter().collect();
        assert_eq!(p["media_movie"], ["r2"]);
        // la saison 2 la plus récente plutôt que la saison 1 d'une série plus récente
        assert_eq!(p["media_season"], ["r4:s2"]);
        // saisons distinctes d'une même série, dans l'ordre
        assert_eq!(p["media_seasons"], ["r4:s1", "r4:s2"]);
        // 4 titres distincts, le plus récent d'abord
        assert_eq!(p["media_digest"], ["r3:s1", "r2", "r4:s2", "r6"]);
        assert!(pick_media(&[]).is_empty());
        let only = pick_media(&[ready("r1", "movie", 1, None, 1)]);
        assert_eq!(only, vec![("media_movie", vec!["r1".to_string()])]);
    }

    #[test]
    fn titles_are_read_by_an_account_that_sees_every_library() {
        let users = vec![
            json!({"Id": "AAAA-1", "Policy": {"EnableAllFolders": false, "IsDisabled": false}}),
            json!({"Id": "BBBB-2", "Policy": {"EnableAllFolders": true, "IsDisabled": true}}),
            json!({"Id": "CCCC-3", "Policy": {"EnableAllFolders": true, "IsDisabled": false}}),
        ];
        assert_eq!(viewer_of(&users).as_deref(), Some("cccc3"));
        assert_eq!(viewer_of(&users[..2]), None);
    }

    #[test]
    fn dump_writes_a_viewable_page_the_full_message_and_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut mail = rendered("sub_activated");
        mail.subject = format!("{SUBJECT_PREFIX}{}", mail.subject);
        let s = Sample {
            kind: "sub_activated",
            mail,
            reply_to: Some("contact@example.test".into()),
            headers: vec![("List-Unsubscribe".into(), "<mailto:x@example.test>".into())],
            origin: "exemple".into(),
        };
        let out = dump(
            dir.path(),
            &placeholder_smtp(),
            DUMP_TO,
            std::slice::from_ref(&s),
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].images, s.mail.images.len());
        assert!(out[0].images >= 2, "logo et bandeau");
        let html = std::fs::read_to_string(dir.path().join("sub_activated.html")).unwrap();
        assert!(!html.contains("cid:"));
        for img in &s.mail.images {
            let p = image_path(img);
            assert!(html.contains(&format!("src=\"{p}\"")), "{p}");
            assert_eq!(std::fs::read(dir.path().join(&p)).unwrap(), &img.bytes[..]);
        }
        let eml = std::fs::read_to_string(dir.path().join("sub_activated.eml")).unwrap();
        assert_eq!(eml.len(), out[0].eml_bytes);
        assert!(eml.contains("To: Exemple <exemple@example.test>\r\n"));
        assert!(eml.contains("Reply-To: contact@example.test\r\n"));
        assert!(eml.contains("List-Unsubscribe: <mailto:x@example.test>\r\n"));
        assert!(eml.contains("Subject: [Exemple] Ton compte Premium est actif\r\n"));
        assert!(eml.contains("multipart/related"));
        for img in &s.mail.images {
            assert!(eml.contains(&format!("Content-ID: <{}>", img.cid)));
        }
        let txt = std::fs::read_to_string(dir.path().join("sub_activated.txt")).unwrap();
        assert_eq!(txt, s.mail.text);
    }

    /// L'aperçu est le vrai mail de l'annonce (même nettoyage, même fonction), seul l'objet est préfixé.
    #[test]
    fn preview_is_the_real_announcement_with_a_prefixed_subject() {
        let jf = "https://tv.example.test";
        let raw = "Nouvelle adresse, rien à changer  \r\n\r\nTout est dans le guide : https://o.example.test/guide.   \r\n\r\n\r\n\r\n\r\nBon visionnage !\r\n";
        let s = announcement_sample("Admin", raw, jf, 2000).unwrap();
        let real =
            chat::announcement_mail("Admin", &chat::validate_body(raw, usize::MAX).unwrap(), jf);
        assert_eq!(s.mail.subject, format!("[Aperçu] {}", real.subject));
        assert_eq!(s.mail.subject, "[Aperçu] Nouvelle adresse, rien à changer");
        assert_eq!(s.mail.html, real.html);
        assert_eq!(s.mail.text, real.text);
        assert_eq!(s.mail.preheader, real.preheader);
        assert_eq!(s.mail.images, real.images);
        assert_member_mail(&s.mail);
        assert!(s.mail.html.contains(">https://o.example.test/guide</a>."));
        assert_eq!(
            announcement_sample("Admin", " \r\n \n", jf, 2000).unwrap_err(),
            "message vide"
        );
    }

    /// Même enveloppe que l'envoi aux membres (`mail_members` : ni `Reply-To` ni autre en-tête) ; l'origine dit en
    /// combien de messages le tchat la découpera.
    #[test]
    fn announcement_sample_has_the_member_envelope() {
        let s = announcement_sample(
            "Admin",
            "Titre\n\nTexte court.",
            "https://tv.example.test",
            2000,
        )
        .unwrap();
        assert_eq!(s.kind, ANNOUNCE_KIND);
        assert!(s.reply_to.is_none() && s.headers.is_empty());
        assert_eq!(
            s.origin,
            "annonce de Admin, 1 message(s) dans le tchat, 19 caractères"
        );
        let long = format!("Titre\n\n{}\n\n{}", "a ".repeat(700), "b ".repeat(600));
        let s = announcement_sample("Admin", &long, "https://tv.example.test", 2000).unwrap();
        assert!(
            s.origin.contains(", 2 message(s) dans le tchat, "),
            "{}",
            s.origin
        );
    }

    #[test]
    fn dump_writes_the_announcement_preview_without_reply_to() {
        let dir = tempfile::tempdir().unwrap();
        let s = announcement_sample(
            "Admin",
            "Nouvelle adresse\n\nLe guide : https://o.example.test/guide",
            "https://tv.example.test",
            2000,
        )
        .unwrap();
        let out = dump(dir.path(), &placeholder_smtp(), "a@example.test", &[s]).unwrap();
        assert_eq!(out[0].kind, "announce");
        for f in ["announce.html", "announce.eml", "announce.txt"] {
            assert!(dir.path().join(f).exists(), "{f}");
        }
        let eml = std::fs::read_to_string(dir.path().join("announce.eml")).unwrap();
        assert!(eml.contains("To: Exemple <a@example.test>\r\n"));
        assert!(!eml.contains("Reply-To:") && !eml.contains("List-Unsubscribe:"));
        assert!(eml.contains(&format!(
            "Subject: {}\r\n",
            mail::header_word("[Aperçu] Nouvelle adresse")
        )));
        let html = std::fs::read_to_string(dir.path().join("announce.html")).unwrap();
        assert!(html.contains(r#"href="https://o.example.test/guide""#) && !html.contains("cid:"));
    }
}
