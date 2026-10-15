//! Envoi de mails via `curl smtps://` (même mécanique que l'ancien script : éprouvée avec Gmail, aucune
//! dépendance TLS côté Rust). Le message MIME est construit ici, avec ce que les filtres anti-spam attendent
//! d'un vrai mail (2026-09-19, le mail de bienvenue finissait en spam) : en-têtes `Date` et `Message-ID`,
//! sujet et noms encodés RFC 2047, corps UTF-8 en quoted-printable avec les vrais accents, et pour les mails
//! aux membres une version texte **et** une version HTML (`multipart/alternative`). Jamais d'identifiant ni de
//! mot de passe dans un mail : ils passent par un lien de bienvenue (`welcome.rs`).
//!
//! Mails riches (gabarit [`crate::mail_layout`], 2026-10-15) : les images (logo, affiche, fond) peuvent voyager
//! dans le mail, affichées par `cid:` : `multipart/alternative` = texte, puis `multipart/related` = HTML +
//! images en base64 (`Content-ID`, `Content-Disposition: inline`). Sans image, la structure reste celle d'avant.
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Smtp;
use crate::mail_layout::RenderedMail;

/// Un mail à envoyer : le texte est obligatoire, le HTML optionnel (même contenu, mis en forme).
/// `..Default::default()` complète les champs qu'un appel n'utilise pas.
#[derive(Default)]
pub struct Outgoing<'a> {
    pub to_name: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: Option<&'a str>,
    pub reply_to: Option<&'a str>,
    /// Images affichées dans le HTML par `cid:` ; ignorées sans HTML.
    pub images: &'a [InlineImage],
    /// En-têtes en plus (`List-Unsubscribe`…) : un nom invalide ou réservé (`From`, `Subject`, `Content-*`…)
    /// est ignoré, les sauts de ligne sont retirés de la valeur.
    pub headers: &'a [(&'a str, &'a str)],
}

impl<'a> Outgoing<'a> {
    /// Mail du gabarit [`crate::mail_layout`] : objet, texte, HTML et images jointes.
    pub fn rendered(to_name: &'a str, to: &'a str, mail: &'a RenderedMail) -> Self {
        Self {
            to_name,
            to,
            subject: &mail.subject,
            text: &mail.text,
            html: Some(&mail.html),
            images: &mail.images,
            ..Default::default()
        }
    }
}

/// Image jointe au mail et affichée dans le HTML par `src="cid:<cid>"` (partie de `multipart/related`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    /// Identifiant sans chevrons : `<nom>.<12 hex du contenu>@groscailloux` (ASCII, sans espace). La même image
    /// a toujours le même `cid`, ce qui permet de ne la joindre qu'une fois.
    pub cid: String,
    /// `image/png` ou `image/jpeg`, lu dans les premiers octets (jamais dans l'en-tête d'un serveur).
    pub content_type: &'static str,
    pub bytes: Arc<[u8]>,
}

impl InlineImage {
    /// Image PNG ou JPEG reconnue à ses premiers octets ; `None` pour tout autre format (WebP, SVG, GIF, HTML…).
    /// `name` donne le début du `cid` et le nom du fichier joint (`affiche` → `affiche.jpg`).
    pub fn from_bytes(name: &str, bytes: impl Into<Arc<[u8]>>) -> Option<Self> {
        let bytes: Arc<[u8]> = bytes.into();
        let content_type = sniff_image(&bytes)?;
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .take(6)
            .map(|b| format!("{b:02x}"))
            .collect();
        Some(Self {
            cid: format!("{}.{hash}@groscailloux", slug(name)),
            content_type,
            bytes,
        })
    }

    /// Nom du fichier joint : début du `cid` + extension du type.
    pub fn filename(&self) -> String {
        let stem = slug(self.cid.split(['.', '@']).next().unwrap_or(""));
        let ext = if self.content_type == "image/png" {
            "png"
        } else {
            "jpg"
        };
        format!("{stem}.{ext}")
    }
}

/// `image/png` ou `image/jpeg` d'après la signature du fichier, sinon `None`.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// `[a-z0-9-]` seulement (un seul `-` d'affilée), 32 caractères au plus, jamais vide (nom de fichier et début
/// de `cid`).
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "image".into()
    } else {
        out.into()
    }
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
            ..Default::default()
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
            ..Default::default()
        },
    )
    .await
}

