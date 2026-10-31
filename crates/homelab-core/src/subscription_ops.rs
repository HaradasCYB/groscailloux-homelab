//! Opérations sur les abonnés qui touchent aux services (Jellyfin, PayPal, mails) : ce que le
//! webhook, la page `/premium`, « Mon compte », `/accounts` et les tâches appellent. Les décisions
//! pures sont dans [`crate::subscriptions`]. Toute suspension/activation passe par
//! [`crate::accounts::set_premium`] (plafond, comptes protégés, Jellyseerr, lectures arrêtées).

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::accounts::{self, Outcome};
use crate::alerts::{self, Level};
use crate::clients::paypal::EventFacts;
use crate::context::TaskContext;
use crate::mail;
use crate::mail_layout::RenderedMail;
use crate::state::now;
use crate::subscriptions::{self as subs, Action, Reconcile, Status, Subscriber, DAY};

/// Lien de paiement pré-rempli pour un membre (page `/premium`), ou le lien PayPal hébergé. Il porte
/// la clé `k` ([`subs::premium_link_key`], secret = `HOMELABD_ONBOARD_TOKEN`) : seul un lien émis ici
/// fait afficher « déjà abonné » à la page publique (un rappel recliqué après s'être abonné).
pub fn pay_url(ctx: &TaskContext, username: &str) -> String {
    pay_link(ctx, username, true)
}

/// Même lien, sans clé : « Mon compte » ne le montre qu'à un compte qui ne se renouvelle pas tout seul
/// (la clé n'y servirait à rien), et l'écrit en clair sur une télé pour qu'il soit retapé ailleurs.
pub fn pay_url_short(ctx: &TaskContext, username: &str) -> String {
    pay_link(ctx, username, false)
}

fn pay_link(ctx: &TaskContext, username: &str, keyed: bool) -> String {
    match &ctx.secrets.premium_public_url {
        Some(base) => premium_url(
            base,
            username,
            ctx.secrets
                .onboard_token
                .as_ref()
                .filter(|_| keyed)
                .map(|t| t.expose()),
        ),
        None => ctx
            .paypal
            .as_ref()
            .map(|p| p.subscribe_url())
            .unwrap_or_default(),
    }
}

/// `<base>/premium?compte=<nom>[&k=<clé>]` (sans secret, pas de clé : la page reste la page normale).
fn premium_url(base: &str, username: &str, secret: Option<&str>) -> String {
    let mut url = format!("{base}/premium?compte={}", urlencode(username));
    if let Some(secret) = secret {
        url.push_str("&k=");
        url.push_str(&subs::premium_link_key(secret, username));
    }
    url
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn date_text(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m/%Y")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

fn is_exempt(ctx: &TaskContext, username: &str) -> bool {
    let low = username.to_lowercase();
    ctx.cfg
        .accounts
        .protected
        .iter()
        .chain(ctx.cfg.subscriptions.exempt.iter())
        .any(|n| n.to_lowercase() == low)
}

/// Bilan de [`ensure_fiches`].
#[derive(Debug, Default)]
pub struct FichesSync {
    /// Fiches créées pour des comptes qui n'en avaient pas.
    pub created: usize,
    /// Fiches gardées alors que leur compte Jellyfin a disparu (trop de disparitions d'un coup) : le
    /// cycle n'y touche pas tant que l'admin n'a pas tranché.
    pub held: HashSet<String>,
}

/// Une fiche pour chaque compte Jellyfin (hors admins) : les comptes actifs sans fiche arrivent
/// « à qualifier » (jamais suspendus par le cycle), les suspendus en « suspendu », les protégés et
/// exemptés en « exempt ». Les noms renommés sont suivis. Renvoie les fiches créées et gardées.
pub async fn ensure_fiches(ctx: &TaskContext) -> Result<FichesSync> {
    let users = ctx.jellyfin.users().await?;
    if users.is_empty() {
        bail!("Jellyfin n'a renvoyé aucun compte : fiches d'abonnés laissées telles quelles");
    }
    let t = now();
    let mut created = 0;
    for u in &users {
        let (Some(id), Some(name)) = (
            u.get("Id").and_then(Value::as_str),
            u.get("Name").and_then(Value::as_str),
        ) else {
            continue;
        };
        if accounts::is_admin(u) && !is_exempt(ctx, name) {
            continue;
        }
        let premium = accounts::is_premium(u);
        let status = if is_exempt(ctx, name) {
            Status::Exempt
        } else if premium {
            Status::Unknown
        } else {
            Status::Suspended
        };
        match ctx.subs.get(id)? {
            Some(s) => {
                if s.username != name {
                    ctx.subs.set_username(id, name, t)?;
                }
            }
            None => {
                ctx.subs.ensure(id, name, status, None, "import", t)?;
                created += 1;
            }
        }
    }
    // fiches orphelines (compte supprimé hors de homelabd) : retirées (leurs événements restent), sauf
    // si trop de fiches qui ont quelque chose à perdre disparaissent d'un coup — réponse de Jellyfin
    // incomplète ? On les garde, l'admin tranche.
    let ids: HashSet<String> = users
        .iter()
        .filter_map(|u| u.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let fiches = ctx.subs.list()?;
    let max = ctx.cfg.subscriptions.max_orphan_removals_per_run;
    let plan = subs::orphan_plan(&fiches, &ids, max);
    for s in &plan.remove {
        info!(task = "subs", user = %s.username, "fiche orpheline retirée (compte Jellyfin absent)");
        ctx.subs.remove(&s.user_id)?;
    }
    if !plan.held.is_empty() {
        warn!(
            task = "subs",
            held = plan.held.len(),
            max,
            "fiches sans compte Jellyfin gardées : trop de disparitions d'un coup"
        );
        // une alerte par fiche, pas à chaque passage : l'événement « orphan_held » la marque
        let mut fresh = Vec::new();
        for s in &plan.held {
            let last = ctx.subs.history(&s.user_id, 1)?;
            if last.first().map(|e| e.kind.as_str()) != Some("orphan_held") {
                ctx.subs.log(
                    &s.user_id,
                    "orphan_held",
                    "compte Jellyfin absent, fiche gardée (trop de disparitions d'un coup)",
                    "system",
                    t,
                )?;
                fresh.push(s.username.clone());
            }
        }
        if !fresh.is_empty() {
            alerts::admin(
                ctx,
                Level::Warn,
                "Abonnés : fiches sans compte Jellyfin gardées",
                &format!(
                    "{} fiche(s) d'abonné avec une échéance, un lien PayPal ou une décision de l'admin n'ont plus de compte dans la réponse de Jellyfin (plus de {max} d'un coup) : aucune n'a été retirée, et le cycle n'y touche pas (ni mail ni suspension). Nouvelles : {}.\n\nSi ces comptes ont bien été supprimés, relever [subscriptions] max_orphan_removals_per_run le temps d'un passage.",
                    plan.held.len(),
                    fresh.join(", ")
                ),
            )
            .await;
        }
    }
    Ok(FichesSync {
        created,
        held: plan.held_ids(),
    })
}

/// Parrain d'une inscription publique (affiché dans le mail de l'admin, que l'essai démarre ou non).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sponsor {
    /// Aucun code saisi.
    None,
    /// Code reconnu : pseudo du parrain.
    Known(String),
    /// Code saisi mais inconnu (ou celui du compte lui-même) : ignoré.
    Unknown,
    /// Code saisi mais non vérifié (erreur de la base des abonnés).
    Unchecked,
}

impl Sponsor {
    /// Phrase du mail de l'admin.
    pub fn line(&self) -> String {
        match self {
            Self::None => "Sans parrain.".into(),
            Self::Known(p) => format!("Parrainé par {p}."),
            Self::Unknown => "Code de parrainage saisi inconnu, ignoré.".into(),
            Self::Unchecked => {
                "Code de parrainage saisi mais non vérifié (erreur), à regarder.".into()
            }
        }
    }
}

/// Parrain d'après le code saisi et la fiche qui porte ce code (fonction pure) : le code du compte lui-même ne
/// compte pas.
pub fn sponsor_for(code: Option<&str>, found: Option<(&str, &str)>, user_id: &str) -> Sponsor {
    match (code.map(str::trim).filter(|c| !c.is_empty()), found) {
        (None, _) => Sponsor::None,
        (Some(_), Some((id, name))) if id != user_id => Sponsor::Known(name.to_string()),
        (Some(_), _) => Sponsor::Unknown,
    }
}

/// Pourquoi l'essai d'une inscription publique n'a pas démarré.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrialBlock {
    /// `[subscriptions] enabled = false`.
    Disabled,
    /// `[subscriptions] trial_days = 0`.
    NoTrialDays,
    /// `[accounts] max_premium` atteint : compte laissé suspendu, fiche passée en « suspendu ».
    CapReached { premium: usize, max: usize },
    /// Une fiche existait déjà pour ce compte avec un autre statut (rien de touché).
    Existing(String),
    /// Erreur (base des abonnés, Jellyfin) : état du compte à vérifier.
    Failed(String),
}

impl TrialBlock {
    /// Raison lisible, pour le mail de l'admin.
    pub fn reason(&self) -> String {
        match self {
            Self::Disabled => "abonnements désactivés ([subscriptions] enabled = false)".into(),
            Self::NoTrialDays => "aucun essai configuré ([subscriptions] trial_days = 0)".into(),
            Self::CapReached { premium, max } => {
                format!("plafond premium atteint ({premium}/{max})")
            }
            Self::Existing(status) => format!("fiche d'abonné déjà présente (statut {status})"),
            Self::Failed(e) => format!("erreur ({e}) : vérifier l'état du compte sur /accounts"),
        }
    }
}

/// Issue de l'essai d'une inscription publique, décidée **avant** le mail de bienvenue et le mail de l'admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrialOutcome {
    /// Fiche « essai » posée et compte activé.
    Started {
        days: u32,
        ends_at: i64,
        sponsor: Sponsor,
    },
    NotStarted {
        block: TrialBlock,
        sponsor: Sponsor,
    },
}

