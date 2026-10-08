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
//! - pour une **série**, Jellyfin réécrit aussi chaque saison et chaque épisode rangé dans une saison, AVANT la série
//!   (`ItemUpdateController.UpdateItem`, bloc `if (item is Series rseries)`) : leur classification (`OfficialRating`)
//!   reçoit celle de la série, **quelle qu'elle soit**, sauf si ce champ est verrouillé chez eux (`LockedFields`), et
//!   leur classification personnalisée (`CustomRating`) reçoit celle de la série **sans aucune exception**. Une
//!   saison ou un épisode qui avait sa propre valeur la perdrait : la série n'est alors pas écrite
//!   (`children_differ`, `children_check`). Sinon, seuls changent les enfants non verrouillés sans classification
//!   (1 458 épisodes et 130 saisons sur la copie du 08/10 ; 0 enfant avec sa propre valeur sur 2 297, vérifié en
//!   lecture seule le 08/10) : ils reçoivent la valeur qu'ils héritaient déjà, et leurs ids sont notés dans l'état
//!   (`children_unrated`) pour le retour arrière. D'où aussi l'interrupteur `series` (false par défaut) ;
//! - **regroupement** : `GET /Items` avec un compte et `IncludeItemTypes=Series` ne garde qu'une fiche par
//!   `PresentationUniqueKey` ; les bibliothèques de séries ayant `EnableAutomaticSeriesGrouping`, une série présente
//!   dans deux dossiers (VPS et seedbox) n'y apparaît qu'une fois (un cas le 08/10). Les épisodes, eux, ne sont pas
//!   regroupés : la fiche cachée est retrouvée par leur `SeriesId` (`unlisted_series`), quand les séries sont en jeu.
//!   Pour la même raison, les saisons et épisodes d'une série sont lus avec et sans compte, puis filtrés sur leur
//!   `SeriesId` (`own_children`) : avec un compte, la liste mêle les deux copies et cache une saison de même numéro.
//!
//! Garde-fous : fenêtre du matin, quelques fiches par passage, films d'abord (les séries attendent qu'aucun film ne
//! reste à essayer), jamais une fiche en lecture ni pendant une analyse de la médiathèque, fiche verrouillée laissée
//! telle quelle, langue TMDB inconnue (`xx`, vide) = rien, chaque écriture relue et notée dans l'état (avant/après).
//! Une fiche écrite (`written`) n'est **jamais** réécrite d'office : remise à vide par un retour arrière, elle le
//! reste (comptée « remise à vide depuis l'écriture »). Pareil pour une écriture partie dont la réponse est en échec
//! (`posted`) : on ne sait pas si Jellyfin l'a appliquée (pour une série, les enfants le sont avant la série) ; elle
//! passe en `written` dès qu'un passage relit la langue écrite (`settle`), sinon elle reste « non confirmée ».
//! Revenir en arrière : `backups/original-language-20261008/rollback.py` (tâche coupée d'abord pour un retour complet)
//! réécrit `old` (vide = `null`) par `update_body` sur les fiches `written` (et `error` + `posted` dont la langue est
//! celle écrite), puis remet à `null` la classification des `children_unrated` qui ont encore celle de la série.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Essayées depuis moins de `retry_hours` (TMDB sans langue, échec net, fiche verrouillée, enfants à part).
    pub deferred: usize,
    /// Sautées ce passage : en lecture.
    pub playing: usize,
    /// Écrites par la tâche puis revenues vides (retour arrière, rafraîchissement) : jamais réécrites d'office.
    pub reverted: usize,
    /// Écriture partie, réponse en échec, langue toujours vide : Jellyfin a pu en appliquer une partie (les enfants
    /// d'une série) ; jamais reprises d'office.
    pub unconfirmed: usize,
}

