//! Liens de bienvenue : le mail ne contient qu'un bouton vers `/bienvenue/<jeton>`, page où le membre définit son
//! mot de passe et retrouve ses accès (2026-09-19 : plus d'identifiant ni de mot de passe par mail, c'est ce qui
//! faisait finir le mail en spam et c'est plus sûr). Le jeton (32 octets aléatoires, base64url) n'est stocké
//! que **haché** (SHA-256) dans l'état ; il expire (`[onboard] link_ttl_mins`) et ne sert qu'une fois. Un jeton
//! `Demo` sert au mail de test : la page s'affiche en exemple, aucun compte n'est touché.
use anyhow::{bail, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::context::TaskContext;
use crate::mail_layout::{welcome_banner, Block, MailContent, Para, RenderedMail, Tone};
use crate::state::{now, WelcomeLink};

pub const KIND_WELCOME: &str = "welcome";
pub const KIND_ACTIVATED: &str = "activated";
pub const KIND_DEMO: &str = "demo";

/// Un lien gardé dans l'état une fois consommé ou expiré, avant purge (pour l'affichage « lien consommé »).
const KEEP_SECS: i64 = 7 * 24 * 3600;

/// 32 octets aléatoires en base64url (43 caractères, sans `=`).
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64url(&bytes)
}

pub fn hash(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Un jeton reçu par l'URL : forme attendue seulement (évite de hacher n'importe quoi).
pub fn well_formed(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Valide = ni consommé, ni expiré.
pub fn is_valid(link: &WelcomeLink, now: i64) -> bool {
    link.used_at.is_none() && link.expires_at > now
}

/// État d'un lien pour l'affichage admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkStatus {
    None,
    Pending { expires_at: i64 },
    Used { at: i64 },
    Expired { at: i64 },
}

/// Dernier lien émis pour un compte (hors démo) et son état.
pub fn status_for<'a>(
    links: impl IntoIterator<Item = &'a WelcomeLink>,
    user_id: &str,
    now: i64,
) -> LinkStatus {
    let last = links
        .into_iter()
        .filter(|l| l.user_id == user_id && l.kind != KIND_DEMO)
        .max_by_key(|l| l.created_at);
    match last {
        None => LinkStatus::None,
        Some(l) => match l.used_at {
            Some(at) => LinkStatus::Used { at },
            None if l.expires_at > now => LinkStatus::Pending {
                expires_at: l.expires_at,
            },
            None => LinkStatus::Expired { at: l.expires_at },
        },
    }
}

/// Émet un lien et renvoie le jeton en clair (à mettre dans l'URL, jamais journalisé).
pub async fn issue(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    email: &str,
    kind: &str,
) -> Result<String> {
    let token = new_token();
    let t = now();
    let link = WelcomeLink {
        user_id: user_id.to_string(),
        username: username.to_string(),
        email: email.to_string(),
        kind: kind.to_string(),
        created_at: t,
        expires_at: t + (ctx.cfg.onboard.link_ttl_mins as i64) * 60,
        used_at: None,
    };
    let key = hash(&token);
    ctx.state
        .update(|st| {
            // un seul lien actif par compte : les précédents sont invalidés
            for l in st.welcome_links.values_mut() {
                if l.user_id == link.user_id && l.used_at.is_none() && l.kind != KIND_DEMO {
                    l.expires_at = t;
                }
            }
            st.welcome_links.insert(key, link);
        })
        .await?;
    Ok(token)
}

/// Le lien s'il est valide.
pub async fn lookup(ctx: &TaskContext, token: &str) -> Option<WelcomeLink> {
    if !well_formed(token) {
        return None;
    }
    let key = hash(token);
    let t = now();
    ctx.state
        .read(|st| {
            st.welcome_links
                .get(&key)
                .filter(|l| is_valid(l, t))
                .cloned()
        })
        .await
}

