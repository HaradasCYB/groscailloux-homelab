//! Abonnés et cycle premium (v1.18, 2026-09-20).
//!
//! Une fiche par compte Jellyfin dans `state/subscriptions.db` (SQLite, sauvegardée par
//! `homelabctl backup`) : statut, échéance, source (manuel, PayPal, essai), abonnement PayPal lié,
//! parrainage, et un historique horodaté de chaque changement. Les décisions du cycle
//! ([`decide`]) sont pures et testées ; leur application (suspension, mails) vit dans la tâche
//! `subscription_cycle`. Premium = compte Jellyfin actif ([`crate::accounts::set_premium`]) :
//! ce module n'y touche jamais directement.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::config::Subscriptions as SubsConfig;

pub const DAY: i64 = 86_400;

/// Statut d'une fiche. `Unknown` = compte actif trouvé sans abonnement connu (« à qualifier ») :
/// le cycle ne le suspend jamais, l'admin tranche depuis `/accounts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Trial,
    Active,
    Grace,
    Suspended,
    Offered,
    Exempt,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Trial => "trial",
            Status::Active => "active",
            Status::Grace => "grace",
            Status::Suspended => "suspended",
            Status::Offered => "offered",
            Status::Exempt => "exempt",
            Status::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "trial" => Status::Trial,
            "active" => Status::Active,
            "grace" => Status::Grace,
            "suspended" => Status::Suspended,
            "offered" => Status::Offered,
            "exempt" => Status::Exempt,
            "unknown" => Status::Unknown,
            _ => return None,
        })
    }

    /// Libellé affiché aux membres et à l'admin.
    pub fn label(self) -> &'static str {
        match self {
            Status::Trial => "Essai",
            Status::Active => "Actif",
            Status::Grace => "Échéance dépassée",
            Status::Suspended => "Suspendu",
            Status::Offered => "Offert",
            Status::Exempt => "Exempté",
            Status::Unknown => "À qualifier",
        }
    }

    /// Le compte doit-il être premium (actif) dans Jellyfin ?
    pub fn wants_premium(self) -> bool {
        !matches!(self, Status::Suspended)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Subscriber {
    pub user_id: String,
    pub username: String,
    pub status: Status,
    pub starts_at: i64,
    /// Fin de la période payée (ou de l'essai). Absente pour offert/exempt/à qualifier.
    pub expires_at: Option<i64>,
    /// `manual`, `paypal`, `trial`, `import`.
    pub source: String,
    pub paypal_sub_id: Option<String>,
    pub paypal_email: Option<String>,
    pub referral_code: String,
    pub referred_by: Option<String>,
    pub referral_credited: bool,
    pub note: String,
    pub reminded: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: i64,
    pub at: i64,
    pub user_id: String,
    pub kind: String,
    pub detail: String,
    pub actor: String,
}

/// Ce que le cycle doit faire pour une fiche, dans l'ordre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Rappel « ton accès se termine dans N jours » (N = valeur de `remind_days`).
    Remind(i64),
    /// Échéance passée, encore dans le délai de grâce.
    ToGrace,
    /// Grâce écoulée : suspendre (compte Jellyfin désactivé, rien de supprimé).
    Suspend,
}

/// Décisions pures pour une fiche à l'instant `now`.
pub fn decide(s: &Subscriber, now: i64, cfg: &SubsConfig) -> Vec<Action> {
    let mut out = Vec::new();
    let Some(exp) = s.expires_at else {
        return out;
    };
    match s.status {
        // « offert » avec une échéance = cadeau limité dans le temps, traité comme un abonnement
        Status::Trial | Status::Active | Status::Grace | Status::Offered => {}
        _ => return out,
    }
    let grace_end = exp + cfg.grace_days as i64 * DAY;
    if now >= grace_end {
        out.push(Action::Suspend);
        return out;
    }
    if now >= exp {
        if s.status != Status::Grace {
            out.push(Action::ToGrace);
        }
        return out;
    }
    // rappels : du plus proche au plus lointain, un seul par passage, jamais deux fois le même
    let mut days: Vec<i64> = cfg.remind_days.iter().map(|d| *d as i64).collect();
    days.sort_unstable();
    for d in days {
        if exp - now > d * DAY {
            continue; // ce palier n'est pas encore atteint, on regarde le suivant (plus lointain)
        }
        let already = s.reminded & (1 << d.min(62)) != 0;
        if !already {
            out.push(Action::Remind(d));
        }
        break; // le palier le plus proche décide seul : un J-7 après un J-1 n'aurait pas de sens
    }
    out
}

