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

### User Story 2 — Automatic rotation and retry on failure (Priority: P1)

As a Vibe Kanban user running long agent tasks, I want my task attempt to keep going when a single Claude request fails or hits a rate limit, by automatically switching to my next enrolled account and retrying with exponential back-off, so that I don't have to babysit the run.

**Why this priority**: This is the user's stated pain ("right now when Claude gives us a 400, the progress stops"). It is the headline value of the feature — multi-account enrollment without rotation/retry would be a worse UX than today.

**Independent Test**: Enroll two accounts. Force the first account into a rate-limit response (e.g., by mocking the Claude CLI to exit with a 429-shaped stderr). Confirm that the executor logs a back-off, switches to the second account, and the task completes successfully without user input.

**Acceptance Scenarios**:

1. **Given** two healthy enrolled accounts and an active task attempt, **When** the active account returns a retryable failure (rate limit, transient 5xx, network error), **Then** Vibe Kanban marks the account "Throttled" with a back-off until-timestamp, sleeps for the back-off interval, switches to the next healthy account, and resumes the task attempt without user intervention.
2. **Given** all enrolled accounts are throttled, **When** the executor needs to spawn a Claude process, **Then** Vibe Kanban waits until the soonest unblock-time, then retries with the account whose back-off expired first.
3. **Given** the active account returns a non-retryable failure (e.g., invalid credentials, permanently revoked token), **When** the executor detects it, **Then** the account is marked "Needs re-auth", the task attempt switches to the next healthy account, and the user is shown a re-auth prompt in Settings.
4. **Given** the retry policy's maximum attempt count is exceeded across all accounts, **When** all retries are exhausted, **Then** the task attempt is marked failed with a normalized log entry that names the last error and which accounts were tried.
5. **Given** a back-off is in progress, **When** the user cancels the task attempt, **Then** the back-off sleep is interrupted and the attempt stops within 2 seconds.

---

### User Story 3 — View accounts and per-account usage in Settings (Priority: P2)

As a Vibe Kanban user, I want to see all my enrolled Claude accounts, their current status, when they were last used, and how much I've used each one, so that I can decide whether to add another account or pause an over-used one.

**Why this priority**: Strong UX win but the rotation/retry in P2 works without observability. Treat as the "make it understandable" layer on top of the engine.

**Independent Test**: Enroll two accounts, run a small task. Open Settings → Claude Accounts. Both rows show distinct request counts, distinct last-used timestamps, and the status of each (Active / Throttled with countdown / Needs re-auth / Disabled).

**Acceptance Scenarios**:

1. **Given** at least one enrolled account, **When** the user opens Settings → Claude Accounts, **Then** each account row shows label, status, last-used timestamp, request count since enrollment, and the most recent error (if any).
2. **Given** an account is currently throttled, **When** the user views the accounts list, **Then** that row shows a countdown until back-off expires.
3. **Given** the user clicks "Disable" on an active account, **When** the action confirms, **Then** the account is moved to "Disabled" status and is skipped by the rotator until the user re-enables it.
4. **Given** the user clicks "Remove" on an account, **When** the action is confirmed via a destructive-action dialog, **Then** the account and its locally stored credentials are deleted and the row disappears.
5. **Given** the user clicks "Re-auth" on a "Needs re-auth" account, **When** the OAuth flow completes, **Then** the existing account's credentials are replaced (its id, label, and usage history are preserved) and its status returns to "Active".

---

### Edge Cases

- **Single-account regression**: A user with exactly one enrolled account must see no behavior change other than the new Settings UI and the retry-with-back-off (no rotation needed; back-off and retry against the same account still apply).
- **No accounts enrolled at all**: The Claude executor falls back to whatever credentials exist in the user's actual `~/.claude/.credentials.json` (today's behavior), so existing installs keep working without forced migration.
- **Token refresh during a long run**: If an account's access token expires mid-task, the system refreshes it transparently using the refresh token and does NOT count this as a retry.
- **Credentials file corruption**: If the accounts file is unreadable or malformed, the system renames it to `claude_accounts.json.bad`, starts with an empty list, and surfaces a one-time warning in Settings.
- **Concurrent task attempts**: Two task attempts running in parallel must not collide over the active-account selection. Each attempt must get an isolated credential context.
- **Stderr-pattern false positives**: A user-authored prompt that contains the phrase "rate limit" must not trigger account rotation. Pattern detection MUST be scoped to stderr lines, not stdout/assistant content.
- **Clock skew**: Back-off "until" timestamps are stored as absolute UTC; the rotator does not depend on monotonic clocks across process restarts.
- **Process killed during back-off**: If Vibe Kanban is restarted mid-back-off, account statuses MUST persist and the rotator MUST honor any unexpired back-off on next spawn.

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

#### Rotation and retry

