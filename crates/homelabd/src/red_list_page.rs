//! Page « Liste rouge » (`/accounts/liste-rouge`) : titres que personne n'a lancés depuis `min_idle_days` jours, des
//! plus gros aux plus petits (`tasks::red_list`), avec Exempter / Retirer l'exemption, et « Nettoyer »
//! (`red_list_clean::clean`) après une confirmation côté serveur. Rendu côté serveur, sans JavaScript : chaque action
//! est un formulaire POST qui porte le jeton (champ caché, masqué par `admin_auth`) ; la page d'un travail se recharge
//! seule toutes les 5 s tant qu'il tourne. Sous `/accounts/` : session admin (`admin_auth::required`) et liste d'accès
//! NPM existante.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use homelab_core::html::esc;
use homelab_core::red_list_clean;
use homelab_core::state::{now, CleanJob, CleanResult, Exemption, RedEntry, RedListReport};
use homelab_core::subscription_ops::date_text;
use homelab_core::tasks::catalogue_report::human_size;
use homelab_core::tasks::red_list::{self, parse_key, regroup};
use homelab_core::TaskContext;
use serde::Deserialize;
use tracing::{info, warn};

use crate::accounts_page::shell_with;
use crate::status_page::ago;

const BASE: &str = "/accounts/liste-rouge";
const TASK: &str = "red_list";

