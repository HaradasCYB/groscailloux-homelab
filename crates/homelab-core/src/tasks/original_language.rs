//! Langue d'origine des fiches Jellyfin (`OriginalLanguage`), remplie depuis TMDB (2026-10-08, lot 4 de la revue du
//! 07/10).
//!
//! Jellyfin 12 sait choisir côté serveur la piste de la langue d'origine (préférence audio « OriginalLanguage »,
//! valable pour toutes les applis, télés comprises), mais la métadonnée était vide sur 262 fiches sur 275 (176 films,
//! 86 séries) : elle n'est remplie qu'au rafraîchissement, et le rafraîchissement d'une série relit ses épisodes par
//! le lien seedbox. Ce que dit le code de Jellyfin 12.1 (lu le 2026-10-08, puis vérifié sur une instance d'essai) :
//! - stockage : colonne `BaseItems.OriginalLanguage`, code ISO 639-1 (`ja`, `en`, `fr` : le setter ramène tout code
//!   connu, `jpn` compris, à deux lettres), NULL quand elle est vide (jamais une chaîne vide en base) ;
//! - héritage : un épisode ou une saison sans valeur prend celle de sa série (`GetInheritedOriginalLanguage` :
//!   `OriginalLanguage ?? Series…`). Vérifié sur l'essai : la série écrite, son épisode resté NULL passe en piste
//!   japonaise pour un compte réglé sur « OriginalLanguage ». Les épisodes ne sont donc jamais écrits ;
//! - écriture : `POST /Items/{id}` (l'éditeur de métadonnées), sans rafraîchissement ni lecture vidéo. Ce point
//!   d'entrée **remet à vide** tout champ « toujours écrit » absent du corps (nom, résumé, dates, notes,
//!   classification…) et ne touche aux listes (personnes, genres, studios, étiquettes, identifiants, champs
//!   verrouillés) que si elles sont présentes. Renvoyer la fiche entière n'est PAS neutre : les personnes perdent
//!   leur ordre TMDB (`SortOrder` remis à NULL, 22 personnes sur l'essai), et une fiche qui a des vignettes de
//!   navigation (`Trickplay`) fait répondre 500. D'où `update_body` : les seuls champs toujours écrits, relus juste
//!   avant, plus la langue ;
//! - effets de bord mesurés pour un film (copies de la base avant/après) : la langue, `DateLastSaved`, les lignes
//!   d'images réécrites à l'identique (nouvel id) et les étiquettes héritées (`InheritedTags`) recalculées comme à
//!   toute sauvegarde (Jellyfin les efface toutes à la fin de chaque analyse de la médiathèque). Aucun fichier écrit
//!   dans les dossiers médias (bibliothèques en `MetadataSavers` vide), aucune ligne NotifySync changée, Intro Skipper
//!   ne réagit pas (`AutoDetectIntros = false`). Une note communautaire à 0 devient vide (Jellyfin ne l'envoie pas
//!   sous 0,1 : 5 fiches sur 262, rien d'affiché dans les deux cas) ;
//! - pour une **série**, Jellyfin réécrit aussi chaque saison et chaque épisode (`ItemUpdateController`) : leur
//!   classification (`OfficialRating`) vide reçoit celle de la série (1 458 épisodes et 130 saisons sur la copie du
//!   08/10), la valeur qu'ils héritaient déjà. D'où l'interrupteur `series` (false par défaut) et, pour revenir en
//!   arrière, la liste des enfants sans classification notée dans l'état (`children_unrated`).
//!
//! Garde-fous : fenêtre du matin, quelques fiches par passage, films d'abord (les séries attendent qu'aucun film ne
//! reste à essayer), jamais une fiche en lecture ni pendant une analyse de la médiathèque, fiche verrouillée laissée
//! telle quelle, langue TMDB inconnue (`xx`, vide) = rien, chaque écriture relue et notée dans l'état (avant/après).
//! Revenir en arrière : réécrire `old` (vide = `null`) par `update_body`, et pour une série remettre à `null` la
//! classification des `children_unrated`.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::NaiveTime;
use serde_json::{json, Map, Value};
use tracing::{info, warn};

