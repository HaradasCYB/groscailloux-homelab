//! Page publique `/guide` : guide des nouveaux membres (connexion, navigation, demandes).
//! Le HTML (`assets/guide.html`, captures intégrées) ne contient ni adresse ni contact : ils
//! viennent de `.env` (dépôt public), expéditeur des mails compris (`SMTP_FROM`). Captures d'origine :
//! `backups/guide-draft-20260915/` (le gabarit qui s'y trouve date du 17/09 : on édite l'asset).
//!
//! DA commune (`crate::ui`, `docs/DESIGN.md`) : l'asset ne contient que le corps de la page, rendu par
//! [`crate::ui::page`] avec [`GUIDE_CSS`] (police système, aucune ressource externe). Sommaire en grille sous
//! l'accroche sur téléphone et tablette, colonne collante à gauche sur grand écran ; impression sur fond clair.

use crate::ui;
use homelab_core::config::Secrets;
use homelab_core::html::esc;

const GUIDE_HTML: &str = include_str!("../assets/guide.html");
/// Ancienne icône de l'onglet du guide : la page prend désormais celle de `ui` (`/assets/logo.png`) ; la route
/// `/guide/icon.png` reste servie pour les liens et favoris déjà enregistrés.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/guide-icon.png");

/// Titre de l'onglet (inchangé).
const TITLE: &str = "Bienvenue sur Groscailloux";

