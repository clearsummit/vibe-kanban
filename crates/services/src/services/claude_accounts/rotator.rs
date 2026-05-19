//! Rotator: decides what to do with a Claude executor failure.
//!
//! Policy (locked in spec FR-013..FR-021):
//! - `Transient`        → retry SAME account after exponential back-off
//! - `UsageExhausted`   → mark account throttled, rotate immediately to next healthy
//! - `NeedsReauth`      → mark account NeedsReauth, rotate, do NOT cost retry budget
//! - `Fatal`            → bubble up

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::classifier::{fallback_reset, parse_usage_reset};
use super::store::{ClaudeAccountsService, PickNextResult, StoreError};
use super::types::{
    ClaudeAccount, ClaudeAccountThrottleReason, ClaudeRetryPolicy, FailureClass,
};

#[derive(Debug, Clone)]
pub enum RotatorPick {
    /// An account is ready to spawn against.
    Account(ClaudeAccount),
    /// All enrolled accounts are throttled — sleep until the soonest reset, then retry.
    SleepUntil(DateTime<Utc>),
    /// No accounts are usable (all Disabled / NeedsReauth, or no accounts enrolled).
    NoAccountsAvailable,
}

#[derive(Debug, Clone)]
pub enum RotatorDecision {
    /// Retry the same account after this back-off delay. Counts against retry budget.
    RetrySame {
        backoff: Duration,
    },
    /// Switch to the next healthy account. Counts against retry budget.
    RotateNext,
    /// Mark the account NeedsReauth and rotate. Does NOT count against retry budget.
    MarkReauthAndRotate,
    /// Hard-fail the task attempt.
    Fatal,
}

pub struct Rotator {
    service: Arc<ClaudeAccountsService>,
    policy: ClaudeRetryPolicy,
}

impl Rotator {
    pub fn new(service: Arc<ClaudeAccountsService>, policy: ClaudeRetryPolicy) -> Self {
        Self { service, policy }
    }

    pub fn policy(&self) -> ClaudeRetryPolicy {
        self.policy
    }

    /// Pick the next account to spawn against, excluding any already tried for
    /// this task attempt. Returns SleepUntil when all enrolled accounts are
    /// currently throttled.
    pub async fn pick_next(&self, attempts_tried: &[Uuid]) -> RotatorPick {
        match self.service.store.pick_next(attempts_tried).await {
            PickNextResult::Account(a) => RotatorPick::Account(a),
            PickNextResult::AllThrottledUntil(t) => RotatorPick::SleepUntil(t),
            PickNextResult::NoAccountsEnrolled | PickNextResult::NoUsableAccounts => {
                RotatorPick::NoAccountsAvailable
            }
        }
    }

    /// Decide what to do with a failure on `account_id`. Mutates the store
    /// (marking throttled / needs-reauth) as a side effect.
    pub async fn on_failure(
        &self,
        account_id: Uuid,
        class: FailureClass,
        attempt_number: u32,
        combined_output: &str,
    ) -> Result<RotatorDecision, StoreError> {
        // Always record the last error for visibility.
        let _ = self
            .service
            .store
            .record_failure(account_id, class, truncate_for_log(combined_output))
            .await;

        match class {
            FailureClass::Transient => {
                if self.policy.max_attempts == 0 {
                    return Ok(RotatorDecision::Fatal);
                }
                if attempt_number >= self.policy.max_attempts {
                    return Ok(RotatorDecision::Fatal);
                }
                let backoff = self.policy.backoff_delay(attempt_number);
                Ok(RotatorDecision::RetrySame { backoff })
            }
            FailureClass::UsageExhausted => {
                let now = Utc::now();
                let (reset_at, reason) = parse_usage_reset(combined_output, now)
                    .unwrap_or_else(|| {
                        let reason = ClaudeAccountThrottleReason::FiveHour;
                        (fallback_reset(reason, now), reason)
                    });
                self.service
                    .store
                    .mark_throttled(account_id, reset_at, reason)
                    .await?;
                if self.policy.max_attempts == 0 {
                    return Ok(RotatorDecision::Fatal);
                }
                if attempt_number >= self.policy.max_attempts {
                    return Ok(RotatorDecision::Fatal);
                }
                Ok(RotatorDecision::RotateNext)
            }
            FailureClass::NeedsReauth => {
                self.service
                    .store
                    .mark_needs_reauth(account_id, truncate_for_log(combined_output))
                    .await?;
                Ok(RotatorDecision::MarkReauthAndRotate)
            }
            FailureClass::Fatal => Ok(RotatorDecision::Fatal),
        }
    }