const CSS: &str = r#"<style>
.badge.ru{background:#3b1f22;color:#fca5a5}.badge.sim{background:#1d2633;color:#b9cbe3}
h2{font-size:15px;margin:6px 0 4px;font-weight:620}
.why{display:block;color:#9aa6b1;font-size:12px;font-weight:400}
details summary{cursor:pointer;color:#c3ccd5}
.row form{display:inline}
.card{max-width:none}
.card li{margin:2px 0}
td.t .acts{gap:6px}
</style>"#;

#[derive(Clone)]
struct PageState {
    ctx: Arc<TaskContext>,
}

pub fn router(ctx: Arc<TaskContext>) -> Router {
    Router::new()
        .route(BASE, get(list))
        .route("/accounts/liste-rouge/exempter", post(exempt))
        .route("/accounts/liste-rouge/retirer", post(unexempt))
        .route(
            "/accounts/liste-rouge/nettoyer",
            get(confirm).post(start_clean),
        )
        .route("/accounts/liste-rouge/travail", get(job_page))
        .route("/accounts/liste-rouge/recalculer", post(recompute))
        .with_state(PageState { ctx })
}

// ---------------------------------------------------------------------------------------------
// Décisions (pures, testées)
// ---------------------------------------------------------------------------------------------

/// Ce que « Nettoyer » propose : le titre `key` seul (s'il est rouge et non exempté), sinon les `max` premiers rouges.
/// `Err` = code de message (`nolist`, `incomplete`, `notred`, `empty`).
pub fn selection<'a>(
    report: Option<&'a RedListReport>,
    exemptions: &BTreeMap<String, Exemption>,
    key: Option<&str>,
    max: usize,
) -> Result<Vec<&'a RedEntry>, &'static str> {
    let Some(r) = report else {
        return Err("nolist");
    };
    if !r.complete {
        return Err("incomplete");
    }
    let picked: Vec<&RedEntry> = match key {
        Some(k) => red_list_clean::proposed(r, exemptions, usize::MAX)
            .into_iter()
            .filter(|e| e.key == k)
            .take(1)
            .collect(),
        None => red_list_clean::proposed(r, exemptions, max),
    };
    if picked.is_empty() {
        return Err(if key.is_some() { "notred" } else { "empty" });
    }
    Ok(picked)
}

/// POST « Nettoyer » : refusé si un travail tourne (`busy`) ou si l'empreinte relue dans l'état n'est plus celle de la
/// page confirmée (`changed`). Rend les clés et l'empreinte à confier au moteur.
pub fn check_start(
    report: Option<&RedListReport>,
    exemptions: &BTreeMap<String, Exemption>,
    key: Option<&str>,
    max: usize,
    given_snap: &str,
    busy: bool,
) -> Result<(Vec<String>, String), &'static str> {
    if busy {
        return Err("busy");
    }
    let picked = selection(report, exemptions, key, max)?;
    let keys: Vec<String> = picked.iter().map(|e| e.key.clone()).collect();
    let snap = match report {
        Some(r) => red_list_clean::snap(r, &keys),
        None => return Err("nolist"),
    };
    if snap != given_snap {
        return Err("changed");
    }
    Ok((keys, snap))
}

/// Simulation demandée par la page : la liste suit `clean_dry_run` ; un titre seul (`?key=`) est l'essai réel.
fn requested_dry(single: bool, clean_dry_run: bool) -> bool {
    !single && clean_dry_run
}

// ---------------------------------------------------------------------------------------------
// Rendu
// ---------------------------------------------------------------------------------------------

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn message(code: &str, who: &str) -> Option<(&'static str, String)> {
    let who = esc(who);
    Some(match code {
        "exempted" => (
            "ok",
            format!("<b>{who}</b> est exempté : il ne sera jamais proposé au nettoyage."),
        ),
        "unexempted" => (
            "ok",
            format!("<b>{who}</b> n'est plus exempté : il reprend sa place selon sa dernière activité."),
        ),
        "unchanged" => ("info", format!("Rien à changer pour <b>{who}</b>.")),
        "unknown" => ("err", "Titre inconnu.".to_string()),
        "changed" => (
            "err",
            "La liste a changé depuis la confirmation (nouveau calcul, exemption, taille) : rien n'est lancé, relis la nouvelle liste.".to_string(),
        ),
        "busy" => (
            "warn",
            "Un « Nettoyer » ou un recalcul est déjà en cours : réessaie quand il aura fini.".to_string(),
        ),
        "incomplete" => (
            "err",
            "Le dernier calcul est incomplet : « Nettoyer » est refusé. Recalcule d'abord.".to_string(),
        ),
        "nolist" => ("info", "Pas encore de liste : lance un recalcul.".to_string()),
        "empty" => ("info", "Aucun titre rouge à nettoyer.".to_string()),
        "notred" => (
            "warn",
            format!("<b>{who}</b> n'est pas (ou plus) dans la liste rouge."),
        ),
        "recompute" => (
            "ok",
            "Recalcul lancé en arrière-plan (quelques minutes) : recharge la page ensuite.".to_string(),
        ),
        "error" => (
            "err",
            "L'action a échoué (voir journalctl -u homelabd).".to_string(),
        ),
        _ => return None,
    })
}

fn flash(msg: Option<(&str, &str)>) -> String {
    msg.and_then(|(code, who)| message(code, who))
        .map(|(cls, text)| format!(r#"<p class="flash {cls}" role="status">{text}</p>"#))
        .unwrap_or_default()
}

fn sides(e: &RedEntry) -> String {
    if e.sides.is_empty() {
        return "—".into();
    }
    e.sides
        .iter()
        .map(|s| s.side.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

fn date_or(t: Option<i64>, none: &str) -> String {
    t.map(date_text).unwrap_or_else(|| none.to_string())
}

fn last_played(e: &RedEntry) -> String {
    match (e.last_played, e.undated_since) {
        (Some(t), _) => date_text(t),
        (None, Some(_)) => "vu sans date".into(),
        _ => "jamais".into(),
    }
}

fn title_cell(e: &RedEntry) -> String {
    format!(
        r#"{title}{ru}<span class="why">{kind} · {sides}</span>"#,
        title = esc(&e.title),
        ru = if e.russian {
            r#"<span class="badge ru">voie russe</span>"#
        } else {
            ""
        },
        kind = esc(&e.kind),
        sides = esc(&sides(e)),
    )
}

fn key_form(action: &str, token: &str, key: &str, label: &str, title: &str) -> String {
    format!(
        r#"<form method="post" action="{BASE}/{action}"><input type="hidden" name="token" value="{token}"><input type="hidden" name="key" value="{key}"><button class="lnk" type="submit" title="{title}">{label}</button></form>"#,
        token = esc(token),
        key = esc(key),
        title = esc(title),
    )
}

fn job_status(j: &CleanJob) -> &'static str {
    match j.status.as_str() {
        "running" => "en cours",
        "done" => "terminé",
        "interrupted" => "interrompu",
        "refused" => "refusé",
        "failed" => "en échec",
        _ => "inconnu",
    }
}

fn job_href(id: &str) -> String {
    esc(&format!("{BASE}/travail?id={}", urlencode(id)))
}

pub struct ListData<'a> {
    pub now: i64,
    pub report: Option<&'a RedListReport>,
    pub exemptions: &'a BTreeMap<String, Exemption>,
    pub jobs: &'a [CleanJob],
    pub pending: usize,
    pub token: &'a str,
    pub msg: Option<(&'a str, &'a str)>,
    pub clean_dry_run: bool,
    pub global_dry: bool,
    pub busy: bool,
    pub max: usize,
}

pub fn render_list(d: &ListData<'_>) -> String {
    let token = esc(d.token);
    let mut out = String::new();
    out.push_str(&format!(
        r#"<header><div><h1>Liste rouge</h1><p class="sub">Titres que personne n'a lancés depuis {days} j, des plus gros aux plus petits. « Exempter » les garde pour toujours ; « Nettoyer » supprime au plus {max} titres par clic, après confirmation.</p></div><nav class="row"><a class="btn ghost" href="/accounts">Comptes</a><a class="btn ghost" href="/status.html">État</a></nav></header>"#,
        days = d.report.map_or(0, |r| r.min_idle_days),
        max = d.max,
    ));
    out.push_str(&flash(d.msg));
    if d.global_dry {
        out.push_str(r#"<p class="flash info"><span class="badge sim">Simulation</span> homelabd tourne en dry-run : rien n'est supprimé, même un titre seul.</p>"#);
    } else if d.clean_dry_run {
        out.push_str(r#"<p class="flash info"><span class="badge sim">Simulation</span> <code>clean_dry_run = true</code> : « Nettoyer » sur la liste ne supprime rien. Seul « Nettoyer ce titre » (un titre, sa propre confirmation) supprime pour de vrai : c'est l'essai réel.</p>"#);
    }
    let running = d.jobs.iter().rev().find(|j| j.status == "running");
    if d.busy || running.is_some() {
        let link = running
            .map(|j| format!(r#" <a href="{}">Suivre le travail</a>"#, job_href(&j.id)))
            .unwrap_or_default();
        out.push_str(&format!(
            r#"<p class="flash warn">Un « Nettoyer » ou un recalcul est en cours.{link}</p>"#
        ));
    }
    if d.pending > 0 {
        out.push_str(&format!(
            r#"<p class="flash info">{} étape(s) de nettoyage à rejouer (corbeille, Jellyseerr) : homelabd les reprend à chaque passage.</p>"#,
            d.pending
        ));
    }
    let recalc = format!(
        r#"<form method="post" action="{BASE}/recalculer"><input type="hidden" name="token" value="{token}"><button class="btn ghost" type="submit"{dis}>Recalculer</button></form>"#,
        dis = if d.busy { " disabled" } else { "" },
    );
    let Some(r) = d.report else {
        out.push_str(&format!(
            r#"<section class="cap"><p>Pas encore calculée (une fois par jour).</p><div class="row">{recalc}</div></section>"#
        ));
        return shell_with("Liste rouge", CSS, &out);
    };
    if !r.complete {
        let missing: String = r
            .missing
            .iter()
            .map(|m| format!("<li>{}</li>", esc(m)))
            .collect();
        out.push_str(&format!(
            r#"<div class="flash err"><b>Calcul incomplet</b> : une lecture a pu échapper au calcul, « Nettoyer » est refusé.<ul>{missing}</ul></div>"#
        ));
    }
    let total: u64 = r.entries.iter().map(|e| e.bytes).sum();
    let can_clean = r.complete && !r.entries.is_empty() && !d.busy;
    let clean_link = if can_clean {
        format!(
            r#"<a class="btn danger" href="{BASE}/nettoyer">Nettoyer les {n} plus gros…</a>"#,
            n = r.entries.len().min(d.max)
        )
    } else {
        String::new()
    };
    out.push_str(&format!(
        r#"<section class="cap" aria-label="Totaux"><div class="ct"><b>{n} titre(s) · {size}</b><span>calculé {at} · {ev} titre(s) évalué(s) · {soon} bientôt rouge(s) · {ex} exempté(s)</span></div><div class="row" style="margin-top:10px">{clean_link}{recalc}</div></section>"#,
        n = r.entries.len(),
        size = esc(&human_size(total)),
        at = esc(&ago(d.now, r.at)),
        ev = r.evaluated,
        soon = r.soon.len(),
        ex = d.exemptions.len(),
    ));
    // rouges
    if r.entries.is_empty() {
        out.push_str(r#"<p class="foot">Aucun titre rouge.</p>"#);
    } else {
        let rows: String = r
            .entries
            .iter()
            .map(|e| {
                let actions = if e.key.is_empty() {
                    String::new()
                } else {
                    let one = if can_clean {
                        format!(
                            r#"<a class="del" href="{}">Nettoyer ce titre</a>"#,
                            esc(&format!("{BASE}/nettoyer?key={}", urlencode(&e.key)))
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        r#"<div class="acts">{}{one}</div>"#,
                        key_form(
                            "exempter",
                            d.token,
                            &e.key,
                            "Exempter",
                            &format!("Exempter {}", e.title)
                        )
                    )
                };
                format!(
                    r#"<tr><td class="n">{title}<span class="why">{why}</span></td><td class="w">{size}</td><td class="w">{arr}</td><td class="w">{last}</td><td class="t">{actions}</td></tr>"#,
                    title = title_cell(e),
                    why = esc(&e.reason),
                    size = esc(&human_size(e.bytes)),
                    arr = esc(&date_or(e.arrived, "—")),
                    last = esc(&last_played(e)),
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<div class="tw"><table><thead><tr><th>Titre</th><th>Taille</th><th>Arrivée</th><th>Dernière lecture</th><th><span hidden>Actions</span></th></tr></thead><tbody>{rows}</tbody></table></div>"#
        ));
    }
    // bientôt rouges
    if !r.soon.is_empty() {
        let rows: String = r
            .soon
            .iter()
            .map(|e| {
                format!(
                    r#"<tr><td class="n">{title}<span class="why">{why}</span></td><td class="w">{size}</td><td class="w">rouge le {due}</td><td class="t">{form}</td></tr>"#,
                    title = title_cell(e),
                    why = esc(&e.reason),
                    size = esc(&human_size(e.bytes)),
                    due = esc(&date_or(e.due, "—")),
                    form = if e.key.is_empty() {
                        String::new()
                    } else {
                        key_form("exempter", d.token, &e.key, "Exempter", &format!("Exempter {}", e.title))
                    },
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<h2>Bientôt rouges ({days} j)</h2><div class="tw"><table><tbody>{rows}</tbody></table></div>"#,
            days = r.soon_days
        ));
    }
    // exemptés : toutes les exemptions, même d'un titre disparu du catalogue
    if !d.exemptions.is_empty() {
        let rows: String = d
            .exemptions
            .iter()
            .map(|(k, x)| {
                let found = r.exempted.iter().find(|e| &e.key == k);
                let (title, size) = match found {
                    Some(e) => (title_cell(e), esc(&human_size(e.bytes))),
                    None => (
                        format!(r#"{}<span class="why">absent du dernier calcul</span>"#, esc(&x.title)),
                        "—".into(),
                    ),
                };
                format!(
                    r#"<tr><td class="n">{title}</td><td class="w">{size}</td><td class="w">depuis le {at}</td><td class="t">{form}</td></tr>"#,
                    at = esc(&date_text(x.at)),
                    form = key_form(
                        "retirer",
                        d.token,
                        k,
                        "Retirer l'exemption",
                        &format!("Retirer l'exemption de {}", x.title)
                    ),
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<h2>Exemptés ({n})</h2><div class="tw"><table><tbody>{rows}</tbody></table></div>"#,
            n = d.exemptions.len()
        ));
    }
    // non évalués
    if !r.unmatched.is_empty() {
        let rows: String = r
            .unmatched
            .iter()
            .map(|e| {
                format!(
                    r#"<tr><td class="n">{title}</td><td class="w">{size}</td><td class="w">{why}</td></tr>"#,
                    title = title_cell(e),
                    size = esc(&human_size(e.bytes)),
                    why = esc(&e.reason),
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<details><summary>Non évalués ({n}) : jamais rouges</summary><div class="tw"><table><tbody>{rows}</tbody></table></div></details>"#,
            n = r.unmatched.len()
        ));
    }
    // derniers travaux
    if !d.jobs.is_empty() {
        let items: String = d
            .jobs
            .iter()
            .rev()
            .take(5)
            .map(|j| {
                format!(
                    r#"<li><a href="{href}">{at}</a> · {status}{sim} · {n} titre(s)</li>"#,
                    href = job_href(&j.id),
                    at = esc(&ago(d.now, j.started)),
                    status = job_status(j),
                    sim = if j.dry_run { " (simulation)" } else { "" },
                    n = j.keys.len(),
                )
            })
            .collect();
        out.push_str(&format!(
            r#"<h2>Derniers « Nettoyer »</h2><ul class="foot">{items}</ul>"#
        ));
    }
    out.push_str(r#"<p class="foot">Chaque titre est revérifié juste avant sa suppression (exempté, en lecture, plus rouge, fiche changée : écarté). Un titre présent sur le VPS et la seedbox n'est rouge que si aucune copie n'a été lue.</p>"#);
    shell_with("Liste rouge", CSS, &out)
}

pub struct ConfirmData<'a> {
    pub entries: &'a [&'a RedEntry],
    pub snap: &'a str,
    pub token: &'a str,
    /// Variante « un seul titre » (`?key=`).
    pub key: Option<&'a str>,
    /// Simulation effective (`red_list_clean::effective_dry`).
    pub dry: bool,
    /// Rouges proposables en tout (au-delà du plafond, ils attendent un prochain clic).
    pub total_red: usize,
}

pub fn render_confirm(d: &ConfirmData<'_>) -> String {
    let total: u64 = d.entries.iter().map(|e| e.bytes).sum();
    let items: String = d
        .entries
        .iter()
        .map(|e| {
            format!(
                "<li><b>{}</b>{} · {} · {}</li>",
                esc(&e.title),
                if e.russian { " (voie russe)" } else { "" },
                esc(&sides(e)),
                esc(&human_size(e.bytes))
            )
        })
        .collect();
    let mode = if d.dry {
        r#"<p class="flash info"><span class="badge sim">Simulation</span> Rien ne sera supprimé : le travail dit seulement ce qu'il ferait.</p>"#
    } else if d.key.is_some() {
        r#"<p class="flash err"><b>Essai réel</b> sur ce seul titre : la suppression est définitive.</p>"#
    } else {
        r#"<p class="flash err"><b>Suppression réelle</b> et définitive.</p>"#
    };
    let rest = d.total_red.saturating_sub(d.entries.len());
    let rest = if d.key.is_none() && rest > 0 {
        format!(r#"<p class="sub">Les {rest} titre(s) suivant(s) attendent un prochain clic.</p>"#)
    } else {
        String::new()
    };
    let key_field = d
        .key
        .map(|k| format!(r#"<input type="hidden" name="key" value="{}">"#, esc(k)))
        .unwrap_or_default();
    shell_with(
        "Nettoyer la liste rouge",
        CSS,
        &format!(
            r#"<header><div><h1>Nettoyer {n} titre(s) ?</h1><p class="sub">{size} au total.</p></div></header>
{mode}
<section class="card" aria-label="Confirmation">
<p>Pour chaque titre : fiche Radarr/Sonarr et fichiers (corbeille de l'Arr vidée), torrent, média Jellyseerr. Chaque titre est revérifié juste avant (exempté, en lecture, plus rouge : écarté).</p>
<ul>{items}</ul>{rest}
<div class="row"><form method="post" action="{BASE}/nettoyer"><input type="hidden" name="token" value="{token}"><input type="hidden" name="snap" value="{snap}">{key_field}<button class="btn danger" type="submit">{button}</button></form><a class="btn ghost" href="{BASE}">Annuler</a></div>
</section>"#,
            n = d.entries.len(),
            size = esc(&human_size(total)),
            token = esc(d.token),
            snap = esc(d.snap),
            button = if d.dry {
                "Lancer la simulation"
            } else {
                "Supprimer définitivement"
            },
        ),
    )
}

fn outcome_label(r: &CleanResult) -> &'static str {
    match r.outcome.as_str() {
        "deleted" => "supprimé",
        "partial" => "en partie",
        "skipped" => "écarté",
        "simulated" => "simulé",
        "error" => "erreur",
        _ => "?",
    }
}

pub fn render_job(id: &str, job: Option<&CleanJob>, busy: bool, now: i64) -> String {
    let refresh = format!(
        r#"<meta http-equiv="refresh" content="5;url={}">"#,
        job_href(id)
    );
    let back = format!(r#"<p><a href="{BASE}">← Liste rouge</a></p>"#);
    let Some(j) = job else {
        return if busy {
            shell_with(
                "Nettoyer : en préparation",
                &format!("{CSS}{refresh}"),
                &format!(
                    r#"{back}<h1>Travail en préparation…</h1><p class="sub">La page se recharge toutes les 5 s.</p>"#
                ),
            )
        } else {
            shell_with(
                "Travail introuvable",
                CSS,
                &format!(
                    r#"{back}<h1>Travail introuvable</h1><p class="sub">Seuls les derniers travaux sont gardés.</p>"#
                ),
            )
        };
    };
    let running = j.status == "running";
    let head = if running {
        format!("{CSS}{refresh}")
    } else {
        CSS.to_string()
    };
    let state = if running {
        format!(
            "{} / {} titre(s) traité(s) · lancé {} · la page se recharge toutes les 5 s",
            j.results.len(),
            j.keys.len(),
            ago(now, j.started)
        )
    } else {
        format!("{} · {}", job_status(j), red_list_clean::job_summary(j))
    };
    let detail = if j.detail.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="flash warn">{}</p>"#, esc(&j.detail))
    };
    let quota = j
        .quota_before
        .map(|q| {
            format!(
                r#"<p class="foot">Quota de la seedbox au début : {} sur {}.</p>"#,
                esc(&human_size(q.used_kb.saturating_mul(1024))),
                esc(&human_size(q.quota_kb.saturating_mul(1024)))
            )
        })
        .unwrap_or_default();
    let rows: String = j
        .results
        .iter()
        .map(|r| {
            let pending = if r.pending.is_empty() {
                String::new()
            } else {
                format!(" · à rejouer : {}", r.pending.join(", "))
            };
            format!(
                r#"<tr><td class="n">{title}<span class="why">{detail}{pending}</span></td><td class="w">{outcome}</td><td class="w">{now_b} tout de suite · {later} plus tard · {shared} partagé(s)</td></tr>"#,
                title = esc(&r.title),
                detail = esc(&r.detail),
                pending = esc(&pending),
                outcome = outcome_label(r),
                now_b = esc(&human_size(r.freed_now)),
                later = esc(&human_size(r.freed_later)),
                shared = esc(&human_size(r.shared)),
            )
        })
        .collect();
    let table = if rows.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="tw"><table><tbody>{rows}</tbody></table></div>"#)
    };
    shell_with(
        "Nettoyer : travail",
        &head,
        &format!(
            r#"{back}<header><div><h1>Nettoyer{sim}</h1><p class="sub">{state}</p></div></header>{detail}{table}{quota}"#,
            sim = if j.dry_run { " (simulation)" } else { "" },
            state = esc(&state),
        ),
    )
}

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

fn allowed(st: &PageState, given: Option<&str>) -> bool {
    crate::admin_auth::token_matches(st.ctx.secrets.onboard_token.as_ref(), given)
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(format!(
            "<p>Session expirée : <a href=\"/connexion?next={BASE}\">se reconnecter</a>.</p>"
        )),
    )
        .into_response()
}

fn page(html: String) -> Response {
    let mut resp = Html(html).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn back(code: &str, who: &str) -> Response {
    Redirect::to(&format!("{BASE}?msg={code}&who={}", urlencode(who))).into_response()
}

async fn list(State(st): State<PageState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let token = q.get("token").map(String::as_str);
    if !allowed(&st, token) {
        return denied();
    }
    let (report, exemptions, jobs, pending) = st
        .ctx
        .state
        .read(|s| {
            (
                s.red_list.report.clone(),
                s.red_list.exemptions.clone(),
                s.red_list.jobs.clone(),
                s.red_list.pending.len(),
            )
        })
        .await;
    let who = q.get("who").map(String::as_str).unwrap_or("");
    let cfg = &st.ctx.cfg.tasks.red_list;
    page(render_list(&ListData {
        now: now(),
        report: report.as_ref(),
        exemptions: &exemptions,
        jobs: &jobs,
        pending,
        token: token.unwrap_or(""),
        msg: q.get("msg").map(|m| (m.as_str(), who)),
        clean_dry_run: cfg.clean_dry_run,
        global_dry: st.ctx.dry_run,
        busy: red_list_clean::busy(),
        max: cfg.max_titles_per_clean,
    }))
}

#[derive(Deserialize)]
struct KeyForm {
    token: String,
    #[serde(default)]
    key: String,
}

/// Titre d'une clé dans le dernier calcul (ou l'exemption), pour le message.
fn title_of(s: &homelab_core::state::State, key: &str) -> Option<String> {
    s.red_list
        .report
        .as_ref()
        .and_then(|r| {
            r.entries
                .iter()
                .chain(&r.soon)
                .chain(&r.exempted)
                .chain(&r.unmatched)
                .find(|e| e.key == key)
                .map(|e| e.title.clone())
        })
        .or_else(|| s.red_list.exemptions.get(key).map(|x| x.title.clone()))
}

async fn exempt(State(st): State<PageState>, Form(f): Form<KeyForm>) -> Response {
    if !allowed(&st, Some(&f.token)) {
        return denied();
    }
    if parse_key(&f.key).is_none() {
        return back("unknown", "");
    }
    let key = f.key.clone();
    let done = st
        .ctx
        .state
        .update(move |s| {
            let title = title_of(s, &key)?;
            if s.red_list.exemptions.contains_key(&key) {
                return Some((title, false));
            }
            s.red_list.exemptions.insert(
                key.clone(),
                Exemption {
                    at: now(),
                    title: title.clone(),
                },
            );
            let ex = s.red_list.exemptions.clone();
            if let Some(r) = s.red_list.report.as_mut() {
                regroup(r, &ex);
            }
            Some((title, true))
        })
        .await;
    match done {
        Ok(Some((title, true))) => {
            info!(task = TASK, key = %f.key, "title exempted via web");
            back("exempted", &title)
        }
        Ok(Some((title, false))) => back("unchanged", &title),
        Ok(None) => back("unknown", ""),
        Err(e) => {
            warn!(task = TASK, error = format!("{e:#}"), "exemption not saved");
            back("error", "")
        }
    }
}

async fn unexempt(State(st): State<PageState>, Form(f): Form<KeyForm>) -> Response {
    if !allowed(&st, Some(&f.token)) {
        return denied();
    }
    if parse_key(&f.key).is_none() {
        return back("unknown", "");
    }
    let key = f.key.clone();
    let done = st
        .ctx
        .state
        .update(move |s| {
            let removed = s.red_list.exemptions.remove(&key)?;
            let ex = s.red_list.exemptions.clone();
            if let Some(r) = s.red_list.report.as_mut() {
                regroup(r, &ex);
            }
            Some(removed.title)
        })
        .await;
    match done {
        Ok(Some(title)) => {
            info!(task = TASK, key = %f.key, "exemption removed via web");
            back("unexempted", &title)
        }
        Ok(None) => back("unchanged", &f.key),
        Err(e) => {
            warn!(
                task = TASK,
                error = format!("{e:#}"),
                "exemption not removed"
            );
            back("error", "")
        }
    }
}

/// Page de confirmation (GET) : rien n'est supprimé ici.
async fn confirm(
    State(st): State<PageState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let token = q.get("token").map(String::as_str);
    if !allowed(&st, token) {
        return denied();
    }
    if red_list_clean::busy() {
        return back("busy", "");
    }
    let key = q.get("key").map(String::as_str).filter(|k| !k.is_empty());
    let (report, exemptions) = st
        .ctx
        .state
        .read(|s| (s.red_list.report.clone(), s.red_list.exemptions.clone()))
        .await;
    let cfg = &st.ctx.cfg.tasks.red_list;
    let entries = match selection(report.as_ref(), &exemptions, key, cfg.max_titles_per_clean) {
        Ok(e) => e,
        Err(code) => return back(code, key.unwrap_or("")),
    };
    let Some(r) = report.as_ref() else {
        return back("nolist", "");
    };
    let keys: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
    let snap = red_list_clean::snap(r, &keys);
    let dry = red_list_clean::effective_dry(
        requested_dry(key.is_some(), cfg.clean_dry_run),
        cfg.clean_dry_run,
        st.ctx.dry_run,
        keys.len(),
    );
    let total_red = red_list_clean::proposed(r, &exemptions, usize::MAX).len();
    page(render_confirm(&ConfirmData {
        entries: &entries,
        snap: &snap,
        token: token.unwrap_or(""),
        key,
        dry,
        total_red,
    }))
}

#[derive(Deserialize)]
struct CleanForm {
    token: String,
    snap: String,
    #[serde(default)]
    key: String,
}

async fn start_clean(State(st): State<PageState>, Form(f): Form<CleanForm>) -> Response {
    if !allowed(&st, Some(&f.token)) {
        return denied();
    }
    let key = Some(f.key.as_str()).filter(|k| !k.is_empty());
    let (report, exemptions) = st
        .ctx
        .state
        .read(|s| (s.red_list.report.clone(), s.red_list.exemptions.clone()))
        .await;
    let cfg = &st.ctx.cfg.tasks.red_list;
    let (keys, snap) = match check_start(
        report.as_ref(),
        &exemptions,
        key,
        cfg.max_titles_per_clean,
        &f.snap,
        red_list_clean::busy(),
    ) {
        Ok(v) => v,
        Err(code) => {
            info!(task = TASK, code, "clean refused via web");
            return back(code, key.unwrap_or(""));
        }
    };
    let dry = requested_dry(key.is_some(), cfg.clean_dry_run);
    let id = red_list_clean::new_job_id();
    info!(task = TASK, job = %id, titles = keys.len(), "clean started via web");
    let (ctx, jid) = (st.ctx.clone(), id.clone());
    tokio::spawn(async move {
        match red_list_clean::clean(&ctx, &jid, &keys, &snap, dry).await {
            Ok(job) => info!(task = TASK, job = %jid, status = %job.status, "clean finished"),
            Err(e) => warn!(task = TASK, job = %jid, error = format!("{e:#}"), "clean failed"),
        }
    });
    Redirect::to(&format!("{BASE}/travail?id={}", urlencode(&id))).into_response()
}

async fn job_page(
    State(st): State<PageState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !allowed(&st, q.get("token").map(String::as_str)) {
        return denied();
    }
    let id = q.get("id").cloned().unwrap_or_default();
    let job = {
        let id = id.clone();
        st.ctx
            .state
            .read(move |s| s.red_list.jobs.iter().find(|j| j.id == id).cloned())
            .await
    };
    page(render_job(&id, job.as_ref(), red_list_clean::busy(), now()))
}

#[derive(Deserialize)]
struct TokenForm {
    token: String,
}

async fn recompute(State(st): State<PageState>, Form(f): Form<TokenForm>) -> Response {
    if !allowed(&st, Some(&f.token)) {
        return denied();
    }
    let Some(slot) = red_list_clean::reserve_recompute() else {
        return back("busy", "");
    };
    info!(task = TASK, "recompute started via web");
    let ctx = st.ctx.clone();
    tokio::spawn(async move {
        let _slot = slot;
        match red_list::refresh(&ctx).await {
            Ok(Some(r)) => info!(task = TASK, summary = %red_list::summary(&r), "recompute done"),
            Ok(None) => warn!(
                task = TASK,
                "recompute: Arr side unreachable, previous list kept"
            ),
            Err(e) => warn!(task = TASK, error = format!("{e:#}"), "recompute failed"),
        }
    });
    back("recompute", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelab_core::state::RedSide;

    fn entry(key: &str, title: &str, bytes: u64) -> RedEntry {
        RedEntry {
            key: key.into(),
            title: title.into(),
            kind: "film".into(),
            movie: true,
            sides: vec![RedSide {
                side: "seedbox".into(),
                arr_id: 7,
                bytes,
                folder: "/data/x".into(),
                ..Default::default()
            }],
            bytes,
            arrived: Some(1_000_000),
            due: Some(1_500_000),
            reason: "jamais lu".into(),
            requester: "membre".into(),
            ..Default::default()
        }
    }

    fn report() -> RedListReport {
        RedListReport {
            at: 2_000_000,
            min_idle_days: 60,
            soon_days: 14,
            complete: true,
            evaluated: 4,
            entries: vec![
                entry("movie:tmdb:1", "<script>alert(1)</script>", 30),
                entry("movie:tmdb:2", "Deux", 20),
                entry("movie:tmdb:3", "Trois", 10),
            ],
            soon: vec![entry("movie:tmdb:4", "Bientôt", 5)],
            ..Default::default()
        }
    }

    fn list_html(
        r: Option<&RedListReport>,
        ex: &BTreeMap<String, Exemption>,
        busy: bool,
    ) -> String {
        render_list(&ListData {
            now: 2_000_100,
            report: r,
            exemptions: ex,
            jobs: &[],
            pending: 0,
            token: "t\"k",
            msg: Some(("exempted", "<b>x</b>")),
            clean_dry_run: true,
            global_dry: false,
            busy,
            max: 2,
        })
    }

    fn forms_are_post(html: &str) {
        assert_eq!(
            html.matches("<form").count(),
            html.matches(r#"<form method="post""#).count(),
            "{html}"
        );
        assert!(!html.contains("token="), "jamais de token= dans la page");
        assert!(!html.contains("<script"), "ni script ni window.confirm");
    }

    #[test]
    fn list_is_escaped_post_only_and_shows_sections() {
        let r = report();
        let mut ex = BTreeMap::new();
        ex.insert(
            "series:tvdb:9".to_string(),
            Exemption {
                at: 1_000_000,
                title: "<Gardé>".into(),
            },
        );
        let html = list_html(Some(&r), &ex, false);
        forms_are_post(&html);
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"), "message échappé");
        assert!(html.contains(r#"value="t&quot;k""#));
        assert!(html.contains("Simulation"));
        assert!(html.contains("Bientôt rouges (14 j)"));
        assert!(html.contains("Exemptés (1)") && html.contains("&lt;Gardé&gt;"));
        assert!(html.contains(r#"action="/accounts/liste-rouge/retirer""#));
        assert_eq!(
            html.matches(r#"action="/accounts/liste-rouge/exempter""#)
                .count(),
            4
        );
        assert!(html.contains("Nettoyer les 2 plus gros"));
        assert!(html.contains(r#"href="/accounts/liste-rouge/nettoyer?key=movie%3Atmdb%3A2""#));
        assert!(html.contains(r#"action="/accounts/liste-rouge/recalculer""#));
    }

    #[test]
    fn incomplete_or_busy_offers_no_clean() {
        let mut r = report();
        r.complete = false;
        r.missing = vec!["Playback <Reporting>".into()];
        let html = list_html(Some(&r), &BTreeMap::new(), false);
        assert!(html.contains("Calcul incomplet") && html.contains("Playback &lt;Reporting&gt;"));
        assert!(!html.contains("/nettoyer"));
        let busy = list_html(Some(&report()), &BTreeMap::new(), true);
        assert!(!busy.contains("/nettoyer") && busy.contains("est en cours"));
        let none = list_html(None, &BTreeMap::new(), false);
        assert!(none.contains("Pas encore calculée"));
        forms_are_post(&none);
    }

    #[test]
    fn confirmation_lists_exactly_the_capped_titles() {
        let r = report();
        let ex = BTreeMap::new();
        let picked = selection(Some(&r), &ex, None, 2).unwrap();
        let keys: Vec<String> = picked.iter().map(|e| e.key.clone()).collect();
        assert_eq!(keys, ["movie:tmdb:1", "movie:tmdb:2"]);
        let snap = red_list_clean::snap(&r, &keys);
        let html = render_confirm(&ConfirmData {
            entries: &picked,
            snap: &snap,
            token: "t",
            key: None,
            dry: true,
            total_red: 3,
        });
        forms_are_post(&html);
        assert_eq!(html.matches("<li><b>").count(), 2);
        assert!(html.contains("&lt;script&gt;") && html.contains("<b>Deux</b>"));
        assert!(!html.contains("Trois"));
        assert!(html.contains(&format!(r#"name="snap" value="{snap}""#)));
        assert!(html.contains("Lancer la simulation") && html.contains("1 titre(s) suivant(s)"));
        assert!(!html.contains(r#"name="key""#));
        // un titre seul : sa clé part avec le formulaire, essai réel
        let one = selection(Some(&r), &ex, Some("movie:tmdb:3"), 2).unwrap();
        let html = render_confirm(&ConfirmData {
            entries: &one,
            snap: "s",
            token: "t",
            key: Some("movie:tmdb:3"),
            dry: false,
            total_red: 3,
        });
        assert!(html.contains("Essai réel") && html.contains("Supprimer définitivement"));
        assert!(html.contains(r#"name="key" value="movie:tmdb:3""#));
        assert_eq!(
            selection(Some(&r), &ex, Some("movie:tmdb:4"), 2),
            Err("notred")
        );
        // exempté : jamais proposé
        let mut ex2 = BTreeMap::new();
        ex2.insert("movie:tmdb:1".to_string(), Exemption::default());
        let picked = selection(Some(&r), &ex2, None, 2).unwrap();
        assert_eq!(picked[0].key, "movie:tmdb:2");
    }

    #[test]
    fn start_refuses_changed_list_busy_and_incomplete() {
        let r = report();
        let ex = BTreeMap::new();
        let keys = vec!["movie:tmdb:1".to_string(), "movie:tmdb:2".to_string()];
        let snap = red_list_clean::snap(&r, &keys);
        assert_eq!(
            check_start(Some(&r), &ex, None, 2, &snap, false),
            Ok((keys.clone(), snap.clone()))
        );
        assert_eq!(
            check_start(Some(&r), &ex, None, 2, &snap, true),
            Err("busy")
        );
        // nouveau calcul, taille changée, exemption : l'empreinte ne correspond plus
        let mut later = r.clone();
        later.at += 1;
        assert_eq!(
            check_start(Some(&later), &ex, None, 2, &snap, false),
            Err("changed")
        );
        let mut bigger = r.clone();
        bigger.entries[1].sides[0].bytes += 1;
        assert_eq!(
            check_start(Some(&bigger), &ex, None, 2, &snap, false),
            Err("changed")
        );
        let mut ex2 = BTreeMap::new();
        ex2.insert("movie:tmdb:2".to_string(), Exemption::default());
        assert_eq!(
            check_start(Some(&r), &ex2, None, 2, &snap, false),
            Err("changed")
        );
        let mut inc = r.clone();
        inc.complete = false;
        assert_eq!(
            check_start(Some(&inc), &ex, None, 2, &snap, false),
            Err("incomplete")
        );
        assert_eq!(check_start(None, &ex, None, 2, &snap, false), Err("nolist"));
    }

    #[test]
    fn single_title_is_the_real_trial_list_follows_dry_run() {
        assert!(requested_dry(false, true));
        assert!(!requested_dry(true, true));
        assert!(!requested_dry(false, false));
        assert!(!red_list_clean::effective_dry(
            requested_dry(true, true),
            true,
            false,
            1
        ));
        assert!(red_list_clean::effective_dry(
            requested_dry(false, true),
            true,
            false,
            1
        ));
    }

    #[test]
    fn job_page_refreshes_only_while_running() {
        let mut job = CleanJob {
            id: "20261021-101500-3fa2".into(),
            started: 1_000,
            status: "running".into(),
            keys: vec!["movie:tmdb:1".into(), "movie:tmdb:2".into()],
            results: vec![CleanResult {
                key: "movie:tmdb:1".into(),
                title: "<Un>".into(),
                outcome: "deleted".into(),
                freed_now: 1_000_000_000,
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_job(&job.id, Some(&job), true, 1_060);
        assert!(html.contains(r#"http-equiv="refresh" content="5;url=/accounts/liste-rouge/travail?id=20261021-101500-3fa2""#));
        assert!(html.contains("1 / 2 titre(s)") && html.contains("&lt;Un&gt;"));
        job.status = "done".into();
        let html = render_job(&job.id, Some(&job), false, 1_060);
        assert!(!html.contains("http-equiv"));
        assert!(html.contains("supprimé") && html.contains("terminé"));
        assert!(!html.contains("token="));
        assert!(render_job("x", None, true, 0).contains("http-equiv"));
        assert!(render_job("x", None, false, 0).contains("introuvable"));
    }

    #[test]
    fn every_route_needs_the_admin_session() {
        use crate::admin_auth::{is_admin_path, required, Scope};
        for p in [
            "/accounts/liste-rouge",
            "/accounts/liste-rouge/exempter",
            "/accounts/liste-rouge/retirer",
            "/accounts/liste-rouge/nettoyer",
            "/accounts/liste-rouge/travail",
            "/accounts/liste-rouge/recalculer",
        ] {
            assert_eq!(required(p), Some(Scope::Admin), "{p}");
            assert!(is_admin_path(p), "{p}");
        }
    }

    #[test]
    fn messages_are_escaped_and_known() {
        let (cls, text) = message("notred", "<x>").unwrap();
        assert_eq!(cls, "warn");
        assert!(text.contains("&lt;x&gt;"));
        for c in ["changed", "busy", "incomplete", "recompute", "unexempted"] {
            assert!(message(c, "").is_some(), "{c}");
        }
        assert!(message("autre", "").is_none());
    }
}
