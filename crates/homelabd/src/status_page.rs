//! Page d'état HTML (tableau « Opérations » de Homarr, en iFrame) : dernier passage de chaque
//! tâche, disque du VPS, quota de la seedbox. Rendu côté serveur, sans JavaScript, thème sombre,
//! rechargée toutes les 60 s. Protégée par `HOMELABD_STATUS_TOKEN` (voir `web.rs`).

use std::collections::BTreeMap;

use homelab_core::state::RunInfo;
use serde::Deserialize;

/// Quota écrit par la seedbox (cron `quota -w`) dans `~/media/.homelab/quota.json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SeedboxQuota {
    pub used_kb: u64,
    pub quota_kb: u64,
    pub at: i64,
}

pub struct PageData<'a> {
    pub now: i64,
    pub runs: &'a BTreeMap<String, RunInfo>,
    /// Tâches planifiées, dans l'ordre d'affichage.
    pub tasks: &'a [&'static str],
    pub vps_disk_pct: Option<u8>,
    pub seedbox: Option<SeedboxQuota>,
    pub mount_ok: Option<bool>,
}

fn label(task: &str) -> &'static str {
    match task {
        "stack_health" => "Santé des services",
        "seedbox_refresh" => "Imports seedbox → Jellyfin",
        "tracker_ratio" => "Limites de partage",
        "stuck_handler" => "Téléchargements bloqués",
        "disk_pressure" => "Disque plein",
        "tba_bypass" => "Épisodes « TBA »",
        "id_match_import" => "Imports « matched by ID »",
        "torrent_import" => "Torrents ajoutés à la main",
        "series_search" => "Recherche des séries (TMDB)",
        "movie_search" => "Rattrapage des films (TMDB)",
        "indexer_unblock" => "Déblocage des indexeurs",
        "anime_library" => "Rangement des animés",
        "deletion_cleanup" => "Suppressions Jellyfin",
        "trending" => "Tendances de l'accueil",
        "monitor_sync" => "Saisons demandées",
        "user_poller" => "Comptes Jellyseerr",
        "cleanup" => "Nettoyage",
        _ => "Tâche",
    }
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// État d'une tâche : `ok`, `err` ou `none` (jamais lancée depuis le démarrage de l'état).
pub fn task_state(r: Option<&RunInfo>) -> &'static str {
    match r.and_then(|r| r.last_ok) {
        Some(true) => "ok",
        Some(false) => "err",
        None => "none",
    }
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
        rows.push_str(&format!(
            r#"<tr><td><i class="dot {st}"></i>{name}</td><td class="w">{when}</td><td class="s" title="{full}">{sum}</td></tr>"#,
            name = esc(label(t)),
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
</style></head><body>
<header><h1>Automatisation homelabd</h1><div>{headline} {mount}</div></header>
<div class="gs">{vps}{sb}</div>
<table>{rows}</table>
</body></html>"#,
        vps = gauge(
            "Disque VPS",
            d.vps_disk_pct,
            "médias, téléchargements, état des services"
        ),
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
        }
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
        assert!(html.contains("jamais"));
        assert!(html.contains("74 %"));
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
        });
        assert!(html.contains("quota non disponible"));
    }
}