/// Marque le lien consommé (le mot de passe est défini).
pub async fn consume(ctx: &TaskContext, token: &str) -> Result<()> {
    let key = hash(token);
    let t = now();
    let ok = ctx
        .state
        .update(|st| match st.welcome_links.get_mut(&key) {
            Some(l) if is_valid(l, t) => {
                l.used_at = Some(t);
                true
            }
            _ => false,
        })
        .await?;
    if !ok {
        bail!("lien invalide ou déjà utilisé");
    }
    Ok(())
}

/// URL publique de la page pour un jeton.
/// Hôte des pages de bienvenue : `ONBOARD_PUBLIC_URL`, sinon l'adresse d'écoute de homelabd.
pub fn link_base(ctx: &TaskContext) -> String {
    ctx.secrets
        .onboard_public_url
        .clone()
        .unwrap_or_else(|| format!("http://{}", ctx.cfg.web.listen))
}

pub fn url(base: &str, token: &str) -> String {
    format!("{}/bienvenue/{token}", base.trim_end_matches('/'))
}

/// Retire les liens consommés ou expirés depuis plus de `KEEP_SECS`.
pub fn purge(links: &mut std::collections::BTreeMap<String, WelcomeLink>, now: i64) -> usize {
    let before = links.len();
    links.retain(|_, l| {
        let ended = l.used_at.unwrap_or(l.expires_at);
        is_valid(l, now) || now - ended < KEEP_SECS
    });
    before - links.len()
}

/// Inscriptions publiques : garde les dates de moins de 24 h et dit si une nouvelle est admise.
pub fn signup_allowed(times: &mut Vec<i64>, now: i64, max_per_day: usize) -> bool {
    times.retain(|t| now - *t < 24 * 3600);
    if times.len() >= max_per_day {
        return false;
    }
    times.push(now);
    true
}

/// Renvois de lien pour une adresse : au plus `per_hour` par heure glissante.
pub fn renew_allowed(times: &mut Vec<i64>, now: i64, per_hour: usize) -> bool {
    times.retain(|t| now - *t < 3600);
    if times.len() >= per_hour {
        return false;
    }
    times.push(now);
    true
}

fn base64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn link(user: &str, kind: &str, created: i64, expires: i64, used: Option<i64>) -> WelcomeLink {
        WelcomeLink {
            user_id: user.into(),
            username: "u".into(),
            email: "u@example.test".into(),
            kind: kind.into(),
            created_at: created,
            expires_at: expires,
            used_at: used,
        }
    }

    #[test]
    fn tokens_are_well_formed_unique_and_hashed() {
        let a = new_token();
        let b = new_token();
        assert!(well_formed(&a) && well_formed(&b));
        assert_ne!(a, b);
        assert_eq!(hash(&a).len(), 64);
        assert_ne!(hash(&a), hash(&b));
        assert!(!well_formed("../etc/passwd"));
        assert!(!well_formed(&a[..42]));
    }

    #[test]
    fn validity_expiry_and_use() {
        let l = link("id", KIND_WELCOME, 100, 200, None);
        assert!(is_valid(&l, 150));
        assert!(!is_valid(&l, 200));
        let used = link("id", KIND_WELCOME, 100, 200, Some(120));
        assert!(!is_valid(&used, 150));
    }

    #[test]
    fn status_takes_the_latest_link_of_the_account() {
        let links = vec![
            link("a", KIND_WELCOME, 10, 20, Some(15)),
            link("a", KIND_ACTIVATED, 30, 40, None),
            link("b", KIND_WELCOME, 50, 60, None),
            link("a", KIND_DEMO, 99, 999, None),
        ];
        assert_eq!(
            status_for(&links, "a", 35),
            LinkStatus::Pending { expires_at: 40 }
        );
        assert_eq!(status_for(&links, "a", 45), LinkStatus::Expired { at: 40 });
        assert_eq!(status_for(&links, "c", 45), LinkStatus::None);
        let used = vec![link("a", KIND_WELCOME, 10, 20, Some(15))];
        assert_eq!(status_for(&used, "a", 30), LinkStatus::Used { at: 15 });
    }

    #[test]
    fn purge_keeps_valid_and_recent_links() {
        let mut m = BTreeMap::new();
        m.insert("valid".into(), link("a", KIND_WELCOME, 0, 1_000_000, None));
        m.insert("old-used".into(), link("a", KIND_WELCOME, 0, 10, Some(5)));
        m.insert(
            "recent-expired".into(),
            link("a", KIND_WELCOME, 0, 999_000, None),
        );
        assert_eq!(purge(&mut m, 1_000_000), 1);
        assert!(m.contains_key("valid") && m.contains_key("recent-expired"));
    }

    #[test]
    fn signup_and_renew_caps() {
        let mut t = vec![];
        for _ in 0..10 {
            assert!(signup_allowed(&mut t, 1000, 10));
        }
        assert!(!signup_allowed(&mut t, 1000, 10));
        assert!(signup_allowed(&mut t, 1000 + 24 * 3600, 10));
        let mut r = vec![];
        assert!(
            renew_allowed(&mut r, 0, 3)
                && renew_allowed(&mut r, 1, 3)
                && renew_allowed(&mut r, 2, 3)
        );
        assert!(!renew_allowed(&mut r, 3, 3));
        assert!(renew_allowed(&mut r, 3601, 3));
    }

    #[test]
    fn base64url_matches_reference() {
        assert_eq!(base64url(b"hello"), "aGVsbG8");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }
}

