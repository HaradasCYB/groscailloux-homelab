//! Contrôle strict de `diun/images.yml` (revue Kaizen du 2026-10-07, `homelabctl check`).
//!
//! Du 18/09 au 07/10, diun n'a surveillé **aucune** image : le retrait de Jackett et de FlareSolverr (commit
//! 297d8df) avait laissé trois clés orphelines par entrée, et le décodeur YAML de diun refuse tout le fichier à la
//! première clé en double (« mapping key "watch_repo" already defined »). Chaque matin « No image found », aucun
//! mail de mise à jour, personne ne l'a vu en 20 jours.
//!
//! Le fichier n'a qu'une forme : une liste d'entrées `- name: image:tag` suivies de leurs clés à deux espaces
//! d'indentation (`watch_repo`, `max_tags`, `include_tags`…). Pas de bibliothèque YAML dans le dépôt : ce contrôle
//! lit exactement cette forme, refuse tout ce qui en sort et refait ce que fait diun (clé en double ⇒ rejet).
//! Il vérifie aussi que chaque image du compose a son entrée (une image oubliée n'est jamais surveillée).

use std::collections::BTreeMap;

/// Une entrée du fichier de diun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `image:tag` (clé `name`), vide si l'entrée n'en a pas.
    pub name: String,
    /// Ligne (à partir de 1) du `- `.
    pub line: usize,
}

/// Ce que le contrôle a trouvé : `errors` rendent diun aveugle (ou laissent une image sans surveillance),
/// `warnings` sont des restes sans effet sur diun.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Findings {
    pub entries: Vec<Entry>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn key_of(text: &str) -> Option<&str> {
    let (k, _) = text.split_once(':')?;
    let k = k.trim_end();
    (!k.is_empty() && !k.contains([' ', '#', '\'', '"', '[', '{'])).then_some(k)
}

fn value_of(text: &str) -> String {
    let v = text.split_once(':').map(|(_, v)| v).unwrap_or("").trim();
    // commentaire en fin de ligne (« # » précédé d'une espace), puis guillemets
    let v = v.split(" #").next().unwrap_or("").trim();
    v.trim_matches(['"', '\'']).to_string()
}