impl TrialOutcome {
    pub fn started(&self) -> bool {
        matches!(self, Self::Started { .. })
    }

    /// Durée de l'essai démarré (page qui suit l'inscription), `None` s'il n'a pas démarré.
    pub fn started_days(&self) -> Option<u32> {
        match self {
            Self::Started { days, .. } => Some(*days),
            Self::NotStarted { .. } => None,
        }
    }

    pub fn sponsor(&self) -> &Sponsor {
        match self {
            Self::Started { sponsor, .. } | Self::NotStarted { sponsor, .. } => sponsor,
        }
    }
}

/// Première décision, avant toute écriture (fonction pure) : durée de l'essai d'après la configuration.
pub fn trial_days_for(enabled: bool, trial_days: u32) -> std::result::Result<u32, TrialBlock> {
    if !enabled {
        Err(TrialBlock::Disabled)
    } else if trial_days == 0 {
        Err(TrialBlock::NoTrialDays)
    } else {
        Ok(trial_days)
    }
}

/// Essai qu'une inscription démarrerait maintenant (fonction pure) : configuration et place sous le plafond
/// premium. Sert aux réponses neutres de la page d'inscription, qui doivent dire la même chose qu'une vraie.
pub fn expected_trial(enabled: bool, trial_days: u32, premium: usize, max: usize) -> Option<u32> {
    trial_days_for(enabled, trial_days)
        .ok()
        .filter(|_| accounts::can_activate(premium, max))
}

/// Décision finale (fonction pure), une fois la fiche posée et l'activation tentée : `fiche` = statut de la fiche
/// après `ensure` (une fiche préexistante garde le sien), `activation` = réponse de
/// [`accounts::set_premium_locked`] (`Unchanged` = compte déjà actif).
pub fn trial_outcome(
    days: u32,
    ends_at: i64,
    fiche: Status,
    sponsor: Sponsor,
    activation: std::result::Result<&Outcome, String>,
) -> TrialOutcome {
    let block = if fiche != Status::Trial {
        TrialBlock::Existing(fiche.as_str().to_string())
    } else {
        match activation {
            Ok(Outcome::Activated | Outcome::Unchanged) => {
                return TrialOutcome::Started {
                    days,
                    ends_at,
                    sponsor,
                }
            }
            Ok(Outcome::CapReached { premium, max }) => TrialBlock::CapReached {
                premium: *premium,
                max: *max,
            },
            Ok(Outcome::Suspended) => TrialBlock::Failed("activation : compte suspendu".into()),
            Err(e) => TrialBlock::Failed(e),
        }
    };
    TrialOutcome::NotStarted { block, sponsor }
}

