//! Abonnés et cycle premium (v1.18, 2026-09-20).
//!
//! Une fiche par compte Jellyfin dans `state/subscriptions.db` (SQLite, sauvegardée par
//! `homelabctl backup`) : statut, échéance, source (manuel, PayPal, essai), abonnement PayPal lié,
//! parrainage, et un historique horodaté de chaque changement. Les décisions du cycle
//! ([`decide`]) sont pures et testées ; leur application (suspension, mails) vit dans la tâche
//! `subscription_cycle`. Premium = compte Jellyfin actif ([`crate::accounts::set_premium`]) :
//! ce module n'y touche jamais directement.
//!
//! Depuis le 2026-10-08 (décision du propriétaire : « les abonnés hors PayPal, je les gère moi-même ») le
//! cycle ne suit plus que les **essais de l'inscription publique** ([`is_trial`]) et les fiches **liées à
//! un abonnement PayPal** (actif ou arrêté chez PayPal). Toute autre fiche ([`managed_by_hand`] : actif,
//! offert ou essai posé par décision de l'admin, import, exempté, à qualifier) n'est jamais suspendue et
//! ne reçoit aucun rappel ; à son échéance, l'admin est prévenu une seule fois ([`Action::ManualDue`]) et
//! `/accounts` l'affiche « à gérer (échéance passée) ».

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

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
    /// Dernier statut connu de l'abonnement PayPal lié (`ACTIVE`, `CANCELLED`, `SUSPENDED`,
    /// `EXPIRED`…), tenu par les webhooks et le contrôle quotidien. Absent = pas encore relu.
    pub paypal_status: Option<String>,
    /// Date du dernier paiement PayPal appliqué à la fiche : le contrôle quotidien ne prolonge que
    /// pour un paiement plus récent.
    pub paypal_paid_at: Option<i64>,
    /// Fiche gérée à la main ([`managed_by_hand`]) : échéance pour laquelle l'admin a déjà été prévenu
    /// (2026-10-08). Une information par échéance : une nouvelle échéance (prolongation, nouvelle
    /// décision) en donnera une autre.
    pub due_noted: Option<i64>,
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
    /// Abonnement PayPal qui se renouvelle tout seul : simple information « se renouvelle le JJ/MM »,
    /// sans lien de paiement, une fois par période (palier le plus lointain de `remind_days`).
    RenewalNotice(i64),
    /// Échéance passée, encore dans le délai de grâce.
    ToGrace,
    /// Grâce écoulée : suspendre (compte Jellyfin désactivé, rien de supprimé).
    Suspend,
    /// Fiche gérée à la main ([`managed_by_hand`]) arrivée à échéance : une information à l'admin, une
    /// seule fois par échéance ; le compte reste actif, rien n'est écrit au membre (2026-10-08).
    ManualDue,
}

/// Statut PayPal d'un abonnement qui prélève encore.
pub const PAYPAL_ACTIVE: &str = "ACTIVE";
/// Statut noté quand PayPal ne connaît pas l'abonnement lié (identifiant de l'autre environnement,
/// sandbox ou live) : il ne prélève rien.
pub const PAYPAL_NOT_FOUND: &str = "NOT_FOUND";

/// Libellé d'un statut d'abonnement PayPal pour l'historique, `/accounts` et les alertes.
pub fn paypal_status_label(st: &str) -> &str {
    match st {
        PAYPAL_ACTIVE => "actif",
        "CANCELLED" => "annulé",
        "SUSPENDED" => "suspendu",
        "EXPIRED" => "expiré",
        PAYPAL_NOT_FOUND => "introuvable",
        "APPROVAL_PENDING" | "APPROVED" => "en attente d'approbation",
        other => other,
    }
}

/// La fiche se renouvelle-t-elle toute seule par PayPal ? Abonnement lié et pas arrêté chez PayPal
/// (statut encore inconnu = actif : le contrôle quotidien le renseigne). La source n'entre pas en
/// compte : une décision de l'admin sur la fiche n'arrête pas les prélèvements. Un tel membre ne
/// reçoit jamais « renouvelle ici » : il prendrait un second abonnement, donc deux prélèvements par
/// mois.
pub fn auto_renews(s: &Subscriber) -> bool {
    s.paypal_sub_id.is_some()
        && s.paypal_status
            .as_deref()
            .is_none_or(|st| st == PAYPAL_ACTIVE)
}

/// Abonnement déjà rattaché à la fiche, autre que `new_sub` : s'il prélève encore (PayPal fait foi),
/// `new_sub` est un second abonnement, refusé (ni rattaché ni compté).
pub fn other_linked_sub<'a>(s: &'a Subscriber, new_sub: &str) -> Option<&'a str> {
    s.paypal_sub_id.as_deref().filter(|l| *l != new_sub)
}

/// L'abonnement déjà rattaché prélève-t-il encore ? `lookup` = ce que PayPal en dit : `None` sans
/// client PayPal, `Some(None)` s'il ne le connaît pas (autre environnement : rien n'est prélevé) ou
/// n'en donne aucun statut, sinon son statut. Sans client, le dernier statut connu de la fiche
/// (`local`) ; inconnu = actif, par prudence (un second abonnement refusé à tort se rattache à la
/// main, un second prélèvement se rembourse).
pub fn linked_still_charges(lookup: Option<Option<&str>>, local: Option<&str>) -> bool {
    match lookup {
        Some(Some(st)) => st == PAYPAL_ACTIVE,
        Some(None) => false,
        None => local.is_none_or(|st| st == PAYPAL_ACTIVE),
    }
}