    /// Convenience for the executor: cap a "sleep until <when>" duration at
    /// `max_backoff_seconds` so we don't sleep for the full 5h cap when the
    /// user might cancel.
    pub fn cap_sleep(&self, until: DateTime<Utc>) -> Duration {
        let now = Utc::now();
        let raw = (until - now).num_seconds().max(1) as u64;
        Duration::from_secs(raw.min(self.policy.max_backoff_seconds as u64))
    }
}

fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 1024;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::claude_accounts::oauth::ClaudeOAuthClient;
    use crate::services::claude_accounts::store::ClaudeAccountsStore;
    use crate::services::claude_accounts::types::ClaudeOAuthCredentials;
    use chrono::Duration as ChronoDuration;
    use tempfile::TempDir;

    async fn fresh_rotator(policy: ClaudeRetryPolicy) -> (TempDir, Arc<ClaudeAccountsService>, Rotator) {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(ClaudeAccountsStore::new(
            tmp.path().join("claude_accounts.json"),
        ));
        store.load().await.unwrap();
        let oauth = Arc::new(ClaudeOAuthClient::new());
        let service = Arc::new(ClaudeAccountsService::new(store, oauth));
        let rotator = Rotator::new(service.clone(), policy);
        (tmp, service, rotator)
    }

    fn fake_creds() -> ClaudeOAuthCredentials {
        ClaudeOAuthCredentials {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + ChronoDuration::hours(8),
            scopes: vec!["user:inference".into()],
        }
    }

    #[tokio::test]
    async fn transient_under_budget_retries_same_with_backoff() {
        let (_tmp, svc, rotator) = fresh_rotator(ClaudeRetryPolicy::default()).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        let decision = rotator
            .on_failure(a.id, FailureClass::Transient, 1, "API Error: 503")
            .await
            .unwrap();
        match decision {
            RotatorDecision::RetrySame { backoff } => {
                assert_eq!(backoff.as_secs(), 30);
            }
            other => panic!("expected RetrySame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transient_at_budget_is_fatal() {
        let policy = ClaudeRetryPolicy {
            max_attempts: 2,
            ..Default::default()
        };
        let (_tmp, svc, rotator) = fresh_rotator(policy).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        let decision = rotator
            .on_failure(a.id, FailureClass::Transient, 2, "API Error: 503")
            .await
            .unwrap();
        assert!(matches!(decision, RotatorDecision::Fatal));
    }

    #[tokio::test]
    async fn max_attempts_zero_disables_retry() {
        let policy = ClaudeRetryPolicy {
            max_attempts: 0,
            ..Default::default()
        };
        let (_tmp, svc, rotator) = fresh_rotator(policy).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        let decision = rotator
            .on_failure(a.id, FailureClass::Transient, 1, "API Error: 503")
            .await
            .unwrap();
        assert!(matches!(decision, RotatorDecision::Fatal));
    }

    #[tokio::test]
    async fn usage_exhausted_marks_throttled_and_rotates() {
        let (_tmp, svc, rotator) = fresh_rotator(ClaudeRetryPolicy::default()).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        let now_ts = Utc::now().timestamp();
        let stderr = format!("Claude AI usage limit reached|{}", now_ts + 3600);
        let decision = rotator
            .on_failure(a.id, FailureClass::UsageExhausted, 1, &stderr)
            .await
            .unwrap();
        assert!(matches!(decision, RotatorDecision::RotateNext));
        let listed = svc.store.list_views().await;
        let row = listed.iter().find(|r| r.id == a.id).unwrap();
        assert_eq!(row.status, super::super::types::ClaudeAccountStatus::Throttled);
        assert_eq!(row.throttle_reason, Some(ClaudeAccountThrottleReason::FiveHour));
    }

    #[tokio::test]
    async fn needs_reauth_does_not_cost_budget() {
        let policy = ClaudeRetryPolicy {
            max_attempts: 1,
            ..Default::default()
        };
        let (_tmp, svc, rotator) = fresh_rotator(policy).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        // attempt_number well above max_attempts — NeedsReauth must still rotate, not fatal.
        let decision = rotator
            .on_failure(a.id, FailureClass::NeedsReauth, 50, "invalid_grant")
            .await
            .unwrap();
        assert!(matches!(decision, RotatorDecision::MarkReauthAndRotate));
    }

    #[tokio::test]
    async fn fatal_returns_fatal() {
        let (_tmp, svc, rotator) = fresh_rotator(ClaudeRetryPolicy::default()).await;
        let a = svc.store.add(fake_creds(), None).await.unwrap();
        let decision = rotator
            .on_failure(a.id, FailureClass::Fatal, 1, "kernel panic")
            .await
            .unwrap();
        assert!(matches!(decision, RotatorDecision::Fatal));
    }
}
