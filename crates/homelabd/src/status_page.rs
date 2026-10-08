//! Page d'état HTML (tableau « Opérations » de Homarr, en iFrame) : dernier passage de chaque
//! tâche, disque du VPS, quota de la seedbox. Rendu côté serveur, sans JavaScript, thème sombre,
//! rechargée toutes les 60 s. Protégée par `HOMELABD_STATUS_TOKEN` (voir `web.rs`).

use std::collections::BTreeMap;

use homelab_core::html::esc;
use homelab_core::state::{
    AlertStats, CatalogueBucket, CatalogueEntry, CatalogueReport, CatalogueSplit, RunInfo,
};
use homelab_core::tasks::catalogue_report::{human_size, pct_text, percent};

/// Quota écrit par la seedbox (cron `quota -w`) dans `~/media/.homelab/quota.json` (aussi lu par `seedbox_health`).
pub use homelab_core::quota::SeedboxQuota;

pub struct PageData<'a> {
    pub now: i64,
    pub runs: &'a BTreeMap<String, RunInfo>,
    /// Tâches planifiées, dans l'ordre d'affichage.
    pub tasks: &'a [&'static str],
    pub vps_disk_pct: Option<u8>,
    pub seedbox: Option<SeedboxQuota>,
    pub mount_ok: Option<bool>,
    /// Ce qui n'avance pas : torrents terminés que personne ne rattache (`no_match` de `torrent_import`).
    /// Texte brut, échappé au rendu : un nom de release vient d'un tracker (Nyaa, World-torrent en secours).
    pub stuck_torrents: &'a [String],
    /// Saisons suivies dont l'indexer ne propose **rien** pour certains épisodes : la recherche
    /// repartira tous les jours sans jamais rien trouver, il faut la main d'un admin (`/recherche`).
    /// Texte brut, échappé au rendu (titre TVDB/TMDB).
    pub blocked_seasons: &'a [String],
    /// Dernier résultat du canari de lecture (`state.canary`), texte prêt à afficher.
    pub canary: Option<(bool, String)>,
    /// Dernier rapport « catalogue jamais regardé » (`state.catalogue`) ; `None` avant le premier calcul.
    pub catalogue: Option<&'a CatalogueReport>,
    /// Alertes admin parties (ou non) par `alerts::admin` : la preuve qu'un message a bien été livré.
    pub alerts: &'a AlertStats,
}

/// Saisons dont le dernier passage a laissé des épisodes sans aucune release, les plus récentes
/// d'abord. Clé d'état : `<arr>:<série>:<saison>`. Texte brut : `render` l'échappe.
pub fn blocked_seasons(
    records: &BTreeMap<String, homelab_core::state::SeasonSearchRecord>,
    now: i64,
    max: usize,
) -> Vec<String> {
    let mut v: Vec<(i64, String)> = records
        .iter()
        .filter(|(_, r)| !r.uncovered.is_empty())
        .map(|(k, r)| {
            let mut it = k.split(':');
            let side = it.next().unwrap_or("?");
            let season = k.rsplit(':').next().unwrap_or("?");
            let title = if r.title.is_empty() { k } else { &r.title };
            (
                r.at,
                format!(
                    "{side} · {title} S{season} — {n} épisode(s) introuvable(s) : {list} (vu {ago})",
                    n = r.uncovered.len(),
                    list = episode_list(&r.uncovered),
                    ago = ago(now, r.at)
                ),
            )
        })
        .collect();
    v.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    v.into_iter().take(max).map(|(_, s)| s).collect()
}

/// `[1,2,3,7]` → `1-3, 7` (une saison entière tiendrait sinon sur trois lignes).
fn episode_list(eps: &[i64]) -> String {
    let mut v = eps.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = i;
        while i + 1 < v.len() && v[i + 1] == v[i] + 1 {
            i += 1;
        }
        out.push(if i > start {
            format!("{}-{}", v[start], v[i])
        } else {
            v[start].to_string()
        });
        i += 1;
    }
    out.join(", ")
}

/// Torrents finis que rien n'a rattachés : aucune fiche n'en a voulu (`no_match`) ou aucun fichier
/// n'était importable (`nothing_importable` — nommage que ni Sonarr ni nous ne savons lire). Les deux
/// états sont **définitifs** : sans cet affichage, les octets restent sur la seedbox sans que personne
/// le sache (Erased, le 2026-09-18 : 2,11 Gio téléchargés, 0 importé, demande bloquée « en cours »).
/// Texte brut : `render` l'échappe (le nom vient du tracker).
pub fn unmatched(
    records: &BTreeMap<String, homelab_core::state::TorrentImportRecord>,
    now: i64,
    max: usize,
) -> Vec<String> {
    let mut v: Vec<(i64, String)> = records
        .iter()
        .filter(|(_, r)| matches!(r.outcome.as_str(), "no_match" | "nothing_importable"))
        .map(|(k, r)| {
            let side = k.split_once(':').map(|(s, _)| s).unwrap_or("?");
            (
                r.at,
                format!(
                    "{side} · {} — {} (depuis {})",
                    r.name.chars().take(60).collect::<String>(),
                    if r.outcome == "nothing_importable" {
                        "rien d'importable"
                    } else {
                        "aucune fiche"
                    },
                    ago(now, r.at)
                ),
            )
        })
        .collect();
    v.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    v.into_iter().take(max).map(|(_, s)| s).collect()
}

