# Requirements Quality Checklist: Multi-Account Claude OAuth with Automatic Rotation and Retry

**Purpose**: Validate that `spec.md` is implementation-ready: testable requirements, measurable success criteria, no implementation details, complete user-journey coverage.
**Created**: 2026-05-19
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
- [x] CHK012 Retry-policy FRs (FR-012 through FR-018) specify both the trigger and the resulting state.
- [x] CHK013 Token-refresh FRs (FR-019, FR-020) distinguish refresh from retry so test counters are unambiguous.
- [x] CHK014 Settings/observability FRs (FR-021 through FR-023) name the rendered fields explicitly so a UI test can assert on them.

## Measurable Success Criteria

- [x] CHK020 SC-001 has a hard time bound (90 s) and a defined start/end event.
- [x] CHK021 SC-002 has a numeric success rate (95%) and a defined induced-failure setup.
- [x] CHK022 SC-003 has a numeric latency bound (<1 s) and a defined "rotation event" trigger.
- [x] CHK023 SC-004 is a zero-incident invariant — pass/fail by counting.
- [x] CHK024 SC-005 is a zero-regression invariant — pass/fail by behavioral diff against current build.
- [x] CHK025 Every SC is technology-agnostic (no library, runtime, or schema names).

## User-Journey Coverage

- [x] CHK030 Each User Story has a Priority (P1–P3).
- [x] CHK031 Each User Story has an Independent Test paragraph that names the start state and the assertion.
- [x] CHK032 Each User Story has at least two GIVEN/WHEN/THEN scenarios.
- [x] CHK033 The headline pain ("when Claude gives us a 400, progress stops") is covered by a P1 story with explicit acceptance criteria (US2).
- [x] CHK034 The headline UX ask ("see different accounts and their usage in settings") is covered by a dedicated story (US3).

## Edge Case Coverage

- [x] CHK040 Single-account regression is explicitly addressed.
- [x] CHK041 Zero-account (existing installs) fallback is explicitly addressed.
- [x] CHK042 Mid-task token refresh is distinguished from a retry.
- [x] CHK043 Concurrent task attempts are addressed (no credential leak / collision).
- [x] CHK044 Pattern-detection false positives (user prompts containing "rate limit") are explicitly addressed.
- [x] CHK045 Persistence of back-off state across restart is addressed.
- [x] CHK046 Cancellation during back-off is addressed.

## Open Questions Are Bounded

- [x] CHK050 The spec carries at most 3 `[NEEDS CLARIFICATION]` markers.
- [x] CHK051 Each clarification names a default-position so the spec is implementable as-is if the question is not answered.

## Scope Discipline

- [x] CHK060 Out-of-scope section explicitly excludes per-project pinning, cross-machine sync, and rotation for non-Claude executors.
- [x] CHK061 Out-of-scope items would otherwise have been ambiguous in scope — they were the obvious "is this in?" questions a reader would ask.

## Notes

- Items are pre-validated by the spec author; reviewers should re-run this checklist when they read the spec for the first time and uncheck anything they disagree with.
- The `[NEEDS CLARIFICATION]` markers in the spec are the only deferred decisions; everything else has a default.
