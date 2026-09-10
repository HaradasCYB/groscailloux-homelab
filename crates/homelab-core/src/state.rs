use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// État persistant du daemon (remplace les fichiers `.state` TSV des scripts).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    /// Items de queue Arr bloqués, clé `service:downloadId`.
    #[serde(default)]
    pub stuck: BTreeMap<String, StuckEntry>,
    /// Emails déjà traités par le poller Jellyseerr.
    #[serde(default)]
    pub onboarded: BTreeMap<String, OnboardRecord>,
    #[serde(default)]
    pub task_runs: BTreeMap<String, RunInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StuckEntry {
    pub service: String,
    pub download_id: String,
    pub first_seen: i64,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardRecord {
    pub at: i64,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub last_start: i64,
    pub last_end: Option<i64>,
    pub last_ok: Option<bool>,
    pub last_summary: String,
    pub runs: u64,
    pub errors: u64,
}

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    inner: Arc<Mutex<State>>,
}

impl StateStore {
    pub fn load(path: &Path) -> Result<Self> {
        let state = if path.is_file() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("lecture de {}", path.display()))?;
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?
        } else {
            State::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            inner: Arc::new(Mutex::new(state)),
        })
    }

    pub async fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        let guard = self.inner.lock().await;
        f(&guard)
    }

    /// Applique une mutation puis persiste atomiquement (tempfile + rename).
    pub async fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R> {
        let mut guard = self.inner.lock().await;
        let out = f(&mut guard);
        self.persist(&guard)?;
        Ok(out)
    }

    fn persist(&self, state: &State) -> Result<()> {
        let dir = self
            .path
            .parent()
            .context("state_file sans dossier parent")?;
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, state)?;
        tmp.write_all(b"\n")?;
        tmp.persist(&self.path)
            .map_err(|e| anyhow::anyhow!("écriture {} : {}", self.path.display(), e.error))?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_persists_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");
        let store = StateStore::load(&path).unwrap();
        store
            .update(|s| {
                s.stuck.insert(
                    "sonarr:abc".into(),
                    StuckEntry {
                        service: "sonarr".into(),
                        download_id: "abc".into(),
                        first_seen: 1,
                        title: "t".into(),
                    },
                );
            })
            .await
            .unwrap();
        let reloaded = StateStore::load(&path).unwrap();
        assert_eq!(reloaded.read(|s| s.stuck.len()).await, 1);
        assert!(std::fs::read_dir(path.parent().unwrap()).unwrap().count() == 1);
    }
}
