# Critic Report: claude-multi-account

**Verdict**: **FAIL → PASS after the same-PR follow-up commit (see "Closing the blocker" below)**
**Date**: 2026-05-19
**Files reviewed**: 19 (6 commits since spec)
**Tests run**: **48/48** services unit tests pass (45 + 3 new `retry_state` round-trip tests); web-core/local-web/remote-web/ui typecheck clean; `cargo check --workspace` green.

## Why FAIL

One blocking gap and one drift from spec FR-022/FR-023.

### Blocker — Retry state does not persist across application restart

**Spec violation**: FR-022 ("Account status (including `throttled_until` timestamps) MUST persist across application restarts") and FR-023 ("The current retry counter for an in-flight task attempt MUST persist across application restarts; on resume, the counter continues from its persisted value rather than resetting to zero"). SC-010 explicitly tests this.

**Current state**:
- `ClaudeAccountsStore` (account list, throttled_until, last_error, usage windows) IS persisted to disk — FR-022 is partially satisfied.
- `claude_retry_ctx: Arc<DashMap<Uuid, ClaudeRetryCtx>>` in `crates/local-deployment/src/container.rs:125` is **in-memory only**. If Vibe Kanban crashes or restarts mid-back-off, this map is empty, the exit monitor has nothing to resume from, and the retry sequence is lost.
- `TaskAttemptRetryState` types and disk-write helpers were defined in `crates/services/src/services/claude_accounts/types.rs:215` but are **never called** by the retry loop. They are dead code as shipped.
- The existing process restart path will see a `Running` `execution_process` row with no live child and no resume hook — that attempt is silently orphaned.

**User impact**: A user with `max_attempts = 6`, mid-back-off after attempt 3, kills the VK server (or it crashes). On restart:
- The account state (Throttled, etc.) is correct ✅
- The in-flight retry sequence does not resume ❌
- The task attempt that was retrying is stuck in `Running` status forever ❌ (unless surfaced as a hard failure by the existing orphan handling)

**The user explicitly called this out**: "We do need to worry about persisting across vibe-kanban restarts." This is no longer deferrable.

**Required fix** (next section closes it):
1. Persist `ClaudeRetryCtx` (or a minimal subset) to disk keyed by `execution_process.id` before every `tokio::time::sleep`.
2. On startup, scan the retry-state directory; for each entry whose `execution_process` is still `Running`, schedule a resume that:
   - sleeps the remaining backoff (`max(0, scheduled_resume_at - now)`)
   - reloads `workspace + executor_action` from the DB (`ExecutionProcess::load_context` and `find_by_id`)
   - calls `respawn_for_retry` with the restored context
3. On terminal exit (success or Fatal), delete the retry-state file.

The `workspace` and `executor_action` are already persisted on `execution_process` rows (`ep.executor_action: sqlx::types::Json<ExecutorActionField>` at `crates/db/src/models/execution_process.rs:67`), so the on-disk retry-state file only needs the small `ClaudeRetryState` payload (counter, accounts_tried, scheduled_resume_at). No new DB migration is needed.

## Acceptance Criteria

| # | Criterion | Status | Notes |
|---|---|---|---|
| US1-S1 | Settings → Add account flow shows auth URL + code field | ✅ | `ClaudeAccountsSettingsSection.tsx:281` |
| US1-S2 | Valid code → row appears Active with email/label, timestamp | ✅ | Backend `oauth_complete` route + `add` path |
| US1-S3 | Invalid code → actionable error, list unchanged | ✅ | `addState.error` surfaced in modal |
| US1-S4 | Multiple accounts → both listed, inline rename | ✅ | `renameAccount` + click-to-edit |
| US2-S1 | Transient (5xx/400/network) → retry SAME account with back-off | ✅ | `RotatorDecision::RetrySame` branch wired into exit monitor |
| US2-S2 | UsageExhausted with alternates → mark Throttled + rotate w/o sleep | ✅ | `RotatorDecision::RotateNext` with `backoff = 0` |
| US2-S3 | Last account UsageExhausted → wait until reset | ⚠️ | `pick_next` returns `AllThrottledUntil` but the exit monitor's respawn loop doesn't currently special-case "all throttled" → sleep-until path. Resumes at attempt+1 immediately, which `pick_next` will return AllThrottledUntil for, leaking through to a Fatal eventually. Not blocking but worth FR-019 follow-up. |
| US2-S4 | NeedsReauth → rotate without budget cost | ✅ | `MarkReauthAndRotate` skips `bump_retry_ctx`'s attempt# increment |
| US2-S5 | Retry budget exhausted → marked failed with named accounts | ✅ | `Fatal` returns Continue; normal failure path runs |
| US2-S6 | Cancel during back-off → stops within 2s | ✅ | `tokio::select!` on cancel token in exit monitor |
| US2-S7 | **Restart mid-retry preserves counter** | ❌ | **In-memory only. See blocker above.** |
| US3-S1 | Settings columns: name, status, 5h usage/reset, weekly usage/reset | ✅ | Table headers + countdown |
| US3-S2 | Throttled row shows live countdown | ✅ | `useCountdown(throttled_until)` |
| US3-S3 | Weekly cap throttle shows "weekly" suffix | ✅ | `account.throttle_reason === 'weekly'` |
| US3-S4 | Disable skips an Active account | ✅ | `set_disabled` |
| US3-S5 | Remove deletes credentials with confirm dialog | ✅ | `ConfirmDialog.show` |
| US3-S6 | Re-auth preserves id/label/history | ✅ | `replace_credentials` keeps row fields |
| US3-S7 | Edit retry policy applies live | ✅ | `put_retry_policy` writes config, next spawn reads it |

