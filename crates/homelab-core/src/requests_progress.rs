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
    /// `downloading`, `importPending`, `importing`, `imported`, `failedPending`… ; pour un torrent étiqueté :
    /// `importBlocked`, `importPartial` (rangé en partie, voir `left_out`), `shelved` (rangé, jugé à `judged_at`).
    pub tracked_state: String,
    /// `importPartial` : vidéos laissées de côté par `torrent_import`.
    pub left_out: usize,
    /// `shelved` : date de la décision de `torrent_import`.
    pub judged_at: Option<i64>,
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
/// `None` quand le torrent a déjà été rangé (`imported`, `arr_managed`) : les fichiers de l'Arr font foi (l'appelant
/// le garde de côté avec [`shelved`]). `partial` ⇒ `importPartial` (des vidéos sont restées de côté).
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
            ..Default::default()
        });
    }
    let state = match record_outcome {
        None | Some("retry") => "importPending",
        Some("imported") | Some("arr_managed") => return None,
        Some("partial") => "importPartial",
        Some(_) => "importBlocked",
    };
    Some(QueueSummary {
        season,
        size,
        size_left: 0.0,
        time_left_secs: Some(0),
        tracked_state: state.into(),
        ..Default::default()
    })
}

/// Torrent étiqueté complet et rangé par `torrent_import` (décision à `at`) : il ne compte plus dans la file, mais
/// prouve que le téléchargement n'attend plus qBittorrent (`progress`, branche « grabbed »). Le 2026-10-18, la carte de
/// One Piece S23 a affiché « en attente de qBittorrent » deux jours après la fin du torrent.
pub fn shelved(season: Option<i64>, at: i64) -> QueueSummary {
    QueueSummary {
        season,
        tracked_state: "shelved".into(),
        judged_at: Some(at),
        ..Default::default()
    }
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
        // secours sans résultat (C411 en panne) : 12 h au plus, souvent moins (retour de C411)
        "fallback_none" => 12 * 3600,
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
/// - `queue` : éléments de file Arr rattachés à la demande (vide si rien ne se télécharge), dont les torrents
///   étiquetés déjà rangés (`shelved`), qui ne servent qu'à la branche « grabbed » ;
/// - `has_file` : l'Arr a déjà au moins un fichier pour ce qui est demandé (série : toutes les saisons demandées
///   complètes) ; tout torrent restant alors bloqué ou rangé en partie est ignoré, les fichiers sont là ;
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
    // torrents étiquetés déjà rangés : hors file, seule la branche « grabbed » s'en sert
    let shelved_items = || queue.iter().filter(|q| q.tracked_state == "shelved");
    let judged_at = shelved_items().filter_map(|q| q.judged_at).max();
    // une saison rangée sans être complète : « en partie » ; un film (pas de saison) : seulement « téléchargé »
    let season_pack = shelved_items().any(|q| q.season.is_some());
    let live: Vec<&QueueSummary> = queue
        .iter()
        .filter(|q| q.tracked_state != "shelved")
        .collect();
    let all_blocked = !live.is_empty()
        && live
            .iter()
            .all(|q| matches!(q.tracked_state.as_str(), "importBlocked" | "importPartial"));
    // fichiers déjà là (rangés à la main, ou par un autre torrent) : un torrent bloqué ou rangé en partie ne dit plus
    // rien de la demande
    if !live.is_empty() && !(has_file && all_blocked) {
        let size: f64 = live.iter().map(|q| q.size).sum();
        let left: f64 = live.iter().map(|q| q.size_left).sum();
        let importing = live.iter().all(|q| {
            matches!(
                q.tracked_state.as_str(),
                "importPending" | "importing" | "imported" | "importBlocked" | "importPartial"
            )
        });
        if all_blocked {
            // rangé en partie (One Piece S23, 2026-10-18 : 7 épisodes sur 25) : `torrent_import` a alerté l'admin
            if live.iter().any(|q| q.tracked_state == "importPartial") {
                let n: usize = live.iter().map(|q| q.left_out).sum();
                return Progress {
                    stage: Stage::Import,
                    percent: 92,
                    eta_secs: None,
                    label: if n == 0 {
                        "Téléchargé en partie : des épisodes pas rangés, l'administrateur est prévenu"
                            .into()
                    } else {
                        format!(
                            "Téléchargé en partie : {n} épisode(s) pas rangé(s), l'administrateur est prévenu"
                        )
                    },
                };
            }
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
        let tl = live.iter().filter_map(|q| q.time_left_secs).max();
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
        Some((rec, retry_h, grabbed_h, error_h)) if rec.outcome.starts_with("grabbed") => {
            match judged_at {
                // pris mais aucun torrent visible (qBittorrent injoignable, torrent retiré ou pas encore arrivé) : ce
                // n'est pas une « prochaine tentative dans 7 j »
                None => Progress {
                    stage: Stage::Download,
                    percent: 5,
                    eta_secs: None,
                    label: "Téléchargement lancé, en attente de qBittorrent".into(),
                },
                // torrent rangé à l'instant : l'Arr et Jellyfin n'ont pas encore vu les fichiers
                Some(at) if now - at < import_allowance + scan_delay => {
                    let eta = import_allowance + scan_delay - (now - at).max(0);
                    Progress {
                        stage: Stage::Import,
                        percent: 95,
                        eta_secs: Some(eta),
                        label: format!("Téléchargé, ajout à la médiathèque ({})", human_eta(eta)),
                    }
                }
                // torrent rangé, saison toujours incomplète : il ne couvrait pas tout. Après un épisode seul
                // (`grabbed_episode`), `series_search` reprend en quelques minutes : `next_search_in` surestimerait.
                Some(_) => {
                    let in_secs = next_search_in(rec, now, retry_h, grabbed_h, error_h);
                    let soon = rec.outcome == "grabbed_episode" || in_secs == 0;
                    Progress {
                        stage: Stage::Search,
                        percent: 3,
                        eta_secs: None,
                        label: match (season_pack, soon) {
                            (true, true) => "Téléchargé en partie : le reste est recherché".into(),
                            (true, false) => format!(
                                "Téléchargé en partie : le reste sera recherché dans {}",
                                human_eta(in_secs)
                            ),
                            (false, true) => "Téléchargé : nouvelle recherche en cours".into(),
                            (false, false) => format!(
                                "Téléchargé : nouvelle recherche dans {}",
                                human_eta(in_secs)
                            ),
                        },
                    }
                }
            }
        }
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
            ..Default::default()
        }
    }

    /// One Piece S23 (2026-10-18) : saison prise à 18:13, torrent jugé par `torrent_import` à 18:21, carte lue le 20/10.
    const OP_GRAB: i64 = 1_792_339_980;
    const OP_JUDGED: i64 = 1_792_340_460;
    const OP_SEEN: i64 = 1_792_479_600;

    fn grabbed(outcome: &str, at: i64) -> SeasonSearchRecord {
        SeasonSearchRecord {
            at,
            outcome: outcome.into(),
            detail: String::new(),
            title: "One Piece".into(),
            uncovered: vec![],
        }
    }

    /// `progress` avec les délais de la production : recherche 24 h, reprise après une saison prise 168 h, erreur
    /// 1 h ; import 120 s, passage de `seedbox_refresh` 300 s.
    fn card(
        queue: &[QueueSummary],
        has_file: bool,
        rec: Option<&SeasonSearchRecord>,
        now: i64,
    ) -> Progress {
        progress(
            queue,
            has_file,
            false,
            rec.map(|r| (r, 24, 168, 1)),
            now,
            120,
            300,
            false,
        )
    }

    #[test]
    fn a_partial_import_is_not_waiting_for_qbittorrent() {
        // torrent complet, `torrent_import` : `partial`, 18 vidéos de côté ; la recherche de la saison dit « grabbed »
        let mut t = from_torrent(Some(23), 9_800_000_000, 1.0, 8_640_000, Some("partial")).unwrap();
        assert_eq!(t.tracked_state, "importPartial");
        t.left_out = 18;
        let rec = grabbed("grabbed", OP_GRAB);
        let p = card(&[t.clone()], false, Some(&rec), OP_SEEN);
        assert_eq!((p.stage, p.percent, p.eta_secs), (Stage::Import, 92, None));
        assert_eq!(
            p.label,
            "Téléchargé en partie : 18 épisode(s) pas rangé(s), l'administrateur est prévenu"
        );
        assert!(!p.label.contains("qBittorrent"), "{}", p.label);
        // liste inconnue (ancien enregistrement) : le libellé reste juste
        t.left_out = 0;
        let p = card(&[t], false, Some(&rec), OP_SEEN);
        assert!(p.label.contains("des épisodes pas rangés"), "{}", p.label);

        // l'état du 18/10, avant le correctif : issue `imported` ⇒ torrent rangé, saison à 7/25
        assert!(from_torrent(Some(23), 9_800_000_000, 1.0, 0, Some("imported")).is_none());
        let p = card(&[shelved(Some(23), OP_JUDGED)], false, Some(&rec), OP_SEEN);
        assert_eq!(p.stage, Stage::Search);
        assert!(!p.label.contains("qBittorrent"), "{}", p.label);
        assert_eq!(
            p.label, "Téléchargé en partie : le reste sera recherché dans ~5 j",
            "168 h après la prise du 18/10 18:13"
        );
    }

    #[test]
    fn files_already_there_hide_a_blocked_torrent() {
        let blocked = q(1000.0, 0.0, Some(0), "importBlocked");
        let mut partial = q(1000.0, 0.0, Some(0), "importPartial");
        partial.left_out = 18;
        // l'admin a rangé le reste à la main : les fichiers font foi
        for t in [&blocked, &partial] {
            let p = card(std::slice::from_ref(t), true, None, 0);
            assert_eq!(
                (p.stage, p.percent),
                (Stage::Import, 95),
                "{}",
                t.tracked_state
            );
            assert!(p.label.starts_with("Ajout à la médiathèque"), "{}", p.label);
        }
        // sans fichier : le torrent bloqué parle
        assert!(card(std::slice::from_ref(&blocked), false, None, 0)
            .label
            .contains("pas rangé"));
        // un autre torrent encore en cours : la file reste la référence
        let p = card(
            &[blocked, q(1000.0, 500.0, Some(60), "downloading")],
            true,
            None,
            0,
        );
        assert_eq!(p.stage, Stage::Download);
        // rangé en partie et rangé en attente : l'import est en cours
        let p = card(
            &[partial, q(1000.0, 0.0, Some(0), "importPending")],
            false,
            None,
            0,
        );
        assert!(p.label.starts_with("Téléchargé, ajout"), "{}", p.label);
    }

    #[test]
    fn grabbed_waits_for_qbittorrent_only_without_a_torrent() {
        let rec = grabbed("grabbed", OP_GRAB);
        // aucun torrent visible : en attente de qBittorrent, comme avant
        let p = card(&[], false, Some(&rec), OP_GRAB + 600);
        assert_eq!((p.stage, p.percent), (Stage::Download, 5));
        assert_eq!(p.label, "Téléchargement lancé, en attente de qBittorrent");
        // torrent en cours : son avancement
        let p = card(
            &[from_torrent(Some(23), 1000, 0.4, 600, None).unwrap()],
            false,
            Some(&rec),
            OP_GRAB + 600,
        );
        assert_eq!(p.stage, Stage::Download);
        assert!(p.label.starts_with("Téléchargement 40 %"), "{}", p.label);
        // torrent rangé il y a une minute : l'ajout suit
        let fresh = shelved(Some(23), OP_JUDGED);
        let p = card(
            std::slice::from_ref(&fresh),
            false,
            Some(&rec),
            OP_JUDGED + 60,
        );
        assert_eq!((p.stage, p.percent), (Stage::Import, 95));
        assert_eq!(p.eta_secs, Some(360));
        assert_eq!(p.label, "Téléchargé, ajout à la médiathèque (~6 min)");
        // rangé depuis longtemps et saison incomplète : le reste sera recherché
        let p = card(
            std::slice::from_ref(&fresh),
            false,
            Some(&rec),
            OP_JUDGED + 3600,
        );
        assert_eq!((p.stage, p.percent), (Stage::Search, 3));
        assert!(
            p.label
                .starts_with("Téléchargé en partie : le reste sera recherché dans"),
            "{}",
            p.label
        );
        // après un épisode seul, `series_search` reprend en quelques minutes : pas de délai de 7 j
        let ep = grabbed("grabbed_episode", OP_GRAB);
        let p = card(
            std::slice::from_ref(&fresh),
            false,
            Some(&ep),
            OP_JUDGED + 3600,
        );
        assert_eq!(p.label, "Téléchargé en partie : le reste est recherché");
        // délai de reprise écoulé
        let p = card(&[fresh], false, Some(&rec), OP_GRAB + 200 * 3600);
        assert_eq!(p.label, "Téléchargé en partie : le reste est recherché");
        // un film (étiquette sans saison) rangé sans fichier côté Radarr : pas de « en partie »
        let p = card(
            &[shelved(None, OP_JUDGED)],
            false,
            Some(&rec),
            OP_JUDGED + 3600,
        );
        assert!(
            p.label.starts_with("Téléchargé : nouvelle recherche dans"),
            "{}",
            p.label
        );
        // un torrent rangé n'empêche pas une autre recherche d'afficher son délai
        let none = grabbed("none", OP_GRAB);
        let p = card(
            &[shelved(Some(23), OP_JUDGED)],
            false,
            Some(&none),
            OP_GRAB + 3600,
        );
        assert!(
            p.label.starts_with("Recherche : prochaine tentative"),
            "{}",
            p.label
        );
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
