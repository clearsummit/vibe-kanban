# Tasks: Multi-Account Claude OAuth with Automatic Rotation and Retry

**Feature**: [claude-multi-account](./spec.md) • **Plan**: [plan.md](./plan.md)
**Branch**: `vk/d3a0-please-update-vi`
**Date**: 2026-05-19

**Legend**: `[P]` = parallelizable (different files, no shared mutable state); `[US1]` / `[US2]` / `[US3]` = user-story tag from `spec.md`.

> Constitution V: every acceptance criterion gets a test stub written **before** the implementation task that satisfies it. Test stubs use `#[ignore = "TDD: pending T0xx"]` (Rust) or `it.todo()` (Vitest) until the implementing task lands.

---

## Phase 1 — Setup

Pre-work shared by every user story.

- [ ] **T001** Create the new module skeleton: empty files for `crates/services/src/services/claude_accounts/mod.rs`, `store.rs`, `oauth.rs`, `rotator.rs`, `classifier.rs`, `isolation.rs`; wire `pub mod claude_accounts;` into `crates/services/src/services/mod.rs`.
- [ ] **T002** [P] Add `claude_accounts_path()` and `claude_retry_state_dir()` helpers to `crates/utils/src/assets.rs` (returning `<asset_dir>/claude_accounts.json` and `<asset_dir>/claude_retry_state/` respectively, creating parent dirs on demand).
- [ ] **T003** [P] Add `claude_spawn_tmp_dir()` helper to `crates/utils/src/assets.rs` returning `<asset_dir>/claude_spawn_tmp/`.
- [ ] **T004** [P] Wire a startup sweep in `crates/server/src/lib.rs` (or wherever `Deployment::init()` is) that calls `claude_accounts::isolation::sweep_orphaned_tmp_dirs()` (mtime > 1h) and `claude_accounts::store::sweep_stale_retry_state()` (mtime > 30d).

**Checkpoint**: `cargo check -p services -p utils -p server` is green.

---

## Phase 2 — Foundational (BLOCKS every user story)

Shared types, classifier, retry policy, isolation primitives. NOTHING in Phase 3+ may start until this phase passes.

### Types & data model

- [ ] **T005** Define the data-model types in `crates/services/src/services/claude_accounts/types.rs`: `ClaudeAccount`, `ClaudeAccountView`, `ClaudeAccountStatus`, `ClaudeAccountThrottleReason`, `ClaudeAccountUsageWindow`, `ClaudeAccountLastError`, `FailureClass`, `ClaudeOAuthCredentials` (no `TS` derive; `#[ts(skip)]` on the field inside `ClaudeAccount`). Match the shapes in `data-model.md`.
- [ ] **T006** Define `ClaudeRetryPolicy` (with `Default` impl: 6 / 30 / 2.0 / 300) in `crates/services/src/services/claude_accounts/types.rs`. Implement `fn backoff_delay(&self, attempt: u32) -> Duration`.
- [ ] **T007** Define `TaskAttemptRetryState` in `crates/services/src/services/claude_accounts/types.rs`, with `load_for(task_attempt_id)`, `save()`, `delete()` async methods that read/write `<asset_dir>/claude_retry_state/<id>.json` (mode 0600 via tmp-file + rename).
- [ ] **T008** Register every TS-exported type in `crates/server/src/bin/generate_types.rs` per the list in `data-model.md` §"Type-export registration".
- [ ] **T009** Run `pnpm run generate-types`. Commit the regenerated `shared/types.ts` change.

### Failure classifier

- [ ] **T010** Test fixtures: capture 6 representative stderr/stdout snippets under `crates/services/tests/fixtures/claude_failures/` — `usage_limit_5h.txt`, `usage_limit_weekly.txt`, `invalid_grant.txt`, `transient_rate_limit.txt`, `transient_5xx.txt`, `unknown_fatal.txt`.
- [ ] **T011** Write classifier unit tests in `crates/services/src/services/claude_accounts/classifier.rs#[cfg(test)]` covering all 6 fixtures + ordering rule (UsageExhausted wins over NeedsReauth, NeedsReauth wins over Transient).
- [ ] **T012** Implement `classify_failure(stdout: &str, stderr: &str, exit_code: Option<i32>) -> FailureClass` plus `parse_usage_reset(text: &str) -> Option<(DateTime<Utc>, ClaudeAccountThrottleReason)>` in `crates/services/src/services/claude_accounts/classifier.rs`. Make T011 pass.

