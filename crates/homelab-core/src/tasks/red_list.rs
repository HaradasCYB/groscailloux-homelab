//! Liste rouge (2026-10-20, décidé par l'admin) : les films et séries que personne n'a lancés depuis `min_idle_days`
//! jours (60), affichés sur `/accounts/liste-rouge` avec « Exempter » et « Nettoyer ».
//!
//! Cette tâche **calcule** (une fois par jour, et à la demande : « Recalculer », début d'un « Nettoyer ») : elle n'écrit
//! que `state.red_list.report` et les horloges « vu sans date ». Seul le moteur « Nettoyer » (`red_list_clean`)
//! supprime, après un nouveau calcul complet ([`compute`]). À chaque passage, elle marque aussi « interrompu » un
//! « Nettoyer » coupé par un redémarrage et rejoue quelques étapes restées en attente (corbeille d'un Arr, média Seerr).
//!
//! Un titre est rouge si `now − max(arrivée, dernière lecture, horloge « vu sans date ») ≥ min_idle_days` :
//! - **arrivée** : film, la plus tardive de la fiche et du fichier (comme `catalogue_report`) ; série, la plus tardive de
//!   la fiche, de la première diffusion, de la dernière diffusion passée (`previousAiring`, `lastAired` passé) et de
//!   l'import du dernier fichier d'épisode (lu seulement pour les séries déjà candidates) : 60 j sans lecture **et** sans
//!   nouvel épisode. Arrivée illisible : jamais rouge ;
//! - **dernière lecture**, tous comptes confondus : `UserData.LastPlayedDate` de chaque compte et dernière ligne d'au
//!   moins 60 s de Playback Reporting ; pour une série, le maximum sur ses épisodes ;
//! - **vu sans date** (marqué vu à la main, aucune date nulle part) : l'horloge démarre à la première observation
//!   (`state.red_list.undated_since`).
//!
//! Clé stable : `movie:tmdb:<id>`, `series:tvdb:<id>` (à défaut `series:tmdb:<id>`) ; sans identifiant, jamais rouge.
//! Un titre présent des deux côtés est **une** entrée à plusieurs copies, rouge seulement si toutes le sont. Un titre
//! qu'une copie au moins ne rapproche pas de Jellyfin n'est pas évalué (« je ne sais pas » n'est pas « jamais lu »).
//! Playback Reporting muet ou compte illisible : le calcul est `complete = false` et « Nettoyer » est refusé.
//!
//! Limite connue : les fiches de la seedbox datent du 12/09 (création des Arrs) ; leurs titres ne deviennent rouges
//! qu'à partir du 11/11, le plafond par clic étale la vague.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use super::catalogue_report::{
    human_size, parse_date, pick_admin, read_jf_index, read_requests, read_seen, read_titles,
    JfIndex, Requests, Seen, SeenRead, Title,
};
use super::{Report, Task};
use crate::config::{Config, RedList as Cfg};
use crate::context::TaskContext;
use crate::state::{now, Exemption, RedEntry, RedListReport, RedSide};

pub struct RedListTask;

const TASK: &str = "red_list";
const DAY: i64 = 86_400;

/// Motifs des titres non évalués.
const NOT_IN_JELLYFIN: &str = "absent de Jellyfin";
const NO_ID: &str = "sans identifiant TMDB/TVDB";
const NO_ARRIVAL: &str = "date d'arrivée illisible";
const FILES_UNREADABLE: &str = "fichiers d'épisodes illisibles";

// ---------------------------------------------------------------------------------------------
// Clés
// ---------------------------------------------------------------------------------------------

/// Clé stable d'un titre : `movie:tmdb:<id>`, `series:tvdb:<id>`, à défaut `series:tmdb:<id>` ; `None` sans identifiant.
pub fn key_of(t: &Title) -> Option<String> {
    if t.movie {
        (t.tmdb > 0).then(|| format!("movie:tmdb:{}", t.tmdb))
    } else if t.tvdb > 0 {
        Some(format!("series:tvdb:{}", t.tvdb))
    } else {
        (t.tmdb > 0).then(|| format!("series:tmdb:{}", t.tmdb))
    }
}

