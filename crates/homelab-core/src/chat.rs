//! Tchat des membres, affiché dans Jellyfin (script chargé par JavaScript Injector, API dans
//! homelabd). Salons `annonces` (modérateurs seulement), `entraide`, `discussion`, et un fil privé
//! par membre (`prive:<id Jellyfin>`) lisible par lui et par les modérateurs. Identité = compte
//! Jellyfin (jeton vérifié par `/Users/Me`, jamais stocké). Stockage : SQLite (`[chat] db_file`).

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::config::Chat as ChatConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    Annonces,
    Entraide,
    Discussion,
    /// Fil privé d'un membre (id Jellyfin) avec les modérateurs.
    Private(String),
}

impl Channel {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "annonces" => Some(Self::Annonces),
            "entraide" => Some(Self::Entraide),
            "discussion" => Some(Self::Discussion),
            _ => {
                let id = s.strip_prefix("prive:")?;
                (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                    .then(|| Self::Private(id.to_ascii_lowercase().replace('-', "")))
            }
        }
    }

    pub fn key(&self) -> String {
        match self {
            Self::Annonces => "annonces".into(),
            Self::Entraide => "entraide".into(),
            Self::Discussion => "discussion".into(),
            Self::Private(id) => format!("prive:{id}"),
        }
    }

    pub const PUBLIC: [Channel; 3] = [Self::Annonces, Self::Entraide, Self::Discussion];
}

/// Membre authentifié (compte Jellyfin actif).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatUser {
    /// Id Jellyfin sans tirets, en minuscules.
    pub id: String,
    pub name: String,
    pub moderator: bool,
}

pub fn normalize_id(id: &str) -> String {
    id.to_ascii_lowercase().replace('-', "")
}

pub fn is_listed(name: &str, list: &[String]) -> bool {
    list.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Pendant la phase de test (`beta_users` non vide), seuls ces comptes voient le tchat.
pub fn allowed(name: &str, cfg: &ChatConfig) -> bool {
    cfg.beta_users.is_empty() || is_listed(name, &cfg.beta_users)
}

pub fn can_read(u: &ChatUser, ch: &Channel) -> bool {
    match ch {
        Channel::Private(owner) => u.moderator || *owner == u.id,
        _ => true,
    }
}

pub fn can_post(u: &ChatUser, ch: &Channel) -> bool {
    match ch {
        Channel::Annonces => u.moderator,
        Channel::Private(owner) => u.moderator || *owner == u.id,
        _ => true,
    }
}

/// Son propre message dans le délai, ou n'importe lequel pour un modérateur.
pub fn can_delete(u: &ChatUser, m: &Message, now: i64, own_window_secs: i64) -> bool {
    if m.deleted {
        return false;
    }
    u.moderator || (m.author_id == u.id && now - m.created_at <= own_window_secs)
}

/// Texte nettoyé (fins de ligne, espaces aux bords, 3 lignes vides au plus) ou erreur lisible.
pub fn validate_body(raw: &str, max_chars: usize) -> std::result::Result<String, &'static str> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = text
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    let mut out = String::new();
    let mut blank = 0;
    for line in text.trim().lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 2 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        return Err("message vide");
    }
    if out.chars().count() > max_chars {
        return Err("message trop long");
    }
    Ok(out)
}

/// Jeton Jellyfin envoyé par l'interface : `X-Emby-Token`, ou `Token="…"` de l'en-tête
/// `Authorization: MediaBrowser …`.
pub fn token_from_headers(
    x_emby_token: Option<&str>,
    authorization: Option<&str>,
) -> Option<String> {
    if let Some(t) = x_emby_token.map(str::trim).filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    let auth = authorization?;
    let i = auth.find("Token=\"")? + "Token=\"".len();
    let rest = &auth[i..];
    let t = &rest[..rest.find('"')?];
    (!t.is_empty()).then(|| t.to_string())
}

/// Limite par membre : un message toutes les `min_gap` s et `burst_max` par fenêtre.
#[derive(Default)]
pub struct RateLimiter {
    sent: HashMap<String, VecDeque<i64>>,
}