/// Règles propres au guide, après `ui::BASE_CSS`. `.row` (deux colonnes texte/capture), `.lead` et `.small`
/// (téléphone étroit) existent aussi dans la base : on les redéfinit ici, sous `.gd`.
const GUIDE_CSS: &str = r#"html{scroll-behavior:smooth}
.gd{max-width:66rem;margin:0 auto}
.gd p{max-width:66ch}
.gd img{display:block}
.g-hero{display:grid;gap:20px;padding-bottom:8px}
.g-hero h1{margin:0;text-wrap:balance}
.gd .lead{max-width:62ch;margin:0}
.logo-print{display:none}
.essentiel{display:grid;grid-template-columns:repeat(auto-fit,minmax(15rem,1fr));gap:12px}
.essentiel>div,.app,.rules>div,.contact>div{background:var(--surface);border:1px solid var(--line);border-radius:var(--r-lg);padding:16px 18px}
.essentiel .jf,.essentiel .js,.contact>div{border-top:3px solid var(--accent)}
.essentiel .k,.contact .k,.app .dev{font-size:.78rem;font-weight:600;text-transform:uppercase;letter-spacing:.08em;color:var(--dim)}
.essentiel .v{font-weight:600;color:var(--strong);margin-top:4px}
.essentiel .v small{display:block;font-weight:400;color:var(--dim);font-size:.9rem;margin-top:4px}
.url{font:500 .86rem/1.4 var(--mono);color:var(--text);background:var(--raised);border:1px solid var(--line);border-radius:var(--r-sm);padding:2px 7px;overflow-wrap:anywhere}
a.url{color:var(--accent-hover)}
a.url:hover{color:var(--strong)}
.news{display:grid;gap:8px;background:rgba(52,199,123,.07);border:1px solid rgba(52,199,123,.4);border-radius:var(--r-lg);padding:16px 18px;scroll-margin-top:72px}
.news h2{font-size:1.15rem;margin:0;display:flex;flex-wrap:wrap;align-items:center;gap:6px 12px}
.news .tag{font:700 .7rem/1 var(--font);text-transform:uppercase;letter-spacing:.1em;color:var(--ok);background:rgba(52,199,123,.14);border-radius:999px;padding:5px 9px}
.news p{margin:0;max-width:74ch}
.news .more a{font-weight:600}
nav.steps{margin:32px 0 8px}
.toc-t{font-size:.78rem;font-weight:600;text-transform:uppercase;letter-spacing:.08em;color:var(--dim);margin:0 0 10px}
nav.steps ol{list-style:none;margin:0;padding:0;display:grid;grid-template-columns:repeat(auto-fill,minmax(10.5rem,1fr));gap:8px}
nav.steps a{display:flex;align-items:center;gap:10px;min-height:44px;padding:8px 12px;border:1px solid var(--line);border-radius:var(--r-md);background:var(--surface);color:var(--text);text-decoration:none;font-size:.94rem;line-height:1.25}
nav.steps a:hover{border-color:var(--field-hover);color:var(--strong)}
nav.steps b{flex:none;min-width:1.6em;font:700 .8rem/1 var(--font-display);color:var(--accent-hover);font-variant-numeric:tabular-nums}
section,#changer-adresse,#sommaire{scroll-margin-top:72px}
section{padding:48px 0 8px}
.eyebrow{display:flex;align-items:center;gap:12px;margin-bottom:10px}
.num{flex:none;width:36px;height:36px;display:grid;place-items:center;border-radius:var(--r-md);background:rgba(47,143,255,.14);color:var(--accent-hover);font:700 .95rem/1 var(--font-display)}
.eyebrow span:last-child{font-size:.8rem;font-weight:600;text-transform:uppercase;letter-spacing:.1em;color:var(--dim)}
section h2{margin:0 0 12px;text-wrap:balance}
section h3{margin:26px 0 8px}
ul.clean{margin:0 0 12px;padding-left:1.1em;max-width:66ch}
ul.clean li{margin-bottom:6px}
ul.clean li::marker{color:var(--accent)}
ol.quick{margin:14px 0 0;padding-left:1.3em;display:grid;gap:8px;max-width:66ch}
ol.quick li::marker{color:var(--accent-hover);font-weight:700}
.gd .row{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1.15fr);gap:32px;align-items:start;margin-top:18px}
.gd .row.rev>:first-child{order:2}
figure{margin:0}
figure img{border-radius:var(--r-lg);border:1px solid var(--line)}
figcaption{color:var(--dim);font-size:.875rem;margin-top:8px}
.duo{display:grid;grid-template-columns:minmax(0,2.6fr) minmax(0,1fr);gap:18px;align-items:end;margin-top:18px}
.phone img{border-radius:26px;border:6px solid var(--raised)}
.gd .phone.small{max-width:240px;font-size:inherit}
.apps,.rules{display:grid;grid-template-columns:repeat(auto-fit,minmax(14.5rem,1fr));gap:12px;margin-top:16px}
.app{display:grid;gap:4px;align-content:start;padding:14px 16px}
.app .name{font-weight:600;color:var(--strong)}
.app .where{font-size:.9rem;color:var(--dim)}
.app.tv{border-color:rgba(47,143,255,.45)}
.flow{display:flex;flex-wrap:wrap;align-items:center;gap:8px;margin:16px 0 6px}
.chip{font-size:.86rem;font-weight:600;padding:5px 12px;border-radius:999px;border:1px solid var(--line);background:var(--raised);color:var(--text)}
.chip.b{color:var(--accent-hover);border-color:rgba(47,143,255,.45);background:rgba(47,143,255,.1)}
.chip.c{color:var(--ok);border-color:rgba(52,199,123,.4);background:rgba(52,199,123,.1)}
.chip.p{color:var(--warn);border-color:rgba(240,180,41,.4);background:rgba(240,180,41,.1)}
.arrow{color:var(--dim)}
.legend{display:grid;gap:6px;margin-top:10px;font-size:.94rem}
.legend div{display:grid;grid-template-columns:190px 1fr;gap:10px;align-items:baseline}
.tip{background:var(--surface);border:1px solid var(--line);border-left:4px solid var(--accent);border-radius:var(--r-md);padding:12px 16px;margin:14px 0;max-width:66ch;font-size:.95rem}
.rules .big{font:800 1.5rem/1.1 var(--font-display);letter-spacing:-.02em;color:var(--strong)}
.rules p{margin:8px 0 0;font-size:.93rem;color:var(--dim)}
.contact{display:grid;grid-template-columns:repeat(auto-fit,minmax(15rem,1fr));gap:12px;margin:16px 0 18px;max-width:720px}
.contact .v{font:500 1rem/1.4 var(--mono);margin-top:6px;overflow-wrap:anywhere}
.contact .v a{color:var(--text)}
.gd footer{margin-top:64px;padding:24px 0 88px;color:var(--dim);font-size:.875rem}
.g-up{position:fixed;right:max(var(--gutter),env(safe-area-inset-right));bottom:max(var(--gutter),env(safe-area-inset-bottom));z-index:9;display:inline-flex;align-items:center;gap:6px;min-height:44px;padding:8px 16px;border-radius:999px;border:1px solid var(--line);background:var(--raised);color:var(--strong);font-size:.9rem;font-weight:600;text-decoration:none;box-shadow:0 6px 20px rgba(0,0,0,.45)}
.g-up:hover{color:var(--strong);border-color:var(--field-hover)}
@media (max-width:820px){.gd .row,.gd .row.rev{grid-template-columns:minmax(0,1fr)}.gd .row.rev>:first-child{order:0}}
@media (max-width:640px){.duo{grid-template-columns:minmax(0,1fr)}figure img{max-width:100%!important}}
@media (max-width:560px){.legend div{grid-template-columns:1fr;gap:2px}nav.steps ol{grid-template-columns:repeat(2,minmax(0,1fr))}}
@media (min-width:1100px){
.gd{max-width:none}
.g-layout{display:grid;grid-template-columns:13.5rem minmax(0,1fr);gap:48px;align-items:start}
nav.steps{position:sticky;top:81px;margin:0;max-height:calc(100vh - 97px);max-height:calc(100dvh - 97px);overflow-y:auto;padding:4px 4px 4px 0}
nav.steps ol{display:flex;flex-direction:column;gap:2px}
nav.steps a{min-height:40px;border-color:transparent;background:none;color:var(--dim)}
nav.steps a:hover{background:var(--surface);border-color:var(--line);color:var(--strong)}
.g-layout{margin-top:40px}
.g-content>section:first-child{padding-top:0}
.g-up{display:none}
.gd footer{padding-bottom:48px}
}
@media print{
@page{size:A4;margin:14mm 14mm 18mm}
:root{color-scheme:light;--bg:#fff;--surface:#f3f6fa;--raised:#eaeff6;--line:#d3dbe6;--divider:#d3dbe6;--text:#111723;--strong:#0b0f17;--dim:#4c586a;--accent:#1f6fd6;--accent-hover:#1f6fd6}
*{-webkit-print-color-adjust:exact;print-color-adjust:exact}
body{font-size:10.5pt}
.gc-top,.g-up{display:none}
.gc-main{padding:0;max-width:none}
.logo-print{display:block;width:260px}
.g-hero{gap:4mm;padding-bottom:4mm}
.g-layout{display:block;margin:0}
nav.steps{position:static;max-height:none;overflow:visible;margin:0 0 2mm;padding:0}
nav.steps ol{display:flex;flex-wrap:wrap;gap:4px}
nav.steps a{min-height:0;padding:3px 9px;border-color:var(--line);background:none;color:var(--text)}
section{padding:9mm 0 0}
#connexion{break-before:page;padding-top:0}
h2,h3,.eyebrow{break-after:avoid}
figure,.app,.rules>div,.contact>div,.essentiel>div,.legend>div,.flow,.tip,.news{break-inside:avoid}
.news{background:none}.news .tag{color:#1c8a52}.news .more{display:none}
figure img{border-color:var(--line);width:auto;max-width:100%;max-height:92mm}
.phone img{max-height:118mm}
figure.tall img{max-height:140mm}
.duo{grid-template-columns:auto auto;justify-content:start;gap:8mm}
.gd .row{margin-top:3mm;gap:4mm}
section h3{margin-top:5mm}
.url{white-space:nowrap;font-size:8.5pt}
.chip{color:var(--text)}.chip.b{color:#1f6fd6}.chip.c{color:#1c8a52}.chip.p{color:#8a5b00}
.contact .v a{color:var(--accent)}
.gd footer{margin-top:10mm;padding:5mm 0 0}
}"#;

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
    let body = GUIDE_HTML
        .replace("{{JELLYFIN_URL}}", &esc(jellyfin_url))
        .replace("{{JELLYFIN_HOST}}", &esc(host(jellyfin_url)))
        .replace("{{JELLYSEERR_URL}}", &esc(jellyseerr_url))
        .replace("{{JELLYSEERR_HOST}}", &esc(host(jellyseerr_url)))
        .replace("{{MAIL_FROM}}", &mail_from(sender))
        .replace("{{CONTACT}}", &contact(email, discord));
    ui::page(
        TITLE,
        &format!("<div class=\"gd\">\n{body}</div>"),
        GUIDE_CSS,
    )
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
    fn page_uses_the_shared_design_without_external_fonts() {
        let p = render("https://a.example", "https://b.example", None, None, None);
        assert!(p.starts_with("<!doctype html>\n<html lang=\"fr\">"));
        assert!(p.contains("<title>Bienvenue sur Groscailloux</title>"));
        assert!(p.contains("--bg:#08090b") && p.contains("-apple-system"));
        assert!(p.contains(r#"<link rel="icon" type="image/png" href="/assets/logo.png">"#));
        let lower = p.to_ascii_lowercase();
        for banned in [
            "fonts.googleapis",
            "fonts.gstatic",
            "onest",
            "unbounded",
            "jetbrains",
            "@import",
            "<script",
            "rel=\"stylesheet\"",
        ] {
            assert!(!lower.contains(banned), "reste : {banned}");
        }
        // les seules URL absolues sont celles des services (aucune ressource tierce chargée) ; « https:// »
        // apparaît aussi en toutes lettres dans le texte (« https:// compris »)
        let p = p.replace("<em>https://</em>", "");
        assert!(!p.contains("http://") && !p.contains("url("));
        assert_eq!(
            p.matches("https://").count(),
            p.matches("https://a.example").count() + p.matches("https://b.example").count()
        );
        // un seul <main> (celui de ui::page), un seul <body>, un seul <style>
        assert_eq!(p.matches("<main").count(), 1);
        assert_eq!(p.matches("<body>").count(), 1);
        assert_eq!(p.matches("<style>").count(), 1);
        // captures intégrées gardées ; logo d'impression seulement (la marque à l'écran est l'en-tête commun)
        assert_eq!(p.matches("src=\"data:image/").count(), 16);
        assert!(!p.contains("logo-screen"));
    }

    #[test]
    fn toc_links_every_section() {
        let p = render("https://a.example", "https://b.example", None, None, None);
        assert!(p.contains(r#"<nav class="steps" id="sommaire" aria-labelledby="sommaire-titre">"#));
        assert!(p.contains(r##"<a class="g-up" href="#sommaire">"##));
        let toc = &p[p.find(r#"id="sommaire""#).unwrap()..];
        let toc = &toc[..toc.find("</nav>").unwrap()];
        for id in [
            "demarrer",
            "mail",
            "connexion",
            "naviguer",
            "demander",
            "suivre",
            "soustitres",
            "compte",
            "tchat",
            "regles",
            "probleme",
        ] {
            assert!(
                toc.contains(&format!("href=\"#{id}\"")),
                "sommaire sans {id}"
            );
            assert!(
                p.contains(&format!("<section id=\"{id}\"")),
                "section absente : {id}"
            );
        }
    }

    #[test]
    fn asset_carries_no_address() {
        assert!(!GUIDE_HTML.contains("duckdns"));
        assert!(!GUIDE_HTML.contains("mailto:"));
    }
}