// ---------------------------------------------------------------------------------------------
// Mails de bienvenue / d'activation (gabarit sombre `mail_layout`, 2026-10-15) : un bouton vers la page du
// lien (ou vers Jellyfin), rien d'autre de sensible. Fonctions pures : le banc d'essai les rend sans rien émettre.
// ---------------------------------------------------------------------------------------------

const WELCOME_REASON: &str = "Tu reçois ce mail parce qu'un compte a été créé pour cette adresse sur Groscailloux TV, le service de streaming privé entre amis. Si ce n'est pas toi, ignore-le : personne ne pourra utiliser ce compte sans le lien.";
const ACTIVATED_REASON: &str = "Tu reçois ce mail parce que ton compte vient d'être activé sur Groscailloux TV, le service de streaming privé entre amis.";
const CONTACT_REPLY: &str = "Une question ? Réponds simplement à ce mail.";
/// « Sur quel écran ? » : quoi installer, sans identifiant ; le détail est sur `/premiers-pas`.
const SCREENS: [(&str, &str); 3] = [
    ("Télé", "L'appli « Jellyfin » du store de la télé (ou d'une clé Fire TV / Google TV). Connexion par un code « Quick Connect » validé depuis ton téléphone."),
    ("Téléphone", "« Jellyfin Mobile » sur iPhone, « Jellyfin » sur le Play Store."),
    ("Ordinateur", "Ton navigateur, ou l'appli « Jellyfin Desktop »."),
];

/// Encadré du mail de bienvenue, sous le lien.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeNotice {
    /// Compte déjà actif (démo, compte que la suspension n'a pas pu mettre en pause) : pas d'encadré.
    None,
    /// Compte créé suspendu : « sera activé par l'administrateur… tu recevras un mail ».
    Activation,
    /// Inscription publique dont l'essai gratuit de `days` jours a démarré (décidé avant l'envoi, 1.30.5).
    Trial { days: u32 },
}

