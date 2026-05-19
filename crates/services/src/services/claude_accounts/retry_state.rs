//! On-disk persistence of in-flight Claude retry state, keyed by
//! `execution_process.id`. Written before every back-off sleep so a crash or
//! restart mid-retry doesn't lose the attempt counter or the scheduled
//! resume time (spec FR-023, SC-010).
//!
//! The on-disk record is intentionally small — it does NOT carry the
//! `workspace` or `executor_action`. Those are re-derived from the SQLite
//! `execution_processes` row on resume (the executor_action is already
//! persisted there as JSON).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Minimal retry state required to resume a Claude executor's retry sequence
/// after an application restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeRetryState {
    pub execution_process_id: Uuid,
    /// 1-indexed attempt number. On resume, the rotator picks up here.
    pub attempt_number: u32,
    /// Account ids already tried for this execution_process — passed to
    /// `pick_next` so resume doesn't re-pick a failed account.
    pub accounts_tried: Vec<Uuid>,
    /// Absolute UTC wake-up time. On resume, sleep `max(0, scheduled_resume_at - now)`.
    pub scheduled_resume_at: DateTime<Utc>,
    /// When this record was last persisted (for diagnostics; sweep-stale uses
    /// the file mtime, not this field).
    pub updated_at: DateTime<Utc>,
}

impl ClaudeRetryState {
    pub fn path_for(execution_process_id: Uuid) -> PathBuf {
        utils::assets::claude_retry_state_dir().join(format!("{execution_process_id}.json"))
    }

    pub async fn save(&self) -> std::io::Result<()> {
        let path = Self::path_for(self.execution_process_id);
        let tmp = path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&tmp)?;
        serde_json::to_writer_pretty(&file, self)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub async fn load_for(execution_process_id: Uuid) -> Option<Self> {
        let path = Self::path_for(execution_process_id);
        if !path.exists() {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    ?path,
                    "claude retry-state file corrupt; renaming to .bad"
                );
                let bad = path.with_extension("bad");
                let _ = std::fs::rename(&path, bad);
                None
            }
        }
    }

    pub async fn delete(execution_process_id: Uuid) -> std::io::Result<()> {
        let path = Self::path_for(execution_process_id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Delete retry-state files older than 30 days. Called from the server
    /// startup sweep so stale, never-resumed records don't pile up forever.
    pub fn sweep_stale(now: DateTime<Utc>) {
        let dir = utils::assets::claude_retry_state_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let cutoff = now - chrono::Duration::days(30);
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let modified_chrono: DateTime<Utc> = modified.into();
            if modified_chrono < cutoff {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Enumerate all persisted retry-state files. Called from the startup
    /// resume sweep; corrupt files have already been renamed to .bad by
    /// `load_for`.
    pub async fn list_all() -> Vec<Self> {
        let dir = utils::assets::claude_retry_state_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            // Skip tmp + bad files; only consume <uuid>.json.
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(id) = stem.parse::<Uuid>() else {
                continue;
            };
            if let Some(state) = Self::load_for(id).await {
                out.push(state);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;

    use super::*;

    fn fresh_state() -> ClaudeRetryState {
        ClaudeRetryState {
            execution_process_id: Uuid::new_v4(),
            attempt_number: 3,
            accounts_tried: vec![Uuid::new_v4(), Uuid::new_v4()],
            scheduled_resume_at: Utc::now() + ChronoDuration::seconds(60),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn save_then_load_round_trip() {
        let state = fresh_state();
        state.save().await.unwrap();
        let loaded = ClaudeRetryState::load_for(state.execution_process_id)
            .await
            .expect("file should load");
        assert_eq!(loaded.attempt_number, state.attempt_number);
        assert_eq!(loaded.accounts_tried, state.accounts_tried);
        assert_eq!(
            loaded.scheduled_resume_at.timestamp(),
            state.scheduled_resume_at.timestamp()
        );
        // Cleanup
        ClaudeRetryState::delete(state.execution_process_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_is_idempotent_on_missing_file() {
        let id = Uuid::new_v4();
        ClaudeRetryState::delete(id).await.unwrap();
        ClaudeRetryState::delete(id).await.unwrap();
    }

    #[tokio::test]
    async fn list_all_returns_persisted_states_and_ignores_garbage() {
        let dir = utils::assets::claude_retry_state_dir();
        // Plant a corrupt sibling that should be silently ignored.
        let trash = dir.join("not-a-uuid.json");
        std::fs::write(&trash, b"not valid json").unwrap();
        let stray = dir.join("00000000-0000-0000-0000-000000000099.json");
        std::fs::write(&stray, b"also not json").unwrap();

        let state = fresh_state();
        state.save().await.unwrap();

        let all = ClaudeRetryState::list_all().await;
        let ours = all
            .iter()
            .find(|s| s.execution_process_id == state.execution_process_id);
        assert!(ours.is_some(), "our saved state should be listed");

        // Cleanup
        ClaudeRetryState::delete(state.execution_process_id)
            .await
            .unwrap();
        let _ = std::fs::remove_file(&trash);
        // The malformed stray will have been renamed to .bad by load_for.
        let _ = std::fs::remove_file(dir.join("00000000-0000-0000-0000-000000000099.bad"));
    }
}
