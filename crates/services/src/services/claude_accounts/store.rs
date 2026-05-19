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

use super::{
    oauth::{ClaudeOAuthClient, ClaudeOAuthError, OauthAccountInfo},
    types::{
        ClaudeAccount, ClaudeAccountLastError, ClaudeAccountStatus, ClaudeAccountThrottleReason,
        ClaudeAccountUsageWindow, ClaudeAccountView, ClaudeOAuthCredentials, FailureClass,
    },
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
            Ok(mut loaded) => {
                // Backfill precedence for accounts persisted before the field
                // existed (all defaulted to 0 by serde). If we see more than
                // one zero, assign each a stable sequential precedence based
                // on its created_at order so they actually sort.
                if loaded.iter().filter(|a| a.precedence == 0).count() > 1 {
                    let mut indices: Vec<usize> = (0..loaded.len()).collect();
                    indices.sort_by_key(|i| loaded[*i].created_at);
                    for (new_p, idx) in indices.into_iter().enumerate() {
                        loaded[idx].precedence = new_p as i32;
                    }
                }
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
        // Clear expired throttles so the UI reflects the same state the
        // rotator would see on the next spawn (matters after restart).
        let mut guard = self.accounts.write().await;
        self.unthrottle_expired_locked(&mut guard).await;
        let mut views: Vec<ClaudeAccountView> = guard.iter().map(ClaudeAccountView::from).collect();
        // Surface in precedence order so the UI table matches the rotation order.
        views.sort_by_key(|v| v.precedence);
        views
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
        let next_precedence = guard.iter().map(|a| a.precedence).max().unwrap_or(-1) + 1;
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
            precedence: next_precedence,
            credentials: creds,
        };
        let view = ClaudeAccountView::from(&account);
        guard.push(account);
        self.save_locked(&guard).await?;
        Ok(view)
    }

    /// Replace the rotation precedence of every account in one shot. The
    /// caller supplies the desired full ordering — the i-th id in `order`
    /// gets precedence `i`. Ids not present in `order` are pushed to the
    /// end with their existing relative order preserved (defensive: a
    /// stale client that misses a newly-enrolled account doesn't lose it).
    pub async fn reorder(&self, order: &[Uuid]) -> Result<Vec<ClaudeAccountView>, StoreError> {
        let mut guard = self.accounts.write().await;

        // Build the new precedence map. First pass: positions from the
        // supplied order. Second pass: any account not in `order` gets the
        // next available slot, preserving its current relative ordering.
        let mut new_precedence: std::collections::HashMap<Uuid, i32> =
            std::collections::HashMap::new();
        let mut next: i32 = 0;
        for id in order {
            if guard.iter().any(|a| a.id == *id) && !new_precedence.contains_key(id) {
                new_precedence.insert(*id, next);
                next += 1;
            }
        }
        // Anything not assigned yet, sorted by current precedence asc.
        let mut leftovers: Vec<&ClaudeAccount> = guard
            .iter()
            .filter(|a| !new_precedence.contains_key(&a.id))
            .collect();
        leftovers.sort_by_key(|a| a.precedence);
        for acct in leftovers {
            new_precedence.insert(acct.id, next);
            next += 1;
        }

        for acct in guard.iter_mut() {
            if let Some(p) = new_precedence.get(&acct.id) {
                acct.precedence = *p;
            }
        }
        // Sort the in-memory Vec so the file is human-readable in precedence order.
        guard.sort_by_key(|a| a.precedence);
        let views: Vec<ClaudeAccountView> = guard.iter().map(ClaudeAccountView::from).collect();
        self.save_locked(&guard).await?;
        Ok(views)
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
    /// at the start of each `pick_next()` and `list_views()` so user-visible
    /// status (and rotator decisions) stay accurate even across restarts.
    async fn unthrottle_expired_locked(&self, accounts: &mut [ClaudeAccount]) {
        let now = Utc::now();
        for acct in accounts.iter_mut() {
            if acct.status == ClaudeAccountStatus::Throttled
                && acct.throttled_until.map(|t| t <= now).unwrap_or(false)
            {
                // Capture the prior reason BEFORE clearing it — otherwise the
                // weekly-window reset branch can never fire (it was reading
                // the field we just set to None).
                let prior_reason = acct.throttle_reason;
                acct.status = ClaudeAccountStatus::Active;
                acct.throttled_until = None;
                acct.throttle_reason = None;
                acct.five_hour_window = ClaudeAccountUsageWindow::default();
                if prior_reason == Some(ClaudeAccountThrottleReason::Weekly) {
                    acct.weekly_window = ClaudeAccountUsageWindow::default();
                }
            }
        }
    }

    /// Round-robin select the next non-throttled, non-disabled, non-needs-reauth
    /// account, skipping any in `exclude`. Returns the soonest `throttled_until`
    /// if no healthy account remains.
    pub async fn pick_next(&self, exclude: &[Uuid]) -> PickNextResult {
        let mut guard = self.accounts.write().await;
        self.unthrottle_expired_locked(&mut guard).await;
        if guard.is_empty() {
            return PickNextResult::NoAccountsEnrolled;
        }

        // Healthy accounts, ordered by user-configured precedence (lower =
        // higher priority). Tie-break by least-recently-used so accounts at
        // the same precedence level still spread load evenly.
        let mut healthy: Vec<&ClaudeAccount> = guard
            .iter()
            .filter(|a| a.status == ClaudeAccountStatus::Active && !exclude.contains(&a.id))
            .collect();
        healthy.sort_by(|a, b| {
            a.precedence.cmp(&b.precedence).then_with(|| {
                a.last_used_at
                    .unwrap_or(a.created_at)
                    .cmp(&b.last_used_at.unwrap_or(b.created_at))
            })
        });
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
        // Always update the in-memory counters + last_used_at so the
        // rotator's pick_next sees fresh state even on rapid-fire successes.
        // Only the disk write is debounced (at 1/s/account), preventing
        // chatty fsyncs during a burst.
        let now_ms = Utc::now().timestamp_millis();
        let entry = debounce_map.entry(id).or_insert_with(|| AtomicU64::new(0));
        let last = entry.load(Ordering::Relaxed) as i64;
        let should_persist = now_ms - last >= 1000;
        if should_persist {
            entry.store(now_ms as u64, Ordering::Relaxed);
        }
        drop(entry);

        let mut guard = self.accounts.write().await;
        let acct = guard
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(StoreError::NotFound(id))?;
        acct.last_used_at = Some(Utc::now());
        acct.five_hour_window.used = acct.five_hour_window.used.saturating_add(1);
        acct.weekly_window.used = acct.weekly_window.used.saturating_add(1);
        if should_persist {
            self.save_locked(&guard).await?;
        }
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
    use chrono::Duration as ChronoDuration;
    use tempfile::TempDir;

    use super::*;

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

    #[tokio::test]
    async fn add_assigns_increasing_precedence() {
        let (_tmp, store) = fresh_store().await;
        let a = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let b = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let c = store.add(fake_creds(8 * 3600), None).await.unwrap();
        assert_eq!(a.precedence, 0);
        assert_eq!(b.precedence, 1);
        assert_eq!(c.precedence, 2);
    }

    #[tokio::test]
    async fn pick_next_honors_precedence_order() {
        let (_tmp, store) = fresh_store().await;
        let a = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let b = store.add(fake_creds(8 * 3600), None).await.unwrap();
        // a has precedence 0, b has precedence 1; pick should return a.
        match store.pick_next(&[]).await {
            PickNextResult::Account(acct) => assert_eq!(acct.id, a.id),
            other => panic!("expected Account, got {other:?}"),
        }
        // Reorder so b is first; now pick should return b.
        store.reorder(&[b.id, a.id]).await.unwrap();
        match store.pick_next(&[]).await {
            PickNextResult::Account(acct) => assert_eq!(acct.id, b.id),
            other => panic!("expected Account, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reorder_preserves_unknown_accounts_at_end() {
        let (_tmp, store) = fresh_store().await;
        let a = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let b = store.add(fake_creds(8 * 3600), None).await.unwrap();
        let c = store.add(fake_creds(8 * 3600), None).await.unwrap();
        // Client only knows about a + c (stale; missed b). Reorder a, c.
        // b should keep its slot at the end, not vanish.
        let views = store.reorder(&[c.id, a.id]).await.unwrap();
        assert_eq!(views.len(), 3);
        assert_eq!(views[0].id, c.id);
        assert_eq!(views[1].id, a.id);
        assert_eq!(views[2].id, b.id);
    }
}