/// Mail riche du gabarit [`crate::mail_layout`] (texte + HTML + images jointes) ; `headers` : voir
/// [`Outgoing::headers`].
pub async fn send_rendered(
    smtp: &Smtp,
    to_name: &str,
    to: &str,
    mail: &RenderedMail,
    reply_to: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<()> {
    send(
        smtp,
        &Outgoing {
            reply_to,
            headers,
            ..Outgoing::rendered(to_name, to, mail)
        },
    )
    .await
}

/// Message complet tel qu'il part : `Date` de maintenant, `Message-ID` et séparateur neufs. Sert aussi à écrire
/// un `.eml` d'essai (`homelabctl mail-test --dump`).
pub fn message(smtp: &Smtp, msg: &Outgoing<'_>) -> String {
    let date = chrono::Local::now().to_rfc2822();
    let id = message_id(&smtp.from);
    let boundary = format!("=_gc_{}", random_hex(12));
    build_message(smtp, msg, &date, &id, &boundary)
}

/// Envoi au résultat inconnu (2026-10-15) : `curl` a passé le message entier au serveur, puis a échoué sans réponse
/// nette (délai dépassé, connexion coupée en attendant la réponse) ; le serveur l'a peut-être accepté. Un nouvel
/// essai risquerait un doublon : [`maybe_sent`] le reconnaît dans une erreur de [`send`].
#[derive(Debug)]
pub struct MaybeSent;

impl std::fmt::Display for MaybeSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("message transmis en entier, sans réponse nette du serveur : peut-être parti")
    }
}

impl std::error::Error for MaybeSent {}

/// L'erreur d'un envoi dit-elle « peut-être parti » ([`MaybeSent`]) ?
pub fn maybe_sent(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<MaybeSent>())
}

/// Échec de `curl` après avoir passé `uploaded` octets d'un message de `len` octets : peut-être parti si le message
/// est passé en entier et que le serveur n'a pas répondu par un refus (code 8 = réponse inattendue, refus après
/// `DATA`). Message incomplet (refus de `RCPT`, coupure pendant l'envoi) : jamais remis, nouvel essai possible.
fn send_uncertain(code: Option<i32>, uploaded: u64, len: usize) -> bool {
    code != Some(8) && uploaded >= len as u64
}

/// Adresse utilisable telle quelle dans `--mail-rcpt` et l'en-tête `To` : ASCII imprimable, un seul `@`, rien qui
/// ferait une deuxième adresse ou un saut de ligne (une adresse Jellyseerr est saisie par le membre).
pub fn deliverable_address(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && s.bytes().all(|b| {
            b.is_ascii_graphic()
                && !matches!(
                    b,
                    b'<' | b'>' | b'(' | b')' | b',' | b';' | b':' | b'"' | b'[' | b']' | b'\\'
                )
        })
        && !domain.contains('@')
}

