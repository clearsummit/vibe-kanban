# Quickstart: Multi-Account Claude OAuth + Retry

**Spec**: [./spec.md](./spec.md)
**Plan**: [./plan.md](./plan.md)
**Date**: 2026-05-19

This document is the integration-test walkthrough for the feature. Each scenario maps to one User Story / SC so a reviewer can manually verify the slice or wire the equivalent automated test.

## 0. Setup

1. Build & run Vibe Kanban locally: `pnpm i && pnpm run dev`.
2. Have at least two real Claude Pro/Max accounts available (browser sessions in two profiles, or two Google logins).
3. Open the Vibe Kanban web UI in a browser.

## 1. Enroll the first account (US1, FR-001 → FR-005, SC-001)

1. Open Settings → "Claude Accounts" (new section).
2. Confirm the page shows an empty accounts table and the retry-policy form with defaults: max_attempts=6, initial_backoff_seconds=30, backoff_multiplier=2.0, max_backoff_seconds=300.
3. Click "Add account".
4. A modal opens showing:
   - A URL ("Authorize Vibe Kanban on claude.ai")
   - An "Open in browser" button
   - A text field labeled "Paste code from claude.ai"
5. Click "Open in browser" → claude.ai opens in a new tab → log in → grant permissions → claude.ai redirects to `console.anthropic.com/oauth/code/callback` showing a code.
6. Copy the code → paste into Vibe Kanban → click "Complete enrollment".
7. The modal closes; a new row appears in the table:
   - Label: derived from your account email (e.g. "shane@clearsumm.it") OR "Account 1" if email is not surfaced.
   - Status: Active
   - 5h usage: 0 (no reset time yet)
   - Weekly usage: 0 (no reset time yet)
8. **Stopwatch from "Open Settings" to "Active row visible" should be < 90 s** (SC-001).

## 2. Enroll a second account (US1 multi-account)

Repeat §1 in a different browser tab/profile so a different Claude account is used. Confirm both accounts list with their own labels.

## 3. Edit the retry policy live (US3 scenario 7, SC-009, FR-029)

1. In Settings → Claude Accounts, change `max_attempts` from 6 to 2.
2. Tab away from the field. The "Save" button enables. Click Save.
3. Refresh the page. Confirm the value still reads 2.
4. (Test hook) Trigger a task attempt. Internal log lines should show `claude_retry_policy.max_attempts=2` at spawn time, with no app restart required.

## 4. Single-account transient retry (US2 scenario 1, SC-004)

This requires a test hook to inject failures. Use the env var `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE` (added by this feature) set to `transient_two_then_succeed`. The Claude executor will replay that mode against a mock CLI.

1. With exactly one account enrolled, start a task attempt.
2. Watch the executor log:
   - Spawn #1 → FailureClass::Transient (first injected failure)
   - Back-off sleep ~30s — visible in normalized log entries (FR-030)
   - Spawn #2 (SAME account) → FailureClass::Transient (second injected failure)
   - Back-off sleep ~60s
   - Spawn #3 (SAME account) → success
3. Confirm: account status remained `Active` the whole time. No rotation occurred. The 5h usage counter increment is 3.

## 5. Usage-exhausted rotation (US2 scenario 2, SC-002, SC-003)

1. With two accounts enrolled, set `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE=usage_limit_account_1` for the test.
2. Start a task attempt.
3. Watch the executor log:
   - Spawn #1 → FailureClass::UsageExhausted on Account 1 → Account 1 marked `Throttled` with `throttled_until` ~5h out
   - Rotator picks Account 2 → Spawn #2 → success (no sleep between spawn #1 and spawn #2)
4. Open Settings → Claude Accounts. Account 1 row reads "Throttled until <time> (5h cap)" with a live countdown. Account 2 reads "Active".
5. **Time between spawn-#1 exit and spawn-#2 start is < 1 s** (SC-003).

## 6. Usage-exhausted single account (US2 scenario 3, FR-019)

1. With exactly one account, induce `usage_limit_account_1`.
2. Spawn fails with UsageExhausted; account is marked Throttled.
3. Executor sleeps until `throttled_until` (or `max_backoff_seconds`, whichever is shorter).
4. After the sleep, spawn re-runs. (For test: short-circuit by setting `throttled_until` near in the future.)