/// Où mène le bouton du mail « ton compte est actif ».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivatedButton<'a> {
    /// Mot de passe déjà choisi (dernier lien consommé) : Jellyfin directement, aucun lien émis.
    Jellyfin(&'a str),
    /// Pas encore de mot de passe : nouveau lien de bienvenue, valable `ttl_mins` minutes.
    Link { url: &'a str, ttl_mins: u64 },
}

/// Page des premiers pas (`<base>/premiers-pas`), sur l'hôte de la page de bienvenue.
pub fn steps_url(base: &str) -> String {
    format!("{}/premiers-pas", base.trim_end_matches('/'))
}

fn validity(ttl_mins: u64) -> String {
    format!("Ce lien est valable {ttl_mins} minutes et ne sert qu'une fois. Passé ce délai, la page te proposera d'en recevoir un nouveau.")
}

/// « Sur quel écran ? », la carte des trois écrans et le lien vers les premiers pas.
fn push_screens(m: &mut MailContent, steps_url: &str) {
    m.push(Block::h2("Sur quel écran ?"))
        .push(Block::list_card(SCREENS))
        .glue()
        .push_with_text(
            Block::small([Para::new()
                .text("Tout est expliqué pas à pas : ")
                .link("les premiers pas", steps_url)
                .text(".")])
            .top(12),
            format!("Tout est expliqué pas à pas : {steps_url}"),
        )
        .glue();
}

/// Mail de bienvenue (et de démonstration) : un bouton vers la page où le membre choisit son mot de passe,
/// le lien de secours, l'encadré éventuel, les écrans. Le lien apparaît 3 fois dans le HTML (bouton, lien de
/// secours en `href` et en texte), une fois dans le texte.
pub fn welcome_mail(
    username: &str,
    link: &str,
    steps_url: &str,
    ttl_mins: u64,
    notice: WelcomeNotice,
) -> RenderedMail {
    let mut m = MailContent::new(
        "Bienvenue sur Groscailloux TV : définis ton mot de passe",
        format!("Choisis ton mot de passe pour ouvrir ton compte : le lien est valable {ttl_mins} minutes."),
    );
    m.push(Block::header(None))
        .push(Block::banner(welcome_banner()))
        .push(Block::intro("Ton compte est créé", format!("Bienvenue, {username} !")).top(8))
        .push(Block::text(["Il ne reste qu'à choisir ton mot de passe. Tu retrouveras ensuite tes accès sur la même page."]))
        .push_with_text(
            Block::button("Définir mon mot de passe", link).top(24),
            format!("Définir mon mot de passe : {link}"),
        )
        .push(Block::note(validity(ttl_mins)))
        .push(Block::fallback_link(link));
    match notice {
        WelcomeNotice::None => {}
        WelcomeNotice::Activation => {
            m.push(Block::notice(
                Tone::Info,
                "Activation",
                "Ton compte sera activé par l'administrateur sous peu. Tu recevras un mail à ce moment-là.",
            ));
        }
        WelcomeNotice::Trial { days } => {
            m.push(Block::notice(
                Tone::Success,
                format!("Ton essai de {days} jours a commencé"),
                "Dès que ton mot de passe est choisi, films et séries sont à toi.",
            ));
        }
    }
    push_screens(&mut m, steps_url);
    m.push(Block::footer(WELCOME_REASON, CONTACT_REPLY, None));
    m.render()
}

/// Mail « ton compte est actif » : bouton vers Jellyfin si le mot de passe est déjà choisi (logo cliquable,
/// ni validité ni lien de secours), sinon vers un nouveau lien de bienvenue (3 fois dans le HTML, comme la
/// bienvenue).
pub fn activated_mail(username: &str, steps_url: &str, button: ActivatedButton) -> RenderedMail {
    let mut m;
    match button {
        ActivatedButton::Jellyfin(home) => {
            m = MailContent::new(
                "Ton compte Groscailloux TV est actif",
                "Films et séries t'attendent : ouvre Groscailloux TV quand tu veux.",
            );
            m.push(Block::header(Some(home)))
                .push(Block::banner(welcome_banner()))
                .push(Block::intro("Compte activé", format!("C'est bon, {username} !")).top(8))
                .push(Block::text(["Ton compte vient d'être activé : tu peux regarder films et séries dès maintenant, avec ton mot de passe habituel."]))
                .push_with_text(
                    Block::button("Ouvrir Groscailloux TV", home).top(24),
                    format!("Ouvrir Groscailloux TV : {home}"),
                );
        }
        ActivatedButton::Link { url, ttl_mins } => {
            m = MailContent::new(
                "Ton compte Groscailloux TV est actif",
                "Tu peux regarder films et séries dès maintenant.",
            );
            m.push(Block::header(None))
                .push(Block::banner(welcome_banner()))
                .push(Block::intro("Compte activé", format!("C'est bon, {username} !")).top(8))
                .push(Block::text(["Ton compte vient d'être activé : tu peux regarder films et séries dès maintenant."]))
                .push_with_text(
                    Block::button("Ouvrir Groscailloux TV", url).top(24),
                    format!("Ouvrir Groscailloux TV : {url}"),
                )
                .push(Block::note(format!(
                    "Pas encore de mot de passe ? Le bouton t'amène d'abord sur la page pour le choisir. Sinon, connecte-toi avec ton mot de passe habituel. {}",
                    validity(ttl_mins)
                )))
                .push(Block::fallback_link(url));
        }
    }
    push_screens(&mut m, steps_url);
    m.push(Block::footer(ACTIVATED_REASON, CONTACT_REPLY, None));
    m.render()
}

/// Encadré par défaut d'un mail de bienvenue : rien pour un compte actif, « activation » pour un compte suspendu.
pub fn notice_for(premium: bool) -> WelcomeNotice {
    if premium {
        WelcomeNotice::None
    } else {
        WelcomeNotice::Activation
    }
}

/// Contenu du mail d'un lien selon son type : bienvenue et démo (`premium` = compte déjà actif, sans encadré
/// « activation »), ou activation d'un compte sans mot de passe. Les premiers pas sont sur l'hôte du lien.
pub fn render(
    kind: &str,
    username: &str,
    link: &str,
    ttl_mins: u64,
    premium: bool,
) -> RenderedMail {
    render_with(kind, username, link, ttl_mins, notice_for(premium))
}

/// Comme [`render`], avec l'encadré du mail de bienvenue choisi par l'appelant (essai de l'inscription publique).
pub fn render_with(
    kind: &str,
    username: &str,
    link: &str,
    ttl_mins: u64,
    notice: WelcomeNotice,
) -> RenderedMail {
    let steps = steps_url(link.split("/bienvenue/").next().unwrap_or(link));
    match kind {
        KIND_ACTIVATED => activated_mail(
            username,
            &steps,
            ActivatedButton::Link {
                url: link,
                ttl_mins,
            },
        ),
        _ => welcome_mail(username, link, &steps, ttl_mins, notice),
    }
}

/// Mot de passe déjà choisi : le dernier lien du compte (hors démo) a été consommé. Un compte sans lien connu
/// (lien purgé, compte d'avant les liens) n'est pas présumé en avoir un : il reçoit un nouveau lien, comme avant.
async fn password_chosen(ctx: &TaskContext, user_id: &str) -> bool {
    let t = now();
    ctx.state
        .read(|st| {
            matches!(
                status_for(st.welcome_links.values(), user_id, t),
                LinkStatus::Used { .. }
            )
        })
        .await
}

/// Ce que renvoie l'émission d'un lien : l'URL (à ne jamais journaliser) et l'échéance.
pub struct Sent {
    pub url: String,
    pub expires_at: i64,
    pub mail_sent: bool,
}

/// Émet un lien pour ce compte et envoie le mail correspondant. Sans SMTP, le lien est émis quand même
/// (l'admin peut le transmettre à la main). En dry-run : rien n'est écrit ni envoyé, l'URL est factice.
/// Activation d'un compte dont le mot de passe est déjà choisi : aucun lien émis, le bouton mène à Jellyfin
/// (l'URL renvoyée est alors celle de Jellyfin).
pub async fn send(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    email: &str,
    kind: &str,
    premium: bool,
) -> Result<Sent> {
    send_with(ctx, user_id, username, email, kind, notice_for(premium)).await
}

/// Comme [`send`], avec l'encadré du mail de bienvenue choisi par l'appelant (l'onboarding d'une inscription
/// publique : essai démarré ou compte à activer).
pub async fn send_with(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    email: &str,
    kind: &str,
    notice: WelcomeNotice,
) -> Result<Sent> {
    let s = &ctx.secrets;
    let base = link_base(ctx);
    let ttl = ctx.cfg.onboard.link_ttl_mins;
    if ctx.dry_run {
        return Ok(Sent {
            url: url(&base, "dry-run"),
            expires_at: now() + (ttl as i64) * 60,
            mail_sent: false,
        });
    }
    let to_jellyfin = kind == KIND_ACTIVATED && password_chosen(ctx, user_id).await;
    let (link, expires_at, mail) = if to_jellyfin {
        let home = s.jellyfin_public_url.clone();
        let mail = activated_mail(
            username,
            &steps_url(&base),
            ActivatedButton::Jellyfin(&home),
        );
        (home, now(), mail)
    } else {
        let token = issue(ctx, user_id, username, email, kind).await?;
        let link = url(&base, &token);
        let mail = render_with(kind, username, &link, ttl, notice);
        (link, now() + (ttl as i64) * 60, mail)
    };
    let Some(smtp) = &s.smtp else {
        return Ok(Sent {
            url: link,
            expires_at,
            mail_sent: false,
        });
    };
    // `GUIDE_CONTACT_EMAIL` en `Reply-To` seulement s'il est sur le domaine d'envoi (`mail::reply_to_kept`) : sinon
    // une réponse arrive à l'adresse d'envoi
    let reply_to = s.guide_contact_email.as_deref();
    let mail_sent = match crate::mail::send_rendered(smtp, username, email, &mail, reply_to, &[])
        .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(task = "onboard", username = %username, error = format!("{e:#}"), "link mail failed");
            let what = if crate::mail::maybe_sent(&e) {
                "est peut-être parti (réponse du serveur perdue : demander au membre avant de transmettre quoi que ce soit)"
            } else {
                "n'est pas parti"
            };
            let body = if to_jellyfin {
                format!("Le mail ({kind}) pour {username} {what} ({e:#}). Aucun lien à transmettre : son mot de passe est déjà choisi.")
            } else {
                format!("Le mail ({kind}) pour {username} {what} ({e:#}). Lien à transmettre à la main : {link}")
            };
            crate::alerts::admin(
                ctx,
                crate::alerts::Level::Error,
                &format!("Mail de bienvenue non parti : {username}"),
                &body,
            )
            .await;
            false
        }
    };
    Ok(Sent {
        url: link,
        expires_at,
        mail_sent,
    })
}

