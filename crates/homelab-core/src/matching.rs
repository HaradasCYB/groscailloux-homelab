//! Choix de la bonne fiche parmi les résultats de `movie/lookup` / `series/lookup`.
//!
//! Prendre le premier résultat se trompe souvent : « Daybreak 2019 » donne d'abord la série de
//! 2012, « Fusion (2003) » (titre français de *The Core*) donne d'abord *Jimmy Neutron*. On
//! cherche donc un titre identique après normalisation (accents, ponctuation, année entre
//! parenthèses), en tenant compte des titres original et alternatifs (Radarr) et de l'année ;
//! à défaut, le premier résultat n'est accepté que s'il commence par le même mot (« approximatif »).

use serde_json::Value;

/// Titre et année extraits par le `parse` de l'Arr.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub year: i64,
    /// Saison du fichier (séries), 0 si inconnue.
    pub season: i64,
}

/// Résultat retenu et s'il a été choisi par repli.
#[derive(Debug, Clone)]
pub struct Match {
    pub hit: Value,
    pub fuzzy: bool,
}

/// `parse` de Sonarr → titre sans l'année finale (« Daybreak 2019 » → « Daybreak »), année, saison.
pub fn parsed_series(parse: &Value) -> Option<Parsed> {
    let pei = parse.get("parsedEpisodeInfo")?;
    let info = pei.get("seriesTitleInfo");
    let raw = info
        .and_then(|i| i.get("title"))
        .or_else(|| pei.get("seriesTitle"))
        .and_then(Value::as_str)?;
    let year = info
        .and_then(|i| i.get("year"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let season = pei.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0);
    let title = strip_trailing_year(raw, year);
    (!title.is_empty()).then_some(Parsed {
        title,
        year,
        season,
    })
}

/// `parse` de Radarr → premier titre et année.
pub fn parsed_movie(parse: &Value) -> Option<Parsed> {
    let info = parse.get("parsedMovieInfo")?;
    let title = info.pointer("/movieTitles/0").and_then(Value::as_str)?;
    let year = info.get("year").and_then(Value::as_i64).unwrap_or(0);
    (!title.trim().is_empty()).then(|| Parsed {
        title: title.trim().to_string(),
        year,
        season: 0,
    })
}

pub fn pick_movie(hits: &[Value], p: &Parsed) -> Option<Match> {
    let want = normalize(&p.title);
    let year_ok = |h: &Value| p.year == 0 || (hit_year(h) - p.year).abs() <= 1;
    let exact = hits.iter().find(|h| {
        year_ok(h)
            && h.get("tmdbId").and_then(Value::as_i64).unwrap_or(0) > 0
            && movie_titles(h).iter().any(|t| normalize(t) == want)
    });
    if let Some(h) = exact {
        return Some(Match {
            hit: h.clone(),
            fuzzy: false,
        });
    }
    fallback(hits, p, |h| {
        year_ok(h) && h.get("tmdbId").and_then(Value::as_i64).unwrap_or(0) > 0
    })
}

pub fn pick_series(hits: &[Value], p: &Parsed) -> Option<Match> {
    let want = normalize(&p.title);
    let usable = |h: &Value| {
        h.get("tvdbId").and_then(Value::as_i64).unwrap_or(0) > 0 && has_season(h, p.season)
    };
    let exact: Vec<&Value> = hits
        .iter()
        .filter(|h| usable(h))
        .filter(|h| series_titles(h).iter().any(|t| normalize(t) == want))
        .collect();
    let chosen = if p.year > 0 {
        exact
            .iter()
            .find(|h| hit_year(h) == p.year)
            .or_else(|| exact.iter().find(|h| (hit_year(h) - p.year).abs() <= 1))
            .copied()
    } else {
        // même titre, pas d'année dans le nom : la plus récente
        exact.iter().max_by_key(|h| hit_year(h)).copied()
    };
    if let Some(h) = chosen {
        return Some(Match {
            hit: h.clone(),
            fuzzy: false,
        });
    }
    fallback(hits, p, |h| {
        usable(h) && (p.year == 0 || (hit_year(h) - p.year).abs() <= 1)
    })
}

/// Premier résultat, s'il est acceptable et commence par le même mot que le titre cherché
/// (« Gomorra Les Origines » → « Gomorrah: The Origins »).
fn fallback(hits: &[Value], p: &Parsed, ok: impl Fn(&Value) -> bool) -> Option<Match> {
    let first_word = normalize(p.title.split_whitespace().next().unwrap_or(""));
    if first_word.chars().count() < 4 {
        return None;
    }
    let h = hits.first().filter(|h| ok(h))?;
    let t = normalize(h.get("title").and_then(Value::as_str).unwrap_or(""));
    t.starts_with(&first_word).then(|| Match {
        hit: h.clone(),
        fuzzy: true,
    })
}

/// Titres d'une fiche série : le principal et les titres alternatifs (une release peut porter le titre
/// d'origine — « Shingeki no Kyojin » pour *Attack on Titan* — ou une traduction).
fn series_titles(h: &Value) -> Vec<String> {
    let mut out: Vec<String> = h
        .get("title")
        .and_then(Value::as_str)
        .map(|t| vec![t.to_string()])
        .unwrap_or_default();
    if let Some(alts) = h.get("alternateTitles").and_then(Value::as_array) {
        out.extend(
            alts.iter()
                .filter_map(|a| a.get("title").and_then(Value::as_str).map(str::to_string)),
        );
    }
    out
}

fn movie_titles(h: &Value) -> Vec<String> {
    let mut out: Vec<String> = ["title", "originalTitle"]
        .iter()
        .filter_map(|k| h.get(*k).and_then(Value::as_str).map(str::to_string))
        .collect();
    if let Some(alts) = h.get("alternateTitles").and_then(Value::as_array) {
        out.extend(
            alts.iter()
                .filter_map(|a| a.get("title").and_then(Value::as_str).map(str::to_string)),
        );
    }
    out
}

fn has_season(h: &Value, season: i64) -> bool {
    if season <= 0 {
        return true;
    }
    h.get("seasons")
        .and_then(Value::as_array)
        .map(|s| {
            s.iter()
                .any(|x| x.get("seasonNumber").and_then(Value::as_i64) == Some(season))
        })
        .unwrap_or(true)
}

fn hit_year(h: &Value) -> i64 {
    h.get("year").and_then(Value::as_i64).unwrap_or(0)
}

fn strip_trailing_year(title: &str, year: i64) -> String {
    let t = title.trim();
    if year > 0 {
        if let Some(rest) = t.strip_suffix(&year.to_string()) {
            return rest
                .trim_end_matches([' ', '.', '(', '-'])
                .trim()
                .to_string();
        }
    }
    t.to_string()
}

/// Minuscules, sans accents ni ponctuation, « & » = « and », sans « (2019) » final.
pub fn normalize(s: &str) -> String {
    let mut t = s.trim();
    if t.ends_with(')') {
        if let Some(open) = t.rfind('(') {
            let inner = &t[open + 1..t.len() - 1];
            if inner.len() == 4 && inner.chars().all(|c| c.is_ascii_digit()) {
                t = t[..open].trim_end();
            }
        }
    }
    t.replace('&', "and")
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold)
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn series(title: &str, year: i64, tvdb: i64, seasons: &[i64]) -> Value {
        let s: Vec<Value> = seasons.iter().map(|n| json!({"seasonNumber": n})).collect();
        json!({"title": title, "year": year, "tvdbId": tvdb, "seasons": s})
    }

    #[test]
    fn normalizes_accents_punctuation_and_year() {
        assert_eq!(
            normalize("On a retrouvé la 7ème compagnie !"),
            "onaretrouvela7emecompagnie"
        );
        assert_eq!(
            normalize("IT: Welcome to Derry"),
            normalize("IT Welcome to Derry")
        );
        assert_eq!(normalize("Daybreak (2019)"), "daybreak");
        assert_eq!(normalize("Average Joe (FR)"), "averagejoefr");
        assert_eq!(normalize("Law & Order"), "lawandorder");
    }

    #[test]
    fn series_title_loses_its_trailing_year() {
        let p = parsed_series(&json!({"parsedEpisodeInfo": {
            "seriesTitle": "Daybreak 2019", "seasonNumber": 1,
            "seriesTitleInfo": {"title": "Daybreak 2019", "year": 2019}}}))
        .unwrap();
        assert_eq!(
            p,
            Parsed {
                title: "Daybreak".into(),
                year: 2019,
                season: 1
            }
        );
    }

    #[test]
    fn series_year_picks_the_right_daybreak() {
        let hits = vec![
            series("Daybreak", 2012, 1, &[1]),
            series("Daybreak (2019)", 2019, 354735, &[1]),
            series("Daybreak (1985)", 1985, 3, &[1]),
        ];
        let p = Parsed {
            title: "Daybreak".into(),
            year: 2019,
            season: 1,
        };
        let m = pick_series(&hits, &p).unwrap();
        assert_eq!(m.hit["tvdbId"], 354735);
        assert!(!m.fuzzy);
    }

    #[test]
    fn series_without_year_takes_most_recent_with_the_season() {
        let hits = vec![
            series("Average Joe", 2003, 1, &[1, 2, 3, 4]),
            series("Average Joe (2023)", 2023, 2, &[1, 2]),
            series("Average Joe (2012)", 2012, 3, &[1, 2, 3]),
        ];
        let p = Parsed {
            title: "Average Joe".into(),
            year: 0,
            season: 3,
        };
        assert_eq!(pick_series(&hits, &p).unwrap().hit["tvdbId"], 3);
        let p2 = Parsed { season: 2, ..p };
        assert_eq!(pick_series(&hits, &p2).unwrap().hit["tvdbId"], 2);
    }

    #[test]
    fn series_fallback_needs_the_same_first_word() {
        let hits = vec![
            series("Gomorrah: The Origins", 2026, 10, &[1]),
            series("Pokémon: Origins", 2013, 11, &[1]),
        ];
        let p = Parsed {
            title: "Gomorra Les Origines".into(),
            year: 0,
            season: 1,
        };
        let m = pick_series(&hits, &p).unwrap();
        assert_eq!(m.hit["tvdbId"], 10);
        assert!(m.fuzzy);
        let other = Parsed {
            title: "Totally Unrelated".into(),
            year: 0,
            season: 1,
        };
        assert!(pick_series(&hits, &other).is_none());
    }

    #[test]
    fn movie_matches_french_and_alternate_titles() {
        let hits = vec![
            json!({"title": "Jimmy Neutron: Operation: Rescue Jet Fusion", "year": 2003, "tmdbId": 1, "alternateTitles": []}),
            json!({"title": "The Core", "year": 2003, "tmdbId": 9341, "alternateTitles": [{"title": "Fusion"}]}),
        ];
        let p = Parsed {
            title: "Fusion".into(),
            year: 2003,
            season: 0,
        };
        let m = pick_movie(&hits, &p).unwrap();
        assert_eq!(m.hit["tmdbId"], 9341);
        assert!(!m.fuzzy);

        let hits = vec![
            json!({"title": "The Seventh Company Has Been Found", "year": 1975, "tmdbId": 401306,
                               "originalTitle": "On a retrouvé la 7ème compagnie !"}),
        ];
        let p = Parsed {
            title: "On a retrouve la 7eme compagnie".into(),
            year: 1975,
            season: 0,
        };
        assert_eq!(pick_movie(&hits, &p).unwrap().hit["tmdbId"], 401306);
    }

    #[test]
    fn movie_year_must_be_within_one() {
        let hits = vec![json!({"title": "Fusion", "year": 2010, "tmdbId": 5})];
        let p = Parsed {
            title: "Fusion".into(),
            year: 2003,
            season: 0,
        };
        assert!(pick_movie(&hits, &p).is_none());
        let p = Parsed { year: 2011, ..p };
        assert_eq!(pick_movie(&hits, &p).unwrap().hit["tmdbId"], 5);
    }

    #[test]
    fn series_matched_by_alternate_title() {
        // la release porte le titre d'origine, la fiche s'appelle autrement
        let hits = vec![json!({
            "title": "Attack on Titan", "tvdbId": 267440, "year": 2013,
            "alternateTitles": [{"title": "Shingeki no Kyojin"}],
            "seasons": [{"seasonNumber": 2}]
        })];
        let p = Parsed {
            title: "Shingeki no Kyojin".into(),
            year: 0,
            season: 2,
        };
        assert_eq!(
            pick_series(&hits, &p).map(|m| m.hit["tvdbId"].as_i64().unwrap()),
            Some(267440)
        );
    }
}
