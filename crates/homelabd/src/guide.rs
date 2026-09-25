//! Page publique `/guide` : guide des nouveaux membres (connexion, navigation, demandes).
//! Le HTML (`assets/guide.html`, captures intégrées) ne contient ni adresse ni contact : ils
//! viennent de `.env` (dépôt public). Source et captures : `backups/guide-draft-20260915/`.

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

pub fn render(
    jellyfin_url: &str,
    jellyseerr_url: &str,
    email: Option<&str>,
    discord: Option<&str>,
) -> String {
    GUIDE_HTML
        .replace("{{JELLYFIN_URL}}", &esc(jellyfin_url))
        .replace("{{JELLYFIN_HOST}}", &esc(host(jellyfin_url)))
        .replace("{{JELLYSEERR_URL}}", &esc(jellyseerr_url))
        .replace("{{JELLYSEERR_HOST}}", &esc(host(jellyseerr_url)))
        .replace("{{CONTACT}}", &contact(email, discord))
}

pub fn page(s: &Secrets) -> String {
    render(
        &s.jellyfin_public_url,
        &s.jellyseerr_public_url,
        s.guide_contact_email.as_deref(),
        s.guide_contact_discord.as_deref(),
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
        );
        assert!(!p.contains("{{"), "placeholder restant");
        assert!(p.contains(r#"href="https://jf.example.org""#));
        assert!(p.contains(">js.example.org<"));
        assert!(p.contains("mailto:a@b.io") && p.contains("pseudo"));
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
