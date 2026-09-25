//! Liens de bienvenue : le mail ne contient qu'un bouton vers `/bienvenue/<jeton>`, page où le membre définit son
//! mot de passe et retrouve ses accès (2026-09-19 : plus d'identifiant ni de mot de passe par mail, c'est ce qui
//! faisait finir le mail en spam et c'est plus sûr). Le jeton (32 octets aléatoires, base64url) n'est stocké
//! que **haché** (SHA-256) dans l'état ; il expire (`[onboard] link_ttl_mins`) et ne sert qu'une fois. Un jeton
//! `Demo` sert au mail de test : la page s'affiche en exemple, aucun compte n'est touché.
use anyhow::{bail, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::context::TaskContext;
use crate::html::esc;
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
// Mails de bienvenue / d'activation : un bouton vers la page, rien d'autre de sensible.
// ---------------------------------------------------------------------------------------------

const MAIL_HTML: &str = include_str!("../assets/mail/welcome.html");

/// Texte d'un mail de lien : sujet, version texte, version HTML.
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Contenu du mail selon le type de lien. `premium` = compte déjà actif.
pub fn render(kind: &str, username: &str, link: &str, ttl_mins: u64, premium: bool) -> Rendered {
    let (subject, heading, intro, button, outro) = match kind {
        KIND_ACTIVATED => (
            "Ton compte Groscailloux est actif".to_string(),
            format!("C'est bon, {username} !"),
            "Ton compte vient d'être activé : tu peux regarder films et séries dès maintenant.".to_string(),
            "Ouvrir Groscailloux".to_string(),
            "Si tu n'as pas encore choisi ton mot de passe, le bouton t'amène d'abord sur la page pour le définir. Une question ? Réponds simplement à ce mail.".to_string(),
        ),
        _ => (
            "Bienvenue sur Groscailloux : définis ton mot de passe".to_string(),
            format!("Bienvenue, {username} !"),
            "Ton compte est créé. Il ne reste qu'à choisir ton mot de passe, et tu retrouveras ensuite tes accès sur la même page.".to_string(),
            "Définir mon mot de passe".to_string(),
            if premium {
                "Une question ? Réponds simplement à ce mail.".to_string()
            } else {
                "Ton compte sera activé par l'administrateur sous peu : tu recevras un mail à ce moment-là. Une question ? Réponds simplement à ce mail.".to_string()
            },
        ),
    };
    let validity = format!("Ce lien est valable {ttl_mins} minutes et ne sert qu'une fois. Passé ce délai, la page te proposera d'en recevoir un nouveau.");
    let footer = "Tu reçois ce mail parce qu'un compte a été créé pour cette adresse sur Groscailloux, un service privé de streaming entre amis. Si ce n'est pas toi, ignore-le : rien ne sera activé sans le lien.";
    let text = format!(
        "{heading}\n\n{intro}\n\n{button} : {link}\n\n{validity}\n\n{outro}\n\n— Groscailloux\n{footer}\n"
    );
    let html = MAIL_HTML
        .replace("{{title}}", &esc(&subject))
        .replace("{{heading}}", &esc(&heading))
        .replace("{{intro}}", &esc(&intro))
        .replace("{{button}}", &esc(&button))
        .replace("{{validity}}", &esc(&validity))
        .replace("{{outro}}", &esc(&outro))
        .replace("{{footer}}", footer)
        .replace("{{link}}", &esc(link));
    Rendered {
        subject,
        text,
        html,
    }
}

/// Ce que renvoie l'émission d'un lien : l'URL (à ne jamais journaliser) et l'échéance.
pub struct Sent {
    pub url: String,
    pub expires_at: i64,
    pub mail_sent: bool,
}

/// Émet un lien pour ce compte et envoie le mail correspondant. Sans SMTP, le lien est émis quand même
/// (l'admin peut le transmettre à la main). En dry-run : rien n'est écrit ni envoyé, l'URL est factice.
pub async fn send(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    email: &str,
    kind: &str,
    premium: bool,
) -> Result<Sent> {
    let s = &ctx.secrets;
    let base = s
        .onboard_public_url
        .clone()
        .unwrap_or_else(|| format!("http://{}", ctx.cfg.web.listen));
    let ttl = ctx.cfg.onboard.link_ttl_mins;
    if ctx.dry_run {
        return Ok(Sent {
            url: url(&base, "dry-run"),
            expires_at: now() + (ttl as i64) * 60,
            mail_sent: false,
        });
    }
    let token = issue(ctx, user_id, username, email, kind).await?;
    let link = url(&base, &token);
    let expires_at = now() + (ttl as i64) * 60;
    let Some(smtp) = &s.smtp else {
        return Ok(Sent {
            url: link,
            expires_at,
            mail_sent: false,
        });
    };
    let r = render(kind, username, &link, ttl, premium);
    let reply_to = s.guide_contact_email.as_deref();
    let mail_sent = match crate::mail::send_html(
        smtp, username, email, &r.subject, &r.text, &r.html, reply_to,
    )
    .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(task = "onboard", username = %username, error = %e, "link mail failed");
            crate::alerts::admin(
                ctx,
                crate::alerts::Level::Error,
                &format!("Mail de bienvenue non parti : {username}"),
                &format!("Le mail ({kind}) pour {username} n'est pas parti ({e}). Lien à transmettre à la main : {link}"),
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

    #[test]
    fn welcome_mail_has_one_link_and_no_credentials() {
        let r = render(
            KIND_WELCOME,
            "léa",
            "https://x.test/bienvenue/abc",
            60,
            false,
        );
        assert_eq!(
            r.subject,
            "Bienvenue sur Groscailloux : définis ton mot de passe"
        );
        assert!(r.text.contains("https://x.test/bienvenue/abc"));
        assert!(r.text.contains("activé par l'administrateur"));
        assert_eq!(r.html.matches("https://x.test/bienvenue/abc").count(), 3); // bouton + lien de secours (href + texte)
        for bad in ["Password", "Username :", "spam", "duckdns", "Reset"] {
            assert!(!r.text.contains(bad) && !r.html.contains(bad), "{bad}");
        }
        assert!(!r.html.contains("{{"));
    }

    #[test]
    fn activated_mail_for_premium_account() {
        let r = render(
            KIND_ACTIVATED,
            "bob",
            "https://x.test/bienvenue/t",
            60,
            true,
        );
        assert_eq!(r.subject, "Ton compte Groscailloux est actif");
        assert!(r.html.contains("Ouvrir Groscailloux"));
        assert!(!r.text.contains("activé par l'administrateur"));
    }

    #[test]
    fn html_escapes_user_input() {
        let r = render(KIND_WELCOME, "<b>x</b>", "https://x.test/b/t", 60, true);
        assert!(r.html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(!r.html.contains("<b>x</b>"));
    }
}