#[cfg(test)]
mod mail_tests {
    use super::*;
    use crate::mail_layout::checks::assert_member_mail;

    const LINK: &str = "https://x.test/bienvenue/Xq3vN8pL2rT6yW0aZ4cE7gJ1kM5oR9sU2wY6bD8fH0j";

    #[test]
    fn welcome_mail_has_one_link_and_no_credentials() {
        let r = render(KIND_WELCOME, "léa", LINK, 60, false);
        assert_member_mail(&r);
        assert_eq!(
            r.subject,
            "Bienvenue sur Groscailloux TV : définis ton mot de passe"
        );
        assert!(r
            .text
            .contains(&format!("Définir mon mot de passe : {LINK}")));
        assert!(r.text.contains("activé par l'administrateur"));
        assert_eq!(r.html.matches(LINK).count(), 3); // bouton + lien de secours (href + texte)
        assert_eq!(r.text.matches(LINK).count(), 1);
        for bad in ["Password", "Username :", "spam", "duckdns", "Reset"] {
            assert!(!r.text.contains(bad) && !r.html.contains(bad), "{bad}");
        }
        // quoi installer, avec le lien vers les premiers pas (même hôte que le lien)
        assert!(
            r.text.contains("Sur quel écran ?") && r.text.contains("https://x.test/premiers-pas")
        );
        assert!(r.html.contains("Quick Connect") && r.html.contains("https://x.test/premiers-pas"));
        assert!(r.text.contains("valable 60 minutes"));
        // logo et bandeau joints, rien d'autre
        assert_eq!(r.images.len(), 2);
        // compte déjà actif (ou démo) : pas d'encadré « activation »
        let demo = render(KIND_DEMO, "Exemple", LINK, 60, true);
        assert_member_mail(&demo);
        assert!(demo.text.starts_with("Bienvenue, Exemple !"));
        assert!(
            !demo.text.contains("activé par l'administrateur") && !demo.html.contains("Activation")
        );
    }

