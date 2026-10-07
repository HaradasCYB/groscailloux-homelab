//! Quota du compte seedbox : la seedbox écrit `~/media/.homelab/quota.json` toutes les 15 min (cron `quota -w`), le
//! VPS le lit à travers le montage rclone. Lu par `/status.html` et, depuis le 2026-10-07, par `seedbox_health`
//! (alerte au franchissement de `quota_alert_pct`). L'espace utile de la seedbox est ce quota (3,7 To), pas le
//! `df` du disque partagé de l'hébergeur.
use std::time::Duration;

use serde::Deserialize;

use crate::context::TaskContext;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SeedboxQuota {
    pub used_kb: u64,
    pub quota_kb: u64,
    pub at: i64,
}

impl SeedboxQuota {
    /// Pourcentage utilisé, arrondi, plafonné à 100 ; `None` sans quota connu.
    pub fn percent(&self) -> Option<u8> {
        (self.quota_kb > 0).then(|| {
            ((self.used_kb as f64 / self.quota_kb as f64) * 100.0)
                .round()
                .min(100.0) as u8
        })
    }

    /// « 2 108 Go sur 3,7 To » pour un message.
    pub fn describe(&self) -> String {
        format!(
            "{:.0} Go utilisés sur {} To",
            self.used_kb as f64 / 1e6,
            format!("{:.1}", self.quota_kb as f64 / 1e9).replace('.', ",")
        )
    }
}

/// Contenu de `quota.json` ; `None` s'il est illisible ou incohérent (aucun quota ⇒ pas de pourcentage).
pub fn parse(raw: &str) -> Option<SeedboxQuota> {
    serde_json::from_str::<SeedboxQuota>(raw)
        .ok()
        .filter(|q| q.quota_kb > 0)
}

/// Relevé courant du quota, ou `None` si le montage est absent, vide ou trop lent (la seedbox peut être
/// injoignable : l'appelant ne doit jamais rester bloqué).
pub async fn read(ctx: &TaskContext) -> Option<SeedboxQuota> {
    let cfg = &ctx.cfg.seedbox;
    if !cfg.enabled {
        return None;
    }
    // le cache de répertoires rclone (1 h) masquerait le quota réécrit toutes les 15 min
    let rc = format!("{}/vfs/refresh", cfg.rclone_rc.trim_end_matches('/'));
    if !ctx.dry_run {
        let _ = ctx
            .http
            .post(&rc)
            .json(&serde_json::json!({ "dir": ".homelab" }))
            .timeout(Duration::from_secs(2))
            .send()
            .await;
    }
    let path = cfg.mount_point.join(".homelab/quota.json");
    let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(path).ok());
    let raw = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .ok()?
        .ok()??;
    parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_matches_the_status_page() {
        let q = SeedboxQuota {
            used_kb: 1_429_000_000,
            quota_kb: 3_725_000_000,
            at: 0,
        };
        assert_eq!(q.percent(), Some(38));
        let full = SeedboxQuota {
            used_kb: 4_000_000_000,
            quota_kb: 3_725_000_000,
            at: 0,
        };
        assert_eq!(full.percent(), Some(100), "plafonné");
        let none = SeedboxQuota {
            used_kb: 1,
            quota_kb: 0,
            at: 0,
        };
        assert_eq!(none.percent(), None);
    }

    #[test]
    fn quota_json_is_parsed_and_junk_refused() {
        let q = parse(r#"{"used_kb": 2108000000, "quota_kb": 3725000000, "at": 1790000000}"#)
            .expect("quota");
        assert_eq!(q.percent(), Some(57));
        assert_eq!(q.describe(), "2108 Go utilisés sur 3,7 To");
        assert_eq!(parse("pas du json"), None);
        assert_eq!(parse(r#"{"used_kb":1,"quota_kb":0,"at":1}"#), None);
        assert_eq!(parse(r#"{"used_kb":1}"#), None);
    }
}