/// Choix d'un passage : films d'abord (les séries attendent qu'aucun film ne reste à essayer, et seulement si
/// `series`), au plus `max_*_per_run`, jamais une fiche en lecture ni une fiche essayée depuis moins de `retry_hours`.
/// Une fiche déjà écrite (`written`) ou dont l'écriture est partie sans confirmation (`error` + `posted`) n'est
/// jamais reprise (2026-10-08) : sinon un retour arrière serait défait au matin suivant.
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
            continue;
        }
        if let Some(r) = tried.get(&c.id) {
            if r.outcome == "written" {
                p.reverted += 1;
                continue;
            }
            if r.outcome == "error" && r.posted {
                p.unconfirmed += 1;
                continue;
            }
            if now - r.at < retry {
                p.deferred += 1;
                continue;
            }
        }
        if c.kind == Kind::Movie {
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

/// Écriture notée en échec alors que la requête était partie (`posted`) : si la fiche porte maintenant la langue
/// qu'on écrivait, c'est la nôtre → `written` (que le retour arrière la reprenne et que la tâche ne la réécrive
/// jamais). `None` = rien à changer.
pub fn settle(prev: &OriginalLanguageRecord, current: &str) -> Option<OriginalLanguageRecord> {
    let current = current.trim();
    (prev.outcome == "error" && prev.posted && !prev.new.is_empty() && current == prev.new).then(
        || {
            let mut r = prev.clone();
            r.outcome = "written".into();
            r.detail = truncate(
                &format!(
                    "réponse en échec, langue relue ensuite : écrite ({})",
                    prev.detail
                ),
                200,
            );
            r
        },
    )
}

/// Id Jellyfin au format compact (sans tirets, minuscules).
fn compact(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

/// Valeur texte d'un champ (absent, `null` ou blanc = vide).
fn text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Classification verrouillée sur cette fiche (`LockedFields` contient `OfficialRating`) : Jellyfin n'y touche pas.
fn rating_locked(item: &Value) -> bool {
    item.get("LockedFields")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|f| f.as_str() == Some("OfficialRating")))
}

