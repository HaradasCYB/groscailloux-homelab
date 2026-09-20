//! Client PayPal REST (abonnements) : jeton OAuth mis en cache, lecture d'un abonnement,
//! vérification de signature des webhooks, création du webhook, recherche de transactions
//! (rattachement des abonnés existants). Sandbox ou Live selon `PAYPAL_ENV`. Aucun secret n'est
//! journalisé.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::config::PayPal as PayPalConfig;

pub struct PayPalClient {
    http: reqwest::Client,
    cfg: PayPalConfig,
    token: Mutex<Option<(String, Instant)>>,
}

/// Événements auxquels le webhook est abonné.
pub const WEBHOOK_EVENTS: &[&str] = &[
    "BILLING.SUBSCRIPTION.ACTIVATED",
    "BILLING.SUBSCRIPTION.CANCELLED",
    "BILLING.SUBSCRIPTION.SUSPENDED",
    "BILLING.SUBSCRIPTION.EXPIRED",
    "BILLING.SUBSCRIPTION.PAYMENT.FAILED",
    "BILLING.SUBSCRIPTION.RE-ACTIVATED",
    "PAYMENT.SALE.COMPLETED",
    "PAYMENT.SALE.REFUNDED",
    "PAYMENT.SALE.REVERSED",
];

impl PayPalClient {
    pub fn new(http: reqwest::Client, cfg: PayPalConfig) -> Self {
        Self {
            http,
            cfg,
            token: Mutex::new(None),
        }
    }

    pub fn base(&self) -> &'static str {
        if self.cfg.sandbox {
            "https://api-m.sandbox.paypal.com"
        } else {
            "https://api-m.paypal.com"
        }
    }

    pub fn cfg(&self) -> &PayPalConfig {
        &self.cfg
    }

    /// Lien d'abonnement hébergé par PayPal pour le plan configuré (repli sans notre page).
    pub fn subscribe_url(&self) -> String {
        let host = if self.cfg.sandbox {
            "https://www.sandbox.paypal.com"
        } else {
            "https://www.paypal.com"
        };
        format!(
            "{host}/webapps/billing/plans/subscribe?plan_id={}",
            self.cfg.plan_id
        )
    }

    async fn token(&self) -> Result<String> {
        if let Some((t, at)) = self.token.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            if at.elapsed() < Duration::from_secs(8 * 60) {
                return Ok(t);
            }
        }
        let resp = self
            .http
            .post(format!("{}/v1/oauth2/token", self.base()))
            .basic_auth(&self.cfg.client_id, Some(self.cfg.secret.expose()))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .context("paypal oauth")?;
        if !resp.status().is_success() {
            return Err(anyhow!("paypal oauth: HTTP {}", resp.status()));
        }
        let v: Value = resp.json().await?;
        let t = v
            .get("access_token")
            .and_then(Value::as_str)
            .context("paypal oauth: pas de jeton")?
            .to_string();
        *self.token.lock().unwrap_or_else(|e| e.into_inner()) = Some((t.clone(), Instant::now()));
        Ok(t)
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let t = self.token().await?;
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.base()))
            .bearer_auth(t)
            .header("Content-Type", "application/json");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("paypal {method} {path}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(anyhow!("paypal {method} {path}: HTTP {status} {msg}"));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("paypal {path}: JSON invalide"))
    }

    /// Détail d'un abonnement (`status`, `custom_id`, `subscriber.email_address`,
    /// `billing_info.next_billing_time`, `plan_id`).
    pub async fn subscription(&self, id: &str) -> Result<Value> {
        self.call(
            reqwest::Method::GET,
            &format!("/v1/billing/subscriptions/{id}"),
            None,
        )
        .await
    }

    pub async fn plan(&self, id: &str) -> Result<Value> {
        self.call(
            reqwest::Method::GET,
            &format!("/v1/billing/plans/{id}"),
            None,
        )
        .await
    }

    /// Vérifie la signature d'un webhook auprès de PayPal (`verify-webhook-signature`). `raw_body` :
    /// le corps **tel que reçu**, octet pour octet — re-sérialisé (clés triées), la signature ne
    /// correspond plus et PayPal répond FAILURE.
    pub async fn verify_webhook(&self, headers: &WebhookHeaders, raw_body: &str) -> Result<bool> {
        let Some(webhook_id) = self.cfg.webhook_id.as_deref() else {
            return Err(anyhow!("PAYPAL_WEBHOOK_ID absent : webhook non vérifiable"));
        };
        let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
        let body = format!(
            "{{\"auth_algo\":{},\"cert_url\":{},\"transmission_id\":{},\"transmission_sig\":{},\"transmission_time\":{},\"webhook_id\":{},\"webhook_event\":{}}}",
            q(&headers.auth_algo),
            q(&headers.cert_url),
            q(&headers.transmission_id),
            q(&headers.transmission_sig),
            q(&headers.transmission_time),
            q(webhook_id),
            raw_body.trim()
        );
        let t = self.token().await?;
        let resp = self
            .http
            .post(format!(
                "{}/v1/notifications/verify-webhook-signature",
                self.base()
            ))
            .bearer_auth(t)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .context("paypal verify-webhook-signature")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "paypal verify-webhook-signature: HTTP {status} {}",
                text.chars().take(200).collect::<String>()
            ));
        }
        let v: Value = serde_json::from_str(&text).context("verify: JSON invalide")?;
        Ok(v.get("verification_status").and_then(Value::as_str) == Some("SUCCESS"))
    }

    pub async fn webhooks(&self) -> Result<Vec<Value>> {
        let v = self
            .call(reqwest::Method::GET, "/v1/notifications/webhooks", None)
            .await?;
        Ok(v.get("webhooks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Crée (ou retrouve) le webhook pointant sur `url`, abonné à [`WEBHOOK_EVENTS`]. Renvoie son id.
    pub async fn ensure_webhook(&self, url: &str) -> Result<String> {
        for w in self.webhooks().await? {
            if w.get("url").and_then(Value::as_str) == Some(url) {
                if let Some(id) = w.get("id").and_then(Value::as_str) {
                    return Ok(id.to_string());
                }
            }
        }
        let v = self
            .call(
                reqwest::Method::POST,
                "/v1/notifications/webhooks",
                Some(json!({
                    "url": url,
                    "event_types": WEBHOOK_EVENTS.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
                })),
            )
            .await?;
        v.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("paypal: webhook créé sans id")
    }

    /// Envoie un événement de test sur le webhook (`simulate-event`) : le corps arrive signé comme
    /// un vrai, avec des données factices.
    pub async fn simulate_event(&self, event_type: &str) -> Result<Value> {
        let webhook_id = self
            .cfg
            .webhook_id
            .as_deref()
            .context("PAYPAL_WEBHOOK_ID absent")?;
        self.call(
            reqwest::Method::POST,
            "/v1/notifications/simulate-event",
            Some(json!({ "webhook_id": webhook_id, "event_type": event_type })),
        )
        .await
    }

    /// Transactions des `days` derniers jours (31 max par appel PayPal, on enchaîne), pour retrouver
    /// les abonnements existants : chaque transaction porte `paypal_reference_id` = `I-…`.
    pub async fn subscription_transactions(&self, days: i64) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let now = chrono::Utc::now();
        let mut end = now;
        let mut left = days.max(1);
        while left > 0 {
            let span = left.min(31);
            let start = end - chrono::Duration::days(span);
            let path = format!(
                "/v1/reporting/transactions?start_date={}&end_date={}&fields=all&page_size=500",
                start.format("%Y-%m-%dT%H:%M:%SZ"),
                end.format("%Y-%m-%dT%H:%M:%SZ")
            );
            let v = self.call(reqwest::Method::GET, &path, None).await?;
            if let Some(list) = v.get("transaction_details").and_then(Value::as_array) {
                out.extend(list.iter().cloned());
            }
            end = start;
            left -= span;
        }
        Ok(out)
    }
}

/// En-têtes de signature envoyés par PayPal avec chaque webhook.
#[derive(Debug, Clone, Default)]
pub struct WebhookHeaders {
    pub auth_algo: String,
    pub cert_url: String,
    pub transmission_id: String,
    pub transmission_sig: String,
    pub transmission_time: String,
}

impl WebhookHeaders {
    pub fn complete(&self) -> bool {
        !(self.auth_algo.is_empty()
            || self.cert_url.is_empty()
            || self.transmission_id.is_empty()
            || self.transmission_sig.is_empty()
            || self.transmission_time.is_empty())
    }
}

/// Ce qu'un événement d'abonnement nous apprend, quel que soit son type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFacts {
    pub event_id: String,
    pub event_type: String,
    pub sub_id: Option<String>,
    pub custom_id: Option<String>,
    pub email: Option<String>,
    pub status: Option<String>,
    pub next_billing: Option<i64>,
    pub amount: Option<String>,
}

