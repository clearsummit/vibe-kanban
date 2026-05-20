# Requirements Quality Checklist: Multi-Account Claude OAuth with Automatic Rotation and Retry

**Purpose**: Validate that `spec.md` is implementation-ready: testable requirements, measurable success criteria, no implementation details, complete user-journey coverage.
**Created**: 2026-05-19
**Updated**: 2026-05-19 (folded in user clarifications: configurable retry, usage-only rotation, 5h/weekly UI)
**Feature**: [spec.md](../spec.md)

## Implementation-Free Spec

- [x] CHK001 Spec does not name specific files, modules, structs, functions, or DB tables.
- [x] CHK002 Spec does not name specific languages, frameworks, libraries, or CLI tools by version.
- [x] CHK003 Spec does not prescribe a UI layout, component tree, or styling approach.
- [x] CHK004 Spec does not specify wire protocols (HTTP routes, status codes, JSON shapes).
- [x] CHK005 Where the spec references external systems (Anthropic OAuth, Claude Code CLI), it does so only to fix WHAT — not HOW the integration is wired internally.

## Testable Requirements

- [x] CHK010 Every FR is phrased as an observable behavior (input → state change or output), not a design directive.
- [x] CHK011 Every FR is independently testable — a single FR can be the subject of one test case.
- [x] CHK012 Failure classification is explicit (FR-012: Transient / UsageExhausted / NeedsReauth / Fatal) — the four classes are mutually exclusive and exhaustively cover the failure modes a test must induce.
- [x] CHK013 Retry-with-back-off FRs (FR-013–FR-016) and rotation FRs (FR-017–FR-020) are clearly separated by trigger class, so tests cannot accidentally exercise rotation when validating retry-on-transient-error.
- [x] CHK014 Token-refresh FRs (FR-024, FR-025) distinguish refresh from retry so test counters are unambiguous.
- [x] CHK015 Persistence FRs (FR-022, FR-023) are testable by killing the process between failures.
- [x] CHK016 Settings/observability FRs (FR-027 through FR-030) name the rendered fields explicitly so a UI test can assert on them.
- [x] CHK017 The retry-policy panel FR (FR-029) names every editable field so the form test is unambiguous.

## Measurable Success Criteria

- [x] CHK020 SC-001 has a hard time bound (90 s) and a defined start/end event.
- [x] CHK021 SC-002 has a numeric success rate (95%) and a defined induced-failure setup (UsageExhausted, not "anything").
- [x] CHK022 SC-003 has a numeric latency bound (<1 s) and a defined "rotation event" trigger.
- [x] CHK023 SC-004 covers the single-account transient case — distinct from rotation success, ensures retry-w/-back-off itself is verified.
- [x] CHK024 SC-005 is a zero-incident invariant — pass/fail by counting.
- [x] CHK025 SC-006 is a zero-regression invariant — pass/fail by behavioral diff against current build.
- [x] CHK026 SC-009 is testable without a restart (UI change applies live).
- [x] CHK027 SC-010 is testable with an explicit restart in the middle of an attempt.
- [x] CHK028 Every SC is technology-agnostic (no library, runtime, or schema names).

## User-Journey Coverage

- [x] CHK030 Each User Story has a Priority (P1–P3).
- [x] CHK031 Each User Story has an Independent Test paragraph that names the start state and the assertion.
- [x] CHK032 US2 covers both the retry-same-account path AND the rotate-on-usage-exhausted path in distinct acceptance scenarios.
- [x] CHK033 US2 covers a restart-mid-retry scenario (scenario 7) so the persistence invariant is journey-verified, not just FR-verified.
- [x] CHK034 The headline pain ("when Claude gives us a 400, progress stops") is covered by a P1 story with explicit acceptance criteria (US2 scenario 1).
- [x] CHK035 The headline UX ask ("see different accounts and their usage in settings") is covered by US3 with explicit columns (account name, 5h usage, 5h reset, weekly usage, weekly reset).
- [x] CHK036 US3 has a dedicated scenario (7) for the retry-policy editor so the configurable-retry user input is journey-tested.

## Edge Case Coverage

- [x] CHK040 Single-account regression is explicitly addressed (no rotation possible; back-off-then-retry against the same account).
- [x] CHK041 Zero-account (existing installs) fallback to ambient `~/.claude/.credentials.json` is explicitly addressed.
- [x] CHK042 Mid-task token refresh is distinguished from a retry (FR-024).
- [x] CHK043 Concurrent task attempts are addressed (no credential leak / collision).
- [x] CHK044 Pattern-detection false positives (user prompts containing "rate limit") are explicitly addressed AND the rule is tightened: rotation triggers only on usage-exhausted signals from the CLI itself.
- [x] CHK045 Persistence of throttled-until AND of the retry counter across restart is addressed (FR-022, FR-023).
- [x] CHK046 Cancellation during back-off is addressed (FR-016).
- [x] CHK047 Unknown / unstructured usage window has a defined conservative fallback (FR-017).
- [x] CHK048 Max-retries `= 0` is explicitly allowed and defined to disable retry (FR-015 + Edge Case).
- [x] CHK049 Misclassification policy is explicit: prefer Transient over UsageExhausted when ambiguous (avoids false rotations).

## Resolved Clarifications

- [x] CHK050 Max retry attempts default + configurable: resolved (FR-015 + FR-029 + Resolved Clarifications section).
- [x] CHK051 Rotation trigger: resolved as UsageExhausted-only (FR-013 vs FR-017/018).
- [x] CHK052 Retry-counter persistence: resolved as persistent (FR-023).
- [x] CHK053 Settings UI columns: resolved (FR-027).
- [x] CHK054 Default account label: resolved (OAuth userinfo first, "Account N" fallback).

## Remaining Open Questions

- [x] CHK060 Two `[NEEDS CLARIFICATION]` markers remain (≤ 3) and each carries a default so the spec is implementable without further input:
  - Source of structured 5h/weekly usage data (default: Vibe Kanban's own counter + conservative reset defaults).
  - Display format of 5h usage (default: raw count + cap when known).

## Scope Discipline

- [x] CHK070 Out-of-scope section explicitly excludes per-project pinning, cross-machine sync, lifetime cost dashboards, smarter rotation strategies, and rotation for non-Claude executors.
- [x] CHK071 Out-of-scope items would otherwise have been ambiguous in scope — they are the obvious "is this in?" questions a reviewer would ask.

## Notes

- Items are pre-validated by the spec author; reviewers should re-run this checklist on first read and uncheck anything they disagree with.
- The "rotation only on UsageExhausted" decision is the load-bearing change vs the first draft — any subsequent plan/implementation must NOT rotate on plain 429/5xx.