impl RateLimiter {
    pub fn check(&mut self, user: &str, now: i64, cfg: &ChatConfig) -> bool {
        let q = self.sent.entry(user.to_string()).or_default();
        while q
            .front()
            .is_some_and(|t| now - t >= cfg.burst_window_secs as i64)
        {
            q.pop_front();
        }
        if q.back().is_some_and(|t| now - t < cfg.min_gap_secs as i64) || q.len() >= cfg.burst_max {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Message {
    pub id: i64,
    pub channel: String,
    pub author_id: String,
    pub author_name: String,
    pub author_moderator: bool,
    /// Vide si le message a été supprimé.
    pub body: String,
    pub created_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PrivateThread {
    pub channel: String,
    pub member_id: String,
    pub member_name: String,
    pub last_id: i64,
    pub last_at: i64,
    pub last_body: String,
    pub unread: i64,
}

pub struct ChatStore {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel TEXT NOT NULL,
  author_id TEXT NOT NULL,
  author_name TEXT NOT NULL,
  author_moderator INTEGER NOT NULL DEFAULT 0,
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER,
  deleted_by TEXT
);
CREATE INDEX IF NOT EXISTS messages_channel ON messages(channel, id);
CREATE TABLE IF NOT EXISTS reads (
  user_id TEXT NOT NULL,
  channel TEXT NOT NULL,
  last_read_id INTEGER NOT NULL,
  PRIMARY KEY (user_id, channel)
);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
";

fn row_to_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let deleted: Option<i64> = r.get(7)?;
    Ok(Message {
        id: r.get(0)?,
        channel: r.get(1)?,
        author_id: r.get(2)?,
        author_name: r.get(3)?,
        author_moderator: r.get::<_, i64>(4)? != 0,
        body: if deleted.is_some() {
            String::new()
        } else {
            r.get(5)?
        },
        created_at: r.get(6)?,
        deleted: deleted.is_some(),
    })
}

const COLS: &str =
    "id, channel, author_id, author_name, author_moderator, body, created_at, deleted_at";

impl ChatStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn =
            Connection::open(path).with_context(|| format!("ouverture {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, ch: &Channel, author: &ChatUser, body: &str, now: i64) -> Result<Message> {
        let db = self.db();
        db.execute(
            "INSERT INTO messages (channel, author_id, author_name, author_moderator, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ch.key(), author.id, author.name, author.moderator as i64, body, now],
        )?;
        let id = db.last_insert_rowid();
        // l'auteur a lu son propre message
        db.execute(
            "INSERT INTO reads (user_id, channel, last_read_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, channel) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)",
            params![author.id, ch.key(), id],
        )?;
        Ok(Message {
            id,
            channel: ch.key(),
            author_id: author.id.clone(),
            author_name: author.name.clone(),
            author_moderator: author.moderator,
            body: body.to_string(),
            created_at: now,
            deleted: false,
        })
    }

    pub fn get(&self, id: i64) -> Result<Option<Message>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLS} FROM messages WHERE id = ?1"),
                [id],
                row_to_message,
            )
            .optional()?)
    }

    /// Messages d'un salon : après `after` (suite), sinon les `limit` derniers avant `before`.
    pub fn list(
        &self,
        ch: &Channel,
        after: Option<i64>,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let db = self.db();
        let limit = limit.clamp(1, 200) as i64;
        let out: Vec<Message> = match after {
            Some(a) => {
                let mut st = db.prepare(&format!(
                    "SELECT {COLS} FROM messages WHERE channel = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3"
                ))?;
                let rows = st.query_map(params![ch.key(), a, limit], row_to_message)?;
                rows.collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut st = db.prepare(&format!(
                    "SELECT {COLS} FROM messages WHERE channel = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"
                ))?;
                let rows = st.query_map(
                    params![ch.key(), before.unwrap_or(i64::MAX), limit],
                    row_to_message,
                )?;
                let mut v: Vec<Message> = rows.collect::<rusqlite::Result<_>>()?;
                v.reverse();
                v
            }
        };
        Ok(out)
    }

    pub fn mark_deleted(&self, id: i64, by: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET deleted_at = ?2, deleted_by = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, now, by],
        )?;
        Ok(())
    }

    pub fn mark_read(&self, user_id: &str, ch: &Channel, last_id: i64) -> Result<()> {
        self.db().execute(
            "INSERT INTO reads (user_id, channel, last_read_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, channel) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)",
            params![user_id, ch.key(), last_id],
        )?;
        Ok(())
    }

    /// Messages non lus d'un salon (ni supprimés, ni écrits par le lecteur).
    pub fn unread(&self, user_id: &str, ch: &Channel) -> Result<i64> {
        Ok(self.db().query_row(
            "SELECT COUNT(*) FROM messages m
             WHERE m.channel = ?2 AND m.deleted_at IS NULL AND m.author_id <> ?1
               AND m.id > COALESCE((SELECT last_read_id FROM reads WHERE user_id = ?1 AND channel = ?2), 0)",
            params![user_id, ch.key()],
            |r| r.get(0),
        )?)
    }

    /// Fils privés (pour les modérateurs), du plus récent au plus ancien.
    pub fn private_threads(&self, reader_id: &str) -> Result<Vec<PrivateThread>> {
        let channels: Vec<String> = {
            let db = self.db();
            let mut st = db.prepare(
                "SELECT channel FROM messages WHERE channel LIKE 'prive:%' GROUP BY channel ORDER BY MAX(id) DESC",
            )?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for key in channels {
            let Some(ch) = Channel::parse(&key) else {
                continue;
            };
            let Channel::Private(owner) = &ch else {
                continue;
            };
            let (last_id, last_at, last_body, name) = {
                let db = self.db();
                let last: (i64, i64, String, Option<i64>) = db.query_row(
                    "SELECT id, created_at, body, deleted_at FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT 1",
                    [&key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
                let name: Option<String> = db
                    .query_row(
                        "SELECT author_name FROM messages WHERE channel = ?1 AND author_id = ?2 ORDER BY id DESC LIMIT 1",
                        params![key, owner],
                        |r| r.get(0),
                    )
                    .optional()?;
                let body = if last.3.is_some() {
                    String::new()
                } else {
                    last.2
                };
                (
                    last.0,
                    last.1,
                    body,
                    name.unwrap_or_else(|| "membre".into()),
                )
            };
            out.push(PrivateThread {
                channel: key.clone(),
                member_id: owner.clone(),
                member_name: name,
                last_id,
                last_at,
                last_body: last_body.chars().take(120).collect(),
                unread: self.unread(reader_id, &ch)?,
            });
        }
        Ok(out)
    }

    /// Dernier message non lu d'un modérateur dans un salon (bannière « message de l'admin »).
    pub fn latest_unread_from_moderator(
        &self,
        user_id: &str,
        ch: &Channel,
    ) -> Result<Option<Message>> {
        Ok(self
            .db()
            .query_row(
                &format!(
                    "SELECT {COLS} FROM messages
                     WHERE channel = ?2 AND deleted_at IS NULL AND author_moderator = 1 AND author_id <> ?1
                       AND id > COALESCE((SELECT last_read_id FROM reads WHERE user_id = ?1 AND channel = ?2), 0)
                     ORDER BY id DESC LIMIT 1"
                ),
                params![user_id, ch.key()],
                row_to_message,
            )
            .optional()?)
    }

    pub fn meta(&self, key: &str) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    pub fn set_meta(&self, key: &str, value: i64) -> Result<()> {
        self.db().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Messages d'entraide et privés écrits par des membres (pas des modérateurs) depuis `after_id`.
    pub fn pending_for_moderators(&self, after_id: i64) -> Result<Vec<Message>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {COLS} FROM messages
             WHERE id > ?1 AND deleted_at IS NULL AND author_moderator = 0
               AND (channel = 'entraide' OR channel LIKE 'prive:%')
             ORDER BY id ASC LIMIT 200"
        ))?;
        let rows = st.query_map([after_id], row_to_message)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn max_id(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| {
                r.get(0)
            })?)
    }
}

