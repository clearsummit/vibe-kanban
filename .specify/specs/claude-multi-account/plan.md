# Implementation Plan: Multi-Account Claude OAuth with Automatic Rotation and Retry

**Branch**: `vk/d3a0-please-update-vi`
**Date**: 2026-05-19
**Spec**: [./spec.md](./spec.md)
**Input**: [./spec.md](./spec.md) (3 user stories P1/P1/P2, 30 FRs, 10 SCs)

## Summary

Add a `crates/services/src/services/claude_accounts/` module that owns a `Vec<ClaudeAccount>` persisted in `<asset_dir>/claude_accounts.json` (mode 0600). Implement Claude's OAuth (PKCE S256) flow in a new route group `POST /api/claude-accounts/oauth/start` + `POST /api/claude-accounts/oauth/complete`, plus CRUD routes for accounts. Wrap the Claude executor spawn in a retry shell that classifies failures (Transient / UsageExhausted / NeedsReauth / Fatal), retries the same account with exponential back-off on Transient (Unix-style `tokio::time::sleep`), rotates immediately on UsageExhausted, and surfaces NeedsReauth to the user. Per-spawn credential isolation via `CLAUDE_CONFIG_DIR=<task-attempt tmpdir>` + pre-written `.credentials.json` and `.claude.json`. New retry-policy fields land in the existing `config.json`. UI: new `claude-accounts` Settings section with accounts table (name / status / 5h usage+reset / weekly usage+reset / actions) and an inline retry-policy editor.

## Technical Context

