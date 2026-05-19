//! Multi-account Claude OAuth management.
//!
//! Stores enrolled Claude accounts locally in `<asset_dir>/claude_accounts.json`
//! (mode 0600). Owns: PKCE OAuth flow against Anthropic, per-spawn credential
//! isolation (`CLAUDE_CONFIG_DIR`), failure classification, and rotation across
//! healthy accounts when one hits its Anthropic 5h / weekly cap.

pub mod classifier;
pub mod isolation;
pub mod oauth;
pub mod rotator;
pub mod store;
pub mod types;

pub use classifier::{classify_failure, parse_usage_reset};
pub use isolation::TempCredentialDir;
pub use oauth::{ClaudeOAuthClient, OauthAccountInfo, PendingOAuthState};
pub use rotator::{Rotator, RotatorDecision, RotatorPick};
pub use store::{ClaudeAccountsService, ClaudeAccountsStore};
pub use types::{
    ClaudeAccount, ClaudeAccountLastError, ClaudeAccountStatus, ClaudeAccountThrottleReason,
    ClaudeAccountUsageWindow, ClaudeAccountView, ClaudeOAuthCredentials, ClaudeRetryPolicy,
    FailureClass, TaskAttemptRetryState,
};
