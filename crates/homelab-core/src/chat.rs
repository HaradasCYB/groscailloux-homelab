//! Tchat des membres, affiché dans Jellyfin (script chargé par JavaScript Injector, API dans
//! homelabd). Salons `annonces` (modérateurs seulement) et `entraide`, et un fil privé par membre
//! (`prive:<id Jellyfin>`) lisible par lui et par les modérateurs. Identité = compte Jellyfin (jeton
//! vérifié par `/Users/Me`, jamais stocké). Stockage : SQLite (`[chat] db_file`).
//!
//! 2026-10-08 : l'ancien salon `discussion` est fusionné dans `entraide` (voir `merge_discussion`) ; le
//! mot « discussion » reste accepté comme synonyme d'`entraide` pour une page restée ouverte.

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
    /// Fil privé d'un membre (id Jellyfin) avec les modérateurs.
    Private(String),
}

impl Channel {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "annonces" => Some(Self::Annonces),
            // « discussion » : ancien salon fusionné dans Entraide (2026-10-08) ; une page ouverte avant la
            // mise à jour continue de le demander, elle lit et écrit dans Entraide sans erreur
            "entraide" | "discussion" => Some(Self::Entraide),
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
            Self::Private(id) => format!("prive:{id}"),
        }
    }

    pub const PUBLIC: [Channel; 2] = [Self::Annonces, Self::Entraide];
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