- **FR-010**: When spawning a Claude executor, the system MUST select exactly one enrolled, non-disabled, non-throttled account; if no enrolled accounts exist, it MUST fall back to the user's ambient Claude credentials.
- **FR-011**: System MUST isolate the selected account's credentials to the spawned executor process only (i.e., MUST NOT mutate the user's actual `~/.claude/.credentials.json` or other ambient Claude state).
- **FR-012**: When the executor exits with an error matching a retryable failure pattern (rate-limit, quota, transient transport, 5xx), the system MUST mark the active account as Throttled with a back-off "until" timestamp and retry.
- **FR-013**: Back-off MUST follow a capped exponential schedule (e.g., 60s → 5m → 30m, capped at 30m) per account.
- **FR-014**: Retries MUST rotate to the next non-throttled account before sleeping; the system MUST only sleep on the back-off interval if no other healthy account is available.
- **FR-015**: System MUST cap total retry attempts per task attempt at a configurable maximum (default: 6) and mark the task attempt failed if the cap is exceeded.
- **FR-016**: When the executor exits with a non-retryable authentication failure (e.g., revoked token), the system MUST mark the account "Needs re-auth", skip it for rotation, and continue retrying with other accounts.
- **FR-017**: User-initiated cancellation of a task attempt MUST interrupt any in-progress back-off sleep within 2 seconds.
- **FR-018**: System MUST persist account status (including throttled-until timestamps) across application restarts.

#### Token lifecycle

- **FR-019**: System MUST refresh expired access tokens using the stored refresh token without counting the refresh as a retry attempt.
- **FR-020**: If token refresh fails with a non-retryable error, the system MUST move the account to "Needs re-auth" and rotate to another account.

#### Observability and Settings UI

- **FR-021**: Settings MUST include a "Claude Accounts" section listing every enrolled account with: label, status (Active / Throttled / Needs re-auth / Disabled), last-used timestamp, request count since enrollment, and last-error summary.
- **FR-022**: A throttled account row MUST display a live countdown until back-off expiry.
- **FR-023**: Per-attempt logs MUST include normalized entries indicating account selection, back-off start/end, and rotation events, scoped so they can be filtered out of model output but remain visible to the user.

### Key Entities

- **ClaudeAccount**: Represents one enrolled Claude OAuth identity. Attributes: stable id, user-facing label, full credential blob (access token, refresh token, expires-at), created-at, last-used-at, request count, status (Active / Throttled / NeedsReauth / Disabled), throttled-until (nullable), last-error (nullable text + timestamp).
- **AccountSelection**: Per-spawn snapshot of which account was used and how. Attributes: account id, spawn timestamp, outcome (success / retryable-failure / non-retryable-failure), back-off applied (nullable duration), error classification (nullable enum).
- **RetryPolicy**: Effective retry configuration for a spawn. Attributes: max attempts per task attempt, base back-off, max back-off, retryable-error classifier (the pattern set).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user with zero prior Claude credentials can enroll a working account from the Settings UI in under 90 seconds end-to-end (open Settings → click Add → complete OAuth → see Active row).
- **SC-002**: With two or more healthy accounts enrolled, a task attempt that would have failed today due to a single 429/quota response from Claude succeeds without user intervention in at least 95% of induced rate-limit scenarios in test.
- **SC-003**: The mean time the system spends sleeping on back-off when at least one healthy alternate account exists is under 1 second per rotation event (i.e., rotation happens before sleep).
- **SC-004**: 0 incidents in test of the rotator mutating the user's ambient `~/.claude/.credentials.json` or leaking credentials between concurrent task attempts.
- **SC-005**: Existing installs with no enrolled accounts continue to run tasks with no behavior change other than the new Settings section being visible (zero-config backward compatibility).
- **SC-006**: A user can identify, from Settings, which account is currently throttled and how long until it recovers, without consulting application logs.
- **SC-007**: A task attempt that exhausts all retries fails with a normalized log entry naming every account tried and the final error class — 0 "silent" terminations.

## Out of Scope

- Sharing or syncing enrolled accounts across machines or users (credentials stay local).
- Per-project or per-task-attempt account pinning (a user-selectable "use account X for this project only" rule).
- Cost/usage dashboards beyond the simple per-account request counter.
- Rotation strategies more sophisticated than "round-robin across healthy accounts" (e.g., weighted-by-quota, lowest-latency-first).
- Rotation for executors other than Claude Code (Codex, Amp, etc.) — same retry-with-back-off semantics may be desirable later but are not part of this spec.

## Open Questions / [NEEDS CLARIFICATION]

- **[NEEDS CLARIFICATION]**: Maximum retry attempts default value — proposed 6 — and whether this is exposed as a user-configurable setting in this iteration or hard-coded.
- **[NEEDS CLARIFICATION]**: Whether the "request count since enrollment" counter should reset on re-auth or persist (proposed: persist).
- **[NEEDS CLARIFICATION]**: Source of the default account label — Anthropic's OAuth userinfo response (if available) vs. a static "Account N" fallback.