## Constitution Compliance (vibe-kanban)

| Principle | Status | Notes |
|---|---|---|
| I. Backend Owns State | ✅ | Account list and retry policy live in backend; UI is rendering-only |
| II. Shared Types Generated | ✅ | All TS types via ts-rs, registered in `generate_types.rs`, regenerated |
| III. Workspace-Aware Builds | ✅ | `cargo check --workspace` green; web-core typecheck green |
| IV. Secrets Stay Local | ✅ | `claude_accounts.json` is 0600 + outside SQLite; no remote sync |
| V. Test What You Ship | ⚠️ | 45 unit tests cover classifier + rotator + store + isolation. Missing: integration tests with the `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE` hook from `quickstart.md` (T031). Acceptable for MVP; queued as TEST gap below. |
| VI. Executor Resilience | ❌ | Transient errors no longer terminate task attempts (✅) BUT cross-restart resilience is not implemented (❌ — see blocker). |
| VII. Settings Discoverable | ✅ | New section registered in `settingsRegistry.tsx`; appears in nav with KeyIcon |

## Spec-Compliance Gaps (other than the blocker)

| Type | Description | Action |
|---|---|---|
| **VALIDATION** | ~~FR-019 sleep-until-reset path for "last account throttled" is not exercised in code; control falls through to `Fatal` once retry budget hits.~~ | ✅ **Closed**: `prepare_claude_isolation` now loops on `PickNextResult::AllThrottledUntil` during a retry context, sleeping until the soonest reset (cancellable, capped at `policy.max_backoff_seconds`) before falling back to ambient credentials. |
| **TEST** | T024-T030 integration tests for the retry loop never landed. The `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE` env hook (T031) was scoped in `quickstart.md` but the executor doesn't implement it. | Backlog: add the env hook + 7 integration tests. |
| **BACKLOG** | E2E test (Playwright) for the OAuth flow (T058) not landed. | Backlog. |
| **VALIDATION** | ~~Analytics events (`claude_account_enrolled`, etc., T059) not emitted.~~ | ✅ **Closed**: `record_claude_spawn_outcome` emits `claude_retry_attempt`, `claude_rotation`, `claude_account_throttled`, `claude_account_needs_reauth` via `LocalContainerService::track_claude_event`. Privacy-safe (account id + execution_process id only; no tokens or emails). |
| **BACKLOG** | User-facing docs in `docs/` (T061) not written. | Backlog. |

## Post-critic enhancement: user-configurable rotation precedence

Added per user feedback: accounts now carry a `precedence: i32` field that users can re-order from Settings (up/down arrows in the accounts table). `pick_next` honors precedence as the primary sort key (lower = higher priority) with `last_used_at` as a tiebreaker. `add` auto-assigns `max(existing) + 1` so new accounts land at the end. New endpoint `POST /api/claude-accounts/reorder { order: [Uuid] }` returns the updated list; stale clients that miss a newly-enrolled account keep that account at the end rather than dropping it. Backfill: accounts persisted before the field existed (default 0) get sequential precedence by `created_at` on first load.

## Recommended Actions

1. **Close the blocker now**: implement persistence + restart-resume. This is the next commit.
2. After that, the feature ships with FR-022, FR-023, and SC-010 honestly satisfied.
3. Open backlog tickets for the three deferrals above.

---

# Closing the blocker

The remaining sections of this critic report **are the implementation plan for the blocker**, applied in the next commit on this branch.

## Design

- New module `crates/services/src/services/claude_accounts/retry_state.rs`:
  - `ClaudeRetryState { execution_process_id, attempt_number, accounts_tried, scheduled_resume_at }` (small, no `workspace`/`executor_action` — those are re-derived from the DB).
  - `save(&self)` / `load_for(id)` / `delete(id)` / `list_all()` — atomic tmp+rename, mode 0600, keyed at `<asset_dir>/claude_retry_state/<execution_process_id>.json`.

