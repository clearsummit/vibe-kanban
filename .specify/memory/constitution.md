# Vibe Kanban Constitution

## Core Principles

### I. Backend Owns State
All persistent state lives in the Rust backend. The web frontend is a rendering layer that calls API endpoints. No business logic in React components beyond view/local-interaction concerns. Refreshing the page MUST restore full state from the backend.

### II. Shared Types Are Generated
TypeScript types shared between Rust and the web are generated via ts-rs. Never hand-edit `shared/types.ts` or `shared/remote-types.ts`. Edit the Rust generator binaries instead and re-run `pnpm run generate-types` (or `pnpm run remote:generate-types`).

### III. Workspace-Aware Builds
This is a Cargo + pnpm monorepo. Changes that touch crates MUST pass `pnpm run check` and `pnpm run backend:check`. Changes that touch web code MUST pass `pnpm run lint`. Format with `pnpm run format` before completing a task.

### IV. Secrets Stay Local
Never commit secrets. Per-user credentials (OAuth tokens, API keys, refresh tokens) MUST be stored in the user's local config directory with `0600` permissions on Unix, never in the database or in any file checked into git.

### V. Test What You Ship
- Rust: prefer `#[cfg(test)]` unit tests next to the code; run `cargo test --workspace`
- Web: ensure `pnpm run check` and `pnpm run lint` pass; add Vitest tests for runtime logic
- A bug fix MUST include a regression test that would have caught it

### VI. Executor Resilience
Coding-agent executor processes are owned by Vibe Kanban. Transient upstream failures (rate limits, auth errors, transport blips) MUST NOT silently terminate a task attempt without an explicit retry policy. Failures that ARE terminal MUST be surfaced to the user in normalized log entries, not swallowed.

### VII. Settings Are Discoverable
Anything a user can configure MUST be reachable through the Settings dialog. Configuration that lives only in CLI flags, env vars, or hidden config files is not "configurable" — it is invisible.