pub async fn send(smtp: &Smtp, msg: &Outgoing<'_>) -> Result<()> {
    if !deliverable_address(msg.to) {
        // l'adresse n'est jamais écrite dans l'erreur (journal)
        bail!("adresse du destinataire inutilisable : mail non envoyé");
    }
    let message = message(smtp, msg);
    let url = format!("smtps://{}:{}", smtp.host, smtp.port);
    let userpass = format!("{}:{}", smtp.user, smtp.pass.expose());
    let mut child = Command::new("curl")
        .args([
            "-fsS",
            // octets du message passés au serveur, écrits même en cas d'échec : distingue « jamais parti » de
            // « peut-être parti » (voir `MaybeSent`)
            "-w",
            "%{size_upload}",
            // un serveur SMTP muet ne bloque plus une page web ou une tâche indéfiniment
            "--connect-timeout",
            "20",
            "--max-time",
            "60",
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("lancement de curl")?;
    let mut stdin = child.stdin.take().context("stdin curl")?;
    // curl qui renonce avant DATA (authentification, destinataire refusé) ferme son entrée : au-delà du tampon du
    // tube (64 Kio, mails à images) l'écriture échoue en EPIPE. Le vrai motif est dans le code et le stderr de curl.
    if let Err(e) = stdin.write_all(message.as_bytes()).await {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e.into());
        }
    }
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        let detail = format!(
            "curl smtp {} : {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        let uploaded: u64 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        if send_uncertain(out.status.code(), uploaded, message.len()) {
            return Err(anyhow::Error::new(MaybeSent).context(detail));
        }
        bail!(detail);
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
        "From: {}\r\n",
        mailbox(&smtp.from_name, &smtp.from)
    ));
    h.push_str(&format!("To: {}\r\n", mailbox(msg.to_name, msg.to)));
    if let Some(r) = reply_to_kept(&smtp.from, msg.reply_to) {
        h.push_str(&format!("Reply-To: {r}\r\n"));
    }
    h.push_str(&format!("Subject: {}\r\n", header_word(msg.subject)));
    h.push_str(&format!("Date: {date}\r\n"));
    h.push_str(&format!("Message-ID: {message_id}\r\n"));
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str("X-Auto-Response-Suppress: All\r\n");
    h.push_str("Auto-Submitted: auto-generated\r\n");
    for (name, value) in msg.headers {
        if let Some(line) = extra_header(name, value) {
            h.push_str(&line);
        }
    }
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
            if msg.images.is_empty() {
                html_part(&mut h, html);
            } else {
                // Frontière intérieure : ni préfixe de l'extérieure ni l'inverse (certains lecteurs comparent des
                // préfixes), et `=_` ne peut apparaître ni en quoted-printable ni en base64.
                let related = format!("=_rel{}", boundary.trim_start_matches("=_"));
                h.push_str(&format!(
                    "Content-Type: multipart/related; type=\"text/html\";\r\n boundary=\"{related}\"\r\n\r\n"
                ));
                h.push_str(&format!("--{related}\r\n"));
                html_part(&mut h, html);
                let mut seen: Vec<&str> = Vec::new();
                for img in msg.images {
                    if seen.contains(&img.cid.as_str()) {
                        continue;
                    }
                    seen.push(&img.cid);
                    let cid = header_token(&img.cid);
                    let file = img.filename();
                    h.push_str(&format!("\r\n--{related}\r\n"));
                    h.push_str(&format!(
                        "Content-Type: {}; name=\"{file}\"\r\n",
                        img.content_type
                    ));
                    h.push_str("Content-Transfer-Encoding: base64\r\n");
                    h.push_str(&format!("Content-ID: <{cid}>\r\n"));
                    h.push_str(&format!(
                        "Content-Disposition: inline; filename=\"{file}\"\r\n\r\n"
                    ));
                    h.push_str(&base64_lines(&img.bytes));
                }
                h.push_str(&format!("\r\n--{related}--\r\n"));
            }
            h.push_str(&format!("\r\n--{boundary}--\r\n"));
        }
    }
    h
}

/// Partie `text/html` (en-têtes + corps quoted-printable), sans saut de ligne final.
fn html_part(h: &mut String, html: &str) {
    h.push_str("Content-Type: text/html; charset=UTF-8\r\n");
    h.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    h.push_str(&quoted_printable(html));
}

/// Identifiant d'en-tête (`Content-ID`) : ASCII `[A-Za-z0-9._@-]` seulement (ni chevron ni saut de ligne).
fn header_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | '-'))
        .collect()
}

/// En-têtes qu'un appel ne peut ni remplacer ni doubler.
const RESERVED_HEADERS: &[&str] = &[
    "from",
    "to",
    "cc",
    "bcc",
    "sender",
    "reply-to",
    "return-path",
    "subject",
    "date",
    "message-id",
    "mime-version",
    "auto-submitted",
    "x-auto-response-suppress",
];