/// Clé `k` d'un lien `/premium?compte=…` émis par homelabd (mails de rappel et de suspension) :
/// HMAC-SHA256 du nom de compte (en minuscules, comme `by_username`) par un secret du serveur, 16
/// caractères hexa. `/premium` est public : sans cette clé, il dirait à n'importe qui qu'un pseudo
/// existe et paie par PayPal ; il n'affiche donc « déjà abonné » que pour un lien qui la porte.
pub fn premium_link_key(secret: &str, username: &str) -> String {
    let msg = format!("gc-premium-v1|{}", username.to_lowercase());
    hmac_sha256(secret.as_bytes(), msg.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `given` est-elle la clé de ce nom de compte ? Comparaison à temps constant.
pub fn premium_link_ok(secret: &str, username: &str, given: &str) -> bool {
    let want = premium_link_key(secret, username);
    want.len() == given.len()
        && want
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// HMAC-SHA256 (RFC 2104), même construction que la session d'administration de homelabd.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Essai gratuit de l'**inscription publique** (source `trial`, posée par `start_trial`), y compris
/// pendant la grâce qui suit sa fin : le passage en grâce (`set_status` sans source) garde la source
/// `trial`. Un « essai » posé par l'admin (`subs set --status trial`, case de `/accounts`) porte la
/// source `manual` : c'est une décision de l'admin, gérée à la main dès le départ (aucun rappel, une
/// information à l'échéance) — sinon il recevait les rappels « t'abonner » puis, passé en grâce avec la
/// source `manual`, n'était plus jamais suspendu (2026-10-08). Un essai prolongé par l'admin après sa
/// fin redevient « actif » (`admin_extend`) : c'est alors aussi une décision de l'admin.
pub fn is_trial(s: &Subscriber) -> bool {
    s.source == "trial" && matches!(s.status, Status::Trial | Status::Grace)
}

/// Fiche gérée à la main par l'admin (2026-10-08) : ni essai, ni abonnement PayPal lié. Source manuelle,
/// import (CSV ou compte trouvé sans abonnement), offert avec ou sans échéance, exempté, à qualifier :
/// homelabd ne voit aucun paiement pour elles, il ne les suspend donc jamais et ne leur écrit jamais.
/// Une fiche liée à PayPal le reste même arrêtée chez PayPal (rappels avec lien, grâce, suspension).
pub fn managed_by_hand(s: &Subscriber) -> bool {
    s.paypal_sub_id.is_none() && !is_trial(s)
}

/// Fiche gérée à la main dont l'échéance est passée : `/accounts` l'affiche « à gérer (échéance passée) ».
/// Seuls les statuts qui portent une échéance comptent (actif, grâce, offert, essai posé par l'admin) ;
/// un exempté ou un « à qualifier » n'en a pas de sens.
pub fn hand_due(s: &Subscriber, now: i64) -> bool {
    managed_by_hand(s)
        && matches!(
            s.status,
            Status::Active | Status::Grace | Status::Offered | Status::Trial
        )
        && s.expires_at.is_some_and(|e| now >= e)
}

/// Un événement de l'historique est-il montré au membre dans « Mon compte » ? Ni les événements PayPal
/// bruts, ni la note « échéance à gérer à la main » (`due_noted`, 2026-10-08), qui s'adresse à l'admin.
pub fn member_visible(kind: &str) -> bool {
    !matches!(kind, "paypal_event" | "due_noted")
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
    // 2026-10-08 : hors essai et hors PayPal, l'admin gère à la main. Ni rappel, ni grâce, ni
    // suspension : à l'échéance, une information à l'admin, une fois par échéance.
    if managed_by_hand(s) {
        if hand_due(s, now) && s.due_noted != Some(exp) {
            out.push(Action::ManualDue);
        }
        return out;
    }
    let auto = auto_renews(s);
    // prélèvement automatique : l'échéance est la date de facturation annoncée par PayPal, qui
    // encaisse parfois des heures plus tard ; la grâce ne commence qu'après la marge
    let end = if auto {
        exp + cfg.paypal_margin_hours as i64 * 3600
    } else {
        exp
    };
    let grace_end = end + cfg.grace_days as i64 * DAY;
    if now >= grace_end {
        out.push(Action::Suspend);
        return out;
    }
    if now >= end {
        if s.status != Status::Grace {
            out.push(Action::ToGrace);
        }
        return out;
    }
    if now >= exp {
        return out; // dans la marge : le prélèvement est attendu, rien à dire au membre
    }
    let mut days: Vec<i64> = cfg.remind_days.iter().map(|d| *d as i64).collect();
    days.sort_unstable();
    if auto {
        // une seule information par période, au palier le plus lointain, sans lien de paiement
        if let Some(&far) = days.last() {
            if exp - now <= far * DAY && s.reminded & (1 << far.min(62)) == 0 {
                out.push(Action::RenewalNotice(far));
            }
        }
        return out;
    }
    // rappels : du plus proche au plus lointain, un seul par passage, jamais deux fois le même
    for d in days {
        if exp - now > d * DAY {
            continue; // ce palier n'est pas encore atteint, on regarde le suivant (plus lointain)
        }
        // essai : un palier au moins égal à sa durée tomberait dès l'inscription (« se termine dans
        // 7 jours » une heure après le mail de bienvenue) ; les paliers suivants sont plus lointains
        if s.status == Status::Trial && d * DAY >= exp - s.starts_at {
            break;
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

/// Un crédit de parrainage peut-il prolonger cette fiche ? Jamais un accès sans échéance (offert sans
/// limite, exempté, à qualifier) : `extend` lui donnerait une échéance, donc une fin, puis une
/// suspension.
pub fn referral_extends(s: &Subscriber) -> bool {
    !(s.expires_at.is_none()
        && matches!(s.status, Status::Offered | Status::Exempt | Status::Unknown))
}

/// Fiches dont le compte Jellyfin a disparu : celles à retirer ce passage, et celles gardées.
#[derive(Debug, Default)]
pub struct OrphanPlan<'a> {
    pub remove: Vec<&'a Subscriber>,
    pub held: Vec<&'a Subscriber>,
}

impl OrphanPlan<'_> {
    /// Comptes des fiches gardées : le cycle n'y touche pas (ni mail ni suspension) tant que l'admin
    /// n'a pas tranché.
    pub fn held_ids(&self) -> HashSet<String> {
        self.held.iter().map(|s| s.user_id.clone()).collect()
    }
}

/// Fiche qui n'a rien à perdre : ni lien PayPal, ni échéance, et un statut qu'`ensure_fiches` recrée
/// tel quel si le compte revient (à qualifier, suspendu, exempté). Les comptes de banc (`zz_`…) en
/// sont, par dizaines.
pub fn orphan_disposable(s: &Subscriber) -> bool {
    s.paypal_sub_id.is_none()
        && s.expires_at.is_none()
        && matches!(
            s.status,
            Status::Unknown | Status::Suspended | Status::Exempt
        )
}

/// Rien n'est retiré si Jellyfin n'a renvoyé aucun compte (réponse vide ou inattendue). Sinon, les
/// fiches sans rien à perdre ([`orphan_disposable`]) partent toutes ; les autres (lien PayPal, échéance,
/// décision de l'admin) partent aussi, sauf si plus de `max` disparaîtraient d'un coup (« jamais de
/// purge globale ») : elles sont alors toutes gardées, l'admin tranche.
pub fn orphan_plan<'a>(
    fiches: &'a [Subscriber],
    jellyfin_ids: &HashSet<String>,
    max: usize,
) -> OrphanPlan<'a> {
    let orphans = fiches.iter().filter(|s| !jellyfin_ids.contains(&s.user_id));
    if jellyfin_ids.is_empty() {
        return OrphanPlan {
            remove: Vec::new(),
            held: orphans.collect(),
        };
    }
    let (disposable, valued): (Vec<&Subscriber>, Vec<&Subscriber>) =
        orphans.partition(|s| orphan_disposable(s));
    if valued.len() > max {
        return OrphanPlan {
            remove: disposable,
            held: valued,
        };
    }
    OrphanPlan {
        remove: disposable.into_iter().chain(valued).collect(),
        held: Vec::new(),
    }
}

/// Deux paiements d'un abonnement mensuel sont espacés d'au moins 28 jours : à moins de 12 h d'écart,
/// c'est le même (l'heure de la vente et celle du « dernier paiement » de l'abonnement diffèrent de
/// quelques secondes).
pub const SAME_PAYMENT_SECS: i64 = DAY / 2;

/// Abonnement PayPal tel que relu par le contrôle quotidien.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaypalView {
    pub status: String,
    pub next_billing: Option<i64>,
    pub last_payment: Option<i64>,
}

impl PaypalView {
    /// Lit la réponse de `GET /v1/billing/subscriptions/{id}`.
    pub fn from_subscription(v: &Value) -> Self {
        let time = |p: &str| {
            v.pointer(p)
                .and_then(Value::as_str)
                .and_then(crate::clients::paypal::parse_time)
        };
        Self {
            status: v
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            next_billing: time("/billing_info/next_billing_time"),
            last_payment: time("/billing_info/last_payment/time"),
        }
    }
}

/// Ce que le contrôle quotidien fait d'un abonnement PayPal relu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconcile {
    /// Rien de neuf.
    Nothing,
    /// Fiche d'avant le suivi des paiements, qui reflète déjà ce paiement : il devient la référence,
    /// rien n'est prolongé.
    Baseline(i64),
    /// Paiement constaté chez PayPal mais jamais appliqué (webhook perdu) : on l'applique.
    Apply(i64),
    /// Abonnement actif mais facturation due sans nouveau paiement (prochaine facturation absente, ou
    /// passée depuis plus de `paypal_margin_hours`, ou échéance de la fiche dépassée, marge comprise) :
    /// alerte admin, rien n'est prolongé.
    PendingCharge,
    /// Abonnement arrêté chez PayPal (`CANCELLED`, `SUSPENDED`, `EXPIRED`) ou inconnu de lui : noté,
    /// l'accès va jusqu'à l'échéance déjà payée, le cycle fait le reste.
    Stopped,
}

