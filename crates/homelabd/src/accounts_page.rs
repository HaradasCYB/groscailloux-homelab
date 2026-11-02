//! Page « Comptes » (onboarder.<domaine>/accounts) : tous les comptes Jellyfin, un interrupteur
//! Premium et un bouton Supprimer par compte non protégé. Rendu côté serveur, sans JavaScript :
//! chaque action est un formulaire POST qui porte le jeton d'onboarding (champ caché, ce qui protège
//! aussi du CSRF) ; la suppression passe par une page de confirmation. NPM ajoute la connexion
//! « admin-outils » devant `/accounts`.

use homelab_core::accounts::Account;

use crate::status_page::ago;
use homelab_core::html::esc;

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
    /// Fiche abonné par compte (`subscriptions`), si le module est activé.
    pub subs: &'a std::collections::HashMap<String, SubInfo>,
}

/// Ce que la page affiche d'une fiche abonné.
#[derive(Debug, Clone, Default)]
pub struct SubInfo {
    /// `trial`, `active`, `grace`, `suspended`, `offered`, `exempt`, `unknown`.
    pub status: String,
    pub label: String,
    /// Échéance lisible (« 12/10/2026 ») ou vide.
    pub expires: String,
    /// `paypal`, `manual`, `trial`, `import`.
    pub source: String,
    /// Fiche gérée à la main (ni essai ni PayPal) dont l'échéance est passée : « à gérer » (2026-10-08).
    pub hand_due: bool,
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
        "sub_set" => {
            let (w, until) = who.split_once('|').unwrap_or((who.as_str(), ""));
            let tail = if until.is_empty() {
                String::new()
            } else {
                format!(", actif jusqu'au {until}")
            };
            ("ok", format!("<b>{w}</b> : statut enregistré{tail}."))
        }
        "sub_extended" => {
            let (w, until) = who.split_once('|').unwrap_or((who.as_str(), ""));
            ("ok", format!("<b>{w}</b> : accès prolongé jusqu'au {until}."))
        }
        "error" => (
            "err",
            format!("L'action sur <b>{who}</b> a échoué (voir journalctl -u homelabd)."),
        ),
        _ => return None,
    })
}

