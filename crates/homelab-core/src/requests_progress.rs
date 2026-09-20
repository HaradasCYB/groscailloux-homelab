//! Avancement d'une demande Jellyseerr, pour la barre de progression de l'onglet Demandes de
//! Jellyfin : étape (recherche, téléchargement, ajout, disponible), pourcentage et estimation.
//! Fonctions pures et testées ; la collecte (Jellyseerr, files Sonarr/Radarr, état des recherches)
//! est dans `homelabd::subs_api::requests`.

use serde::Serialize;

use crate::state::SeasonSearchRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Search,
    Download,
    Import,
    Available,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub stage: Stage,
    /// 0–100 sur l'ensemble : recherche 0–5, téléchargement 5–90, ajout 90–99, disponible 100.
    pub percent: u8,
    /// Estimation restante en secondes, si on sait la calculer.
    pub eta_secs: Option<i64>,
    pub label: String,
}

/// Élément de file Arr résumé (un par épisode ou film).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueSummary {
    /// Saison (Sonarr), pour ne compter que les saisons encore manquantes d'une demande.
    pub season: Option<i64>,
    pub size: f64,
    pub size_left: f64,
    pub time_left_secs: Option<i64>,
    /// `downloading`, `importPending`, `importing`, `imported`, `failedPending`…
    pub tracked_state: String,
}

