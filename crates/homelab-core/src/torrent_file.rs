//! Lecture du contenu d'un fichier `.torrent`, sans rien ajouter à qBittorrent ni annoncer au tracker.
//!
//! Sert à savoir **ce qu'il y a dans un pack avant de le prendre** : `series_search` télécharge déjà les
//! octets du `.torrent` par Prowlarr (`ProwlarrClient::download`), il suffit de les lire. C'est ce qui
//! permet de vérifier qu'un cours d'animé publié sous son propre titre (« Thousand-Year Blood War S03 »)
//! couvre exactement les épisodes qui manquent, avant d'engager quoi que ce soit.
//!
//! Décodeur bencode réduit au strict nécessaire (entiers, chaînes, listes, dictionnaires) : on ne lit que
//! `info.name`, `info.length` et `info.files[].{path,length}`.

use anyhow::{bail, Context, Result};

/// Un fichier annoncé par le `.torrent` : chemin relatif à la racine du torrent, et taille.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentEntry {
    pub path: String,
    pub length: i64,
}

/// Valeur bencode, réduite à ce qu'on exploite.
enum Item {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Item>),
    Dict(Vec<(Vec<u8>, Item)>),
}

impl Item {
    fn get(&self, key: &[u8]) -> Option<&Item> {
        match self {
            Item::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_int(&self) -> Option<i64> {
        match self {
            Item::Int(i) => Some(*i),
            _ => None,
        }
    }
    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Item::Bytes(b) => Some(b),
            _ => None,
        }
    }
    fn as_list(&self) -> Option<&[Item]> {
        match self {
            Item::List(l) => Some(l),
            _ => None,
        }
    }
}

/// Un `.torrent` fait quelques dizaines de kio ; au-delà, ce n'en est pas un.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Profondeur d'imbrication : un `.torrent` sain n'en demande que 3 ou 4.
const MAX_DEPTH: usize = 32;

fn decode(b: &[u8], i: &mut usize, depth: usize) -> Result<Item> {
    if depth > MAX_DEPTH {
        bail!("bencode trop imbriqué");
    }
    let c = *b.get(*i).context("bencode tronqué")?;
    match c {
        b'i' => {
            let end = b[*i..]
                .iter()
                .position(|&x| x == b'e')
                .context("entier bencode sans fin")?
                + *i;
            let n: i64 = std::str::from_utf8(&b[*i + 1..end])?
                .parse()
                .context("entier bencode illisible")?;
            *i = end + 1;
            Ok(Item::Int(n))
        }
        b'l' | b'd' => {
            let dict = c == b'd';
            *i += 1;
            let mut list = Vec::new();
            let mut pairs = Vec::new();
            while *b.get(*i).context("liste bencode sans fin")? != b'e' {
                if dict {
                    let Item::Bytes(k) = decode(b, i, depth + 1)? else {
                        bail!("clé de dictionnaire non textuelle");
                    };
                    pairs.push((k, decode(b, i, depth + 1)?));
                } else {
                    list.push(decode(b, i, depth + 1)?);
                }
            }
            *i += 1;
            Ok(if dict {
                Item::Dict(pairs)
            } else {
                Item::List(list)
            })
        }
        b'0'..=b'9' => {
            let colon = b[*i..]
                .iter()
                .position(|&x| x == b':')
                .context("chaîne bencode sans « : »")?
                + *i;
            let n: usize = std::str::from_utf8(&b[*i..colon])?
                .parse()
                .context("longueur de chaîne illisible")?;
            let start = colon + 1;
            let end = start
                .checked_add(n)
                .context("longueur de chaîne aberrante")?;
            if end > b.len() {
                bail!("chaîne bencode tronquée");
            }
            *i = end;
            Ok(Item::Bytes(b[start..end].to_vec()))
        }
        other => bail!("octet bencode inattendu : {other:#04x}"),
    }
}