/// Nouvelle échéance après un paiement : la prochaine facturation annoncée par PayPal si elle est
/// connue et dans le futur, sinon `period_days` à partir de la fin de la période en cours (ou de
/// maintenant si elle est déjà passée).
pub fn next_expiry(
    current: Option<i64>,
    next_billing: Option<i64>,
    now: i64,
    cfg: &SubsConfig,
) -> i64 {
    if let Some(nb) = next_billing.filter(|t| *t > now) {
        return nb;
    }
    let base = current.filter(|c| *c > now).unwrap_or(now);
    base + cfg.period_days as i64 * DAY
}

/// Jours de parrainage accordables cette année (plafond `referral_cap_days_per_year`).
pub fn referral_allowance(credited_last_year: i64, cfg: &SubsConfig) -> i64 {
    (cfg.referral_cap_days_per_year as i64 - credited_last_year).clamp(0, cfg.referral_days as i64)
}

/// Code de parrainage : 8 caractères sans ambiguïté (pas de 0/O, 1/I).
pub fn new_referral_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

pub fn valid_referral_code(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Identifiant d'abonnement PayPal : `I-` suivi de 10 à 20 caractères alphanumériques.
pub fn valid_paypal_sub_id(s: &str) -> bool {
    s.len() >= 12
        && s.len() <= 24
        && s.starts_with("I-")
        && s[2..].bytes().all(|b| b.is_ascii_alphanumeric())
}

pub struct SubStore {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS subscribers (
  user_id TEXT PRIMARY KEY,
  username TEXT NOT NULL,
  status TEXT NOT NULL,
  starts_at INTEGER NOT NULL,
  expires_at INTEGER,
  source TEXT NOT NULL DEFAULT 'manual',
  paypal_sub_id TEXT,
  paypal_email TEXT,
  referral_code TEXT NOT NULL UNIQUE,
  referred_by TEXT,
  referral_credited INTEGER NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT '',
  reminded INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS subscribers_paypal ON subscribers(paypal_sub_id);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  user_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  detail TEXT NOT NULL,
  actor TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_user ON events(user_id, id);
CREATE TABLE IF NOT EXISTS paypal_events (
  event_id TEXT PRIMARY KEY,
  at INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  sub_id TEXT,
  summary TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS referral_credits (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  user_id TEXT NOT NULL,
  days INTEGER NOT NULL,
  reason TEXT NOT NULL
);
";

const COLS: &str = "user_id, username, status, starts_at, expires_at, source, paypal_sub_id, paypal_email, referral_code, referred_by, referral_credited, note, reminded, updated_at";

fn row_to_sub(r: &rusqlite::Row<'_>) -> rusqlite::Result<Subscriber> {
    let status: String = r.get(2)?;
    Ok(Subscriber {
        user_id: r.get(0)?,
        username: r.get(1)?,
        status: Status::parse(&status).unwrap_or(Status::Unknown),
        starts_at: r.get(3)?,
        expires_at: r.get(4)?,
        source: r.get(5)?,
        paypal_sub_id: r.get(6)?,
        paypal_email: r.get(7)?,
        referral_code: r.get(8)?,
        referred_by: r.get(9)?,
        referral_credited: r.get::<_, i64>(10)? != 0,
        note: r.get(11)?,
        reminded: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

impl SubStore {
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

    pub fn get(&self, user_id: &str) -> Result<Option<Subscriber>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLS} FROM subscribers WHERE user_id = ?1"),
                params![user_id],
                row_to_sub,
            )
            .optional()?)
    }

    pub fn by_username(&self, username: &str) -> Result<Option<Subscriber>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLS} FROM subscribers WHERE lower(username) = lower(?1)"),
                params![username],
                row_to_sub,
            )
            .optional()?)
    }

    pub fn by_paypal_sub(&self, sub_id: &str) -> Result<Option<Subscriber>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLS} FROM subscribers WHERE paypal_sub_id = ?1"),
                params![sub_id],
                row_to_sub,
            )
            .optional()?)
    }

    pub fn by_referral_code(&self, code: &str) -> Result<Option<Subscriber>> {
        Ok(self
            .db()
            .query_row(
                &format!("SELECT {COLS} FROM subscribers WHERE referral_code = upper(?1)"),
                params![code],
                row_to_sub,
            )
            .optional()?)
    }

    pub fn list(&self) -> Result<Vec<Subscriber>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {COLS} FROM subscribers ORDER BY lower(username)"
        ))?;
        let rows = st.query_map([], row_to_sub)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Crée la fiche si elle n'existe pas (statut donné), sinon renvoie l'existante.
    pub fn ensure(
        &self,
        user_id: &str,
        username: &str,
        status: Status,
        expires_at: Option<i64>,
        source: &str,
        now: i64,
    ) -> Result<Subscriber> {
        if let Some(s) = self.get(user_id)? {
            return Ok(s);
        }
        let mut code = new_referral_code();
        while self.by_referral_code(&code)?.is_some() {
            code = new_referral_code();
        }
        self.db().execute(
            "INSERT INTO subscribers (user_id, username, status, starts_at, expires_at, source, referral_code, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?4)",
            params![user_id, username, status.as_str(), now, expires_at, source, code],
        )?;
        self.log(
            user_id,
            "created",
            &format!("{} ({source})", status.label()),
            "system",
            now,
        )?;
        self.get(user_id)?.context("fiche créée introuvable")
    }

    pub fn set_username(&self, user_id: &str, username: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET username = ?2, updated_at = ?3 WHERE user_id = ?1",
            params![user_id, username, now],
        )?;
        Ok(())
    }

    /// Change statut et échéance (l'échéance `None` la conserve ; `Some(None)` l'efface).
    #[allow(clippy::too_many_arguments)]
    pub fn set_status(
        &self,
        user_id: &str,
        status: Status,
        expires_at: Option<Option<i64>>,
        source: Option<&str>,
        actor: &str,
        detail: &str,
        now: i64,
    ) -> Result<()> {
        let db = self.db();
        match expires_at {
            Some(e) => db.execute(
                "UPDATE subscribers SET status = ?2, expires_at = ?3, reminded = 0, source = COALESCE(?4, source), updated_at = ?5 WHERE user_id = ?1",
                params![user_id, status.as_str(), e, source, now],
            )?,
            None => db.execute(
                "UPDATE subscribers SET status = ?2, source = COALESCE(?3, source), updated_at = ?4 WHERE user_id = ?1",
                params![user_id, status.as_str(), source, now],
            )?,
        };
        drop(db);
        self.log(
            user_id,
            "status",
            &format!("{} — {detail}", status.label()),
            actor,
            now,
        )
    }

    pub fn set_reminded(&self, user_id: &str, day: i64, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET reminded = reminded | ?2, updated_at = ?3 WHERE user_id = ?1",
            params![user_id, 1i64 << day.min(62), now],
        )?;
        Ok(())
    }

    pub fn set_note(&self, user_id: &str, note: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET note = ?2, updated_at = ?3 WHERE user_id = ?1",
            params![user_id, note, now],
        )?;
        Ok(())
    }

    pub fn link_paypal(
        &self,
        user_id: &str,
        sub_id: &str,
        email: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<()> {
        if let Some(other) = self.by_paypal_sub(sub_id)? {
            if other.user_id != user_id {
                bail!(
                    "abonnement {sub_id} déjà rattaché au compte {}",
                    other.username
                );
            }
        }
        self.db().execute(
            "UPDATE subscribers SET paypal_sub_id = ?2, paypal_email = COALESCE(?3, paypal_email), source = 'paypal', updated_at = ?4 WHERE user_id = ?1",
            params![user_id, sub_id, email, now],
        )?;
        self.log(user_id, "paypal_linked", sub_id, actor, now)
    }

    pub fn set_referred_by(&self, user_id: &str, referrer_id: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET referred_by = ?2, updated_at = ?3 WHERE user_id = ?1 AND referred_by IS NULL",
            params![user_id, referrer_id, now],
        )?;
        Ok(())
    }

    pub fn mark_referral_credited(&self, user_id: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET referral_credited = 1, updated_at = ?2 WHERE user_id = ?1",
            params![user_id, now],
        )?;
        Ok(())
    }

    /// Prolonge l'échéance de `days` jours (à partir de l'échéance si elle est future, sinon de
    /// maintenant) ; une fiche sans échéance en reçoit une.
    pub fn extend(
        &self,
        user_id: &str,
        days: i64,
        actor: &str,
        reason: &str,
        now: i64,
    ) -> Result<i64> {
        let s = self.get(user_id)?.context("fiche introuvable")?;
        let base = s.expires_at.filter(|e| *e > now).unwrap_or(now);
        let new = base + days * DAY;
        self.db().execute(
            "UPDATE subscribers SET expires_at = ?2, reminded = 0, updated_at = ?3 WHERE user_id = ?1",
            params![user_id, new, now],
        )?;
        self.log(
            user_id,
            "extended",
            &format!("+{days} j — {reason}"),
            actor,
            now,
        )?;
        Ok(new)
    }

    pub fn add_referral_credit(
        &self,
        user_id: &str,
        days: i64,
        reason: &str,
        now: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO referral_credits (at, user_id, days, reason) VALUES (?1, ?2, ?3, ?4)",
            params![now, user_id, days, reason],
        )?;
        Ok(())
    }

    pub fn referral_credited_last_year(&self, user_id: &str, now: i64) -> Result<i64> {
        Ok(self.db().query_row(
            "SELECT COALESCE(SUM(days), 0) FROM referral_credits WHERE user_id = ?1 AND at > ?2",
            params![user_id, now - 365 * DAY],
            |r| r.get(0),
        )?)
    }

    pub fn remove(&self, user_id: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM subscribers WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn log(
        &self,
        user_id: &str,
        kind: &str,
        detail: &str,
        actor: &str,
        now: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO events (at, user_id, kind, detail, actor) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, user_id, kind, detail, actor],
        )?;
        Ok(())
    }

    pub fn history(&self, user_id: &str, limit: usize) -> Result<Vec<Event>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, at, user_id, kind, detail, actor FROM events WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![user_id, limit as i64], |r| {
            Ok(Event {
                id: r.get(0)?,
                at: r.get(1)?,
                user_id: r.get(2)?,
                kind: r.get(3)?,
                detail: r.get(4)?,
                actor: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Enregistre un événement PayPal ; `false` s'il avait déjà été traité (relivraison).
    pub fn record_paypal_event(
        &self,
        event_id: &str,
        event_type: &str,
        sub_id: Option<&str>,
        summary: &str,
        now: i64,
    ) -> Result<bool> {
        let n = self.db().execute(
            "INSERT OR IGNORE INTO paypal_events (event_id, at, event_type, sub_id, summary) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_id, now, event_type, sub_id, summary],
        )?;
        Ok(n == 1)
    }
}

/// Une ligne d'export CSV des abonnements PayPal (colonnes reconnues à la volée, en-tête libre).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvRow {
    pub sub_id: String,
    pub email: String,
    pub name: String,
    pub next_billing: Option<String>,
    pub status: String,
}

/// Lit un export PayPal (séparateur `,` ou `;`, guillemets simples) : on repère les colonnes par leur
/// nom (identifiant d'abonnement, e-mail, nom, prochaine facturation, statut) sans dépendre de l'ordre.
pub fn parse_paypal_csv(text: &str) -> Vec<CsvRow> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let sep = if header.matches(';').count() > header.matches(',').count() {
        ';'
    } else {
        ','
    };
    let head: Vec<String> = split_csv(header, sep)
        .into_iter()
        .map(|h| h.to_lowercase())
        .collect();
    let find = |keys: &[&str]| head.iter().position(|h| keys.iter().any(|k| h.contains(k)));
    let i_sub = find(&[
        "subscription id",
        "id d'abonnement",
        "identifiant d'abonnement",
        "billing agreement",
        "profile id",
        "id de profil",
    ]);
    let i_mail = find(&["email", "e-mail", "adresse"]);
    let i_name = find(&["name", "nom"]);
    let i_next = find(&["next billing", "next payment", "prochain", "prochaine"]);
    let i_status = find(&["status", "statut", "état"]);
    let mut out = Vec::new();
    for line in lines {
        let cells = split_csv(line, sep);
        let get = |i: Option<usize>| {
            i.and_then(|i| cells.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let sub_id = get(i_sub);
        let sub_id = cells
            .iter()
            .map(|c| c.trim())
            .find(|c| valid_paypal_sub_id(c))
            .map(str::to_string)
            .unwrap_or(sub_id);
        if !valid_paypal_sub_id(&sub_id) {
            continue;
        }
        out.push(CsvRow {
            sub_id,
            email: get(i_mail).to_lowercase(),
            name: get(i_name),
            next_billing: Some(get(i_next)).filter(|s| !s.is_empty()),
            status: get(i_status),
        });
    }
    out
}

fn split_csv(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Texte des mails et messages du cycle.
pub fn reminder_mail(username: &str, days: i64, status: Status, pay_url: &str) -> (String, String) {
    let quoi = match status {
        Status::Trial => "ton essai gratuit",
        Status::Offered => "ton accès offert",
        _ => "ton abonnement",
    };
    let quand = match days {
        0 | 1 => "demain".to_string(),
        d => format!("dans {d} jours"),
    };
    (
        format!("Groscailloux : {quoi} se termine {quand}"),
        format!(
            "Salut {username},\n\n{quoi} se termine {quand}. Pour garder l'accès sans coupure, tu peux {} ici :\n{pay_url}\n\nSi tu es déjà abonné et que ce message te surprend, réponds à ce mail : on regarde ensemble.\n\nÀ bientôt sur Groscailloux.",
            if status == Status::Trial { "t'abonner (3,50 € par mois, sans engagement)" } else { "renouveler ou vérifier ton abonnement" },
            quoi = capitalize(quoi),
        ),
    )
}

pub fn suspended_mail(username: &str, pay_url: &str) -> (String, String) {
    (
        "Groscailloux : ton accès est en pause".to_string(),
        format!(
            "Salut {username},\n\nTon abonnement est arrivé à échéance et ton accès est en pause. Rien n'est supprimé : ton historique, tes favoris et tes demandes t'attendent.\n\nPour reprendre, il suffit de t'abonner à nouveau :\n{pay_url}\n\nL'accès est rétabli automatiquement dès le paiement.\n\nÀ bientôt sur Groscailloux."
        ),
    )
}

pub fn activated_mail(
    username: &str,
    expires_at_text: &str,
    jellyfin_url: &str,
) -> (String, String) {
    (
        "Groscailloux : ton compte Premium est actif".to_string(),
        format!(
            "Salut {username},\n\nMerci ! Ton abonnement est enregistré : ton compte est actif jusqu'au {expires_at_text}, et se prolonge automatiquement à chaque paiement.\n\nC'est par ici : {jellyfin_url}\n\nBon visionnage."
        ),
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SubsConfig {
        SubsConfig::default()
    }

    fn sub(status: Status, expires_in_days: Option<f64>) -> Subscriber {
        Subscriber {
            user_id: "u".into(),
            username: "membre".into(),
            status,
            starts_at: 0,
            expires_at: expires_in_days.map(|d| 1_000_000 + (d * DAY as f64) as i64),
            source: "manual".into(),
            paypal_sub_id: None,
            paypal_email: None,
            referral_code: "ABCDEFGH".into(),
            referred_by: None,
            referral_credited: false,
            note: String::new(),
            reminded: 0,
            updated_at: 0,
        }
    }

    const NOW: i64 = 1_000_000;

    #[test]
    fn reminders_then_grace_then_suspend() {
        let c = cfg();
        assert_eq!(decide(&sub(Status::Active, Some(20.0)), NOW, &c), vec![]);
        assert_eq!(
            decide(&sub(Status::Active, Some(6.5)), NOW, &c),
            vec![Action::Remind(7)]
        );
        assert_eq!(
            decide(&sub(Status::Active, Some(0.5)), NOW, &c),
            vec![Action::Remind(1)]
        );
        let mut s = sub(Status::Active, Some(0.5));
        s.reminded = 1 << 1;
        assert_eq!(decide(&s, NOW, &c), vec![]);
        assert_eq!(
            decide(&sub(Status::Active, Some(-1.0)), NOW, &c),
            vec![Action::ToGrace]
        );
        assert_eq!(decide(&sub(Status::Grace, Some(-1.0)), NOW, &c), vec![]);
        assert_eq!(
            decide(&sub(Status::Grace, Some(-3.0)), NOW, &c),
            vec![Action::Suspend]
        );
        assert_eq!(
            decide(&sub(Status::Active, Some(-10.0)), NOW, &c),
            vec![Action::Suspend]
        );
    }

    #[test]
    fn exempt_unknown_suspended_never_touched_offered_only_with_expiry() {
        let c = cfg();
        for st in [Status::Exempt, Status::Unknown, Status::Suspended] {
            assert_eq!(decide(&sub(st, Some(-30.0)), NOW, &c), vec![], "{st:?}");
        }
        assert_eq!(decide(&sub(Status::Active, None), NOW, &c), vec![]);
        assert_eq!(decide(&sub(Status::Offered, None), NOW, &c), vec![]);
        assert_eq!(
            decide(&sub(Status::Offered, Some(-30.0)), NOW, &c),
            vec![Action::Suspend]
        );
        assert_eq!(
            decide(&sub(Status::Offered, Some(0.5)), NOW, &c),
            vec![Action::Remind(1)]
        );
    }

    #[test]
    fn expiry_follows_paypal_then_period() {
        let c = cfg();
        assert_eq!(
            next_expiry(None, Some(NOW + 5 * DAY), NOW, &c),
            NOW + 5 * DAY
        );
        assert_eq!(
            next_expiry(Some(NOW + 2 * DAY), None, NOW, &c),
            NOW + 2 * DAY + c.period_days as i64 * DAY
        );
        assert_eq!(
            next_expiry(Some(NOW - 2 * DAY), Some(NOW - DAY), NOW, &c),
            NOW + c.period_days as i64 * DAY
        );
    }

    #[test]
    fn referral_allowance_is_capped() {
        let c = cfg();
        assert_eq!(referral_allowance(0, &c), c.referral_days as i64);
        assert_eq!(
            referral_allowance(c.referral_cap_days_per_year as i64 - 5, &c),
            5
        );
        assert_eq!(
            referral_allowance(c.referral_cap_days_per_year as i64, &c),
            0
        );
    }

    #[test]
    fn store_round_trip_and_dedupe() {
        let st = SubStore::open_in_memory().unwrap();
        let s = st
            .ensure(
                "id1",
                "Alice",
                Status::Trial,
                Some(NOW + 7 * DAY),
                "trial",
                NOW,
            )
            .unwrap();
        assert!(valid_referral_code(&s.referral_code));
        assert_eq!(
            st.ensure("id1", "Alice", Status::Active, None, "manual", NOW)
                .unwrap()
                .status,
            Status::Trial
        );
        st.link_paypal("id1", "I-ABCDEFGHIJ12", Some("a@b.c"), "webhook", NOW)
            .unwrap();
        assert_eq!(
            st.by_paypal_sub("I-ABCDEFGHIJ12")
                .unwrap()
                .unwrap()
                .username,
            "Alice"
        );
        st.ensure("id2", "Bob", Status::Unknown, None, "import", NOW)
            .unwrap();
        assert!(st
            .link_paypal("id2", "I-ABCDEFGHIJ12", None, "admin", NOW)
            .is_err());
        let new = st.extend("id1", 30, "admin", "test", NOW).unwrap();
        assert_eq!(new, NOW + 37 * DAY);
        assert!(st
            .record_paypal_event(
                "WH-1",
                "PAYMENT.SALE.COMPLETED",
                Some("I-ABCDEFGHIJ12"),
                "",
                NOW
            )
            .unwrap());
        assert!(!st
            .record_paypal_event("WH-1", "PAYMENT.SALE.COMPLETED", None, "", NOW)
            .unwrap());
        st.set_status(
            "id1",
            Status::Suspended,
            Some(None),
            None,
            "cycle",
            "grâce écoulée",
            NOW,
        )
        .unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!((s.status, s.expires_at), (Status::Suspended, None));
        assert!(st.history("id1", 10).unwrap().len() >= 4);
        assert_eq!(
            st.by_referral_code(&s.referral_code.to_lowercase())
                .unwrap()
                .unwrap()
                .user_id,
            "id1"
        );
    }

    #[test]
    fn paypal_csv_is_parsed_whatever_the_column_order() {
        let text = "\"Nom\";\"Adresse e-mail\";\"Statut\";\"Identifiant d'abonnement\";\"Prochaine facturation\"\n\"Dupont, Jean\";\"j@ex.fr\";\"Actif\";\"I-ABCDEFGHIJ12\";\"2026-10-01\"\n\"Sans id\";\"x@y.z\";\"Actif\";\"\";\"\"\n";
        let rows = parse_paypal_csv(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0],
            CsvRow {
                sub_id: "I-ABCDEFGHIJ12".into(),
                email: "j@ex.fr".into(),
                name: "Dupont, Jean".into(),
                next_billing: Some("2026-10-01".into()),
                status: "Actif".into()
            }
        );
        assert!(valid_paypal_sub_id("I-ABCDEFGHIJ12"));
        assert!(!valid_paypal_sub_id("ABCDEFGHIJ12"));
    }
}