/// Extrait les champs utiles d'un webhook (`resource.id` ou `billing_agreement_id`, `custom_id`,
/// e-mail du payeur, prochaine facturation, montant).
pub fn event_facts(event: &Value) -> EventFacts {
    let res = event.get("resource").cloned().unwrap_or(Value::Null);
    let et = event
        .get("event_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let sub_id = if et.starts_with("BILLING.SUBSCRIPTION") {
        res.get("id").and_then(Value::as_str)
    } else {
        res.get("billing_agreement_id").and_then(Value::as_str)
    }
    .map(str::to_string);
    EventFacts {
        event_id: event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        event_type: et,
        sub_id,
        custom_id: res
            .get("custom_id")
            .or_else(|| res.get("custom"))
            .and_then(Value::as_str)
            .map(str::to_string),
        email: res
            .pointer("/subscriber/email_address")
            .and_then(Value::as_str)
            .map(|s| s.to_lowercase()),
        status: res
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        next_billing: res
            .pointer("/billing_info/next_billing_time")
            .and_then(Value::as_str)
            .and_then(parse_time),
        amount: res
            .pointer("/amount/total")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// `2026-10-20T10:00:00Z` → epoch.
pub fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_from_subscription_and_sale_events() {
        let sub = json!({"id":"WH-1","event_type":"BILLING.SUBSCRIPTION.ACTIVATED","resource":{"id":"I-ABC123456789","custom_id":"alice","status":"ACTIVE","subscriber":{"email_address":"A@Ex.fr"},"billing_info":{"next_billing_time":"2026-10-20T10:00:00Z"}}});
        let f = event_facts(&sub);
        assert_eq!(f.sub_id.as_deref(), Some("I-ABC123456789"));
        assert_eq!(f.custom_id.as_deref(), Some("alice"));
        assert_eq!(f.email.as_deref(), Some("a@ex.fr"));
        assert_eq!(f.next_billing, parse_time("2026-10-20T10:00:00Z"));
        let sale = json!({"id":"WH-2","event_type":"PAYMENT.SALE.COMPLETED","resource":{"id":"7X","billing_agreement_id":"I-ABC123456789","amount":{"total":"3.50","currency":"EUR"}}});
        let f = event_facts(&sale);
        assert_eq!(f.sub_id.as_deref(), Some("I-ABC123456789"));
        assert_eq!(f.amount.as_deref(), Some("3.50"));
        assert!(parse_time("2026-10-01").is_some());
    }
}
