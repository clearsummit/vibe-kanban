//! File-backed `Vec<ClaudeAccount>` store. Mirrors the pattern of
//! `oauth_credentials.rs` (tmp-file + rename, 0600 on Unix). NEVER hits SQLite —
//! credentials must not flow into the syncing DB (spec FR-007).

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

use super::oauth::{ClaudeOAuthClient, ClaudeOAuthError, OauthAccountInfo};
use super::types::{
    ClaudeAccount, ClaudeAccountLastError, ClaudeAccountStatus, ClaudeAccountThrottleReason,
    ClaudeAccountUsageWindow, ClaudeAccountView, ClaudeOAuthCredentials, FailureClass,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("account {0} not found")]
    NotFound(Uuid),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    OAuth(#[from] ClaudeOAuthError),
}

pub struct ClaudeAccountsStore {
    path: PathBuf,
    accounts: RwLock<Vec<ClaudeAccount>>,
}

impl ClaudeAccountsStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            accounts: RwLock::new(Vec::new()),
        }
    }

    /// Load from disk. On parse failure, rename to `.bad` and start empty.
    pub async fn load(&self) -> std::io::Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let bytes = std::fs::read(&self.path)?;
        match serde_json::from_slice::<Vec<ClaudeAccount>>(&bytes) {
            Ok(loaded) => {
                *self.accounts.write().await = loaded;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(?e, "claude_accounts.json corrupt; renaming to .bad");
                let bad = self.path.with_extension("bad");
                let _ = std::fs::rename(&self.path, bad);
                *self.accounts.write().await = Vec::new();
                Ok(())
            }
        }
    }

    async fn save_locked(&self, accounts: &[ClaudeAccount]) -> std::io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&tmp)?;
        serde_json::to_writer_pretty(&file, accounts)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub async fn list_views(&self) -> Vec<ClaudeAccountView> {
        self.accounts
            .read()
            .await
            .iter()
            .map(ClaudeAccountView::from)
            .collect()
    }

    pub async fn list_full(&self) -> Vec<ClaudeAccount> {
        self.accounts.read().await.clone()
    }

    pub async fn count(&self) -> usize {
        self.accounts.read().await.len()
    }

    pub async fn add(
        &self,
        creds: ClaudeOAuthCredentials,
        oauth_info: Option<&OauthAccountInfo>,
    ) -> Result<ClaudeAccountView, StoreError> {
        let mut guard = self.accounts.write().await;
        let next_index = guard.len() + 1;
        let label = oauth_info
            .and_then(|i| i.email.clone())
            .unwrap_or_else(|| format!("Account {next_index}"));
        let email = oauth_info.and_then(|i| i.email.clone());
        let account = ClaudeAccount {
            id: Uuid::new_v4(),
            label,
            email,
            created_at: Utc::now(),
            last_used_at: None,
            status: ClaudeAccountStatus::Active,
            throttled_until: None,
            throttle_reason: None,
            five_hour_window: ClaudeAccountUsageWindow::default(),
            weekly_window: ClaudeAccountUsageWindow::default(),
            last_error: None,
            credentials: creds,
        };
        let view = ClaudeAccountView::from(&account);
        guard.push(account);
        self.save_locked(&guard).await?;
        Ok(view)
    }

    pub async fn replace_credentials(
        &self,
        id: Uuid,
        creds: ClaudeOAuthCredentials,
        oauth_info: Option<&OauthAccountInfo>,
    ) -> Result<ClaudeAccountView, StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.credentials = creds;
        if let Some(info) = oauth_info
            && let Some(email) = &info.email
        {
            acct.email = Some(email.clone());
        }
        acct.status = ClaudeAccountStatus::Active;
        acct.throttled_until = None;
        acct.throttle_reason = None;
        acct.last_error = None;
        let view = ClaudeAccountView::from(&*acct);
        self.save_locked(&guard).await?;
        Ok(view)
    }

    pub async fn update_label(
        &self,
        id: Uuid,
        label: String,
    ) -> Result<ClaudeAccountView, StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.label = label;
        let view = ClaudeAccountView::from(&*acct);
        self.save_locked(&guard).await?;
        Ok(view)
    }

    pub async fn set_disabled(
        &self,
        id: Uuid,
        disabled: bool,
    ) -> Result<ClaudeAccountView, StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.status = if disabled {
            ClaudeAccountStatus::Disabled
        } else if acct.status == ClaudeAccountStatus::Disabled {
            ClaudeAccountStatus::Active
        } else {
            acct.status
        };
        let view = ClaudeAccountView::from(&*acct);
        self.save_locked(&guard).await?;
        Ok(view)
    }

    pub async fn remove(&self, id: Uuid) -> Result<(), StoreError> {
        let mut guard = self.accounts.write().await;
        let before = guard.len();
        guard.retain(|a| a.id != id);
        if guard.len() == before {
            return Err(StoreError::NotFound(id));
        }
        self.save_locked(&guard).await?;
        Ok(())
    }

    pub async fn mark_throttled(
        &self,
        id: Uuid,
        until: DateTime<Utc>,
        reason: ClaudeAccountThrottleReason,
    ) -> Result<(), StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.status = ClaudeAccountStatus::Throttled;
        acct.throttled_until = Some(until);
        acct.throttle_reason = Some(reason);
        self.save_locked(&guard).await?;
        Ok(())
    }

    pub async fn mark_needs_reauth(
        &self,
        id: Uuid,
        message: impl Into<String>,
    ) -> Result<(), StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.status = ClaudeAccountStatus::NeedsReauth;
        acct.last_error = Some(ClaudeAccountLastError {
            message: message.into(),
            classification: FailureClass::NeedsReauth,
            occurred_at: Utc::now(),
        });
        self.save_locked(&guard).await?;
        Ok(())
    }

    /// Clear a `Throttled` account whose `throttled_until` has passed. Called
    /// at the start of each `pick_next()`.
    async fn unthrottle_expired_locked(&self, accounts: &mut [ClaudeAccount]) {
        let now = Utc::now();
        for acct in accounts.iter_mut() {
            if acct.status == ClaudeAccountStatus::Throttled
                && acct.throttled_until.map(|t| t <= now).unwrap_or(false)
            {
                acct.status = ClaudeAccountStatus::Active;
                acct.throttled_until = None;
                acct.throttle_reason = None;
                acct.five_hour_window = ClaudeAccountUsageWindow::default();
                if acct.throttle_reason == Some(ClaudeAccountThrottleReason::Weekly) {
                    acct.weekly_window = ClaudeAccountUsageWindow::default();
                }
            }
        }
    }

    /// Round-robin select the next non-throttled, non-disabled, non-needs-reauth
    /// account, skipping any in `exclude`. Returns the soonest `throttled_until`
    /// if no healthy account remains.
    pub async fn pick_next(
        &self,
        exclude: &[Uuid],
    ) -> PickNextResult {
        let mut guard = self.accounts.write().await;
        self.unthrottle_expired_locked(&mut guard).await;
        if guard.is_empty() {
            return PickNextResult::NoAccountsEnrolled;
        }

        // Healthy accounts, ordered by least-recently-used.
        let mut healthy: Vec<&ClaudeAccount> = guard
            .iter()
            .filter(|a| a.status == ClaudeAccountStatus::Active && !exclude.contains(&a.id))
            .collect();
        healthy.sort_by_key(|a| a.last_used_at.unwrap_or(a.created_at));
        if let Some(chosen) = healthy.first() {
            return PickNextResult::Account((*chosen).clone());
        }

        // No healthy account — find soonest unthrottle time among Throttled.
        let soonest = guard
            .iter()
            .filter(|a| a.status == ClaudeAccountStatus::Throttled && !exclude.contains(&a.id))
            .filter_map(|a| a.throttled_until)
            .min();
        match soonest {
            Some(t) => PickNextResult::AllThrottledUntil(t),
            None => PickNextResult::NoUsableAccounts,
        }
    }

    /// Record a successful spawn against `id`. Debounced to 1 write/sec/account.
    pub async fn record_success(
        &self,
        id: Uuid,
        debounce_map: &DashMap<Uuid, AtomicU64>,
    ) -> Result<(), StoreError> {
        let now_ms = Utc::now().timestamp_millis();
        let entry = debounce_map.entry(id).or_insert_with(|| AtomicU64::new(0));
        let last = entry.load(Ordering::Relaxed) as i64;
        if now_ms - last < 1000 {
            // Still increment the counter in memory but skip the disk write.
            return Ok(());
        }
        entry.store(now_ms as u64, Ordering::Relaxed);
        drop(entry);

        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.last_used_at = Some(Utc::now());
        acct.five_hour_window.used = acct.five_hour_window.used.saturating_add(1);
        acct.weekly_window.used = acct.weekly_window.used.saturating_add(1);
        self.save_locked(&guard).await?;
        Ok(())
    }

    pub async fn record_failure(
        &self,
        id: Uuid,
        class: FailureClass,
        message: impl Into<String>,
    ) -> Result<(), StoreError> {
        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.last_error = Some(ClaudeAccountLastError {
            message: message.into(),
            classification: class,
            occurred_at: Utc::now(),
        });
        self.save_locked(&guard).await?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum PickNextResult {
    Account(ClaudeAccount),
    AllThrottledUntil(DateTime<Utc>),
    NoUsableAccounts,
    NoAccountsEnrolled,
}

/// Top-level service held by the deployment. Wraps the store, refresh mutex
/// map, and an `Arc<ClaudeOAuthClient>` for code-exchange + refresh.
pub struct ClaudeAccountsService {
    pub store: Arc<ClaudeAccountsStore>,
    pub oauth: Arc<ClaudeOAuthClient>,
    refresh_locks: Arc<DashMap<Uuid, Arc<Mutex<()>>>>,
    record_debounce: Arc<DashMap<Uuid, AtomicU64>>,
}

impl ClaudeAccountsService {
    pub fn new(store: Arc<ClaudeAccountsStore>, oauth: Arc<ClaudeOAuthClient>) -> Self {
        Self {
            store,
            oauth,
            refresh_locks: Arc::new(DashMap::new()),
            record_debounce: Arc::new(DashMap::new()),
        }
    }

    pub async fn refresh_lock(&self, id: Uuid) -> OwnedMutexGuard<()> {
        let entry = self
            .refresh_locks
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(())));
        entry.value().clone().lock_owned().await
    }

    /// Returns a valid credential bundle for the given account, refreshing if
    /// the access token is within 60s of expiry. Refresh is serialized
    /// per-account by `refresh_lock`.
    pub async fn get_active_credentials(
        &self,
        id: Uuid,
    ) -> Result<ClaudeOAuthCredentials, StoreError> {
        let creds = {
            let guard = self.store.accounts.read().await;
            let acct = guard
                .iter()
                .find(|a| a.id == id)
                .ok_or(StoreError::NotFound(id))?;
            acct.credentials.clone()
        };
        let expires_soon = Utc::now() + chrono::Duration::seconds(60) >= creds.expires_at;
        if !expires_soon {
            return Ok(creds);
        }
        let _guard = self.refresh_lock(id).await;
        // Re-check after acquiring the lock — another caller may have refreshed.
        let mut current = {
            let guard = self.store.accounts.read().await;
            let acct = guard
                .iter()
                .find(|a| a.id == id)
                .ok_or(StoreError::NotFound(id))?;
            acct.credentials.clone()
        };
        if Utc::now() + chrono::Duration::seconds(60) < current.expires_at {
            return Ok(current);
        }
        if let Err(e) = self.oauth.refresh(&mut current).await {
            let msg = e.to_string();
            self.store.mark_needs_reauth(id, &msg).await?;
            return Err(StoreError::OAuth(e));
        }
        // Persist the rotated credentials.
        let mut guard = self.store.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.credentials = current.clone();
        self.store.save_locked(&guard).await?;
        Ok(current)
    }

    pub async fn record_success(&self, id: Uuid) -> Result<(), StoreError> {
        self.store.record_success(id, &self.record_debounce).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use tempfile::TempDir;

    fn fake_creds(expires_in: i64) -> ClaudeOAuthCredentials {
        ClaudeOAuthCredentials {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + ChronoDuration::seconds(expires_in),
            scopes: vec!["user:inference".into()],
        }
    }

    async fn fresh_store() -> (TempDir, ClaudeAccountsStore) {
        let tmp = TempDir::new().unwrap();
        let store = ClaudeAccountsStore::new(tmp.path().join("claude_accounts.json"));
        store.load().await.unwrap();
        (tmp, store)
    }

    #[tokio::test]
    async fn add_and_list_round_trip() {
        let (_tmp, store) = fresh_store().await;
        let view = store.add(fake_creds(8 * 3600), None).await.unwrap();
        assert_eq!(view.status, ClaudeAccountStatus::Active);
        assert_eq!(store.count().await, 1);
        let listed = store.list_views().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, view.id);
    }

    #[tokio::test]
    async fn corrupt_file_is_renamed_to_bad() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("claude_accounts.json");
        std::fs::write(&path, b"{ not valid json").unwrap();
        let store = ClaudeAccountsStore::new(path.clone());
        store.load().await.unwrap();
        assert_eq!(store.count().await, 0);
        assert!(path.with_extension("bad").exists());
    }

    #[tokio::test]
    async fn pick_next_skips_disabled_and_throttled() {
        let (_tmp, store) = fresh_store().await;
        let a = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let b = store.add(fake_creds(8 * 3600), None).await.unwrap();
        store.set_disabled(a.id, true).await.unwrap();
        store
            .mark_throttled(
                b.id,
                Utc::now() + ChronoDuration::hours(1),
                ClaudeAccountThrottleReason::FiveHour,
            )
            .await
            .unwrap();
        match store.pick_next(&[]).await {
            PickNextResult::AllThrottledUntil(_) => {}
            other => panic!("expected AllThrottledUntil, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pick_next_returns_active_account() {
        let (_tmp, store) = fresh_store().await;
        let a = store.add(fake_creds(8 * 3600), None).await.unwrap();
        match store.pick_next(&[]).await {
            PickNextResult::Account(acct) => assert_eq!(acct.id, a.id),
            other => panic!("expected Account, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_returns_not_found_for_unknown() {
        let (_tmp, store) = fresh_store().await;
        let err = store.remove(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }
}