/// Essai gratuit à l'inscription publique : fiche « essai » et compte activé (si `[subscriptions] enabled` et
/// `trial_days > 0`, plafond premium non atteint), parrain vérifié. Appelé par l'onboarding, **qui tient
/// `onboard_lock`** (activation par [`accounts::set_premium_locked`]), avant le mail de bienvenue et celui de
/// l'admin : les deux disent ce qui s'est vraiment passé. Décisions : [`sponsor_for`], [`trial_days_for`],
/// [`trial_outcome`]. Une fois le compte activé, une écriture ratée du journal n'est qu'un avertissement : l'essai
/// a bien démarré.
pub async fn start_trial(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    referral: Option<&str>,
) -> TrialOutcome {
    let cfg = &ctx.cfg.subscriptions;
    let t = now();
    let code = referral.map(str::trim).filter(|c| !c.is_empty());
    let (sponsor, referrer_id) = match code.map(|c| ctx.subs.by_referral_code(c)) {
        None => (Sponsor::None, None),
        Some(Ok(found)) => {
            let found = found.map(|r| (r.user_id, r.username));
            let sponsor = sponsor_for(
                code,
                found.as_ref().map(|(i, n)| (i.as_str(), n.as_str())),
                user_id,
            );
            let id = matches!(sponsor, Sponsor::Known(_))
                .then(|| found.map(|(i, _)| i))
                .flatten();
            (sponsor, id)
        }
        Some(Err(e)) => {
            warn!(task = "subs", user = %username, error = format!("{e:#}"), "code de parrainage non vérifié");
            (Sponsor::Unchecked, None)
        }
    };
    if sponsor == Sponsor::Unknown {
        info!(task = "subs", user = %username, "code de parrainage inconnu ignoré");
    }
    let note_referral = |ctx: &TaskContext| {
        if let (Some(rid), Sponsor::Known(name)) = (&referrer_id, &sponsor) {
            let r = ctx.subs.set_referred_by(user_id, rid, t).and_then(|()| {
                ctx.subs.log(
                    user_id,
                    "referred",
                    &format!("parrainé par {name}"),
                    "signup",
                    t,
                )
            });
            if let Err(e) = r {
                warn!(task = "subs", user = %username, error = format!("{e:#}"), "parrainage non noté");
            }
        }
    };
    let days = match trial_days_for(cfg.enabled, cfg.trial_days) {
        Ok(d) => d,
        Err(block) => {
            if block == TrialBlock::NoTrialDays {
                match ctx
                    .subs
                    .ensure(user_id, username, Status::Suspended, None, "signup", t)
                {
                    Ok(_) => note_referral(ctx),
                    Err(e) => {
                        warn!(task = "subs", user = %username, error = format!("{e:#}"), "fiche suspendue non créée")
                    }
                }
            }
            return TrialOutcome::NotStarted { block, sponsor };
        }
    };
    let ends_at = t + i64::from(days) * DAY;
    let fiche = match ctx
        .subs
        .ensure(user_id, username, Status::Trial, Some(ends_at), "trial", t)
    {
        Ok(s) => s,
        Err(e) => {
            warn!(task = "subs", user = %username, error = format!("{e:#}"), "trial not started");
            return TrialOutcome::NotStarted {
                block: TrialBlock::Failed(format!("{e:#}")),
                sponsor,
            };
        }
    };
    note_referral(ctx);
    let activation = if fiche.status == Status::Trial {
        accounts::set_premium_locked(ctx, user_id, true)
            .await
            .map_err(|e| format!("{e:#}"))
    } else {
        Ok(Outcome::Unchanged)
    };
    let outcome = trial_outcome(
        days,
        fiche.expires_at.unwrap_or(ends_at),
        fiche.status,
        sponsor,
        activation.as_ref().map_err(String::clone),
    );
    // journal et fiche d'après la décision ; une erreur ici ne change plus ce qui a été fait
    let noted = match &outcome {
        TrialOutcome::Started { .. } => {
            info!(task = "subs", user = %username, "trial started at signup");
            ctx.subs.log(
                user_id,
                "trial_started",
                &format!("{days} jours"),
                "signup",
                t,
            )
        }
        TrialOutcome::NotStarted {
            block: TrialBlock::CapReached { premium, max },
            ..
        } => {
            warn!(task = "subs", user = %username, premium, max, "essai : plafond premium atteint, compte laissé suspendu");
            // pas de fiche « essai » qui courrait sur un compte suspendu (rappels, fin d'essai) : l'admin choisit
            // le statut en activant le compte
            ctx.subs.set_status(
                user_id,
                Status::Suspended,
                Some(None),
                None,
                "signup",
                "essai impossible : plafond premium atteint",
                t,
            )
        }
        TrialOutcome::NotStarted {
            block: TrialBlock::Failed(e),
            ..
        } => {
            warn!(task = "subs", user = %username, error = %e, "trial not started");
            Ok(())
        }
        TrialOutcome::NotStarted { .. } => Ok(()),
    };
    if let Err(e) = noted {
        warn!(task = "subs", user = %username, error = format!("{e:#}"), "essai : journal ou fiche non mis à jour");
    }
    outcome
}

/// Issue d'un paiement reçu.
#[derive(Debug)]
pub enum Payment {
    /// Appliqué à cette fiche (échéance prolongée, compte réactivé).
    Applied(Subscriber),
    /// Aucune fiche ne correspond : à rattacher depuis `/accounts` (admin prévenu).
    Unlinked,
    /// Le compte a déjà un AUTRE abonnement PayPal actif : rien n'est rattaché ni prolongé, l'admin
    /// est prévenu (deux abonnements = deux prélèvements par mois).
    Duplicate(Subscriber),
}

/// L'abonnement déjà rattaché à la fiche prélève-t-il encore ? PayPal fait foi (une panne est une
/// erreur : le webhook sera rejoué, rien n'est rattaché entre-temps) ; la décision est
/// [`subs::linked_still_charges`].
async fn linked_sub_active(ctx: &TaskContext, s: &Subscriber, linked: &str) -> Result<bool> {
    let found = match ctx.paypal.as_ref() {
        Some(pp) => Some(
            pp.find_subscription(linked)
                .await
                .with_context(|| format!("abonnement déjà rattaché {linked} illisible"))?,
        ),
        None => None,
    };
    let lookup = found.as_ref().map(|v| {
        v.as_ref()
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
    });
    Ok(subs::linked_still_charges(
        lookup,
        s.paypal_status.as_deref(),
    ))
}

