# Feature Specification: Multi-Account Claude OAuth with Automatic Rotation and Retry

**Feature Branch**: `vk/d3a0-please-update-vi` (working branch)
**Created**: 2026-05-19
**Status**: Draft
**Input**: User description: "Update Vibe Kanban to allow OAuth through the interface to add multiple OAuth tokens for Claude and to function like claude-rotate. We should be able to see our different accounts and their usage in settings. Also add a retry mechanism with back-off — right now when Claude gives us a 400 or similar, progress stops; it should keep retrying with back-off instead."

## Background

Today Vibe Kanban invokes the Claude Code CLI (`@anthropic-ai/claude-code`) once per task attempt. The CLI reads a single set of Claude OAuth credentials from `~/.claude/.credentials.json`. When Anthropic rate-limits or rejects the request (HTTP 400/401/403/429/5xx, "quota exceeded", "usage limit reached"), the executor process exits, the attempt is marked failed, and the user must intervene manually. This blocks long-running multi-step agent tasks — especially during high-traffic hours and for users on Pro/Team plans with usage caps.

This feature lets a user enroll multiple Claude accounts via OAuth from inside the Vibe Kanban UI, view per-account status and usage from Settings, and have the executor automatically rotate to a healthy account and retry with exponential back-off when the current account is throttled or returns a transient failure.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Enroll a Claude account via OAuth (Priority: P1)

As a Vibe Kanban user, I want to add one or more Claude accounts to Vibe Kanban through the Settings UI by signing in with OAuth, so that tasks can run against my Claude subscriptions without me hand-editing credential files.

**Why this priority**: Multi-account rotation is impossible without a way to register accounts. This is the foundation; every other story depends on it. Even by itself, a single-account "log in from the UI" experience is a meaningful improvement over the current "run the CLI elsewhere and copy credentials" workflow.

**Independent Test**: From a fresh install with no Claude credentials, the user opens Settings → Claude Accounts, clicks "Add account", completes the OAuth flow, and sees the new account listed as Active with a label and a created-at timestamp. A subsequent task run uses that account.

**Acceptance Scenarios**:

1. **Given** no Claude accounts are enrolled, **When** the user opens Settings → Claude Accounts and clicks "Add account", **Then** Vibe Kanban displays an authorization URL and a code-input field, opens the URL in the user's browser, and accepts the returned code.
2. **Given** the user pastes a valid authorization code, **When** Vibe Kanban exchanges it for tokens, **Then** the account appears in the accounts list with a default label, a created-at timestamp, and status "Active".
3. **Given** the user pastes an invalid or expired authorization code, **When** the exchange fails, **Then** Vibe Kanban shows an actionable error message, the accounts list is unchanged, and the user can retry without restarting the flow.
4. **Given** an account is already enrolled, **When** the user adds a second account, **Then** both accounts appear in the list and the user can rename either of them inline.

---

### User Story 2 — Retry-with-back-off and rotation on usage exhaustion (Priority: P1)

As a Vibe Kanban user running long agent tasks, I want the task attempt to keep going when Claude returns a transient error (e.g., a 400/5xx/network blip) by retrying the same account with exponential back-off, AND to automatically rotate to my next enrolled account specifically when the current account is out of its Anthropic usage quota (5-hour or weekly limit), so that I don't have to babysit the run.

**Why this priority**: This is the user's stated pain ("right now when Claude gives us a 400, the progress stops"). It is the headline value of the feature — multi-account enrollment without retry/rotation would be a worse UX than today.

**Rotation policy (decided)**:
- **Transient errors** (400/5xx/network/transport): retry the SAME account with exponential back-off. Do NOT rotate.
- **Usage-exhausted errors** (Anthropic 5-hour cap reached, Anthropic weekly cap reached, "usage limit"/"quota" errors): mark the current account "Throttled until <reset_time>", rotate to the next healthy account, do NOT sleep if another healthy account exists.
- **Permanent auth errors** (revoked/invalid token): mark the account "Needs re-auth", rotate to the next healthy account, do NOT count against retry budget.

**Retry counter persistence**: The current retry count for an in-flight task attempt persists across application restart so a crash mid-back-off does not reset the budget.

