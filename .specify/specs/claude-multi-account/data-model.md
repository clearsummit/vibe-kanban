# Phase 1 — Data Model: Multi-Account Claude OAuth + Retry

**Spec**: [./spec.md](./spec.md)
**Plan**: [./plan.md](./plan.md)
**Date**: 2026-05-19

## Storage map

| Entity | Lives in | Mode | Synced to remote? |
|---|---|---|---|
| `ClaudeAccount` (Vec) | `<asset_dir>/claude_accounts.json` | 0600 | No |
| `ClaudeRetryPolicy` | `<asset_dir>/config.json` (extends existing config) | 0600 | No |
| `TaskAttemptRetryState` | `<asset_dir>/claude_retry_state/<task_attempt_id>.json` | 0600 | No |
| Per-spawn credential dir | `<asset_dir>/claude_spawn_tmp/<uuid>/` | 0700 | No |

`<asset_dir>` is the directory returned by `utils::assets::asset_dir()` (e.g. `~/Library/Application Support/ai.bloop.vibe-kanban` on macOS, `~/.local/share/vibe-kanban` on Linux).

No SQLite tables. No remote sync.

---

## Entity: `ClaudeAccount`

Rust declaration (illustrative — exact location: `crates/services/src/services/claude_accounts/store.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClaudeAccount {
    pub id: Uuid,
    pub label: String,                              // user-editable; defaults to email or "Account N"
    pub email: Option<String>,                      // from OAuth userinfo or oauthAccount block, best-effort
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: ClaudeAccountStatus,
    pub throttled_until: Option<DateTime<Utc>>,
    pub throttle_reason: Option<ClaudeAccountThrottleReason>,
    pub five_hour_window: ClaudeAccountUsageWindow,
    pub weekly_window: ClaudeAccountUsageWindow,
    pub last_error: Option<ClaudeAccountLastError>,

    // Credentials — NOT exposed to API responses.
    #[ts(skip)]
    #[serde(rename = "credentials")]
    pub credentials: ClaudeOAuthCredentials,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeAccountStatus {
    Active,
    Throttled,
    NeedsReauth,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeAccountThrottleReason {
    FiveHour,
    Weekly,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct ClaudeAccountUsageWindow {
    pub used: u32,                                  // VK-owned counter
    pub reset_at: Option<DateTime<Utc>>,            // from CLI's <unix_ts> tail when seen
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClaudeAccountLastError {
    pub message: String,
    pub classification: FailureClass,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transient,
    UsageExhausted,
    NeedsReauth,
    Fatal,
}

// NOT a TS-exported type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub scopes: Vec<String>,
}
```

### API-visible projection

When serializing to the HTTP API, `credentials` MUST be stripped. The route handler converts `ClaudeAccount` to a `ClaudeAccountView`:

```rust
#[derive(Debug, Clone, Serialize, TS)]
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
    // credentials intentionally absent
}
```

### Operations (`ClaudeAccountsStore` service)

| Method | Purpose | Persists? |
|---|---|---|
| `list() -> Vec<ClaudeAccountView>` | Read for the API/UI | n/a |
| `add(creds, email) -> ClaudeAccount` | Append a new account, write file | yes |
| `replace_credentials(id, creds)` | Re-auth in place | yes |
| `update_label(id, label)` | Rename | yes |
| `set_disabled(id, bool)` | Disable/enable | yes |
| `remove(id)` | Delete and rewrite file | yes |
| `mark_throttled(id, until, reason)` | Called by rotator on UsageExhausted | yes |
| `mark_needs_reauth(id, message)` | Called by rotator on NeedsReauth | yes |
| `record_success(id)` | Increment counters, bump last_used_at | yes (throttled to ≤ 1 write/sec/account) |
| `record_failure(id, class, message)` | Update last_error | yes |
| `pick_next() -> Option<ClaudeAccount>` | Round-robin across non-Throttled+non-Disabled+non-NeedsReauth | no |

All writes use the same tmp-file + rename pattern as `oauth_credentials.rs`.

### Single-process locking

Each account-id gets a `tokio::sync::Mutex<()>` held by the rotator while a spawn is using that account, to serialize token refresh attempts. The Mutex map is wrapped in `Arc<DashMap<Uuid, Arc<Mutex<()>>>>` inside the service struct.

### Concurrent file-write safety

The store holds a single `tokio::sync::RwLock<Vec<ClaudeAccount>>` in memory. All mutations: acquire write-lock → mutate → `save_to_disk()` → release lock. Reads use the read-lock. The on-disk file is always rewritten in full (small Vec, ≤ 10 accounts).

---

## Entity: `ClaudeRetryPolicy`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClaudeRetryPolicy {
    /// Maximum retry attempts per task attempt. 0 disables retry.
    pub max_attempts: u32,
    /// First back-off delay in seconds.
    pub initial_backoff_seconds: u32,
    /// Exponent applied per attempt.
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
```

Lives in the existing config:

```rust
// crates/services/src/services/config/mod.rs (illustrative)
pub struct Config {
    // ... existing fields ...
    #[serde(default)]
    pub claude_retry_policy: ClaudeRetryPolicy,
}
```

### Validation

- `max_attempts` ≥ 0 (no upper bound enforced; UI hint at 50).
- `initial_backoff_seconds` ∈ [1, 600].
- `backoff_multiplier` ∈ [1.0, 10.0].
- `max_backoff_seconds` ∈ [`initial_backoff_seconds`, 3600].

The PATCH route returns `400` with a structured error for out-of-range values.

---

## Entity: `TaskAttemptRetryState`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TaskAttemptRetryState {
    pub task_attempt_id: Uuid,
    pub attempt_number: u32,                        // 1-indexed; starts at 1
    pub last_failure_class: Option<FailureClass>,
    pub last_backoff_seconds: u32,
    pub accounts_tried: Vec<Uuid>,
    pub updated_at: DateTime<Utc>,
}
```

### Persistence

- One file per task attempt: `<asset_dir>/claude_retry_state/<task_attempt_id>.json` (0600).
- Written BEFORE each `tokio::time::sleep` so a crash mid-back-off doesn't lose progress.
- Deleted when the task attempt either succeeds or hits Fatal.
- Stale files (mtime > 30 days) cleaned on startup.

### Read on resume

When the executor resumes a task attempt:

1. Check for `<asset_dir>/claude_retry_state/<task_attempt_id>.json`.
2. If present, load `attempt_number` and `accounts_tried`. The rotator skips already-tried accounts on this attempt's resume.
3. If absent, start fresh.

---

## Auxiliary types (TS-exported request/response shapes)

```rust
#[derive(Debug, Deserialize, TS)]
pub struct ClaudeOAuthStartRequest {
    /// If present, this is a re-auth for an existing account.
    pub account_id: Option<Uuid>,
}

#[derive(Debug, Serialize, TS)]
pub struct ClaudeOAuthStartResponse {
    pub auth_url: String,
    /// Opaque token that ties oauth/complete back to the corresponding /oauth/start call.
    pub state: String,
}

#[derive(Debug, Deserialize, TS)]
pub struct ClaudeOAuthCompleteRequest {
    pub state: String,
    pub code: String,
}

#[derive(Debug, Deserialize, TS)]
pub struct UpdateClaudeAccount {
    pub label: Option<String>,
    pub disabled: Option<bool>,
}
```

### OAuth in-flight state (server memory only)

```rust
// Not persisted; lives in a per-process Mutex<HashMap<String, PendingOAuth>>.
struct PendingOAuth {
    code_verifier: String,
    code_challenge: String,
    account_id: Option<Uuid>,                       // Some(_) iff re-auth
    created_at: Instant,
}
```

Entries TTL after 10 minutes. The HashMap is bounded — a soft cap of 8 simultaneous in-flight flows; older entries get evicted FIFO.

---

## Per-spawn credential isolation directory

`<asset_dir>/claude_spawn_tmp/<task-attempt-id>-<attempt-uuid>/` contains:

- `.credentials.json` — written by us with the selected account's `claudeAiOauth` block.
- `.claude.json` — minimal `{ "hasCompletedOnboarding": true, "oauthAccount": { "uuid": "...", "emailAddress": "..." } }`.

The dir is created mode `0700` on Unix. Deleted at end of attempt (success or fail). Orphan sweep at startup deletes any directory with `mtime` > 1 hour.

---

## Type-export registration

The following `Type::decl()` calls must be added to `crates/server/src/bin/generate_types.rs`:

```rust
services::services::claude_accounts::ClaudeAccountView::decl(),
services::services::claude_accounts::ClaudeAccountStatus::decl(),
services::services::claude_accounts::ClaudeAccountThrottleReason::decl(),
services::services::claude_accounts::ClaudeAccountUsageWindow::decl(),
services::services::claude_accounts::ClaudeAccountLastError::decl(),
services::services::claude_accounts::FailureClass::decl(),
services::services::claude_accounts::ClaudeRetryPolicy::decl(),
services::services::claude_accounts::TaskAttemptRetryState::decl(),
server::routes::claude_accounts::ClaudeOAuthStartRequest::decl(),
server::routes::claude_accounts::ClaudeOAuthStartResponse::decl(),
server::routes::claude_accounts::ClaudeOAuthCompleteRequest::decl(),
server::routes::claude_accounts::UpdateClaudeAccount::decl(),
```

After registration: `pnpm run generate-types` regenerates `shared/types.ts`.

---

## Privacy / security invariants enforced by the data model

1. `ClaudeOAuthCredentials` has `#[ts(skip)]` so it cannot appear in TypeScript types.
2. The HTTP `ClaudeAccountView` projection strips credentials at the route boundary.
3. The store file is mode `0600`; the tmp credential dir is mode `0700`.
4. Logging: `tracing::debug!` and lower may include account `id` (Uuid) and `label`. NEVER access token, refresh token, or code verifier. A `redact::Redact` wrapper around credential strings enforces this at compile time.
