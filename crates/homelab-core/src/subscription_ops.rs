//! Opérations sur les abonnés qui touchent aux services (Jellyfin, PayPal, mails) : ce que le
//! webhook, la page `/premium`, « Mon compte », `/accounts` et les tâches appellent. Les décisions
//! pures sont dans [`crate::subscriptions`]. Toute suspension/activation passe par
//! [`crate::accounts::set_premium`] (plafond, comptes protégés, Jellyseerr, lectures arrêtées).

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::accounts::{self, Outcome};
use crate::alerts::{self, Level};
use crate::clients::paypal::EventFacts;
use crate::context::TaskContext;
use crate::mail;
use crate::state::now;
use crate::subscriptions::{self as subs, Action, Status, Subscriber, DAY};

/// Lien de paiement pré-rempli pour un membre (page `/premium`), ou le lien PayPal hébergé.
pub fn pay_url(ctx: &TaskContext, username: &str) -> String {
    match &ctx.secrets.premium_public_url {
        Some(base) => format!("{base}/premium?compte={}", urlencode(username)),
        None => ctx
            .paypal
            .as_ref()
            .map(|p| p.subscribe_url())
            .unwrap_or_default(),
    }
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

/// Une fiche pour chaque compte Jellyfin (hors admins) : les comptes actifs sans fiche arrivent
/// « à qualifier » (jamais suspendus par le cycle), les suspendus en « suspendu », les protégés et
/// exemptés en « exempt ». Les noms renommés sont suivis. Renvoie le nombre de fiches créées.
pub async fn ensure_fiches(ctx: &TaskContext) -> Result<usize> {
    let users = ctx.jellyfin.users().await?;
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
    // fiches orphelines (compte supprimé hors de homelabd) : retirées, l'historique part avec
    let ids: std::collections::HashSet<String> = users
        .iter()
        .filter_map(|u| u.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    for s in ctx.subs.list()? {
        if !ids.contains(&s.user_id) {
            info!(task = "subs", user = %s.username, "fiche orpheline retirée (compte Jellyfin absent)");
            ctx.subs.remove(&s.user_id)?;
        }
    }
    Ok(created)
}

/// Essai gratuit à l'inscription : fiche « essai » et compte activé (si `trial_days > 0`).
pub async fn start_trial(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    referral: Option<&str>,
) -> Result<bool> {
    let cfg = &ctx.cfg.subscriptions;
    let t = now();
    let days = cfg.trial_days as i64;
    if days == 0 {
        ctx.subs
            .ensure(user_id, username, Status::Suspended, None, "signup", t)?;
        return Ok(false);
    }
    let s = ctx.subs.ensure(
        user_id,
        username,
        Status::Trial,
        Some(t + days * DAY),
        "trial",
        t,
    )?;
    if let Some(code) = referral.map(str::trim).filter(|c| !c.is_empty()) {
        match ctx.subs.by_referral_code(code)? {
            Some(r) if r.user_id != user_id => {
                ctx.subs.set_referred_by(user_id, &r.user_id, t)?;
                ctx.subs.log(
                    user_id,
                    "referred",
                    &format!("parrainé par {}", r.username),
                    "signup",
                    t,
                )?;
            }
            _ => info!(task = "subs", user = %username, "code de parrainage inconnu ignoré"),
        }
    }
    if s.status != Status::Trial {
        return Ok(false);
    }
    match accounts::set_premium(ctx, user_id, true).await {
        Ok(Outcome::CapReached { premium, max }) => {
            warn!(task = "subs", user = %username, premium, max, "essai : plafond premium atteint, compte laissé suspendu");
            ctx.subs.log(
                user_id,
                "trial_blocked",
                "plafond premium atteint",
                "signup",
                t,
            )?;
            Ok(false)
        }
        Ok(_) => {
            ctx.subs.log(
                user_id,
                "trial_started",
                &format!("{days} jours"),
                "signup",
                t,
            )?;
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

/// Paiement reçu (webhook, page `/premium` ou réconciliation) : rattache l'abonnement, prolonge
/// l'échéance, réactive le compte s'il était suspendu, crédite un éventuel parrainage, prévient
/// le membre. `facts.sub_id` obligatoire ; le compte est trouvé par abonnement lié, sinon par
/// `custom_id` (nom de compte), sinon par e-mail PayPal connu.
pub async fn on_payment(
    ctx: &TaskContext,
    facts: &EventFacts,
    actor: &str,
) -> Result<Option<Subscriber>> {
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
                return Ok(None);
            };
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
                warn!(task = "subs", user = %sub.username, error = %e, "réactivation impossible")
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
                subs::activated_mail(
                    &sub.username,
                    &date_text(new_exp),
                    &ctx.secrets.jellyfin_public_url,
                ),
            )
            .await;
        }
    }
    ctx.subs.get(&sub.user_id)
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
        if ctx.subs.get(uid)?.is_none() {
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
            warn!(task = "subs", user = %s.username, error = %e, "premium non modifié");
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
                warn!(task = "subs", user = %s.username, error = %e, "réactivation impossible");
            }
        }
    }
    Ok(new)
}