/// Règles propres aux pages d'administration (`/accounts`, `/accounts/delete`, liste rouge), après
/// [`crate::ui::BASE_CSS`] : mise en page en grille (`.adm`), compteur (`.cap`), tableau des comptes, interrupteur
/// Premium (`.sw`), petits boutons de ligne (`.lnk`), liens Supprimer (`.del`), pastilles d'abonnement (`.sb`).
/// Les classes `del`, `sb …`, `lk` et `w` sont vérifiées par les tests.
pub(crate) const ADMIN_CSS: &str = r#".adm{display:grid;gap:16px}
.adm>*{margin-top:0;margin-bottom:0}
.ph{display:flex;justify-content:space-between;align-items:flex-start;gap:12px 16px;flex-wrap:wrap}
.ph h1{font-size:clamp(1.6rem,1.2rem + 1.6vw,2.1rem);margin:0 0 .25em}
.ph .lead{font-size:1rem;margin:0}
.ph nav{flex:none}
.adm h2{font-size:1.2rem;margin:8px 0 0}
.cap{padding:16px 18px}
.ct{display:flex;justify-content:space-between;align-items:baseline;gap:8px;flex-wrap:wrap}
.ct b{font:800 1.5rem/1.1 var(--font-display);letter-spacing:-.02em;color:var(--strong);font-variant-numeric:tabular-nums}
.ct span{color:var(--dim);font-size:.875rem}
.bar{height:6px;background:var(--raised);border-radius:3px;margin-top:10px;overflow:hidden}.bar i{display:block;height:100%}
.bar i.ok{background:var(--ok)}.bar i.warn{background:var(--warn)}.bar i.err{background:var(--err)}
.adm table{font-size:.875rem}
.adm td{vertical-align:middle}
td.n{font-weight:600;color:var(--strong);min-width:10em;overflow-wrap:break-word}tr.off td.n{color:var(--dim)}
td.w{white-space:nowrap;color:var(--dim);font-variant-numeric:tabular-nums}
td.sub{min-width:15em}
td.t{width:1%;white-space:nowrap}
.acts{display:flex;align-items:center;justify-content:flex-end;gap:10px}
td.t .acts{gap:6px}
form{margin:0}
td.n .pill{margin-left:6px;vertical-align:1px}
.sb{display:inline-block;padding:1px 8px;border-radius:999px;font-size:.75rem;font-weight:600;margin-bottom:4px;border:1px solid var(--line);background:var(--raised);color:#d6d8dc}
.sb.active,.sb.offered,.sb.exempt{color:var(--ok);border-color:rgba(52,199,123,.4);background:rgba(52,199,123,.1)}
.sb.trial{color:var(--accent-hover);border-color:rgba(47,143,255,.45);background:rgba(47,143,255,.1)}
.sb.grace,.sb.due{color:var(--warn);border-color:rgba(240,180,41,.4);background:rgba(240,180,41,.1)}
.sb.suspended{color:#ff8a8a;border-color:rgba(239,68,68,.45);background:rgba(239,68,68,.1)}
.lk{display:block;font-size:.75rem;color:var(--dim);margin-bottom:4px;white-space:normal}
.subf{display:flex;gap:4px;align-items:center;margin-top:2px}
.subf .lk{display:inline;margin:0}
.subf select,.subf input{width:auto;min-height:32px;padding:3px 8px;border-radius:var(--r-sm);font-size:.8rem}
.subf input{width:4.6em}
button.lnk{min-height:32px;padding:4px 10px;border-radius:var(--r-sm);font-size:.8rem;font-weight:600;background:rgba(255,255,255,.08);color:var(--strong);border-color:var(--line)}
button.lnk:hover{background:rgba(255,255,255,.14);color:var(--strong)}
button.sw{display:inline-flex;align-items:center;gap:8px;min-height:32px;border:0;background:none;color:inherit;font:inherit;font-size:.8rem;font-weight:400;padding:4px 2px;border-radius:999px}
button.sw:hover{background:none;color:var(--strong)}
.sw i{position:relative;width:34px;height:20px;border-radius:999px;background:#4a4f58;transition:background-color .15s}
.sw i::after{content:"";position:absolute;top:3px;left:3px;width:14px;height:14px;border-radius:50%;background:var(--strong);transition:transform .15s}
.sw.on i{background:var(--ok)}.sw.on i::after{transform:translateX(14px)}
.sw span{min-width:62px;text-align:left}.sw.off span{color:var(--dim)}
button.sw:disabled{cursor:not-allowed;opacity:.45}
.del{color:#ff8a8a;font-size:.8rem;text-decoration:none;padding:6px 8px;border-radius:var(--r-sm)}
.del:hover{background:rgba(239,68,68,.14);color:#ffb3b3}
.lock{color:var(--dim);font-size:.8rem}
.foot{color:var(--dim);font-size:.85rem}
.confirm{max-width:var(--measure)}
.confirm ul{margin:0 0 1em;padding-left:20px}
.confirm li{margin:2px 0}
@media (prefers-reduced-motion:reduce){.sw i,.sw i::after{transition:none}}
@media (forced-colors:active){.sw i,.sw i::after{forced-color-adjust:none}.sw i{background:Canvas;border:1px solid CanvasText}.sw i::after{background:CanvasText}.sw.on i{background:Highlight}.sw.on i::after{background:HighlightText}}"#;

/// Page d'administration sur [`crate::ui::page_admin`] (en-tête commun, `no-referrer`) : `body` dans
/// `<div class="adm">`, [`ADMIN_CSS`] puis `extra_css` (règles d'une page, liste rouge) ; `refresh` = rechargement
/// vers un chemin local (travail de la liste rouge).
pub(crate) fn shell_with(
    title: &str,
    extra_css: &str,
    refresh: Option<(u32, &str)>,
    body: &str,
) -> String {
    let css = if extra_css.is_empty() {
        ADMIN_CSS.to_string()
    } else {
        format!("{ADMIN_CSS}\n{extra_css}")
    };
    crate::ui::page_admin(
        title,
        refresh,
        &format!(r#"<div class="adm">{body}</div>"#),
        &css,
    )
}

fn shell(title: &str, body: &str) -> String {
    shell_with(title, "", None, body)
}

/// Notice de résultat d'une action : `role="alert"` pour une erreur, `status` sinon.
pub(crate) fn flash_html(cls: &str, text: &str) -> String {
    let role = if cls == "err" { "alert" } else { "status" };
    format!(r#"<p class="notice {cls}" role="{role}">{text}</p>"#)
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
            badges.push_str(r#"<span class="pill info">Admin</span>"#);
        }
        if a.protected {
            badges.push_str(r#"<span class="pill warn">Protégé</span>"#);
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
                r#"<div class="acts"><form method="post" action="/accounts/premium"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><input type="hidden" name="on" value="{action}"><button class="sw {state}" type="submit" role="switch" aria-checked="{checked}" title="{title}"{dis}><i></i><span>{label}</span></button></form><a class="del" href="/accounts/delete?user_id={id}" title="Supprimer {name}">Supprimer</a></div>"#,
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
        let sub = if a.protected {
            r#"<span class="lock">—</span>"#.to_string()
        } else {
            match d.subs.get(&a.id) {
                Some(si) => {
                    let sel = |v: &str| if si.status == v { " selected" } else { "" };
                    format!(
                        r#"<span class="sb {st}">{label}</span><span class="lk">{was}{exp}{src}</span><form method="post" action="/accounts/subs" class="subf"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><select name="action" aria-label="Décision pour {name}"><option value="extend">Prolonger de N jours</option><option value="active"{sa}>Actif pour N jours</option><option value="offered_days">Offert pour N jours</option><option value="offered"{so}>Offert sans limite</option><option value="exempt"{se}>Exempté</option><option value="unknown"{su}>À qualifier</option><option value="suspended"{ss}>Suspendu</option></select><input type="number" name="days" min="1" max="730" value="30" aria-label="N jours" title="N jours : la case revient à 30 après validation, l'échéance obtenue s'affiche à gauche et dans le message"><span class="lk">j</span><button class="lnk" type="submit">OK</button></form>"#,
                        // géré à la main, échéance passée : le compte reste actif, l'admin tranche
                        st = if si.hand_due {
                            "due".to_string()
                        } else {
                            esc(&si.status)
                        },
                        label = if si.hand_due {
                            "À gérer (échéance passée)".to_string()
                        } else {
                            esc(&si.label)
                        },
                        was = if si.hand_due {
                            format!("{} · ", esc(&si.label))
                        } else {
                            String::new()
                        },
                        exp = if si.expires.is_empty() {
                            String::new()
                        } else if si.hand_due {
                            format!("échéance du {} · ", esc(&si.expires))
                        } else {
                            format!("jusqu'au {} · ", esc(&si.expires))
                        },
                        src = esc(&si.source),
                        id = esc(&a.id),
                        name = esc(&a.name),
                        sa = sel("active"),
                        so = sel("offered"),
                        se = sel("exempt"),
                        su = sel("unknown"),
                        ss = sel("suspended"),
                    )
                }
                None => r#"<span class="lock">pas de fiche</span>"#.to_string(),
            }
        };
        rows.push_str(&format!(
            r#"<tr class="{state}"><td class="n">{name}{badges}</td><td class="w">{act}</td><td class="w">{streams}</td><td class="w sub">{sub}</td><td class="w">{link}</td><td class="t">{actions}</td></tr>"#,
            name = esc(&a.name),
            act = esc(&activity(d.now, a.last_activity.as_deref())),
        ));
    }
    let flash = d
        .msg
        .and_then(|(code, who)| message(code, who, d.max_premium))
        .map(|(cls, text)| flash_html(cls, &text))
        .unwrap_or_default();
    shell(
        "Comptes Groscailloux",
        &format!(
            r#"<div class="ph"><div><h1>Comptes</h1><p class="lead">Premium : accès au catalogue. Suspendu : connexion refusée, historique et favoris conservés.</p></div><nav class="row" aria-label="Administration"><a class="btn secondary" href="/accounts/liste-rouge">Liste rouge</a><a class="btn" href="/">Créer un compte</a></nav></div>
<section class="card cap" aria-label="Comptes premium"><div class="ct"><b>{premium} / {max}</b><span>comptes premium · {streams} lectures simultanées par compte</span></div><div class="bar"><i class="{bar}" style="width:{pct}%"></i></div></section>
{flash}
<div class="table-wrap"><table><thead><tr><th scope="col">Compte</th><th scope="col">Dernière activité</th><th scope="col">Lectures</th><th scope="col">Abonnement</th><th scope="col">Lien de bienvenue</th><th scope="col"><span class="sr-only">Actions</span></th></tr></thead><tbody>{rows}</tbody></table></div>
<p class="foot">Abonnement : « Actif (période) » pose une échéance de N jours (30 par défaut), « Prolonger » l'ajoute. Le cycle (rappels, grâce, suspension) ne suit que les essais et les abonnements PayPal ; un membre qui paie sur PayPal est rattaché et prolongé tout seul. Toute autre fiche (actif ou offert par ta décision, exempté, à qualifier) n'est jamais suspendue et le membre ne reçoit aucun rappel : à l'échéance, tu reçois une information et la fiche passe « À gérer (échéance passée) », compte laissé actif. « À qualifier » = compte actif sans abonnement connu.</p>
<p class="foot">Les comptes protégés ne se gèrent que dans le tableau de bord Jellyfin et ne comptent pas dans le plafond. Les nouveaux comptes arrivent suspendus (sauf une inscription publique dont l'essai a démarré) ; à l'activation, le membre reçoit un mail. « Renvoyer le lien » envoie un nouveau lien de bienvenue (définir ou changer son mot de passe).</p>"#,
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
            r#"<div class="ph"><div><h1>Supprimer « {name} » ?</h1><p class="lead">Cette action est définitive.</p></div></div>
<section class="card confirm" aria-label="Confirmation">
<p>Seront supprimés :</p>
<ul><li>le compte Jellyfin <b>{name}</b> (historique, favoris, reprise de lecture) ;</li><li>son compte Jellyseerr et ses demandes. Ce qui est déjà téléchargé reste dans la bibliothèque.</li></ul>
<p class="muted">Pour bloquer l'accès sans rien perdre, suspends plutôt le compte.</p>
<div class="row"><form method="post" action="/accounts/delete"><input type="hidden" name="token" value="{token}"><input type="hidden" name="user_id" value="{id}"><button class="btn danger" type="submit">Supprimer définitivement</button></form><a class="btn secondary" href="/accounts">Annuler</a></div>
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
            subs: &std::collections::HashMap::new(),
        });
        assert!(html.contains("1 / 25"));
        assert!(html.contains(r#"aria-checked="true""#));
        assert!(html.contains("&lt;bob&gt;"));
        assert!(!html.contains("<bob>"));
        assert!(html.contains(r#"value="t&quot;k""#));
        assert!(html.contains(r#"href="/""#));
        assert!(!html.contains("?token="));
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
            subs: &std::collections::HashMap::new(),
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
            subs: &std::collections::HashMap::new(),
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
    fn hand_managed_fiche_past_its_expiry_shows_to_manage() {
        let list = [acc("alice", true), acc("bob", true)];
        let mut subs = std::collections::HashMap::new();
        subs.insert(
            "id-alice".to_string(),
            SubInfo {
                status: "active".into(),
                label: "Actif".into(),
                expires: "20/10/2026".into(),
                source: "manual".into(),
                hand_due: true,
            },
        );
        subs.insert(
            "id-bob".to_string(),
            SubInfo {
                status: "active".into(),
                label: "Actif".into(),
                expires: "21/10/2026".into(),
                source: "paypal".into(),
                hand_due: false,
            },
        );
        let html = render(&PageData {
            now: 0,
            accounts: &list,
            max_premium: 25,
            max_playbacks: 2,
            token: "t",
            msg: None,
            links: &std::collections::HashMap::new(),
            subs: &subs,
        });
        assert!(html.contains(
            r#"<span class="sb due">À gérer (échéance passée)</span><span class="lk">Actif · échéance du 20/10/2026 · manual</span>"#
        ));
        assert!(html.contains(
            r#"<span class="sb active">Actif</span><span class="lk">jusqu'au 21/10/2026 · paypal</span>"#
        ));
        assert_eq!(html.matches("À gérer (échéance passée)</span>").count(), 1);
        // le pied de page ne promet plus qu'« offert » échappe seul au cycle
        assert!(html.contains("ne suit que les essais et les abonnements PayPal"));
        // la décision en cours reste présélectionnée dans la liste
        assert_eq!(
            html.matches(r#"<option value="active" selected>"#).count(),
            2
        );
    }

    /// Nouvelle DA (`ui::page_admin`) : en-tête commun, `no-referrer`, aucun reste violet ni script, et les mêmes
    /// formulaires qu'avant (interrupteur + lien de bienvenue par compte non protégé, tous en POST).
    #[test]
    fn page_uses_the_shared_design_and_keeps_its_forms() {
        let list = [acc("alice", true), acc("bob", false)];
        let html = render(&PageData {
            now: 0,
            accounts: &list,
            max_premium: 25,
            max_playbacks: 2,
            token: "t",
            msg: Some(("cap", "")),
            links: &std::collections::HashMap::new(),
            subs: &std::collections::HashMap::new(),
        });
        assert!(html.contains(r#"class="gc-top""#) && html.contains("--bg:#08090b"));
        assert!(html.contains(r#"<meta name="referrer" content="no-referrer">"#));
        assert_eq!(html.matches("<main").count(), 1);
        assert_eq!(html.matches("<form").count(), 4);
        assert_eq!(html.matches(r#"<form method="post""#).count(), 4);
        assert!(!html.contains("<script"));
        for violet in ["#3a325a", "#c4b5fd", "#16121f", "#9b94b8", "#e6e1f5"] {
            assert!(!html.contains(violet), "{violet}");
        }
        assert!(html.contains(r#"<p class="notice err" role="alert">Plafond atteint"#));
        let confirm = render_confirm(&acc("bob", false), "t");
        assert_eq!(confirm.matches("<form").count(), 1);
        assert!(confirm.contains(r#"<button class="btn danger" type="submit">"#));
        assert!(confirm.contains("no-referrer"));
    }

    #[test]
    fn confirm_page_posts_the_deletion() {
        let html = render_confirm(&acc("bob", false), "t");
        assert!(html.contains(r#"action="/accounts/delete""#));
        assert!(html.contains(r#"value="id-bob""#));
        assert!(html.contains("Supprimer définitivement"));
        assert!(html.contains(r#"href="/accounts""#));
        assert!(!html.contains("?token="));
    }
}