**Independent Test**: Enroll two accounts. Induce a `usage_limit` response from the first account (e.g., by mocking the Claude CLI to exit with a usage-exhausted-shaped stderr). Confirm that the executor immediately switches to the second account without sleeping and the task completes. Separately, induce a transient 5xx from a single-account install and confirm the SAME account retries with exponential back-off until the request succeeds.

**Acceptance Scenarios**:

1. **Given** an active task attempt and the active account returns a transient error (400, 5xx, network), **When** the executor detects the failure, **Then** Vibe Kanban sleeps for the next exponential back-off interval and retries against the SAME account, without marking the account throttled and without rotating.
2. **Given** two or more enrolled accounts and the active account returns a usage-exhausted error, **When** the executor detects the failure, **Then** Vibe Kanban marks the active account "Throttled" with the reset-time from the error (or a sensible fallback) and immediately rotates to the next healthy account WITHOUT a back-off sleep.
3. **Given** only one healthy account remains and it returns a usage-exhausted error, **When** the executor needs to spawn a Claude process, **Then** Vibe Kanban waits until the soonest reset-time, then retries.
4. **Given** the active account returns a permanent auth failure (revoked/invalid token), **When** the executor detects it, **Then** the account is marked "Needs re-auth", the task attempt rotates to the next healthy account, the user is shown a re-auth prompt in Settings, and the failure does NOT count against the retry budget.
5. **Given** the user-configured maximum retry attempts is exceeded for a single task attempt, **When** all retries are exhausted, **Then** the task attempt is marked failed with a normalized log entry that names the last error class and every account that was tried.
6. **Given** a back-off sleep is in progress for the active account, **When** the user cancels the task attempt, **Then** the back-off sleep is interrupted and the attempt stops within 2 seconds.
7. **Given** the application is restarted in the middle of a retry sequence, **When** Vibe Kanban resumes the task attempt, **Then** the retry counter resumes from its persisted value (does not reset to 0) and any unexpired throttled-until timestamps are honored.

---

### User Story 3 — View accounts with 5-hour and weekly usage in Settings (Priority: P2)

As a Vibe Kanban user, I want to see each enrolled Claude account along with its current Anthropic 5-hour usage, the 5-hour reset time, its weekly usage, and the weekly reset time, so that I can predict which account the rotator will pick next, decide whether to add another account, or disable an over-used one.

**Why this priority**: Strong UX win but the rotation/retry engine works without observability. Treat as the "make it understandable" layer on top of the engine.

**Independent Test**: Enroll two accounts, run a small task that consumes some quota on Account A. Open Settings → Claude Accounts. Account A row shows non-zero 5-hour usage with a future reset time, weekly usage with a weekly reset time, and Active status. Account B row shows zero 5-hour usage and Active status. Throttle Account A (e.g., induce a usage_limit error); confirm the row flips to "Throttled until <5h reset_time>" with a live countdown.

**Acceptance Scenarios**:

1. **Given** at least one enrolled account, **When** the user opens Settings → Claude Accounts, **Then** each account row shows: account name, status (Active / Throttled / Needs re-auth / Disabled), 5-hour usage (used or used/limit), 5-hour reset time, weekly usage (used or used/limit), weekly reset time.
2. **Given** an account is currently throttled because the 5-hour window is exhausted, **When** the user views the accounts list, **Then** that row shows "Throttled until <reset_time>" with a live countdown until the 5-hour reset.
3. **Given** an account is currently throttled because the weekly window is exhausted, **When** the user views the accounts list, **Then** that row shows "Throttled until <weekly_reset_time>" with a live countdown until the weekly reset.
4. **Given** the user clicks "Disable" on an Active account, **When** the action confirms, **Then** the account is moved to "Disabled" status and is skipped by the rotator until the user re-enables it.
5. **Given** the user clicks "Remove" on an account, **When** the action is confirmed via a destructive-action dialog, **Then** the account and its locally stored credentials are deleted and the row disappears.
6. **Given** the user clicks "Re-auth" on a "Needs re-auth" account, **When** the OAuth flow completes, **Then** the existing account's credentials are replaced (id, label, and usage history are preserved) and the status returns to "Active".
7. **Given** the user opens the retry settings panel, **When** they change the "Maximum retry attempts per task attempt" value, **Then** the new value is persisted and applies to all subsequent task attempts immediately.

