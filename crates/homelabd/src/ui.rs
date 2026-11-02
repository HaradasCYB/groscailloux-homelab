//! Identité visuelle commune des pages servies par homelabd (« Groscailloux TV » : noir + bleu galet, façon
//! Apple/Netflix). Mêmes jetons que les mails (`homelab_core::mail_layout`) et que l'habillage de Jellyfin
//! (`branding/jellyfin/groscailloux-tv.css`) : un changement de couleur se fait dans les trois. Référence de la DA
//! (jetons, composants, à faire / à éviter) : `docs/DESIGN.md`.
//!
//! - [`TOKENS`] : variables CSS (`:root`) ; [`BASE_CSS`] : composants partagés (en-tête, carte, titres, champs,
//!   boutons, notices, tableaux et pastilles d'administration) ;
//! - [`page`] : squelette HTML unique (`lang="fr"`, viewport, `theme-color`, `noindex`, icône), le titre est
//!   échappé ; [`page_described`] y ajoute un `<meta name="description">` (aperçu d'un lien partagé),
//!   [`page_admin`] le `no-referrer` des pages d'administration et un rechargement automatique facultatif
//!   (`<meta http-equiv="refresh">`), [`page_bare`] la variante compacte sans en-tête (cadre Homarr) ;
//! - [`error_page`] / [`error_response`] : page d'erreur (titre, message, lien de retour facultatif), même code
//!   HTTP que la réponse brute qu'elle remplace ;
//! - [`logo`] : `GET /assets/logo.png`, le galet de `branding/jellyfin/logo/icon-transparent.png`, public.
//!
//! Aucune ressource externe : police système, aucun script, aucune URL absolue (les pages publiques ne doivent
//! appeler aucun tiers).

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use homelab_core::html::esc;

/// Galet du logo (512 × 512, fond transparent), le même que dans Jellyfin. Servi par [`logo`].
pub const LOGO_PNG: &[u8] = include_bytes!("../../../branding/jellyfin/logo/icon-transparent.png");

/// Chemin public du logo (aussi icône de l'onglet).
pub const LOGO_PATH: &str = "/assets/logo.png";

/// Jetons de la DA. Contrastes (WCAG AA) : texte `#e6e7ea` et atténué `#a3a7af` sur le fond comme sur la surface,
/// liens `#5aa6ff`, texte blanc sur le remplissage `#1b6fd8` (4,9:1) et sur son survol `#1760c4` (6:1 ; `#2f8fff`
/// ne donnerait que 3,2:1) ; bord des champs `#60656f` : 3,4:1 sur le fond, 3,15:1 sur la carte (WCAG 1.4.11 ;
/// `--line` n'en donne que 1,4:1).
pub const TOKENS: &str = r#":root{
color-scheme:dark;
--bg:#08090b;--surface:#121418;--raised:#1f2228;--line:#2a2d33;--divider:#1d1f24;
--text:#e6e7ea;--strong:#ffffff;--dim:#a3a7af;
--accent:#2f8fff;--accent-hover:#5aa6ff;--accent-fill:#1b6fd8;
--ok:#34c77b;--warn:#f0b429;--err:#ef4444;
--field-line:#60656f;--field-hover:#7a7f89;--accent-press:#1760c4;
--btn-bg:#ffffff;--btn-text:#0b0b0c;--btn-hover:#d9dbe0;
--r-lg:.6em;--r-md:.45em;--r-sm:.3em;
--font:-apple-system,BlinkMacSystemFont,"SF Pro Text",Inter,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;
--font-display:-apple-system,BlinkMacSystemFont,"SF Pro Display",Inter,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;
--mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
--gutter:16px;--measure:34rem;--wide:72rem
}
"#;

