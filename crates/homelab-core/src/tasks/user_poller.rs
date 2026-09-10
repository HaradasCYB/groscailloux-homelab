//! Détecte les users locaux créés via l'UI Jellyseerr (« Add User ») et les
//! remplace par un onboarding unifié Jellyfin + Jellyseerr-imported + mail
//! (ex `jellyseerr-user-poller.sh`, toutes les 60 s).

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::DateTime;
use serde_json::Value;
use tracing::{info, warn};

use super::onboard::{self, OnboardRequest};
use super::{Report, Task};
use crate::config::Config;
use crate::context::TaskContext;
use crate::state::{now, OnboardRecord};

pub struct UserPoller;

const USERTYPE_LOCAL: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: i64,
    pub email: String,
    pub username: String,
}

pub fn candidates(users: &[Value], admin_id: i64, min_created: i64) -> Vec<Candidate> {
    users
        .iter()
        .filter_map(|u| {
            let id = u.get("id").and_then(Value::as_i64)?;
            if u.get("userType").and_then(Value::as_i64) != Some(USERTYPE_LOCAL) || id == admin_id {
                return None;
            }
            let email = u.get("email").and_then(Value::as_str)?.trim().to_string();
            if !onboard::valid_email(&email) {
                return None;
            }
            let created = u.get("createdAt").and_then(Value::as_str)?;
            let ts = DateTime::parse_from_rfc3339(created).ok()?.timestamp();
            if ts < min_created {
                return None;
            }
            let username = u
                .get("username")
                .and_then(Value::as_str)
                .or_else(|| u.get("jellyfinUsername").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());
            Some(Candidate {
                id,
                email,
                username,
            })
        })
        .collect()
}

#[async_trait]
impl Task for UserPoller {
    fn name(&self) -> &'static str {
        "user_poller"
    }

    fn interval(&self, cfg: &Config) -> Duration {
        Duration::from_secs(cfg.tasks.user_poller.interval_secs)
    }

    async fn run(&self, ctx: &TaskContext) -> Result<Report> {
        let cfg = &ctx.cfg.tasks.user_poller;
        if ctx.jellyseerr.status().await.is_err() {
            return Ok(Report::new("jellyseerr_unreachable", 0));
        }
        let users = ctx.jellyseerr.users(200).await?;
        let ts = now();
        let found = candidates(&users, cfg.admin_user_id, ts - cfg.max_age_secs);
        let mut actions = 0u32;
        for c in found {
            let key = c.email.to_lowercase();
            if ctx.state.read(|s| s.onboarded.contains_key(&key)).await {
                continue;
            }
            if ctx.jellyfin.find_user(&c.username).await?.is_some() {
                info!(task = "user_poller", id = c.id, username = %c.username, "skip: jellyfin_user_already_exists");
                ctx.state
                    .update(|s| {
                        s.onboarded.insert(
                            key.clone(),
                            OnboardRecord {
                                at: ts,
                                outcome: "skip_existing_jf".into(),
                            },
                        )
                    })
                    .await?;
                continue;
            }
            info!(task = "user_poller", id = c.id, username = %c.username, email = %c.email, "detected");
            if ctx.dry_run {
                info!(
                    task = "user_poller",
                    id = c.id,
                    "dry-run: would delete local user and onboard"
                );
                actions += 1;
                continue;
            }
            if let Err(e) = ctx.jellyseerr.delete_user(c.id).await {
                warn!(task = "user_poller", id = c.id, error = %e, "delete_local_failed");
                continue;
            }
            info!(task = "user_poller", id = c.id, "deleted_local");
            let req = OnboardRequest {
                username: c.username.clone(),
                email: c.email.clone(),
                password: None,
            };
            let outcome = match onboard::run(ctx, req).await {
                Ok(r) => {
                    info!(task = "user_poller", username = %c.username, email = %c.email, mail_sent = r.mail_sent, "onboarded");
                    actions += 1;
                    "onboarded"
                }
                Err(e) => {
                    warn!(task = "user_poller", username = %c.username, email = %c.email, error = %e, "onboard_failed");
                    "onboard_failed"
                }
            };
            ctx.state
                .update(|s| {
                    s.onboarded.insert(
                        key.clone(),
                        OnboardRecord {
                            at: ts,
                            outcome: outcome.into(),
                        },
                    )
                })
                .await?;
        }
        Ok(Report::new(format!("actions={actions}"), actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_local_recent_users_with_email() {
        let users = vec![
            json!({"id":1,"userType":2,"email":"admin@x.io","createdAt":"2026-09-10T10:00:00.000Z"}),
            json!({"id":2,"userType":2,"email":"new@x.io","username":"newbie","createdAt":"2026-09-10T10:00:00.000Z"}),
            json!({"id":3,"userType":1,"email":"jf@x.io","createdAt":"2026-09-10T10:00:00.000Z"}),
            json!({"id":4,"userType":2,"email":"","createdAt":"2026-09-10T10:00:00.000Z"}),
            json!({"id":5,"userType":2,"email":"old@x.io","createdAt":"2020-01-01T00:00:00.000Z"}),
            json!({"id":6,"userType":2,"email":"noname@x.io","createdAt":"2026-09-10T10:00:00.000Z"}),
        ];
        let min = DateTime::parse_from_rfc3339("2026-09-10T09:55:00Z")
            .unwrap()
            .timestamp();
        let got = candidates(&users, 1, min);
        assert_eq!(
            got,
            vec![
                Candidate {
                    id: 2,
                    email: "new@x.io".into(),
                    username: "newbie".into()
                },
                Candidate {
                    id: 6,
                    email: "noname@x.io".into(),
                    username: "noname".into()
                },
            ]
        );
    }
}