/// « il y a 4 min », « il y a 2 h », « il y a 3 j ».
pub fn ago(now: i64, t: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..=59 => "à l'instant".into(),
        60..=3599 => format!("il y a {} min", d / 60),
        3600..=86_399 => format!("il y a {} h", d / 3600),
        _ => format!("il y a {} j", d / 86_400),
    }
}

/// État d'une tâche : `ok`, `err` ou `none` (jamais lancée depuis le démarrage de l'état).
pub fn task_state(r: Option<&RunInfo>) -> &'static str {
    match r.and_then(|r| r.last_ok) {
        Some(true) => "ok",
        Some(false) => "err",
        None => "none",
    }
}

/// « Alertes admin » : compteurs, date de la dernière livrée et les 8 dernières (la plus récente en haut). Vide si
/// aucune alerte n'est passée depuis l'ajout de cette trace (2026-10-07).
fn alerts_section(a: &AlertStats, now: i64) -> String {
    if a.recent.is_empty() && a.delivered == 0 && a.failed == 0 {
        return String::new();
    }
    let last = a
        .last_delivered_at
        .map(|t| format!("dernière livrée {}", ago(now, t)))
        .unwrap_or_else(|| "aucune livrée".into());
    let rows: String = a
        .recent
        .iter()
        .rev()
        .take(8)
        .map(|r| {
            format!(
                r#"<tr><td class="w">{when}</td><td class="w">{channels}</td><td class="s">{subject}</td></tr>"#,
                when = esc(&ago(now, r.at)),
                channels = esc(r.channels()),
                subject = esc(&r.subject)
            )
        })
        .collect();
    format!(
        r#"<h1 style="margin:14px 0 6px">Alertes admin · {ok} livrée(s), {ko} non livrée(s) · {last}</h1><table>{rows}</table>"#,
        ok = a.delivered,
        ko = a.failed,
        last = esc(&last)
    )
}

