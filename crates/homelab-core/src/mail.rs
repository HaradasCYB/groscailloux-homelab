//! Envoi de mails via `curl smtps://` (même mécanique que l'ancien script : éprouvée avec Gmail, aucune
//! dépendance TLS côté Rust). Le message MIME est construit ici, avec ce que les filtres anti-spam attendent
//! d'un vrai mail (2026-09-19, le mail de bienvenue finissait en spam) : en-têtes `Date` et `Message-ID`,
//! sujet et noms encodés RFC 2047, corps UTF-8 en quoted-printable avec les vrais accents, et pour les mails
//! aux membres une version texte **et** une version HTML (`multipart/alternative`). Jamais d'identifiant ni de
//! mot de passe dans un mail : ils passent par un lien de bienvenue (`welcome.rs`).
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use rand::Rng;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Smtp;

/// Un mail à envoyer : le texte est obligatoire, le HTML optionnel (même contenu, mis en forme).
pub struct Outgoing<'a> {
    pub to_name: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: Option<&'a str>,
    pub reply_to: Option<&'a str>,
}

/// Mail texte (alertes admin, récapitulatifs du tchat).
pub async fn send_plain(
    smtp: &Smtp,
    to_name: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    send(
        smtp,
        &Outgoing {
            to_name,
            to,
            subject,
            text: body,
            html: None,
            reply_to: None,
        },
    )
    .await
}

/// Mail texte + HTML (membres) ; `reply_to` = adresse de contact de l'admin si elle existe.
pub async fn send_html(
    smtp: &Smtp,
    to_name: &str,
    to: &str,
    subject: &str,
    text: &str,
    html: &str,
    reply_to: Option<&str>,
) -> Result<()> {
    send(
        smtp,
        &Outgoing {
            to_name,
            to,
            subject,
            text,
            html: Some(html),
            reply_to,
        },
    )
    .await
}

pub async fn send(smtp: &Smtp, msg: &Outgoing<'_>) -> Result<()> {
    let date = chrono::Local::now().to_rfc2822();
    let id = message_id(&smtp.from);
    let boundary = format!("=_gc_{}", random_hex(12));
    let message = build_message(smtp, msg, &date, &id, &boundary);
    let url = format!("smtps://{}:{}", smtp.host, smtp.port);
    let userpass = format!("{}:{}", smtp.user, smtp.pass.expose());
    let mut child = Command::new("curl")
        .args([
            "-fsS",
            "--url",
            &url,
            "--ssl-reqd",
            "--user",
            &userpass,
            "--mail-from",
            &smtp.from,
            "--mail-rcpt",
            msg.to,
            "--upload-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("lancement de curl")?;
    let mut stdin = child.stdin.take().context("stdin curl")?;
    stdin.write_all(message.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!(
            "curl smtp {} : {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Message MIME complet (en-têtes + corps), CRLF partout.
pub fn build_message(
    smtp: &Smtp,
    msg: &Outgoing<'_>,
    date: &str,
    message_id: &str,
    boundary: &str,
) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        "From: {} <{}>\r\n",
        header_word(&smtp.from_name),
        smtp.from
    ));
    h.push_str(&format!(
        "To: {} <{}>\r\n",
        header_word(msg.to_name),
        msg.to
    ));
    if let Some(r) = msg.reply_to.filter(|r| !r.is_empty()) {
        h.push_str(&format!("Reply-To: {r}\r\n"));
    }
    h.push_str(&format!("Subject: {}\r\n", header_word(msg.subject)));
    h.push_str(&format!("Date: {date}\r\n"));
    h.push_str(&format!("Message-ID: {message_id}\r\n"));
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str("X-Auto-Response-Suppress: All\r\n");
    h.push_str("Auto-Submitted: auto-generated\r\n");
    match msg.html {
        None => {
            h.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
            h.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
            h.push_str(&quoted_printable(msg.text));
        }
        Some(html) => {
            h.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n"
            ));
            h.push_str(&format!("--{boundary}\r\n"));
            h.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
            h.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
            h.push_str(&quoted_printable(msg.text));
            h.push_str(&format!("\r\n--{boundary}\r\n"));
            h.push_str("Content-Type: text/html; charset=UTF-8\r\n");
            h.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
            h.push_str(&quoted_printable(html));
            h.push_str(&format!("\r\n--{boundary}--\r\n"));
        }
    }
    h
}

/// `<aléa@domaine de l'expéditeur>` : Gmail et consorts en veulent un, et unique.
pub fn message_id(from: &str) -> String {
    let domain = from.rsplit('@').next().unwrap_or("localhost");
    format!(
        "<{}.{}@{domain}>",
        chrono::Utc::now().timestamp(),
        random_hex(16)
    )
}

