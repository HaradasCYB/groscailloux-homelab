//! Page « Recherche manuelle » (onboarder.<domaine>/recherche) : trouver un titre des Arrs, choisir une saison ou
//! le film, lancer la recherche en arrière-plan (`homelab_core::manual_search`), puis télécharger une release.
//! Rendu côté serveur, sans JavaScript : la page de résultats se recharge seule tant que la recherche tourne.
//! Accès : jeton d'onboarding (paramètre ou champ caché, protège aussi du CSRF) ; NPM ajoute la liste
//! « admin-outils » devant `/recherche`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
pub use homelab_core::html::esc;
use homelab_core::manual_search::{self, Found, Job, Row};
use homelab_core::state::now;
use homelab_core::tasks::series_search::Target;
use homelab_core::TaskContext;
use rand::Rng;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Clone)]
struct SearchState {
    ctx: Arc<TaskContext>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

pub fn router(ctx: Arc<TaskContext>) -> Router {
    let st = SearchState {
        ctx,
        jobs: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/recherche", get(home))
        .route("/recherche/lancer", post(start))
        .route("/recherche/resultats", get(results))
        .route("/recherche/telecharger", post(download))
        .with_state(st)
}

fn allowed(st: &SearchState, given: &str) -> bool {
    match &st.ctx.secrets.onboard_token {
        Some(t) => !given.is_empty() && given == t.expose(),
        None => false,
    }
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(
            "<p>Session expirée : <a href=\"/connexion?next=/recherche\">se reconnecter</a>.</p>"
                .to_string(),
        ),
    )
        .into_response()
}

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

pub fn human_size(bytes: i64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} Go")
    } else {
        format!("{:.0} Mo", bytes as f64 / 1_048_576.0)
    }
}

fn lang_label(rank: u8) -> &'static str {
    match rank {
        4 => "VF",
        3 => "MULTi",
        2 => "FRENCH",
        1 => "VOSTFR",
        _ => "VO",
    }
}