/// `hh:mm:ss` ou `d.hh:mm:ss` (format Sonarr/Radarr) → secondes.
pub fn parse_timeleft(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (days, rest) = match s.split_once('.') {
        Some((d, r)) if d.len() <= 3 && d.chars().all(|c| c.is_ascii_digit()) => {
            (d.parse::<i64>().ok()?, r)
        }
        _ => (0, s),
    };
    let parts: Vec<i64> = rest
        .split(':')
        .map(|p| p.parse::<i64>().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [h, m, sec] => h * 3600 + m * 60 + sec,
        [m, sec] => m * 60 + sec,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

/// Délai avant la prochaine tentative de recherche, d'après le dernier passage et les délais configurés
/// (`grabbed` → `grabbed_h`, `none` → `retry_h`, `error` → `error_h`).
pub fn next_search_in(
    rec: &SeasonSearchRecord,
    now: i64,
    retry_h: i64,
    grabbed_h: i64,
    error_h: i64,
) -> i64 {
    let delay = match rec.outcome.as_str() {
        "grabbed" | "grabbed_episode" => grabbed_h * 3600,
        "error" => error_h * 3600,
        _ => retry_h * 3600,
    };
    (rec.at + delay - now).max(0)
}

/// `2026-09-29` → `29/09/2026` (sinon la chaîne telle quelle).
pub fn date_fr(iso: &str) -> String {
    let p: Vec<&str> = iso.split('-').collect();
    match p.as_slice() {
        [y, m, d] if y.len() == 4 => format!("{d}/{m}/{y}"),
        _ => iso.to_string(),
    }
}

pub fn human_eta(secs: i64) -> String {
    if secs < 60 {
        "moins d'une minute".into()
    } else if secs < 3600 {
        format!("~{} min", (secs + 30) / 60)
    } else if secs < 48 * 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m >= 5 {
            format!("~{h} h {m:02}")
        } else {
            format!("~{h} h")
        }
    } else {
        format!("~{} j", (secs + 43_200) / 86_400)
    }
}

/// Décision d'étape.
/// - `queue` : éléments de file Arr rattachés à la demande (vide si rien ne se télécharge) ;
/// - `has_file` : l'Arr a déjà au moins un fichier pour ce qui est demandé ;
/// - `available` : Jellyseerr/Jellyfin voient le titre disponible (statut média 5, ou 4 pour une
///   série partiellement disponible dont toutes les saisons demandées ont un fichier) ;
/// - `search` : dernier passage de `series_search`/`movie_search` pour ce titre, si connu ;
/// - `import_allowance` et `scan_delay` : secondes ajoutées après la fin du téléchargement.
#[allow(clippy::too_many_arguments)]
pub fn progress(
    queue: &[QueueSummary],
    has_file: bool,
    available: bool,
    search: Option<(&SeasonSearchRecord, i64, i64, i64)>,
    now: i64,
    import_allowance: i64,
    scan_delay: i64,
    uncovered: bool,
) -> Progress {
    if available {
        return Progress {
            stage: Stage::Available,
            percent: 100,
            eta_secs: Some(0),
            label: "Disponible".into(),
        };
    }
    if !queue.is_empty() {
        let size: f64 = queue.iter().map(|q| q.size).sum();
        let left: f64 = queue.iter().map(|q| q.size_left).sum();
        let importing = queue.iter().all(|q| {
            matches!(
                q.tracked_state.as_str(),
                "importPending" | "importing" | "imported" | "importBlocked"
            )
        });
        if importing || (size > 0.0 && left <= 0.0) {
            return Progress {
                stage: Stage::Import,
                percent: 92,
                eta_secs: Some(import_allowance + scan_delay),
                label: format!(
                    "Téléchargé, ajout à la médiathèque ({})",
                    human_eta(import_allowance + scan_delay)
                ),
            };
        }
        let frac = if size > 0.0 {
            (1.0 - left / size).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let tl = queue.iter().filter_map(|q| q.time_left_secs).max();
        let eta = tl.map(|t| t + import_allowance + scan_delay);
        let pct = 5 + (frac * 85.0).round() as u8;
        return Progress {
            stage: Stage::Download,
            percent: pct.min(90),
            eta_secs: eta,
            label: match eta {
                Some(e) => format!(
                    "Téléchargement {:.0} % · disponible dans {}",
                    frac * 100.0,
                    human_eta(e)
                ),
                None => format!("Téléchargement {:.0} %", frac * 100.0),
            },
        };
    }
    if has_file {
        return Progress {
            stage: Stage::Import,
            percent: 95,
            eta_secs: Some(scan_delay),
            label: format!("Ajout à la médiathèque ({})", human_eta(scan_delay)),
        };
    }
    if uncovered {
        return Progress {
            stage: Stage::Search,
            percent: 2,
            eta_secs: None,
            label: "Introuvable pour l'instant : l'administrateur est prévenu".into(),
        };
    }
    match search {
        Some((rec, retry_h, grabbed_h, error_h)) => {
            let in_secs = next_search_in(rec, now, retry_h, grabbed_h, error_h);
            Progress {
                stage: Stage::Search,
                percent: 3,
                eta_secs: None,
                label: if in_secs == 0 {
                    "Recherche en cours".into()
                } else {
                    format!(
                        "Recherche : prochaine tentative dans {}",
                        human_eta(in_secs)
                    )
                },
            }
        }
        None => Progress {
            stage: Stage::Search,
            percent: 1,
            eta_secs: None,
            label: "En attente de recherche (quelques minutes)".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(size: f64, left: f64, tl: Option<i64>, st: &str) -> QueueSummary {
        QueueSummary {
            season: None,
            size,
            size_left: left,
            time_left_secs: tl,
            tracked_state: st.into(),
        }
    }

    #[test]
    fn timeleft_formats() {
        assert_eq!(parse_timeleft("00:12:34"), Some(754));
        assert_eq!(parse_timeleft("1.02:03:04"), Some(86_400 + 7384));
        assert_eq!(parse_timeleft("12:34"), Some(754));
        assert_eq!(parse_timeleft(""), None);
        assert_eq!(parse_timeleft("n/a"), None);
    }

    #[test]
    fn stages_in_order() {
        let p = progress(&[], false, true, None, 0, 120, 300, false);
        assert_eq!((p.stage, p.percent), (Stage::Available, 100));
        let p = progress(
            &[q(1000.0, 250.0, Some(600), "downloading")],
            false,
            false,
            None,
            0,
            120,
            300,
            false,
        );
        assert_eq!(p.stage, Stage::Download);
        assert_eq!(p.percent, 5 + 64);
        assert_eq!(p.eta_secs, Some(600 + 420));
        assert!(p.label.contains("75 %"));
        let p = progress(
            &[q(1000.0, 0.0, None, "importPending")],
            false,
            false,
            None,
            0,
            120,
            300,
            false,
        );
        assert_eq!((p.stage, p.percent), (Stage::Import, 92));
        let p = progress(&[], true, false, None, 0, 120, 300, false);
        assert_eq!((p.stage, p.percent), (Stage::Import, 95));
        let rec = SeasonSearchRecord {
            at: 1000,
            outcome: "none".into(),
            detail: String::new(),
            title: String::new(),
            uncovered: vec![],
        };
        let p = progress(
            &[],
            false,
            false,
            Some((&rec, 24, 168, 1)),
            1000 + 3600,
            120,
            300,
            false,
        );
        assert_eq!(p.stage, Stage::Search);
        assert!(p.label.contains("~23 h"));
        let p = progress(&[], false, false, None, 0, 120, 300, true);
        assert!(p.label.contains("Introuvable"));
    }

    #[test]
    fn dates_are_french() {
        assert_eq!(date_fr("2026-09-29"), "29/09/2026");
        assert_eq!(date_fr("bientôt"), "bientôt");
    }

    #[test]
    fn eta_wording() {
        assert_eq!(human_eta(30), "moins d'une minute");
        assert_eq!(human_eta(754), "~13 min");
        assert_eq!(human_eta(7384), "~2 h");
        assert_eq!(human_eta(7800), "~2 h 10");
        assert_eq!(human_eta(3 * 86_400), "~3 j");
    }
}