/// Décision pure du contrôle quotidien : un paiement n'est appliqué que s'il est constaté
/// (`last_payment` plus récent que le dernier appliqué), jamais déduit de la prochaine facturation.
pub fn reconcile_decision(s: &Subscriber, p: &PaypalView, now: i64, cfg: &SubsConfig) -> Reconcile {
    match p.status.as_str() {
        PAYPAL_ACTIVE => {}
        "CANCELLED" | "SUSPENDED" | "EXPIRED" | PAYPAL_NOT_FOUND => return Reconcile::Stopped,
        _ => return Reconcile::Nothing,
    }
    // un accès offert ou exempté n'attend aucun paiement : rien n'y est appliqué
    if !matches!(
        s.status,
        Status::Active | Status::Grace | Status::Trial | Status::Unknown | Status::Suspended
    ) {
        return Reconcile::Nothing;
    }
    // PayPal encaisse parfois des heures après la date de facturation annoncée : une facturation passée
    // n'est « en attente » qu'après la marge (le contrôle repart de chaque redémarrage de homelabd, il
    // peut tomber une demi-heure après l'heure de facturation)
    let margin = cfg.paypal_margin_hours as i64 * 3600;
    let nb = match p.next_billing {
        None => return Reconcile::PendingCharge,
        Some(nb) if nb <= now => {
            return if now >= nb + margin {
                Reconcile::PendingCharge
            } else {
                Reconcile::Nothing
            };
        }
        Some(nb) => nb,
    };
    if let Some(lp) = p.last_payment {
        match s.paypal_paid_at {
            Some(done) if lp > done + SAME_PAYMENT_SECS => return Reconcile::Apply(lp),
            Some(_) => {}
            None => {
                let reflected = s.status == Status::Active
                    && s.expires_at.is_some_and(|e| e >= nb - SAME_PAYMENT_SECS);
                if reflected {
                    return Reconcile::Baseline(lp);
                }
                // facturation repoussée bien au-delà d'une période après ce paiement (nouvelle tentative
                // après un échec) : il couvrait la période d'avant, rien de neuf n'a été payé
                if nb - lp > (cfg.period_days as i64 + 3) * DAY {
                    return Reconcile::PendingCharge;
                }
                return Reconcile::Apply(lp);
            }
        }
    }
    // aucun nouveau paiement : PayPal a pu repousser la facturation (nouvelle tentative après un échec)
    if s.expires_at.is_some_and(|e| now >= e + margin) {
        return Reconcile::PendingCharge;
    }
    Reconcile::Nothing
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
  updated_at INTEGER NOT NULL,
  paypal_status TEXT,
  paypal_paid_at INTEGER,
  due_noted INTEGER
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

/// Colonnes ajoutées après la création de la table (2026-10-07), posées à l'ouverture d'une base plus
/// ancienne. Un ancien binaire les ignore (liste de colonnes explicite).
/// `due_noted` : 2026-10-08 (fiches gérées à la main).
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("paypal_status", "TEXT"),
    ("paypal_paid_at", "INTEGER"),
    ("due_noted", "INTEGER"),
];