    #[test]
    fn activated_mail_for_premium_account() {
        // pas encore de mot de passe : nouveau lien, comme la bienvenue
        let r = render(KIND_ACTIVATED, "bob", LINK, 60, true);
        assert_member_mail(&r);
        assert_eq!(r.subject, "Ton compte Groscailloux TV est actif");
        assert!(r.html.contains("Ouvrir Groscailloux"));
        assert!(!r.text.contains("activé par l'administrateur"));
        assert_eq!(r.html.matches(LINK).count(), 3);
        // mot de passe peut-être déjà choisi (lien purgé après 7 jours) : texte conditionnel
        assert!(
            r.text.contains("Pas encore de mot de passe ?")
                && r.text.contains("ton mot de passe habituel")
                && r.text.contains("valable 60 minutes")
        );
        assert!(!r.text.contains("Il ne reste qu'à choisir"));
        // mot de passe déjà choisi : Jellyfin, sans lien de bienvenue ni validité
        let jf = activated_mail(
            "bob",
            "https://x.test/premiers-pas",
            ActivatedButton::Jellyfin("https://tv.x.test"),
        );
        assert_member_mail(&jf);
        assert_eq!(jf.subject, r.subject);
        assert!(jf.html.contains("Ouvrir Groscailloux"));
        assert!(jf
            .text
            .contains("Ouvrir Groscailloux TV : https://tv.x.test\n"));
        assert!(!jf.html.contains("/bienvenue/") && !jf.text.contains("/bienvenue/"));
        assert!(!jf.text.contains("valable"));
        assert!(jf.html.contains("https://x.test/premiers-pas"));
    }