### Retry policy config

- [ ] **T013** Extend `crates/services/src/services/config/mod.rs` (or sibling file holding the `Config` struct) with `#[serde(default)] pub claude_retry_policy: ClaudeRetryPolicy`. Verify `serde` defaulting on existing configs without the field.
- [ ] **T014** Add validation: `ClaudeRetryPolicy::validate(&self) -> Result<(), ValidationError>` enforcing the ranges from `data-model.md` (incl. `max_backoff_seconds >= initial_backoff_seconds`). Unit-test in `types.rs#[cfg(test)]`.

### Per-spawn isolation primitives

- [ ] **T015** Implement `materialize_credentials(creds: &ClaudeOAuthCredentials, oauth_account: Option<&OauthAccountInfo>) -> Result<TempCredentialDir>` in `crates/services/src/services/claude_accounts/isolation.rs`. Creates `<asset_dir>/claude_spawn_tmp/<uuid>/` (0700), writes `.credentials.json` and `.claude.json` (0600).
- [ ] **T016** Implement `TempCredentialDir::env_vars(&self) -> HashMap<String, String>` returning `CLAUDE_CONFIG_DIR` and `HOME` set to the tmpdir.
- [ ] **T017** Implement `Drop` for `TempCredentialDir` that recursively deletes the directory (or schedules deletion if drop happens off the runtime).
- [ ] **T018** Unit test in `isolation.rs#[cfg(test)]` that materializes a fake credential set, asserts file existence + mode 0600 on Unix, and that drop removes the directory.

### Store (file-backed `Vec<ClaudeAccount>`)

- [ ] **T019** Implement `ClaudeAccountsStore` in `crates/services/src/services/claude_accounts/store.rs`: `RwLock<Vec<ClaudeAccount>>` + atomic save (tmp-file + rename, mode 0600). All methods listed in `data-model.md` §Operations: `list`, `add`, `replace_credentials`, `update_label`, `set_disabled`, `remove`, `mark_throttled`, `mark_needs_reauth`, `record_success` (debounced 1/s), `record_failure`, `pick_next` (round-robin, skip Throttled+Disabled+NeedsReauth).
- [ ] **T020** Per-account refresh mutex map: `Arc<DashMap<Uuid, Arc<Mutex<()>>>>` field on the store. Method `lock_for_refresh(id) -> OwnedMutexGuard`.
- [ ] **T021** Failure-mode tests in `store.rs#[cfg(test)]`: corrupt-file recovery (renames to `.bad`, starts empty), atomic save under concurrent writers, round-robin selection with mixed statuses.

### Dependency injection / wiring

- [ ] **T022** Expose `claude_accounts: Arc<ClaudeAccountsService>` on the `DeploymentImpl` struct. Construct at startup. Pattern: mirror how `oauth_credentials` is held today.
- [ ] **T023** Update `crates/services/src/services/mod.rs` re-exports so `services::services::claude_accounts::ClaudeAccountView` etc. are reachable from `server` and `executors` crates.

**Checkpoint**: `cargo test -p services` is green for all new unit tests. `pnpm run generate-types` produces no diff after a clean run.

---

## Phase 3 — US2 (P1): Retry-with-back-off + Rotation on UsageExhausted

> **Story goal**: When Claude fails, retry the same account with exponential back-off (Transient) OR immediately rotate to the next healthy account (UsageExhausted), bounded by a user-configurable retry budget.
>
> **Independent test**: §4 and §5 of [`quickstart.md`](./quickstart.md). Inducing `transient_two_then_succeed` recovers without rotation; inducing `usage_limit_account_1` rotates within < 1s of the failure.

### Test stubs (write FIRST)