/// Nom court d'une saison ou d'un épisode pour un message (`S02E05`, `saison 2`, sinon son nom).
fn child_label(c: &Value) -> String {
    let n = |k: &str| c.get(k).and_then(Value::as_i64);
    match (
        c.get("Type").and_then(Value::as_str),
        n("ParentIndexNumber"),
        n("IndexNumber"),
    ) {
        (Some("Episode"), Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
        (Some("Season"), _, Some(s)) => format!("saison {s}"),
        _ => text(c, "Name"),
    }
}

/// Ce que l'écriture d'une série ferait à ses saisons et épisodes (`children` = `own_children` de `GET /Items?ParentId=
/// <série>&Recursive=true&IncludeItemTypes=Season,Episode&Fields=CustomRating,Settings`). Jellyfin 12.1 leur donne la
/// classification de la série (sauf champ verrouillé chez eux) et sa classification personnalisée (sans exception).
/// `Ok(ids)` : rien n'est perdu ; `ids` = enfants non verrouillés sans classification qui vont recevoir celle de la
/// série (vide si la série n'en a pas), à remettre à vide en cas de retour arrière. `Err(raison)` : un enfant a sa
/// propre valeur, que l'écriture effacerait sans trace : la série n'est pas écrite (2026-10-08). Les épisodes hors
/// saison, que Jellyfin ne réécrit pas, sont comptés aussi : prudence plutôt que précision.
pub fn children_check(series: &Value, children: &[Value]) -> Result<Vec<String>, String> {
    let rating = text(series, "OfficialRating");
    let custom = text(series, "CustomRating");
    let (mut unrated, mut differ) = (Vec::new(), Vec::new());
    for c in children {
        let (own, own_custom, locked) = (
            text(c, "OfficialRating"),
            text(c, "CustomRating"),
            rating_locked(c),
        );
        if !locked && !own.is_empty() && own != rating {
            differ.push(format!(
                "{} : classification « {own} », série « {rating} »",
                child_label(c)
            ));
        } else if own_custom != custom {
            differ.push(format!(
                "{} : classification personnalisée « {own_custom} », série « {custom} »",
                child_label(c)
            ));
        } else if !locked && own.is_empty() && !rating.is_empty() {
            if let Some(id) = c.get("Id").and_then(Value::as_str) {
                unrated.push(compact(id));
            }
        }
    }
    match differ.first() {
        None => Ok(unrated),
        Some(first) => Err(format!(
            "{} saison(s)/épisode(s) ont leur propre valeur, que Jellyfin remplacerait par celle de la série (ex. {first})",
            differ.len()
        )),
    }
}

/// Séries que `GET /Items?IncludeItemTypes=Series` ne montre pas (regroupées avec une autre fiche du même titre) mais
/// dont des épisodes existent : ids compacts, sans doublon, dans l'ordre des épisodes.
pub fn unlisted_series(listed: &[Value], episodes: &[Value]) -> Vec<String> {
    let known: BTreeSet<String> = listed
        .iter()
        .filter(|i| i.get("Type").and_then(Value::as_str) == Some("Series"))
        .filter_map(|i| i.get("Id").and_then(Value::as_str))
        .map(compact)
        .collect();
    let mut seen = BTreeSet::new();
    episodes
        .iter()
        .filter_map(|e| e.get("SeriesId").and_then(Value::as_str))
        .map(compact)
        .filter(|id| !known.contains(id) && seen.insert(id.clone()))
        .collect()
}

/// Saisons et épisodes de CETTE fiche série (`SeriesId` = `series_id`), sans doublon, d'après des listes
/// `GET /Items?ParentId=<série>&Recursive=true`. Une série regroupée avec une autre fiche (même titre dans deux
/// dossiers) liste les enfants des deux fiches ; avec un compte, Jellyfin cache en plus une saison qui a le même numéro
/// qu'une saison de l'autre fiche (regroupement par `PresentationUniqueKey`), sans compte rien n'est regroupé. Vu sur
/// l'instance d'essai le 2026-10-08 : la saison de la copie VPS de Chainsmoker Cat, réécrite par la cascade,
/// n'apparaissait que dans la liste sans compte, et la liste avec compte portait les 13 enfants de la copie seedbox.
pub fn own_children(series_id: &str, lists: &[Vec<Value>]) -> Vec<Value> {
    let me = compact(series_id);
    let mut seen = BTreeSet::new();
    lists
        .iter()
        .flatten()
        .filter(|c| c.get("SeriesId").and_then(Value::as_str).map(compact) == Some(me.clone()))
        .filter(|c| {
            c.get("Id")
                .and_then(Value::as_str)
                .is_some_and(|id| seen.insert(compact(id)))
        })
        .cloned()
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

/// Films et séries vus par le compte admin, les plus récemment ajoutés d'abord.
async fn listing(ctx: &TaskContext, admin: &str) -> Result<Vec<Value>> {
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
        out.extend(items);
    }
    Ok(out)
}

/// Fiches des séries regroupées par Jellyfin (absentes de `listing`), retrouvées par les épisodes (~2 300 épisodes,
/// 1,8 Mo, 0,7 s le 08/10 : lu seulement quand les séries sont en jeu).
async fn hidden_series(ctx: &TaskContext, admin: &str, listed: &[Value]) -> Result<Vec<Value>> {
    let episodes = ctx
        .jellyfin
        .items_paged(
            &[
                ("UserId", admin),
                ("Recursive", "true"),
                ("IncludeItemTypes", "Episode"),
                ("EnableImages", "false"),
                ("EnableUserData", "false"),
            ],
            1000,
        )
        .await?;
    let mut out = Vec::new();
    for id in unlisted_series(listed, &episodes) {
        match ctx.jellyfin.item(&id, admin).await {
            Ok(d) if d.get("Type").and_then(Value::as_str) == Some("Series") => out.push(d),
            Ok(_) => {}
            Err(e) => {
                warn!(task = "original_language", id = %id, error = %format!("{e:#}"), "série hors liste illisible")
            }
        }
    }
    Ok(out)
}

/// Saisons et épisodes de la fiche série `id` (voir `own_children`) : liste avec le compte admin et liste sans compte,
/// réunies puis filtrées sur `SeriesId`.
async fn series_children(ctx: &TaskContext, admin: &str, id: &str) -> Result<Vec<Value>> {
    let base = [
        ("ParentId", id),
        ("Recursive", "true"),
        ("IncludeItemTypes", "Season,Episode"),
        ("Fields", "CustomRating,Settings"),
        ("EnableImages", "false"),
        ("EnableUserData", "false"),
    ];
    let mut with_user = vec![("UserId", admin)];
    with_user.extend(base);
    let a = ctx.jellyfin.items(&with_user).await?;
    let b = ctx.jellyfin.items(&base).await?;
    Ok(own_children(id, &[a, b]))
}

/// Écritures parties sans confirmation (`error` + `posted`) dont la fiche porte maintenant la langue écrite : à noter
/// `written`. Langue lue dans `langs` (la liste du passage), sinon fiche relue une à une (rares).
async fn settle_posted(
    ctx: &TaskContext,
    admin: &str,
    tried: &BTreeMap<String, OriginalLanguageRecord>,
    langs: &BTreeMap<String, String>,
) -> Vec<(String, OriginalLanguageRecord)> {
    let mut out = Vec::new();
    for (id, r) in tried
        .iter()
        .filter(|(_, r)| r.outcome == "error" && r.posted)
    {
        let current = match langs.get(id) {
            Some(l) => l.clone(),
            None => match ctx.jellyfin.item(id, admin).await {
                Ok(d) => language_of(&d),
                Err(_) => continue,
            },
        };
        if let Some(done) = settle(r, &current) {
            out.push((id.clone(), done));
        }
    }
    out
}

/// Une fiche : langue TMDB, fiche relue, écriture, relecture. `None` = rien à noter (déjà remplie entre-temps par
/// autre chose qu'une écriture de la tâche). `prev` = dernier passage noté sur cette fiche.
async fn fill(
    ctx: &TaskContext,
    admin: &str,
    c: &Candidate,
    prev: Option<&OriginalLanguageRecord>,
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
        // remplie entre-temps : par une écriture de la tâche dont la réponse était en échec, ou par autre chose
        return prev.and_then(|p| settle(p, &rec.old));
    }
    if dto.get("LockData").and_then(Value::as_bool) == Some(true) {
        rec.outcome = "locked".into();
        return Some(rec);
    }
    if c.kind == Kind::Series {
        // Jellyfin va réécrire saisons et épisodes : refus si l'un d'eux perdrait sa propre valeur, sinon noter
        // ceux dont la classification était vide (retour arrière)
        let children = match series_children(ctx, admin, &c.id).await {
            Ok(ch) => ch,
            Err(e) => {
                rec.outcome = "error".into();
                rec.detail = truncate(&format!("épisodes : {e:#}"), 200);
                return Some(rec);
            }
        };
        match children_check(&dto, &children) {
            Ok(ids) => rec.children_unrated = ids,
            Err(why) => {
                rec.outcome = "children_differ".into();
                rec.detail = truncate(&why, 300);
                return Some(rec);
            }
        }
        rec.children_rating = text(&dto, "OfficialRating");
    }
    if ctx.dry_run {
        rec.outcome = "dry-run".into();
        return Some(rec);
    }
    let body = update_body(&dto, "OriginalLanguage", json!(lang));
    if let Err(e) = ctx.jellyfin.update_item(&c.id, &body).await {
        rec.outcome = "error".into();
        rec.posted = e.maybe_applied;
        rec.detail = truncate(&format!("écriture : {e}"), 200);
        // réponse en échec mais requête partie : une relecture tranche si la langue est là
        if e.maybe_applied {
            if let Ok(back) = ctx.jellyfin.item(&c.id, admin).await {
                if let Some(done) = settle(&rec, &language_of(&back)) {
                    return Some(done);
                }
            }
        }
        return Some(rec);
    }
    rec.posted = true;
    // relue : Jellyfin ramène un code connu à deux lettres ; s'il en garde un autre, c'est celui-là qui est noté
    match ctx.jellyfin.item(&c.id, admin).await {
        Ok(back) => match language_of(&back) {
            got if got.is_empty() => {
                // réponse 2xx mais langue vide : non confirmée, jamais reprise d'office (enfants peut-être réécrits)
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
            // écrite (réponse 2xx) mais non relue : notée comme écrite
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
        let listed = listing(ctx, &admin).await?;
        let mut langs: BTreeMap<String, String> = listed
            .iter()
            .filter_map(|i| {
                i.get("Id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_string(), language_of(i)))
            })
            .collect();
        let mut cands: Vec<Candidate> = listed.iter().filter_map(candidate).collect();
        // sans la liste des lectures, on n'écrit pas à l'aveugle
        let playing = ctx.jellyfin.playing_paths().await?;
        let mut tried = ctx.state.read(|s| s.original_language.clone()).await;
        let now = crate::state::now();
        // séries regroupées par Jellyfin : cherchées quand les séries sont en jeu (ou pour un dry-run)
        let mut hidden = 0;
        if ctx.dry_run || (cfg.series && plan(&cands, &tried, &playing, now, cfg).movies_due == 0) {
            for dto in hidden_series(ctx, &admin, &listed).await? {
                hidden += 1;
                if let Some(id) = dto.get("Id").and_then(Value::as_str) {
                    langs.insert(id.to_string(), language_of(&dto));
                }
                cands.extend(candidate(&dto));
            }
        }
        // écritures parties dont la réponse était en échec : notées `written` si la langue écrite est là
        let settled = settle_posted(ctx, &admin, &tried, &langs).await;
        for (id, r) in &settled {
            info!(task = "original_language", title = %r.name, lang = %r.new, "écriture constatée après une réponse en échec");
            tried.insert(id.clone(), r.clone());
        }
        if !settled.is_empty() && !ctx.dry_run {
            let settled = settled.clone();
            ctx.state
                .update(|s| {
                    for (id, r) in settled {
                        s.original_language.insert(id, r);
                    }
                })
                .await?;
        }
        let p = plan(&cands, &tried, &playing, now, cfg);
        let series_note = if cfg.series { "" } else { " (désactivées)" };
        let mut rest = format!(
            "à remplir : {} film(s), {} série(s){series_note} ; sans TMDB : {} ; en attente de nouvel essai : {}",
            p.movies_due, p.series_due, p.no_tmdb, p.deferred,
        );
        for (n, what) in [
            (p.playing, "en lecture"),
            (
                p.reverted,
                "remises à vide depuis l'écriture (jamais réécrites d'office)",
            ),
            (
                p.unconfirmed,
                "écritures non confirmées (jamais reprises d'office)",
            ),
            (hidden, "séries hors liste (regroupées par Jellyfin)"),
            (settled.len(), "écritures constatées après coup"),
        ] {
            if n > 0 {
                rest.push_str(&format!(" ; {what} : {n}"));
            }
        }
        if p.todo.is_empty() {
            return Ok(Report::new(format!("rien à écrire — {rest}"), 0));
        }
        ctx.jellyseerr
            .status()
            .await
            .context("Jellyseerr injoignable : langues TMDB illisibles")?;
        let (mut written, mut other) = (Vec::new(), Vec::new());
        for c in &p.todo {
            let Some(rec) = fill(ctx, &admin, c, tried.get(&c.id), now).await else {
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
                    match rec.outcome.as_str() {
                        "error" => {
                            warn!(task = "original_language", title = %c.name, detail = %rec.detail, posted = rec.posted, "langue d'origine non écrite")
                        }
                        "children_differ" => {
                            info!(task = "original_language", title = %c.name, detail = %rec.detail, "série laissée : un enfant a sa propre classification")
                        }
                        _ => {}
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
    use crate::clients::jellyfin::write_maybe_applied;

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
    fn a_written_title_that_became_empty_again_is_never_rewritten() {
        // retour arrière fait (rollback.py) : la fiche est revenue vide, l'état garde « written » d'il y a 2 jours
        let c = vec![
            cand("m1", Kind::Movie, 1, "/media/movies/A/A.mkv"),
            cand("m2", Kind::Movie, 2, "/media/movies/B/B.mkv"),
            cand("m3", Kind::Movie, 3, "/media/movies/C/C.mkv"),
            cand("m4", Kind::Movie, 4, "/media/movies/D/D.mkv"),
        ];
        let now = 1_000_000;
        let mut posted = tried("m2", now - 30 * 24 * 3600, "error").1;
        posted.posted = true;
        let t: BTreeMap<_, _> = [
            tried("m1", now - 48 * 3600, "written"),
            ("m2".to_string(), posted),
            tried("m3", now - 48 * 3600, "error"),
            tried("m4", now - 48 * 3600, "children_differ"),
        ]
        .into_iter()
        .collect();
        let p = plan(&c, &t, &[], now, &cfg());
        let ids: Vec<&str> = p.todo.iter().map(|c| c.id.as_str()).collect();
        // m1 écrite puis remise à vide : jamais reprise ; m2 écriture partie sans confirmation : jamais reprise ;
        // m3 (échec net) et m4 (enfants à part) : reprises après le délai
        assert_eq!(ids, ["m3", "m4"]);
        assert_eq!((p.reverted, p.unconfirmed, p.movies_due), (1, 1, 2));
        // même vieille d'un an, même avec un délai de nouvel essai à 0
        let zero = config::OriginalLanguage {
            retry_hours: 0,
            ..cfg()
        };
        let old: BTreeMap<_, _> = [tried("m1", now - 365 * 24 * 3600, "written")]
            .into_iter()
            .collect();
        let p = plan(&c[..1], &old, &[], now, &zero);
        assert!(p.todo.is_empty());
        assert_eq!(p.reverted, 1);
        // une série écrite puis remise à vide est sautée, la suivante passe
        let s = vec![
            cand("s1", Kind::Series, 10, "/seedbox/media/Anime/A"),
            cand("s2", Kind::Series, 11, "/seedbox/media/Anime/B"),
        ];
        let ts: BTreeMap<_, _> = [tried("s1", now - 48 * 3600, "written")]
            .into_iter()
            .collect();
        let p = plan(&s, &ts, &[], now, &cfg());
        assert_eq!(p.todo.len(), 1);
        assert_eq!(p.todo[0].id, "s2");
    }

    #[test]
    fn an_unconfirmed_write_is_settled_once_its_language_is_read_back() {
        let mut r = OriginalLanguageRecord {
            at: 5,
            kind: "series".into(),
            name: "Frieren".into(),
            tmdb: 209867,
            outcome: "error".into(),
            new: "ja".into(),
            posted: true,
            children_unrated: vec!["aa".into()],
            children_rating: "FR-10".into(),
            detail: "écriture : operation timed out".into(),
            ..Default::default()
        };
        let done = settle(&r, " ja ").unwrap();
        assert_eq!(done.outcome, "written");
        // tout le reste est gardé, enfants compris (le retour arrière en a besoin)
        assert_eq!(
            (done.at, done.new.as_str(), done.children_unrated.len()),
            (5, "ja", 1)
        );
        assert!(done.posted);
        // langue toujours vide, ou une autre que la nôtre : rien
        assert!(settle(&r, "").is_none());
        assert!(settle(&r, "en").is_none());
        // requête jamais partie (connexion refusée) : rien à constater
        r.posted = false;
        assert!(settle(&r, "ja").is_none());
        // déjà écrite, ou autre issue : rien
        r.posted = true;
        r.outcome = "written".into();
        assert!(settle(&r, "ja").is_none());
        r.outcome = "unknown".into();
        assert!(settle(&r, "ja").is_none());
        // un refus net de Jellyfin n'a rien appliqué ; tout le reste a pu l'être
        for st in [401, 403, 404] {
            assert!(!write_maybe_applied(st), "{st}");
        }
        for st in [400, 409, 500, 502, 503] {
            assert!(write_maybe_applied(st), "{st}");
        }
    }

    #[test]
    fn a_series_whose_children_have_their_own_rating_is_left_alone() {
        let series = json!({"Id": "s1", "Type": "Series", "OfficialRating": "TV-14"});
        // saison et épisode sans classification : recevront TV-14, notés (ids compacts) ; même classification : rien
        let ok = vec![
            json!({"Id": "AAAA-bbbb", "Type": "Season", "IndexNumber": 1}),
            json!({"Id": "cccc", "Type": "Episode", "OfficialRating": "TV-14"}),
            json!({"Id": "dddd", "Type": "Episode", "OfficialRating": " "}),
            // verrouillée : Jellyfin ne touche pas sa classification, ni notée ni bloquante
            json!({"Id": "eeee", "Type": "Episode", "OfficialRating": "TV-MA", "LockedFields": ["OfficialRating"]}),
            json!({"Id": "ffff", "Type": "Episode", "LockedFields": ["OfficialRating"]}),
        ];
        assert_eq!(children_check(&series, &ok).unwrap(), ["aaaabbbb", "dddd"]);
        // un épisode avec sa propre classification : la série n'est pas écrite (elle serait remplacée)
        let mut own = ok.clone();
        own.push(
            json!({"Id": "gggg", "Type": "Episode", "ParentIndexNumber": 2, "IndexNumber": 5,
                        "OfficialRating": "TV-MA"}),
        );
        let why = children_check(&series, &own).unwrap_err();
        assert!(why.contains("S02E05") && why.contains("TV-MA"), "{why}");
        // classification personnalisée : écrite sur tous les enfants sans exception, verrou compris
        let mut custom = ok.clone();
        custom.push(
            json!({"Id": "hhhh", "Type": "Season", "IndexNumber": 3, "CustomRating": "12+",
                           "LockedFields": ["OfficialRating"]}),
        );
        let why = children_check(&series, &custom).unwrap_err();
        assert!(why.contains("saison 3") && why.contains("12+"), "{why}");
        // même classification personnalisée que la série : rien ne change
        let s2 =
            json!({"Id": "s1", "Type": "Series", "OfficialRating": "TV-14", "CustomRating": "12+"});
        let same = vec![
            json!({"Id": "iiii", "Type": "Episode", "OfficialRating": "TV-14", "CustomRating": "12+"}),
        ];
        assert_eq!(children_check(&s2, &same).unwrap(), Vec::<String>::new());
        // série sans classification : un enfant qui en a une la perdrait ; des enfants vides restent vides (rien à noter)
        let bare = json!({"Id": "s1", "Type": "Series"});
        assert!(children_check(
            &bare,
            &[json!({"Id": "jjjj", "Type": "Episode", "OfficialRating": "FR-12"})]
        )
        .is_err());
        assert_eq!(
            children_check(&bare, &[json!({"Id": "kkkk", "Type": "Episode"})]).unwrap(),
            Vec::<String>::new()
        );
        // aucun enfant : rien
        assert_eq!(children_check(&series, &[]).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn only_the_children_of_this_series_are_checked_and_noted() {
        // liste avec compte : enfants des deux copies, saison de la copie VPS cachée par le regroupement
        let with_user = vec![
            json!({"Id": "s-seedbox", "Type": "Season", "SeriesId": "seedbox0000"}),
            json!({"Id": "e-seedbox", "Type": "Episode", "SeriesId": "seedbox0000", "OfficialRating": "R"}),
            json!({"Id": "e-vps", "Type": "Episode", "SeriesId": "VPS0-0000"}),
        ];
        // liste sans compte : rien de regroupé, la saison VPS apparaît
        let without = vec![
            json!({"Id": "s-seedbox", "Type": "Season", "SeriesId": "seedbox0000"}),
            json!({"Id": "S-VPS", "Type": "Season", "SeriesId": "vps00000"}),
            json!({"Id": "e-vps", "Type": "Episode", "SeriesId": "vps00000"}),
        ];
        let own = own_children("vps0-0000", &[with_user, without]);
        let ids: Vec<&str> = own.iter().filter_map(|c| c["Id"].as_str()).collect();
        assert_eq!(ids, ["e-vps", "S-VPS"], "sans doublon, sans l'autre copie");
        // l'épisode de l'autre copie, à classification propre, ne bloque pas cette série
        let series = json!({"Id": "vps00000", "Type": "Series", "OfficialRating": "TV-MA"});
        assert_eq!(children_check(&series, &own).unwrap(), ["evps", "svps"]);
        assert!(own_children("x", &[]).is_empty());
    }

    #[test]
    fn series_hidden_by_jellyfin_grouping_are_found_through_their_episodes() {
        let listed = vec![
            json!({"Id": "11112222333344445555666677778888", "Type": "Series"}),
            json!({"Id": "aaaa", "Type": "Movie"}),
        ];
        let episodes = vec![
            json!({"Id": "e1", "SeriesId": "11112222333344445555666677778888"}),
            json!({"Id": "e2", "SeriesId": "9999AAAA-bbbb-cccc-dddd-eeeeffff0000"}),
            json!({"Id": "e3", "SeriesId": "9999aaaabbbbccccddddeeeeffff0000"}),
            json!({"Id": "e4"}),
            // un film n'est pas une série : son id ne compte pas comme « listé »
            json!({"Id": "e5", "SeriesId": "aaaa"}),
        ];
        assert_eq!(
            unlisted_series(&listed, &episodes),
            ["9999aaaabbbbccccddddeeeeffff0000", "aaaa"]
        );
        assert!(unlisted_series(&listed, &[]).is_empty());
    }
}