/// Ligne `Nom: valeur\r\n` d'un en-tête en plus, ou `None` si le nom est invalide ou réservé. Valeur ASCII
/// gardée telle quelle (`<mailto:…>` d'un `List-Unsubscribe`), sinon encodée RFC 2047.
fn extra_header(name: &str, value: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !RESERVED_HEADERS.contains(&lower.as_str())
        && !lower.starts_with("content-");
    if !valid {
        return None;
    }
    let value: String = value
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n'))
        .collect();
    let value = if value.is_ascii() {
        value
    } else {
        header_word(&value)
    };
    // ligne de 78 caractères au plus (RFC 5322 § 2.1.1) : une valeur longue passe à la ligne suivante
    if value.is_ascii() && name.len() + 2 + value.len() > 78 {
        return Some(format!("{name}:\r\n {value}\r\n"));
    }
    Some(format!("{name}: {value}\r\n"))
}

/// `Nom <adresse>` (`From`, `To`), ou `<adresse>` sans nom.
fn mailbox(name: &str, addr: &str) -> String {
    match display_name(name) {
        n if n.is_empty() => format!("<{addr}>"),
        n => format!("{n} <{addr}>"),
    }
}

/// `Reply-To` gardé seulement sur le domaine de l'expéditeur (2026-10-15) : une adresse de réponse sur une messagerie
/// gratuite alors que `From` est sur le domaine du service est le schéma de l'hameçonnage (règle SpamAssassin
/// `FREEMAIL_FORGED_REPLYTO`, heuristiques voisines chez Gmail), et ces mails demandent de choisir un mot de passe.
/// Sans lui, une réponse arrive à l'adresse d'envoi. Même adresse que `From` : inutile, retiré aussi.
pub fn reply_to_kept<'a>(from: &str, reply_to: Option<&'a str>) -> Option<&'a str> {
    let r = reply_to?.trim();
    let domain = |a: &str| a.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase());
    (deliverable_address(r)
        && !r.eq_ignore_ascii_case(from.trim())
        && domain(r).is_some()
        && domain(r) == domain(from.trim()))
    .then_some(r)
}

/// Base64 (RFC 2045) : lignes de 76 caractères séparées par CRLF, sans CRLF final.
pub fn base64_lines(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(57) * 78);
    // 57 octets → 76 caractères : une ligne par bloc
    for (i, line) in bytes.chunks(57).enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        for chunk in line.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
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

/// Charge utile d'un mot encodé : 54 caractères, soit 66 avec `=?UTF-8?Q?…?=` et une ligne `Subject: …` de
/// 75 caractères au plus (RFC 2047 : un mot encodé ne dépasse pas 75 caractères).
const ENCODED_PAYLOAD_MAX: usize = 54;

/// Mot d'en-tête : tel quel s'il est ASCII sans caractère spécial, sinon un ou plusieurs mots RFC 2047 (Q),
/// coupés entre deux caractères (jamais au milieu d'un caractère UTF-8) et pliés par `CRLF + espace`. Un objet
/// long (titre de film) ne donne plus un seul mot géant hors norme ; le lecteur recolle les mots à l'identique.
pub fn header_word(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    let plain = clean
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) && !matches!(b, b'"' | b'<' | b'>' | b'=' | b'?'));
    if plain {
        return clean;
    }
    encoded_words(&clean, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'!' | b',' | b'\'')
    })
}

