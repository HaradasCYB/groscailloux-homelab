use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Series,
    Movie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Rar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Video,
    Archive(ArchiveKind),
    /// Fichiers temporaires de qBittorrent/pyLoad ou extensions non gérées.
    Ignore,
}

fn series_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)S\d{1,2}E\d{1,2}|\d{1,2}x\d{1,2}|Season|Saison|Complete").unwrap()
    })
}

/// Même heuristique que l'ancien `auto-import.sh` : un marqueur d'épisode/saison
/// dans le nom ⇒ série, sinon film.
pub fn classify(label: &str) -> MediaKind {
    if series_re().is_match(label) {
        MediaKind::Series
    } else {
        MediaKind::Movie
    }
}

pub fn file_kind(name: &str) -> FileKind {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".!qb") || lower.ends_with(".part") || lower.starts_with('.') {
        return FileKind::Ignore;
    }
    if lower.ends_with(".mkv") || lower.ends_with(".mp4") || lower.ends_with(".avi") {
        return FileKind::Video;
    }
    if lower.ends_with(".zip") {
        return FileKind::Archive(ArchiveKind::Zip);
    }
    if lower.ends_with(".rar") {
        return FileKind::Archive(ArchiveKind::Rar);
    }
    FileKind::Ignore
}

pub fn is_video(name: &str) -> bool {
    file_kind(name) == FileKind::Video
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_and_seasons_are_series() {
        for s in [
            "Show.S01E02.1080p.mkv",
            "show.s1e2.mkv",
            "Show 3x07.mkv",
            "Show.Season.2.Complete",
            "Serie.Saison.1.FRENCH",
        ] {
            assert_eq!(classify(s), MediaKind::Series, "{s}");
        }
    }

    #[test]
    fn plain_titles_are_movies() {
        assert_eq!(
            classify("Dune.Part.Two.2024.MULTi.1080p.mkv"),
            MediaKind::Movie
        );
        assert_eq!(classify("Un.Juge.Implacable.1975.mkv"), MediaKind::Movie);
    }

    #[test]
    fn file_kinds() {
        assert_eq!(file_kind("a.MKV"), FileKind::Video);
        assert_eq!(file_kind("a.mp4"), FileKind::Video);
        assert_eq!(file_kind("a.zip"), FileKind::Archive(ArchiveKind::Zip));
        assert_eq!(file_kind("a.RAR"), FileKind::Archive(ArchiveKind::Rar));
        assert_eq!(file_kind("a.mkv.!qB"), FileKind::Ignore);
        assert_eq!(file_kind("a.part"), FileKind::Ignore);
        assert_eq!(file_kind("a.nfo"), FileKind::Ignore);
    }
}