/// « Catalogue jamais regardé » : totaux, détail, voie russe et liste des plus gros titres jamais commencés (2026-10-08).
/// **Lecture seule** : un tableau, aucun formulaire ni bouton, rien qui supprime ; l'admin tranche à la main. Tout texte
/// venu des Arrs (titres) passe par `esc`. Avant le premier calcul (`None`), une ligne le dit.
fn catalogue_section(r: Option<&CatalogueReport>, now: i64) -> String {
    let Some(r) = r else {
        return r#"<h1 style="margin:14px 0 6px">Catalogue jamais regardé</h1><p class="gd">pas encore calculé (une fois par jour)</p>"#.into();
    };
    let share = |b: &CatalogueBucket| percent(b.bytes, r.catalogue.bytes);
    let card = |title: &str, value: String, detail: String, pct: f64| {
        format!(
            r#"<div class="g"><div class="gt"><span>{title}</span><b>{value}</b></div><div class="bar"><i class="none" style="width:{w}%"></i></div><div class="gd">{detail}</div></div>"#,
            title = esc(title),
            value = esc(&value),
            detail = esc(&detail),
            w = pct.clamp(0.0, 100.0).round() as u8
        )
    };
    let cards = format!(
        "{}{}{}",
        card(
            &format!("Jamais vus depuis {} j ou plus", r.min_age_days),
            format!("{} titre(s)", r.never_aged.titles),
            format!(
                "{} · {} % du catalogue ({})",
                human_size(r.never_aged.bytes),
                pct_text(share(&r.never_aged)),
                human_size(r.catalogue.bytes)
            ),
            share(&r.never_aged)
        ),
        card(
            "Jamais vus, tous âges",
            format!("{} titre(s)", r.never_all.titles),
            format!(
                "{} · {} % du catalogue",
                human_size(r.never_all.bytes),
                pct_text(share(&r.never_all))
            ),
            share(&r.never_all)
        ),
        card(
            "Séries en cours (arriéré normal)",
            format!("{} série(s)", r.backlog.series),
            format!(
                "{} épisode(s) non vus · ~{}",
                r.backlog.unseen_episodes,
                human_size(r.backlog.bytes)
            ),
            percent(r.backlog.bytes, r.catalogue.bytes)
        ),
    );
    let parts = |m: &BTreeMap<String, CatalogueBucket>| {
        m.iter()
            .map(|(k, b)| format!("{k} {} · {}", b.titles, human_size(b.bytes)))
            .collect::<Vec<_>>()
            .join(" ; ")
    };
    let split = |s: &CatalogueSplit| {
        format!(
            "par sorte {} — par côté {} — par demandeur {}",
            parts(&s.by_kind),
            parts(&s.by_side),
            parts(&s.by_requester)
        )
    };
    let mut notes = Vec::new();
    if r.never_aged.titles > 0 {
        notes.push(format!(
            "Jamais vus depuis {} j ou plus ({}) : {}.",
            r.min_age_days,
            r.never_aged.titles,
            split(&r.aged)
        ));
    }
    if r.never_all.titles > r.never_aged.titles {
        notes.push(format!(
            "Jamais vus, tous âges ({}) : {}.",
            r.never_all.titles,
            split(&r.all)
        ));
    }
    let recent = r.never_all.titles - r.never_aged.titles;
    if recent > 0 {
        let next = r
            .next_aged_at
            .map(|t| {
                if t <= now {
                    ", le prochain atteint le seuil au prochain calcul".to_string()
                } else {
                    format!(
                        ", le prochain atteint le seuil dans {} j",
                        (t - now + 86_399) / 86_400
                    )
                }
            })
            .unwrap_or_default();
        notes.push(format!(
            "{recent} titre(s) jamais vus ({}) sont arrivés il y a moins de {} j : trop tôt pour conclure{next}. Les fiches de la seedbox datent de la création des Arrs.",
            human_size(r.never_all.bytes - r.never_aged.bytes),
            r.min_age_days
        ));
    }
    if r.unmatched.titles > 0 {
        notes.push(format!(
            "{} fiche(s) ({}) sans élément Jellyfin correspondant : non évaluées.",
            r.unmatched.titles,
            human_size(r.unmatched.bytes)
        ));
    }
    if !r.playback_reporting {
        notes.push(
            "Playback Reporting n'a pas répondu : seuls les marqueurs « vu » et « en cours » des comptes comptent."
                .into(),
        );
    }
    let ru = &r.russian;
    if ru.series + ru.movies > 0 {
        notes.push(format!(
            "Voie russe : {} série(s) ({} épisode(s) disponible(s), {} lu(s)) · {} film(s) ({} vu(s)) · {}.",
            ru.series,
            ru.episodes,
            ru.episodes_seen,
            ru.movies,
            ru.movies_seen,
            human_size(ru.bytes)
        ));
    }
    let notes: String = notes
        .iter()
        .map(|n| format!(r#"<p class="gd" style="margin:4px 0">{}</p>"#, esc(n)))
        .collect();
    let rows = |list: &[CatalogueEntry], progress: bool| -> String {
        list.iter()
            .map(|e| {
                let seen = if progress {
                    format!(r#"<td class="w">{}/{}</td>"#, e.seen, e.total)
                } else if e.total > 1 {
                    format!(r#"<td class="w">{} ép.</td>"#, e.total)
                } else {
                    r#"<td class="w"></td>"#.to_string()
                };
                format!(
                    r#"<tr><td class="s">{title}</td><td class="w">{kind} · {side}</td><td class="w">{size}</td><td class="w">{added}</td><td class="w">{who}</td>{seen}</tr>"#,
                    title = esc(&e.title),
                    kind = esc(&e.kind),
                    side = esc(&e.side),
                    size = esc(&human_size(e.bytes)),
                    added = esc(&ago(now, e.added)),
                    who = esc(&e.requester),
                )
            })
            .collect()
    };
    let table = |heading: String, list: &[CatalogueEntry], progress: bool| {
        if list.is_empty() {
            String::new()
        } else {
            format!(
                r#"<h1 style="margin:14px 0 6px">{}</h1><table class="cat">{}</table>"#,
                esc(&heading),
                rows(list, progress)
            )
        }
    };
    let listed = table(
        format!(
            "Les plus gros titres jamais commencés depuis {} j ou plus ({} sur {})",
            r.min_age_days,
            r.listed.len(),
            r.never_aged.titles
        ),
        &r.listed,
        false,
    );
    let backlog = table(
        format!(
            "Séries commencées, le plus d'épisodes non vus ({} sur {}, volume non vu estimé)",
            r.backlog_listed.len(),
            r.backlog.series
        ),
        &r.backlog_listed,
        true,
    );
    format!(
        r#"<h1 style="margin:14px 0 6px">Catalogue jamais regardé · calculé {at}</h1><div class="gs" style="grid-template-columns:repeat(auto-fit,minmax(220px,1fr))">{cards}</div>{notes}{listed}{backlog}"#,
        at = esc(&ago(now, r.at)),
    )
}

fn gauge(title: &str, pct: Option<u8>, detail: &str) -> String {
    let (p, cls) = match pct {
        Some(p) if p >= 90 => (p, "err"),
        Some(p) if p >= 80 => (p, "warn"),
        Some(p) => (p, "ok"),
        None => (0, "none"),
    };
    let value = pct.map(|p| format!("{p} %")).unwrap_or_else(|| "—".into());
    format!(
        r#"<div class="g"><div class="gt"><span>{title}</span><b>{value}</b></div><div class="bar"><i class="{cls}" style="width:{p}%"></i></div><div class="gd">{detail}</div></div>"#,
        title = esc(title),
        detail = esc(detail)
    )
}

pub fn render(d: &PageData<'_>) -> String {
    let mut rows = String::new();
    let (mut ok, mut err) = (0, 0);
    for t in d.tasks {
        let r = d.runs.get(*t);
        let st = task_state(r);
        match st {
            "ok" => ok += 1,
            "err" => err += 1,
            _ => {}
        }
        let when = r
            .and_then(|r| r.last_end.or(Some(r.last_start)))
            .map(|t| ago(d.now, t))
            .unwrap_or_else(|| "jamais".into());
        let summary: String = r
            .map(|r| r.last_summary.chars().take(90).collect())
            .unwrap_or_default();
        let note = r
            .and_then(|r| r.error_note(d.now))
            .map(|n| format!(r#"<div class="e">{}</div>"#, esc(&n)))
            .unwrap_or_default();
        rows.push_str(&format!(
            r#"<tr><td><i class="dot {st}"></i>{name}</td><td class="w">{when}</td><td class="s" title="{full}">{sum}{note}</td></tr>"#,
            name = esc(&homelab_core::tasks::label_of(t)),
            when = esc(&when),
            full = esc(&summary),
            sum = esc(&summary)
        ));
    }
    let headline = if err == 0 {
        format!(r#"<span class="pill ok">{ok} tâches OK</span>"#)
    } else {
        format!(
            r#"<span class="pill err">{err} en erreur</span> <span class="pill ok">{ok} OK</span>"#
        )
    };
    let sb = match &d.seedbox {
        Some(q) if q.quota_kb > 0 => {
            let pct = ((q.used_kb as f64 / q.quota_kb as f64) * 100.0)
                .round()
                .min(100.0) as u8;
            let detail = format!(
                "{:.0} Go sur {} To · relevé {}",
                q.used_kb as f64 / 1e6,
                format!("{:.1}", q.quota_kb as f64 / 1e9).replace('.', ","),
                ago(d.now, q.at)
            );
            gauge("Seedbox (quota)", Some(pct), &detail)
        }
        _ => gauge("Seedbox (quota)", None, "quota non disponible"),
    };
    let mount = match d.mount_ok {
        Some(true) => r#"<span class="pill ok">montage seedbox OK</span>"#,
        Some(false) => r#"<span class="pill err">montage seedbox absent</span>"#,
        None => "",
    };
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="60"><title>État de l'automatisation</title><style>
:root{{color-scheme:dark}}*{{box-sizing:border-box}}
body{{margin:0;padding:12px 14px;background:#141517;color:#dfe5ea;font:13px/1.45 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}}
header{{display:flex;flex-wrap:wrap;gap:8px;align-items:center;justify-content:space-between;margin-bottom:10px}}
h1{{font-size:14px;margin:0;font-weight:600;letter-spacing:.02em}}
.pill{{display:inline-block;padding:1px 8px;border-radius:999px;font-size:11.5px;font-variant-numeric:tabular-nums}}
.pill.ok{{background:#12321f;color:#6ee7a0}}.pill.err{{background:#3b1518;color:#fca5a5}}
.gs{{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:10px;margin-bottom:12px}}
.g{{background:#1b1d20;border:1px solid #2a2d31;border-radius:8px;padding:8px 10px}}
.gt{{display:flex;justify-content:space-between;font-size:12px;color:#9aa6b1}}.gt b{{color:#dfe5ea;font-variant-numeric:tabular-nums}}
.bar{{height:6px;background:#2a2d31;border-radius:3px;margin:6px 0 4px;overflow:hidden}}.bar i{{display:block;height:100%}}
i.ok{{background:#22c55e}}i.warn{{background:#f59e0b}}i.err{{background:#ef4444}}i.none{{background:#4b5563}}
.gd{{font-size:11px;color:#8591a0}}
table{{width:100%;border-collapse:collapse}}td{{padding:5px 4px;border-top:1px solid #24272b;vertical-align:top}}
td.w{{white-space:nowrap;color:#9aa6b1;font-variant-numeric:tabular-nums;width:1%}}td.s{{color:#8591a0;font-family:ui-monospace,Menlo,Consolas,monospace;font-size:11.5px;word-break:break-word}}
.dot{{display:inline-block;width:8px;height:8px;border-radius:50%;margin-right:8px;vertical-align:1px}}
.dot.ok{{background:#22c55e}}.dot.err{{background:#ef4444}}.dot.none{{background:#4b5563}}
.e{{margin-top:2px;color:#d6a45a}}
@media(max-width:600px){{table.cat tr{{display:block;border-top:1px solid #24272b;padding:4px 0}}table.cat td{{display:inline-block;width:auto;border:0;padding:1px 10px 1px 4px}}table.cat td.s{{display:block;padding-bottom:2px}}}}
</style></head><body>
<header><h1>Automatisation homelabd</h1><div>{headline} {mount}</div></header>
<div class="gs">{vps}{sb}</div>
<table>{rows}</table>{canary}{stuck}{blocked}{catalogue}{alerts}
</body></html>"#,
        alerts = alerts_section(d.alerts, d.now),
        catalogue = catalogue_section(d.catalogue, d.now),
        vps = gauge(
            "Disque VPS",
            d.vps_disk_pct,
            "médias, téléchargements, état des services"
        ),
        canary = match &d.canary {
            Some((ok, text)) => format!(
                r#"<p class="{cls}" style="margin:12px 0 0"><b>Canari de lecture</b> : {text}</p>"#,
                cls = if *ok { "ok" } else { "err" },
                text = esc(text),
            ),
            None => String::new(),
        },
        stuck = if d.stuck_torrents.is_empty() {
            String::new()
        } else {
            format!(
                r#"<h1 style="margin:14px 0 6px">Rien ne bouge ({n})</h1><table>{rows}</table>"#,
                n = d.stuck_torrents.len(),
                rows = d
                    .stuck_torrents
                    .iter()
                    .map(|t| format!("<tr><td class=\"s\">{}</td></tr>", esc(t)))
                    .collect::<String>()
            )
        },
        blocked = if d.blocked_seasons.is_empty() {
            String::new()
        } else {
            format!(
                r#"<h1 style="margin:14px 0 6px">Saisons sans release ({n})</h1><table>{rows}</table>"#,
                n = d.blocked_seasons.len(),
                rows = d
                    .blocked_seasons
                    .iter()
                    .map(|t| format!("<tr><td class=\"s\">{}</td></tr>", esc(t)))
                    .collect::<String>()
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ok: Option<bool>, end: i64, summary: &str) -> RunInfo {
        RunInfo {
            last_start: end - 5,
            last_end: Some(end),
            last_ok: ok,
            last_summary: summary.into(),
            runs: 1,
            errors: 0,
            ..Default::default()
        }
    }

    #[test]
    fn the_last_error_shows_only_when_recent_or_current() {
        let now = 1_790_000_000;
        // jamais d'erreur datée (ancien état) : rien
        assert_eq!(run(Some(true), now, "ok").error_note(now), None);
        // erreur d'il y a 3 jours, tâche rétablie : la note dit combien et quand
        let mut r = run(Some(true), now, "ok");
        r.record_outcome(
            now - 3 * 86_400,
            false,
            "error: sonarr-seedbox: connection refused",
        );
        r.record_outcome(now - 3 * 86_400 + 60, true, "ok");
        let n = r.error_note(now).expect("note");
        assert!(n.contains("0 aujourd'hui, 1 sur 7 j"), "{n}");
        assert!(n.ends_with(": sonarr-seedbox: connection refused"), "{n}");
        // erreur d'il y a 20 jours, tâche rétablie depuis : plus rien (l'ancien compteur cumulé faisait peur)
        let mut old = run(Some(true), now, "ok");
        old.record_outcome(now - 20 * 86_400, false, "error: x");
        old.record_outcome(now - 20 * 86_400 + 60, true, "ok");
        assert_eq!(old.error_note(now), None);
        // tâche en échec en ce moment : toujours affichée, avec la série
        let mut cur = run(Some(true), now, "ok");
        for i in 0..3 {
            cur.record_outcome(
                now - 600 + i * 60,
                false,
                "error: jellyfin Items: operation timed out",
            );
        }
        let n = cur.error_note(now).expect("note");
        assert!(n.starts_with("3 échecs de suite · 3 aujourd'hui"), "{n}");
    }

    #[test]
    fn delivered_and_missed_alerts_are_listed_newest_first() {
        let mut a = AlertStats::default();
        assert_eq!(
            alerts_section(&a, 1000),
            "",
            "rien tant qu'aucune alerte n'est passée"
        );
        a.record(100, "Seedbox : Sonarr injoignable", true, true);
        a.record(500, "<b>Disque</b> à 86 %", false, false);
        let html = alerts_section(&a, 1000);
        assert!(html.contains("1 livrée(s), 1 non livrée(s)"), "{html}");
        assert!(html.contains("dernière livrée il y a 15 min"), "{html}");
        assert!(html.contains("mail + Discord") && html.contains("non livrée"));
        assert!(html.contains("&lt;b&gt;Disque&lt;/b&gt;") && !html.contains("<b>Disque</b>"));
        assert!(
            html.find("Disque").unwrap() < html.find("Sonarr").unwrap(),
            "la plus récente en haut"
        );
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(1000, 990), "à l'instant");
        assert_eq!(ago(10_000, 10_000 - 240), "il y a 4 min");
        assert_eq!(ago(100_000, 100_000 - 7200), "il y a 2 h");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86_400), "il y a 3 j");
    }

    #[test]
    fn renders_states_quota_and_escapes() {
        let mut runs = BTreeMap::new();
        runs.insert(
            "stack_health".to_string(),
            run(Some(true), 1000, "healthy=18"),
        );
        runs.insert(
            "torrent_import".to_string(),
            run(Some(false), 900, "<b>boom</b>"),
        );
        let tasks = ["stack_health", "torrent_import", "cleanup"];
        let html = render(&PageData {
            now: 1060,
            runs: &runs,
            tasks: &tasks,
            vps_disk_pct: Some(74),
            stuck_torrents: &[],
            blocked_seasons: &[],
            canary: None,
            catalogue: None,
            alerts: &AlertStats::default(),
            seedbox: Some(SeedboxQuota {
                used_kb: 1_429_000_000,
                quota_kb: 3_725_000_000,
                at: 1000,
            }),
            mount_ok: Some(true),
        });
        assert!(html.contains("1 en erreur"));
        assert!(html.contains("Santé des services"));
        assert!(html.contains("&lt;b&gt;boom&lt;/b&gt;"));
        assert!(!html.contains("<b>boom</b>"));
        assert!(html.contains("38 %"), "quota 1429/3725 Go ≈ 38 %");
        // section « rien ne bouge » : uniquement les torrents que personne n'a rattachés
        let mut recs = BTreeMap::new();
        recs.insert(
            "seedbox:abc123def".to_string(),
            homelab_core::state::TorrentImportRecord {
                at: 900,
                name: "Un.Anime.S01.VOSTFR.1080p".into(),
                outcome: "no_match".into(),
                detail: String::new(),
                ..Default::default()
            },
        );
        recs.insert(
            "vps:xyz".to_string(),
            homelab_core::state::TorrentImportRecord {
                at: 950,
                name: "Un.Film.2024".into(),
                outcome: "imported".into(),
                detail: String::new(),
                ..Default::default()
            },
        );
        let stuck = unmatched(&recs, 1000, 15);
        assert_eq!(stuck.len(), 1, "seul le no_match");
        assert!(stuck[0].contains("seedbox") && stuck[0].contains("Un.Anime"));
        assert!(html.contains("jamais"));
        assert!(html.contains("74 %"));
    }

    #[test]
    fn episode_ranges_stay_short() {
        assert_eq!(episode_list(&[27, 28, 29, 30, 40]), "27-30, 40");
        assert_eq!(episode_list(&[5]), "5");
        assert_eq!(episode_list(&[3, 1, 2]), "1-3");
    }

    #[test]
    fn only_seasons_the_indexer_cannot_serve_are_listed() {
        use homelab_core::state::SeasonSearchRecord;
        let rec = |outcome: &str, title: &str, uncovered: Vec<i64>| SeasonSearchRecord {
            at: 900,
            outcome: outcome.into(),
            detail: String::new(),
            title: title.into(),
            uncovered,
        };
        let mut recs = BTreeMap::new();
        recs.insert(
            "sonarr-seedbox:58:17".to_string(),
            rec("grabbed_episode", "Bleach", (27..=40).collect()),
        );
        // saison servie entièrement : rien à signaler
        recs.insert(
            "sonarr-seedbox:12:1".to_string(),
            rec("grabbed", "Autre", Vec::new()),
        );
        let out = blocked_seasons(&recs, 1000, 15);
        assert_eq!(out.len(), 1, "seule la saison à trous");
        assert!(out[0].contains("Bleach S17"), "{}", out[0]);
        assert!(out[0].contains("14 épisode(s)"), "{}", out[0]);
        assert!(out[0].contains("27-40"), "{}", out[0]);
    }

    /// Un nom de release ou un titre piégé (tracker public, fiche TVDB) reste du texte : aucune balise ne
    /// passe dans la page, servie sur la même origine que `/accounts` et `/onboard`.
    #[test]
    fn trapped_names_are_escaped() {
        use homelab_core::state::{SeasonSearchRecord, TorrentImportRecord};
        let trap = r#"<img src=x onerror="alert(1)">"#;
        let mut recs = BTreeMap::new();
        recs.insert(
            "seedbox<svg onload=alert(2)>:abc".to_string(),
            TorrentImportRecord {
                at: 900,
                name: format!("Un.Anime.S01 {trap}"),
                outcome: "no_match".into(),
                detail: String::new(),
                ..Default::default()
            },
        );
        let mut seasons = BTreeMap::new();
        seasons.insert(
            "sonarr-seedbox:58:17".to_string(),
            SeasonSearchRecord {
                at: 900,
                outcome: "grabbed_episode".into(),
                detail: String::new(),
                title: format!("Bleach {trap}"),
                uncovered: vec![27, 28],
            },
        );
        let stuck = unmatched(&recs, 1000, 15);
        let blocked = blocked_seasons(&seasons, 1000, 15);
        let runs = BTreeMap::new();
        let html = render(&PageData {
            now: 1000,
            runs: &runs,
            tasks: &[],
            vps_disk_pct: None,
            seedbox: None,
            mount_ok: None,
            stuck_torrents: &stuck,
            blocked_seasons: &blocked,
            canary: None,
            catalogue: None,
            alerts: &AlertStats::default(),
        });
        assert!(!html.contains("<img"), "balise passée telle quelle");
        assert!(!html.contains("<svg"), "côté passé tel quel");
        assert_eq!(html.matches("&lt;img src=x onerror=&quot;").count(), 2);
        assert!(html.contains("seedbox&lt;svg onload=alert(2)&gt;"));
        assert!(html.contains("Bleach &lt;img"));
    }

    #[test]
    fn missing_quota_is_explicit() {
        let runs = BTreeMap::new();
        let html = render(&PageData {
            now: 0,
            runs: &runs,
            tasks: &[],
            vps_disk_pct: None,
            seedbox: None,
            mount_ok: None,
            stuck_torrents: &[],
            blocked_seasons: &[],
            canary: None,
            catalogue: None,
            alerts: &AlertStats::default(),
        });
        assert!(html.contains("quota non disponible"));
    }

    fn catalogue_sample() -> CatalogueReport {
        let entry = |title: &str, kind: &str, bytes: u64, who: &str, seen: u32, total: u32| {
            CatalogueEntry {
                title: title.into(),
                kind: kind.into(),
                side: "vps".into(),
                bytes,
                added: 1_000_000 - 90 * 86_400,
                requester: who.into(),
                seen,
                total,
            }
        };
        let mut r = CatalogueReport {
            at: 1_000_000 - 3 * 3600,
            min_age_days: 60,
            catalogue: CatalogueBucket {
                titles: 40,
                bytes: 1_000_000_000_000,
            },
            unmatched: CatalogueBucket {
                titles: 1,
                bytes: 200_000_000,
            },
            never_all: CatalogueBucket {
                titles: 12,
                bytes: 400_000_000_000,
            },
            never_aged: CatalogueBucket {
                titles: 2,
                bytes: 25_000_000_000,
            },
            next_aged_at: Some(1_000_000 + 2 * 86_400 + 5),
            playback_reporting: true,
            listed: vec![
                entry("Gros film", "film", 15_000_000_000, "membre", 0, 0),
                entry("Une série", "série", 10_000_000_000, "aucune", 0, 24),
            ],
            backlog_listed: vec![entry("En cours", "animé", 80_000_000_000, "admin", 5, 100)],
            ..Default::default()
        };
        r.backlog.series = 3;
        r.backlog.unseen_episodes = 150;
        r.backlog.bytes = 120_000_000_000;
        r.aged.add("film", "vps", "membre", 15_000_000_000);
        r.aged.add("série", "vps", "aucune", 10_000_000_000);
        r.all.add("film", "vps", "membre", 15_000_000_000);
        r.all.add("série", "seedbox", "aucune", 10_000_000_000);
        r.russian.series = 2;
        r.russian.episodes = 120;
        r.russian.episodes_seen = 3;
        r
    }

    #[test]
    fn catalogue_report_shows_totals_lists_and_dates() {
        let html = catalogue_section(Some(&catalogue_sample()), 1_000_000);
        assert!(html.contains("calculé il y a 3 h"), "{html}");
        assert!(html.contains("Jamais vus depuis 60 j ou plus"), "{html}");
        assert!(html.contains("2 titre(s)"), "{html}");
        assert!(
            html.contains("25,0 Go · 2,5 % du catalogue (1,00 To)"),
            "{html}"
        );
        assert!(
            html.contains("12 titre(s)") && html.contains("40,0 % du catalogue"),
            "{html}"
        );
        assert!(
            html.contains("3 série(s)") && html.contains("150 épisode(s) non vus"),
            "{html}"
        );
        // 10 titres plus récents que le seuil, le prochain dans 3 jours (arrondi au jour supérieur)
        assert!(html.contains("10 titre(s) jamais vus (375 Go)"), "{html}");
        assert!(
            html.contains("le prochain atteint le seuil dans 3 j"),
            "{html}"
        );
        assert!(
            html.contains("1 fiche(s) (0,2 Go) sans élément Jellyfin"),
            "{html}"
        );
        assert!(
            html.contains("Voie russe : 2 série(s) (120 épisode(s) disponible(s), 3 lu(s)) · 0 film(s) (0 vu(s))"),
            "{html}"
        );
        // liste triée par taille telle que le calcul l'a rendue, avec la sorte de demandeur
        assert!(html.find("Gros film").unwrap() < html.find("Une série").unwrap());
        assert!(
            html.contains(r#"<td class="w">film · vps</td>"#) && html.contains("15,0 Go"),
            "{html}"
        );
        assert!(
            html.contains(">membre</td>") && html.contains(">aucune</td>"),
            "{html}"
        );
        assert!(html.contains("il y a 90 j"), "{html}");
        assert!(
            html.contains(">5/100</td>"),
            "série commencée : vus sur total"
        );
        assert!(html.contains(">24 ép.</td>"), "{html}");
        // les deux tableaux sont nommés
        assert!(
            html.contains("(2 sur 2)") && html.contains("(1 sur 3, volume non vu estimé)"),
            "{html}"
        );
    }

    /// Consultation seule : ni formulaire, ni bouton, ni lien, ni script — rien qui puisse supprimer quoi que ce soit.
    #[test]
    fn catalogue_report_has_nothing_to_click() {
        let html = catalogue_section(Some(&catalogue_sample()), 1_000_000);
        for forbidden in [
            "<form", "<button", "<a ", "<input", "<script", "onclick", "method=", "href=",
        ] {
            assert!(
                !html.contains(forbidden),
                "{forbidden} dans la section : {html}"
            );
        }
    }

    #[test]
    fn catalogue_report_escapes_every_title() {
        let trap = r#"<img src=x onerror="alert(1)">"#;
        let mut r = catalogue_sample();
        r.listed[0].title = format!("Film {trap}");
        r.backlog_listed[0].title = format!("Série <svg onload=alert(2)> {trap}");
        r.listed[1].kind = "<b>sorte</b>".into();
        r.listed[1].requester = "<i>qui</i>".into();
        r.aged.add("<u>x</u>", "<s>y</s>", "<em>z</em>", 1);
        r.all.add("<u>x</u>", "<s>y</s>", "<em>z</em>", 1);
        let html = catalogue_section(Some(&r), 1_000_000);
        for tag in [
            "<img", "<svg", "<b>sorte", "<i>qui", "<u>x", "<s>y", "<em>z",
        ] {
            assert!(!html.contains(tag), "{tag} passé tel quel : {html}");
        }
        assert!(
            html.contains("Film &lt;img src=x onerror=&quot;alert(1)&quot;&gt;"),
            "{html}"
        );
        assert!(html.contains("&lt;svg onload=alert(2)&gt;"), "{html}");
        // et la page entière, par `render`
        let runs = BTreeMap::new();
        let page = render(&PageData {
            now: 1_000_000,
            runs: &runs,
            tasks: &[],
            vps_disk_pct: None,
            seedbox: None,
            mount_ok: None,
            stuck_torrents: &[],
            blocked_seasons: &[],
            canary: None,
            catalogue: Some(&r),
            alerts: &AlertStats::default(),
        });
        assert!(page.contains("Catalogue jamais regardé") && !page.contains("<img"));
    }

    #[test]
    fn catalogue_report_before_the_first_run_and_without_candidates() {
        let waiting = catalogue_section(None, 1_000_000);
        assert!(waiting.contains("pas encore calculé"), "{waiting}");
        // catalogue vide ou rien à examiner : ni division par zéro, ni tableau vide
        let empty = catalogue_section(
            Some(&CatalogueReport {
                at: 1_000_000,
                min_age_days: 60,
                ..Default::default()
            }),
            1_000_000,
        );
        assert!(
            empty.contains("0 titre(s)") && empty.contains("0,0 % du catalogue"),
            "{empty}"
        );
        assert!(!empty.contains("<table"), "{empty}");
        assert!(
            empty.contains("Playback Reporting n&#39;a pas répondu"),
            "{empty}"
        );
        // le prochain titre a déjà passé le seuil : pas de « dans -3 j »
        let mut r = catalogue_sample();
        r.next_aged_at = Some(1_000_000 - 10);
        assert!(catalogue_section(Some(&r), 1_000_000).contains("au prochain calcul"));
    }
}