    #[test]
    fn trial_notice_replaces_the_activation_promise() {
        let r = welcome_mail(
            "léa",
            LINK,
            "https://x.test/premiers-pas",
            60,
            WelcomeNotice::Trial { days: 7 },
        );
        assert_member_mail(&r);
        assert!(r
            .text
            .contains("Ton essai de 7 jours a commencé : dès que ton mot de passe est choisi"));
        assert!(!r.text.contains("activé par l'administrateur"));
        assert_eq!(r.html.matches(LINK).count(), 3);
    }

    #[test]
    fn html_escapes_user_input() {
        let r = render(KIND_WELCOME, "<b>x</b>", "https://x.test/b/t", 60, true);
        assert!(r.html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!r.html.contains("<b>x</b>"));
    }

    #[test]
    fn only_a_consumed_last_link_means_the_password_is_chosen() {
        let l = |kind: &str, used: Option<i64>| WelcomeLink {
            user_id: "a".into(),
            username: "a".into(),
            email: "a@example.test".into(),
            kind: kind.into(),
            created_at: 10,
            expires_at: 20,
            used_at: used,
        };
        let used = [l(KIND_WELCOME, Some(15))];
        assert!(matches!(
            status_for(&used, "a", 30),
            LinkStatus::Used { .. }
        ));
        for links in [
            vec![l(KIND_WELCOME, None)],
            vec![l(KIND_DEMO, Some(15))],
            vec![],
        ] {
            assert!(!matches!(
                status_for(&links, "a", 30),
                LinkStatus::Used { .. }
            ));
        }
    }
}