---

### Edge Cases

- **Single-account regression**: A user with exactly one enrolled account must see no behavior change other than the new Settings UI and the retry-with-back-off. Usage-exhausted on the only account waits for the reset time (no rotation possible); transient errors retry the same account.
- **No accounts enrolled at all**: The Claude executor falls back to whatever credentials exist in the user's actual `~/.claude/.credentials.json` (today's behavior), so existing installs keep working without forced migration.
- **Token refresh during a long run**: If an account's access token expires mid-task, the system refreshes it transparently using the refresh token and does NOT count this as a retry.
- **Credentials file corruption**: If the accounts file is unreadable or malformed, the system renames it to a backup name, starts with an empty list, and surfaces a one-time warning in Settings.
- **Concurrent task attempts**: Two task attempts running in parallel must not collide over the active-account selection. Each attempt must get an isolated credential context.
- **Pattern-detection false positives**: A user-authored prompt that contains the phrase "rate limit" or "quota" must not trigger account rotation. Usage-exhausted detection MUST be scoped to executor exit signals (e.g., stderr lines emitted by the Claude CLI itself), not stdout/assistant content.
- **Distinguishing transient from usage-exhausted**: A naked HTTP 429 with no explicit usage-window context defaults to transient (retry same account); only signals that clearly indicate the 5h or weekly cap (the CLI's `usage_limit`-shaped error or an explicit `quota exceeded`) trigger rotation. Misclassification toward transient is preferred (fewer false rotations).
- **Clock skew**: Throttled-until reset timestamps are stored as absolute UTC. The countdown UI honors these even across process restarts.
- **Process killed during back-off**: If Vibe Kanban is restarted mid-back-off, account statuses, throttled-until timestamps, AND the retry counter for the in-flight task attempt MUST persist; on resume the rotator honors any unexpired throttled-until and the retry budget continues from where it left off.
- **Unknown usage window**: If the Claude CLI does not surface a structured reset time, the system uses a conservative default (e.g., 1 hour for the 5h window, end-of-current-week UTC for the weekly window) and updates it on the next successful response that surfaces real usage data.
- **Max-retries set to 0**: Allowed — disables retry entirely. The first failure surfaces as a hard task failure (matches today's behavior).

## Requirements *(mandatory)*

### Functional Requirements

#### Account enrollment

- **FR-001**: System MUST allow a user to enroll a Claude account by completing an OAuth authorization flow initiated from the Settings UI.
- **FR-002**: System MUST allow a user to enroll multiple Claude accounts; the count of enrolled accounts is unbounded.
- **FR-003**: System MUST allow a user to provide an optional human-readable label for each account, defaulting to a derived label (e.g., the account email if discoverable, otherwise "Account N").
- **FR-004**: System MUST allow a user to remove an enrolled account, which deletes the locally stored credentials for that account.
- **FR-005**: System MUST allow a user to re-authorize an existing account in place, preserving its id, label, and usage history.
- **FR-006**: System MUST allow a user to disable an enrolled account so the rotator skips it without removing its credentials.

#### Credential storage

- **FR-007**: System MUST persist enrolled-account credentials and metadata locally on the user's machine, never transmit them to any server other than Anthropic's OAuth and Claude endpoints, and never commit them to any database that syncs to a remote.
- **FR-008**: Credential files MUST be created with owner-read/write-only permissions on Unix (`0600`).
- **FR-009**: System MUST tolerate a missing or unreadable credentials store by recovering to an empty list and surfacing a single, dismissible warning.

#### Account selection (rotation)

- **FR-010**: When spawning a Claude executor, the system MUST select exactly one enrolled, non-disabled, non-throttled account; if no enrolled accounts exist, it MUST fall back to the user's ambient Claude credentials.
- **FR-011**: System MUST isolate the selected account's credentials to the spawned executor process only (i.e., MUST NOT mutate the user's actual `~/.claude/.credentials.json` or other ambient Claude state).
- **FR-012**: System MUST classify executor failures into one of: (a) `Transient` — generic 400, 5xx, network/transport, ambiguous 429 with no usage-window context; (b) `UsageExhausted` — Anthropic 5-hour or weekly usage cap hit (matched by explicit "usage limit"/"quota exceeded"/`usage_limit`-shaped signals from the Claude CLI); (c) `NeedsReauth` — revoked/invalid token, 401/403 with auth context; (d) `Fatal` — anything else that should hard-fail.

#### Retry-with-back-off (Transient errors)

- **FR-013**: On a `Transient` failure, the system MUST sleep for the next exponential-back-off interval (using a Unix-style monotonic sleep, NOT an OS scheduler / cron) and retry the SAME account.
- **FR-014**: Back-off MUST follow a capped exponential schedule with user-configurable initial delay, multiplier, and cap. The default schedule is: initial 30s, multiplier 2x, cap 5m (30s → 1m → 2m → 4m → 5m → 5m …). The schedule MUST NOT rotate accounts.
- **FR-015**: Total retry attempts per task attempt MUST be capped at a user-configurable maximum. The default is 6. The setting `0` MUST be allowed and MUST disable retry entirely.
- **FR-016**: User-initiated cancellation of a task attempt MUST interrupt any in-progress back-off sleep within 2 seconds.

#### Rotation (UsageExhausted) — the ONLY trigger for rotation

- **FR-017**: On a `UsageExhausted` failure, the system MUST mark the active account as `Throttled` with a `throttled_until` timestamp set to the reset time reported by the error (or to a conservative default — 1 hour for the 5h window, end-of-current-week UTC for the weekly window — if no structured reset time is available).
- **FR-018**: After marking the account `Throttled`, the system MUST immediately rotate to the next healthy enrolled account WITHOUT a back-off sleep, and MUST retry the task attempt against that account.
- **FR-019**: If no healthy account remains, the system MUST sleep until the soonest `throttled_until` timestamp, then retry against that account.
- **FR-020**: A `UsageExhausted` rotation MUST count against the retry budget (FR-015) the same way a `Transient` retry does, so a pathological cycle cannot loop forever.

#### Permanent auth failures

- **FR-021**: On a `NeedsReauth` failure, the system MUST mark the account `NeedsReauth`, skip it for selection until the user re-authorizes, and rotate to the next healthy account. This failure MUST NOT count against the retry budget.

#### State persistence

- **FR-022**: Account status (including `throttled_until` timestamps and `NeedsReauth` flags) MUST persist across application restarts.
- **FR-023**: The current retry counter for an in-flight task attempt MUST persist across application restarts; on resume, the counter continues from its persisted value rather than resetting to zero. (Persisted retry counts.)

#### Token lifecycle

- **FR-024**: System MUST refresh expired access tokens transparently using the stored refresh token. A successful refresh MUST NOT count as a retry.
- **FR-025**: If token refresh fails with a non-retryable error, the system MUST move the account to `NeedsReauth` and apply FR-021.

#### Usage tracking

- **FR-026**: System MUST track, per enrolled account: current 5-hour usage (used count and reset time) and current weekly usage (used count and reset time). Values MUST be updated from data surfaced by the Claude CLI on successful and failed spawns; when no structured usage data is available, the system MUST fall back to its own incremented counter and the conservative reset defaults defined in FR-017.

#### Settings UI

- **FR-027**: Settings MUST include a "Claude Accounts" section listing every enrolled account with the following columns: account name, status (Active / Throttled / Needs re-auth / Disabled), 5-hour usage, 5-hour reset time, weekly usage, weekly reset time.
- **FR-028**: A `Throttled` row MUST display a live countdown until the relevant reset time.
- **FR-029**: Settings MUST expose a "Retry policy" panel with editable fields for: maximum retry attempts (integer ≥ 0), initial back-off delay (seconds), back-off multiplier (decimal ≥ 1), maximum back-off (seconds). Changes MUST persist immediately and apply to all subsequent task attempts.
- **FR-030**: Per-attempt logs MUST include normalized entries indicating: account selection, classification of any failure (`Transient` / `UsageExhausted` / `NeedsReauth` / `Fatal`), back-off start/end, and rotation events.

### Key Entities

- **ClaudeAccount**: Represents one enrolled Claude OAuth identity. Attributes: stable id, user-facing label, full credential blob (access token, refresh token, expires-at), created-at, last-used-at, status (`Active` / `Throttled` / `NeedsReauth` / `Disabled`), throttled-until (nullable), throttle-reason (`five_hour` / `weekly` / null), five-hour usage (used count, reset-at), weekly usage (used count, reset-at), last-error (nullable text + timestamp + classification).
- **RetryPolicy**: User-editable configuration in Settings. Attributes: max attempts per task attempt (integer ≥ 0, default 6), initial back-off seconds (default 30), back-off multiplier (default 2.0), max back-off seconds (default 300).
- **TaskAttemptRetryState**: Per task attempt persisted retry state. Attributes: task-attempt-id, current attempt number, last failure classification, last back-off duration, accounts tried (ordered list).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user with zero prior Claude credentials can enroll a working account from the Settings UI in under 90 seconds end-to-end (open Settings → click Add → complete OAuth → see Active row).
- **SC-002**: With two or more healthy accounts enrolled, a task attempt that would have failed today due to a `UsageExhausted` response from Claude succeeds without user intervention in at least 95% of induced usage-exhausted scenarios in test.
- **SC-003**: When at least one healthy alternate account exists, the time spent sleeping on back-off in response to a `UsageExhausted` event is under 1 second per rotation event (i.e., rotation happens before any sleep).
- **SC-004**: A single-account task attempt that hits a `Transient` 5xx recovers without user intervention in at least 95% of induced-transient-error scenarios in test, given the default retry policy (6 attempts).
- **SC-005**: 0 incidents in test of the rotator mutating the user's ambient `~/.claude/.credentials.json` or leaking credentials between concurrent task attempts.
- **SC-006**: Existing installs with no enrolled accounts continue to run tasks with no behavior change other than the new Settings section being visible (zero-config backward compatibility).
- **SC-007**: A user can identify, from Settings, which account is currently throttled, what kind of cap (5h or weekly) is the cause, and how long until it resets, without consulting application logs.
- **SC-008**: A task attempt that exhausts all retries fails with a normalized log entry naming every account tried and the final error class — 0 "silent" terminations.
- **SC-009**: A user can change the maximum retry attempts (and back-off knobs) from the Settings UI, and the new values apply to the next task attempt with no application restart required.
- **SC-010**: An application restart in the middle of a retry sequence preserves the retry counter — counted by comparing in-flight attempt count before and after restart in test.

## Resolved Clarifications (from user input)

- **Max retry attempts**: User-configurable in the UI. Default 6. `0` is allowed (disables retry). See FR-015 and FR-029.
- **Back-off shape**: Exponential, with user-configurable initial delay / multiplier / cap. Default 30s × 2 capped at 5m. See FR-014.
- **Rotation trigger**: ONLY on `UsageExhausted` (Anthropic 5h or weekly cap). Transient errors retry the SAME account. See FR-013 vs FR-017/018.
- **Retry counter persistence**: Persisted across application restart. The retry counter for an in-flight task attempt is durable. See FR-023.
- **Settings UI columns**: account name, 5-hour usage, 5-hour reset time, weekly usage, weekly reset time, plus status. See FR-027.
- **Default account label**: Use Anthropic OAuth userinfo (email) when available; fall back to "Account N".

## Out of Scope

- Sharing or syncing enrolled accounts across machines or users (credentials stay local).
- Per-project or per-task-attempt account pinning (a user-selectable "use account X for this project only" rule).
- Lifetime / monthly cost dashboards beyond the 5h + weekly windows.
- Smarter rotation strategies (weighted-by-remaining-quota, lowest-latency-first). MVP rotates in enrollment order across healthy accounts.
- Rotation for executors other than Claude Code (Codex, Amp, etc.) — same retry-with-back-off semantics may be desirable later but are not part of this spec.

## Open Questions / [NEEDS CLARIFICATION]

- **[NEEDS CLARIFICATION]**: Source of the structured 5h-window and weekly-window usage data. Options: (a) parse the Claude CLI's stdout/stderr usage hints, (b) call an Anthropic usage endpoint with the OAuth token, (c) maintain Vibe Kanban's own counter and use the conservative defaults from FR-017 if no real data is available. Defaulting to (c) for the first implementation unless (a) is trivially available.
- **[NEEDS CLARIFICATION]**: Whether Settings UI shows the 5-hour usage as a raw count (e.g., "120 requests") or as a percentage of a cap (e.g., "42% of 5h cap"). Defaulting to raw count plus, if known, the cap.
