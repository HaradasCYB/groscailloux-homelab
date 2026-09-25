//! Échappement HTML commun aux pages de homelabd et aux mails (il en existait cinq copies, dont une seule
//! échappait l'apostrophe : un attribut entre apostrophes pouvait être cassé par un nom de membre).

/// Échappe `& < > " '` pour un texte ou un attribut HTML.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_all_five() {
        assert_eq!(
            super::esc(r#"<a href="x">L'été & co</a>"#),
            "&lt;a href=&quot;x&quot;&gt;L&#39;été &amp; co&lt;/a&gt;"
        );
    }
}
