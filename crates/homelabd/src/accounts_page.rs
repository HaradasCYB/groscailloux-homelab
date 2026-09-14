//! Page « Comptes » (onboarder.<domaine>/accounts) : un interrupteur Premium par compte.
//! Rendu côté serveur, sans JavaScript : chaque interrupteur est un formulaire POST qui porte le
//! jeton d'onboarding (champ caché, ce qui protège aussi du CSRF). NPM ajoute la connexion
//! « admin-outils » devant `/accounts`.

use homelab_core::accounts::Account;

use crate::status_page::ago;

pub struct PageData<'a> {
    pub now: i64,
    pub accounts: &'a [Account],
    pub max_premium: usize,
    pub max_streams: u32,
    pub token: &'a str,
    /// Code de résultat de la dernière action (`activated`, `suspended`, `cap`, …) et compte visé.
    pub msg: Option<(&'a str, &'a str)>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn activity(now: i64, iso: Option<&str>) -> String {
    iso.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| ago(now, d.timestamp()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "jamais".into())
}

pub fn message(code: &str, who: &str, max_premium: usize) -> Option<(&'static str, String)> {
    let who = esc(who);
    Some(match code {
        "activated" => ("ok", format!("<b>{who}</b> est premium : accès ouvert.")),
        "suspended" => (
            "warn",
            format!("<b>{who}</b> est suspendu : connexion refusée, rien n'est supprimé."),
        ),
        "unchanged" => ("info", format!("<b>{who}</b> était déjà dans cet état.")),
        "cap" => (
            "err",
            format!("Plafond atteint ({max_premium} comptes premium) : suspends un compte avant d'en activer un autre."),
        ),
        "error" => (
            "err",
            format!("La modification de <b>{who}</b> a échoué (voir journalctl -u homelabd)."),
        ),
        _ => return None,
    })
}