const COLS: &str = "user_id, username, status, starts_at, expires_at, source, paypal_sub_id, paypal_email, referral_code, referred_by, referral_credited, note, reminded, updated_at, paypal_status, paypal_paid_at, due_noted";

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
        paypal_status: r.get(14)?,
        paypal_paid_at: r.get(15)?,
        due_noted: r.get(16)?,
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
        let present = conn
            .prepare("SELECT name FROM pragma_table_info('subscribers')")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (col, decl) in ADDED_COLUMNS {
            if !present.iter().any(|p| p == col) {
                conn.execute_batch(&format!("ALTER TABLE subscribers ADD COLUMN {col} {decl}"))?;
            }
        }
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

    /// L'admin a été prévenu de l'échéance `exp` d'une fiche gérée à la main : noté sur la fiche (une
    /// information par échéance) et dans l'historique (événement `due_noted`, que « Mon compte » ne
    /// montre pas au membre).
    pub fn note_due(&self, user_id: &str, exp: i64, detail: &str, now: i64) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET due_noted = ?2, updated_at = ?3 WHERE user_id = ?1",
            params![user_id, exp, now],
        )?;
        self.log(user_id, "due_noted", detail, "cycle", now)
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
        // autre abonnement : le statut et le dernier paiement de l'ancien ne valent plus pour lui (les
        // expressions d'un UPDATE lisent l'ancienne ligne)
        self.db().execute(
            "UPDATE subscribers SET paypal_status = CASE WHEN paypal_sub_id IS ?2 THEN paypal_status END, paypal_paid_at = CASE WHEN paypal_sub_id IS ?2 THEN paypal_paid_at END, paypal_sub_id = ?2, paypal_email = COALESCE(?3, paypal_email), source = 'paypal', updated_at = ?4 WHERE user_id = ?1",
            params![user_id, sub_id, email, now],
        )?;
        self.log(user_id, "paypal_linked", sub_id, actor, now)
    }

    /// Note le statut PayPal (`None` le garde) et le dernier paiement appliqué (`None` le garde, et
    /// jamais en arrière).
    pub fn set_paypal_state(
        &self,
        user_id: &str,
        status: Option<&str>,
        paid_at: Option<i64>,
        now: i64,
    ) -> Result<()> {
        self.db().execute(
            "UPDATE subscribers SET paypal_status = COALESCE(?2, paypal_status), paypal_paid_at = CASE WHEN ?3 IS NULL THEN paypal_paid_at WHEN paypal_paid_at IS NULL OR paypal_paid_at < ?3 THEN ?3 ELSE paypal_paid_at END, updated_at = ?4 WHERE user_id = ?1",
            params![user_id, status, paid_at, now],
        )?;
        Ok(())
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

    /// Oublie un événement dont le traitement a échoué, pour que la relance de PayPal le rejoue. Sans ça,
    /// la relance répondait « déjà traité » et le paiement n'était appliqué qu'au rapprochement du lendemain
    /// (audit du 2026-09-23).
    pub fn forget_paypal_event(&self, event_id: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM paypal_events WHERE event_id = ?1",
            params![event_id],
        )?;
        Ok(())
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

/// Information avant un renouvellement PayPal automatique : aucun lien de paiement (un clic ferait un
/// second abonnement, donc un second prélèvement).
pub fn renewal_mail(username: &str, renews_on: &str) -> (String, String) {
    (
        format!("Groscailloux : ton abonnement se renouvelle le {renews_on}"),
        format!(
            "Salut {username},\n\nPetite information : ton abonnement Premium se renouvelle automatiquement le {renews_on} par PayPal. Tu n'as rien à faire.\n\nPour le consulter ou l'arrêter : ton compte PayPal → Paiements automatiques.\n\nÀ bientôt sur Groscailloux."
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

/// Suspension d'un abonnement PayPal qui se renouvelle tout seul (prélèvement échoué, abonnement encore
/// actif chez PayPal, qui retente) : aucun lien de paiement — un second abonnement serait débité puis
/// refusé, et le membre resterait suspendu.
pub fn payment_failed_mail(username: &str) -> (String, String) {
    (
        "Groscailloux : ton prélèvement PayPal n'est pas passé".to_string(),
        format!(
            "Salut {username},\n\nTon prélèvement PayPal n'est pas passé, et ton accès est en pause en attendant. Rien n'est supprimé : ton historique, tes favoris et tes demandes t'attendent.\n\nPour reprendre, mets à jour ton moyen de paiement dans ton compte PayPal → Paiements automatiques. PayPal retente le prélèvement, et l'accès revient tout seul dès qu'il passe. Inutile de t'abonner une deuxième fois : ce serait deux prélèvements.\n\nSi ce message te surprend, réponds à ce mail : on regarde ensemble.\n\nÀ bientôt sur Groscailloux."
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
            paypal_status: None,
            paypal_paid_at: None,
            due_noted: None,
        }
    }

    /// Fiche liée à un abonnement PayPal qui se renouvelle tout seul.
    fn paypal(status: Status, expires_in_days: Option<f64>) -> Subscriber {
        Subscriber {
            source: "paypal".into(),
            paypal_sub_id: Some("I-ABCDEFGHIJ12".into()),
            paypal_status: Some(PAYPAL_ACTIVE.into()),
            ..sub(status, expires_in_days)
        }
    }

    /// Fiche liée à un abonnement PayPal arrêté chez PayPal (annulé) : plus de prélèvement, rappels
    /// avec lien, grâce puis suspension.
    fn stopped(status: Status, expires_in_days: Option<f64>) -> Subscriber {
        Subscriber {
            paypal_status: Some("CANCELLED".into()),
            ..paypal(status, expires_in_days)
        }
    }

    /// Essai de l'inscription publique (commencé il y a longtemps : les paliers de rappel tombent).
    fn trial(status: Status, expires_in_days: Option<f64>) -> Subscriber {
        Subscriber {
            source: "trial".into(),
            starts_at: NOW - 60 * DAY,
            ..sub(status, expires_in_days)
        }
    }

    const NOW: i64 = 1_000_000;

    #[test]
    fn reminders_then_grace_then_suspend() {
        // abonnement PayPal arrêté chez PayPal : le cycle complet, comme avant le 2026-10-08
        let c = cfg();
        assert_eq!(
            decide(&stopped(Status::Active, Some(20.0)), NOW, &c),
            vec![]
        );
        assert_eq!(
            decide(&stopped(Status::Active, Some(6.5)), NOW, &c),
            vec![Action::Remind(7)]
        );
        assert_eq!(
            decide(&stopped(Status::Active, Some(0.5)), NOW, &c),
            vec![Action::Remind(1)]
        );
        let mut s = stopped(Status::Active, Some(0.5));
        s.reminded = 1 << 1;
        assert_eq!(decide(&s, NOW, &c), vec![]);
        assert_eq!(
            decide(&stopped(Status::Active, Some(-1.0)), NOW, &c),
            vec![Action::ToGrace]
        );
        assert_eq!(decide(&stopped(Status::Grace, Some(-1.0)), NOW, &c), vec![]);
        assert_eq!(
            decide(&stopped(Status::Grace, Some(-3.0)), NOW, &c),
            vec![Action::Suspend]
        );
        assert_eq!(
            decide(&stopped(Status::Active, Some(-10.0)), NOW, &c),
            vec![Action::Suspend]
        );
    }

    #[test]
    fn exempt_unknown_suspended_never_touched() {
        let c = cfg();
        for st in [Status::Exempt, Status::Unknown, Status::Suspended] {
            for exp in [None, Some(-30.0), Some(0.5)] {
                assert_eq!(decide(&sub(st, exp), NOW, &c), vec![], "{st:?} {exp:?}");
                assert!(!hand_due(&sub(st, exp), NOW), "{st:?} {exp:?}");
            }
            // même lié à PayPal : un exempté, un « à qualifier » ou un suspendu n'est pas suivi
            assert_eq!(decide(&stopped(st, Some(-30.0)), NOW, &c), vec![], "{st:?}");
        }
        assert!(managed_by_hand(&sub(Status::Exempt, None)));
        assert!(managed_by_hand(&sub(Status::Unknown, None)));
        assert_eq!(decide(&sub(Status::Active, None), NOW, &c), vec![]);
        assert_eq!(decide(&sub(Status::Offered, None), NOW, &c), vec![]);
        assert!(!hand_due(&sub(Status::Offered, None), NOW));
    }

    /// Fiche gérée à la main (2026-10-08) : jamais de rappel au membre, de grâce ni de suspension, quel
    /// que soit le délai ; une seule information à l'admin par échéance.
    fn assert_hand_managed(make: impl Fn(Option<f64>) -> Subscriber, what: &str) {
        let c = cfg();
        // avant l'échéance, y compris aux paliers J-7 et J-1 : rien
        for d in [40.0, 20.0, 7.0, 6.5, 1.0, 0.5, 0.01] {
            let s = make(Some(d));
            assert!(managed_by_hand(&s), "{what} J-{d}");
            assert_eq!(decide(&s, NOW, &c), vec![], "{what} J-{d}");
            assert!(!hand_due(&s, NOW), "{what} J-{d}");
        }
        // échéance atteinte, puis bien au-delà de la grâce : une information, jamais de suspension
        for d in [0.0, -1.0, -3.0, -3.5, -10.0, -400.0] {
            let s = make(Some(d));
            assert_eq!(decide(&s, NOW, &c), vec![Action::ManualDue], "{what} J+{d}");
            assert!(hand_due(&s, NOW), "{what} J+{d}");
            // l'admin a été prévenu de cette échéance : plus rien
            let noted = Subscriber {
                due_noted: s.expires_at,
                ..s.clone()
            };
            assert_eq!(decide(&noted, NOW, &c), vec![], "{what} J+{d} prévenu");
            assert!(hand_due(&noted, NOW), "{what} J+{d} reste « à gérer »");
            // nouvelle échéance (prolongation, nouvelle décision), passée à son tour : nouvelle information
            let again = Subscriber {
                due_noted: s.expires_at.map(|e| e - 30 * DAY),
                ..s
            };
            assert_eq!(
                decide(&again, NOW, &c),
                vec![Action::ManualDue],
                "{what} J+{d} autre échéance"
            );
        }
        // des rappels déjà partis avant le 2026-10-08 ne changent rien
        let mut s = make(Some(-5.0));
        s.reminded = (1 << 7) | (1 << 1);
        assert_eq!(decide(&s, NOW, &c), vec![Action::ManualDue], "{what}");
    }

    #[test]
    fn manual_active_is_never_reminded_nor_suspended() {
        assert_hand_managed(|e| sub(Status::Active, e), "actif manuel");
        // « Actif » posé sur un compte importé (prolongation d'un « à qualifier ») : pareil
        assert_hand_managed(
            |e| Subscriber {
                source: "import".into(),
                ..sub(Status::Active, e)
            },
            "actif importé",
        );
        // essai prolongé par l'admin après sa fin : redevenu « actif », décision de l'admin
        assert_hand_managed(|e| trial(Status::Active, e), "essai prolongé");
    }

    #[test]
    fn offered_with_expiry_is_never_reminded_nor_suspended() {
        assert_hand_managed(|e| sub(Status::Offered, e), "offert pour N jours");
    }

    #[test]
    fn manual_fiche_left_in_grace_by_the_old_cycle_is_never_suspended() {
        // passée en grâce avant le 2026-10-08 : elle reste active, l'admin est prévenu une fois
        assert_hand_managed(|e| sub(Status::Grace, e), "grâce manuelle");
    }

    #[test]
    fn trials_keep_reminders_grace_and_suspension() {
        let c = cfg();
        assert!(is_trial(&trial(Status::Trial, Some(3.0))));
        assert!(!managed_by_hand(&trial(Status::Trial, Some(3.0))));
        assert_eq!(
            decide(&trial(Status::Trial, Some(0.5)), NOW, &c),
            vec![Action::Remind(1)]
        );
        assert_eq!(
            decide(&trial(Status::Trial, Some(-0.1)), NOW, &c),
            vec![Action::ToGrace]
        );
        // la grâce d'un essai garde sa source : toujours un essai, suspendu à la fin de la grâce
        let grace = trial(Status::Grace, Some(-1.0));
        assert!(is_trial(&grace) && !managed_by_hand(&grace));
        assert_eq!(decide(&grace, NOW, &c), vec![]);
        assert_eq!(
            decide(&trial(Status::Grace, Some(-3.0)), NOW, &c),
            vec![Action::Suspend]
        );
        assert!(!hand_due(&trial(Status::Grace, Some(-3.0)), NOW));
        // essai posé par l'admin (`subs set --status trial`, source manuelle) : décision de l'admin,
        // gérée à la main (2026-10-08) — ni rappel « t'abonner », ni grâce, ni suspension
        let by_admin = |e| Subscriber {
            starts_at: NOW - 60 * DAY,
            ..sub(Status::Trial, e)
        };
        assert!(!is_trial(&by_admin(Some(0.5))));
        assert_hand_managed(by_admin, "essai posé par l'admin");
        // une grâce sans la source `trial` n'est pas celle d'un essai (fiche passée en grâce par l'ancien
        // cycle) : gérée à la main
        assert!(!is_trial(&sub(Status::Grace, Some(-1.0))));
    }

    /// Rejoue le cycle horaire sur une fiche du magasin, en appliquant chaque action comme `run_cycle`
    /// (rappel noté, passage en grâce sans changer la source, suspension, information notée), de
    /// `from` à `to`. Renvoie les actions et leur heure.
    fn replay(st: &SubStore, uid: &str, from: i64, to: i64) -> Vec<(i64, Action)> {
        let c = cfg();
        let mut seen = Vec::new();
        let mut t = from;
        while t <= to {
            let s = st.get(uid).unwrap().unwrap();
            for a in decide(&s, t, &c) {
                match a {
                    Action::Remind(d) | Action::RenewalNotice(d) => {
                        st.set_reminded(uid, d, t).unwrap()
                    }
                    Action::ToGrace => st
                        .set_status(uid, Status::Grace, None, None, "cycle", "grâce", t)
                        .unwrap(),
                    Action::Suspend => st
                        .set_status(uid, Status::Suspended, Some(None), None, "cycle", "fin", t)
                        .unwrap(),
                    Action::ManualDue => st
                        .note_due(uid, s.expires_at.unwrap(), "échéance", t)
                        .unwrap(),
                }
                seen.push((t, a));
            }
            t += 3600;
        }
        seen
    }

    #[test]
    fn trial_paths_hour_by_hour() {
        let c = cfg();
        let grace = c.grace_days as i64 * DAY;
        // essai de l'inscription publique (comme `start_trial`) : J-1, grâce à l'échéance, suspension à
        // la fin de la grâce — le comportement d'avant le 2026-10-08, inchangé
        let st = SubStore::open_in_memory().unwrap();
        let exp = NOW + 7 * DAY;
        st.ensure("pub", "membre", Status::Trial, Some(exp), "trial", NOW)
            .unwrap();
        let seen = replay(&st, "pub", NOW, exp + grace + 30 * DAY);
        let kinds: Vec<&Action> = seen.iter().map(|(_, a)| a).collect();
        assert_eq!(
            kinds,
            vec![&Action::Remind(1), &Action::ToGrace, &Action::Suspend]
        );
        assert!(
            seen[1].0 >= exp && seen[1].0 < exp + 3600,
            "grâce à l'échéance"
        );
        assert!(
            seen[2].0 >= exp + grace && seen[2].0 < exp + grace + 3600,
            "suspension à la fin de la grâce"
        );
        let s = st.get("pub").unwrap().unwrap();
        assert_eq!((s.status, s.source.as_str()), (Status::Suspended, "trial"));

        // essai posé par l'admin (comme `admin_set(Status::Trial)` : source `manual`) : aucun rappel,
        // aucune grâce, aucune suspension ; une information à l'admin à l'échéance, une seule
        let st = SubStore::open_in_memory().unwrap();
        st.ensure("adm", "membre", Status::Unknown, None, "import", NOW)
            .unwrap();
        st.set_status(
            "adm",
            Status::Trial,
            Some(Some(exp)),
            Some("manual"),
            "admin",
            "décision admin",
            NOW,
        )
        .unwrap();
        let seen = replay(&st, "adm", NOW, exp + grace + 30 * DAY);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].1, Action::ManualDue);
        assert!(seen[0].0 >= exp && seen[0].0 < exp + 3600);
        let s = st.get("adm").unwrap().unwrap();
        assert_eq!((s.status, s.due_noted), (Status::Trial, Some(exp)));
        assert!(hand_due(&s, exp + grace + 30 * DAY), "affichée « à gérer »");
    }

    #[test]
    fn manual_active_hour_by_hour_is_never_reminded_nor_suspended() {
        // « Actif pour 30 jours » posé par l'admin (`admin_set`) : rejoué heure par heure jusqu'à deux
        // mois après l'échéance, une seule action en tout (l'information à l'admin)
        let st = SubStore::open_in_memory().unwrap();
        let exp = NOW + 30 * DAY;
        st.ensure("m", "membre", Status::Unknown, None, "import", NOW)
            .unwrap();
        st.set_status(
            "m",
            Status::Active,
            Some(Some(exp)),
            Some("manual"),
            "admin",
            "décision admin",
            NOW,
        )
        .unwrap();
        let seen = replay(&st, "m", NOW, exp + 60 * DAY);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].1, Action::ManualDue);
        let s = st.get("m").unwrap().unwrap();
        assert_eq!((s.status, s.reminded), (Status::Active, 0));
    }

    #[test]
    fn paypal_linked_fiches_keep_their_cycle() {
        let c = cfg();
        // abonnement actif : information sans lien, marge, grâce, suspension
        let active = paypal(Status::Active, Some(6.5));
        assert!(!managed_by_hand(&active));
        assert_eq!(decide(&active, NOW, &c), vec![Action::RenewalNotice(7)]);
        assert_eq!(
            decide(&paypal(Status::Active, Some(-10.0)), NOW, &c),
            vec![Action::Suspend]
        );
        assert!(!hand_due(&paypal(Status::Active, Some(-10.0)), NOW));
        // abonnement arrêté chez PayPal (annulé, suspendu, expiré, introuvable) : rappels avec lien
        for st in ["CANCELLED", "SUSPENDED", "EXPIRED", PAYPAL_NOT_FOUND] {
            let s = Subscriber {
                paypal_status: Some(st.into()),
                ..paypal(Status::Active, Some(0.5))
            };
            assert!(!managed_by_hand(&s), "{st}");
            assert_eq!(decide(&s, NOW, &c), vec![Action::Remind(1)], "{st}");
            let s = Subscriber {
                expires_at: Some(NOW - 4 * DAY),
                ..s
            };
            assert_eq!(decide(&s, NOW, &c), vec![Action::Suspend], "{st}");
            assert!(!hand_due(&s, NOW), "{st}");
        }
        // décision de l'admin sur une fiche liée (offert pour N jours, source manuelle) : le lien PayPal
        // décide, comme avant
        let offered = Subscriber {
            source: "manual".into(),
            ..stopped(Status::Offered, Some(-10.0))
        };
        assert!(!managed_by_hand(&offered));
        assert_eq!(decide(&offered, NOW, &c), vec![Action::Suspend]);
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
    fn paypal_auto_renewal_one_notice_without_link_then_margin_before_grace() {
        let c = cfg();
        let h = 3600i64;
        assert_eq!(decide(&paypal(Status::Active, Some(20.0)), NOW, &c), vec![]);
        // une seule information, au palier le plus lointain ; jamais de « renouvelle ici »
        assert_eq!(
            decide(&paypal(Status::Active, Some(6.5)), NOW, &c),
            vec![Action::RenewalNotice(7)]
        );
        let mut s = paypal(Status::Active, Some(0.5));
        s.reminded = 1 << 7;
        assert_eq!(
            decide(&s, NOW, &c),
            vec![],
            "pas de J-1 après l'information"
        );
        // échéance = date de facturation PayPal : rien pendant la marge, grâce ensuite
        let at = |hours: i64| Subscriber {
            expires_at: Some(NOW - hours * h),
            ..paypal(Status::Active, Some(0.0))
        };
        assert_eq!(decide(&at(1), NOW, &c), vec![]);
        assert_eq!(decide(&at(35), NOW, &c), vec![]);
        assert_eq!(decide(&at(37), NOW, &c), vec![Action::ToGrace]);
        assert_eq!(decide(&at(3 * 24), NOW, &c), vec![Action::ToGrace]);
        assert_eq!(
            decide(&at(36 + 3 * 24), NOW, &c),
            vec![Action::Suspend],
            "suspension à échéance + marge + grâce"
        );
        // statut encore inconnu (fiche d'avant le suivi) ou décision de l'admin : prélève toujours
        let mut legacy = paypal(Status::Active, Some(6.5));
        legacy.paypal_status = None;
        legacy.source = "manual".into();
        assert_eq!(decide(&legacy, NOW, &c), vec![Action::RenewalNotice(7)]);
    }

    #[test]
    fn a_second_subscription_is_spotted_against_the_linked_one() {
        let s = paypal(Status::Active, Some(10.0));
        assert_eq!(
            other_linked_sub(&s, "I-ZZZZZZZZZZ99"),
            Some("I-ABCDEFGHIJ12")
        );
        // paiement de l'abonnement déjà lié, ou fiche sans abonnement : rien à vérifier
        assert_eq!(other_linked_sub(&s, "I-ABCDEFGHIJ12"), None);
        assert_eq!(
            other_linked_sub(&sub(Status::Suspended, None), "I-ZZZZZZZZZZ99"),
            None
        );
    }

    #[test]
    fn the_linked_subscription_blocks_a_second_one_only_while_it_charges() {
        // PayPal répond : seul ACTIVE prélève encore (refus du second)
        assert!(linked_still_charges(Some(Some(PAYPAL_ACTIVE)), None));
        for st in ["CANCELLED", "SUSPENDED", "EXPIRED"] {
            assert!(
                !linked_still_charges(Some(Some(st)), Some(PAYPAL_ACTIVE)),
                "{st}"
            );
        }
        // inconnu de PayPal (404, autre environnement) : rien n'est prélevé, le second se rattache
        assert!(!linked_still_charges(Some(None), Some(PAYPAL_ACTIVE)));
        // sans client PayPal : dernier statut connu, inconnu = actif
        assert!(linked_still_charges(None, None));
        assert!(linked_still_charges(None, Some(PAYPAL_ACTIVE)));
        assert!(!linked_still_charges(None, Some("CANCELLED")));
    }

    #[test]
    fn premium_link_key_is_tied_to_the_account_and_the_secret() {
        let k = premium_link_key("secret-serveur", "John");
        assert_eq!(k.len(), 16);
        assert!(k.bytes().all(|b| b.is_ascii_hexdigit()));
        // même nom à la casse près (`by_username` l'ignore aussi)
        assert!(premium_link_ok("secret-serveur", "john", &k));
        assert!(!premium_link_ok("secret-serveur", "jane", &k));
        assert!(!premium_link_ok("autre-secret", "john", &k));
        assert!(!premium_link_ok("secret-serveur", "john", ""));
        assert!(!premium_link_ok("secret-serveur", "john", &k[..15]));
        let last = if k.ends_with('0') { "1" } else { "0" };
        assert!(!premium_link_ok(
            "secret-serveur",
            "john",
            &format!("{}{last}", &k[..15])
        ));
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn stopped_paypal_subscription_gets_the_usual_reminders_and_no_margin() {
        let c = cfg();
        for st in ["CANCELLED", "SUSPENDED", "EXPIRED", PAYPAL_NOT_FOUND] {
            let mut s = paypal(Status::Active, Some(6.5));
            s.paypal_status = Some(st.into());
            assert!(!auto_renews(&s), "{st}");
            assert_eq!(decide(&s, NOW, &c), vec![Action::Remind(7)], "{st}");
            s.expires_at = Some(NOW - 3600);
            assert_eq!(decide(&s, NOW, &c), vec![Action::ToGrace], "{st}");
        }
        // sans abonnement lié : jamais de renouvellement automatique (et, depuis le 2026-10-08, géré à
        // la main : aucun rappel)
        assert!(!auto_renews(&sub(Status::Active, Some(6.5))));
        assert_eq!(decide(&sub(Status::Active, Some(6.5)), NOW, &c), vec![]);
    }

    #[test]
    fn trial_reminder_never_right_after_signup() {
        let c = cfg();
        // essai de l'inscription publique (source `trial`, comme `start_trial`)
        let trial = |starts: i64, days: i64| Subscriber {
            starts_at: starts,
            expires_at: Some(starts + days * DAY),
            source: "trial".into(),
            ..sub(Status::Trial, None)
        };
        // inscription il y a une heure, essai de 7 jours : pas de J-7
        assert_eq!(decide(&trial(NOW - 3600, 7), NOW, &c), vec![]);
        assert_eq!(decide(&trial(NOW, 7), NOW, &c), vec![]);
        // le J-1 part bien
        assert_eq!(
            decide(&trial(NOW - 6 * DAY - 3600, 7), NOW, &c),
            vec![Action::Remind(1)]
        );
        // essai prolongé (parrainage) : le J-7 tombe au bon moment
        assert_eq!(
            decide(&trial(NOW - 15 * DAY - 3600, 22), NOW, &c),
            vec![Action::Remind(7)]
        );
        // un abonnement (ici PayPal arrêté, qui garde les rappels) n'est pas concerné
        let active = Subscriber {
            starts_at: NOW - 3600,
            ..stopped(Status::Active, Some(6.5))
        };
        assert_eq!(decide(&active, NOW, &c), vec![Action::Remind(7)]);
    }

    #[test]
    fn referral_never_ends_an_unlimited_access() {
        for st in [Status::Offered, Status::Exempt, Status::Unknown] {
            assert!(!referral_extends(&sub(st, None)), "{st:?}");
        }
        for (st, exp) in [
            (Status::Offered, Some(10.0)),
            (Status::Active, Some(10.0)),
            (Status::Trial, Some(3.0)),
            (Status::Grace, Some(-1.0)),
            (Status::Suspended, None),
        ] {
            assert!(referral_extends(&sub(st, exp)), "{st:?}");
        }
    }

    #[test]
    fn orphans_removed_only_when_few_and_jellyfin_answered() {
        // fiches qui ont quelque chose à perdre (échéance) : elles seules comptent dans le plafond
        let fiche = |id: &str| Subscriber {
            user_id: id.into(),
            username: id.into(),
            ..sub(Status::Active, Some(10.0))
        };
        let fiches: Vec<Subscriber> = ["a", "b", "c", "d"].into_iter().map(fiche).collect();
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        let names = |v: &[&Subscriber]| v.iter().map(|s| s.user_id.clone()).collect::<Vec<_>>();
        let p = orphan_plan(&fiches, &ids(&["a", "b", "c", "d", "e"]), 3);
        assert!(p.remove.is_empty() && p.held.is_empty());
        let p = orphan_plan(&fiches, &ids(&["a", "b", "c"]), 3);
        assert_eq!((names(&p.remove), p.held.len()), (vec!["d".to_string()], 0));
        let p = orphan_plan(&fiches, &ids(&["a"]), 3);
        assert_eq!((p.remove.len(), p.held.len()), (3, 0));
        // plus de fiches qu'autorisé disparaîtraient d'un coup : rien n'est retiré
        let p = orphan_plan(&fiches, &ids(&["a"]), 2);
        assert_eq!(
            (p.remove.len(), names(&p.held)),
            (0, vec!["b".into(), "c".into(), "d".into()])
        );
        assert_eq!(p.held_ids(), ids(&["b", "c", "d"]));
        // Jellyfin n'a renvoyé aucun compte : rien n'est retiré
        let p = orphan_plan(&fiches, &ids(&[]), 10);
        assert_eq!((p.remove.len(), p.held.len()), (0, 4));
    }

    #[test]
    fn disposable_orphans_go_without_cap_valued_ones_are_held() {
        // 5 fiches sans rien à perdre (comptes de banc à qualifier, suspendus ou exemptés, sans
        // échéance) + 1 liée à PayPal, plafond 0 : les 5 partent, la fiche PayPal reste
        let mut fiches: Vec<Subscriber> = (0..5)
            .map(|i| Subscriber {
                user_id: format!("zz_{i}"),
                username: format!("zz_{i}"),
                ..sub(
                    [Status::Unknown, Status::Suspended, Status::Exempt][i % 3],
                    None,
                )
            })
            .collect();
        fiches.push(Subscriber {
            user_id: "paie".into(),
            username: "paie".into(),
            ..paypal(Status::Active, Some(10.0))
        });
        let ids: HashSet<String> = ["autre".to_string()].into_iter().collect();
        let p = orphan_plan(&fiches, &ids, 0);
        assert_eq!(p.remove.len(), 5);
        assert!(p.remove.iter().all(|s| s.user_id.starts_with("zz_")));
        assert_eq!(p.held_ids(), ["paie".to_string()].into_iter().collect());
        // ce qui a quelque chose à perdre n'est jamais « sans valeur »
        assert!(!orphan_disposable(&paypal(Status::Suspended, None)));
        assert!(!orphan_disposable(&sub(Status::Suspended, Some(-5.0))));
        assert!(!orphan_disposable(&sub(Status::Offered, None)));
        assert!(!orphan_disposable(&sub(Status::Trial, Some(3.0))));
        // Jellyfin muet : même les fiches sans valeur restent
        let p = orphan_plan(&fiches, &HashSet::new(), 10);
        assert_eq!((p.remove.len(), p.held.len()), (0, 6));
    }

    fn view(status: &str, next: Option<i64>, last: Option<i64>) -> PaypalView {
        PaypalView {
            status: status.into(),
            next_billing: next,
            last_payment: last,
        }
    }

    #[test]
    fn reconcile_applies_only_a_payment_seen_at_paypal() {
        let c = cfg();
        let paid = |status: Status, exp_days: Option<f64>, paid_at: Option<i64>| Subscriber {
            paypal_paid_at: paid_at,
            ..paypal(status, exp_days)
        };
        let s = paid(Status::Active, Some(10.0), Some(NOW - 21 * DAY));
        // rien de neuf (même paiement, à quelques secondes près)
        let v = view("ACTIVE", Some(NOW + 10 * DAY), Some(NOW - 21 * DAY + 5));
        assert_eq!(reconcile_decision(&s, &v, NOW, &c), Reconcile::Nothing);
        // PayPal repousse la facturation sans paiement (échec puis nouvelle tentative) : rien n'est
        // prolongé (l'ancien contrôle aurait compté un mois de plus)
        let v = view("ACTIVE", Some(NOW + 41 * DAY), Some(NOW - 21 * DAY));
        assert_eq!(reconcile_decision(&s, &v, NOW, &c), Reconcile::Nothing);
        // nouveau paiement constaté : appliqué
        let v = view("ACTIVE", Some(NOW + 30 * DAY), Some(NOW - 3600));
        assert_eq!(
            reconcile_decision(&s, &v, NOW, &c),
            Reconcile::Apply(NOW - 3600)
        );
        // prochaine facturation absente, ou passée depuis plus que la marge : alerte, jamais
        // « maintenant + 31 jours »
        for nb in [None, Some(NOW - 37 * 3600)] {
            let v = view("ACTIVE", nb, Some(NOW - 21 * DAY));
            assert_eq!(
                reconcile_decision(&s, &v, NOW, &c),
                Reconcile::PendingCharge
            );
        }
        // facturation passée d'une heure (contrôle juste après un redémarrage) : PayPal encaisse
        // parfois des heures plus tard, rien à signaler encore
        for h in [1, 35] {
            let v = view("ACTIVE", Some(NOW - h * 3600), Some(NOW - 21 * DAY));
            assert_eq!(
                reconcile_decision(&s, &v, NOW, &c),
                Reconcile::Nothing,
                "{h} h"
            );
        }
        // échéance de la fiche dépassée (marge comprise) sans nouveau paiement : alerte
        let late = paid(Status::Grace, Some(-2.0), Some(NOW - 33 * DAY));
        let v = view("ACTIVE", Some(NOW + 3 * DAY), Some(NOW - 33 * DAY));
        assert_eq!(
            reconcile_decision(&late, &v, NOW, &c),
            Reconcile::PendingCharge
        );
        let in_margin = paid(Status::Active, Some(-1.0), Some(NOW - 32 * DAY));
        assert_eq!(
            reconcile_decision(&in_margin, &v, NOW, &c),
            Reconcile::Nothing
        );
    }

    #[test]
    fn reconcile_takes_the_reflected_payment_as_reference_for_older_fiches() {
        let c = cfg();
        // fiche d'avant le suivi, déjà à la date de facturation PayPal : rien n'est prolongé
        let legacy = paypal(Status::Active, Some(14.0));
        let v = view("ACTIVE", Some(NOW + 14 * DAY), Some(NOW - 17 * DAY));
        assert_eq!(
            reconcile_decision(&legacy, &v, NOW, &c),
            Reconcile::Baseline(NOW - 17 * DAY)
        );
        // webhook de renouvellement manqué : la fiche est en retard sur PayPal
        let behind = paypal(Status::Grace, Some(-2.0));
        let v = view("ACTIVE", Some(NOW + 29 * DAY), Some(NOW - DAY));
        assert_eq!(
            reconcile_decision(&behind, &v, NOW, &c),
            Reconcile::Apply(NOW - DAY)
        );
        // rattachée par l'import CSV, sans échéance : le paiement pose l'échéance
        let imported = paypal(Status::Unknown, None);
        assert_eq!(
            reconcile_decision(&imported, &v, NOW, &c),
            Reconcile::Apply(NOW - DAY)
        );
        // offert ou exempté : aucun paiement appliqué
        for st in [Status::Offered, Status::Exempt] {
            assert_eq!(
                reconcile_decision(&paypal(st, None), &v, NOW, &c),
                Reconcile::Nothing
            );
        }
        // fiche sans paiement de référence, mais PayPal a repoussé la facturation après un échec : le
        // dernier paiement est celui de la période d'avant, il ne réactive ni ne prolonge rien
        let v = view("ACTIVE", Some(NOW + 3 * DAY), Some(NOW - 33 * DAY));
        for s in [
            paypal(Status::Grace, Some(-2.0)),
            paypal(Status::Suspended, None),
            paypal(Status::Unknown, None),
        ] {
            assert_eq!(
                reconcile_decision(&s, &v, NOW, &c),
                Reconcile::PendingCharge,
                "{:?}",
                s.status
            );
        }
    }

    #[test]
    fn reconcile_follows_stops_without_touching_the_paid_period() {
        let c = cfg();
        let s = paypal(Status::Active, Some(10.0));
        for st in ["CANCELLED", "SUSPENDED", "EXPIRED", PAYPAL_NOT_FOUND] {
            assert_eq!(
                reconcile_decision(&s, &view(st, None, Some(NOW - 20 * DAY)), NOW, &c),
                Reconcile::Stopped,
                "{st}"
            );
        }
        for st in ["APPROVAL_PENDING", "APPROVED", ""] {
            assert_eq!(
                reconcile_decision(&s, &view(st, None, None), NOW, &c),
                Reconcile::Nothing,
                "{st}"
            );
        }
        let v = serde_json::json!({"status":"ACTIVE","billing_info":{"next_billing_time":"2026-10-21T10:00:00Z","last_payment":{"time":"2026-09-21T10:00:05Z"}}});
        assert_eq!(
            PaypalView::from_subscription(&v),
            view(
                "ACTIVE",
                crate::clients::paypal::parse_time("2026-10-21T10:00:00Z"),
                crate::clients::paypal::parse_time("2026-09-21T10:00:05Z")
            )
        );
    }

    #[test]
    fn renewal_notice_has_no_payment_link() {
        let (subject, body) = renewal_mail("membre", "21/10/2026");
        assert!(subject.contains("21/10/2026") && body.contains("21/10/2026"));
        assert!(!body.contains("http") && !body.contains("/premium"));
    }

    #[test]
    fn failed_charge_suspension_has_no_payment_link() {
        let (subject, body) = payment_failed_mail("membre");
        assert!(subject.contains("PayPal") && body.contains("Paiements automatiques"));
        assert!(!body.contains("http") && !body.contains("/premium"));
    }

    #[test]
    fn paypal_state_follows_the_linked_subscription_and_never_goes_back() {
        let st = SubStore::open_in_memory().unwrap();
        st.ensure("id1", "Alice", Status::Unknown, None, "import", NOW)
            .unwrap();
        st.link_paypal("id1", "I-AAAAAAAAAA11", None, "webhook", NOW)
            .unwrap();
        st.set_paypal_state("id1", Some(PAYPAL_ACTIVE), Some(500), NOW)
            .unwrap();
        st.set_paypal_state("id1", None, Some(400), NOW).unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(
            (s.paypal_status.as_deref(), s.paypal_paid_at),
            (Some(PAYPAL_ACTIVE), Some(500))
        );
        st.set_paypal_state("id1", Some("CANCELLED"), None, NOW)
            .unwrap();
        // même abonnement relié : rien ne change
        st.link_paypal("id1", "I-AAAAAAAAAA11", None, "admin", NOW)
            .unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(
            (s.paypal_status.as_deref(), s.paypal_paid_at),
            (Some("CANCELLED"), Some(500))
        );
        // autre abonnement : son état repart de zéro
        st.link_paypal("id1", "I-BBBBBBBBBB22", None, "admin", NOW)
            .unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!((s.paypal_status, s.paypal_paid_at), (None, None));
    }

    #[test]
    fn an_older_database_gets_the_new_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE subscribers (user_id TEXT PRIMARY KEY, username TEXT NOT NULL, status TEXT NOT NULL, starts_at INTEGER NOT NULL, expires_at INTEGER, source TEXT NOT NULL DEFAULT 'manual', paypal_sub_id TEXT, paypal_email TEXT, referral_code TEXT NOT NULL UNIQUE, referred_by TEXT, referral_credited INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '', reminded INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL);
             INSERT INTO subscribers (user_id, username, status, starts_at, expires_at, source, paypal_sub_id, referral_code, updated_at) VALUES ('id1', 'Alice', 'active', 1, 99, 'paypal', 'I-AAAAAAAAAA11', 'ABCDEFGH', 1);",
        )
        .unwrap();
        let st = SubStore::init(conn).unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(
            (
                s.paypal_sub_id.as_deref(),
                s.paypal_status.as_deref(),
                s.paypal_paid_at
            ),
            (Some("I-AAAAAAAAAA11"), None, None)
        );
        assert!(auto_renews(&s));
        assert_eq!(s.due_noted, None);
        st.set_paypal_state("id1", Some(PAYPAL_ACTIVE), Some(7), 2)
            .unwrap();
        assert_eq!(st.get("id1").unwrap().unwrap().paypal_paid_at, Some(7));
    }

    #[test]
    fn a_database_of_2026_10_07_gets_due_noted() {
        // colonnes PayPal déjà là, `due_noted` absente : posée à l'ouverture, les fiches sont relues
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE subscribers (user_id TEXT PRIMARY KEY, username TEXT NOT NULL, status TEXT NOT NULL, starts_at INTEGER NOT NULL, expires_at INTEGER, source TEXT NOT NULL DEFAULT 'manual', paypal_sub_id TEXT, paypal_email TEXT, referral_code TEXT NOT NULL UNIQUE, referred_by TEXT, referral_credited INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '', reminded INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL, paypal_status TEXT, paypal_paid_at INTEGER);
             INSERT INTO subscribers (user_id, username, status, starts_at, expires_at, source, referral_code, updated_at) VALUES ('id1', 'Alice', 'active', 1, 99, 'manual', 'ABCDEFGH', 1);",
        )
        .unwrap();
        let st = SubStore::init(conn).unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(
            (s.status, s.expires_at, s.due_noted),
            (Status::Active, Some(99), None)
        );
        assert_eq!(decide(&s, 100, &cfg()), vec![Action::ManualDue]);
    }

    #[test]
    fn due_noted_is_kept_once_per_expiry() {
        let c = cfg();
        let st = SubStore::open_in_memory().unwrap();
        st.ensure("id1", "Alice", Status::Unknown, None, "import", NOW)
            .unwrap();
        st.set_status(
            "id1",
            Status::Active,
            Some(Some(NOW - DAY)),
            Some("manual"),
            "admin",
            "décision admin",
            NOW - 31 * DAY,
        )
        .unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(decide(&s, NOW, &c), vec![Action::ManualDue]);
        st.note_due("id1", NOW - DAY, "échéance atteinte", NOW)
            .unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(s.due_noted, Some(NOW - DAY));
        assert_eq!(decide(&s, NOW + 10 * DAY, &c), vec![], "une seule fois");
        let last = &st.history("id1", 1).unwrap()[0];
        assert_eq!(last.kind, "due_noted");
        // note pour l'admin : jamais montrée au membre dans « Mon compte »
        assert!(!member_visible(&last.kind));
        assert!(!member_visible("paypal_event"));
        assert!(member_visible("status") && member_visible("extended"));
        // prolongation par l'admin : nouvelle échéance, nouvelle information quand elle passe
        let new = st.extend("id1", 30, "admin", "test", NOW).unwrap();
        let s = st.get("id1").unwrap().unwrap();
        assert_eq!(decide(&s, NOW, &c), vec![]);
        assert_eq!(decide(&s, new, &c), vec![Action::ManualDue]);
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

    #[test]
    fn a_failed_webhook_can_be_replayed() {
        let st = SubStore::open_in_memory().unwrap();
        assert!(st
            .record_paypal_event(
                "WH-1",
                "PAYMENT.SALE.COMPLETED",
                Some("I-ABCDEFGHIJ"),
                "",
                1
            )
            .unwrap());
        // relance pendant le traitement : ignorée
        assert!(!st
            .record_paypal_event("WH-1", "PAYMENT.SALE.COMPLETED", None, "", 2)
            .unwrap());
        // le traitement échoue : l'événement est oublié, la relance suivante est rejouée
        st.forget_paypal_event("WH-1").unwrap();
        assert!(st
            .record_paypal_event("WH-1", "PAYMENT.SALE.COMPLETED", None, "", 3)
            .unwrap());
    }
}