const STYLE: &str = r#"<style>
:root{--bg:#f6f7f9;--card:#fff;--fg:#1c2230;--mut:#6b7385;--line:#e3e6ec;--acc:#3b5bdb;--ok:#2f9e44;--warn:#e67700;--err:#c92a2a}
@media (prefers-color-scheme:dark){:root{--bg:#12151c;--card:#1b2029;--fg:#e8ebf1;--mut:#98a1b3;--line:#2a303c;--acc:#748ffc;--ok:#51cf66;--warn:#ffa94d;--err:#ff6b6b}}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:1100px;margin:0 auto;padding:24px 16px 60px}
h1{font-size:22px;margin:0 0 4px}h2{font-size:17px;margin:24px 0 8px}
p.sub{color:var(--mut);margin:0 0 18px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px;margin:10px 0}
form.q{display:flex;gap:8px;flex-wrap:wrap}input[type=text]{flex:1;min-width:200px;padding:9px 11px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--fg);font:inherit}
button{padding:8px 13px;border:0;border-radius:8px;background:var(--acc);color:#fff;font:inherit;cursor:pointer}
button.ghost{background:transparent;color:var(--acc);border:1px solid var(--line)}
.seasons{display:flex;gap:6px;flex-wrap:wrap;margin-top:8px}.seasons form{margin:0}
.tag{display:inline-block;font-size:12px;padding:1px 7px;border-radius:99px;border:1px solid var(--line);color:var(--mut);margin-left:6px}
.tag.anime{color:var(--acc);border-color:var(--acc)}
.msg{padding:10px 14px;border-radius:8px;margin:10px 0;border:1px solid var(--line)}.msg.ok{border-color:var(--ok)}.msg.err{border-color:var(--err)}
.scroll{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:14px}th,td{text-align:left;padding:7px 8px;border-bottom:1px solid var(--line);vertical-align:top}
th{color:var(--mut);font-weight:600;white-space:nowrap}td.t{word-break:break-word;min-width:260px}td.n{white-space:nowrap}
.flag{display:inline-block;font-size:12px;color:var(--warn);margin-right:6px}.clean{color:var(--ok);font-size:12px}
ul.notes{color:var(--mut);margin:6px 0 0;padding-left:18px}a{color:var(--acc)}
</style>"#;

fn page(title: &str, extra_head: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex"><title>{title}</title>{extra_head}{STYLE}</head><body><main>{body}</main></body></html>"#,
        title = esc(title)
    )
}

/// Liste des titres trouvés, avec un bouton par saison (séries) ou pour le film.
pub fn render_home(token: &str, query: &str, found: &[Found], msg: Option<&str>) -> String {
    let t = esc(token);
    let mut body = format!(
        r#"<h1>Recherche manuelle</h1><p class="sub">Par identifiant TMDB chez C411 (une requête, plafond horaire), plus Nyaa pour un animé. À utiliser à la place de la recherche de Sonarr/Radarr pour un animé.</p>
<form class="q" method="get" action="/recherche"><input type="text" name="q" value="{q}" placeholder="Titre (français, anglais ou japonais)" autofocus><button type="submit">Chercher</button></form>"#,
        q = esc(query)
    );
    if let Some(m) = msg {
        body.push_str(&format!(r#"<div class="msg err">{}</div>"#, esc(m)));
    }
    if !query.trim().is_empty() && found.is_empty() {
        body.push_str(r#"<div class="msg">Aucun titre dans Sonarr ou Radarr : le demander d'abord dans Jellyseerr.</div>"#);
    }
    for f in found {
        let side = if f.arr.ends_with("seedbox") {
            "seedbox"
        } else {
            "VPS"
        };
        let anime = if f.anime {
            r#"<span class="tag anime">animé</span>"#
        } else {
            ""
        };
        let year = if f.year > 0 {
            format!(" ({})", f.year)
        } else {
            String::new()
        };
        let launch = |season: Option<i64>, label: &str, class: &str| {
            format!(
                r#"<form method="post" action="/recherche/lancer"><input type="hidden" name="token" value="{t}"><input type="hidden" name="arr" value="{arr}"><input type="hidden" name="id" value="{id}"><input type="hidden" name="season" value="{s}"><button class="{class}" type="submit">{label}</button></form>"#,
                arr = esc(f.arr),
                id = f.id,
                s = season.map(|s| s.to_string()).unwrap_or_default(),
                label = esc(label),
            )
        };
        let buttons = if f.movie {
            launch(
                None,
                if f.has_file {
                    "Film (déjà présent)"
                } else {
                    "Chercher le film"
                },
                if f.has_file { "ghost" } else { "" },
            )
        } else {
            f.seasons
                .iter()
                .map(|(n, have, total)| {
                    let complete = *total > 0 && have >= total;
                    launch(
                        Some(*n),
                        &format!("Saison {n} · {have}/{total}"),
                        if complete { "ghost" } else { "" },
                    )
                })
                .collect::<String>()
        };
        body.push_str(&format!(
            r#"<div class="card"><b>{title}</b>{year}<span class="tag">{kind} · {side}</span>{anime}<div class="seasons">{buttons}</div></div>"#,
            title = esc(&f.title),
            kind = if f.movie { "film" } else { "série" },
        ));
    }
    page("Recherche manuelle", "", &body)
}

fn row_html(token: &str, job: &str, idx: usize, r: &Row) -> String {
    let flags = if r.flags.is_empty() {
        r#"<span class="clean">conforme</span>"#.to_string()
    } else {
        r.flags
            .iter()
            .map(|f| format!(r#"<span class="flag">{}</span>"#, esc(f)))
            .collect()
    };
    let what = match (r.season, r.full_season, r.episodes.is_empty()) {
        (Some(s), true, _) => format!("S{s:02} complète"),
        (Some(s), false, false) => format!(
            "S{s:02} E{}",
            r.episodes
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        (Some(s), _, _) => format!("S{s:02}"),
        (None, _, _) => "—".into(),
    };
    format!(
        r#"<tr><td class="t">{title}<br>{flags}</td><td class="n">{idx_name}</td><td class="n">{lang}</td><td class="n">{quality}{codec}</td><td class="n">{what}</td><td class="n">{size}</td><td class="n">{seeders}</td><td><form method="post" action="/recherche/telecharger"><input type="hidden" name="token" value="{t}"><input type="hidden" name="job" value="{job}"><input type="hidden" name="idx" value="{idx}"><button type="submit">Télécharger</button></form></td></tr>"#,
        title = esc(&r.title),
        idx_name = esc(&r.indexer),
        lang = lang_label(r.lang),
        quality = esc(&r.quality),
        codec = if r.h264 { "" } else { " · HEVC" },
        size = human_size(r.size),
        seeders = r.seeders,
        t = esc(token),
        job = esc(job),
    )
}

/// Résultats d'une recherche (en cours : rechargement automatique).
pub fn render_results(token: &str, job: &Job, msg: Option<(&str, &str)>, now: i64) -> String {
    let head = if job.done {
        String::new()
    } else {
        format!(
            r#"<meta http-equiv="refresh" content="4;url=/recherche/resultats?job={}">"#,
            urlencode(&job.id)
        )
    };
    let mut body = format!(
        r#"<p><a href="/recherche">← Nouvelle recherche</a></p><h1>{label}</h1><p class="sub">{arr} · {state}</p>"#,
        label = esc(&job.label),
        arr = esc(job.arr),
        state = if job.done {
            format!("{} release(s)", job.rows.len())
        } else {
            format!("recherche en cours ({} s)…", now - job.started)
        },
    );
    if let Some((class, text)) = msg {
        body.push_str(&format!(
            r#"<div class="msg {}">{}</div>"#,
            esc(class),
            esc(text)
        ));
    }
    if !job.notes.is_empty() {
        body.push_str(r#"<ul class="notes">"#);
        for n in &job.notes {
            body.push_str(&format!("<li>{}</li>", esc(n)));
        }
        body.push_str("</ul>");
    }
    if job.done && !job.rows.is_empty() {
        body.push_str(r#"<div class="card scroll"><table><thead><tr><th>Release</th><th>Indexeur</th><th>Langue</th><th>Qualité</th><th>Contenu</th><th>Taille</th><th>Sources</th><th></th></tr></thead><tbody>"#);
        for (i, r) in job.rows.iter().enumerate() {
            body.push_str(&row_html(token, &job.id, i, r));
        }
        body.push_str("</tbody></table></div>");
    }
    page(&format!("Recherche · {}", job.label), &head, &body)
}

#[derive(Deserialize)]
struct HomeQuery {
    #[serde(default)]
    token: String,
    #[serde(default)]
    q: String,
}

async fn home(State(st): State<SearchState>, Query(q): Query<HomeQuery>) -> Response {
    if !allowed(&st, &q.token) {
        return denied();
    }
    let found = if q.q.trim().is_empty() {
        Vec::new()
    } else {
        manual_search::find(&st.ctx, &q.q).await
    };
    Html(render_home(&q.token, &q.q, &found, None)).into_response()
}

#[derive(Deserialize)]
struct StartForm {
    token: String,
    arr: String,
    id: i64,
    #[serde(default)]
    season: String,
}

async fn start(State(st): State<SearchState>, Form(f): Form<StartForm>) -> Response {
    if !allowed(&st, &f.token) {
        return denied();
    }
    let Some(arr) = manual_search::arr_by_name(&st.ctx, &f.arr) else {
        return Html(render_home(&f.token, "", &[], Some("Application inconnue"))).into_response();
    };
    let season = f.season.trim().parse::<i64>().ok();
    let movie = arr.is_radarr();
    let target = if movie {
        Target::Movie { movie_id: f.id }
    } else {
        match season {
            Some(s) => Target::Season {
                series_id: f.id,
                season: s,
            },
            None => {
                return Html(render_home(&f.token, "", &[], Some("Saison manquante")))
                    .into_response()
            }
        }
    };
    let title = arr
        .get(
            &format!("api/v3/{}/{}", if movie { "movie" } else { "series" }, f.id),
            &[],
        )
        .await
        .ok()
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("#{}", f.id));
    let label = match season {
        Some(s) if !movie => format!("{title} — saison {s}"),
        _ => title,
    };
    let id: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    let ttl = st.ctx.cfg.manual_search.results_ttl_mins * 60;
    let t = now();
    {
        let mut jobs = st.jobs.lock().await;
        jobs.retain(|_, j| t - j.started < ttl);
        jobs.insert(
            id.clone(),
            Job {
                id: id.clone(),
                arr: arr.name,
                label: label.clone(),
                target,
                started: t,
                done: false,
                rows: Vec::new(),
                notes: Vec::new(),
                sent: Vec::new(),
            },
        );
    }
    info!(task = "manual_search", service = arr.name, %label, "search started via web");
    let (ctx, jobs, jid, arr_name, item_id) =
        (st.ctx.clone(), st.jobs.clone(), id.clone(), arr.name, f.id);
    tokio::spawn(async move {
        let Some(arr) = manual_search::arr_by_name(&ctx, arr_name) else {
            return;
        };
        let outcome = manual_search::run(&ctx, arr, item_id, season).await;
        let mut jobs = jobs.lock().await;
        if let Some(j) = jobs.get_mut(&jid) {
            match outcome {
                Ok((rows, notes)) => {
                    j.rows = rows;
                    j.notes = notes;
                }
                Err(e) => {
                    warn!(task = "manual_search", service = arr_name, error = %e, "search failed");
                    j.notes.push(format!("Erreur : {e:#}"));
                }
            }
            j.done = true;
            info!(task = "manual_search", service = arr_name, label = %j.label, releases = j.rows.len(), "search done");
        }
    });
    Redirect::to(&format!("/recherche/resultats?job={}", urlencode(&id))).into_response()
}

#[derive(Deserialize)]
struct ResultsQuery {
    #[serde(default)]
    token: String,
    #[serde(default)]
    job: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    ok: String,
}

async fn results(State(st): State<SearchState>, Query(q): Query<ResultsQuery>) -> Response {
    if !allowed(&st, &q.token) {
        return denied();
    }
    let jobs = st.jobs.lock().await;
    let Some(job) = jobs.get(&q.job) else {
        return Html(render_home(
            &q.token,
            "",
            &[],
            Some("Recherche expirée : la relancer."),
        ))
        .into_response();
    };
    let msg = (!q.msg.is_empty()).then(|| (if q.ok == "1" { "ok" } else { "err" }, q.msg.as_str()));
    Html(render_results(&q.token, job, msg, now())).into_response()
}

#[derive(Deserialize)]
struct DownloadForm {
    token: String,
    job: String,
    idx: usize,
}

async fn download(State(st): State<SearchState>, Form(f): Form<DownloadForm>) -> Response {
    if !allowed(&st, &f.token) {
        return denied();
    }
    let picked = {
        let jobs = st.jobs.lock().await;
        jobs.get(&f.job)
            .and_then(|j| j.rows.get(f.idx).cloned().map(|r| (j.arr, j.target, r)))
    };
    let (ok, text) = match picked {
        None => (false, "Recherche expirée : la relancer.".to_string()),
        Some((arr_name, target, row)) => match manual_search::arr_by_name(&st.ctx, arr_name) {
            None => (false, "Application inconnue".to_string()),
            Some(arr) => match manual_search::grab(&st.ctx, arr, &row, target).await {
                Ok(detail) => {
                    let mut jobs = st.jobs.lock().await;
                    if let Some(j) = jobs.get_mut(&f.job) {
                        j.sent.push(row.title.clone());
                    }
                    (true, detail)
                }
                Err(e) => {
                    warn!(task = "manual_search", service = arr_name, release = %row.title, error = %e, "manual grab failed");
                    (false, format!("Échec : {e:#}"))
                }
            },
        },
    };
    Redirect::to(&format!(
        "/recherche/resultats?job={}&ok={}&msg={}",
        urlencode(&f.job),
        if ok { "1" } else { "0" },
        urlencode(&text)
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(title: &str, flags: Vec<&'static str>) -> Row {
        Row {
            indexer: "C411".into(),
            title: title.into(),
            size: 3 * 1_073_741_824,
            seeders: 12,
            lang: 3,
            resolution: 1080,
            h264: true,
            quality: "WEBDL-1080p".into(),
            season: Some(2),
            episodes: vec![],
            full_season: true,
            flags,
            release: json!({}),
        }
    }

    #[test]
    fn html_is_escaped_and_token_carried() {
        let found = vec![Found {
            arr: "sonarr-seedbox",
            id: 7,
            title: "<script>x</script>".into(),
            year: 2005,
            movie: false,
            anime: true,
            has_file: false,
            seasons: vec![(1, 26, 26), (2, 0, 10)],
        }];
        let h = render_home("tok\"en", "a<b", &found, None);
        assert!(!h.contains("<script>x"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(h.contains(r#"value="tok&quot;en""#));
        assert!(h.contains("Saison 2 · 0/10"));
        assert!(h.contains("animé"));
    }

    #[test]
    fn results_refresh_only_while_running() {
        let mut job = Job {
            id: "abc".into(),
            arr: "sonarr",
            label: "Série — saison 2".into(),
            target: Target::Season {
                series_id: 1,
                season: 2,
            },
            started: 100,
            done: false,
            rows: vec![],
            notes: vec!["C411 : 3 release(s)".into()],
            sent: vec![],
        };
        let running = render_results("t", &job, None, 110);
        assert!(running.contains("http-equiv=\"refresh\"") && running.contains("10 s"));
        job.done = true;
        job.rows = vec![
            row("Show.S02.MULTi.1080p.<b>", vec![]),
            row("Show.S02.VOSTFR", vec!["VOSTFR"]),
        ];
        let done = render_results("t", &job, Some(("ok", "envoyé")), 120);
        assert!(!done.contains("http-equiv"));
        assert!(
            done.contains("&lt;b&gt;") && done.contains("conforme") && done.contains(">VOSTFR<")
        );
        assert!(done.contains(r#"name="idx" value="1""#));
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(3 * 1_073_741_824), "3.0 Go");
        assert_eq!(human_size(700 * 1_048_576), "700 Mo");
    }
}