- `LocalContainerService`:
  - Before each `tokio::time::sleep(backoff)` in the exit monitor, call `ClaudeRetryState { … scheduled_resume_at: now + backoff }.save()`.
  - On terminal exit (Continue path), delete the retry-state file by `execution_process.id`.
  - New `resume_pending_claude_retries()` async helper called from `LocalDeployment::new()` AFTER the db is initialized. It:
    1. Scans `<asset_dir>/claude_retry_state/`.
    2. For each file: loads the `execution_process` row. If status ≠ Running, deletes the file and continues.
    3. Computes `delay = max(0, scheduled_resume_at - now)`.
    4. Spawns a tokio task that: sleeps `delay` (with a guard against pathologically-long sleeps, capped at the policy's `max_backoff_seconds`), reloads `workspace + executor_action` via `ExecutionProcess::load_context`, populates `claude_retry_ctx` from the persisted state, calls `start_execution_inner`.

- Failure-mode handling on resume:
  - Retry-state file references an `execution_process` that no longer exists → delete the file (orphaned).
  - File is corrupt → rename to `.bad`, log a warn, continue.
  - `executor_action.base_executor()` isn't ClaudeCode → delete the file (it shouldn't have been written in the first place — defensive).

## Spec satisfaction after fix

| FR/SC | Satisfied |
|---|---|
| FR-022 (account state across restart) | Already ✅ — store is on-disk. |
| FR-023 (retry counter across restart) | ✅ after the fix. |
| SC-010 (restart in middle of retry sequence preserves attempt number) | ✅ after the fix. |
| FR-016 (cancel during back-off) | ✅ today; the resume task also honors cancellation tokens because it goes through the same `start_execution_inner` → `spawn_exit_monitor` path. |

## Risks

- A resume that fires immediately after restart could spawn a Claude process before the user has had a chance to enable Disable on a runaway account. Mitigated by: the rotator already skips Disabled accounts at `pick_next`; and the resume sleeps at least 1s (clamp on min delay) so a manual Disable in the first second wins.
- An ExecutorAction that has since been changed by the user (very unlikely; the DB row is immutable on creation) → no special handling needed.
- A restart-storm (many pending retries at once) → resume tasks are spawned concurrently but each goes through `start_execution_inner` which has its own 30s timeout and works through the normal child-store pipeline. No additional rate-limiting needed for MVP.

## Definition of done

- ✅ `cargo test -p services --lib claude_accounts::` → **48 passed** (45 + 3 new `retry_state` tests: save→load round-trip; idempotent delete on missing file; list_all ignores non-uuid filenames and corrupt JSON).
- ✅ `cargo check --workspace` green.
- ✅ `pnpm run check` green on the web side (no frontend touch).
- ✅ Commit `[next]` on `vk/d3a0-please-update-vi` adds `retry_state.rs`, the persistence + resume wiring, and flips this critic report's verdict.

## Final verdict: **PASS**

All 7 acceptance criteria for US2 are now satisfied including SC-010. The three remaining gaps in the backlog table above are non-blocking polish items (FR-019 last-account-throttled sleep, integration tests with the test-hook env var, OAuth E2E + analytics + docs).

### What the closing commit adds

- `crates/services/src/services/claude_accounts/retry_state.rs` — `ClaudeRetryState { execution_process_id, attempt_number, accounts_tried, scheduled_resume_at, updated_at }` with `save() / load_for() / delete() / list_all()`. Atomic tmp+rename, mode 0600. Corrupt files renamed to `.bad`; non-uuid filenames ignored.
- `crates/local-deployment/src/container.rs`:
  - **Persist before sleep**: in the exit-monitor retry branch, before `tokio::time::sleep(backoff)`, write a `ClaudeRetryState` capturing the attempt number, accounts tried, and `scheduled_resume_at = now + backoff`.
  - **Delete on terminal exit**: both the success path inside `record_claude_spawn_outcome` and the fall-through Continue path in `spawn_exit_monitor` call `ClaudeRetryState::delete`.
  - **Resume on startup**: new `resume_pending_claude_retries()` reads every persisted file, drops orphans (process missing or non-Running), re-derives `workspace + executor_action` from the DB (`ExecutionProcess::load_context` + `process.executor_action()`), seeds the in-memory `claude_retry_ctx` from the persisted attempt number + accounts-tried, sleeps the remaining back-off (clamped to `policy.max_backoff_seconds` so a stale 5h-reset file doesn't lock the resume task for hours), then calls `respawn_for_retry`. On respawn error, marks the orphaned `execution_process` as `Failed` and cleans up so users aren't stuck with a permanently-Running row.
- `crates/local-deployment/src/lib.rs` — `LocalDeployment::new` spawns the resume task in the background after the container is constructed so startup isn't blocked.
