//! Page « Comptes » (onboarder.<domaine>/accounts) : tous les comptes Jellyfin, un interrupteur
//! Premium et un bouton Supprimer par compte non protégé. Rendu côté serveur, sans JavaScript :
//! chaque action est un formulaire POST qui porte le jeton d'onboarding (champ caché, ce qui protège
//! aussi du CSRF) ; la suppression passe par une page de confirmation. NPM ajoute la connexion
//! « admin-outils » devant `/accounts`.

use homelab_core::accounts::Account;

use crate::status_page::ago;

pub struct PageData<'a> {
    pub now: i64,
    pub accounts: &'a [Account],
    pub max_premium: usize,
    /// Lectures simultanées par compte (`accounts.max_playbacks_per_user`, 0 = illimité).
    pub max_playbacks: usize,
    pub token: &'a str,
    /// Code de résultat de la dernière action (`activated`, `suspended`, `cap`, …) et compte visé.
    pub msg: Option<(&'a str, &'a str)>,
    /// État du lien de bienvenue par compte (`welcome::LinkStatus`), texte prêt à afficher.
    pub links: &'a std::collections::HashMap<String, String>,
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
        "deleted" => (
            "ok",
            format!("<b>{who}</b> est supprimé (Jellyfin et Jellyseerr)."),
        ),
        "protected" => ("err", format!("<b>{who}</b> est un compte protégé.")),
        "link_sent" => (
            "ok",
            format!("Nouveau lien de bienvenue envoyé à <b>{who}</b> (valable une heure)."),
        ),
        "link_failed" => (
            "err",
            format!("Le lien pour <b>{who}</b> n'est pas parti (SMTP ou adresse inconnue ; voir journalctl -u homelabd)."),
        ),
        "error" => (
            "err",
            format!("L'action sur <b>{who}</b> a échoué (voir journalctl -u homelabd)."),
        ),
        _ => return None,
    })
}