pub fn render(d: &PageData<'_>) -> String {
    let premium = d.accounts.iter().filter(|a| a.premium).count();
    let full = premium >= d.max_premium;
    let pct = (premium * 100)
        .checked_div(d.max_premium)
        .map_or(100, |p| p.min(100));
    let bar = if full {
        "err"
    } else if pct >= 80 {
        "warn"
    } else {
        "ok"
    };
    let token = esc(d.token);
    let mut rows = String::new();
    for a in d.accounts {
        let (state, label, action) = if a.premium {
            ("on", "Premium", "0")
        } else {
            ("off", "Suspendu", "1")
        };
        let blocked = !a.premium && full;
        let title = if a.premium {
            format!("Suspendre {}", a.name)
        } else if blocked {
            "Plafond atteint".to_string()
        } else {
            format!("Activer {}", a.name)
        };
        let streams = if a.max_streams == 0 {
            "illimité".to_string()
        } else {
            format!("{} max", a.max_streams)
        };
        rows.push_str(&format!(
            r#"<tr class="{state}"><td class="n">{name}</td><td class="w">{act}</td><td class="w">{streams}</td><td class="t"><form method="post" action="/accounts/premium"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><input type="hidden" name="on" value="{action}"><button class="sw {state}" type="submit" role="switch" aria-checked="{checked}" title="{title}"{dis}><i></i><span>{label}</span></button></form></td></tr>"#,
            name = esc(&a.name),
            act = esc(&activity(d.now, a.last_activity.as_deref())),
            id = esc(&a.id),
            checked = a.premium,
            title = esc(&title),
            dis = if blocked { " disabled" } else { "" },
        ));
    }
    let flash = d
        .msg
        .and_then(|(code, who)| message(code, who, d.max_premium))
        .map(|(cls, text)| format!(r#"<p class="flash {cls}" role="status">{text}</p>"#))
        .unwrap_or_default();
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>Comptes Groscailloux</title><style>
:root{{color-scheme:dark}}*{{box-sizing:border-box}}
body{{margin:0;padding-block:20px;padding-inline:16px;background:#141517;color:#dfe5ea;font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}}
main{{max-width:760px;margin:0 auto;display:grid;gap:16px}}
h1{{font-size:18px;margin:0;font-weight:650;letter-spacing:.01em}}
.sub{{margin:2px 0 0;color:#8591a0;font-size:12.5px}}
.cap{{background:#1b1d20;border:1px solid #2a2d31;border-radius:10px;padding:12px 14px}}
.ct{{display:flex;justify-content:space-between;align-items:baseline;gap:8px;flex-wrap:wrap}}
.ct b{{font-size:22px;font-variant-numeric:tabular-nums}}.ct span{{color:#9aa6b1;font-size:12.5px}}
.bar{{height:6px;background:#2a2d31;border-radius:3px;margin-top:8px;overflow:hidden}}.bar i{{display:block;height:100%}}
i.ok{{background:#22c55e}}i.warn{{background:#f59e0b}}i.err{{background:#ef4444}}
.flash{{margin:0;padding:9px 12px;border-radius:8px;font-size:13px}}
.flash.ok{{background:#12321f;color:#a7f3c4}}.flash.warn{{background:#3a2a0e;color:#fcd38a}}.flash.err{{background:#3b1518;color:#fca5a5}}.flash.info{{background:#1d2633;color:#b9cbe3}}
.tw{{overflow-x:auto}}table{{width:100%;border-collapse:collapse}}
th{{text-align:left;font-weight:500;color:#8591a0;font-size:11.5px;letter-spacing:.06em;text-transform:uppercase;padding:0 6px 6px}}
td{{padding:8px 6px;border-top:1px solid #24272b;vertical-align:middle}}
td.n{{font-weight:550;word-break:break-word}}tr.off td.n{{color:#8591a0}}
td.w{{white-space:nowrap;color:#9aa6b1;font-variant-numeric:tabular-nums;font-size:12.5px}}td.t{{width:1%;text-align:right}}
form{{margin:0}}
.sw{{display:inline-flex;align-items:center;gap:8px;border:0;background:none;color:inherit;font:inherit;font-size:12.5px;cursor:pointer;padding:4px 2px;border-radius:999px}}
.sw i{{position:relative;width:34px;height:20px;border-radius:999px;background:#3a3f45;transition:background .15s}}
.sw i::after{{content:"";position:absolute;top:3px;left:3px;width:14px;height:14px;border-radius:50%;background:#dfe5ea;transition:transform .15s}}
.sw.on i{{background:#22c55e}}.sw.on i::after{{transform:translateX(14px)}}
.sw span{{min-width:62px;text-align:left}}.sw.off span{{color:#8591a0}}
.sw:focus-visible{{outline:2px solid #60a5fa;outline-offset:2px}}.sw:disabled{{cursor:not-allowed;opacity:.45}}
.foot{{color:#8591a0;font-size:12px;margin:0}}
@media (prefers-reduced-motion:reduce){{.sw i,.sw i::after{{transition:none}}}}
</style></head><body><main>
<header><h1>Comptes</h1><p class="sub">Premium : accès au catalogue. Suspendu : connexion refusée, historique et favoris conservés.</p></header>
<section class="cap" aria-label="Comptes premium"><div class="ct"><b>{premium} / {max}</b><span>comptes premium · {streams} lectures simultanées par compte</span></div><div class="bar"><i class="{bar}" style="width:{pct}%"></i></div></section>
{flash}
<div class="tw"><table><thead><tr><th>Compte</th><th>Dernière activité</th><th>Lectures</th><th><span hidden>Premium</span></th></tr></thead><tbody>{rows}</tbody></table></div>
<p class="foot">Les comptes administrateurs n'apparaissent pas ici. Les nouveaux comptes arrivent suspendus.</p>
</main></body></html>"#,
        max = d.max_premium,
        streams = d.max_streams,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(name: &str, premium: bool) -> Account {
        Account {
            id: format!("id-{name}"),
            name: name.into(),
            premium,
            max_streams: 2,
            last_activity: Some("1970-01-01T00:16:40Z".into()),
        }
    }

    #[test]
    fn renders_switches_counter_and_escapes() {
        let list = [acc("alice", true), acc("<bob>", false)];
        let html = render(&PageData {
            now: 1060,
            accounts: &list,
            max_premium: 25,
            max_streams: 2,
            token: "t\"k",
            msg: Some(("suspended", "<bob>")),
        });
        assert!(html.contains("1 / 25"));
        assert!(html.contains(r#"aria-checked="true""#));
        assert!(html.contains("&lt;bob&gt;"));
        assert!(!html.contains("<bob>"));
        assert!(html.contains(r#"value="t&quot;k""#));
        assert!(html.contains("il y a 1 min"));
        assert!(!html.contains(" disabled>"));
    }

    #[test]
    fn full_cap_disables_activation_only() {
        let list = [acc("alice", true), acc("bob", false)];
        let html = render(&PageData {
            now: 0,
            accounts: &list,
            max_premium: 1,
            max_streams: 2,
            token: "t",
            msg: Some(("cap", "")),
        });
        assert_eq!(html.matches(" disabled>").count(), 1);
        assert!(html.contains("Plafond atteint"));
    }
}