- [ ] **T024** [US2] Integration test stub `crates/executors/tests/claude_retry_test.rs::transient_two_then_succeed_single_account` covering FR-013, FR-014, FR-015, US2 acceptance scenario 1.
- [ ] **T025** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::usage_exhausted_rotates_without_sleep` covering FR-017, FR-018, US2 acceptance scenario 2, SC-002, SC-003.
- [ ] **T026** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::usage_exhausted_single_account_sleeps_until_reset` covering FR-019, US2 acceptance scenario 3.
- [ ] **T027** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::needs_reauth_rotates_without_budget_cost` covering FR-021, FR-025, US2 acceptance scenario 4.
- [ ] **T028** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::budget_exhaustion_marks_failed` covering FR-015, SC-008, US2 acceptance scenario 5.
- [ ] **T029** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::cancel_during_backoff_stops_within_2s` covering FR-016, US2 acceptance scenario 6.
- [ ] **T030** [US2] [P] Integration test stub `crates/executors/tests/claude_retry_test.rs::restart_mid_retry_preserves_counter` covering FR-023, SC-010, US2 acceptance scenario 7.

### Test infrastructure

- [ ] **T031** [US2] Add the test-hook env var `VIBE_KANBAN_CLAUDE_TEST_FAILURE_MODE` in `crates/executors/src/executors/claude.rs` (gated behind `#[cfg(any(test, feature = "test-hooks"))]` or `if std::env::var("VIBE_KANBAN_ALLOW_TEST_HOOKS").is_ok()`). Modes: `transient_two_then_succeed`, `usage_limit_account_1`, `invalid_grant`, `always_transient`. The hook intercepts at spawn time and emits the appropriate stderr/stdout + exit code instead of running the real CLI.

### Rotator

- [ ] **T032** [US2] Implement `Rotator` in `crates/services/src/services/claude_accounts/rotator.rs` with:
  - `pick_next(&self, attempts_tried: &[Uuid]) -> Result<RotatorPick>` returning either a `ClaudeAccount` or `SleepUntil(DateTime<Utc>)` when all healthy accounts are exhausted but at least one is Throttled. Returns `NoAccountsAvailable` if every account is Disabled / NeedsReauth.
  - `on_failure(&self, account_id: Uuid, class: FailureClass, parsed_reset: Option<(DateTime<Utc>, ThrottleReason)>) -> RotatorDecision` (`RetrySame(delay)` / `RotateNext` / `MarkReauthAndRotate` / `Fatal`).
  - Honors `ClaudeRetryPolicy` from `Config`.
- [ ] **T033** [US2] Unit tests in `rotator.rs#[cfg(test)]` for every `RotatorDecision` branch including the bias-toward-Transient rule.

### Retry wrapper around Claude spawn

- [ ] **T034** [US2] Extract today's `ClaudeCode::spawn_internal()` body into `spawn_internal_once()` and add a new `spawn_with_retry()` in `crates/executors/src/executors/claude.rs`. The wrapper:
  - Loads or creates a `TaskAttemptRetryState`.
  - On each iteration: `rotator.pick_next()` → `materialize_credentials()` → run `spawn_internal_once()` with the tmpdir env → wait for exit → read collected stdout/stderr → `classifier.classify_failure()` → `rotator.on_failure()`.
  - On `RetrySame(delay)`: write `TaskAttemptRetryState` to disk, then `tokio::select! { _ = tokio::time::sleep(delay) => {} _ = cancel.cancelled() => return Cancelled }`.
  - On `RotateNext`: NO sleep; loop immediately.
  - On `MarkReauthAndRotate`: do NOT increment retry counter (FR-021).
  - On success: delete the `TaskAttemptRetryState` file.
  - On `Fatal` or budget-exhaustion: emit a normalized log entry naming every account tried (FR-030, SC-008) and bubble the error.
- [ ] **T035** [US2] Pipe the user's `CancellationToken` from the existing executor code (`claude.rs:664`) through to the sleep `tokio::select!` so cancellation interrupts back-off in ≤ 2s (FR-016).

### Make T024-T030 pass

- [ ] **T036** [US2] Iterate on `rotator.rs` + `spawn_with_retry()` until every test from T024-T030 passes. Remove `#[ignore]` markers as each lands.