/// Lit `images.yml`. Les erreurs de structure sont dans `Findings::errors` (une par problème, avec la ligne).
pub fn check_images_file(text: &str) -> Findings {
    let mut out = Findings::default();
    // entrée en cours : (ligne, clés vues → ligne, nom)
    let mut current: Option<(usize, BTreeMap<String, usize>, String)> = None;
    let mut names: BTreeMap<String, usize> = BTreeMap::new();

    fn close(
        out: &mut Findings,
        names: &mut BTreeMap<String, usize>,
        cur: Option<(usize, BTreeMap<String, usize>, String)>,
    ) {
        let Some((line, _, name)) = cur else { return };
        if name.is_empty() {
            out.errors
                .push(format!("ligne {line} : entrée sans clé `name`"));
        } else if let Some(first) = names.get(&name) {
            out.errors.push(format!(
                "ligne {line} : image « {name} » déjà déclarée ligne {first}"
            ));
        } else {
            names.insert(name.clone(), line);
        }
        out.entries.push(Entry { name, line });
    }

    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || line == "---" || line == "..." {
            continue;
        }
        if raw.contains('\t') {
            out.errors.push(format!(
                "ligne {n} : tabulation (YAML n'en accepte pas en indentation)"
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            close(&mut out, &mut names, current.take());
            let mut keys = BTreeMap::new();
            let mut name = String::new();
            match key_of(rest.trim_start()) {
                Some(k) => {
                    keys.insert(k.to_string(), n);
                    if k == "name" {
                        name = value_of(rest.trim_start());
                    }
                }
                None => out.errors.push(format!(
                    "ligne {n} : entrée illisible (« - clé: valeur » attendu)"
                )),
            }
            current = Some((n, keys, name));
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            out.errors.push(format!(
                "ligne {n} : contenu inattendu hors d'une entrée « - name: … »"
            ));
            continue;
        }
        let Some((_, keys, name)) = current.as_mut() else {
            out.errors.push(format!(
                "ligne {n} : clé orpheline avant la première entrée (reste d'une entrée retirée ?)"
            ));
            continue;
        };
        if indent > 2 {
            continue; // suite d'une valeur (liste ou texte sur plusieurs lignes)
        }
        match key_of(trimmed) {
            Some(k) => {
                if let Some(first) = keys.insert(k.to_string(), n) {
                    out.errors.push(format!(
                        "ligne {n} : clé « {k} » en double (déjà ligne {first}) — diun rejette tout le fichier \
                         (« mapping key already defined »), le plus souvent une entrée retirée à moitié"
                    ));
                }
                if k == "name" {
                    *name = value_of(trimmed);
                }
            }
            None => out
                .errors
                .push(format!("ligne {n} : ligne illisible « {trimmed} »")),
        }
    }
    close(&mut out, &mut names, current.take());
    if out.entries.is_empty() && out.errors.is_empty() {
        out.errors
            .push("aucune entrée : diun n'aurait rien à surveiller (« No image found »)".into());
    }
    out
}

/// Images déclarées par le compose (`image: repo:tag@sha256:…` → `repo:tag`), sans doublon. Une image construite
/// par interpolation (`${VAR}`) ne peut pas être comparée : renvoyée à part.
pub fn compose_images(compose: &str) -> (Vec<String>, Vec<String>) {
    let mut images: Vec<String> = Vec::new();
    let mut dynamic = Vec::new();
    for raw in compose.lines() {
        let t = raw.trim_start();
        let Some(rest) = t.strip_prefix("image:") else {
            continue;
        };
        let v = value_of(&format!("image:{rest}"));
        if v.is_empty() {
            continue;
        }
        let v = v.split('@').next().unwrap_or("").to_string();
        if v.contains("${") {
            dynamic.push(v);
        } else if !images.contains(&v) {
            images.push(v);
        }
    }
    (images, dynamic)
}

/// Contrôle complet : structure du fichier de diun, puis chaque image du compose a son entrée.
pub fn check(images_yml: &str, compose: &str) -> Findings {
    let mut f = check_images_file(images_yml);
    let (images, dynamic) = compose_images(compose);
    for img in &images {
        if !f.entries.iter().any(|e| &e.name == img) {
            f.errors.push(format!(
                "image du compose sans entrée dans diun/images.yml : {img} (jamais surveillée)"
            ));
        }
    }
    for e in &f.entries {
        if !e.name.is_empty() && !images.contains(&e.name) && dynamic.is_empty() {
            f.warnings.push(format!(
                "ligne {} : « {} » n'est utilisée par aucun service du compose (ou son tag a changé : mettre \
                 diun/images.yml en cohérence)",
                e.line, e.name
            ));
        }
    }
    for d in dynamic {
        f.warnings.push(format!(
            "image du compose construite par variable, non comparée : {d}"
        ));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# commentaire
- name: lscr.io/linuxserver/sonarr:4.0.17.2952-ls309
  watch_repo: true
  max_tags: 3
  include_tags: ['^\\d+\\.\\d+\\.\\d+\\.\\d+-ls\\d+$']

# --- tags mobiles
- name: qmcgaw/gluetun:latest
- name: mysql:8.0
";

    #[test]
    fn a_clean_file_has_no_finding() {
        let f = check_images_file(GOOD);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.entries.len(), 3);
        assert_eq!(
            f.entries[0].name,
            "lscr.io/linuxserver/sonarr:4.0.17.2952-ls309"
        );
        assert_eq!(f.entries[1].line, 8);
    }

    #[test]
    fn the_diun_blind_spot_of_september_is_caught() {
        // le fichier réel du 18/09 : les trois clés d'une entrée retirée (Jackett) restées sous l'entrée voisine
        let broken = "\
- name: lscr.io/linuxserver/radarr:6.1.1.10360-ls300
  watch_repo: true
  max_tags: 3
  include_tags: ['^\\d+$']
  watch_repo: true
  max_tags: 3
  include_tags: ['^v\\d+$']
- name: lscr.io/linuxserver/prowlarr:2.3.5.5327-ls144
  watch_repo: true
";
        let f = check_images_file(broken);
        assert_eq!(f.errors.len(), 3, "{:?}", f.errors);
        assert!(
            f.errors[0].starts_with("ligne 5 : clé « watch_repo » en double (déjà ligne 2)"),
            "{}",
            f.errors[0]
        );
        assert!(f.errors[1].contains("max_tags") && f.errors[2].contains("include_tags"));
    }

    #[test]
    fn orphan_keys_before_any_entry_and_stray_lines_are_refused() {
        let f = check_images_file("  watch_repo: true\n- name: a:1\nn'importe quoi\n");
        assert_eq!(f.errors.len(), 2, "{:?}", f.errors);
        assert!(f.errors[0].contains("ligne 1") && f.errors[0].contains("orpheline"));
        assert!(f.errors[1].contains("ligne 3") && f.errors[1].contains("hors d'une entrée"));
    }

    #[test]
    fn an_entry_without_name_or_with_a_repeated_name_is_refused() {
        let f = check_images_file("- watch_repo: true\n  max_tags: 3\n- name: a:1\n- name: a:1\n");
        assert_eq!(f.errors.len(), 2, "{:?}", f.errors);
        assert!(f.errors[0].contains("sans clé `name`"));
        assert!(f.errors[1].contains("déjà déclarée ligne 3"));
    }

    #[test]
    fn tabs_values_with_quotes_and_comments_are_handled() {
        let f = check_images_file("- name: \"a:1\"   # note\n  max_tags: 3\n");
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.entries[0].name, "a:1");
        let f = check_images_file("- name: a:1\n\tmax_tags: 3\n");
        assert!(f.errors[0].contains("tabulation"));
    }

    #[test]
    fn an_empty_file_is_an_error_because_diun_would_watch_nothing() {
        let f = check_images_file("# rien\n\n");
        assert_eq!(f.errors.len(), 1);
        assert!(f.errors[0].contains("No image found"));
    }

    const COMPOSE: &str = "\
services:
  sonarr:
    image: lscr.io/linuxserver/sonarr:4.0.17.2952-ls309@sha256:3580aec3802c915f0f819a88d5099abce61734b925732b8393d176b5dc561020
  qbittorrent:
    image: 'qmcgaw/gluetun:latest@sha256:bc38477325577b747de8b5667daba6947064f11a96f1b27109c2a554f686598f'
  qbittorrent-direct:
    image: qmcgaw/gluetun:latest@sha256:bc38477325577b747de8b5667daba6947064f11a96f1b27109c2a554f686598f
  # image: commentaire:1
  db:
    image: mysql:8.0@sha256:d0304ed9fdb64a3f6c7ad11a5fb4f13abfc10e6dfa3f288d652e7320c34df7f9
";

    #[test]
    fn compose_images_lose_their_digest_and_are_deduplicated() {
        let (imgs, dynamic) = compose_images(COMPOSE);
        assert_eq!(
            imgs,
            vec![
                "lscr.io/linuxserver/sonarr:4.0.17.2952-ls309",
                "qmcgaw/gluetun:latest",
                "mysql:8.0"
            ]
        );
        assert!(dynamic.is_empty());
        let (_, dynamic) = compose_images("    image: foo/bar:${TAG}\n");
        assert_eq!(dynamic, vec!["foo/bar:${TAG}"]);
    }

    #[test]
    fn every_compose_image_needs_its_entry() {
        let f = check(GOOD, COMPOSE);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        // une image du compose passée dans un nouveau tag sans toucher à diun : jamais surveillée
        let f = check(
            GOOD,
            &COMPOSE.replace("4.0.17.2952-ls309", "4.0.19.2979-ls324"),
        );
        assert_eq!(f.errors.len(), 1, "{:?}", f.errors);
        assert!(
            f.errors[0].contains("4.0.19.2979-ls324") && f.errors[0].contains("jamais surveillée")
        );
        // et l'ancienne entrée n'est plus utilisée : simple avertissement
        assert_eq!(f.warnings.len(), 1, "{:?}", f.warnings);
        assert!(f.warnings[0].contains("4.0.17.2952-ls309"));
    }

    #[test]
    fn variable_images_are_not_compared_and_silence_the_unused_warning() {
        let compose = format!("{COMPOSE}  x:\n    image: foo/bar:${{TAG}}\n");
        let f = check(GOOD, &compose);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.warnings.len(), 1);
        assert!(f.warnings[0].contains("foo/bar:${TAG}"));
    }
}