/// Nom affiché d'une adresse (`From`, `To`) : une `phrase` RFC 5322. Tel quel s'il n'est fait que de mots ASCII sans
/// caractère spécial ; entre guillemets (`\` et `"` échappés) s'il en porte un (`Dupont, Jean` ferait sinon deux
/// adresses, `Team: Bob` un groupe) ; mots RFC 2047 (Q) réduits aux lettres, chiffres et `!*+-/` s'il n'est pas ASCII
/// (RFC 2047 § 5(3) : ni `.` ni `,` ni `'` en clair dans un nom). Le nom vient du membre (Jellyseerr) : jamais de
/// saut de ligne ni de caractère de contrôle.
pub fn display_name(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_ascii() {
        let atom = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-/^_`{|}~".contains(&b);
        if clean
            .split(' ')
            .all(|w| !w.is_empty() && w.bytes().all(atom))
            || clean.is_empty()
        {
            return clean.to_string();
        }
        return format!("\"{}\"", clean.replace('\\', "\\\\").replace('"', "\\\""));
    }
    encoded_words(clean, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'!' | b'*' | b'+' | b'-' | b'/')
    })
}

/// Mots RFC 2047 (Q) : `keep` dit quels octets restent en clair (l'espace devient `_`, le reste `=XX`), coupés entre
/// deux caractères (jamais au milieu d'un caractère UTF-8) et pliés par `CRLF + espace`.
fn encoded_words(clean: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in clean.chars() {
        let mut buf = [0u8; 4];
        let mut piece = String::new();
        for &b in ch.encode_utf8(&mut buf).as_bytes() {
            match b {
                b' ' => piece.push('_'),
                b if keep(b) => piece.push(b as char),
                _ => piece.push_str(&format!("={b:02X}")),
            }
        }
        if !cur.is_empty() && cur.len() + piece.len() > ENCODED_PAYLOAD_MAX {
            words.push(std::mem::take(&mut cur));
        }
        cur.push_str(&piece);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .iter()
        .map(|w| format!("=?UTF-8?Q?{w}?="))
        .collect::<Vec<_>>()
        .join("\r\n ")
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
                reply_to: Some("contact@example.test"),
                ..Default::default()
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
                ..Default::default()
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

    // --- Lecture minimale d'un message, pour vérifier la structure sans faire confiance au code testé ---

    /// Parties d'un `multipart` délimité par `boundary` (préambule et fermeture exclus).
    fn parts<'m>(body: &'m str, boundary: &str) -> Vec<&'m str> {
        let delim = format!("--{boundary}");
        let mut out = Vec::new();
        for chunk in body.split(delim.as_str()).skip(1) {
            if chunk.starts_with("--") {
                break;
            }
            let chunk = chunk.strip_prefix("\r\n").expect("CRLF après la frontière");
            out.push(chunk.strip_suffix("\r\n").unwrap_or(chunk));
        }
        out
    }

    fn split_head(part: &str) -> (&str, &str) {
        part.split_once("\r\n\r\n")
            .expect("ligne vide après les en-têtes")
    }

    fn qp_decode(s: &str) -> String {
        let s = s.replace("=\r\n", "").replace("\r\n", "\n");
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'=' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn b64_decode(s: &str) -> Vec<u8> {
        const T: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0);
        for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
            acc = (acc << 6) | T.find(c).expect("caractère base64") as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    /// Mots RFC 2047 Q (`=?UTF-8?Q?…?=` séparés par `CRLF + espace`) → texte.
    fn q_decode(header: &str) -> String {
        let mut bytes = Vec::new();
        for w in header.split("\r\n ") {
            let w = w
                .strip_prefix("=?UTF-8?Q?")
                .unwrap()
                .strip_suffix("?=")
                .unwrap();
            // chaque mot se décode seul : jamais un caractère UTF-8 coupé en deux
            let one = qp_decode(&w.replace('_', " "));
            bytes.extend_from_slice(one.as_bytes());
        }
        String::from_utf8(bytes).unwrap()
    }

    fn png(extra: &[u8]) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(extra);
        v
    }

    fn jpeg(extra: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(extra);
        v
    }

    #[test]
    fn rich_message_nests_related_html_and_images_inside_alternative() {
        let logo = InlineImage::from_bytes("logo", png(&[7; 100])).unwrap();
        let poster = InlineImage::from_bytes("Affiche : Le Menu", jpeg(&[1; 200])).unwrap();
        let html = format!(
            r#"<p>Le Menu</p><img src="cid:{}"><img src="cid:{}"><img src="cid:{}">"#,
            logo.cid, poster.cid, logo.cid
        );
        // le logo deux fois : une seule partie jointe
        let images = vec![logo.clone(), poster.clone(), logo.clone()];
        let m = build_message(
            &smtp(),
            &Outgoing {
                to_name: "Léa",
                to: "lea@example.test",
                subject: "Le Menu est disponible",
                text: "Salut Léa,\nLe Menu est arrivé.",
                html: Some(&html),
                images: &images,
                ..Default::default()
            },
            "d",
            "<i@x>",
            "=_gc_0123456789ab",
        );
        assert!(
            m.match_indices('\n')
                .all(|(i, _)| i > 0 && &m[i - 1..i] == "\r"),
            "CRLF partout"
        );
        assert!(m.split("\r\n").all(|l| l.len() <= 78), "lignes courtes");
        let (head, body) = split_head(&m);
        assert!(
            head.contains("Content-Type: multipart/alternative; boundary=\"=_gc_0123456789ab\"")
        );
        let alt = parts(body, "=_gc_0123456789ab");
        assert_eq!(alt.len(), 2);
        let (h_text, text) = split_head(alt[0]);
        assert!(h_text.starts_with("Content-Type: text/plain; charset=UTF-8"));
        assert_eq!(qp_decode(text), "Salut Léa,\nLe Menu est arrivé.");
        let (h_rel, rel) = split_head(alt[1]);
        assert_eq!(
            h_rel,
            "Content-Type: multipart/related; type=\"text/html\";\r\n boundary=\"=_relgc_0123456789ab\""
        );
        let rel = parts(rel, "=_relgc_0123456789ab");
        assert_eq!(rel.len(), 3, "HTML + 2 images");
        let (h_html, body_html) = split_head(rel[0]);
        assert!(h_html.starts_with("Content-Type: text/html; charset=UTF-8"));
        assert_eq!(qp_decode(body_html), html);
        for (part, img, file) in [
            (rel[1], &logo, "logo.png"),
            (rel[2], &poster, "affiche-le-menu.jpg"),
        ] {
            let (h, data) = split_head(part);
            assert!(h.contains(&format!(
                "Content-Type: {}; name=\"{file}\"",
                img.content_type
            )));
            assert!(h.contains("Content-Transfer-Encoding: base64"));
            assert!(h.contains(&format!("Content-ID: <{}>", img.cid)));
            assert!(h.contains(&format!("Content-Disposition: inline; filename=\"{file}\"")));
            assert!(data.split("\r\n").all(|l| l.len() <= 76));
            assert_eq!(b64_decode(data), img.bytes.to_vec());
            assert!(html.contains(&format!("cid:{}", img.cid)));
        }
        assert!(m.ends_with("--=_relgc_0123456789ab--\r\n\r\n--=_gc_0123456789ab--\r\n"));
    }

    #[test]
    fn images_without_html_give_a_plain_message() {
        let logo = InlineImage::from_bytes("logo", png(&[0; 10])).unwrap();
        let m = build_message(
            &smtp(),
            &Outgoing {
                to_name: "Bob",
                to: "bob@example.test",
                subject: "Hello",
                text: "Texte",
                images: std::slice::from_ref(&logo),
                ..Default::default()
            },
            "d",
            "<i@x>",
            "=_b",
        );
        assert!(!m.contains("multipart") && !m.contains("Content-ID"));
    }

    #[test]
    fn inline_images_are_png_or_jpeg_with_a_stable_header_safe_cid() {
        let a = InlineImage::from_bytes("Affiche : Le Menu", jpeg(b"abc")).unwrap();
        let b = InlineImage::from_bytes("Affiche : Le Menu", jpeg(b"abc")).unwrap();
        assert_eq!(a.cid, b.cid, "même contenu, même cid");
        assert!(a.cid.starts_with("affiche-le-menu.") && a.cid.ends_with("@groscailloux"));
        assert_eq!(a.content_type, "image/jpeg");
        assert_eq!(a.filename(), "affiche-le-menu.jpg");
        let c = InlineImage::from_bytes("", png(b"x")).unwrap();
        assert_eq!(
            (c.content_type, c.filename().as_str()),
            ("image/png", "image.png")
        );
        assert!(InlineImage::from_bytes("x", b"RIFF\0\0\0\0WEBPVP8 ".to_vec()).is_none());
        assert!(InlineImage::from_bytes("x", b"<svg xmlns=".to_vec()).is_none());
        assert!(InlineImage::from_bytes("x", Vec::<u8>::new()).is_none());
        assert!(a
            .cid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '@')));
    }

    #[test]
    fn extra_headers_cannot_inject_or_replace_core_headers() {
        let headers = [
            (
                "List-Unsubscribe",
                "<mailto:contact@example.test?subject=Stop>",
            ),
            ("Subject", "remplacé ?"),
            ("Content-Type", "text/html"),
            ("X-Bad\r\nBcc", "y"),
            ("X-Note", "a\r\nBcc: evil@example.test"),
            ("X-Libellé", "z"),
            ("X-Titre", "Été"),
        ];
        let m = build_message(
            &smtp(),
            &Outgoing {
                to_name: "Bob",
                to: "bob@example.test",
                subject: "Hello",
                text: "Texte",
                headers: &headers,
                ..Default::default()
            },
            "d",
            "<i@x>",
            "=_b",
        );
        assert!(m.contains("\r\nList-Unsubscribe: <mailto:contact@example.test?subject=Stop>\r\n"));
        assert_eq!(m.matches("Subject:").count(), 1);
        assert_eq!(m.matches("Content-Type:").count(), 1);
        assert!(!m.split("\r\n").any(|l| l.starts_with("Bcc")));
        assert!(m.contains("\r\nX-Note: aBcc: evil@example.test\r\n"));
        assert!(!m.contains("X-Libell"));
        assert!(m.contains("\r\nX-Titre: =?UTF-8?Q?=C3=89t=C3=A9?=\r\n"));
    }

    #[test]
    fn base64_matches_reference_and_wraps_at_76() {
        assert_eq!(base64_lines(b""), "");
        assert_eq!(base64_lines(b"f"), "Zg==");
        assert_eq!(base64_lines(b"fo"), "Zm8=");
        assert_eq!(base64_lines(b"foo"), "Zm9v");
        assert_eq!(base64_lines(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_lines(&[0xfb, 0xff]), "+/8=");
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let enc = base64_lines(&data);
        let lines: Vec<&str> = enc.split("\r\n").collect();
        assert!(lines[..lines.len() - 1].iter().all(|l| l.len() == 76));
        assert!(lines.last().unwrap().len() <= 76);
        assert_eq!(b64_decode(&enc), data);
    }

    #[test]
    fn long_subjects_fold_into_short_encoded_words_that_decode_back() {
        for subject in [
            "X-Files : Aux frontières du réel — saisons 1 à 10 disponibles",
            "Bienvenue sur Groscailloux TV : définis ton mot de passe",
            "Ton paiement n'est pas passé : ton accès est en pause",
            "Le Menu et 3 autres demandes sont disponibles",
            "ééééééééééééééééééééééééééééééééééééééééééé",
            "🎬 Ça tourne : « Les Évadés » est là",
        ] {
            let h = header_word(subject);
            let line = format!("Subject: {h}");
            assert!(line.split("\r\n").all(|l| l.len() <= 75), "{line}");
            assert!(h.split("\r\n ").all(|w| w.len() <= 75));
            if subject.is_ascii() && !subject.contains(['"', '<', '>', '=', '?']) {
                assert_eq!(h, subject);
            } else {
                assert_eq!(q_decode(&h), subject);
            }
        }
        // un objet court reste un seul mot (test des en-têtes ci-dessus)
        assert!(!header_word("Ton accès est prêt").contains("\r\n"));
    }
    #[test]
    fn display_names_never_break_the_address_header() {
        let to = |name: &str| {
            let m = build_message(
                &smtp(),
                &Outgoing {
                    to_name: name,
                    to: "x@example.test",
                    subject: "s",
                    text: "t",
                    ..Default::default()
                },
                "d",
                "<i@x>",
                "=_b",
            );
            m.split("\r\n")
                .find(|l| l.starts_with("To:"))
                .unwrap()
                .to_string()
        };
        assert_eq!(to("Nina"), "To: Nina <x@example.test>");
        assert_eq!(to("Jean Paul"), "To: Jean Paul <x@example.test>");
        // une virgule ferait deux adresses, un deux-points un groupe : entre guillemets
        assert_eq!(to("Dupont, Jean"), "To: \"Dupont, Jean\" <x@example.test>");
        assert_eq!(to("Team: Bob"), "To: \"Team: Bob\" <x@example.test>");
        assert_eq!(to("a\"b\\c"), "To: \"a\\\"b\\\\c\" <x@example.test>");
        assert_eq!(to("J. R."), "To: \"J. R.\" <x@example.test>");
        assert_eq!(
            to("=?UTF-8?Q?x?="),
            "To: \"=?UTF-8?Q?x?=\" <x@example.test>"
        );
        assert_eq!(to(""), "To: <x@example.test>");
        assert_eq!(
            to("Bob\r\nBcc: y@example.test"),
            "To: \"BobBcc: y@example.test\" <x@example.test>"
        );
        // pas ASCII : mots Q sans `.` `,` `'` en clair (RFC 2047 § 5(3))
        assert_eq!(to("Léa"), "To: =?UTF-8?Q?L=C3=A9a?= <x@example.test>");
        let h = display_name("Zoé, l'aînée.");
        assert_eq!(h, "=?UTF-8?Q?Zo=C3=A9=2C_l=27a=C3=AEn=C3=A9e=2E?=");
        assert_eq!(q_decode(&h), "Zoé, l'aînée.");
    }

    #[test]
    fn reply_to_only_on_the_sender_domain() {
        let from = "contact@example.test";
        assert_eq!(
            reply_to_kept(from, Some("admin@example.test")),
            Some("admin@example.test")
        );
        assert_eq!(
            reply_to_kept(from, Some("admin@EXAMPLE.test")),
            Some("admin@EXAMPLE.test")
        );
        assert_eq!(
            reply_to_kept(from, Some("admin@webmail.example.test")),
            None
        );
        assert_eq!(reply_to_kept(from, Some("contact@example.test")), None);
        assert_eq!(
            reply_to_kept(from, Some("a@example.test\r\nBcc: b@x.example.test")),
            None
        );
        assert_eq!(reply_to_kept(from, Some("")), None);
        assert_eq!(reply_to_kept(from, None), None);
        let m = build_message(
            &smtp(),
            &Outgoing {
                to: "x@example.test",
                subject: "s",
                text: "t",
                reply_to: Some("quelquun@webmail.example.test"),
                ..Default::default()
            },
            "d",
            "<i@x>",
            "=_b",
        );
        assert!(!m.contains("Reply-To"));
    }

    #[test]
    fn only_plain_addresses_are_sent_to() {
        for ok in [
            "a@example.test",
            "a.b+c@sub.example.test",
            "o'neil@example.test",
        ] {
            assert!(deliverable_address(ok), "{ok}");
        }
        for bad in [
            "",
            "pseudo",
            "@example.test",
            "a@localhost",
            "a@b@example.test",
            "a@example.test\r\nBcc: b@example.test",
            "a b@example.test",
            "<a@example.test>",
            "a@example.test, b@example.test",
            "é@example.test",
            "a@.example.test",
        ] {
            assert!(!deliverable_address(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_send_that_passed_the_whole_message_is_maybe_sent() {
        // mesuré avec curl 8.18 contre un faux serveur : réponse attendue après DATA (28), refus après DATA (8),
        // refus du destinataire (55, 0 octet), serveur muet dès l'accueil (28, 0 octet)
        assert!(send_uncertain(Some(28), 270_182, 270_177));
        assert!(send_uncertain(Some(56), 270_182, 270_177));
        assert!(!send_uncertain(Some(8), 270_182, 270_177));
        assert!(!send_uncertain(Some(55), 0, 270_177));
        assert!(!send_uncertain(Some(28), 0, 270_177));
        assert!(!send_uncertain(Some(55), 4_096, 270_177));
        let e = anyhow::Error::new(MaybeSent).context("curl smtp 28");
        assert!(maybe_sent(&e));
        assert!(format!("{e:#}").starts_with("curl smtp 28: message transmis en entier"));
        assert!(!maybe_sent(&anyhow::anyhow!("curl smtp 55")));
    }

    #[test]
    fn long_extra_headers_fold_before_their_value() {
        let unsub = "<mailto:contact@domaine.example.test?subject=Stop%20mails%20disponible>";
        let line = extra_header("List-Unsubscribe", unsub).unwrap();
        assert_eq!(line, format!("List-Unsubscribe:\r\n {unsub}\r\n"));
        assert!(line.split("\r\n").all(|l| l.len() <= 78));
        assert_eq!(
            extra_header("List-Unsubscribe", "<mailto:a@example.test>").unwrap(),
            "List-Unsubscribe: <mailto:a@example.test>\r\n"
        );
    }
}
