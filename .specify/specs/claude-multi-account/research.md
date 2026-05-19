# Phase 0 — Research: Multi-Account Claude OAuth + Retry

**Spec**: [./spec.md](./spec.md)
**Plan**: [./plan.md](./plan.md)
**Date**: 2026-05-19

## 1. Resolution of the two remaining [NEEDS CLARIFICATION] markers

### R-1: Source of structured 5h-window / weekly-window usage data

**Decision**: Use Vibe Kanban's own counter for the "used" number. Use the unix-timestamp tail of the `Claude AI usage limit reached|<ts>` stderr line (when present) as the canonical reset time. When no reset-time is available, use the conservative defaults in FR-017.

**Rationale**: There is no documented, machine-readable Anthropic endpoint for "how much have I used in the current 5h window" available to OAuth tokens issued via the claude-code `/login` flow. The claude-code CLI itself emits usage-limit information only as a free-form string at the moment of cap-hit; it does NOT proactively emit "X requests used so far". Trying to scrape per-request usage from the CLI's stream-json output is fragile and tied to a CLI version. Maintaining our own request counter is reliable and matches the spec's stated goal ("see different accounts and their usage") well enough.

**Alternatives considered**:
- Hit `https://console.anthropic.com/v1/oauth/userinfo` or a usage endpoint — none documented for OAuth-scope tokens; the available scopes (`user:inference`, `user:profile`) don't grant usage telemetry.
- Parse rate-limit response headers — would require us to make direct API calls, which is a separate, larger architecture change.

**Implication**: The 5h/weekly counters in the UI are best-effort. If the user runs Claude outside Vibe Kanban (e.g., in a terminal), Vibe Kanban's counter will under-report. That's acceptable for MVP — the truthful signal users care about is "throttled / not throttled", which IS authoritative because it comes from the CLI's own cap-hit message.

### R-2: 5h-usage display format (raw count vs percentage)

**Decision**: Show raw count only ("142 requests this 5h window"). Do NOT show a percentage or a fraction against a cap.

**Rationale**: Anthropic does not publish per-plan caps in a way we can reliably read. Showing "42% of 5h cap" requires us to hard-code or guess the cap, which would be wrong as plans change. Raw count is unambiguous; users who care about cap can do the math themselves once.

**Implication**: When an account becomes throttled, the row flips to "Throttled until <local-time> (5h cap)" or "(weekly cap)". The discriminator is derived from `reset_at - now`: > 6h → weekly; ≤ 6h → 5h.

---

## 2. Claude OAuth flow (Pro/Max `/login`)

### Confirmed parameters

Extracted from public reverse-engineering of `@anthropic-ai/claude-code` and confirmed in two independent gists (changjonathanc/9f9d635b2f8692e0520a884eaf098351 and shubcodes/3c9c7ff813715aa47018bf22e7cf8cb5):

| Parameter | Value |
|---|---|
| `client_id` (public) | `9d1c250a-e61b-44d9-88ed-5944d1962f5e` |
| Authorization URL | `https://claude.ai/oauth/authorize` |
| Token URL (primary) | `https://console.anthropic.com/v1/oauth/token` |
| Token URL (fallback, newer rebrand) | `https://platform.claude.com/v1/oauth/token` |
| Redirect URI | `https://console.anthropic.com/oauth/code/callback` |
| PKCE | `code_challenge_method=S256`, REQUIRED |
| Scopes (minimum for inference) | `user:inference user:profile` |
| Scopes (extended, optional) | `org:create_api_key user:sessions:claude_code user:mcp_servers` |
| Access token format | Bearer, prefixed `sk-ant-oat01-`, ~8h TTL |
| Refresh token | Rotates on every refresh — MUST persist the new one each time |

### Flow shape

1. Vibe Kanban backend generates a `code_verifier` (32-byte random) and `code_challenge` = `base64url(SHA256(verifier))`.
2. Backend returns `{ auth_url, state }` to the frontend. `auth_url` includes:
   - `response_type=code`
   - `client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e`
   - `redirect_uri=https://console.anthropic.com/oauth/code/callback`
   - `scope=user:inference user:profile`
   - `state=<random>`
   - `code_challenge=<challenge>`
   - `code_challenge_method=S256`
