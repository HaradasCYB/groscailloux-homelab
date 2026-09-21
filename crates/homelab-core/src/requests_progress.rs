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

/// Étiquette qBittorrent posée par `series_search`/`movie_search` (`homelab:series=<id>[:season=<n>…]`,
/// `homelab:movie=<id>`) → (film ?, id Arr, saison). Les grabs côté seedbox ne passent **jamais** par la file
/// de Sonarr/Radarr (pas de Prowlarr là-bas) : sans cette lecture, la barre n'affichait aucun téléchargement.
pub fn homelab_tag(tags: &str) -> Option<(bool, i64, Option<i64>)> {
    tags.split(',').map(str::trim).find_map(|t| {
        let rest = t.strip_prefix("homelab:")?;
        let (kind, tail) = rest.split_once('=')?;
        let movie = match kind {
            "series" => false,
            "movie" => true,
            _ => return None,
        };
        let mut parts = tail.split(':');
        let id: i64 = parts.next()?.parse().ok()?;
        let season = parts
            .filter_map(|p| p.strip_prefix("season="))
            .find_map(|v| v.parse::<i64>().ok());
        Some((movie, id, season))
    })
}

/// Élément de file construit d'après un torrent étiqueté : `progress` 0–1, `eta` de qBittorrent
/// (`ETA_UNKNOWN` = pas d'estimation), `record_outcome` = issue enregistrée par `torrent_import` pour ce torrent.
/// `None` quand le torrent a déjà été rangé (`imported`, `arr_managed`) : les fichiers de l'Arr font foi.
pub fn from_torrent(
    season: Option<i64>,
    size: i64,
    progress: f64,
    eta: i64,
    record_outcome: Option<&str>,
) -> Option<QueueSummary> {
    let size = size.max(0) as f64;
    if progress < 1.0 {
        return Some(QueueSummary {
            season,
            size,
            size_left: size * (1.0 - progress.max(0.0)),
            time_left_secs: (0..crate::clients::ETA_UNKNOWN)
                .contains(&eta)
                .then_some(eta),
            tracked_state: "downloading".into(),
        });
    }
    let state = match record_outcome {
        None | Some("retry") => "importPending",
        Some("imported") | Some("arr_managed") => return None,
        Some(_) => "importBlocked",
    };
    Some(QueueSummary {
        season,
        size,
        size_left: 0.0,
        time_left_secs: Some(0),
        tracked_state: state.into(),
    })
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
        if queue.iter().all(|q| q.tracked_state == "importBlocked") {
            return Progress {
                stage: Stage::Import,
                percent: 92,
                eta_secs: None,
                label: "Téléchargé mais pas rangé : l'administrateur est prévenu".into(),
            };
        }
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
        // pris mais aucun torrent visible (qBittorrent injoignable, torrent retiré) : ce n'est pas une
        // « prochaine tentative dans 7 j »
        Some((rec, _, _, _)) if rec.outcome.starts_with("grabbed") => Progress {
            stage: Stage::Download,
            percent: 5,
            eta_secs: None,
            label: "Téléchargement lancé, en attente de qBittorrent".into(),
        },
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
    fn homelab_tags_are_read() {
        assert_eq!(
            homelab_tag("homelab:series=79:season=1"),
            Some((false, 79, Some(1)))
        );
        assert_eq!(
            homelab_tag("autre,homelab:series=60:season=17:offset=26:eps=27-40"),
            Some((false, 60, Some(17)))
        );
        assert_eq!(homelab_tag("homelab:movie=66"), Some((true, 66, None)));
        assert_eq!(homelab_tag("homelab:series=abc"), None);
        assert_eq!(homelab_tag("homelab:truc=1"), None);
        assert_eq!(homelab_tag(""), None);
    }

    #[test]
    fn torrent_becomes_queue_item() {
        let d = from_torrent(Some(1), 1000, 0.459, 207, None).unwrap();
        assert_eq!(d.tracked_state, "downloading");
        assert!((d.size_left - 541.0).abs() < 0.01);
        assert_eq!(d.time_left_secs, Some(207));
        let unknown = from_torrent(None, 1000, 0.5, 8_640_000, None).unwrap();
        assert_eq!(unknown.time_left_secs, None);
        assert_eq!(
            from_torrent(None, 1000, 1.0, 0, None)
                .unwrap()
                .tracked_state,
            "importPending"
        );
        assert_eq!(
            from_torrent(None, 1000, 1.0, 0, Some("retry"))
                .unwrap()
                .tracked_state,
            "importPending"
        );
        assert!(from_torrent(None, 1000, 1.0, 0, Some("imported")).is_none());
        assert!(from_torrent(None, 1000, 1.0, 0, Some("arr_managed")).is_none());
        assert_eq!(
            from_torrent(None, 1000, 1.0, 0, Some("nothing_importable"))
                .unwrap()
                .tracked_state,
            "importBlocked"
        );
        let p = progress(
            &[from_torrent(None, 1000, 1.0, 0, Some("no_match")).unwrap()],
            false,
            false,
            None,
            0,
            120,
            300,
            false,
        );
        assert!(p.label.contains("pas rangé"));
    }

    #[test]
    fn grabbed_without_torrent_is_not_a_search() {
        let rec = SeasonSearchRecord {
            at: 1000,
            outcome: "grabbed".into(),
            detail: String::new(),
            title: String::new(),
            uncovered: vec![],
        };
        let p = progress(
            &[],
            false,
            false,
            Some((&rec, 24, 168, 1)),
            2000,
            120,
            300,
            false,
        );
        assert_eq!(p.stage, Stage::Download);
        assert!(p.label.contains("lancé"));
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