/// Mail récapitulatif pour l'admin : (sujet, corps).
pub fn moderator_digest(msgs: &[Message], jellyfin_url: &str) -> (String, String) {
    let private = msgs
        .iter()
        .filter(|m| m.channel.starts_with("prive:"))
        .count();
    let help = msgs.len() - private;
    let mut parts = Vec::new();
    if help > 0 {
        parts.push(format!("{help} en entraide"));
    }
    if private > 0 {
        parts.push(format!("{private} en prive"));
    }
    let subject = format!(
        "Tchat Groscailloux : {} nouveau(x) message(s) ({})",
        msgs.len(),
        parts.join(", ")
    );
    let mut body = String::from("Nouveaux messages dans le tchat :\n\n");
    for m in msgs.iter().take(30) {
        let place = if m.channel.starts_with("prive:") {
            "prive"
        } else {
            "entraide"
        };
        let excerpt: String = m.body.chars().take(300).collect();
        let when = chrono::DateTime::from_timestamp(m.created_at, 0)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%d/%m %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        body.push_str(&format!(
            "[{place}] {} ({when}) :\n{excerpt}\n\n",
            m.author_name
        ));
    }
    if msgs.len() > 30 {
        body.push_str(&format!("… et {} autre(s).\n\n", msgs.len() - 30));
    }
    body.push_str(&format!(
        "Repondre : {jellyfin_url} (bulle en haut a droite).\n"
    ));
    (subject, body)
}