/// Jour (UTC, `AAAA-MM-JJ`) des clés « une alerte par jour » de `paypal_events`.
fn day_key(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Second abonnement pour un compte qui en a déjà un actif : noté sur la fiche, admin prévenu une
/// fois par jour et par abonnement (la page, l'activation et la vente arrivent ensemble).
async fn refuse_second_subscription(
    ctx: &TaskContext,
    s: &Subscriber,
    linked: &str,
    new_sub: &str,
    t: i64,
) -> Result<()> {
    warn!(task = "subs", user = %s.username, linked, new_sub, "second abonnement PayPal refusé : le compte en a déjà un actif");
    if ctx.subs.record_paypal_event(
        &format!("second:{new_sub}:{}", day_key(t)),
        "SECOND_SUBSCRIPTION",
        Some(new_sub),
        &format!("compte {} déjà abonné ({linked})", s.username),
        t,
    )? {
        ctx.subs.log(
            &s.user_id,
            "paypal",
            &format!("second abonnement {new_sub} refusé : {linked} est encore actif"),
            "system",
            t,
        )?;
        alerts::admin(
            ctx,
            Level::Warn,
            "Second abonnement PayPal pour un même compte",
            &format!(
                "{} a déjà l'abonnement {linked}, actif chez PayPal, et vient d'en prendre un second ({new_sub}). Le second n'est ni rattaché ni compté : rien n'est prolongé deux fois. Il a pu être débité : à résilier (et rembourser) dans PayPal, ou rattacher à la main après résiliation de l'ancien (homelabctl subs link).",
                s.username
            ),
        )
        .await;
    }
    Ok(())
}

/// Paiement reçu (webhook, page `/premium` ou réconciliation) : rattache l'abonnement, prolonge
/// l'échéance, réactive le compte s'il était suspendu, crédite un éventuel parrainage, prévient
/// le membre. `facts.sub_id` obligatoire ; le compte est trouvé par abonnement lié, sinon par
/// `custom_id` (nom de compte), sinon par e-mail PayPal connu. Un compte qui a déjà un autre
/// abonnement actif ne se voit rien rattacher ni prolonger ([`Payment::Duplicate`]).
pub async fn on_payment(ctx: &TaskContext, facts: &EventFacts, actor: &str) -> Result<Payment> {
    let Some(sub_id) = facts.sub_id.as_deref() else {
        bail!("événement sans identifiant d'abonnement");
    };
    let t = now();
    let cfg = &ctx.cfg.subscriptions;
    let sub = match ctx.subs.by_paypal_sub(sub_id)? {
        Some(s) => s,
        None => {
            let by_custom = match facts.custom_id.as_deref() {
                Some(c) => resolve_account(ctx, c).await?,
                None => None,
            };
            let found = match by_custom {
                Some(f) => Some(f),
                None => match facts.email.as_deref() {
                    Some(e) => ctx
                        .subs
                        .list()?
                        .into_iter()
                        .find(|s| s.paypal_email.as_deref() == Some(e)),
                    None => None,
                },
            };
            let Some(s) = found else {
                warn!(task = "subs", sub_id, custom = ?facts.custom_id, "paiement reçu pour un abonnement non rattaché");
                ctx.subs.record_paypal_event(
                    &format!("unlinked:{sub_id}:{}", facts.event_id),
                    &facts.event_type,
                    Some(sub_id),
                    "non rattaché",
                    t,
                )?;
                alerts::admin(
                    ctx,
                    Level::Warn,
                    "Abonnement PayPal non rattaché",
                    &format!(
                        "Paiement reçu pour l'abonnement {sub_id} (compte indiqué : {}, e-mail : {}) mais aucune fiche ne lui correspond. À rattacher depuis /accounts.",
                        facts.custom_id.as_deref().unwrap_or("—"),
                        facts.email.as_deref().unwrap_or("—")
                    ),
                )
                .await;
                return Ok(Payment::Unlinked);
            };
            if let Some(linked) = subs::other_linked_sub(&s, sub_id) {
                if linked_sub_active(ctx, &s, linked).await? {
                    refuse_second_subscription(ctx, &s, linked, sub_id, t).await?;
                    return Ok(Payment::Duplicate(s));
                }
            }
            ctx.subs
                .link_paypal(&s.user_id, sub_id, facts.email.as_deref(), actor, t)?;
            ctx.subs.get(&s.user_id)?.context("fiche disparue")?
        }
    };
    let first_payment = sub.status != Status::Active || sub.source != "paypal";
    let new_exp = subs::next_expiry(sub.expires_at, facts.next_billing, t, cfg);
    let was = sub.status;
    ctx.subs.set_status(
        &sub.user_id,
        Status::Active,
        Some(Some(new_exp)),
        Some("paypal"),
        actor,
        &format!(
            "paiement {} — actif jusqu'au {}",
            facts
                .amount
                .as_deref()
                .map(|a| format!("{a} €"))
                .unwrap_or_else(|| "reçu".into()),
            date_text(new_exp)
        ),
        t,
    )?;
    // un paiement vient d'arriver : l'abonnement prélève (sauf statut contraire lu chez PayPal) ; son
    // heure sert au contrôle quotidien pour ne jamais compter deux fois le même
    ctx.subs.set_paypal_state(
        &sub.user_id,
        Some(facts.sub_status.as_deref().unwrap_or(subs::PAYPAL_ACTIVE)),
        Some(facts.paid_at.unwrap_or(t)),
        t,
    )?;
    if ctx.dry_run {
        info!(task = "subs", user = %sub.username, "dry-run: compte non réactivé");
    } else {
        match accounts::set_premium(ctx, &sub.user_id, true).await {
            Ok(Outcome::CapReached { premium, max }) => {
                warn!(task = "subs", user = %sub.username, premium, max, "paiement reçu mais plafond premium atteint");
                alerts::admin(
                    ctx,
                    Level::Error,
                    "Plafond premium atteint après un paiement",
                    &format!(
                        "{} a payé mais le plafond ({premium}/{max}) empêche l'activation.",
                        sub.username
                    ),
                )
                .await;
            }
            Ok(o) => info!(task = "subs", user = %sub.username, ?o, "paiement appliqué"),
            Err(e) => {
                warn!(task = "subs", user = %sub.username, error = format!("{e:#}"), "réactivation impossible")
            }
        }
    }
    if first_payment {
        credit_referral(ctx, &sub, t).await?;
        if matches!(
            was,
            Status::Suspended | Status::Grace | Status::Trial | Status::Unknown
        ) || sub.source != "paypal"
        {
            send_member_mail(
                ctx,
                &sub.user_id,
                &sub.username,
                subs::activated_mail(&sub.username, new_exp, &ctx.secrets.jellyfin_public_url),
            )
            .await;
        }
    }
    Ok(Payment::Applied(
        ctx.subs.get(&sub.user_id)?.context("fiche disparue")?,
    ))
}

/// Parrainage : au premier paiement d'un filleul, jours offerts aux deux (plafond annuel).
async fn credit_referral(ctx: &TaskContext, sub: &Subscriber, t: i64) -> Result<()> {
    let cfg = &ctx.cfg.subscriptions;
    let Some(ref_id) = sub.referred_by.as_deref() else {
        return Ok(());
    };
    if sub.referral_credited || cfg.referral_days == 0 {
        return Ok(());
    }
    ctx.subs.mark_referral_credited(&sub.user_id, t)?;
    for (uid, who) in [(sub.user_id.as_str(), "filleul"), (ref_id, "parrain")] {
        let used = ctx.subs.referral_credited_last_year(uid, t)?;
        let days = subs::referral_allowance(used, cfg);
        if days <= 0 {
            continue;
        }
        let Some(fiche) = ctx.subs.get(uid)? else {
            continue;
        };
        if !subs::referral_extends(&fiche) {
            // offert sans limite, exempté, à qualifier : des jours en plus lui donneraient une fin
            ctx.subs.log(
                uid,
                "referral",
                &format!("parrainage ({who}) : accès sans échéance, rien à prolonger"),
                "system",
                t,
            )?;
            continue;
        }
        ctx.subs
            .extend(uid, days, "system", &format!("parrainage ({who})"), t)?;
        ctx.subs
            .add_referral_credit(uid, days, &format!("{who} de {}", sub.username), t)?;
    }
    Ok(())
}

/// Annulation / suspension / expiration côté PayPal : rien n'est coupé tout de suite, le compte
/// va jusqu'au bout de sa période payée puis le cycle fait le reste. Note dans l'historique.
pub async fn on_paypal_stop(ctx: &TaskContext, facts: &EventFacts) -> Result<()> {
    let Some(sub_id) = facts.sub_id.as_deref() else {
        return Ok(());
    };
    let t = now();
    if let Some(s) = ctx.subs.by_paypal_sub(sub_id)? {
        ctx.subs.log(
            &s.user_id,
            "paypal",
            &format!(
                "{} ({})",
                facts.event_type,
                facts.status.as_deref().unwrap_or("?")
            ),
            "webhook",
            t,
        )?;
        // arrêt chez PayPal : plus de prélèvement, donc plus de marge ni d'« information de
        // renouvellement » ; l'échéance payée reste (les autres événements ne touchent pas au statut :
        // un « créé » relivré en retard ne doit pas faire croire à un abonnement inactif)
        if matches!(
            facts.event_type.as_str(),
            "BILLING.SUBSCRIPTION.CANCELLED"
                | "BILLING.SUBSCRIPTION.SUSPENDED"
                | "BILLING.SUBSCRIPTION.EXPIRED"
        ) {
            if let Some(st) = facts.sub_status.as_deref() {
                ctx.subs.set_paypal_state(&s.user_id, Some(st), None, t)?;
            }
        }
        if facts.event_type == "PAYMENT.SALE.REFUNDED"
            || facts.event_type == "PAYMENT.SALE.REVERSED"
        {
            alerts::admin(
                ctx,
                Level::Warn,
                "Remboursement PayPal",
                &format!(
                    "{} : {} — à regarder sur /accounts.",
                    s.username, facts.event_type
                ),
            )
            .await;
        }
    }
    Ok(())
}

/// Compte Jellyfin par nom (insensible à la casse), avec sa fiche (créée « suspendu » si absente).
pub async fn resolve_account(ctx: &TaskContext, username: &str) -> Result<Option<Subscriber>> {
    if let Some(s) = ctx.subs.by_username(username)? {
        return Ok(Some(s));
    }
    let Some(u) = ctx.jellyfin.find_user(username).await? else {
        return Ok(None);
    };
    let (Some(id), Some(name)) = (
        u.get("Id").and_then(Value::as_str),
        u.get("Name").and_then(Value::as_str),
    ) else {
        return Ok(None);
    };
    let status = if accounts::is_premium(&u) {
        Status::Unknown
    } else {
        Status::Suspended
    };
    Ok(Some(ctx.subs.ensure(
        id,
        name,
        status,
        None,
        "import",
        now(),
    )?))
}

/// Action d'admin depuis `/accounts` ou `homelabctl subs`.
pub async fn admin_set(
    ctx: &TaskContext,
    user_id: &str,
    status: Status,
    days: Option<i64>,
    actor: &str,
) -> Result<Subscriber> {
    let t = now();
    let s = ctx.subs.get(user_id)?.context("fiche introuvable")?;
    let expires = match status {
        Status::Active | Status::Trial => {
            let base = s.expires_at.filter(|e| *e > t).unwrap_or(t);
            Some(Some(
                base + days.unwrap_or(ctx.cfg.subscriptions.period_days as i64) * DAY,
            ))
        }
        // offert : sans limite (pas de jours) ou pour N jours à partir de maintenant
        Status::Offered => Some(days.map(|d| t + d * DAY)),
        Status::Grace => None,
        _ => Some(None),
    };
    ctx.subs.set_status(
        user_id,
        status,
        expires,
        Some("manual"),
        actor,
        "décision admin",
        t,
    )?;
    if !ctx.dry_run {
        let want = status.wants_premium();
        if let Err(e) = accounts::set_premium(ctx, user_id, want).await {
            warn!(task = "subs", user = %s.username, error = format!("{e:#}"), "premium non modifié");
        }
    }
    ctx.subs.get(user_id)?.context("fiche introuvable")
}

pub async fn admin_extend(ctx: &TaskContext, user_id: &str, days: i64, actor: &str) -> Result<i64> {
    let t = now();
    let s = ctx.subs.get(user_id)?.context("fiche introuvable")?;
    let new = ctx
        .subs
        .extend(user_id, days, actor, "prolongation admin", t)?;
    if matches!(
        s.status,
        Status::Grace | Status::Suspended | Status::Unknown
    ) {
        ctx.subs.set_status(
            user_id,
            Status::Active,
            None,
            None,
            actor,
            "prolongation",
            t,
        )?;
        if !ctx.dry_run {
            if let Err(e) = accounts::set_premium(ctx, user_id, true).await {
                warn!(task = "subs", user = %s.username, error = format!("{e:#}"), "réactivation impossible");
            }
        }
    }
    Ok(new)
}

/// Mail d'abonnement à un membre (gabarit [`crate::mail_layout`], rendu par les fonctions pures de
/// [`crate::subscriptions`]) : rien sans SMTP ni adresse valide, journal seulement en dry-run ou
/// `cycle_dry_run`. Sans `Reply-To` : une réponse arrive à l'adresse d'envoi.
async fn send_member_mail(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    m: RenderedMail,
) -> bool {
    let Some(smtp) = &ctx.secrets.smtp else {
        return false;
    };
    let to = match accounts::email_of(ctx, user_id).await {
        Ok(Some(e)) if e.contains('@') => e,
        _ => {
            info!(task = "subs", user = %username, "pas d'adresse valide : mail non envoyé");
            return false;
        }
    };
    if ctx.dry_run || ctx.cfg.subscriptions.cycle_dry_run {
        info!(task = "subs", user = %username, subject = %m.subject, "dry-run: mail membre non envoyé");
        return false;
    }
    match mail::send_rendered(smtp, username, &to, &m, None, &[]).await {
        Ok(()) => true,
        Err(e) => {
            warn!(task = "subs", user = %username, error = format!("{e:#}"), "mail membre en échec");
            false
        }
    }
}

/// Un passage du cycle : rappels, grâce, suspensions des essais et des fiches liées à PayPal ; pour les
/// fiches gérées à la main ([`subs::managed_by_hand`], 2026-10-08), une seule information à l'admin par
/// échéance, compte laissé actif. Renvoie un résumé lisible.
pub async fn run_cycle(ctx: &TaskContext) -> Result<(String, u32)> {
    let cfg = &ctx.cfg.subscriptions;
    // Jellyfin injoignable : aucune fiche gardée connue, le cycle suit les fiches comme avant
    let sync = ensure_fiches(ctx).await.unwrap_or_else(|e| {
        warn!(
            task = "subs",
            error = format!("{e:#}"),
            "fiches non synchronisées avec Jellyfin"
        );
        FichesSync::default()
    });
    let created = sync.created;
    let t = now();
    let dry = ctx.dry_run || cfg.cycle_dry_run;
    let mut actions = 0u32;
    let mut lines: Vec<String> = Vec::new();
    // fiches gérées à la main arrivées à échéance (2026-10-08) : (compte, échéance, ligne de l'alerte)
    let mut due: Vec<(String, i64, String)> = Vec::new();
    for s in ctx.subs.list()? {
        if is_exempt(ctx, &s.username) {
            continue;
        }
        let todo = subs::decide(&s, t, cfg);
        if sync.held.contains(&s.user_id) {
            // compte Jellyfin absent, fiche gardée : ni rappel à un compte supprimé, ni suspension
            // impossible (et alerte « cycle » à chaque passage) tant que l'admin n'a pas tranché
            if !todo.is_empty() {
                info!(task = "subs", user = %s.username, actions = ?todo, "compte Jellyfin absent : fiche gardée, rien n'est fait");
            }
            continue;
        }
        for a in todo {
            actions += 1;
            match a {
                Action::Remind(d) => {
                    let sent = send_member_mail(
                        ctx,
                        &s.user_id,
                        &s.username,
                        subs::reminder_mail(&subs::Reminder {
                            username: &s.username,
                            days: d,
                            status: s.status,
                            ends_at: s.expires_at.unwrap_or(t + d * DAY),
                            renewal_stopped: s.paypal_sub_id.is_some() && !subs::auto_renews(&s),
                            price_text: &cfg.price_text,
                            pay_url: &pay_url(ctx, &s.username),
                        }),
                    )
                    .await;
                    if !dry {
                        ctx.subs.set_reminded(&s.user_id, d, t)?;
                    }
                    lines.push(format!(
                        "rappel J-{d} : {}{}",
                        s.username,
                        if sent { "" } else { " (mail non envoyé)" }
                    ));
                }
                Action::RenewalNotice(d) => {
                    // prélèvement automatique : jamais de lien de paiement (second abonnement)
                    let on = date_text(s.expires_at.unwrap_or(t));
                    let sent = send_member_mail(
                        ctx,
                        &s.user_id,
                        &s.username,
                        subs::renewal_mail(&s.username, s.expires_at.unwrap_or(t)),
                    )
                    .await;
                    if !dry {
                        ctx.subs.set_reminded(&s.user_id, d, t)?;
                    }
                    lines.push(format!(
                        "renouvellement PayPal le {on} (information J-{d}) : {}{}",
                        s.username,
                        if sent { "" } else { " (mail non envoyé)" }
                    ));
                }
                Action::ToGrace => {
                    if !dry {
                        ctx.subs.set_status(
                            &s.user_id,
                            Status::Grace,
                            None,
                            None,
                            "cycle",
                            &format!("échéance dépassée, grâce {} j", cfg.grace_days),
                            t,
                        )?;
                    }
                    lines.push(format!(
                        "grâce : {} (échéance {})",
                        s.username,
                        date_text(s.expires_at.unwrap_or(t))
                    ));
                }
                Action::Suspend => {
                    if dry {
                        lines.push(format!("suspendrait : {}", s.username));
                    } else {
                        match accounts::set_premium(ctx, &s.user_id, false).await {
                            Ok(_) => {
                                ctx.subs.set_status(
                                    &s.user_id,
                                    Status::Suspended,
                                    Some(None),
                                    None,
                                    "cycle",
                                    "grâce écoulée",
                                    t,
                                )?;
                                // prélèvement automatique en échec (abonnement encore actif chez
                                // PayPal, qui retente) : pas de lien, un second abonnement serait
                                // débité puis refusé
                                let mail = if subs::auto_renews(&s) {
                                    subs::payment_failed_mail(&s.username)
                                } else {
                                    subs::suspended_mail(
                                        &s.username,
                                        s.expires_at,
                                        subs::is_trial(&s),
                                        &cfg.price_text,
                                        &pay_url(ctx, &s.username),
                                    )
                                };
                                send_member_mail(ctx, &s.user_id, &s.username, mail).await;
                                lines.push(format!("suspendu : {}", s.username));
                            }
                            Err(e) => {
                                lines.push(format!("suspension impossible : {} ({e})", s.username))
                            }
                        }
                    }
                }
                Action::ManualDue => {
                    // géré à la main : le compte reste actif, rien n'est écrit au membre
                    let exp = s.expires_at.unwrap_or(t);
                    let line = format!(
                        "{} : échéance du {} atteinte ({}, source {}) — compte laissé actif, à gérer à la main",
                        s.username,
                        date_text(exp),
                        s.status.label(),
                        s.source
                    );
                    if dry {
                        lines.push(format!("à gérer à la main : {line}"));
                    } else {
                        due.push((s.user_id.clone(), exp, line));
                    }
                }
            }
        }
    }
    if !due.is_empty() {
        let sent = alerts::admin(
            ctx,
            Level::Warn,
            "Abonnés : échéance à gérer à la main",
            &format!(
                "Ces comptes ne sont liés à aucun abonnement PayPal : le cycle ne les suspend jamais et n'écrit jamais au membre (décision du 08/10/2026). Leur échéance est atteinte et le compte reste actif. À trancher sur /accounts (prolonger, offert, suspendre…) ou en rattachant un abonnement PayPal (homelabctl subs link <compte> --sub I-…). Une seule information par échéance.\n\n{}",
                due.iter()
                    .map(|(_, _, l)| l.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .await;
        // noté seulement si l'information est partie (ou si aucun canal n'est configuré : réessayer n'y
        // changerait rien) ; sinon elle repart au passage suivant
        if alerts::retry_later(sent, alerts::configured(ctx)) {
            warn!(
                task = "subs",
                fiches = due.len(),
                "échéance à gérer : information admin non livrée, nouvel essai au prochain passage"
            );
        } else {
            for (uid, exp, _) in &due {
                ctx.subs.note_due(
                    uid,
                    *exp,
                    &format!(
                        "échéance du {} atteinte : compte laissé actif, à gérer à la main",
                        date_text(*exp)
                    ),
                    t,
                )?;
            }
        }
    }
    let fiches = ctx.subs.list()?;
    let unknown = fiches
        .iter()
        .filter(|s| s.status == Status::Unknown)
        .count();
    let hand_due = fiches.iter().filter(|s| subs::hand_due(s, t)).count();
    if !lines.is_empty() {
        let body = format!(
            "{}{}\n\n{}",
            if dry {
                "[mode observation : rien n'est appliqué]\n"
            } else {
                ""
            },
            lines.join("\n"),
            if unknown > 0 {
                format!("{unknown} compte(s) « à qualifier » attendent une décision sur /accounts.")
            } else {
                String::new()
            }
        );
        alerts::admin(ctx, Level::Info, "Abonnés : cycle", &body).await;
    }
    Ok((
        format!(
            "{} fiche(s), {created} créée(s), {actions} action(s){}, {unknown} à qualifier, {hand_due} à gérer (échéance passée)",
            fiches.len(),
            if dry { " (observation)" } else { "" }
        ),
        actions,
    ))
}

/// Relit chaque abonnement PayPal connu (contrôle quotidien, filet des webhooks). Un paiement n'est
/// appliqué que s'il est constaté chez PayPal (`last_payment` plus récent que le dernier appliqué) ;
/// une facturation due sans paiement est signalée à l'admin, jamais prolongée ; un arrêt est noté et
/// l'accès va jusqu'à l'échéance déjà payée ([`subs::reconcile_decision`]).
pub async fn reconcile(ctx: &TaskContext) -> Result<(String, u32)> {
    let Some(pp) = ctx.paypal.as_ref() else {
        return Ok(("paypal non configuré".into(), 0));
    };
    let cfg = &ctx.cfg.subscriptions;
    let t = now();
    let mut checked = 0u32;
    let mut fixed = 0u32;
    let mut pending: Vec<String> = Vec::new();
    let mut pending_total = 0usize;
    let mut stopped: Vec<String> = Vec::new();
    let day_or = |d: Option<i64>, none: &str| d.map(date_text).unwrap_or_else(|| none.into());
    for s in ctx.subs.list()? {
        let Some(sid) = s.paypal_sub_id.clone() else {
            continue;
        };
        checked += 1;
        let view = match pp.find_subscription(&sid).await {
            Ok(Some(v)) => subs::PaypalView::from_subscription(&v),
            // identifiant inconnu de PayPal (autre environnement) : rien ne sera prélevé
            Ok(None) => subs::PaypalView {
                status: subs::PAYPAL_NOT_FOUND.into(),
                ..Default::default()
            },
            Err(e) => {
                warn!(task = "subs", user = %s.username, error = format!("{e:#}"), "abonnement PayPal illisible");
                continue;
            }
        };
        if view.status.is_empty() {
            warn!(task = "subs", user = %s.username, "abonnement PayPal sans statut");
            continue;
        }
        let changed = s.paypal_status.as_deref() != Some(view.status.as_str());
        if changed {
            ctx.subs
                .set_paypal_state(&s.user_id, Some(&view.status), None, t)?;
        }
        match subs::reconcile_decision(&s, &view, t, cfg) {
            Reconcile::Nothing => {}
            Reconcile::Baseline(paid) => {
                ctx.subs.set_paypal_state(&s.user_id, None, Some(paid), t)?
            }
            Reconcile::Apply(paid) => {
                let facts = EventFacts {
                    event_id: format!("reconcile:{sid}:{paid}"),
                    event_type: "RECONCILE".into(),
                    sub_id: Some(sid.clone()),
                    next_billing: view.next_billing,
                    sub_status: Some(view.status.clone()),
                    paid_at: Some(paid),
                    ..Default::default()
                };
                // trace seulement : la référence est le dernier paiement appliqué, noté sur la fiche
                ctx.subs.record_paypal_event(
                    &facts.event_id,
                    "RECONCILE",
                    Some(&sid),
                    "réconciliation",
                    t,
                )?;
                match on_payment(ctx, &facts, "reconcile").await {
                    Ok(_) => {
                        info!(task = "subs", user = %s.username, %sid, "paiement PayPal manqué appliqué");
                        fixed += 1;
                    }
                    Err(e) => {
                        warn!(task = "subs", user = %s.username, error = format!("{e:#}"), "paiement PayPal manqué non appliqué")
                    }
                }
            }
            Reconcile::PendingCharge => {
                pending_total += 1;
                // une alerte par abonnement et par jour : le contrôle repart de chaque redémarrage de
                // homelabd, plusieurs redémarrages dans la journée la répéteraient
                if ctx.subs.record_paypal_event(
                    &format!("pending:{sid}:{}", day_key(t)),
                    "PENDING_CHARGE",
                    Some(&sid),
                    &format!("compte {} : prélèvement en attente", s.username),
                    t,
                )? {
                    pending.push(format!(
                        "{} ({sid}) : prochaine facturation {}, dernier paiement {}, échéance de la fiche {}",
                        s.username,
                        day_or(view.next_billing, "absente"),
                        day_or(view.last_payment, "aucun"),
                        day_or(s.expires_at, "—"),
                    ));
                }
            }
            Reconcile::Stopped => {
                if changed {
                    let until = day_or(s.expires_at, "—");
                    let label = subs::paypal_status_label(&view.status);
                    ctx.subs.log(
                        &s.user_id,
                        "paypal",
                        &format!("abonnement {label} chez PayPal — accès jusqu'au {until}"),
                        "reconcile",
                        t,
                    )?;
                    stopped.push(format!(
                        "{} ({sid}) : {label} — accès jusqu'au {until}",
                        s.username
                    ));
                }
            }
        }
    }
    if !pending.is_empty() {
        alerts::admin(
            ctx,
            Level::Warn,
            "Prélèvement PayPal en attente",
            &format!(
                "PayPal donne ces abonnements actifs, mais aucun nouveau paiement n'est arrivé alors que la facturation est due. Rien n'a été prolongé : si le paiement arrive, tout se remet en ordre seul ; sinon le cycle suit l'échéance de la fiche (grâce, puis suspension). Rappel au plus une fois par jour et par abonnement.\n\n{}",
                pending.join("\n")
            ),
        )
        .await;
    }
    if !stopped.is_empty() {
        alerts::admin(
            ctx,
            Level::Info,
            "Abonnement PayPal arrêté",
            &format!(
                "Arrêts relevés par le contrôle quotidien (webhook manqué, ou arrêt d'avant le suivi du statut PayPal). Rien n'est coupé avant l'échéance déjà payée ; plus aucune information de renouvellement n'est envoyée, les rappels habituels (avec lien de paiement) reprennent.\n\n{}",
                stopped.join("\n")
            ),
        )
        .await;
    }
    Ok((
        format!(
            "abonnements PayPal vérifiés={checked} corrigés={fixed} en attente={pending_total} arrêtés={}",
            stopped.len()
        ),
        fixed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configuration → essai possible ou non (avant toute écriture), et page des réponses neutres.
    #[test]
    fn trial_decision_from_the_config() {
        assert_eq!(trial_days_for(true, 7), Ok(7));
        assert_eq!(trial_days_for(true, 0), Err(TrialBlock::NoTrialDays));
        assert_eq!(trial_days_for(false, 7), Err(TrialBlock::Disabled));
        assert_eq!(trial_days_for(false, 0), Err(TrialBlock::Disabled));
        // réponse neutre : même page qu'une vraie inscription, plafond compris
        assert_eq!(expected_trial(true, 7, 24, 25), Some(7));
        assert_eq!(expected_trial(true, 7, 25, 25), None);
        assert_eq!(expected_trial(true, 0, 0, 25), None);
        assert_eq!(expected_trial(false, 7, 0, 25), None);
    }

    /// Code saisi → parrain : reconnu, absent, inconnu, ou le code du compte lui-même.
    #[test]
    fn trial_decision_sponsor() {
        let bob = Some(("id-bob", "bob"));
        assert_eq!(sponsor_for(None, bob, "id-lea"), Sponsor::None);
        assert_eq!(sponsor_for(Some("  "), None, "id-lea"), Sponsor::None);
        assert_eq!(
            sponsor_for(Some("ABCD1234"), bob, "id-lea"),
            Sponsor::Known("bob".into())
        );
        assert_eq!(
            sponsor_for(Some("ZZZZ9999"), None, "id-lea"),
            Sponsor::Unknown
        );
        assert_eq!(
            sponsor_for(Some("ABCD1234"), Some(("id-lea", "léa")), "id-lea"),
            Sponsor::Unknown
        );
    }

    /// Fiche posée + activation tentée → issue : seul un compte effectivement actif démarre l'essai.
    #[test]
    fn trial_decision_after_activation() {
        let s = || Sponsor::Known("bob".into());
        let started = TrialOutcome::Started {
            days: 7,
            ends_at: 100,
            sponsor: s(),
        };
        assert_eq!(
            trial_outcome(7, 100, Status::Trial, s(), Ok(&Outcome::Activated)),
            started
        );
        // compte déjà actif (`new_accounts_premium`) : l'essai court aussi
        assert_eq!(
            trial_outcome(7, 100, Status::Trial, s(), Ok(&Outcome::Unchanged)),
            started
        );
        assert_eq!(
            trial_outcome(
                7,
                100,
                Status::Trial,
                s(),
                Ok(&Outcome::CapReached {
                    premium: 25,
                    max: 25
                })
            ),
            TrialOutcome::NotStarted {
                block: TrialBlock::CapReached {
                    premium: 25,
                    max: 25
                },
                sponsor: s()
            }
        );
        let failed = trial_outcome(7, 100, Status::Trial, s(), Err("Jellyfin 500".into()));
        assert_eq!(
            failed,
            TrialOutcome::NotStarted {
                block: TrialBlock::Failed("Jellyfin 500".into()),
                sponsor: s()
            }
        );
        assert_eq!(failed.sponsor(), &s());
        assert!(matches!(
            trial_outcome(7, 100, Status::Trial, s(), Ok(&Outcome::Suspended)),
            TrialOutcome::NotStarted {
                block: TrialBlock::Failed(_),
                ..
            }
        ));
        // fiche préexistante (autre statut) : rien ne démarre, quel que soit le compte
        assert_eq!(
            trial_outcome(
                7,
                100,
                Status::Active,
                Sponsor::None,
                Ok(&Outcome::Unchanged)
            ),
            TrialOutcome::NotStarted {
                block: TrialBlock::Existing("active".into()),
                sponsor: Sponsor::None
            }
        );
    }

    #[test]
    fn premium_link_carries_the_key_only_with_a_server_secret() {
        let url = premium_url("https://premium.example", "jo-hn", Some("secret"));
        let (head, k) = url.split_once("&k=").unwrap();
        assert_eq!(head, "https://premium.example/premium?compte=jo-hn");
        assert!(subs::premium_link_ok("secret", "jo-hn", k));
        // sans secret : lien sans clé, la page publique reste la page normale
        assert_eq!(
            premium_url("https://premium.example", "jo-hn", None),
            "https://premium.example/premium?compte=jo-hn"
        );
    }

    #[test]
    fn day_key_is_the_utc_date() {
        // 2026-10-21T10:00:00Z
        assert_eq!(day_key(1_792_576_800), "2026-10-21");
    }
}