/// Découpe une annonce longue en messages d'au plus `max_chars`, de préférence entre deux paragraphes
/// (ligne vide), sinon entre deux lignes, sinon au caractère. Chaque partie est déjà nettoyée.
pub fn split_parts(text: &str, max_chars: usize) -> Vec<String> {
    let max = max_chars.max(1);
    let mut parts = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, parts: &mut Vec<String>| {
        let t = cur.trim().to_string();
        if !t.is_empty() {
            parts.push(t);
        }
        cur.clear();
    };
    for para in text.split("\n\n") {
        let para = para.trim_end();
        if para.trim().is_empty() {
            continue;
        }
        let joined = if cur.is_empty() {
            para.to_string()
        } else {
            format!("{cur}\n\n{para}")
        };
        if joined.chars().count() <= max {
            cur = joined;
            continue;
        }
        flush(&mut cur, &mut parts);
        if para.chars().count() <= max {
            cur = para.to_string();
            continue;
        }
        // paragraphe trop long à lui seul : ligne par ligne, puis au caractère
        for line in para.lines() {
            let joined = if cur.is_empty() {
                line.to_string()
            } else {
                format!("{cur}\n{line}")
            };
            if joined.chars().count() <= max {
                cur = joined;
                continue;
            }
            flush(&mut cur, &mut parts);
            let mut chunk = String::new();
            for c in line.chars() {
                if chunk.chars().count() == max {
                    parts.push(chunk.clone());
                    chunk.clear();
                }
                chunk.push(c);
            }
            cur = chunk;
        }
    }
    flush(&mut cur, &mut parts);
    parts
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

/// 2026-10-08 : fusionne l'ancien salon `discussion` dans `entraide` (8 messages, aucun échange entre membres
/// depuis le 15/09). Appelée à chaque ouverture de la base, sans effet quand il ne reste rien à fusionner :
/// - aucun message n'est supprimé ni modifié, il change seulement de salon (mêmes identifiants, mêmes dates) ;
/// - le repère de lecture de chaque membre devient le plus récent des deux (`MAX`) : un membre qui avait tout
///   lu dans Discussion n'a pas d'Entraide « non lu » en plus (contrepartie : avec un seul repère par salon,
///   un message d'Entraide plus ancien que son repère de Discussion passe pour lu) ;
/// - une seule transaction : un arrêt en cours de route laisse la base telle qu'elle était.
fn merge_discussion(conn: &Connection) -> Result<()> {
    let todo: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM messages WHERE channel = 'discussion')
              + (SELECT COUNT(*) FROM reads WHERE channel = 'discussion')",
        [],
        |r| r.get(0),
    )?;
    if todo == 0 {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         INSERT INTO reads (user_id, channel, last_read_id)
           SELECT user_id, 'entraide', last_read_id FROM reads WHERE channel = 'discussion' AND true
           ON CONFLICT(user_id, channel) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id);
         DELETE FROM reads WHERE channel = 'discussion';
         UPDATE messages SET channel = 'entraide' WHERE channel = 'discussion';
         COMMIT;",
    )?;
    Ok(())
}

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
        merge_discussion(&conn)?;
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

    /// Première visite d'un compte (2026-10-08) : dans chaque salon public où il n'a encore aucun repère de
    /// lecture, tout ce qui a plus de `max_age_days` jours compte comme lu. Un compte neuf voyait « 9 non
    /// lus » (toutes les annonces depuis la création du tchat). Le repère est écrit une fois pour toutes :
    /// ce qui est publié ensuite reste non lu, quel que soit le temps passé avant d'ouvrir le tchat. Les
    /// fils privés ne sont pas concernés (un message de l'admin est toujours pour le membre).
    pub fn init_reads(&self, user_id: &str, now: i64, max_age_days: i64) -> Result<()> {
        let db = self.db();
        let known: i64 = db.query_row(
            "SELECT COUNT(*) FROM reads WHERE user_id = ?1 AND channel IN ('annonces', 'entraide')",
            [user_id],
            |r| r.get(0),
        )?;
        if known >= Channel::PUBLIC.len() as i64 {
            return Ok(()); // cas courant : aucune écriture
        }
        let cutoff = now - max_age_days.max(0) * 86_400;
        for ch in Channel::PUBLIC {
            db.execute(
                "INSERT INTO reads (user_id, channel, last_read_id)
                 SELECT ?1, ?2, COALESCE((SELECT MAX(id) FROM messages WHERE channel = ?2 AND created_at < ?3), 0)
                 WHERE NOT EXISTS (SELECT 1 FROM reads WHERE user_id = ?1 AND channel = ?2)",
                params![user_id, ch.key(), cutoff],
            )?;
        }
        Ok(())
    }

    /// Messages privés non lus de TOUS les fils (somme de `PrivateThread::unread`), en une requête : le
    /// compteur de la bulle d'un modérateur n'a pas besoin de la liste des fils. Réservé aux modérateurs :
    /// un membre ne compte que son propre fil (`unread` sur son salon privé).
    pub fn unread_private_total(&self, reader_id: &str) -> Result<i64> {
        Ok(self.db().query_row(
            "SELECT COUNT(*) FROM messages m
             WHERE m.channel LIKE 'prive:%' AND m.deleted_at IS NULL AND m.author_id <> ?1
               AND m.id > COALESCE((SELECT last_read_id FROM reads r WHERE r.user_id = ?1 AND r.channel = m.channel), 0)",
            [reader_id],
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
        "Aide et annonces Groscailloux : {} nouveau(x) message(s) ({})",
        msgs.len(),
        parts.join(", ")
    );
    let mut body = String::from("Nouveaux messages dans Aide et annonces :\n\n");
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
        "Repondre : {jellyfin_url} (bulle \"Aide et annonces\" en haut a droite).\n"
    ));
    (subject, body)
}

/// Mail d'une annonce aux membres : (sujet, corps).
pub fn announcement_mail(author: &str, text: &str, jellyfin_url: &str) -> (String, String) {
    let first: String = text.lines().next().unwrap_or("").chars().take(70).collect();
    let subject = format!("Groscailloux : {first}");
    let body = format!(
        "{text}\n\n— {author}\n\nAnnonce publiee dans \"Aide et annonces\" sur Groscailloux : {jellyfin_url}\n\
         (bulle en haut a droite, onglet Annonces).\n"
    );
    (subject, body)
}