/// Clé reçue d'un formulaire, relue strictement : (film ?, fournisseur `tmdb`/`tvdb`, identifiant > 0). `None` pour
/// tout le reste (une clé ne sert qu'à retrouver un titre de l'état, jamais à construire une requête).
pub fn parse_key(key: &str) -> Option<(bool, &'static str, i64)> {
    let mut parts = key.split(':');
    let (kind, provider, id) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = id.parse::<i64>().ok().filter(|i| *i > 0)?;
    match (kind, provider) {
        ("movie", "tmdb") => Some((true, "tmdb", id)),
        ("series", "tvdb") => Some((false, "tvdb", id)),
        ("series", "tmdb") => Some((false, "tmdb", id)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Le calcul, pur
// ---------------------------------------------------------------------------------------------

/// Import du dernier fichier d'épisode d'une fiche Sonarr (`episodefile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastImport {
    /// Liste lue : date du fichier le plus récent (`None` : aucun fichier daté).
    Read(Option<i64>),
    /// Liste illisible : la série n'est pas évaluée (un épisode arrivé hier ne doit pas passer inaperçu).
    Unreadable,
}

/// Par fiche Sonarr, clé (côté, identifiant de la fiche). Une fiche absente n'a pas été lue (série non candidate) : son
/// arrivée s'en passe, ce qui ne peut que la vieillir — d'où les deux calculs de [`compute`].
pub type LastImports = HashMap<(String, i64), LastImport>;

pub struct Inputs<'a> {
    pub titles: &'a [Title],
    pub jf: &'a JfIndex,
    pub seen: &'a Seen,
    pub requests: &'a Requests,
    pub last_imports: &'a LastImports,
    /// Playback Reporting a répondu à ce calcul.
    pub playback_reporting: bool,
    /// Comptes Jellyfin dont une liste n'a pas pu être lue.
    pub unreadable_accounts: u32,
}

/// Date du fichier d'épisode le plus récent (`dateAdded`).
pub fn last_import(files: &[Value]) -> Option<i64> {
    files
        .iter()
        .filter_map(|f| {
            f.get("dateAdded")
                .and_then(Value::as_str)
                .and_then(parse_date)
        })
        .max()
}

/// Arrivée d'une copie : celle de l'Arr (`catalogue_report`), repoussée pour une série par sa dernière diffusion
/// passée et l'import de son dernier fichier. `Err(motif)` si la copie ne peut pas être évaluée.
fn arrival(t: &Title, imports: &LastImports, now: i64) -> Result<i64, &'static str> {
    let base = t.arrived.ok_or(NO_ARRIVAL)?;
    if t.movie {
        return Ok(base);
    }
    let imported = match imports.get(&(t.side.clone(), t.arr_id)) {
        Some(LastImport::Unreadable) => return Err(FILES_UNREADABLE),
        Some(LastImport::Read(at)) => *at,
        None => None,
    };
    Ok([t.previous_airing, t.last_aired]
        .into_iter()
        .flatten()
        .filter(|at| *at <= now)
        .chain(imported)
        .fold(base, i64::max))
}

/// « 12/09/2026 » (heure locale).
fn day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m/%Y")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

/// Motif en clair : ce qui fait la dernière activité du titre.
fn reason(movie: bool, arrived: i64, last_played: Option<i64>, undated: Option<i64>) -> String {
    let fresh = if movie {
        format!("arrivé le {}", day(arrived))
    } else {
        format!("rien de neuf depuis le {}", day(arrived))
    };
    match (last_played, undated) {
        (_, Some(u)) if u >= arrived && last_played.is_none_or(|l| u >= l) => {
            format!("vu sans date (horloge depuis le {})", day(u))
        }
        (Some(l), _) if l >= arrived => format!("dernière lecture le {}", day(l)),
        (Some(l), _) => format!("dernière lecture le {}, {fresh}", day(l)),
        (None, _) if movie => format!("jamais lu, {fresh}"),
        (None, _) => format!("jamais lue, {fresh}"),
    }
}

/// Une entrée à partir des copies d'un titre (même clé) ; dates et motif remplis par l'appelant.
fn entry(key: &str, copies: &[&Title], jf: &JfIndex, requests: &Requests) -> RedEntry {
    let first = copies[0];
    let tmdb = copies.iter().map(|t| t.tmdb).find(|i| *i > 0).unwrap_or(0);
    RedEntry {
        key: key.to_string(),
        title: first.name.clone(),
        kind: first.kind.to_string(),
        movie: first.movie,
        tmdb,
        tvdb: copies.iter().map(|t| t.tvdb).find(|i| *i > 0).unwrap_or(0),
        russian: copies.iter().any(|t| t.russian),
        sides: copies
            .iter()
            .map(|t| RedSide {
                side: t.side.clone(),
                arr_id: t.arr_id,
                bytes: t.bytes,
                folder: t.path.clone(),
                jf_id: jf.find(t).unwrap_or_default().to_string(),
                arrived: t.arrived,
            })
            .collect(),
        bytes: copies.iter().map(|t| t.bytes).sum(),
        requester: requests.kind(first.movie, tmdb).to_string(),
        ..Default::default()
    }
}

/// Range les entrées évaluées d'un rapport selon les exemptions et la date du calcul : exempté, rouge (`due <= at`),
/// bientôt rouge (`due <= at + soon_days`) ; le reste n'est pas listé. Sert au calcul, et à la page après « Exempter »
/// ou « Retirer l'exemption » (le rapport suit sans recalcul). Les non évalués ne bougent pas.
pub fn regroup(r: &mut RedListReport, exemptions: &BTreeMap<String, Exemption>) {
    let all: Vec<RedEntry> = r
        .entries
        .drain(..)
        .chain(r.soon.drain(..))
        .chain(r.exempted.drain(..))
        .collect();
    let soon_until = r.at + r.soon_days.max(0) * DAY;
    for e in all {
        let Some(due) = e.due else {
            continue;
        };
        if exemptions.contains_key(&e.key) {
            r.exempted.push(e);
        } else if due <= r.at {
            r.entries.push(e);
        } else if due <= soon_until {
            r.soon.push(e);
        }
    }
    // les plus gros d'abord ; le plus proche d'abord pour « bientôt » ; à égalité, l'ordre alphabétique puis la clé
    r.entries.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.key.cmp(&b.key))
    });
    r.soon.sort_by(|a, b| {
        a.due
            .cmp(&b.due)
            .then_with(|| b.bytes.cmp(&a.bytes))
            .then_with(|| a.key.cmp(&b.key))
    });
    r.exempted
        .sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.key.cmp(&b.key)));
}