/// Mail d'une annonce aux membres : (sujet, corps).
pub fn announcement_mail(author: &str, text: &str, jellyfin_url: &str) -> (String, String) {
    let first: String = text.lines().next().unwrap_or("").chars().take(70).collect();
    let subject = format!("Groscailloux : {first}");
    let body = format!(
        "{text}\n\n— {author}\n\nAnnonce publiee dans le tchat de Groscailloux : {jellyfin_url}\n\
         (bulle en haut a droite, salon Annonces).\n"
    );
    (subject, body)
}

/// Mail d'un message privé de l'admin à un membre : (sujet, corps).
pub fn direct_mail(author: &str, member: &str, text: &str, jellyfin_url: &str) -> (String, String) {
    let subject = "Groscailloux : un message de l'admin pour toi".to_string();
    let body = format!(
        "Salut {member},\n\n{text}\n\n— {author}\n\nTu peux repondre dans le tchat de Groscailloux : {jellyfin_url}\n\
         (bulle en haut a droite, onglet \"Ecrire a l'admin\").\n"
    );
    (subject, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ChatConfig {
        ChatConfig::default()
    }

    fn user(id: &str, moderator: bool) -> ChatUser {
        ChatUser {
            id: id.into(),
            name: id.to_uppercase(),
            moderator,
        }
    }

    #[test]
    fn channel_keys_round_trip_and_reject_junk() {
        for k in ["annonces", "entraide", "discussion", "prive:abc123"] {
            assert_eq!(Channel::parse(k).unwrap().key(), k);
        }
        assert_eq!(
            Channel::parse("prive:4067B499-27a3").unwrap(),
            Channel::Private("4067b49927a3".into())
        );
        for k in ["", "prive:", "prive:../x", "general", "prive:a b"] {
            assert!(Channel::parse(k).is_none(), "{k}");
        }
    }

    #[test]
    fn permissions_matrix() {
        let (m, a, b) = (user("mod", true), user("a", false), user("b", false));
        let pa = Channel::Private("a".into());
        assert!(can_read(&a, &Channel::Annonces) && !can_post(&a, &Channel::Annonces));
        assert!(can_post(&m, &Channel::Annonces));
        for ch in [Channel::Entraide, Channel::Discussion] {
            assert!(can_read(&a, &ch) && can_post(&a, &ch));
        }
        assert!(can_read(&a, &pa) && can_post(&a, &pa));
        assert!(
            !can_read(&b, &pa) && !can_post(&b, &pa),
            "fil privé d'un autre"
        );
        assert!(can_read(&m, &pa) && can_post(&m, &pa));
    }

    #[test]
    fn beta_list_restricts_access() {
        let mut c = cfg();
        assert!(allowed("n'importe qui", &c));
        c.beta_users = vec!["Haradas".into()];
        assert!(allowed("haradas", &c) && !allowed("nina", &c));
    }

    #[test]
    fn body_is_cleaned_and_bounded() {
        assert_eq!(
            validate_body("  salut \r\n\r\n\r\n\r\n\r\nça va ?  ", 2000).unwrap(),
            "salut\n\n\nça va ?"
        );
        assert_eq!(validate_body("a\u{7}b", 2000).unwrap(), "ab");
        assert!(validate_body(" \n\t ", 2000).is_err());
        assert!(validate_body(&"é".repeat(2001), 2000).is_err());
        assert!(validate_body(&"é".repeat(2000), 2000).is_ok());
    }

    #[test]
    fn token_extraction() {
        assert_eq!(
            token_from_headers(Some("abc"), None).as_deref(),
            Some("abc")
        );
        let auth = r#"MediaBrowser Client="Jellyfin Web", Device="Safari", DeviceId="x", Version="10.11.8", Token="tok123""#;
        assert_eq!(
            token_from_headers(None, Some(auth)).as_deref(),
            Some("tok123")
        );
        assert_eq!(
            token_from_headers(Some(" "), Some(r#"MediaBrowser Client="x""#)),
            None
        );
    }

    #[test]
    fn rate_limit_gap_and_burst() {
        let mut c = cfg();
        c.min_gap_secs = 3;
        c.burst_max = 3;
        c.burst_window_secs = 60;
        let mut rl = RateLimiter::default();
        assert!(rl.check("a", 0, &c));
        assert!(!rl.check("a", 2, &c), "trop rapproché");
        assert!(rl.check("b", 2, &c), "par membre");
        assert!(rl.check("a", 3, &c) && rl.check("a", 6, &c));
        assert!(!rl.check("a", 9, &c), "rafale");
        assert!(rl.check("a", 61, &c), "fenêtre écoulée");
    }

    #[test]
    fn store_unread_delete_and_private_threads() {
        let s = ChatStore::open_in_memory().unwrap();
        let (m, a, b) = (user("mod", true), user("a", false), user("b", false));
        s.insert(&Channel::Annonces, &m, "news 1", 10).unwrap();
        let n2 = s.insert(&Channel::Annonces, &m, "news 2", 20).unwrap();
        assert_eq!(s.unread("a", &Channel::Annonces).unwrap(), 2);
        assert_eq!(
            s.unread("mod", &Channel::Annonces).unwrap(),
            0,
            "ses propres messages"
        );
        s.mark_read("a", &Channel::Annonces, n2.id).unwrap();
        s.mark_read("a", &Channel::Annonces, 1).unwrap(); // jamais en arrière
        assert_eq!(s.unread("a", &Channel::Annonces).unwrap(), 0);

        let h = s.insert(&Channel::Entraide, &a, "ça coupe", 30).unwrap();
        assert!(can_delete(&a, &h, 30 + 900, 900) && !can_delete(&a, &h, 30 + 901, 900));
        assert!(!can_delete(&b, &h, 31, 900) && can_delete(&m, &h, 99_999, 900));
        s.mark_deleted(h.id, "a", 40).unwrap();
        let got = s.get(h.id).unwrap().unwrap();
        assert!(got.deleted && got.body.is_empty());
        assert_eq!(
            s.unread("b", &Channel::Entraide).unwrap(),
            0,
            "supprimé = plus compté"
        );

        let pa = Channel::Private("a".into());
        s.insert(&pa, &a, "j'ai un souci", 50).unwrap();
        s.insert(&pa, &m, "je regarde", 60).unwrap();
        let t = s.private_threads("mod").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(
            (
                t[0].member_name.as_str(),
                t[0].unread,
                t[0].last_body.as_str()
            ),
            ("A", 0, "je regarde")
        );
        assert_eq!(s.unread("a", &pa).unwrap(), 1);

        let pending = s.pending_for_moderators(0).unwrap();
        assert_eq!(
            pending.len(),
            1,
            "entraide supprimée et messages de modérateur exclus"
        );
        let (subject, body) = moderator_digest(&pending, "https://jf");
        assert!(subject.contains("1 en prive") && body.contains("j'ai un souci"));
    }

    #[test]
    fn latest_unread_from_moderator_for_banner() {
        let s = ChatStore::open_in_memory().unwrap();
        let (m, a) = (user("mod", true), user("a", false));
        let pa = Channel::Private("a".into());
        assert!(s.latest_unread_from_moderator("a", &pa).unwrap().is_none());
        s.insert(&pa, &m, "ta période d'essai se termine samedi", 10)
            .unwrap();
        let got = s.latest_unread_from_moderator("a", &pa).unwrap().unwrap();
        assert_eq!(got.body, "ta période d'essai se termine samedi");
        assert!(
            s.latest_unread_from_moderator("mod", &pa)
                .unwrap()
                .is_none(),
            "pas pour l'auteur"
        );
        s.mark_read("a", &pa, got.id).unwrap();
        assert!(s.latest_unread_from_moderator("a", &pa).unwrap().is_none());
        s.insert(&pa, &m, "rappel", 30).unwrap();
        s.insert(&pa, &a, "merci !", 40).unwrap(); // répondre = avoir lu ce qui précède
        assert!(s.latest_unread_from_moderator("a", &pa).unwrap().is_none());
        let (subject, body) = direct_mail(
            "Haradas",
            "Nina",
            "ta période d'essai se termine samedi",
            "https://jf",
        );
        assert!(
            subject.contains("admin") && body.starts_with("Salut Nina,") && body.contains("samedi")
        );
    }

    #[test]
    fn list_pages_forward_and_backward() {
        let s = ChatStore::open_in_memory().unwrap();
        let a = user("a", false);
        for i in 0..5 {
            s.insert(&Channel::Discussion, &a, &format!("m{i}"), i)
                .unwrap();
        }
        let last2 = s.list(&Channel::Discussion, None, None, 2).unwrap();
        assert_eq!(
            last2.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["m3", "m4"]
        );
        let after = s
            .list(&Channel::Discussion, Some(last2[0].id), None, 50)
            .unwrap();
        assert_eq!(after.len(), 1);
        let before = s
            .list(&Channel::Discussion, None, Some(last2[0].id), 50)
            .unwrap();
        assert_eq!(before.len(), 3);
    }
}