async fn send_member_mail(
    ctx: &TaskContext,
    user_id: &str,
    username: &str,
    (subject, body): (String, String),
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
        info!(task = "subs", user = %username, subject, "dry-run: mail membre non envoyé");
        return false;
    }
    match mail::send_plain(smtp, username, &to, &subject, &body).await {
        Ok(()) => true,
        Err(e) => {
            warn!(task = "subs", user = %username, error = %e, "mail membre en échec");
            false
        }
    }
}

/// Un passage du cycle : rappels, grâce, suspensions. Renvoie un résumé lisible.
pub async fn run_cycle(ctx: &TaskContext) -> Result<(String, u32)> {
    let cfg = &ctx.cfg.subscriptions;
    let created = ensure_fiches(ctx).await.unwrap_or_else(|e| {
        warn!(task = "subs", error = %e, "fiches non synchronisées avec Jellyfin");
        0
    });
    let t = now();
    let dry = ctx.dry_run || cfg.cycle_dry_run;
    let mut actions = 0u32;
    let mut lines: Vec<String> = Vec::new();
    for s in ctx.subs.list()? {
        if is_exempt(ctx, &s.username) {
            continue;
        }
        for a in subs::decide(&s, t, cfg) {
            actions += 1;
            match a {
                Action::Remind(d) => {
                    let sent = send_member_mail(
                        ctx,
                        &s.user_id,
                        &s.username,
                        subs::reminder_mail(&s.username, d, s.status, &pay_url(ctx, &s.username)),
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
                                send_member_mail(
                                    ctx,
                                    &s.user_id,
                                    &s.username,
                                    subs::suspended_mail(&s.username, &pay_url(ctx, &s.username)),
                                )
                                .await;
                                lines.push(format!("suspendu : {}", s.username));
                            }
                            Err(e) => {
                                lines.push(format!("suspension impossible : {} ({e})", s.username))
                            }
                        }
                    }
                }
            }
        }
    }
    let unknown = ctx
        .subs
        .list()?
        .iter()
        .filter(|s| s.status == Status::Unknown)
        .count();
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
            "fiches={} créées={created} actions={actions}{} à_qualifier={unknown}",
            ctx.subs.list()?.len(),
            if dry { " (observation)" } else { "" }
        ),
        actions,
    ))
}

/// Relit chaque abonnement PayPal connu : statut et prochaine facturation font foi.
pub async fn reconcile(ctx: &TaskContext) -> Result<(String, u32)> {
    let Some(pp) = ctx.paypal.as_ref() else {
        return Ok(("paypal non configuré".into(), 0));
    };
    let t = now();
    let mut checked = 0u32;
    let mut fixed = 0u32;
    for s in ctx.subs.list()? {
        let Some(sid) = s.paypal_sub_id.clone() else {
            continue;
        };
        checked += 1;
        let v = match pp.subscription(&sid).await {
            Ok(v) => v,
            Err(e) => {
                warn!(task = "subs", user = %s.username, error = %e, "abonnement PayPal illisible");
                continue;
            }
        };
        let status = v.get("status").and_then(Value::as_str).unwrap_or("");
        let next = v
            .pointer("/billing_info/next_billing_time")
            .and_then(Value::as_str)
            .and_then(crate::clients::paypal::parse_time);
        let last_paid = v
            .pointer("/billing_info/last_payment/time")
            .and_then(Value::as_str)
            .and_then(crate::clients::paypal::parse_time);
        if status == "ACTIVE" {
            let expected = subs::next_expiry(None, next, t, &ctx.cfg.subscriptions);
            let cur = s.expires_at.unwrap_or(0);
            if matches!(
                s.status,
                Status::Active
                    | Status::Grace
                    | Status::Trial
                    | Status::Unknown
                    | Status::Suspended
            ) && (expected > cur + DAY / 2)
            {
                let facts = EventFacts {
                    event_id: format!("reconcile:{sid}:{}", last_paid.unwrap_or(t)),
                    event_type: "RECONCILE".into(),
                    sub_id: Some(sid.clone()),
                    next_billing: next,
                    ..Default::default()
                };
                if ctx.subs.record_paypal_event(
                    &facts.event_id,
                    "RECONCILE",
                    Some(&sid),
                    "réconciliation",
                    t,
                )? {
                    on_payment(ctx, &facts, "reconcile").await?;
                    fixed += 1;
                }
            }
        } else if s.status == Status::Active && s.expires_at.is_none() {
            ctx.subs.log(
                &s.user_id,
                "paypal",
                &format!("abonnement {status}"),
                "reconcile",
                t,
            )?;
        }
    }
    Ok((
        format!("abonnements PayPal vérifiés={checked} corrigés={fixed}"),
        fixed,
    ))
}