/// Le calcul, pur : même entrée, même liste. Rend le rapport et les horloges « vu sans date » à garder : sur un calcul
/// complet, exactement les titres vus sans date (une horloge déjà en marche garde sa date de départ) plus celles des
/// titres présents mais non évalués cette fois ; sur un calcul incomplet, rien n'est retiré (un compte illisible ne
/// doit pas remettre une horloge à zéro, ni l'arrêter).
pub fn build_red_list(
    inp: &Inputs<'_>,
    cfg: &Cfg,
    exemptions: &BTreeMap<String, Exemption>,
    undated_since: &BTreeMap<String, i64>,
    now: i64,
) -> (RedListReport, BTreeMap<String, i64>) {
    let mut missing = Vec::new();
    if !inp.playback_reporting {
        missing.push("Playback Reporting muet".to_string());
    }
    if inp.unreadable_accounts > 0 {
        missing.push(format!(
            "{} compte(s) Jellyfin illisible(s)",
            inp.unreadable_accounts
        ));
    }
    let mut r = RedListReport {
        at: now,
        min_idle_days: cfg.min_idle_days,
        soon_days: cfg.soon_days,
        complete: missing.is_empty(),
        missing,
        ..Default::default()
    };
    let idle = cfg.min_idle_days.max(0) * DAY;
    // par clé (ordre stable) ; sans clé, une entrée par fiche
    let mut groups: BTreeMap<String, Vec<&Title>> = BTreeMap::new();
    for t in inp.titles {
        if t.russian && !cfg.include_russian {
            continue;
        }
        match key_of(t) {
            Some(k) => groups.entry(k).or_default().push(t),
            None => {
                let mut e = entry("", &[t], inp.jf, inp.requests);
                e.reason = NO_ID.to_string();
                r.unmatched.push(e);
            }
        }
    }
    let mut undated_now: BTreeMap<String, i64> = BTreeMap::new();
    let mut evaluated: BTreeSet<&str> = BTreeSet::new();
    for (key, copies) in &groups {
        let mut e = entry(key, copies, inp.jf, inp.requests);
        let mut arrived: Vec<i64> = Vec::new();
        let mut blocked: Option<&'static str> = None;
        let (mut last_played, mut undated) = (None::<i64>, false);
        for t in copies {
            let Some(jid) = inp.jf.find(t) else {
                blocked = blocked.or(Some(NOT_IN_JELLYFIN));
                continue;
            };
            match arrival(t, inp.last_imports, now) {
                Ok(a) => arrived.push(a),
                Err(why) => blocked = blocked.or(Some(why)),
            }
            last_played = last_played.max(inp.seen.last_played(jid));
            undated |= inp.seen.seen_undated(jid);
        }
        e.last_played = last_played;
        if let Some(why) = blocked {
            e.arrived = arrived.iter().copied().max();
            e.reason = if copies.len() > 1 && why == NOT_IN_JELLYFIN {
                "une copie absente de Jellyfin".to_string()
            } else {
                why.to_string()
            };
            r.unmatched.push(e);
            continue;
        }
        evaluated.insert(key);
        let arrived = arrived.into_iter().max().unwrap_or(now);
        let since = undated.then(|| undated_since.get(key).copied().unwrap_or(now));
        if let Some(s) = since {
            undated_now.insert(key.clone(), s);
        }
        let activity = [Some(arrived), last_played, since]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(arrived);
        e.arrived = Some(arrived);
        e.undated_since = since;
        e.due = Some(activity + idle);
        e.reason = reason(e.movie, arrived, last_played, since);
        r.entries.push(e);
    }
    r.evaluated = u32::try_from(evaluated.len()).unwrap_or(u32::MAX);
    regroup(&mut r, exemptions);
    r.unmatched
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.title.cmp(&b.title)));
    // horloges : un titre présent non évalué cette fois garde la sienne ; un calcul incomplet n'en retire aucune
    let mut clocks = undated_now;
    for (k, since) in undated_since {
        let keep = if r.complete {
            groups.contains_key(k) && !evaluated.contains(k.as_str())
        } else {
            true
        };
        if keep {
            clocks.entry(k.clone()).or_insert(*since);
        }
    }
    (r, clocks)
}