fn random_hex(n: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| format!("{:x}", rng.gen_range(0..16)))
        .collect()
}

/// Mot d'en-tête : tel quel s'il est ASCII sans caractère spécial, sinon encodé RFC 2047 (Q).
pub fn header_word(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    let plain = clean
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) && !matches!(b, b'"' | b'<' | b'>' | b'=' | b'?'));
    if plain {
        return clean;
    }
    let mut out = String::from("=?UTF-8?Q?");
    for b in clean.bytes() {
        match b {
            b' ' => out.push('_'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'!' | b',' | b'\'' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("={b:02X}")),
        }
    }
    out.push_str("?=");
    out
}

/// Quoted-printable (RFC 2045) : lignes ≤ 76 caractères, CRLF, espaces de fin encodés.
pub fn quoted_printable(s: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        let line = line.trim_end_matches('\r');
        let bytes = line.as_bytes();
        let mut col = 0usize;
        for (j, &b) in bytes.iter().enumerate() {
            let last = j + 1 == bytes.len();
            let enc = match b {
                b'=' => true,
                b' ' | b'\t' => last,
                0x21..=0x7e => false,
                _ => true,
            };
            let piece = if enc {
                format!("={b:02X}")
            } else {
                (b as char).to_string()
            };
            if col + piece.len() > 75 {
                out.push_str("=\r\n");
                col = 0;
            }
            out.push_str(&piece);
            col += piece.len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::Secret;

    fn smtp() -> Smtp {
        Smtp {
            host: "smtp.example.test".into(),
            port: 465,
            user: "u".into(),
            pass: Secret::new("p"),
            from: "noreply@example.test".into(),
            from_name: "Groscailloux".into(),
        }
    }

    #[test]
    fn plain_message_has_the_headers_spam_filters_expect() {
        let m = build_message(
            &smtp(),
            &Outgoing {
                to_name: "Léa",
                to: "lea@example.test",
                subject: "Ton accès est prêt",
                text: "Salut Léa,\nà bientôt.",
                html: None,
                reply_to: Some("contact@example.test"),
            },
            "Fri, 19 Sep 2026 22:00:00 +0200",
            "<1.abc@example.test>",
            "=_b",
        );
        assert!(m.starts_with("From: Groscailloux <noreply@example.test>\r\n"));
        assert!(m.contains("To: =?UTF-8?Q?L=C3=A9a?= <lea@example.test>\r\n"));
        assert!(m.contains("Reply-To: contact@example.test\r\n"));
        assert!(m.contains("Subject: =?UTF-8?Q?Ton_acc=C3=A8s_est_pr=C3=AAt?=\r\n"));
        assert!(m.contains("Date: Fri, 19 Sep 2026 22:00:00 +0200\r\n"));
        assert!(m.contains("Message-ID: <1.abc@example.test>\r\n"));
        assert!(m.contains("Content-Type: text/plain; charset=UTF-8\r\n"));
        assert!(m.ends_with("Salut L=C3=A9a,\r\n=C3=A0 bient=C3=B4t."));
        assert!(!m.contains("multipart"));
    }

    #[test]
    fn html_message_is_multipart_alternative_text_first() {
        let m = build_message(
            &smtp(),
            &Outgoing {
                to_name: "Bob",
                to: "bob@example.test",
                subject: "Hello",
                text: "Texte",
                html: Some("<p>Texte</p>"),
                reply_to: None,
            },
            "d",
            "<i@x>",
            "=_b",
        );
        assert!(m.contains("Subject: Hello\r\n"));
        assert!(!m.contains("Reply-To"));
        assert!(m.contains("Content-Type: multipart/alternative; boundary=\"=_b\"\r\n"));
        let text = m.find("text/plain").unwrap();
        let html = m.find("text/html").unwrap();
        assert!(text < html, "la partie texte vient d'abord");
        assert!(m.trim_end().ends_with("--=_b--"));
    }

    #[test]
    fn quoted_printable_folds_long_lines_and_encodes_specials() {
        let long = "a".repeat(100);
        let q = quoted_printable(&long);
        assert!(q.lines().all(|l| l.len() <= 76));
        assert!(q.contains("=\r\n"));
        assert_eq!(quoted_printable("x = y "), "x =3D y=20");
        assert_eq!(quoted_printable("é\r\nè"), "=C3=A9\r\n=C3=A8");
    }

    #[test]
    fn message_ids_are_unique_and_on_sender_domain() {
        let a = message_id("noreply@example.test");
        let b = message_id("noreply@example.test");
        assert_ne!(a, b);
        assert!(a.starts_with('<') && a.ends_with("@example.test>"));
    }

    #[test]
    fn header_word_strips_line_breaks() {
        assert_eq!(header_word("Bob\r\nBcc: x"), "BobBcc: x");
    }
}