## 7. Permanent auth failure (US2 scenario 4, FR-021)

1. With `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE=invalid_grant`, start a task attempt.
2. Account flips to `NeedsReauth`. Settings → Claude Accounts shows a "Re-auth" button.
3. The task attempt rotates to any other healthy account; the failure does NOT count against the retry budget.
4. Click "Re-auth" → repeat §1 — credentials replace in place, status returns to `Active`, label and creation date preserved.

## 8. Retry budget exhausted (US2 scenario 5, SC-008)

1. Set `max_attempts=2` (§3) and `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE=always_transient`.
2. Start a task attempt.
3. After 2 retries the attempt is marked Failed.
4. The failure log shows a normalized entry naming both accounts tried (if applicable) and the final error class. No silent termination.

## 9. Cancel during back-off (US2 scenario 6, FR-016)

1. Set `max_attempts=6` and `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE=always_transient`.
2. Start a task attempt. Wait for the first back-off sleep to begin (~30s).
3. Click "Cancel attempt" in the UI within the sleep window.
4. **Within 2 s** (FR-016), the attempt status flips to `Cancelled` and the next spawn does NOT start.

## 10. Restart mid-retry — counter persistence (US2 scenario 7, FR-023, SC-010)

1. Set `max_attempts=4` and `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE=always_transient`.
2. Start a task attempt. Let it fail twice (`attempt_number=2`).
3. Inspect `<asset_dir>/claude_retry_state/<task-attempt-id>.json` — confirm `attempt_number=2`.
4. Kill the Vibe Kanban backend (Ctrl-C in the terminal running `pnpm run dev`).
5. Restart it. Resume the task attempt.
6. **The attempt resumes at `attempt_number=3`, NOT at 1** (SC-010).

## 11. Credentials never leak to the user's real `~/.claude` (FR-011, SC-005)

Before running any task: `ls -la ~/.claude/.credentials.json && md5 ~/.claude/.credentials.json`.

Run a full task attempt through Vibe Kanban.

After: re-run the same `ls -la && md5`. **The mtime and checksum MUST be unchanged.**

For belt-and-suspenders, verify that `<asset_dir>/claude_spawn_tmp/` is empty after the attempt (cleanup invariant).

## 12. Zero-account regression (Edge Case, SC-006)

1. Remove all enrolled accounts via the UI.
2. Confirm the Claude executor still spawns successfully against the user's ambient `~/.claude/.credentials.json` (today's behavior).
3. Confirm Settings → Claude Accounts shows an empty table with an "Add account" CTA but no error banner.

## 13. Settings UI labels (US3 scenario 1, FR-027)

After completing §1-§5, the Claude Accounts table shows, per row:

- Account name (label, inline-editable)
- Status pill (Active / Throttled / Needs re-auth / Disabled)
- 5h usage: `<n> requests`
- 5h reset: `<local time>` or `—` if unknown
- Weekly usage: `<n> requests`
- Weekly reset: `<local time>` or `—` if unknown
- Actions: Disable/Enable, Re-auth (when applicable), Remove

## 14. Type safety end-to-end (Constitution II)

After implementation:

1. `pnpm run generate-types` MUST succeed and update `shared/types.ts` with the new types.
2. `pnpm run check` (frontend + all Rust workspaces) MUST pass.
3. `cargo test --workspace` MUST pass.
4. `pnpm run lint` MUST pass.
5. `pnpm run format` produces zero diff.

---

## Test hooks summary

The feature adds one debug env var (gated to `cfg(debug_assertions)` builds OR a `VIBE_KANBAN_ALLOW_TEST_HOOKS=1` override):

| Value | Behavior |
|---|---|
| `transient_two_then_succeed` | First 2 spawns exit 1 with stderr matching `"Server is temporarily limiting requests"`; 3rd succeeds. |
| `usage_limit_account_1` | First spawn against the first-enrolled account exits 1 with stdout `"Claude AI usage limit reached\|<now+5h>"`. Other accounts succeed. |
| `invalid_grant` | Token-refresh attempt returns HTTP 400 with `{"error":"invalid_grant"}`. |
| `always_transient` | Every spawn exits 1 with the transient stderr until the retry budget is exhausted. |

These hooks are the foundation for the automated tests required by Constitution V.