**Checkpoint (MVP for US2)**: `cargo test -p executors --test claude_retry_test` is green. Together with Phase 2, this is shippable as a standalone retry-with-back-off PR even without OAuth UI.

---

## Phase 4 — US1 (P1): Enroll Claude account via OAuth

> **Story goal**: Add multiple Claude accounts via OAuth from the Settings UI.
>
> **Independent test**: §1 and §2 of [`quickstart.md`](./quickstart.md). Stopwatch < 90s end-to-end (SC-001).

### Test stubs (write FIRST)

- [ ] **T037** [US1] Rust integration test stub `crates/server/tests/claude_accounts_oauth_test.rs::oauth_start_returns_url_and_state` covering `POST /api/claude-accounts/oauth/start`.
- [ ] **T038** [US1] [P] Test stub `crates/server/tests/claude_accounts_oauth_test.rs::oauth_complete_exchanges_code_and_enrolls` (mocks the Anthropic token endpoint) covering `POST /api/claude-accounts/oauth/complete`, US1 scenario 2.
- [ ] **T039** [US1] [P] Test stub `crates/server/tests/claude_accounts_oauth_test.rs::oauth_complete_rejects_bad_state` covering US1 scenario 3.
- [ ] **T040** [US1] [P] Test stub `crates/server/tests/claude_accounts_oauth_test.rs::patch_and_delete_account` covering FR-003, FR-004 and US1 scenario 4 (rename + remove).
- [ ] **T041** [US1] [P] Test stub `crates/server/tests/claude_accounts_oauth_test.rs::reauth_replaces_credentials_in_place` covering FR-005 (id, label, usage history preserved).

### OAuth backend

- [ ] **T042** [US1] Implement `ClaudeOAuthClient` in `crates/services/src/services/claude_accounts/oauth.rs`:
  - `build_auth_url(&self, account_id: Option<Uuid>) -> ClaudeOAuthStartResponse` (returns `auth_url` + `state`, stores `PendingOAuth` in an internal `Mutex<HashMap<String, PendingOAuth>>` with TTL 10m).
  - `complete(&self, state: &str, code: &str) -> Result<(ClaudeOAuthCredentials, Option<OauthAccountInfo>)>` exchanges the code, tries the primary token URL then falls back to the rebrand host on connection error. Honors `VIBE_KANBAN_CLAUDE_OAUTH_TOKEN_URL` env override.
  - `refresh(&self, creds: &mut ClaudeOAuthCredentials) -> Result<()>` rotates the refresh token.
  - Hardcoded constants: `client_id`, `auth_url`, redirect URI. Pulled from `research.md` §2.
- [ ] **T043** [US1] Unit tests in `oauth.rs#[cfg(test)]`: PKCE pair generation correctness, `build_auth_url` formats every required query param, state-bag eviction at 10m, primary→fallback host on connection error (use a stub HTTP server).

### Route handlers

- [ ] **T044** [US1] Create `crates/server/src/routes/claude_accounts.rs` modeled on `tags.rs` (the cleanest precedent in the codebase). Implement handlers:
  - `list_accounts` → `GET /claude-accounts`
  - `oauth_start` → `POST /claude-accounts/oauth/start` (accepts optional `account_id` for re-auth)
  - `oauth_complete` → `POST /claude-accounts/oauth/complete`
  - `patch_account` → `PATCH /claude-accounts/{id}`
  - `delete_account` → `DELETE /claude-accounts/{id}`
  - `get_retry_policy` → `GET /claude-accounts/retry-policy`
  - `put_retry_policy` → `PUT /claude-accounts/retry-policy`
  Use `ApiError`, `ApiResponse::success`, and the `State<DeploymentImpl>` pattern from `tags.rs`. Every handler converts `ClaudeAccount` → `ClaudeAccountView` before returning.
- [ ] **T045** [US1] Wire `router()` into `crates/server/src/routes/mod.rs` (.merge(claude_accounts::router())) alongside the other route modules.
- [ ] **T046** [US1] Token refresh integration: extend `ClaudeAccountsService` with `get_active_credentials(id) -> Result<ClaudeOAuthCredentials>` that checks `expires_at`, refreshes via `ClaudeOAuthClient::refresh` if needed (under per-account mutex from T020), atomically persists the rotated tokens, and returns the live credential bundle. Used by the rotator.
- [ ] **T047** [US1] Make T037-T041 pass. Remove `#[ignore]` markers.