/// Composants communs. Les pages n'ajoutent que leurs règles propres (`extra_css` de [`page`]).
pub const BASE_CSS: &str = r#"*,*::before,*::after{box-sizing:border-box}
[hidden]{display:none!important}
html{-webkit-text-size-adjust:100%;text-size-adjust:100%;scroll-padding-top:72px}
body{margin:0;min-height:100vh;min-height:100dvh;background:var(--bg);color:var(--text);font:16px/1.55 var(--font);-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale;overflow-wrap:break-word}
img{max-width:100%;height:auto}
a{color:var(--accent-hover);text-decoration:underline;text-decoration-thickness:1px;text-underline-offset:.18em}
a:hover{color:var(--strong)}
a,button,input,select,textarea,label,summary{touch-action:manipulation}
:focus-visible{outline:2px solid var(--accent-hover);outline-offset:2px}
.gc-top{position:sticky;top:0;z-index:10;background:rgba(8,9,11,.86);-webkit-backdrop-filter:saturate(1.4) blur(14px);backdrop-filter:saturate(1.4) blur(14px);border-bottom:1px solid var(--divider)}
.gc-top-in{display:flex;align-items:center;gap:12px;max-width:var(--wide);margin:0 auto;padding:10px var(--gutter);min-height:56px}
.gc-brand{display:inline-flex;align-items:center;gap:10px;color:var(--strong);text-decoration:none;font:800 1.05rem/1 var(--font-display);letter-spacing:-.02em;white-space:nowrap}
.gc-brand img{width:30px;height:30px;flex:none}
.gc-brand .gc-tv{display:inline-block;padding:3px 7px 4px;border-radius:var(--r-sm);background:linear-gradient(135deg,var(--accent-hover),var(--accent-fill));color:var(--strong);font-size:.8em;letter-spacing:0}
.gc-main{max-width:var(--wide);margin:0 auto;padding:32px var(--gutter) 64px}
.gc-narrow{max-width:var(--measure);margin-left:auto;margin-right:auto}
.gc-bare{padding:12px var(--gutter) 16px}
h1,h2,h3{color:var(--strong);font-family:var(--font-display);line-height:1.12;margin:0 0 .5em;text-wrap:balance}
h1{font-size:clamp(1.9rem,1.2rem + 2.6vw,2.75rem);font-weight:800;letter-spacing:-.035em}
h2{font-size:clamp(1.3rem,1.1rem + .9vw,1.6rem);font-weight:700;letter-spacing:-.025em;margin-top:1.6em}
h3{font-size:1.1rem;font-weight:700;letter-spacing:-.015em;margin-top:1.4em}
p{margin:0 0 1em}
.lead{font-size:1.125rem;color:var(--dim);max-width:var(--measure)}
.muted{color:var(--dim)}
.small{font-size:.875rem}
code,kbd,.mono{font-family:var(--mono);font-size:.92em}
code{background:var(--raised);border-radius:var(--r-sm);padding:.1em .35em}
hr{border:0;border-top:1px solid var(--line);margin:2em 0}
.card{background:var(--surface);border:1px solid var(--line);border-radius:var(--r-lg);padding:24px;margin:0 0 20px}
.card>:last-child{margin-bottom:0}
.stack>*+*{margin-top:16px}
.row{display:flex;flex-wrap:wrap;gap:12px;align-items:center}
label{display:block;font-weight:600;font-size:.95rem;color:var(--text);margin:0 0 6px}
.hint{display:block;font-size:.875rem;color:var(--dim);margin-top:6px}
input,select,textarea{font:inherit;color:var(--text);background:var(--raised);border:1px solid var(--field-line);border-radius:var(--r-md);padding:11px 14px;width:100%;min-height:44px;max-width:100%}
textarea{min-height:7em;resize:vertical}
input::placeholder,textarea::placeholder{color:var(--dim);opacity:1}
input:hover,select:hover,textarea:hover{border-color:var(--field-hover)}
input:focus-visible,select:focus-visible,textarea:focus-visible{outline:2px solid var(--accent);outline-offset:1px;border-color:var(--accent)}
input[type=checkbox],input[type=radio]{width:auto;min-height:0;accent-color:var(--accent)}
input[aria-invalid=true]{border-color:var(--err)}
.field{margin:0 0 18px}
.btn,button{display:inline-flex;align-items:center;justify-content:center;gap:8px;min-height:44px;padding:10px 20px;border-radius:var(--r-md);border:1px solid transparent;background:var(--btn-bg);color:var(--btn-text);font:700 1rem/1.2 var(--font);text-decoration:none;cursor:pointer;transition:background-color .15s ease,border-color .15s ease,color .15s ease}
.btn:hover,button:hover{background:var(--btn-hover);color:var(--btn-text)}
.btn.secondary,button.secondary{background:rgba(255,255,255,.08);color:var(--strong);border-color:var(--line)}
.btn.secondary:hover,button.secondary:hover{background:rgba(255,255,255,.14);color:var(--strong)}
.btn.accent,button.accent{background:var(--accent-fill);color:var(--strong)}
.btn.accent:hover,button.accent:hover{background:var(--accent-press)}
.btn.danger,button.danger{background:transparent;color:#ff8a8a;border-color:rgba(239,68,68,.55)}
.btn.danger:hover,button.danger:hover{background:rgba(239,68,68,.14);color:#ffb3b3}
.btn.block,button.block{width:100%}
.btn[disabled],button[disabled],.btn[aria-disabled=true]{opacity:.5;cursor:not-allowed}
.notice{border:1px solid var(--line);border-left:4px solid #4a4f58;background:var(--surface);border-radius:var(--r-md);padding:12px 16px;margin:0 0 16px;color:var(--text)}
.notice>:last-child{margin-bottom:0}
.notice.info{border-left-color:var(--accent)}
.notice.ok{border-left-color:var(--ok);background:rgba(52,199,123,.08)}
.notice.warn{border-left-color:var(--warn);background:rgba(240,180,41,.08)}
.notice.err{border-left-color:var(--err);background:rgba(239,68,68,.09)}
.table-wrap{overflow-x:auto;-webkit-overflow-scrolling:touch;overscroll-behavior-x:contain;scrollbar-color:#4a4f58 transparent;scrollbar-width:thin;border:1px solid var(--line);border-radius:var(--r-lg);margin:0 0 20px;background:linear-gradient(to right,var(--bg) 40%,rgba(8,9,11,0)) left/32px 100% no-repeat local,linear-gradient(to left,var(--bg) 40%,rgba(8,9,11,0)) right/32px 100% no-repeat local,radial-gradient(farthest-side at 0 50%,rgba(90,166,255,.35),rgba(90,166,255,0)) left/12px 100% no-repeat scroll,radial-gradient(farthest-side at 100% 50%,rgba(90,166,255,.35),rgba(90,166,255,0)) right/12px 100% no-repeat scroll}
table{width:100%;border-collapse:collapse;font-size:.92rem;font-variant-numeric:tabular-nums}
th,td{padding:10px 12px;text-align:left;vertical-align:top;border-bottom:1px solid var(--divider)}
th{color:var(--dim);font-weight:600;font-size:.8rem;text-transform:uppercase;letter-spacing:.04em;background:var(--surface);white-space:nowrap}
tbody tr:last-child td{border-bottom:0}
tbody tr:hover td{background:rgba(255,255,255,.025)}
.pill{display:inline-block;padding:2px 9px;border-radius:999px;font-size:.78rem;font-weight:600;line-height:1.5;white-space:nowrap;background:var(--raised);color:#d6d8dc;border:1px solid var(--line)}
.pill.ok{color:var(--ok);border-color:rgba(52,199,123,.4);background:rgba(52,199,123,.1)}
.pill.warn{color:var(--warn);border-color:rgba(240,180,41,.4);background:rgba(240,180,41,.1)}
.pill.err{color:#ff8a8a;border-color:rgba(239,68,68,.45);background:rgba(239,68,68,.1)}
.pill.info{color:var(--accent-hover);border-color:rgba(47,143,255,.45);background:rgba(47,143,255,.1)}
.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
@media (max-width:600px){.gc-main{padding-top:24px}.card{padding:18px}.btn.block-sm,button.block-sm{width:100%}}
@media (prefers-reduced-motion:reduce){*,*::before,*::after{animation-duration:.01ms!important;animation-iteration-count:1!important;transition-duration:.01ms!important;scroll-behavior:auto!important}}
@media (forced-colors:active){.btn,button,.pill,.notice{border:1px solid CanvasText}}
"#;

/// Page complète : `title` est échappé ; `body_html` (déjà échappé par l'appelant) va dans `<main>` sous
/// l'en-tête ; `extra_css` (constante du code, jamais une saisie) suit [`BASE_CSS`].
pub fn page(title: &str, body_html: &str, extra_css: &str) -> String {
    render(title, &Head::default(), body_html, extra_css)
}

/// Comme [`page`], avec un `<meta name="description">` (échappé, omis s'il est vide) : aperçu du lien quand un
/// membre le partage (`/premium`).
pub fn page_described(title: &str, description: &str, body_html: &str, extra_css: &str) -> String {
    let head = Head {
        description,
        ..Head::default()
    };
    render(title, &head, body_html, extra_css)
}

/// Page d'administration (`/accounts`, liste rouge, `/recherche`, connexion, onboarding) : comme [`page`], avec
/// `<meta name="referrer" content="no-referrer">` (aucune adresse de page d'admin ne part dans un en-tête
/// `Referer`) ; `refresh` = `<meta http-equiv="refresh">` vers un chemin local (échappé) après N secondes
/// (résultats de `/recherche` tant que la recherche tourne, travail de la liste rouge), sans JavaScript.
pub fn page_admin(
    title: &str,
    refresh: Option<(u32, &str)>,
    body_html: &str,
    extra_css: &str,
) -> String {
    let head = Head {
        refresh: refresh.map(|(s, u)| (s, Some(u))),
        no_referrer: true,
        ..Head::default()
    };
    render(title, &head, body_html, extra_css)
}

/// Variante compacte **sans en-tête** pour une page affichée dans un cadre (`/status.html` dans Homarr) : même
/// squelette et mêmes jetons, `<main class="gc-bare">` (fond `--bg` opaque : un fond transparent dépendrait du
/// `color-scheme` de la page hôte), `no-referrer` (l'adresse porte le jeton d'état). `refresh_secs` recharge la
/// **même** adresse (`content="N"` sans `url=` : le jeton de la requête est gardé).
pub fn page_bare(
    title: &str,
    refresh_secs: Option<u32>,
    body_html: &str,
    extra_css: &str,
) -> String {
    let head = Head {
        refresh: refresh_secs.map(|s| (s, None)),
        no_referrer: true,
        bare: true,
        ..Head::default()
    };
    render(title, &head, body_html, extra_css)
}

/// Page d'erreur (session expirée, trop d'essais, service injoignable…) : carte étroite, `title` en `<h1>` (échappé,
/// aussi titre de l'onglet), `message_html` (constante du code, déjà échappée) en paragraphe, puis un lien
/// `back` facultatif (`(href, libellé)`, échappés). `no-referrer` comme [`page_admin`] : ces réponses gardent
/// surtout des pages d'administration dont l'adresse peut porter un jeton.
pub fn error_page(title: &str, message_html: &str, back: Option<(&str, &str)>) -> String {
    let back = back
        .map(|(href, label)| {
            format!(
                "<p><a class=\"btn secondary\" href=\"{}\">{}</a></p>",
                esc(href),
                esc(label)
            )
        })
        .unwrap_or_default();
    let body = format!(
        "<div class=\"gc-narrow\"><div class=\"card gc-error\"><h1>{}</h1><p>{message_html}</p>{back}</div></div>",
        esc(title)
    );
    let head = Head {
        no_referrer: true,
        ..Head::default()
    };
    render(title, &head, &body, ERROR_CSS)
}

/// Réponse HTTP d'une [`error_page`] avec le code `status`.
pub fn error_response(
    status: axum::http::StatusCode,
    title: &str,
    message_html: &str,
    back: Option<(&str, &str)>,
) -> Response {
    (
        status,
        axum::response::Html(error_page(title, message_html, back)),
    )
        .into_response()
}

/// Mise en page propre à [`error_page`] : carte centrée un peu plus bas, titre à taille de carte.
const ERROR_CSS: &str =
    ".gc-error{margin-top:8vh}.gc-error h1{font-size:clamp(1.5rem,1.2rem + 1.2vw,1.9rem)}";

/// Options du squelette (privées : chaque variante publique en fixe un jeu).
#[derive(Default)]
struct Head<'a> {
    description: &'a str,
    /// Délai, puis chemin cible (`None` : la même adresse).
    refresh: Option<(u32, Option<&'a str>)>,
    no_referrer: bool,
    /// Sans en-tête `gc-top`, `<main class="gc-bare">`.
    bare: bool,
}

fn render(title: &str, head: &Head<'_>, body_html: &str, extra_css: &str) -> String {
    debug_assert!(!extra_css.to_ascii_lowercase().contains("</style"));
    let mut out = String::with_capacity(TOKENS.len() + BASE_CSS.len() + body_html.len() + 1024);
    out.push_str("<!doctype html>\n<html lang=\"fr\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    out.push_str("<meta name=\"theme-color\" content=\"#08090b\">\n<meta name=\"color-scheme\" content=\"dark\">\n");
    out.push_str("<meta name=\"robots\" content=\"noindex\">\n");
    if head.no_referrer {
        out.push_str("<meta name=\"referrer\" content=\"no-referrer\">\n");
    }
    out.push_str("<title>");
    out.push_str(&esc(title));
    out.push_str("</title>\n");
    if !head.description.is_empty() {
        out.push_str("<meta name=\"description\" content=\"");
        out.push_str(&esc(head.description));
        out.push_str("\">\n");
    }
    match head.refresh {
        Some((secs, Some(url))) => out.push_str(&format!(
            "<meta http-equiv=\"refresh\" content=\"{secs};url={}\">\n",
            esc(url)
        )),
        Some((secs, None)) => out.push_str(&format!(
            "<meta http-equiv=\"refresh\" content=\"{secs}\">\n"
        )),
        None => {}
    }
    out.push_str("<link rel=\"icon\" type=\"image/png\" href=\"");
    out.push_str(LOGO_PATH);
    out.push_str("\">\n<link rel=\"apple-touch-icon\" href=\"");
    out.push_str(LOGO_PATH);
    out.push_str("\">\n<style>\n");
    out.push_str(TOKENS);
    out.push_str(BASE_CSS);
    if !extra_css.is_empty() {
        out.push_str(extra_css);
        out.push('\n');
    }
    out.push_str("</style>\n</head>\n<body>\n");
    if head.bare {
        out.push_str("<main id=\"main\" class=\"gc-bare\">\n");
    } else {
        out.push_str(
            "<header class=\"gc-top\"><div class=\"gc-top-in\"><span class=\"gc-brand\"><img src=\"",
        );
        out.push_str(LOGO_PATH);
        out.push_str("\" width=\"30\" height=\"30\" alt=\"\">Groscailloux <span class=\"gc-tv\">TV</span></span></div></header>\n");
        out.push_str("<main id=\"main\" class=\"gc-main\">\n");
    }
    out.push_str(body_html);
    out.push_str("\n</main>\n</body>\n</html>\n");
    out
}

/// `GET /assets/logo.png` : public (hors [`crate::admin_auth::required`]), mis en cache un jour.
pub async fn logo() -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            ),
        ],
        LOGO_PNG,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_escapes_title_and_keeps_body() {
        let html = page(r#"<script>"x" & 'y'"#, "<p>Corps</p>", "");
        assert!(html.contains("<title>&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;</title>"));
        assert!(!html.contains("<title><script>"));
        assert!(html.contains("<main id=\"main\" class=\"gc-main\">\n<p>Corps</p>"));
        assert!(html.starts_with("<!doctype html>\n<html lang=\"fr\">"));
        assert!(html.contains("name=\"viewport\" content=\"width=device-width,initial-scale=1\""));
        assert!(html.contains("<meta name=\"theme-color\" content=\"#08090b\">"));
        // aucune page de homelabd n'est à indexer (inscription, liens de bienvenue, administration)
        assert!(html.contains("<meta name=\"robots\" content=\"noindex\">"));
        assert!(html.contains("rel=\"icon\" type=\"image/png\" href=\"/assets/logo.png\""));
        assert!(html.contains("Groscailloux <span class=\"gc-tv\">TV</span>"));
    }

    #[test]
    fn error_page_has_title_message_and_optional_back_link() {
        let e = error_page(
            "Session <expirée>",
            "Session expirée : <a href=\"/connexion\">se reconnecter</a>.",
            None,
        );
        assert!(e.contains("<title>Session &lt;expirée&gt;</title>"));
        assert!(e.contains("<h1>Session &lt;expirée&gt;</h1>"));
        assert!(e.contains("<p>Session expirée : <a href=\"/connexion\">se reconnecter</a>.</p>"));
        assert!(e.contains("<meta name=\"referrer\" content=\"no-referrer\">"));
        assert!(e.contains("class=\"gc-top\""));
        assert!(!e.contains("class=\"btn secondary\""));
        let b = error_page("t", "m", Some(("/x?a=1&b=\"2\"", "Retour <ici>")));
        assert!(b.contains(
            "<a class=\"btn secondary\" href=\"/x?a=1&amp;b=&quot;2&quot;\">Retour &lt;ici&gt;</a>"
        ));
    }

    #[tokio::test]
    async fn error_response_keeps_the_status() {
        let r = error_response(axum::http::StatusCode::BAD_GATEWAY, "t", "m", None);
        assert_eq!(r.status(), axum::http::StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("<p>m</p>"));
    }

    #[test]
    fn description_is_escaped_and_optional() {
        let d = page_described("t", "Prix <3,50 €> & \"plus\"", "", "");
        assert!(d.contains(
            "<meta name=\"description\" content=\"Prix &lt;3,50 €&gt; &amp; &quot;plus&quot;\">"
        ));
        assert!(!page("t", "", "").contains("name=\"description\""));
    }

    #[test]
    fn refresh_is_in_the_head_and_escaped() {
        let r = page_admin("t", Some((4, "/x?a=1&b=\"2\"")), "<p>c</p>", "");
        let meta = "<meta http-equiv=\"refresh\" content=\"4;url=/x?a=1&amp;b=&quot;2&quot;\">";
        assert!(r.contains(meta), "{r}");
        assert!(r.find(meta).unwrap() < r.find("</head>").unwrap());
        assert!(!page("t", "", "").contains("http-equiv"));
    }

    #[test]
    fn admin_pages_send_no_referrer() {
        let a = page_admin("t", None, "<p>c</p>", "");
        assert!(a.contains("<meta name=\"referrer\" content=\"no-referrer\">"));
        assert!(a.contains("class=\"gc-top\"") && !a.contains("http-equiv"));
        let r = page_admin("t", Some((5, "/a?id=1&b")), "", "");
        assert!(r.contains("<meta http-equiv=\"refresh\" content=\"5;url=/a?id=1&amp;b\">"));
        assert!(!page("t", "", "").contains("name=\"referrer\""));
    }

    #[test]
    fn bare_page_has_no_header_and_reloads_the_same_address() {
        let b = page_bare("État", Some(60), "<h1>x</h1>", ".y{}");
        assert!(!b.contains("class=\"gc-top\"") && !b.contains("<header"));
        assert!(b.contains("<main id=\"main\" class=\"gc-bare\">\n<h1>x</h1>"));
        // pas de `url=` : le rechargement garde l'adresse, donc le jeton de la requête
        assert!(b.contains("<meta http-equiv=\"refresh\" content=\"60\">"));
        assert!(b.contains("no-referrer") && b.contains("--bg:#08090b"));
        assert!(!page_bare("t", None, "", "").contains("http-equiv"));
        assert!(!b.to_ascii_lowercase().contains("<script"));
    }

    #[test]
    fn page_carries_the_tokens() {
        let html = page("t", "", ".x{color:red}");
        for t in [
            "--bg:#08090b",
            "--surface:#121418",
            "--raised:#1f2228",
            "--line:#2a2d33",
            "--text:#e6e7ea",
            "--dim:#a3a7af",
            "--accent:#2f8fff",
            "--accent-hover:#5aa6ff",
            "--accent-fill:#1b6fd8",
            "--ok:#34c77b",
            "--warn:#f0b429",
            "--err:#ef4444",
            "--btn-bg:#ffffff",
            "--btn-text:#0b0b0c",
            "--r-lg:.6em",
            "--r-md:.45em",
            "--r-sm:.3em",
            "--field-line:#60656f",
            "-apple-system",
        ] {
            assert!(html.contains(t), "jeton absent : {t}");
        }
        // bouton principal blanc, graisse 700 ; focus visible ; mouvement réduit ; CSS de la page après la base
        assert!(BASE_CSS.contains("background:var(--btn-bg);color:var(--btn-text);font:700"));
        assert!(BASE_CSS.contains(":focus-visible{outline:2px solid"));
        // le focus ne change pas la forme des contrôles ; le survol du bouton bleu garde le contraste AA
        assert!(!BASE_CSS.contains("outline-offset:2px;border-radius"));
        assert!(BASE_CSS.contains("button.accent:hover{background:var(--accent-press)}"));
        assert!(BASE_CSS.contains("@media (prefers-reduced-motion:reduce)"));
        // hauteur d'écran réelle sur mobile ; barre de défilement sombre et ombres de défilement des tableaux
        assert!(BASE_CSS.contains("min-height:100dvh"));
        assert!(BASE_CSS.contains("scrollbar-color:#4a4f58 transparent"));
        assert!(BASE_CSS.contains("no-repeat local"));
        assert!(html.find(".x{color:red}") > html.find(".sr-only{"));
    }

    #[test]
    fn page_has_no_external_url_nor_leftover_placeholder() {
        let html = page("Titre", "<p>ok</p>", "");
        let lower = html.to_ascii_lowercase();
        assert!(!lower.contains("http://") && !lower.contains("https://"));
        assert!(!lower.contains("@import") && !lower.contains("url("));
        assert!(!lower.contains("<script"));
        // marque `{{…}}` restante (« }} » seul est légitime : fin d'un @media)
        assert!(!html.contains("{{"));
        // seuls les liens fournis par la page en apportent un
        let linked = page("t", "<a href=\"https://exemple.invalid/\">x</a>", "");
        assert_eq!(linked.matches("https://").count(), 1);
    }

    #[test]
    fn logo_is_the_png_and_public() {
        assert!(LOGO_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(crate::admin_auth::required(LOGO_PATH).is_none());
        assert!(!crate::admin_auth::is_admin_path(LOGO_PATH));
    }

    #[tokio::test]
    async fn logo_route_serves_png() {
        let resp = logo().await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            resp.headers()[header::CACHE_CONTROL],
            "public, max-age=86400"
        );
    }
}