/// Mail d'un message privé de l'admin à un membre : (sujet, corps).
pub fn direct_mail(author: &str, member: &str, text: &str, jellyfin_url: &str) -> (String, String) {
    let subject = "Groscailloux : un message de l'admin pour toi".to_string();
    let body = format!(
        "Salut {member},\n\n{text}\n\n— {author}\n\nTu peux repondre dans \"Aide et annonces\" sur Groscailloux : {jellyfin_url}\n\
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
    fn split_parts_prefers_paragraphs() {
        assert_eq!(split_parts("court", 100), vec!["court"]);
        let long = "aaa\n\nbbb\n\nccc";
        assert_eq!(split_parts(long, 8), vec!["aaa\n\nbbb", "ccc"]);
        assert_eq!(split_parts(long, 4), vec!["aaa", "bbb", "ccc"]);
        // paragraphe trop long : ligne par ligne, puis au caractère
        assert_eq!(split_parts("l1\nl2\nl3", 5), vec!["l1\nl2", "l3"]);
        assert_eq!(split_parts("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert!(split_parts("\n\n  \n", 10).is_empty());
        for p in split_parts(&"mot ".repeat(1000), 200) {
            assert!(p.chars().count() <= 200);
        }
    }

    #[test]
    fn channel_keys_round_trip_and_reject_junk() {
        for k in ["annonces", "entraide", "prive:abc123"] {
            assert_eq!(Channel::parse(k).unwrap().key(), k);
        }
        // ancien salon fusionné : une page restée ouverte le demande encore, elle arrive dans Entraide
        assert_eq!(Channel::parse("discussion"), Some(Channel::Entraide));
        assert_eq!(Channel::parse("discussion").unwrap().key(), "entraide");
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
        assert!(can_read(&a, &Channel::Entraide) && can_post(&a, &Channel::Entraide));
        // l'ancien nom donne les mêmes droits que le salon fusionné
        let old = Channel::parse("discussion").unwrap();
        assert!(can_read(&a, &old) && can_post(&a, &old));
        // seuls les modérateurs écrivent dans Annonces, un membre n'y fait que lire
        assert!(can_read(&b, &Channel::Annonces) && !can_post(&b, &Channel::Annonces));
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

    /// 2026-10-08 : la bulle et le panneau s'appellent « Aide et annonces » ; les trois mails (récapitulatif de
    /// l'admin, annonce, message privé) disent le même nom, plus « le tchat », et pointent le bon onglet.
    #[test]
    fn mails_use_the_panel_name() {
        let a = user("a", false);
        let msg = Message {
            id: 1,
            channel: "entraide".into(),
            author_id: a.id.clone(),
            author_name: a.name.clone(),
            author_moderator: false,
            body: "ca saccade".into(),
            created_at: 100,
            deleted: false,
        };
        let digest = moderator_digest(&[msg], "https://jf");
        assert!(digest.0.contains("Aide et annonces") && digest.1.contains("Aide et annonces"));
        let announce = announcement_mail("Admin", "Maintenance dimanche", "https://jf");
        assert!(announce.0.starts_with("Groscailloux : Maintenance"));
        assert!(announce.1.contains("Aide et annonces") && announce.1.contains("onglet Annonces"));
        let direct = direct_mail("Admin", "Nina", "salut", "https://jf");
        assert!(direct.1.contains("Aide et annonces") && direct.1.contains("Ecrire a l'admin"));
        for (subject, body) in [&digest, &announce, &direct] {
            for t in [subject, body] {
                assert!(!t.to_lowercase().contains("tchat"), "ancien nom : {t}");
            }
        }
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
            s.insert(&Channel::Entraide, &a, &format!("m{i}"), i)
                .unwrap();
        }
        let last2 = s.list(&Channel::Entraide, None, None, 2).unwrap();
        assert_eq!(
            last2.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["m3", "m4"]
        );
        let after = s
            .list(&Channel::Entraide, Some(last2[0].id), None, 50)
            .unwrap();
        assert_eq!(after.len(), 1);
        let before = s
            .list(&Channel::Entraide, None, Some(last2[0].id), 50)
            .unwrap();
        assert_eq!(before.len(), 3);
    }

    /// Base « d'avant la fusion » : le schéma, puis des messages et des repères de lecture dans `discussion`.
    fn old_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for (ch, who, body, at, deleted) in [
            ("annonces", "mod", "news", 10, false),
            ("entraide", "a", "e1", 20, false),
            ("discussion", "a", "d1", 30, false),
            ("discussion", "b", "d2 supprimé", 40, true),
            ("discussion", "b", "d3", 50, false),
            ("entraide", "b", "e2", 60, false),
        ] {
            conn.execute(
                "INSERT INTO messages (channel, author_id, author_name, author_moderator, body, created_at, deleted_at)
                 VALUES (?1, ?2, ?2, 0, ?3, ?4, ?5)",
                params![ch, who, body, at, deleted.then_some(99)],
            )
            .unwrap();
        }
        // a a tout lu dans Discussion (jusqu'au 5) mais seulement le 1er message d'Entraide ; b a lu Discussion
        // jusqu'au 3 ; c n'a lu que Discussion (jusqu'au 4) et la 1re annonce
        for (u, ch, id) in [
            ("a", "discussion", 5),
            ("a", "entraide", 1),
            ("b", "discussion", 3),
            ("c", "discussion", 4),
            ("c", "annonces", 1),
        ] {
            conn.execute(
                "INSERT INTO reads (user_id, channel, last_read_id) VALUES (?1, ?2, ?3)",
                params![u, ch, id],
            )
            .unwrap();
        }
        conn
    }

    fn channel_counts(s: &ChatStore) -> Vec<(String, i64)> {
        let db = s.db();
        let mut st = db
            .prepare("SELECT channel, COUNT(*) FROM messages GROUP BY channel ORDER BY channel")
            .unwrap();
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    fn read_marker(s: &ChatStore, user: &str, channel: &str) -> Option<i64> {
        s.db()
            .query_row(
                "SELECT last_read_id FROM reads WHERE user_id = ?1 AND channel = ?2",
                params![user, channel],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
    }

    #[test]
    fn discussion_is_merged_into_entraide_without_loss() {
        let s = ChatStore::init(old_db()).unwrap();
        assert_eq!(
            channel_counts(&s),
            vec![("annonces".to_string(), 1), ("entraide".to_string(), 5)],
            "les 3 messages de Discussion (dont un supprimé) rejoignent les 2 d'Entraide"
        );
        let all = s.list(&Channel::Entraide, None, None, 50).unwrap();
        assert_eq!(
            all.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["e1", "d1", "", "d3", "e2"],
            "mêmes identifiants, donc même ordre ; le supprimé reste supprimé"
        );
        assert!(all[2].deleted && all[2].author_id == "b");
        assert_eq!(
            all.iter().map(|m| m.created_at).collect::<Vec<_>>(),
            [20, 30, 40, 50, 60],
            "dates intactes"
        );
        // l'ancien nom lit le même salon
        let via_old = s
            .list(&Channel::parse("discussion").unwrap(), None, None, 50)
            .unwrap();
        assert_eq!(via_old, all);
    }

    #[test]
    fn merge_keeps_the_most_recent_read_marker_per_member() {
        let s = ChatStore::init(old_db()).unwrap();
        assert_eq!(
            read_marker(&s, "a", "entraide"),
            Some(5),
            "Discussion (5) > Entraide (1)"
        );
        assert_eq!(
            read_marker(&s, "b", "entraide"),
            Some(3),
            "repère repris tel quel"
        );
        assert_eq!(read_marker(&s, "c", "entraide"), Some(4));
        assert_eq!(
            read_marker(&s, "c", "annonces"),
            Some(1),
            "les autres salons ne bougent pas"
        );
        for u in ["a", "b", "c"] {
            assert_eq!(
                read_marker(&s, u, "discussion"),
                None,
                "plus de repère pour un salon qui n'existe plus"
            );
        }
        // a avait tout lu : seul e2 (id 6, écrit par b) reste à lire
        assert_eq!(s.unread("a", &Channel::Entraide).unwrap(), 1);
        // b : son repère (3) couvre e1 et d1 ; d2 est supprimé, d3 et e2 sont de lui. Contrepartie assumée de
        // l'unique repère par salon : e1 (id 2), jamais ouvert dans Entraide, passe pour lu.
        assert_eq!(s.unread("b", &Channel::Entraide).unwrap(), 0);
        // c : repère 4, il reste d3 et e2
        assert_eq!(s.unread("c", &Channel::Entraide).unwrap(), 2);
    }

    #[test]
    fn merge_is_idempotent_across_reopenings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO messages (channel, author_id, author_name, author_moderator, body, created_at)
                 VALUES ('discussion', 'a', 'A', 0, 'avant', 1)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO reads VALUES ('a', 'discussion', 1)", [])
                .unwrap();
        }
        let first = {
            let s = ChatStore::open(&path).unwrap();
            let v = s.list(&Channel::Entraide, None, None, 50).unwrap();
            // un message écrit sous l'ancien nom par une page restée ouverte : il va dans Entraide
            let m = s
                .insert(
                    &Channel::parse("discussion").unwrap(),
                    &user("a", false),
                    "après",
                    2,
                )
                .unwrap();
            assert_eq!(m.channel, "entraide");
            v
        };
        let s = ChatStore::open(&path).unwrap(); // deuxième ouverture : rien à refaire
        let again = s.list(&Channel::Entraide, None, None, 50).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(again.len(), 2);
        assert_eq!(again[0], first[0], "le premier message n'a pas bougé");
        assert_eq!(channel_counts(&s), vec![("entraide".to_string(), 2)]);
        // et une base déjà fusionnée, ouverte une troisième fois, reste identique
        drop(s);
        let s = ChatStore::open(&path).unwrap();
        assert_eq!(s.list(&Channel::Entraide, None, None, 50).unwrap(), again);
    }

    #[test]
    fn merge_on_a_fresh_database_does_nothing() {
        let s = ChatStore::open_in_memory().unwrap();
        assert_eq!(s.max_id().unwrap(), 0);
        s.insert(&Channel::Entraide, &user("a", false), "x", 1)
            .unwrap();
        merge_discussion(&s.db()).unwrap();
        assert_eq!(channel_counts(&s), vec![("entraide".to_string(), 1)]);
    }

    const DAY: i64 = 86_400;

    #[test]
    fn new_account_does_not_see_old_announcements_as_unread() {
        let s = ChatStore::open_in_memory().unwrap();
        let m = user("mod", true);
        let now = 100 * DAY;
        // 9 annonces : 7 de plus de 14 jours, une d'il y a 12 jours, une d'hier
        for i in 0..7 {
            s.insert(
                &Channel::Annonces,
                &m,
                &format!("vieille {i}"),
                now - (30 - i) * DAY,
            )
            .unwrap();
        }
        s.insert(&Channel::Annonces, &m, "il y a 12 jours", now - 12 * DAY)
            .unwrap();
        let hier = s.insert(&Channel::Annonces, &m, "hier", now - DAY).unwrap();
        assert_eq!(
            s.unread("neuf", &Channel::Annonces).unwrap(),
            9,
            "avant la première visite : tout est non lu"
        );
        s.init_reads("neuf", now, 14).unwrap();
        assert_eq!(
            s.unread("neuf", &Channel::Annonces).unwrap(),
            2,
            "seules les annonces des 14 derniers jours restent non lues"
        );
        // le repère est figé : le temps qui passe ne change rien
        s.init_reads("neuf", now + 60 * DAY, 14).unwrap();
        assert_eq!(s.unread("neuf", &Channel::Annonces).unwrap(), 2);
        // ce qui est publié ensuite reste non lu
        s.insert(&Channel::Annonces, &m, "nouvelle", now + 60 * DAY)
            .unwrap();
        assert_eq!(s.unread("neuf", &Channel::Annonces).unwrap(), 3);
        s.mark_read("neuf", &Channel::Annonces, hier.id).unwrap();
        assert_eq!(s.unread("neuf", &Channel::Annonces).unwrap(), 1);
    }

    #[test]
    fn first_visit_threshold_is_exclusive_and_configurable() {
        let s = ChatStore::open_in_memory().unwrap();
        let m = user("mod", true);
        let now = 100 * DAY;
        // dans l'ordre du temps, comme en production (l'identifiant croît avec la date)
        s.insert(&Channel::Annonces, &m, "14 j et 1 s", now - 14 * DAY - 1)
            .unwrap();
        s.insert(&Channel::Annonces, &m, "pile 14 j", now - 14 * DAY)
            .unwrap();
        s.init_reads("a", now, 14).unwrap();
        assert_eq!(
            s.unread("a", &Channel::Annonces).unwrap(),
            1,
            "« de plus de 14 jours » : 14 jours pile restent non lus"
        );
        s.init_reads("b", now, 0).unwrap();
        assert_eq!(
            s.unread("b", &Channel::Annonces).unwrap(),
            0,
            "0 jour : tout est lu à la première visite"
        );
        s.init_reads("c", now, -5).unwrap(); // valeur absurde : traitée comme 0, jamais de panique
        assert_eq!(s.unread("c", &Channel::Annonces).unwrap(), 0);
    }

    #[test]
    fn init_reads_never_overrides_and_skips_private_threads() {
        let s = ChatStore::open_in_memory().unwrap();
        let (m, a) = (user("mod", true), user("a", false));
        let now = 100 * DAY;
        let old = s
            .insert(&Channel::Annonces, &m, "vieille", now - 40 * DAY)
            .unwrap();
        s.insert(&Channel::Annonces, &m, "récente", now - DAY)
            .unwrap();
        s.insert(&Channel::Entraide, &a, "ancien conseil", now - 50 * DAY)
            .unwrap();
        s.insert(&Channel::Entraide, &a, "récent conseil", now - 2 * DAY)
            .unwrap();
        let pn = Channel::Private("neuf".into());
        s.insert(&pn, &m, "bienvenue, écris-moi si besoin", now - 60 * DAY)
            .unwrap();
        // un compte qui avait déjà un repère dans Annonces mais pas dans Entraide : le premier est intact
        s.mark_read("ancien", &Channel::Annonces, old.id).unwrap();
        s.init_reads("ancien", now, 14).unwrap();
        assert_eq!(read_marker(&s, "ancien", "annonces"), Some(old.id));
        assert_eq!(s.unread("ancien", &Channel::Annonces).unwrap(), 1);
        assert_eq!(
            s.unread("ancien", &Channel::Entraide).unwrap(),
            1,
            "repère Entraide posé à la première visite de ce salon : seul le récent reste non lu"
        );
        s.init_reads("neuf", now, 14).unwrap();
        assert_eq!(s.unread("neuf", &Channel::Entraide).unwrap(), 1);
        assert_eq!(
            s.unread("neuf", &pn).unwrap(),
            1,
            "un message de l'admin reste non lu, même ancien"
        );
        assert_eq!(read_marker(&s, "neuf", "prive:neuf"), None);
    }

    #[test]
    fn unread_private_total_matches_the_threads() {
        let s = ChatStore::open_in_memory().unwrap();
        let (m, a, b) = (user("mod", true), user("a", false), user("b", false));
        let (pa, pb) = (Channel::Private("a".into()), Channel::Private("b".into()));
        s.insert(&pa, &a, "souci 1", 1).unwrap();
        s.insert(&pa, &a, "souci 2", 2).unwrap();
        let sup = s.insert(&pa, &a, "oups", 3).unwrap();
        s.mark_deleted(sup.id, "a", 4).unwrap();
        s.insert(&pb, &b, "question", 5).unwrap();
        s.insert(&pb, &m, "réponse", 6).unwrap(); // répondre = avoir lu : ce fil n'a plus rien de non lu
        let c = user("c", false);
        let pc = Channel::Private("c".into());
        s.insert(&pc, &c, "bonjour", 7).unwrap();
        let sum = |reader: &str| -> i64 {
            s.private_threads(reader)
                .unwrap()
                .iter()
                .map(|t| t.unread)
                .sum()
        };
        assert_eq!(
            s.unread_private_total("mod").unwrap(),
            3,
            "souci 1 et 2, bonjour"
        );
        assert_eq!(s.unread_private_total("mod").unwrap(), sum("mod"));
        s.mark_read("mod", &pa, 3).unwrap();
        assert_eq!(s.unread_private_total("mod").unwrap(), 1);
        assert_eq!(s.unread_private_total("mod").unwrap(), sum("mod"));
        s.mark_read("mod", &pc, 7).unwrap();
        assert_eq!(s.unread_private_total("mod").unwrap(), 0);
        // un membre n'appelle jamais ceci (il ne voit que son fil, `unread` sur son salon privé)
        assert_eq!(s.unread("b", &pb).unwrap(), 1, "la réponse du modérateur");
        assert_eq!(s.unread("a", &pa).unwrap(), 0);
    }
}