**Checkpoint**: backend OAuth flow is end-to-end testable via curl per `contracts/claude-accounts.openapi.yaml`.

---

## Phase 5 — US3 (P2): Settings UI

> **Story goal**: See each account with status, 5h/weekly usage and reset times in Settings; edit retry policy live.
>
> **Independent test**: §3 and §13 of [`quickstart.md`](./quickstart.md).

### Test stubs (write FIRST)

- [ ] **T048** [US3] Vitest stub `packages/web-core/src/shared/dialogs/settings/settings/ClaudeAccountsSettingsSection.test.tsx::renders_columns_per_FR_027` (use `it.todo` until T053 lands).
- [ ] **T049** [US3] [P] Vitest stub `…/ClaudeAccountsSettingsSection.test.tsx::retry_policy_form_persists_on_save` covering FR-029 and SC-009.

### Frontend types & API client

- [ ] **T050** [US3] Verify `shared/types.ts` (regenerated by T009) exposes every new type. Add no new manual types; only consume the generated ones.
- [ ] **T051** [US3] Add API hooks in `packages/web-core/src/shared/api/` (mirror existing tag/repo hook patterns): `useClaudeAccounts`, `useClaudeRetryPolicy`, `useStartClaudeOAuth`, `useCompleteClaudeOAuth`, `usePatchClaudeAccount`, `useDeleteClaudeAccount`, `usePutClaudeRetryPolicy`.

### Settings section UI

- [ ] **T052** [US3] Register the new section in `packages/web-core/src/shared/dialogs/settings/settings/settingsRegistry.tsx`:
  - Add `'claude-accounts'` to the `SettingsSectionType` union.
  - Add `{ id: 'claude-accounts', icon: KeyIcon, group: 'host' }` to `SETTINGS_SECTION_DEFINITIONS` (import `KeyIcon` from `@phosphor-icons/react`).
  - Add `case 'claude-accounts': return <ClaudeAccountsSettingsSection />` to `renderSettingsSection`.
  - Add an entry to `SettingsSectionInitialState`.
- [ ] **T053** [US3] Implement `packages/web-core/src/shared/dialogs/settings/settings/ClaudeAccountsSettingsSection.tsx`:
  - Top: retry-policy form (max_attempts / initial_backoff_seconds / backoff_multiplier / max_backoff_seconds) using `SettingsComponents` primitives. Save button calls `usePutClaudeRetryPolicy`.
  - Bottom: accounts table with columns from FR-027 (Account name, Status pill, 5h usage, 5h reset, Weekly usage, Weekly reset, Actions).
  - Status pill colors: Active=green, Throttled=amber with countdown, NeedsReauth=red, Disabled=gray.
  - Inline-edit label on click.
  - Per-row actions: Disable/Enable, Re-auth (only when NeedsReauth), Remove (with `ConfirmDialog`).
  - Empty state with "Add account" CTA.
- [ ] **T054** [US3] Implement the OAuth modal `packages/web-core/src/shared/dialogs/settings/settings/ClaudeAddAccountModal.tsx`:
  - Shows the `auth_url` returned by `oauth_start`.
  - "Open in browser" button (uses `window.open`).
  - "Paste code" text input + "Complete enrollment" button.
  - On success, closes and triggers a refetch of `useClaudeAccounts`.
  - On error, surfaces actionable message (FR-001 scenario 3).
- [ ] **T055** [US3] Live countdown for Throttled rows: small `useCountdown(throttledUntil)` hook updating every second. Discriminate 5h vs weekly via `(reset - now) > 6h ? 'Weekly' : '5h'`.
- [ ] **T056** [US3] i18n strings under `packages/web-core/public/locales/en/settings.json` (or wherever the existing i18n lives) for every new label. Cross-check `t('…')` calls compile against the bundle.
- [ ] **T057** [US3] Make T048 and T049 pass.