const CSS: &str = r#".lk{display:block;font-size:12px;color:#9b94b8;margin-bottom:4px}.lnk{background:none;border:1px solid #3a325a;color:#c4b5fd;border-radius:8px;padding:4px 8px;font:inherit;font-size:12px;cursor:pointer}
:root{color-scheme:dark}*{box-sizing:border-box}
body{margin:0;padding-block:20px;padding-inline:16px;background:#141517;color:#dfe5ea;font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
main{max-width:800px;margin:0 auto;display:grid;gap:16px}
h1{font-size:18px;margin:0;font-weight:650;letter-spacing:.01em}
.sub{margin:2px 0 0;color:#8591a0;font-size:12.5px}
.cap{background:#1b1d20;border:1px solid #2a2d31;border-radius:10px;padding:12px 14px}
.ct{display:flex;justify-content:space-between;align-items:baseline;gap:8px;flex-wrap:wrap}
.ct b{font-size:22px;font-variant-numeric:tabular-nums}.ct span{color:#9aa6b1;font-size:12.5px}
.bar{height:6px;background:#2a2d31;border-radius:3px;margin-top:8px;overflow:hidden}.bar i{display:block;height:100%}
i.ok{background:#22c55e}i.warn{background:#f59e0b}i.err{background:#ef4444}
.flash{margin:0;padding:9px 12px;border-radius:8px;font-size:13px}
.flash.ok{background:#12321f;color:#a7f3c4}.flash.warn{background:#3a2a0e;color:#fcd38a}.flash.err{background:#3b1518;color:#fca5a5}.flash.info{background:#1d2633;color:#b9cbe3}
.tw{overflow-x:auto}table{width:100%;border-collapse:collapse}
th{text-align:left;font-weight:500;color:#8591a0;font-size:11.5px;letter-spacing:.06em;text-transform:uppercase;padding:0 6px 6px}
td{padding:8px 6px;border-top:1px solid #24272b;vertical-align:middle}
td.n{font-weight:550;word-break:break-word}tr.off td.n{color:#8591a0}
td.w{white-space:nowrap;color:#9aa6b1;font-variant-numeric:tabular-nums;font-size:12.5px}
td.t{width:1%;white-space:nowrap}.acts{display:flex;align-items:center;justify-content:flex-end;gap:10px}
form{margin:0}
.badge{display:inline-block;margin-left:6px;padding:0 7px;border-radius:999px;font-size:11px;font-weight:500;vertical-align:1px}
.badge.admin{background:#1d2633;color:#b9cbe3}.badge.prot{background:#2a2410;color:#fcd38a}
.sw{display:inline-flex;align-items:center;gap:8px;border:0;background:none;color:inherit;font:inherit;font-size:12.5px;cursor:pointer;padding:4px 2px;border-radius:999px}
.sw i{position:relative;width:34px;height:20px;border-radius:999px;background:#3a3f45;transition:background .15s}
.sw i::after{content:"";position:absolute;top:3px;left:3px;width:14px;height:14px;border-radius:50%;background:#dfe5ea;transition:transform .15s}
.sw.on i{background:#22c55e}.sw.on i::after{transform:translateX(14px)}
.sw span{min-width:62px;text-align:left}.sw.off span{color:#8591a0}
.sw:focus-visible,.del:focus-visible,.btn:focus-visible{outline:2px solid #60a5fa;outline-offset:2px}.sw:disabled{cursor:not-allowed;opacity:.45}
.del{color:#f19999;font-size:12.5px;text-decoration:none;padding:4px 6px;border-radius:6px}.del:hover{background:#3b1518;color:#fca5a5}
.lock{color:#8591a0;font-size:12.5px}
.foot{color:#8591a0;font-size:12px;margin:0}
header{display:flex;justify-content:space-between;align-items:flex-start;gap:12px;flex-wrap:wrap}
.new{flex:none;padding:7px 12px;border-radius:8px;background:#0ea5e9;color:#04121c;font-weight:600;font-size:13px;text-decoration:none}
.new:hover{background:#38bdf8}.new:focus-visible{outline:2px solid #60a5fa;outline-offset:2px}
.card{background:#1b1d20;border:1px solid #3b1f22;border-radius:12px;padding:18px;display:grid;gap:12px;max-width:520px}
.card ul{margin:0;padding-left:18px;color:#c3ccd5}.row{display:flex;gap:10px;flex-wrap:wrap}
.btn{border:0;border-radius:8px;padding:8px 14px;font:inherit;font-weight:600;cursor:pointer;text-decoration:none}
.btn.danger{background:#dc2626;color:#fff}.btn.danger:hover{background:#ef4444}.btn.ghost{background:#2a2d31;color:#dfe5ea}
@media (prefers-reduced-motion:reduce){.sw i,.sw i::after{transition:none}}"#;

fn shell(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title}</title><style>{CSS}</style></head><body><main>{body}</main></body></html>"#
    )
}

pub fn render(d: &PageData<'_>) -> String {
    let premium = d
        .accounts
        .iter()
        .filter(|a| a.premium && !a.protected)
        .count();
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
        let state = if a.premium { "on" } else { "off" };
        let mut badges = String::new();
        if a.admin {
            badges.push_str(r#"<span class="badge admin">Admin</span>"#);
        }
        if a.protected {
            badges.push_str(r#"<span class="badge prot">Protégé</span>"#);
        }
        let streams = if a.protected || d.max_playbacks == 0 {
            "illimité".to_string()
        } else {
            format!("{} max", d.max_playbacks)
        };
        let actions = if a.protected {
            r#"<span class="lock">Géré dans Jellyfin</span>"#.to_string()
        } else {
            let (label, action) = if a.premium {
                ("Premium", "0")
            } else {
                ("Suspendu", "1")
            };
            let blocked = !a.premium && full;
            let title = if a.premium {
                format!("Suspendre {}", a.name)
            } else if blocked {
                "Plafond atteint".to_string()
            } else {
                format!("Activer {}", a.name)
            };
            format!(
                r#"<div class="acts"><form method="post" action="/accounts/premium"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><input type="hidden" name="on" value="{action}"><button class="sw {state}" type="submit" role="switch" aria-checked="{checked}" title="{title}"{dis}><i></i><span>{label}</span></button></form><a class="del" href="/accounts/delete?token={token}&amp;user_id={id}" title="Supprimer {name}">Supprimer</a></div>"#,
                id = esc(&a.id),
                name = esc(&a.name),
                checked = a.premium,
                title = esc(&title),
                dis = if blocked { " disabled" } else { "" },
            )
        };
        let link = if a.protected {
            String::new()
        } else {
            format!(
                r#"<span class="lk">{status}</span><form method="post" action="/accounts/link"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><button class="lnk" type="submit" title="Renvoyer un lien de bienvenue à {name}">Renvoyer le lien</button></form>"#,
                status = esc(d.links.get(&a.id).map(String::as_str).unwrap_or("—")),
                id = esc(&a.id),
                name = esc(&a.name),
            )
        };
        rows.push_str(&format!(
            r#"<tr class="{state}"><td class="n">{name}{badges}</td><td class="w">{act}</td><td class="w">{streams}</td><td class="w">{link}</td><td class="t">{actions}</td></tr>"#,
            name = esc(&a.name),
            act = esc(&activity(d.now, a.last_activity.as_deref())),
        ));
    }
    let flash = d
        .msg
        .and_then(|(code, who)| message(code, who, d.max_premium))
        .map(|(cls, text)| format!(r#"<p class="flash {cls}" role="status">{text}</p>"#))
        .unwrap_or_default();
    shell(
        "Comptes Groscailloux",
        &format!(
            r#"<header><div><h1>Comptes</h1><p class="sub">Premium : accès au catalogue. Suspendu : connexion refusée, historique et favoris conservés.</p></div><a class="new" href="/?token={token}">Créer un compte</a></header>
<section class="cap" aria-label="Comptes premium"><div class="ct"><b>{premium} / {max}</b><span>comptes premium · {streams} lectures simultanées par compte</span></div><div class="bar"><i class="{bar}" style="width:{pct}%"></i></div></section>
{flash}
<div class="tw"><table><thead><tr><th>Compte</th><th>Dernière activité</th><th>Lectures</th><th>Lien de bienvenue</th><th><span hidden>Actions</span></th></tr></thead><tbody>{rows}</tbody></table></div>
<p class="foot">Les comptes protégés ne se gèrent que dans le tableau de bord Jellyfin et ne comptent pas dans le plafond. Les nouveaux comptes arrivent suspendus ; à l'activation, le membre reçoit un mail. « Renvoyer le lien » envoie un nouveau lien de bienvenue (définir ou changer son mot de passe).</p>"#,
            max = d.max_premium,
            streams = d.max_playbacks,
        ),
    )
}

/// Page de confirmation avant suppression définitive d'un compte.
pub fn render_confirm(a: &Account, token: &str) -> String {
    let name = esc(&a.name);
    let token = esc(token);
    shell(
        "Supprimer un compte",
        &format!(
            r#"<header><div><h1>Supprimer « {name} » ?</h1><p class="sub">Cette action est définitive.</p></div></header>
<section class="card" aria-label="Confirmation">
<p>Seront supprimés :</p>
<ul><li>le compte Jellyfin <b>{name}</b> (historique, favoris, reprise de lecture) ;</li><li>son compte Jellyseerr et ses demandes. Ce qui est déjà téléchargé reste dans la bibliothèque.</li></ul>
<p class="sub">Pour bloquer l'accès sans rien perdre, suspends plutôt le compte.</p>
<div class="row"><form method="post" action="/accounts/delete"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><button class="btn danger" type="submit">Supprimer définitivement</button></form><a class="btn ghost" href="/accounts?token={token}">Annuler</a></div>
</section>"#,
            id = esc(&a.id),
        ),
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
            admin: false,
            protected: false,
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
            max_playbacks: 2,
            token: "t\"k",
            msg: Some(("suspended", "<bob>")),
            links: &std::collections::HashMap::new(),
        });
        assert!(html.contains("1 / 25"));
        assert!(html.contains(r#"aria-checked="true""#));
        assert!(html.contains("&lt;bob&gt;"));
        assert!(!html.contains("<bob>"));
        assert!(html.contains(r#"value="t&quot;k""#));
        assert!(html.contains(r#"href="/?token=t&quot;k""#));
        assert!(html.contains("il y a 1 min"));
        assert!(!html.contains(" disabled>"));
        assert_eq!(html.matches(r#"class="del""#).count(), 2);
    }

    #[test]
    fn full_cap_disables_activation_only() {
        let list = [acc("alice", true), acc("bob", false)];
        let html = render(&PageData {
            now: 0,
            accounts: &list,
            max_premium: 1,
            max_playbacks: 2,
            token: "t",
            msg: Some(("cap", "")),
            links: &std::collections::HashMap::new(),
        });
        assert_eq!(html.matches(" disabled>").count(), 1);
        assert!(html.contains("Plafond atteint"));
    }

    #[test]
    fn protected_accounts_have_no_controls_and_are_not_counted() {
        let mut boss = acc("Haradas", true);
        boss.admin = true;
        boss.protected = true;
        let mut px = acc("Paul", true);
        px.admin = true;
        let list = [boss, px];
        let html = render(&PageData {
            now: 0,
            accounts: &list,
            max_premium: 25,
            max_playbacks: 2,
            token: "t",
            msg: None,
            links: &std::collections::HashMap::new(),
        });
        assert!(
            html.contains("1 / 25"),
            "le compte protégé est hors plafond"
        );
        assert!(html.contains("Protégé"));
        assert_eq!(
            html.matches(r#"class="del""#).count(),
            1,
            "seul Paul est supprimable"
        );
        assert!(!html.contains("user_id=id-Haradas"));
        assert!(!html.contains(r#"value="id-Haradas""#));
    }

    #[test]
    fn confirm_page_posts_the_deletion() {
        let html = render_confirm(&acc("bob", false), "t");
        assert!(html.contains(r#"action="/accounts/delete""#));
        assert!(html.contains(r#"value="id-bob""#));
        assert!(html.contains("Supprimer définitivement"));
        assert!(html.contains(r#"href="/accounts?token=t""#));
    }
}
