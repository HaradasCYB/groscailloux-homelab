//! Page publique `/guide` : guide des nouveaux membres (connexion, navigation, demandes).
//! Le HTML (`assets/guide.html`, captures intégrées) ne contient ni adresse ni contact : ils
//! viennent de `.env` (dépôt public), expéditeur des mails compris (`SMTP_FROM`). Captures d'origine :
//! `backups/guide-draft-20260915/` (le gabarit qui s'y trouve date du 17/09 : on édite l'asset).

use homelab_core::config::Secrets;
use homelab_core::html::esc;

const GUIDE_HTML: &str = include_str!("../assets/guide.html");
pub const ICON_PNG: &[u8] = include_bytes!("../assets/guide-icon.png");

fn host(url: &str) -> &str {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
}

/// Cartes de contact ; rien si aucun contact n'est configuré.
fn contact(email: Option<&str>, discord: Option<&str>) -> String {
    let mut cards = String::new();
    if let Some(m) = email {
        let m = esc(m);
        cards.push_str(&format!(
            r#"<div><div class="k">Par mail</div><div class="v"><a href="mailto:{m}">{m}</a></div></div>"#
        ));
    }
    if let Some(d) = discord {
        cards.push_str(&format!(
            r#"<div><div class="k">Sur Discord</div><div class="v">{}</div></div>"#,
            esc(d)
        ));
    }
    if cards.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="contact">{cards}</div>"#)
    }
}

/// Expéditeur des mails (`SMTP_FROM`), à ajouter aux contacts ; sans SMTP configuré, une formule
/// neutre qui garde les phrases du guide correctes (« ajoute … à tes contacts »).
fn mail_from(from: Option<&str>) -> String {
    match from.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => format!(r#"<span class="url">{}</span>"#, esc(f)),
        None => "l'adresse d'envoi du serveur".to_string(),
    }
}

pub fn render(
    jellyfin_url: &str,
    jellyseerr_url: &str,
    email: Option<&str>,
    discord: Option<&str>,
    sender: Option<&str>,
) -> String {
    GUIDE_HTML
        .replace("{{JELLYFIN_URL}}", &esc(jellyfin_url))
        .replace("{{JELLYFIN_HOST}}", &esc(host(jellyfin_url)))
        .replace("{{JELLYSEERR_URL}}", &esc(jellyseerr_url))
        .replace("{{JELLYSEERR_HOST}}", &esc(host(jellyseerr_url)))
        .replace("{{MAIL_FROM}}", &mail_from(sender))
        .replace("{{CONTACT}}", &contact(email, discord))
}

pub fn page(s: &Secrets) -> String {
    render(
        &s.jellyfin_public_url,
        &s.jellyseerr_public_url,
        s.guide_contact_email.as_deref(),
        s.guide_contact_discord.as_deref(),
        s.smtp.as_ref().map(|m| m.from.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_placeholder_is_filled() {
        let p = render(
            "https://jf.example.org",
            "https://js.example.org/",
            Some("a@b.io"),
            Some("pseudo"),
            Some("contact@jf.example.org"),
        );
        assert!(!p.contains("{{"), "placeholder restant");
        assert!(p.contains(r#"href="https://jf.example.org""#));
        assert!(p.contains(">js.example.org<"));
        assert!(p.contains("mailto:a@b.io") && p.contains("pseudo"));
        assert!(p.contains(r#"<span class="url">contact@jf.example.org</span>"#));
        assert!(p.contains(r#"id="nouvelle-adresse""#) && p.contains(r#"id="changer-adresse""#));
        // encart : regarder et demander dans Jellyfin (onglet Découvrir), jamais de lien Jellyseerr
        let news = &p[p.find(r#"id="nouvelle-adresse""#).unwrap()..];
        let news = &news[..news.find("</aside>").unwrap()];
        assert!(news.contains("Découvrir") && !news.contains("js.example.org"));
        // l'accroche dit la même chose : on demande aussi dans Jellyfin
        let lead = &p[p.find(r#"<p class="lead">"#).unwrap()..];
        assert!(lead[..lead.find("</p>").unwrap()].contains("Découvrir"));
    }

    #[test]
    fn sender_is_escaped_and_optional() {
        assert_eq!(mail_from(None), "l'adresse d'envoi du serveur");
        assert_eq!(mail_from(Some("  ")), "l'adresse d'envoi du serveur");
        assert!(mail_from(Some("<x>@y")).contains("&lt;x&gt;@y"));
        let p = render("https://a.example", "https://b.example", None, None, None);
        assert!(
            !p.contains("{{") && p.contains("ajoute l'adresse d'envoi du serveur à tes contacts")
        );
    }

    #[test]
    fn contact_is_escaped_and_optional() {
        assert_eq!(contact(None, None), "");
        let c = contact(None, Some("<b>x"));
        assert!(c.contains("&lt;b&gt;x") && !c.contains("Par mail"));
    }

    #[test]
    fn asset_carries_no_address() {
        assert!(!GUIDE_HTML.contains("duckdns"));
        assert!(!GUIDE_HTML.contains("mailto:"));
    }
}