**Checkpoint**: open Settings → Claude Accounts in a real browser; perform every action listed in `quickstart.md` §1, §2, §3, §13.

---

## Phase 6 — Polish & Cross-Cutting

- [ ] **T058** [P] E2E test (Playwright or Vitest+RTL — match the project's existing pattern) for the full OAuth flow + first task attempt using mocked Anthropic endpoints. Covers SC-001 end-to-end.
- [ ] **T059** [P] Analytics: emit `claude_account_enrolled`, `claude_account_throttled`, `claude_retry_attempt`, `claude_rotation` events via `deployment.track_if_analytics_allowed` (pattern from `tags.rs:43`). Privacy: account `id` only, never tokens or email.
- [ ] **T060** [P] Update `CLAUDE.md` / `AGENTS.md` with a short "Claude Accounts" section pointing at `crates/services/src/services/claude_accounts/` and `packages/web-core/src/shared/dialogs/settings/settings/ClaudeAccountsSettingsSection.tsx`.
- [ ] **T061** [P] Add an entry to `docs/` (Mintlify) explaining account enrollment + retry policy from the user's perspective. Use the existing doc layout.
- [ ] **T062** Sweep: `pnpm run format` produces zero diff. `pnpm run lint`, `pnpm run check`, `pnpm run backend:check`, `cargo test --workspace` all green.
- [ ] **T063** Manual run-through of every scenario in `quickstart.md` §0-§14. Capture and attach a screen recording (or a 6-step manual checklist with screenshots) for the PR.

---

## Dependency graph

```
T001 ─┬─ T005 ─ T008 ─ T009
      │
      ├─ T010 ─ T011 ─ T012        (classifier)
      │
      └─ T013 ─ T014               (config)

T002 ─ T015 ─ T016 ─ T017 ─ T018   (isolation)

T019 ─ T020 ─ T021                 (store)

T022 ─ T023                        (DI)

──── Phase 2 complete ────

T031 (test hook)
  │
  ├─ T024 ┐
  ├─ T025 │
  ├─ T026 ├─ T032 ─ T033 ─ T034 ─ T035 ─ T036
  ├─ T027 │
  ├─ T028 │
  ├─ T029 │
  └─ T030 ┘

──── US2 (P1) shippable here ────

T042 ─ T043
  │
T044 ─ T045 ─ T046 ─ T047
  │
T037..T041 (tests)

──── US1 (P1) shippable here ────

T050 ─ T051 ─ T052 ─ T053 ─ T054 ─ T055 ─ T056 ─ T057
  │
T048, T049 (tests)

──── US3 (P2) shippable here ────

T058 .. T063 (polish, parallel)
```

## Parallelization opportunities

- **Wave A (after T001-T004)**: T005, T010, T015, T019 in parallel.
- **Wave B (after Phase 2)**: T024-T030 (test stubs) all `[P]`; T031 sequential.
- **Wave C (after T031)**: T032/T033 → T034 → T035 → T036.
- **Wave D**: T042/T043 parallel with T037-T041 stubs; then T044-T047.
- **Wave E**: T050-T051 → T052-T053-T054 (mostly sequential UI work); T055/T056 in parallel.
- **Wave F**: T058, T059, T060, T061 all `[P]`. T062 sequential. T063 last.

## MVP scope

The minimum shippable slice covering both P1 stories:

- All of Phase 1 + Phase 2
- All of Phase 3 (US2 — retry + rotation)
- All of Phase 4 (US1 — OAuth)
- T050, T051, T052, T053, T054 from Phase 5 (basic Settings UI)
- T062 from Phase 6 (format/lint/test gate)

US3's deeper UX (T055 live countdown, T056 i18n, T057 component tests) and polish (T058-T061, T063) can ship in a follow-up without compromising the P1 user-visible value.

## Totals

- **63 tasks** across 6 phases.
- **24 tasks** are `[P]` parallelizable.
- Per-story split: Foundational + Setup = 23 tasks; US1 = 11; US2 = 13; US3 = 10; Polish = 6.

Ready for `/speckit.implement` (single-PR build) or for wave-by-wave execution.
