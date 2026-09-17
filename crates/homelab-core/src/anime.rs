//! Classement « anime » d'une œuvre d'après sa fiche TMDB (vue par Jellyseerr).
//!
//! Le type « anime » de Sonarr n'est pas une référence : le 2026-09-17, *Bleach*, *Re:Zero*, *Fullmetal
//! Alchemist: Brotherhood* y étaient en « standard ». La langue d'origine seule non plus : *Deep Revenge*
//! et *The Last 10 Years* sont japonais mais en prises de vues réelles. Règle retenue : **animation
//! japonaise** = genre TMDB Animation **et** origine japonaise (langue `ja`, ou pays d'origine / de
//! production `JP`), ou genre Animation avec le mot-clé TMDB `anime`. L'animation chinoise, coréenne ou
//! occidentale reste dans Séries/Films.

use serde_json::Value;

/// Genre TMDB « Animation ».
pub const GENRE_ANIMATION: i64 = 16;
/// Mot-clé TMDB « anime ».
pub const KEYWORD_ANIME: i64 = 210024;
/// Tags posés à la main dans les Arrs pour forcer le classement.
pub const TAG_ANIME: &str = "anime";
pub const TAG_NOT_ANIME: &str = "pas-anime";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Anime,
    NotAnime,
    /// Fiche TMDB absente ou vide : on ne décide pas.
    Unknown,
}

fn ids(details: &Value, key: &str) -> Vec<i64> {
    details
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("id").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// Pays d'origine (séries) et de production (films et séries), codes ISO.
fn countries(details: &Value) -> Vec<String> {
    let mut out: Vec<String> = details
        .get("originCountry")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(a) = details.get("productionCountries").and_then(Value::as_array) {
        out.extend(
            a.iter()
                .filter_map(|c| c.get("iso_3166_1").and_then(Value::as_str))
                .map(str::to_string),
        );
    }
    out
}

/// Classement d'une fiche TMDB (réponse Jellyseerr `tv/{id}` ou `movie/{id}`).
pub fn classify(details: &Value) -> Class {
    let genres = ids(details, "genres");
    if genres.is_empty() && details.get("originalLanguage").is_none() {
        return Class::Unknown;
    }
    if !genres.contains(&GENRE_ANIMATION) {
        return Class::NotAnime;
    }
    let japanese_language = details.get("originalLanguage").and_then(Value::as_str) == Some("ja");
    let cs = countries(details);
    // co-production : le Japon compte s'il est le seul pays, ou si la langue d'origine est le japonais
    let japanese_origin =
        cs.iter().any(|c| c == "JP") && (cs.iter().all(|c| c == "JP") || japanese_language);
    let keyword = ids(details, "keywords").contains(&KEYWORD_ANIME);
    let other_language = details
        .get("originalLanguage")
        .and_then(Value::as_str)
        .is_some_and(|l| !l.is_empty() && l != "ja");
    if japanese_language || japanese_origin || (keyword && !other_language) {
        Class::Anime
    } else {
        Class::NotAnime
    }
}

/// Choix manuel dans l'Arr, prioritaire sur TMDB : tag `anime` ⇒ Anime, tag `pas-anime` ⇒ NotAnime.
/// `labels` : libellés des tags de la fiche.
pub fn manual_override(labels: &[String]) -> Option<Class> {
    let has = |t: &str| labels.iter().any(|l| l.eq_ignore_ascii_case(t));
    if has(TAG_NOT_ANIME) {
        Some(Class::NotAnime)
    } else if has(TAG_ANIME) {
        Some(Class::Anime)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmdb(genres: &[i64], lang: &str, origin: &[&str], prod: &[&str], keywords: &[i64]) -> Value {
        json!({
            "genres": genres.iter().map(|g| json!({"id": g})).collect::<Vec<_>>(),
            "originalLanguage": lang,
            "originCountry": origin,
            "productionCountries": prod.iter().map(|c| json!({"iso_3166_1": c})).collect::<Vec<_>>(),
            "keywords": keywords.iter().map(|k| json!({"id": k})).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn japanese_animation_is_anime() {
        // Attack on Titan, Bleach (en « standard » dans Sonarr), Souvenirs de Marnie (film)
        assert_eq!(
            classify(&tmdb(&[16, 10765, 10759], "ja", &["JP"], &["JP"], &[])),
            Class::Anime
        );
        assert_eq!(
            classify(&tmdb(&[10759, 16, 10765], "ja", &["JP"], &["JP"], &[])),
            Class::Anime
        );
        assert_eq!(
            classify(&tmdb(&[16, 18, 10751], "ja", &[], &["JP"], &[])),
            Class::Anime
        );
        // animation japonaise sans langue renseignée mais d'origine japonaise
        assert_eq!(
            classify(&tmdb(&[16], "", &["JP"], &["JP"], &[])),
            Class::Anime
        );
    }

    #[test]
    fn live_action_and_other_animation_are_not() {
        // Deep Revenge : japonais, prises de vues réelles
        assert_eq!(
            classify(&tmdb(&[18], "ja", &["JP", "FR"], &["JP", "FR"], &[])),
            Class::NotAnime
        );
        // Star Wars Rebels, DuckTales : animation américaine
        assert_eq!(
            classify(&tmdb(&[16, 10765], "en", &["US"], &["US"], &[])),
            Class::NotAnime
        );
        // One Hundred Thousand Years of Qi Refining : animation chinoise
        assert_eq!(
            classify(&tmdb(&[16, 10759], "zh", &["CN"], &["CN"], &[])),
            Class::NotAnime
        );
        // Bleach (S) Abridged : parodie anglophone, même avec le mot-clé anime
        assert_eq!(
            classify(&tmdb(&[16, 35], "en", &["US"], &["US"], &[KEYWORD_ANIME])),
            Class::NotAnime
        );
        // co-production américano-japonaise en anglais
        assert_eq!(
            classify(&tmdb(&[16], "en", &["US", "JP"], &["US", "JP"], &[])),
            Class::NotAnime
        );
    }

    #[test]
    fn keyword_alone_counts_when_language_is_unknown() {
        assert_eq!(
            classify(&tmdb(&[16], "", &[], &[], &[KEYWORD_ANIME])),
            Class::Anime
        );
    }

    #[test]
    fn empty_details_are_unknown() {
        assert_eq!(classify(&json!({})), Class::Unknown);
    }

    #[test]
    fn manual_tags_win() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(manual_override(&l(&["anime"])), Some(Class::Anime));
        assert_eq!(
            manual_override(&l(&["anime", "pas-anime"])),
            Some(Class::NotAnime)
        );
        assert_eq!(manual_override(&l(&["4k"])), None);
    }
}
