//! Data-model types for multi-account Claude OAuth.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// One enrolled Claude OAuth identity. Lives in `<asset_dir>/claude_accounts.json`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeAccount {
    pub id: Uuid,
    pub label: String,
    #[serde(default)]
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: ClaudeAccountStatus,
    #[serde(default)]
    pub throttled_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub throttle_reason: Option<ClaudeAccountThrottleReason>,
    #[serde(default)]
    pub five_hour_window: ClaudeAccountUsageWindow,
    #[serde(default)]
    pub weekly_window: ClaudeAccountUsageWindow,
    #[serde(default)]
    pub last_error: Option<ClaudeAccountLastError>,
    /// User-configurable rotation precedence. Lower values are picked first.
    /// On enroll, assigned to `max(existing) + 1` so new accounts go to the
    /// end of the rotation order. Re-orderable from Settings.
    #[serde(default)]
    pub precedence: i32,

    /// Raw OAuth credentials. NEVER serialized to TypeScript or returned in API responses.
    #[ts(skip)]
    pub credentials: ClaudeOAuthCredentials,
}

/// API-visible projection — credentials stripped at the route boundary.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeAccountView {
    pub id: Uuid,
    pub label: String,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: ClaudeAccountStatus,
    pub throttled_until: Option<DateTime<Utc>>,
    pub throttle_reason: Option<ClaudeAccountThrottleReason>,
    pub five_hour_window: ClaudeAccountUsageWindow,
    pub weekly_window: ClaudeAccountUsageWindow,
    pub last_error: Option<ClaudeAccountLastError>,
    pub precedence: i32,
}