/// Fiches Sonarr à relire (`episodefile`) : les séries rouges, bientôt rouges ou exemptées sur le point de l'être,
/// sans la date du dernier import. Un import récent ne peut que les rajeunir.
pub fn series_candidates(r: &RedListReport) -> Vec<(String, i64)> {
    let soon_until = r.at + r.soon_days.max(0) * DAY;
    let mut out: Vec<(String, i64)> = r
        .entries
        .iter()
        .chain(&r.soon)
        .chain(&r.exempted)
        .filter(|e| !e.movie && e.due.is_some_and(|d| d <= soon_until))
        .flat_map(|e| e.sides.iter().map(|s| (s.side.clone(), s.arr_id)))
        .filter(|(_, id)| *id > 0)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Résumé d'un calcul (une ligne, affichée dans le tableau des tâches).
pub fn summary(r: &RedListReport) -> String {
    let bytes: u64 = r.entries.iter().map(|e| e.bytes).sum();
    let mut s = format!(
        "{} rouge(s) ({}) · {} bientôt · {} exempté(s) · {} non évalué(s)",
        r.entries.len(),
        human_size(bytes),
        r.soon.len(),
        r.exempted.len(),
        r.unmatched.len(),
    );
    if !r.complete {
        s.push_str(&format!(" · INCOMPLET : {}", r.missing.join(", ")));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Lectures et enregistrement
// ---------------------------------------------------------------------------------------------

/// Un calcul : le rapport et les horloges « vu sans date » à enregistrer avec lui.
#[derive(Debug, Clone)]
pub struct Computed {
    pub report: RedListReport,
    pub undated_since: BTreeMap<String, i64>,
}

/// Calcul complet, **en lecture seule** (rien n'est écrit, ni chez les services ni dans l'état) : fiches des deux côtés,
/// catalogue Jellyfin, lectures de chaque compte et de Playback Reporting, demandes Jellyseerr, puis `episodefile` des
/// seules séries candidates. `Ok(None)` si un côté Arr est injoignable (une liste sans la seedbox serait fausse).
/// Le moteur « Nettoyer » ne supprime que si `report.complete`.
pub async fn compute(ctx: &TaskContext) -> Result<Option<Computed>> {
    let cfg = &ctx.cfg.tasks.red_list;
    let users = ctx.jellyfin.users().await?;
    let admin = pick_admin(&users).context("aucun compte admin actif")?;
    let Some(titles) = read_titles(ctx, TASK).await else {
        return Ok(None);
    };
    let jf = read_jf_index(ctx, &admin).await?;
    let SeenRead {
        seen,
        playback_reporting,
        unreadable_accounts,
    } = read_seen(
        ctx,
        &users,
        &admin,
        ctx.cfg.tasks.catalogue_report.user_pause_ms,
        TASK,
        false,
    )
    .await?;
    let requests = read_requests(ctx, TASK).await;
    let (exemptions, undated_since) = ctx
        .state
        .read(|s| {
            (
                s.red_list.exemptions.clone(),
                s.red_list.undated_since.clone(),
            )
        })
        .await;
    let now = now();
    let mut imports = LastImports::new();
    // premier calcul sans les imports d'épisodes : il désigne les séries à relire
    let (first, _) = build_red_list(
        &Inputs {
            titles: &titles,
            jf: &jf,
            seen: &seen,
            requests: &requests,
            last_imports: &imports,
            playback_reporting,
            unreadable_accounts,
        },
        cfg,
        &exemptions,
        &undated_since,
        now,
    );
    for (side, id) in series_candidates(&first) {
        let sonarr = if side == "vps" {
            Some(&ctx.sonarr)
        } else {
            ctx.seedbox_sonarr.as_ref()
        };
        let read = match sonarr {
            Some(s) => s.episode_files(id).await,
            None => Err(anyhow::anyhow!("Sonarr {side} non configuré")),
        };
        let found = match read {
            Ok(files) => LastImport::Read(last_import(&files)),
            Err(e) => {
                warn!(
                    task = TASK,
                    side = side.as_str(),
                    series = id,
                    error = format!("{e:#}"),
                    "fichiers d'épisodes illisibles : série non évaluée"
                );
                LastImport::Unreadable
            }
        };
        imports.insert((side, id), found);
    }
    let (report, undated_since) = build_red_list(
        &Inputs {
            titles: &titles,
            jf: &jf,
            seen: &seen,
            requests: &requests,
            last_imports: &imports,
            playback_reporting,
            unreadable_accounts,
        },
        cfg,
        &exemptions,
        &undated_since,
        now,
    );
    Ok(Some(Computed {
        report,
        undated_since,
    }))
}

/// Enregistre un calcul (écriture durable). Les exemptions posées ou retirées pendant le calcul sont réappliquées.
/// Rend le rapport tel qu'enregistré.
pub async fn store(ctx: &TaskContext, c: Computed) -> Result<RedListReport> {
    ctx.state
        .update(move |s| {
            let mut report = c.report;
            regroup(&mut report, &s.red_list.exemptions);
            s.red_list.undated_since = c.undated_since;
            s.red_list.report = Some(report.clone());
            report
        })
        .await
}

/// Calcule et enregistre (« Recalculer », passage quotidien) ; en dry-run, calcule seulement. `Ok(None)` si un côté
/// Arr est injoignable : la liste précédente reste.
pub async fn refresh(ctx: &TaskContext) -> Result<Option<RedListReport>> {
    let Some(c) = compute(ctx).await? else {
        return Ok(None);
    };
    if ctx.dry_run {
        return Ok(Some(c.report));
    }
    store(ctx, c).await.map(Some)
}

#[async_trait]
impl Task for RedListTask {
    fn name(&self) -> &'static str {
        TASK
    }

    fn label(&self) -> &'static str {
        "Liste rouge"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.red_list.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        // « Nettoyer » coupé par un redémarrage, étapes en attente (corbeille, Seerr) : avant le calcul
        let interrupted = crate::red_list_clean::mark_interrupted(ctx).await?;
        let replayed = crate::red_list_clean::replay_pending(ctx).await?;
        let mut extra = String::new();
        if interrupted > 0 {
            extra.push_str(&format!(" · {interrupted} « Nettoyer » interrompu(s)"));
        }
        if !replayed.is_empty() {
            extra.push_str(&format!(" · {replayed}"));
        }
        let Some(report) = refresh(ctx).await? else {
            return Ok(Report::new(
                format!("un côté Arr est injoignable : liste précédente conservée{extra}"),
                0,
            ));
        };
        if ctx.dry_run {
            info!(
                task = TASK,
                report = %serde_json::to_string(&report)?,
                "dry-run : liste calculée, non enregistrée"
            );
        }
        Ok(Report::new(format!("{}{extra}", summary(&report)), 0))
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_790_000_000;

    fn cfg() -> Cfg {
        Cfg::default()
    }

    fn iso(ts: i64) -> String {
        chrono::DateTime::from_timestamp(ts, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// Une fiche d'Arr arrivée il y a `age_days` jours, présente dans Jellyfin sous `/media/<côté>/<nom>`.
    fn title(name: &str, movie: bool, side: &str, id: i64, bytes: u64, age_days: i64) -> Title {
        Title {
            name: name.into(),
            kind: if movie { "film" } else { "série" },
            side: side.into(),
            movie,
            bytes,
            arrived: Some(NOW - age_days * DAY),
            tmdb: if movie { id } else { id * 10 },
            tvdb: if movie { 0 } else { id },
            episodes: if movie { 0 } else { 10 },
            russian: false,
            jf_path: Some(format!("/media/{side}/{name}")),
            arr_id: id + if side == "vps" { 1000 } else { 2000 },
            path: format!("/arr/{side}/{name}"),
            previous_airing: None,
            last_aired: None,
        }
    }

    /// Identifiant Jellyfin d'une copie dans les tests.
    fn jid(t: &Title) -> String {
        format!("{}{}", t.side, t.name).to_ascii_lowercase()
    }

    /// Dossier qu'aucun élément Jellyfin ne porte (titre non rapproché).
    const LOST: &str = "/media/ailleurs";

    fn jf_for(titles: &[Title]) -> JfIndex {
        let items: Vec<Value> = titles
            .iter()
            .filter(|t| t.jf_path.as_deref() != Some(LOST))
            .map(|t| {
                let p = t.jf_path.clone().unwrap();
                if t.movie {
                    json!({"Id": jid(t), "Type": "Movie", "Path": format!("{p}/f.mkv")})
                } else {
                    json!({"Id": jid(t), "Type": "Series", "Path": p})
                }
            })
            .collect();
        JfIndex::from_items(&items)
    }

    /// Lecture d'un compte il y a `days_ago` jours (`None` : vu sans date) ; pour une série, un épisode `ep`.
    fn watch(seen: &mut Seen, t: &Title, ep: u32, days_ago: Option<i64>) {
        let user_data = match days_ago {
            Some(d) => json!({"Played": true, "LastPlayedDate": iso(NOW - d * DAY)}),
            None => json!({"Played": true}),
        };
        let it = if t.movie {
            json!({"Id": jid(t), "Type": "Movie", "UserData": user_data})
        } else {
            json!({"Id": format!("{}e{ep}", jid(t)), "Type": "Episode", "SeriesId": jid(t),
                "UserData": user_data})
        };
        seen.add_account_item(&it);
    }

    struct Case {
        titles: Vec<Title>,
        seen: Seen,
        imports: LastImports,
        playback_reporting: bool,
        unreadable_accounts: u32,
        exemptions: BTreeMap<String, Exemption>,
        undated: BTreeMap<String, i64>,
        cfg: Cfg,
    }

    impl Case {
        fn new(titles: Vec<Title>) -> Self {
            Self {
                titles,
                seen: Seen::default(),
                imports: LastImports::new(),
                playback_reporting: true,
                unreadable_accounts: 0,
                exemptions: BTreeMap::new(),
                undated: BTreeMap::new(),
                cfg: cfg(),
            }
        }

        fn run(&self) -> (RedListReport, BTreeMap<String, i64>) {
            build_red_list(
                &Inputs {
                    titles: &self.titles,
                    jf: &jf_for(&self.titles),
                    seen: &self.seen,
                    requests: &Requests::unavailable(),
                    last_imports: &self.imports,
                    playback_reporting: self.playback_reporting,
                    unreadable_accounts: self.unreadable_accounts,
                },
                &self.cfg,
                &self.exemptions,
                &self.undated,
                NOW,
            )
        }
    }

    fn keys(v: &[RedEntry]) -> Vec<&str> {
        v.iter().map(|e| e.key.as_str()).collect()
    }

    #[test]
    fn a_never_played_title_turns_red_at_the_threshold() {
        let c = Case::new(vec![
            title("Vieux", true, "seedbox", 1, 5_000, 61),
            title("Pile", true, "seedbox", 2, 4_000, 60),
            title("Presque", true, "seedbox", 3, 3_000, 59),
            title("Neuf", true, "seedbox", 4, 2_000, 5),
        ]);
        let (r, _) = c.run();
        assert_eq!(
            keys(&r.entries),
            ["movie:tmdb:1", "movie:tmdb:2"],
            "61 et 60 jours sans lecture : rouges, les plus gros d'abord"
        );
        assert_eq!(
            keys(&r.soon),
            ["movie:tmdb:3"],
            "59 jours : bientôt rouge (sous 14 j), pas rouge"
        );
        assert_eq!(r.soon[0].due, Some(NOW + DAY), "rouge dans un jour");
        assert_eq!(r.evaluated, 4);
        assert!(r.complete && r.missing.is_empty());
        let e = &r.entries[0];
        assert_eq!(e.last_played, None);
        assert!(
            e.reason.starts_with("jamais lu, arrivé le "),
            "{}",
            e.reason
        );
        assert_eq!(e.sides.len(), 1);
        assert_eq!(
            (
                e.sides[0].side.as_str(),
                e.sides[0].arr_id,
                e.sides[0].bytes
            ),
            ("seedbox", 2001, 5_000)
        );
        assert_eq!(e.sides[0].folder, "/arr/seedbox/Vieux");
        assert_eq!(e.sides[0].jf_id, "seedboxvieux");
    }

    #[test]
    fn the_last_play_restarts_the_clock() {
        let mut c = Case::new(vec![
            title("LuRecemment", true, "vps", 1, 9_000, 300),
            title("LuIlYaLongtemps", true, "vps", 2, 8_000, 300),
        ]);
        let (recent, old) = (c.titles[0].clone(), c.titles[1].clone());
        watch(&mut c.seen, &recent, 0, Some(10));
        watch(&mut c.seen, &old, 0, Some(61));
        let (r, _) = c.run();
        assert_eq!(keys(&r.entries), ["movie:tmdb:2"], "lu il y a 61 j : rouge");
        assert!(r.soon.is_empty(), "lu il y a 10 j : ni rouge ni bientôt");
        let e = &r.entries[0];
        assert_eq!(e.last_played, Some(NOW - 61 * DAY));
        assert!(e.reason.starts_with("dernière lecture le "), "{}", e.reason);
    }

    #[test]
    fn a_series_needs_sixty_days_without_play_and_without_new_episode() {
        let mut c = Case::new(vec![
            title("Diffusee", false, "seedbox", 1, 30_000, 200),
            title("Importee", false, "seedbox", 2, 20_000, 200),
            title("Endormie", false, "seedbox", 3, 10_000, 200),
        ]);
        // vue il y a 100 j, mais un épisode diffusé il y a 10 j : pas rouge
        watch(&mut c.seen, &c.titles[0].clone(), 1, Some(100));
        c.titles[0].previous_airing = Some(NOW - 10 * DAY);
        // jamais vue, un épisode importé il y a 5 j : pas rouge
        c.imports.insert(
            ("seedbox".into(), c.titles[1].arr_id),
            LastImport::Read(Some(NOW - 5 * DAY)),
        );
        // jamais vue ; diffusion « à venir » (lastAired futur) ignorée, import ancien : rouge
        c.titles[2].last_aired = Some(NOW + 30 * DAY);
        c.imports.insert(
            ("seedbox".into(), c.titles[2].arr_id),
            LastImport::Read(Some(NOW - 90 * DAY)),
        );
        let (r, _) = c.run();
        assert_eq!(keys(&r.entries), ["series:tvdb:3"]);
        assert!(
            r.entries[0]
                .reason
                .starts_with("jamais lue, rien de neuf depuis le "),
            "{}",
            r.entries[0].reason
        );
        assert_eq!(r.entries[0].arrived, Some(NOW - 90 * DAY));
        // la plus récente des lectures d'épisodes compte pour la série
        let mut c = Case::new(vec![title("Longue", false, "vps", 4, 1, 300)]);
        let t = c.titles[0].clone();
        watch(&mut c.seen, &t, 1, Some(200));
        watch(&mut c.seen, &t, 2, Some(50));
        let (r, _) = c.run();
        assert!(
            r.entries.is_empty(),
            "un épisode lu il y a 50 j : pas rouge"
        );
        assert_eq!(
            r.soon[0].last_played,
            Some(NOW - 50 * DAY),
            "le plus récent des épisodes"
        );
    }

    #[test]
    fn unreadable_episode_files_keep_a_series_out() {
        let mut c = Case::new(vec![title("Muette", false, "seedbox", 1, 1, 200)]);
        c.imports.insert(
            ("seedbox".into(), c.titles[0].arr_id),
            LastImport::Unreadable,
        );
        let (r, _) = c.run();
        assert!(r.entries.is_empty());
        assert_eq!(keys(&r.unmatched), ["series:tvdb:1"]);
        assert_eq!(r.unmatched[0].reason, FILES_UNREADABLE);
    }

    #[test]
    fn only_candidate_series_have_their_files_read() {
        let mut c = Case::new(vec![
            title("Rouge", false, "seedbox", 1, 1, 200),
            title("Bientot", false, "vps", 2, 1, 50),
            title("Recente", false, "vps", 3, 1, 5),
            title("Film", true, "vps", 4, 1, 200),
        ]);
        c.titles.push(title("Rouge", false, "vps", 1, 1, 200));
        let (r, _) = c.run();
        assert_eq!(
            series_candidates(&r),
            [
                ("seedbox".to_string(), 2001),
                ("vps".to_string(), 1001),
                ("vps".to_string(), 1002)
            ],
            "séries rouges ou bientôt rouges, chaque copie ; ni film ni série récente"
        );
    }

    #[test]
    fn seen_without_date_starts_a_clock_at_the_first_observation() {
        let mut c = Case::new(vec![title("Marque", true, "vps", 1, 1, 300)]);
        watch(&mut c.seen, &c.titles[0].clone(), 0, None);
        let (r, clocks) = c.run();
        assert!(
            r.entries.is_empty(),
            "première observation : l'horloge part de maintenant"
        );
        assert_eq!(clocks["movie:tmdb:1"], NOW);
        // horloge démarrée il y a 61 j : rouge, et la date de départ est gardée
        c.undated.insert("movie:tmdb:1".into(), NOW - 61 * DAY);
        let (r, clocks) = c.run();
        assert_eq!(keys(&r.entries), ["movie:tmdb:1"]);
        assert_eq!(r.entries[0].undated_since, Some(NOW - 61 * DAY));
        assert!(
            r.entries[0].reason.starts_with("vu sans date"),
            "{}",
            r.entries[0].reason
        );
        assert_eq!(clocks["movie:tmdb:1"], NOW - 61 * DAY);
        // une date apparaît (Playback Reporting) : l'horloge s'arrête, la date fait foi
        c.seen.add_play(&jid(&c.titles[0]), Some(NOW - 70 * DAY));
        let (r, clocks) = c.run();
        assert!(clocks.is_empty(), "plus de vu sans date : horloge retirée");
        assert_eq!(r.entries[0].last_played, Some(NOW - 70 * DAY));
        assert_eq!(r.entries[0].undated_since, None);
    }

    #[test]
    fn clocks_survive_an_incomplete_run_and_a_title_not_evaluated() {
        let mut c = Case::new(vec![
            title("Present", true, "vps", 1, 1, 300),
            title("Perdu", true, "vps", 2, 1, 300),
        ]);
        c.titles[1].jf_path = Some(LOST.into());
        c.undated = BTreeMap::from([
            ("movie:tmdb:1".to_string(), NOW - 10 * DAY),
            ("movie:tmdb:2".to_string(), NOW - 20 * DAY),
            ("movie:tmdb:9".to_string(), NOW - 30 * DAY),
        ]);
        // calcul complet : « Present » n'est plus vu sans date, « Perdu » non évalué garde la sienne, le titre disparu
        // du catalogue perd la sienne
        let (_, clocks) = c.run();
        assert_eq!(
            clocks,
            BTreeMap::from([("movie:tmdb:2".to_string(), NOW - 20 * DAY)])
        );
        // calcul incomplet : rien n'est retiré
        c.playback_reporting = false;
        let (r, clocks) = c.run();
        assert_eq!(clocks, c.undated);
        assert!(!r.complete);
    }

    #[test]
    fn a_title_on_both_sides_is_one_entry_red_only_if_every_copy_is() {
        let mut c = Case::new(vec![
            title("Double", true, "vps", 1, 4_000, 200),
            title("Double", true, "seedbox", 1, 6_000, 200),
        ]);
        let (r, _) = c.run();
        assert_eq!(keys(&r.entries), ["movie:tmdb:1"], "une seule entrée");
        let e = &r.entries[0];
        assert_eq!(e.bytes, 10_000, "les deux copies comptées");
        let sides: Vec<(&str, i64)> = e
            .sides
            .iter()
            .map(|s| (s.side.as_str(), s.arr_id))
            .collect();
        assert_eq!(sides, [("vps", 1001), ("seedbox", 2001)]);
        // la copie du VPS est lue il y a 3 j : le titre n'est plus rouge
        watch(&mut c.seen, &c.titles[0].clone(), 0, Some(3));
        let (r, _) = c.run();
        assert!(r.entries.is_empty() && r.soon.is_empty());
        // une copie arrivée récemment suffit aussi
        let mut c = Case::new(vec![
            title("Double", true, "vps", 1, 4_000, 200),
            title("Double", true, "seedbox", 1, 6_000, 10),
        ]);
        let (r, _) = c.run();
        assert!(r.entries.is_empty());
        // une copie absente de Jellyfin : le titre n'est pas évalué du tout
        c.titles[1].arrived = Some(NOW - 200 * DAY);
        c.titles[1].jf_path = Some(LOST.into());
        let (r, _) = c.run();
        assert!(r.entries.is_empty());
        assert_eq!(r.unmatched[0].reason, "une copie absente de Jellyfin");
    }

    #[test]
    fn titles_that_cannot_be_evaluated_are_never_red() {
        let mut c = Case::new(vec![
            title("Perdu", true, "seedbox", 1, 7_000, 300),
            title("SansDate", true, "seedbox", 2, 5_000, 300),
            title("SansId", true, "vps", 3, 3_000, 300),
            title("Rouge", true, "vps", 4, 1_000, 300),
        ]);
        c.titles[0].jf_path = Some(LOST.into());
        c.titles[1].arrived = None;
        c.titles[2].tmdb = 0;
        let (r, _) = c.run();
        assert_eq!(keys(&r.entries), ["movie:tmdb:4"]);
        let why: Vec<(&str, &str)> = r
            .unmatched
            .iter()
            .map(|e| (e.key.as_str(), e.reason.as_str()))
            .collect();
        assert_eq!(
            why,
            [
                ("movie:tmdb:1", NOT_IN_JELLYFIN),
                ("movie:tmdb:2", NO_ARRIVAL),
                ("", NO_ID)
            ]
        );
        assert!(r.unmatched.iter().all(|e| e.due.is_none()));
        assert_eq!(r.evaluated, 1);
    }

    #[test]
    fn an_exempted_title_is_listed_apart_and_comes_back_when_the_exemption_goes() {
        let mut c = Case::new(vec![
            title("Garde", true, "vps", 1, 9_000, 300),
            title("Rouge", true, "vps", 2, 1_000, 300),
            title("Recent", true, "vps", 3, 1_000, 3),
        ]);
        let ex = |title: &str| Exemption {
            at: NOW - DAY,
            title: title.into(),
        };
        c.exemptions.insert("movie:tmdb:1".into(), ex("Garde"));
        c.exemptions.insert("movie:tmdb:3".into(), ex("Recent"));
        let (mut r, _) = c.run();
        assert_eq!(keys(&r.entries), ["movie:tmdb:2"]);
        assert_eq!(
            keys(&r.exempted),
            ["movie:tmdb:1", "movie:tmdb:3"],
            "exemptés rouges ou non, par titre"
        );
        // « Retirer l'exemption » : la page regroupe sans recalcul
        c.exemptions.remove("movie:tmdb:1");
        c.exemptions.remove("movie:tmdb:3");
        regroup(&mut r, &c.exemptions);
        assert_eq!(keys(&r.entries), ["movie:tmdb:1", "movie:tmdb:2"]);
        assert!(r.exempted.is_empty(), "le titre récent n'est plus listé");
        // « Exempter » : retiré de la liste rouge tout de suite
        c.exemptions.insert("movie:tmdb:2".into(), ex("Rouge"));
        regroup(&mut r, &c.exemptions);
        assert_eq!(keys(&r.entries), ["movie:tmdb:1"]);
        assert_eq!(keys(&r.exempted), ["movie:tmdb:2"]);
    }

    #[test]
    fn the_russian_route_is_labelled_and_can_be_left_out() {
        let mut c = Case::new(vec![
            title("Russe", false, "seedbox", 1, 1, 300),
            title("Autre", false, "seedbox", 2, 1, 300),
        ]);
        c.titles[0].russian = true;
        let (r, _) = c.run();
        assert_eq!(r.entries.len(), 2, "voie russe incluse par défaut");
        let ru = r.entries.iter().find(|e| e.key == "series:tvdb:1").unwrap();
        assert!(ru.russian, "étiquette voie russe");
        assert!(
            !r.entries
                .iter()
                .find(|e| e.key == "series:tvdb:2")
                .unwrap()
                .russian
        );
        c.cfg.include_russian = false;
        let (r, _) = c.run();
        assert_eq!(keys(&r.entries), ["series:tvdb:2"]);
        assert!(r.unmatched.is_empty(), "exclue : nulle part");
    }

    #[test]
    fn a_calculation_without_playback_reporting_or_an_account_is_incomplete() {
        let mut c = Case::new(vec![title("X", true, "vps", 1, 1, 300)]);
        c.playback_reporting = false;
        c.unreadable_accounts = 2;
        let (r, _) = c.run();
        assert!(!r.complete);
        assert_eq!(
            r.missing,
            [
                "Playback Reporting muet",
                "2 compte(s) Jellyfin illisible(s)"
            ]
        );
        assert_eq!(
            r.entries.len(),
            1,
            "la liste reste affichée, « Nettoyer » sera refusé"
        );
        assert!(summary(&r).contains("INCOMPLET"), "{}", summary(&r));
    }

    #[test]
    fn keys_are_stable_and_strictly_parsed() {
        let mut s = title("S", false, "vps", 7, 1, 1);
        assert_eq!(key_of(&s).as_deref(), Some("series:tvdb:7"));
        s.tvdb = 0;
        assert_eq!(
            key_of(&s).as_deref(),
            Some("series:tmdb:70"),
            "TMDB à défaut de TVDB"
        );
        s.tmdb = 0;
        assert_eq!(key_of(&s), None);
        let m = title("M", true, "vps", 5, 1, 1);
        assert_eq!(key_of(&m).as_deref(), Some("movie:tmdb:5"));
        assert_eq!(parse_key("movie:tmdb:5"), Some((true, "tmdb", 5)));
        assert_eq!(parse_key("series:tvdb:7"), Some((false, "tvdb", 7)));
        assert_eq!(parse_key("series:tmdb:70"), Some((false, "tmdb", 70)));
        for bad in [
            "",
            "movie:tvdb:5",
            "movie:tmdb:0",
            "movie:tmdb:-5",
            "movie:tmdb:+5",
            "movie:tmdb:5:x",
            "movie:tmdb:5 ",
            "movie:tmdb:",
            "film:tmdb:5",
            "movie:tmdb:99999999999999999999",
            "movie:tmdb:5<script>",
        ] {
            assert_eq!(parse_key(bad), None, "clé {bad:?} acceptée");
        }
    }

    #[test]
    fn the_last_import_is_the_latest_episode_file() {
        let files = [
            json!({"dateAdded": "2026-08-26T12:22:34Z"}),
            json!({"dateAdded": "2026-10-01T08:00:00Z"}),
            json!({"dateAdded": "hier"}),
            json!({}),
        ];
        assert_eq!(last_import(&files), parse_date("2026-10-01T08:00:00Z"));
        assert_eq!(last_import(&[]), None);
    }

    #[test]
    fn the_list_is_sorted_biggest_first_and_soon_by_date() {
        let c = Case::new(vec![
            title("Petit", true, "vps", 1, 1_000, 100),
            title("Gros", true, "vps", 2, 9_000, 100),
            title("Moyen", true, "vps", 3, 5_000, 100),
            title("BientotLoin", true, "vps", 4, 9_000, 47),
            title("BientotProche", true, "vps", 5, 1_000, 58),
        ]);
        let (r, _) = c.run();
        assert_eq!(
            keys(&r.entries),
            ["movie:tmdb:2", "movie:tmdb:3", "movie:tmdb:1"]
        );
        assert_eq!(keys(&r.soon), ["movie:tmdb:5", "movie:tmdb:4"]);
    }
}