3. Frontend opens `auth_url` in a new tab. User authorizes on `claude.ai`. Anthropic redirects to `console.anthropic.com/oauth/code/callback` where the page shows the user a code to copy.
4. User pastes the code into Vibe Kanban's Settings dialog.
5. Frontend POSTs `{ code, state }` to `/api/claude-accounts/oauth/complete`.
6. Backend exchanges the code at the token URL with `grant_type=authorization_code` + `code_verifier`. Tries primary host, falls back to rebrand host on connection error.
7. Backend stores the token bundle in `claude_accounts.json` (0600) and returns the new account row.

### Re-auth flow

Same as enrollment, but the backend keeps the existing `id`, `label`, `created_at`, and usage counters; only the credential blob is replaced. The frontend hits `/api/claude-accounts/{id}/reauth/start` and `…/reauth/complete` so the backend can identify which existing account is being refreshed.

### Token refresh

Backend serializes per-account refresh through `tokio::sync::Mutex<()>` keyed by account id. Refresh body: `grant_type=refresh_token` + `refresh_token=<current>` + `client_id=<above>`. Successful response replaces both access and refresh tokens atomically (tmp-file + rename in the store).

**Unknowns flagged**:
- The exact "production today" token URL host (`console` vs `platform`) shifts with Anthropic's rebrand. Implementation will try primary, fall back to secondary on connection error, and expose `VIBE_KANBAN_CLAUDE_OAUTH_TOKEN_URL` as an emergency override.

---

## 3. Per-spawn credential isolation

### Decision: `CLAUDE_CONFIG_DIR` + pre-write both files

Set the following before spawning the Claude CLI:

```
CLAUDE_CONFIG_DIR=<task-attempt tmpdir>
HOME=<task-attempt tmpdir>          # belt-and-suspenders, esp. macOS Keychain
```

Pre-write into `<tmpdir>`:
- `.credentials.json` — `{ "claudeAiOauth": { "accessToken": "...", "refreshToken": "...", "expiresAt": <ms>, "scopes": [...] } }`
- `.claude.json` — minimal: `{ "hasCompletedOnboarding": true, "oauthAccount": { "uuid": "...", "emailAddress": "..." } }`

### Rationale