**Language/Version**: Rust 1.x (workspace `rust-toolchain.toml`), TypeScript 5.x (strict), React 18, Tailwind.
**Primary Dependencies**: axum, sqlx (SQLite), tokio, reqwest, serde, ts-rs, schemars, directories, chrono, uuid; web: Vite, @phosphor-icons/react, @ebay/nice-modal-react, react-i18next.
**Storage**: Local JSON file (`<asset_dir>/claude_accounts.json`, 0600) for accounts + credentials. Local JSON file (`<asset_dir>/config.json`) extended with `claude_retry_policy`. NO database table for accounts (mirrors the existing `oauth_credentials.rs` pattern — credentials never touch SQLite, never sync to remote).
**Testing**: `cargo test --workspace` for Rust; `pnpm run check` + `pnpm run lint` for web; Vitest for any new runtime-logic in TS.
**Target Platform**: macOS, Linux, Windows (same as current Vibe Kanban). macOS Keychain quirk for Claude credentials addressed in §Phase-0.
**Project Type**: Rust workspace + pnpm web monorepo (Tauri-free; runs as local server + browser/desktop client).
**Performance Goals**: Negligible — file IO < 1ms, retry sleep dominates. Per-spawn isolation tmpdir creation < 10ms.
**Constraints**: Per Constitution IV, credentials MUST be `0600` and MUST NOT enter SQLite. Per FR-011, MUST NOT mutate the user's real `~/.claude/.credentials.json`.
**Scale/Scope**: Expected ≤ 10 enrolled accounts per user. No multi-user / multi-tenant requirement (one Vibe Kanban install = one user's accounts).

## Constitution Check

| Principle | Compliance | Notes |
|---|---|---|
| I. Backend Owns State | ✅ | Account list, retry counters, throttled-until persist in backend. Frontend renders only. |
| II. Shared Types Are Generated | ✅ | All new TS types declared via ts-rs in `crates/server/src/bin/generate_types.rs`. Plan task includes the registration. |
| III. Workspace-Aware Builds | ✅ | Implementation touches `crates/` + `packages/web-core/`; CI runs `pnpm run check`, `backend:check`, `lint`. |
| IV. Secrets Stay Local | ✅ | `claude_accounts.json` written 0600 in `asset_dir()`; never inserted into SQLite; never sent to relay (excluded from sync). |
| V. Test What You Ship | ✅ | Plan includes: failure classifier unit tests, retry-loop integration tests with mocked CLI, settings UI Vitest. |
| VI. Executor Resilience | ✅ | This feature IS the resilience layer. Spec FR-013–FR-021 explicitly satisfy this. |
| VII. Settings Are Discoverable | ✅ | New `claude-accounts` section in the Settings dialog (FR-027), retry-policy editor in same section (FR-029). |

**No principle violations.** Re-check after Phase 1 design.

## Project Structure

### Documentation (this feature)

```text
.specify/specs/claude-multi-account/
├── spec.md                                 # ← already written
├── checklists/requirements.md              # ← already written
├── plan.md                                 # ← this file
├── research.md                             # ← Phase 0 output
├── data-model.md                           # ← Phase 1 output
├── quickstart.md                           # ← Phase 1 output
└── contracts/                              # ← Phase 1 output
    ├── claude-accounts.openapi.yaml
    └── retry-policy.openapi.yaml
```

### Source Code (repository root)

```text
crates/
├── services/src/services/
│   ├── claude_accounts/                    # NEW module
│   │   ├── mod.rs                          # ClaudeAccountsService public API
│   │   ├── store.rs                        # File-backed Vec<ClaudeAccount> persistence
│   │   ├── oauth.rs                        # PKCE flow against Anthropic
│   │   ├── rotator.rs                      # Account selection + back-off bookkeeping
│   │   ├── classifier.rs                   # FailureClass detection from stderr/stdout/exit
│   │   └── isolation.rs                    # Per-spawn CLAUDE_CONFIG_DIR materialization
│   └── config/mod.rs                       # EXTEND: add ClaudeRetryPolicy field
├── executors/src/executors/
│   └── claude.rs                           # EXTEND: spawn_with_retry() wrapper + env injection
├── server/src/
│   ├── routes/
│   │   ├── mod.rs                          # EXTEND: .merge(claude_accounts::router())
│   │   └── claude_accounts.rs              # NEW: CRUD + OAuth start/complete + retry-policy
│   └── bin/generate_types.rs               # EXTEND: register new TS-exported structs
└── db/
    └── (no new tables — file-based only)

packages/web-core/src/shared/dialogs/settings/settings/
├── ClaudeAccountsSettingsSection.tsx       # NEW: accounts table + retry editor
├── settingsRegistry.tsx                    # EXTEND: register 'claude-accounts' section
└── (existing files untouched)
```

## Phase Roadmap

The detailed phase artifacts (research, data-model, quickstart, contracts) follow this plan. Implementation order, optimized for the spec's two P1 stories landing first:

| Phase | Wave | Deliverable | Verifies |
|---|---|---|---|
| 1 | A | DB-less storage + tests | FR-007/008/009 |
| 1 | A | Failure classifier + tests | FR-012 |
| 1 | A | Retry policy in config.json + types | FR-029 |
| 2 | B | Per-spawn isolation (`CLAUDE_CONFIG_DIR`) | FR-010/011 |
| 2 | B | Retry-with-back-off wrapper (single-account path) | FR-013–FR-016, FR-023 |
| 2 | B | Rotation on UsageExhausted | FR-017–FR-020 |
| 2 | B | NeedsReauth handling | FR-021, FR-025 |
| 3 | C | OAuth (PKCE) flow backend | FR-001 |
| 3 | C | Account CRUD routes | FR-002–FR-006 |
| 3 | C | Token refresh | FR-024 |
| 4 | D | `ClaudeAccountsSettingsSection.tsx` | FR-027/028 |
| 4 | D | Retry policy editor | FR-029 |
| 4 | D | OAuth UI flow | US1 |
| 5 | E | E2E: induced UsageExhausted → rotation | SC-002, SC-003 |
| 5 | E | E2E: induced Transient → same-account retry | SC-004 |
| 5 | E | Restart-mid-retry integration test | SC-010 |

Waves A/B are independent of OAuth (wave C) — useful for shipping retry-with-back-off as a standalone PR if scope needs to slip.

## Phase 0 — Research (see research.md)

Resolves the two remaining `[NEEDS CLARIFICATION]` markers in the spec with implementation-grade decisions:

1. **Source of 5h/weekly usage data**: Vibe Kanban's own counter, reset on first observed reset-timestamp. Source-of-truth for "is this account throttled?" is the `Claude AI usage limit reached|<unix_ts>` stderr signal from the CLI; the counter is informational only.
2. **5h usage display format**: Raw count with no cap shown (we don't know Anthropic's per-account cap reliably from the CLI). When throttled, show "Throttled until <local-time>".

Plus design decisions on:
- OAuth client_id / endpoints / redirect URI / scopes (extracted from public claude-code binary; confirmed against two reverse-engineering write-ups).
- `CLAUDE_CONFIG_DIR` as the credential-isolation knob; macOS Keychain mitigation by pre-writing the file.
- Exponential back-off shape and the "do not sleep before rotation" invariant.
- File-based store vs SQLite (file-based wins because of FR-007 "never enter syncing DB").

## Phase 1 — Data & Contracts

See `data-model.md` for entity layout and `contracts/*.openapi.yaml` for API contracts.

Highlights:
- **`ClaudeAccount`** lives in `claude_accounts.json` (Vec) — keyed by `id: Uuid`, holds full credential blob + status + throttle metadata + usage counters.
- **`ClaudeRetryPolicy`** lives in the existing `config.json` so it's user-editable from the same flow as other settings.
- **`TaskAttemptRetryState`** persists per task attempt — small file `<asset_dir>/claude_retry_state/<task_attempt_id>.json` so a crash mid-retry preserves the counter (FR-023).
- HTTP API:
  - `GET    /api/claude-accounts`
  - `POST   /api/claude-accounts/oauth/start` → `{ auth_url, state }`
  - `POST   /api/claude-accounts/oauth/complete` → `{ account }`
  - `POST   /api/claude-accounts/{id}/reauth/start` (same shape as oauth/start, scoped to existing id)
  - `POST   /api/claude-accounts/{id}/reauth/complete`
  - `PATCH  /api/claude-accounts/{id}` (label, disabled flag)
  - `DELETE /api/claude-accounts/{id}`
  - `GET    /api/claude-accounts/retry-policy`
  - `PUT    /api/claude-accounts/retry-policy`

## Composition / UI Strategy

The Vibe Kanban Settings dialog uses a registry pattern (`packages/web-core/src/shared/dialogs/settings/settings/settingsRegistry.tsx`). Adding a section means:

1. Add `'claude-accounts'` to the `SettingsSectionType` union.
2. Add `{ id: 'claude-accounts', icon: KeyIcon, group: 'host' }` to `SETTINGS_SECTION_DEFINITIONS`.
3. Implement `ClaudeAccountsSettingsSection.tsx` rendering:
   - A retry-policy form at the top (max attempts, initial back-off, multiplier, cap) — uses existing `SettingsComponents` primitives.
   - An accounts table with columns from FR-027.
   - Per-row actions: `Disable / Enable`, `Re-auth` (only when `NeedsReauth`), `Remove`.
   - An "Add account" button that opens a small flow modal showing the auth URL + a code paste field.
4. Register the section in the `renderSettingsSection` switch.

No new design system primitives required; reuses existing buttons, modals, and form controls.

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| macOS Keychain overrides `$CLAUDE_CONFIG_DIR/.credentials.json` | Set `HOME=<tmpdir>` AND `CLAUDE_CONFIG_DIR=<tmpdir>` for the spawn; pre-write `.credentials.json`. Verify with an integration test that runs on macOS CI. |
| Claude CLI changes its "usage limit reached" string | Classifier matches a stable substring (`usage limit reached`) with a regex for the optional `|<unix_ts>` tail. Unit-test against captured fixtures. If the pattern goes away, fall back to "treat as Transient" (graceful degradation; biases toward more retries, not silent failure). |
| OAuth endpoints rebrand (`console.anthropic.com` → `platform.claude.com`) | Try the modern host first, fall back to the legacy host on connection error. Both URLs are configurable via `VIBE_KANBAN_CLAUDE_OAUTH_TOKEN_URL` env var for emergency override. |
| Token refresh races between concurrent spawns | All refreshes serialized through a per-account `tokio::sync::Mutex`. Refresh-token rotation persisted atomically (tmp-file + rename, same pattern as `oauth_credentials.rs`). |
| Retry budget too small to absorb realistic incident | Default 6 is plenty for a single-account 5xx burst. Configurable from UI per FR-029 — user can dial up. |
| User cancels mid-back-off | Sleep is via `tokio::time::sleep` inside a `tokio::select!` against the existing executor `CancellationToken` (already wired into the spawn — see `claude.rs:664`). Cancel wins within tokio's scheduler tick. |
| Restart mid-back-off loses retry count | `TaskAttemptRetryState` is written to disk before every sleep (FR-023). On startup, the executor's resumption path reads it. |

## Definition of Done

- All FRs in `spec.md` have either an implementation task in `tasks.md` or are explicitly marked covered-by-existing-code.
- All SCs have a corresponding automated test (unit or integration). Manual checklists are not acceptable.
- `pnpm run check`, `pnpm run lint`, `cargo test --workspace` are green.
- `pnpm run format` has run.
- A demo recording (or a 3-step manual script in `quickstart.md`) shows: add account → induce UsageExhausted → see rotation → restart → see retry counter resume.