impl From<&ClaudeAccount> for ClaudeAccountView {
    fn from(account: &ClaudeAccount) -> Self {
        Self {
            id: account.id,
            label: account.label.clone(),
            email: account.email.clone(),
            created_at: account.created_at,
            last_used_at: account.last_used_at,
            status: account.status,
            throttled_until: account.throttled_until,
            throttle_reason: account.throttle_reason,
            five_hour_window: account.five_hour_window.clone(),
            weekly_window: account.weekly_window.clone(),
            last_error: account.last_error.clone(),
            precedence: account.precedence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeAccountStatus {
    Active,
    Throttled,
    NeedsReauth,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeAccountThrottleReason {
    FiveHour,
    Weekly,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeAccountUsageWindow {
    #[serde(default)]
    pub used: u32,
    #[serde(default)]
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeAccountLastError {
    pub message: String,
    pub classification: FailureClass,
    pub occurred_at: DateTime<Utc>,
}

/// Classification of a Claude executor failure. Drives the rotator decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Transient network / 5xx / ambiguous 429 — retry the same account with back-off.
    Transient,
    /// 5h or weekly cap hit — rotate immediately to another healthy account.
    UsageExhausted,
    /// Permanently revoked / invalid token — mark account NeedsReauth and rotate.
    NeedsReauth,
    /// Anything else — hard-fail the task attempt.
    Fatal,
}

/// Raw OAuth credentials for a single Claude account. Persisted but never exposed via TS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// User-editable retry policy. Lives in `Config.claude_retry_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeRetryPolicy {
    /// Maximum retry attempts per task attempt. `0` disables retry entirely.
    pub max_attempts: u32,
    /// First back-off delay in seconds.
    pub initial_backoff_seconds: u32,
    /// Multiplier applied per attempt (e.g. 2.0 doubles each time).
    pub backoff_multiplier: f32,
    /// Hard cap on a single back-off in seconds.
    pub max_backoff_seconds: u32,
}

impl Default for ClaudeRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            initial_backoff_seconds: 30,
            backoff_multiplier: 2.0,
            max_backoff_seconds: 300,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RetryPolicyValidationError {
    #[error("initial_backoff_seconds must be between 1 and 600 (got {0})")]
    InitialBackoffOutOfRange(u32),
    #[error("max_backoff_seconds must be between 1 and 3600 (got {0})")]
    MaxBackoffOutOfRange(u32),
    #[error(
        "max_backoff_seconds ({max}) must be greater than or equal to initial_backoff_seconds ({initial})"
    )]
    MaxLessThanInitial { initial: u32, max: u32 },
    #[error("backoff_multiplier must be between 1.0 and 10.0 (got {0})")]
    MultiplierOutOfRange(f32),
}

impl ClaudeRetryPolicy {
    pub fn validate(&self) -> Result<(), RetryPolicyValidationError> {
        if !(1..=600).contains(&self.initial_backoff_seconds) {
            return Err(RetryPolicyValidationError::InitialBackoffOutOfRange(
                self.initial_backoff_seconds,
            ));
        }
        if !(1..=3600).contains(&self.max_backoff_seconds) {
            return Err(RetryPolicyValidationError::MaxBackoffOutOfRange(
                self.max_backoff_seconds,
            ));
        }
        if self.max_backoff_seconds < self.initial_backoff_seconds {
            return Err(RetryPolicyValidationError::MaxLessThanInitial {
                initial: self.initial_backoff_seconds,
                max: self.max_backoff_seconds,
            });
        }
        if !self.backoff_multiplier.is_finite() || !(1.0..=10.0).contains(&self.backoff_multiplier)
        {
            return Err(RetryPolicyValidationError::MultiplierOutOfRange(
                self.backoff_multiplier,
            ));
        }
        Ok(())
    }

    /// Calculate the back-off duration for a 1-indexed attempt number.
    /// Capped at `max_backoff_seconds`.
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        let attempt = attempt.max(1);
        let exponent = (attempt - 1) as i32;
        let raw =
            (self.initial_backoff_seconds as f64) * (self.backoff_multiplier as f64).powi(exponent);
        let capped = raw.min(self.max_backoff_seconds as f64);
        Duration::from_secs(capped.round().max(0.0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_default_is_30_2x_cap_300_max_6() {
        let p = ClaudeRetryPolicy::default();
        assert_eq!(p.max_attempts, 6);
        assert_eq!(p.initial_backoff_seconds, 30);
        assert!((p.backoff_multiplier - 2.0).abs() < f32::EPSILON);
        assert_eq!(p.max_backoff_seconds, 300);
    }

    #[test]
    fn backoff_schedule_matches_research_doc() {
        // research.md §5: 30, 60, 120, 240, 300, 300...
        let p = ClaudeRetryPolicy::default();
        assert_eq!(p.backoff_delay(1).as_secs(), 30);
        assert_eq!(p.backoff_delay(2).as_secs(), 60);
        assert_eq!(p.backoff_delay(3).as_secs(), 120);
        assert_eq!(p.backoff_delay(4).as_secs(), 240);
        assert_eq!(p.backoff_delay(5).as_secs(), 300);
        assert_eq!(p.backoff_delay(6).as_secs(), 300);
        assert_eq!(p.backoff_delay(50).as_secs(), 300);
    }

    #[test]
    fn validate_rejects_out_of_range() {
        assert!(matches!(
            ClaudeRetryPolicy {
                max_attempts: 0,
                initial_backoff_seconds: 0,
                backoff_multiplier: 2.0,
                max_backoff_seconds: 300,
            }
            .validate(),
            Err(RetryPolicyValidationError::InitialBackoffOutOfRange(0))
        ));
        assert!(matches!(
            ClaudeRetryPolicy {
                max_attempts: 0,
                initial_backoff_seconds: 60,
                backoff_multiplier: 2.0,
                max_backoff_seconds: 30,
            }
            .validate(),
            Err(RetryPolicyValidationError::MaxLessThanInitial { .. })
        ));
        assert!(matches!(
            ClaudeRetryPolicy {
                max_attempts: 0,
                initial_backoff_seconds: 60,
                backoff_multiplier: 0.5,
                max_backoff_seconds: 300,
            }
            .validate(),
            Err(RetryPolicyValidationError::MultiplierOutOfRange(_))
        ));
    }

    #[test]
    fn validate_accepts_max_attempts_zero() {
        // Per FR-015, 0 is allowed (disables retry).
        let p = ClaudeRetryPolicy {
            max_attempts: 0,
            initial_backoff_seconds: 30,
            backoff_multiplier: 2.0,
            max_backoff_seconds: 300,
        };
        assert_eq!(p.validate(), Ok(()));
    }
}