/// Fichiers annoncés par un `.torrent`. Un torrent mono-fichier renvoie une seule entrée (son `name`).
pub fn files(raw: &[u8]) -> Result<Vec<TorrentEntry>> {
    if raw.len() > MAX_BYTES {
        bail!("fichier .torrent anormalement gros ({} octets)", raw.len());
    }
    let mut i = 0;
    let meta = decode(raw, &mut i, 0)?;
    let info = meta.get(b"info").context("pas de section « info »")?;
    let name = info
        .get(b"name")
        .and_then(Item::as_bytes)
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    // multi-fichiers
    if let Some(list) = info.get(b"files").and_then(Item::as_list) {
        let mut out = Vec::new();
        for f in list {
            let Some(parts) = f.get(b"path").and_then(Item::as_list) else {
                continue;
            };
            let path: Vec<String> = parts
                .iter()
                .filter_map(Item::as_bytes)
                .map(|b| String::from_utf8_lossy(b).to_string())
                .collect();
            if path.is_empty() {
                continue;
            }
            out.push(TorrentEntry {
                path: path.join("/"),
                length: f.get(b"length").and_then(Item::as_int).unwrap_or(0),
            });
        }
        return Ok(out);
    }
    // mono-fichier
    if name.is_empty() {
        bail!("torrent sans nom ni liste de fichiers");
    }
    Ok(vec![TorrentEntry {
        path: name,
        length: info.get(b"length").and_then(Item::as_int).unwrap_or(0),
    }])
}

/// Vidéos du torrent, extraits (`sample`) exclus — même règle que `torrent_import::video_files`
/// (`classify::is_video` + rejet du mot « sample »).
pub fn video_paths(entries: &[TorrentEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| e.path.as_str())
        .filter(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            crate::classify::is_video(base)
                && !p
                    .to_ascii_lowercase()
                    .split(['/', '.', '-', '_', ' '])
                    .any(|w| w == "sample")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construit les octets d'un `.torrent` multi-fichiers.
    fn multi(files: &[(&str, i64)]) -> Vec<u8> {
        let mut s = String::from("d4:infod5:filesl");
        for (path, len) in files {
            let parts: Vec<String> = path
                .split('/')
                .map(|p| format!("{}:{p}", p.len()))
                .collect();
            s.push_str(&format!("d6:lengthi{len}e4:pathl{}ee", parts.join("")));
        }
        s.push_str("e4:name7:Un.Packee");
        s.into_bytes()
    }

    #[test]
    fn reads_a_multi_file_torrent() {
        let raw = multi(&[
            ("BLEACH.S03E01.mkv", 1_000),
            ("Extras/note.nfo", 10),
            ("BLEACH.S03E02.mkv", 2_000),
        ]);
        let f = files(&raw).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].path, "BLEACH.S03E01.mkv");
        assert_eq!(f[0].length, 1_000);
        assert_eq!(f[1].path, "Extras/note.nfo");
        let vids = video_paths(&f);
        assert_eq!(vids, vec!["BLEACH.S03E01.mkv", "BLEACH.S03E02.mkv"]);
    }

    #[test]
    fn reads_a_single_file_torrent() {
        let raw = b"d4:infod6:lengthi4242e4:name11:Un.Film.mkvee".to_vec();
        let f = files(&raw).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "Un.Film.mkv");
        assert_eq!(f[0].length, 4242);
    }

    #[test]
    fn a_sample_is_not_a_video_to_take() {
        let raw = multi(&[("Show.S01E01.mkv", 9), ("sample/Show-sample.mkv", 1)]);
        let f = files(&raw).unwrap();
        assert_eq!(video_paths(&f), vec!["Show.S01E01.mkv"]);
    }

    /// Un vrai `.torrent`, quand on en désigne un : `HOMELAB_TEST_TORRENT=/chemin cargo test`.
    /// Sans la variable, le test ne fait rien (rien à embarquer dans le dépôt).
    #[test]
    fn reads_a_real_torrent_when_one_is_given() {
        let Ok(path) = std::env::var("HOMELAB_TEST_TORRENT") else {
            return;
        };
        let Ok(raw) = std::fs::read(path) else {
            return;
        };
        let f = files(&raw).unwrap();
        let v = video_paths(&f);
        assert!(!f.is_empty(), "un .torrent annonce au moins un fichier");
        assert!(f.iter().all(|e| !e.path.is_empty()));
        assert!(v.len() <= f.len());
    }

    #[test]
    fn rubbish_is_refused_not_panicked() {
        assert!(files(b"pas du bencode").is_err());
        assert!(files(b"d4:infod6:lengthi1e").is_err(), "tronqu\u{e9}");
        assert!(files(b"").is_err());
        // longueur de chaîne qui dépasse le tampon
        assert!(files(b"d4:infod4:name99:abcee").is_err());
    }
}