- `CLAUDE_CONFIG_DIR` is the only documented (even if unofficially — Anthropic issue #3833) way to relocate the CLI's config tree. Honored on Linux and Windows reliably. On macOS, the CLI prefers the Keychain for credentials BUT will fall back to a file-based `.credentials.json` when present. Pre-writing the file is the supported workaround in the third-party ecosystem (e.g., `griffinmartin/opencode-claude-auth`).
- Setting `HOME` too is the belt-and-suspenders: it guarantees the CLI cannot accidentally read or write the user's real `~/.claude/`. This is the strongest defense against FR-011 ("MUST NOT mutate the user's actual `~/.claude/.credentials.json`").
- The CLI still creates a workspace-local `.claude/settings.local.json` inside the project dir. That file contains no credentials and is harmless; we tolerate it.

### Cleanup

Tmpdir is deleted at the end of the task attempt regardless of outcome. If Vibe Kanban crashes, orphaned tmpdirs in `<asset_dir>/claude_spawn_tmp/` get a sweep at startup.

### Alternatives considered

- **Mutate `~/.claude/.credentials.json` directly** — violates FR-011, racy across concurrent spawns. Rejected.
- **Symlink-farm `.claude` directories** — adds platform complexity, no benefit over `CLAUDE_CONFIG_DIR`. Rejected.
- **Patch the Claude CLI** — out of scope and brittle. Rejected.

---

## 4. Failure classification

### Decision: stderr/stdout pattern matching + exit code

| Class | Trigger | Source |
|---|---|---|
| `UsageExhausted` | Stderr OR stdout line matches `/Claude AI usage limit reached(\|(\d+))?/` | Authoritative — emitted by the CLI on cap-hit |
| `NeedsReauth` | Stderr matches `/invalid_grant|token (has been )?revoked|invalid_token/i` OR token-refresh HTTP 401/403 | CLI + our own refresh code |
| `Transient` | Any non-zero exit with stderr matching `/(API Error|server is temporarily limiting|rate limit|ECONNRESET|ETIMEDOUT|fetch failed|5\d\d)/i` AND no UsageExhausted match | CLI |
| `Fatal` | Default for any non-zero exit not matching above | Default branch |

### Ordering

Match in this order: `UsageExhausted` → `NeedsReauth` → `Transient` → `Fatal`. Bias toward `Transient` for ambiguous 429s (the CLI's "Server is temporarily limiting requests (not your usage limit)" message is the canonical example).

### Reset-time parsing

From the `Claude AI usage limit reached|<ts>` form, extract `<ts>` as a UTC unix-seconds value. Classify as 5h-cap if `(ts - now) ≤ 6h`, otherwise weekly. If `<ts>` is absent, default reset to `now + 1h` (5h) and emit a `tracing::warn!` so we can tune.

### Test fixtures

A captured corpus of real claude-code stderr/stdout snippets goes under `crates/services/tests/fixtures/claude_failures/`. The classifier is unit-tested against every fixture; new failure modes get a new fixture and a new test before the fix lands.

---

## 5. Exponential back-off design

### Schedule (defaults; user-editable per FR-029)

| Attempt | Delay |
|---|---|
| 1 (initial) | 30 s |
| 2 | 60 s |
| 3 | 120 s |
| 4 | 240 s |
| 5 | 300 s (cap hit) |
| 6+ | 300 s |

Formula: `delay = min(initial * (multiplier ^ (attempt-1)), max_backoff)`.

### Sleep mechanism

`tokio::time::sleep(delay)` inside `tokio::select!`:

```text
tokio::select! {
    _ = tokio::time::sleep(delay) => { /* proceed to retry */ }
    _ = cancellation_token.cancelled() => { /* user cancelled */ }
}
```

The existing Claude executor already creates a `CancellationToken` (see `crates/executors/src/executors/claude.rs:664`). The retry wrapper reuses it so cancellation aborts both an in-flight spawn AND a sleeping back-off in the same tokio scheduler tick.

### Invariant: rotation precedes sleep

If a `UsageExhausted` event fires AND a healthy alternate account exists, the wrapper rotates immediately with zero sleep. The exponential-back-off schedule applies only to `Transient` errors (FR-013) OR to `UsageExhausted` when no healthy alternate exists (FR-019, where the sleep duration is `min(throttled_until - now, max_backoff)`).

---

## 6. Storage: file-based vs SQLite

### Decision: file-based (`<asset_dir>/claude_accounts.json`, 0600)

Mirrors the existing `oauth_credentials.rs` pattern. Atomic save = tmp + rename.

### Rationale

- **FR-007**: credentials MUST NOT be transmitted to a remote. Several SQLite tables in this codebase are sync'd via ElectricSQL to the remote backend (`crates/remote/`); a credentials table would risk accidental inclusion. File-based store is structurally exempt.
- **FR-008**: `0600` is trivial on a file, harder on a DB row.
- **Schema flexibility**: account credential blobs may evolve as Anthropic's OAuth response shape changes. JSON tolerates that gracefully; SQL doesn't.
- **Scale**: ≤ 10 accounts per install — file IO is fine.

### Failure modes

If the file is corrupt, rename to `claude_accounts.json.bad` and start empty (FR-009). Surface a single dismissible warning in Settings. Logged via `tracing::warn!` with the parse error.

---

## 7. Cross-cutting: types generation

All new types that cross the Rust/TS boundary must be:

1. Declared in Rust with `#[derive(TS, Serialize, Deserialize)]`.
2. Registered in `crates/server/src/bin/generate_types.rs` (alongside `Tag::decl()` etc.).
3. Surfaced via `pnpm run generate-types` so `shared/types.ts` is regenerated.

New TS types expected (final list will land in `data-model.md`):

- `ClaudeAccount`
- `ClaudeAccountStatus`
- `ClaudeAccountThrottleReason`
- `ClaudeAccountUsageWindow`
- `ClaudeRetryPolicy`
- `ClaudeOAuthStartRequest` / `ClaudeOAuthStartResponse`
- `ClaudeOAuthCompleteRequest`
- `UpdateClaudeAccount`

---

## 8. Open items (carried into Phase 1+)

None of these block implementation; flagged for the design phase to keep visible:

- **macOS Keychain regression on a future CLI update**: if Anthropic stops honoring the file-based `.credentials.json` fallback on macOS, we'll need a Keychain shim. Mitigation today: HOME-override should still work because Keychain access keys off `$USER`, not `$HOME` — verify in CI.
- **Concurrent task attempts using the same account**: serialize per-account spawn entries through a `tokio::sync::Mutex` to avoid two refreshes racing. Will be reflected in `data-model.md`.
- **Telemetry**: count rotation events and back-off sleeps. Use the existing `track_if_analytics_allowed` pattern from `tags.rs:43`. Privacy-safe (no token data; just counts + error class).