use super::anime_library::is_playing;
use super::identity_check::provider_id;
use super::{Report, Task};
use crate::config::{self, Config};
use crate::context::TaskContext;
use crate::state::OriginalLanguageRecord;

pub struct OriginalLanguage;

/// Sorte de fiche visée (les épisodes héritent de leur série).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Series,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Series => "series",
        }
    }

    /// Type de fiche chez Jellyseerr/TMDB.
    fn tmdb_kind(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Series => "tv",
        }
    }
}

/// Fiche Jellyfin sans langue d'origine.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Identifiant TMDB (0 = inconnu : la fiche est comptée, jamais écrite).
    pub tmdb: i64,
    /// Fichier (film) ou dossier (série) : sert à ne pas toucher une fiche en lecture.
    pub path: String,
}

/// Champs que `POST /Items/{id}` écrit **toujours** (`ItemUpdateController.UpdateItem`, Jellyfin 12.1) : absents du
/// corps, ils seraient remis à vide. Tous les autres (personnes, genres, studios, étiquettes, identifiants, champs
/// verrouillés, date d'ajout, nom de série…) ne sont écrits que s'ils sont présents : on les omet, la fiche les garde.
pub const ALWAYS_WRITTEN: [&str; 26] = [
    "Name",
    "ForcedSortName",
    "OriginalTitle",
    "OriginalLanguage",
    "CriticRating",
    "CommunityRating",
    "IndexNumber",
    "ParentIndexNumber",
    "Overview",
    "AirsAfterSeasonNumber",
    "AirsBeforeEpisodeNumber",
    "AirsBeforeSeasonNumber",
    "EndDate",
    "PremiereDate",
    "ProductionYear",
    "OfficialRating",
    "CustomRating",
    "PreferredMetadataCountryCode",
    "PreferredMetadataLanguage",
    "DisplayOrder",
    "AspectRatio",
    "LockData",
    "RunTimeTicks",
    "Video3DFormat",
    "Status",
    "Album",
];

/// Corps de `POST /Items/{id}` qui ne change que `field` : les champs toujours écrits, tels que relus (`dto` =
/// `GET /Items/{id}`), et `value` pour `field`. Sert aussi à revenir en arrière (`OriginalLanguage` → `null`,
/// `OfficialRating` → `null` sur un épisode).
pub fn update_body(dto: &Value, field: &str, value: Value) -> Value {
    let mut body = Map::new();
    for k in ALWAYS_WRITTEN {
        if let Some(v) = dto.get(k) {
            body.insert(k.to_string(), v.clone());
        }
    }
    body.insert(field.to_string(), value);
    Value::Object(body)
}

/// Langue utilisable : code ISO 639-1 à deux lettres. `xx` (« pas de langue » chez TMDB), vide ou autre = rien.
pub fn usable_language(code: &str) -> Option<String> {
    let c = code.trim().to_ascii_lowercase();
    // TMDB note le cantonais `cn`, code hors norme que Jellyfin garderait tel quel sans le rapprocher d'aucune piste
    let c = if c == "cn" { "zh".to_string() } else { c };
    (c.len() == 2 && c.chars().all(|ch| ch.is_ascii_lowercase()) && c != "xx").then_some(c)
}

/// Langue d'origine d'une fiche Jellyfin (vide si absente : Jellyfin omet le champ quand il est NULL).
pub fn language_of(item: &Value) -> String {
    item.get("OriginalLanguage")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Élément de `GET /Items` → candidat, s'il s'agit d'un film ou d'une série sans langue d'origine.
pub fn candidate(item: &Value) -> Option<Candidate> {
    if !language_of(item).is_empty() {
        return None;
    }
    let kind = match item.get("Type").and_then(Value::as_str)? {
        "Movie" => Kind::Movie,
        "Series" => Kind::Series,
        _ => return None,
    };
    let s = |k: &str| {
        item.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Some(Candidate {
        id: s("Id"),
        name: s("Name"),
        kind,
        tmdb: provider_id(item, "Tmdb"),
        path: s("Path"),
    })
}

/// `HH:MM` → heure.
pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// `now` dans la fenêtre `[start, end[` (heure locale). Une fenêtre qui passe minuit (`start > end`) est acceptée ;
/// `start == end` = jamais.
pub fn in_window(now: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    if start <= end {
        start <= now && now < end
    } else {
        now >= start || now < end
    }
}

/// Une fiche est-elle en lecture ? Sans chemin connu, on la considère occupée dès qu'une lecture est en cours.
fn busy(c: &Candidate, playing: &[String]) -> bool {
    if c.path.is_empty() {
        !playing.is_empty()
    } else {
        is_playing(&c.path, playing)
    }
}

/// Ce qu'un passage fera.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Plan {
    /// Fiches de ce passage : des films, ou des séries quand plus aucun film n'est à essayer.
    pub todo: Vec<Candidate>,
    /// Fiches sans langue, avec identifiant TMDB et à essayer (hors délai de nouvel essai).
    pub movies_due: usize,
    pub series_due: usize,
    /// Sans identifiant TMDB : jamais écrites.
    pub no_tmdb: usize,
    /// Essayées depuis moins de `retry_hours` (TMDB sans langue, échec, fiche verrouillée).
    pub deferred: usize,
    /// Sautées ce passage : en lecture.
    pub playing: usize,
}

/// Choix d'un passage : films d'abord (les séries attendent qu'aucun film ne reste à essayer, et seulement si
/// `series`), au plus `max_*_per_run`, jamais une fiche en lecture ni une fiche essayée depuis moins de `retry_hours`.
pub fn plan(
    cands: &[Candidate],
    tried: &BTreeMap<String, OriginalLanguageRecord>,
    playing: &[String],
    now: i64,
    cfg: &config::OriginalLanguage,
) -> Plan {
    let retry = cfg.retry_hours * 3600;
    let mut p = Plan::default();
    let (mut movies, mut series) = (Vec::new(), Vec::new());
    for c in cands {
        if c.tmdb <= 0 {
            p.no_tmdb += 1;
        } else if tried.get(&c.id).is_some_and(|r| now - r.at < retry) {
            p.deferred += 1;
        } else if c.kind == Kind::Movie {
            movies.push(c);
        } else {
            series.push(c);
        }
    }
    p.movies_due = movies.len();
    p.series_due = series.len();
    let (pool, max) = if !movies.is_empty() {
        (movies, cfg.max_movies_per_run)
    } else if cfg.series {
        (series, cfg.max_series_per_run)
    } else {
        (Vec::new(), 0)
    };
    for c in pool {
        if busy(c, playing) {
            p.playing += 1;
        } else if p.todo.len() < max {
            p.todo.push(c.clone());
        }
    }
    p
}

/// Ids (format compact) des saisons et épisodes sans classification : ceux que Jellyfin va réécrire avec celle de
/// la série.
pub fn unrated_children(children: &[Value]) -> Vec<String> {
    children
        .iter()
        .filter(|c| {
            c.get("OfficialRating")
                .and_then(Value::as_str)
                .is_none_or(|r| r.trim().is_empty())
        })
        .filter_map(|c| c.get("Id").and_then(Value::as_str))
        .map(|id| id.replace('-', "").to_ascii_lowercase())
        .collect()
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn admin_id(ctx: &TaskContext) -> Result<String> {
    // `Items` SANS UserId renvoie une liste incomplète (21/09) : compte admin.
    ctx.jellyfin
        .users()
        .await?
        .iter()
        .find(|u| crate::accounts::is_admin(u))
        .and_then(|u| u.get("Id").and_then(Value::as_str).map(str::to_string))
        .context("aucun compte admin Jellyfin")
}

async fn candidates(ctx: &TaskContext, admin: &str) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for kind in ["Movie", "Series"] {
        let items = ctx
            .jellyfin
            .items(&[
                ("UserId", admin),
                ("Recursive", "true"),
                ("IncludeItemTypes", kind),
                ("Fields", "ProviderIds,Path"),
                ("SortBy", "DateCreated"),
                ("SortOrder", "Descending"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ])
            .await?;
        out.extend(items.iter().filter_map(candidate));
    }
    Ok(out)
}

/// Une fiche : langue TMDB, fiche relue, écriture, relecture. `None` = rien à noter (déjà remplie entre-temps).
async fn fill(
    ctx: &TaskContext,
    admin: &str,
    c: &Candidate,
    now: i64,
) -> Option<OriginalLanguageRecord> {
    let mut rec = OriginalLanguageRecord {
        at: now,
        kind: c.kind.as_str().to_string(),
        name: c.name.clone(),
        tmdb: c.tmdb,
        ..Default::default()
    };
    let lang = match ctx
        .jellyseerr
        .original_language(c.kind.tmdb_kind(), c.tmdb)
        .await
    {
        Ok(l) => l.as_deref().and_then(usable_language),
        Err(e) => {
            rec.outcome = "error".into();
            rec.detail = truncate(&format!("TMDB : {e:#}"), 200);
            return Some(rec);
        }
    };
    let Some(lang) = lang else {
        rec.outcome = "unknown".into();
        return Some(rec);
    };
    rec.new = lang.clone();
    // fiche relue juste avant l'écriture : le corps renvoie ses valeurs du moment
    let dto = match ctx.jellyfin.item(&c.id, admin).await {
        Ok(d) => d,
        Err(e) => {
            rec.outcome = "error".into();
            rec.detail = truncate(&format!("lecture : {e:#}"), 200);
            return Some(rec);
        }
    };
    rec.old = language_of(&dto);
    if !rec.old.is_empty() {
        return None;
    }
    if dto.get("LockData").and_then(Value::as_bool) == Some(true) {
        rec.outcome = "locked".into();
        return Some(rec);
    }
    if c.kind == Kind::Series {
        // Jellyfin va réécrire saisons et épisodes : noter ceux dont la classification était vide (retour arrière)
        let children = ctx
            .jellyfin
            .items(&[
                ("UserId", admin),
                ("ParentId", c.id.as_str()),
                ("Recursive", "true"),
                ("IncludeItemTypes", "Season,Episode"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ])
            .await;
        match children {
            Ok(ch) => rec.children_unrated = unrated_children(&ch),
            Err(e) => {
                rec.outcome = "error".into();
                rec.detail = truncate(&format!("épisodes : {e:#}"), 200);
                return Some(rec);
            }
        }
        rec.children_rating = dto
            .get("OfficialRating")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    if ctx.dry_run {
        rec.outcome = "dry-run".into();
        return Some(rec);
    }
    let body = update_body(&dto, "OriginalLanguage", json!(lang));
    if let Err(e) = ctx.jellyfin.update_item(&c.id, &body).await {
        rec.outcome = "error".into();
        rec.detail = truncate(&format!("écriture : {e:#}"), 200);
        return Some(rec);
    }
    // relue : Jellyfin ramène un code connu à deux lettres ; s'il en garde un autre, c'est celui-là qui est noté
    match ctx.jellyfin.item(&c.id, admin).await {
        Ok(back) => match language_of(&back) {
            got if got.is_empty() => {
                rec.outcome = "error".into();
                rec.detail = "relue vide après écriture".into();
            }
            got => {
                if got != lang {
                    rec.detail = format!("Jellyfin a noté « {got} » pour « {lang} »");
                    rec.new = got;
                }
                rec.outcome = "written".into();
            }
        },
        Err(e) => {
            // écrite mais non relue : notée comme écrite (le passage suivant ne la reverra plus si elle l'est)
            rec.outcome = "written".into();
            rec.detail = truncate(&format!("relecture impossible : {e:#}"), 200);
        }
    }
    Some(rec)
}

#[async_trait]
impl Task for OriginalLanguage {
    fn name(&self) -> &'static str {
        "original_language"
    }

    fn label(&self) -> &'static str {
        "Langue d'origine des fiches"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.original_language.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.original_language;
        if !cfg.enabled {
            return Ok(Report::new("désactivée (enabled = false)", 0));
        }
        let (Some(start), Some(end)) = (parse_hhmm(&cfg.window_start), parse_hhmm(&cfg.window_end))
        else {
            bail!(
                "fenêtre invalide : « {} »–« {} » (HH:MM attendu)",
                cfg.window_start,
                cfg.window_end
            );
        };
        let window = format!("{}–{}", cfg.window_start, cfg.window_end);
        let open = in_window(chrono::Local::now().time(), start, end);
        // hors fenêtre, rien n'est lu ; un dry-run montre quand même le prochain passage
        if !open && !ctx.dry_run {
            return Ok(Report::new(format!("hors fenêtre ({window})"), 0));
        }
        if ctx.jellyfin.library_scan_running().await.unwrap_or(false) {
            return Ok(Report::new(
                "analyse de la médiathèque en cours : passage sauté",
                0,
            ));
        }
        let admin = admin_id(ctx).await?;
        let cands = candidates(ctx, &admin).await?;
        // sans la liste des lectures, on n'écrit pas à l'aveugle
        let playing = ctx.jellyfin.playing_paths().await?;
        let tried = ctx.state.read(|s| s.original_language.clone()).await;
        let now = crate::state::now();
        let p = plan(&cands, &tried, &playing, now, cfg);
        let series_note = if cfg.series { "" } else { " (désactivées)" };
        let rest = format!(
            "à remplir : {} film(s), {} série(s){series_note} ; sans TMDB : {} ; en attente de nouvel essai : {}{}",
            p.movies_due,
            p.series_due,
            p.no_tmdb,
            p.deferred,
            if p.playing > 0 {
                format!(" ; en lecture : {}", p.playing)
            } else {
                String::new()
            }
        );
        if p.todo.is_empty() {
            return Ok(Report::new(format!("rien à écrire — {rest}"), 0));
        }
        ctx.jellyseerr
            .status()
            .await
            .context("Jellyseerr injoignable : langues TMDB illisibles")?;
        let (mut written, mut other) = (Vec::new(), Vec::new());
        for c in &p.todo {
            let Some(rec) = fill(ctx, &admin, c, now).await else {
                continue;
            };
            let shown = format!(
                "{} ({})",
                c.name,
                if rec.new.is_empty() {
                    rec.outcome.as_str()
                } else {
                    rec.new.as_str()
                }
            );
            match rec.outcome.as_str() {
                "written" => {
                    info!(task = "original_language", title = %c.name, kind = c.kind.as_str(), lang = %rec.new, children = rec.children_unrated.len(), "langue d'origine écrite");
                    written.push(shown);
                }
                "dry-run" => {
                    let extra = if c.kind == Kind::Series {
                        format!(
                            " [{} saison(s)/épisode(s) sans classification recevraient « {} »]",
                            rec.children_unrated.len(),
                            rec.children_rating
                        )
                    } else {
                        String::new()
                    };
                    written.push(format!("{shown}{extra}"));
                }
                _ => {
                    if rec.outcome == "error" {
                        warn!(task = "original_language", title = %c.name, detail = %rec.detail, "langue d'origine non écrite");
                    }
                    other.push(format!("{} : {}", c.name, rec.outcome));
                }
            }
            if !ctx.dry_run {
                let key = c.id.clone();
                ctx.state
                    .update(|s| {
                        s.original_language.insert(key, rec);
                    })
                    .await?;
            }
        }
        let verb = if ctx.dry_run {
            if open {
                "écrirait"
            } else {
                "hors fenêtre ; au prochain passage dans la fenêtre, écrirait"
            }
        } else {
            "écrit"
        };
        let mut summary = if written.is_empty() {
            format!("rien d'écrit — {rest}")
        } else {
            format!("{verb} : {} — {rest}", written.join(", "))
        };
        if !other.is_empty() {
            summary.push_str(&format!(" ; non écrites : {}", other.join(", ")));
        }
        let actions = if ctx.dry_run { 0 } else { written.len() as u32 };
        Ok(Report::new(summary, actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> config::OriginalLanguage {
        config::OriginalLanguage {
            max_movies_per_run: 2,
            max_series_per_run: 1,
            series: true,
            ..Default::default()
        }
    }

    fn cand(id: &str, kind: Kind, tmdb: i64, path: &str) -> Candidate {
        Candidate {
            id: id.into(),
            name: id.into(),
            kind,
            tmdb,
            path: path.into(),
        }
    }

    fn tried(id: &str, at: i64, outcome: &str) -> (String, OriginalLanguageRecord) {
        (
            id.into(),
            OriginalLanguageRecord {
                at,
                outcome: outcome.into(),
                ..Default::default()
            },
        )
    }

    #[test]
    fn only_movies_and_series_without_language_are_candidates() {
        let m = json!({"Id": "a1", "Name": "Marnie", "Type": "Movie", "Path": "/media/movies/Marnie/Marnie.mkv",
                        "ProviderIds": {"Tmdb": "242828"}});
        let c = candidate(&m).unwrap();
        assert_eq!(
            (c.kind, c.tmdb, c.path.as_str()),
            (Kind::Movie, 242828, "/media/movies/Marnie/Marnie.mkv")
        );
        // déjà remplie : rien
        let mut filled = m.clone();
        filled["OriginalLanguage"] = json!("ja");
        assert!(candidate(&filled).is_none());
        // vide ou blanc : candidate
        filled["OriginalLanguage"] = json!(" ");
        assert!(candidate(&filled).is_some());
        // série
        let s = json!({"Id": "s1", "Name": "Frieren", "Type": "Series", "ProviderIds": {"Tmdb": 209867}});
        assert_eq!(candidate(&s).unwrap().kind, Kind::Series);
        // épisodes, saisons, collections : jamais (les épisodes héritent de leur série)
        for t in ["Episode", "Season", "BoxSet"] {
            assert!(
                candidate(&json!({"Id": "x", "Type": t, "ProviderIds": {"Tmdb": "1"}})).is_none(),
                "{t}"
            );
        }
        // sans TMDB : candidate, mais comptée à part par le plan
        assert_eq!(
            candidate(&json!({"Id": "x", "Type": "Movie"}))
                .unwrap()
                .tmdb,
            0
        );
    }

    #[test]
    fn unknown_language_means_nothing() {
        assert_eq!(usable_language("ja").as_deref(), Some("ja"));
        assert_eq!(usable_language(" EN ").as_deref(), Some("en"));
        assert_eq!(
            usable_language("cn").as_deref(),
            Some("zh"),
            "cantonais TMDB"
        );
        for bad in ["", "  ", "xx", "jpn", "e", "fr-FR", "12"] {
            assert_eq!(usable_language(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_window_is_local_time_start_included_end_excluded() {
        let t = |s: &str| parse_hhmm(s).unwrap();
        let (a, b) = (t("07:30"), t("11:30"));
        assert!(!in_window(t("07:29"), a, b));
        assert!(in_window(t("07:30"), a, b));
        assert!(in_window(t("09:00"), a, b));
        assert!(!in_window(t("11:30"), a, b));
        assert!(!in_window(t("13:00"), a, b));
        assert!(!in_window(t("05:00"), a, b), "analyse de 05:00");
        // fenêtre de nuit (passe minuit)
        assert!(in_window(t("23:30"), t("23:00"), t("01:00")));
        assert!(in_window(t("00:30"), t("23:00"), t("01:00")));
        assert!(!in_window(t("12:00"), t("23:00"), t("01:00")));
        // début = fin : jamais
        assert!(!in_window(t("07:30"), a, a));
        // saisie invalide
        assert!(parse_hhmm("7h30").is_none());
        assert!(parse_hhmm("25:00").is_none());
        assert_eq!(parse_hhmm(" 07:30 "), Some(a));
    }

    #[test]
    fn movies_first_then_series_a_few_per_run() {
        let c = vec![
            cand("s1", Kind::Series, 10, "/seedbox/media/Anime/Frieren"),
            cand("m1", Kind::Movie, 1, "/media/movies/A/A.mkv"),
            cand("m2", Kind::Movie, 2, "/media/movies/B/B.mkv"),
            cand("m3", Kind::Movie, 3, "/media/movies/C/C.mkv"),
        ];
        let empty = BTreeMap::new();
        let p = plan(&c, &empty, &[], 1_000_000, &cfg());
        let ids: Vec<&str> = p.todo.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m2"], "films d'abord, au plus 2");
        assert_eq!((p.movies_due, p.series_due), (3, 1));
        // plus aucun film à essayer : une série
        let only_series: Vec<Candidate> = c
            .iter()
            .filter(|x| x.kind == Kind::Series)
            .cloned()
            .collect();
        let p = plan(&only_series, &empty, &[], 1_000_000, &cfg());
        assert_eq!(p.todo.len(), 1);
        assert_eq!(p.todo[0].id, "s1");
        // séries désactivées : rien, mais toujours comptées
        let off = config::OriginalLanguage {
            series: false,
            ..cfg()
        };
        let p = plan(&only_series, &empty, &[], 1_000_000, &off);
        assert!(p.todo.is_empty());
        assert_eq!(p.series_due, 1);
        // un film sans TMDB ne retient pas les séries
        let mut c2 = only_series.clone();
        c2.push(cand("m0", Kind::Movie, 0, "/media/movies/X/X.mkv"));
        let p = plan(&c2, &empty, &[], 1_000_000, &cfg());
        assert_eq!((p.todo.len(), p.no_tmdb), (1, 1));
        assert_eq!(p.todo[0].id, "s1");
    }

    #[test]
    fn a_title_being_played_or_tried_recently_is_left_alone() {
        let c = vec![
            cand("m1", Kind::Movie, 1, "/media/movies/A/A.mkv"),
            cand("m2", Kind::Movie, 2, "/media/movies/B/B.mkv"),
            cand("m3", Kind::Movie, 3, "/media/movies/C/C.mkv"),
            cand("s1", Kind::Series, 10, "/seedbox/media/Anime/Frieren"),
        ];
        let now = 1_000_000;
        let t: BTreeMap<_, _> = [
            tried("m2", now - 3600, "unknown"),
            tried("m3", now - 25 * 3600, "error"),
        ]
        .into_iter()
        .collect();
        let playing = vec!["/media/movies/A/A.mkv".to_string()];
        let p = plan(&c, &t, &playing, now, &cfg());
        // m1 en lecture, m2 essayée il y a 1 h (délai 24 h), m3 essayée il y a 25 h : à nouveau due
        let ids: Vec<&str> = p.todo.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["m3"]);
        assert_eq!((p.playing, p.deferred, p.movies_due), (1, 1, 2));
        // un épisode en lecture bloque sa série, pas les autres
        let series = vec![
            cand("s1", Kind::Series, 10, "/seedbox/media/Anime/Frieren"),
            cand("s2", Kind::Series, 11, "/seedbox/media/Anime/Frieren 2"),
        ];
        let playing = vec!["/seedbox/media/Anime/Frieren/Season 01/E01.mkv".to_string()];
        let p = plan(&series, &BTreeMap::new(), &playing, now, &cfg());
        assert_eq!(p.todo.len(), 1);
        assert_eq!(
            p.todo[0].id, "s2",
            "« Frieren 2 » n'est pas dans le dossier « Frieren »"
        );
        // sans chemin connu : occupée dès qu'une lecture est en cours
        let nopath = vec![cand("m9", Kind::Movie, 9, "")];
        assert!(plan(&nopath, &BTreeMap::new(), &playing, now, &cfg())
            .todo
            .is_empty());
        assert_eq!(
            plan(&nopath, &BTreeMap::new(), &[], now, &cfg()).todo.len(),
            1
        );
    }

    #[test]
    fn the_update_body_only_carries_the_always_written_fields() {
        let dto = json!({
            "Id": "a1", "Name": "Batman Begins", "OriginalTitle": "Batman Begins", "Overview": "…",
            "CommunityRating": 7.73, "CriticRating": 85, "OfficialRating": "FR-12", "ProductionYear": 2005,
            "PremiereDate": "2005-06-10T00:00:00.0000000Z", "LockData": false, "RunTimeTicks": 84_000_000_000_i64,
            "People": [{"Name": "Christian Bale", "Type": "Actor"}], "Genres": ["Action"], "Tags": ["ninja"],
            "Studios": [{"Name": "DC"}], "ProviderIds": {"Tmdb": "272"}, "LockedFields": [],
            "Trickplay": {"x": {"320": {"Width": 320}}}, "MediaSources": [], "UserData": {"Played": false},
            "DateCreated": "2026-09-14T10:00:00.0000000Z", "Taglines": ["…"]
        });
        let b = update_body(&dto, "OriginalLanguage", json!("en"));
        assert_eq!(b["OriginalLanguage"], json!("en"));
        // toujours écrits : renvoyés tels que lus
        for k in [
            "Name",
            "OriginalTitle",
            "Overview",
            "CommunityRating",
            "CriticRating",
            "OfficialRating",
            "ProductionYear",
            "PremiereDate",
            "LockData",
            "RunTimeTicks",
        ] {
            assert_eq!(b[k], dto[k], "{k}");
        }
        // écrits seulement s'ils sont présents : omis (la fiche les garde), et ce qui fait répondre 500 (Trickplay)
        for k in [
            "People",
            "Genres",
            "Tags",
            "Studios",
            "ProviderIds",
            "LockedFields",
            "Trickplay",
            "MediaSources",
            "UserData",
            "DateCreated",
            "Taglines",
            "Id",
        ] {
            assert!(b.get(k).is_none(), "{k}");
        }
        // retour arrière : la langue remise à vide, le reste inchangé
        let mut filled = dto.clone();
        filled["OriginalLanguage"] = json!("en");
        let back = update_body(&filled, "OriginalLanguage", Value::Null);
        assert_eq!(back["OriginalLanguage"], Value::Null);
        assert_eq!(back["Name"], dto["Name"]);
        // changer un autre champ garde la langue (elle aussi toujours écrite)
        let other = update_body(&filled, "OfficialRating", Value::Null);
        assert_eq!(other["OriginalLanguage"], json!("en"));
    }

    #[test]
    fn unrated_children_are_noted_for_a_rollback() {
        let ch = vec![
            json!({"Id": "AAAA-bbbb", "Type": "Season"}),
            json!({"Id": "cccc", "Type": "Episode", "OfficialRating": "TV-14"}),
            json!({"Id": "dddd", "Type": "Episode", "OfficialRating": ""}),
        ];
        assert_eq!(unrated_children(&ch), ["aaaabbbb", "dddd"]);
    }
}
