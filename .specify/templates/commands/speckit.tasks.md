---
description: Generate an actionable, dependency-ordered task list from the implementation plan. Tasks are organized by user story for independent implementation.
---

## User Input

```text
$ARGUMENTS
```

You **MUST** consider the user input before proceeding (if not empty).

## Outline

1. Find the feature directory. If user provides a name, look in `.specify/specs/<name>/`. Otherwise, find the most recently modified spec with a plan.md.

2. Load design documents:
   - **Required:** plan.md (tech stack, architecture, file structure)
   - **Required:** spec.md (user stories with priorities)
   - **Optional:** data-model.md, contracts/, research.md, quickstart.md

3. Generate `tasks.md` organized by user story:

   **Phase 1: Setup** — Project initialization, shared infrastructure
   **Phase 2: Foundational** — Blocking prerequisites (MUST complete before any user story)
   **Phase 3+: User Stories** — One phase per story, in priority order (P1, P2, P3...)
   **Final Phase: Polish** — Cross-cutting concerns, documentation, optimization

4. Task format (REQUIRED):
   ```
   - [ ] T001 [P?] [US?] Description with exact file path
   ```
   - `[P]` = parallelizable (different files, no dependencies)
   - `[US1]` = maps to user story from spec.md
   - Every task MUST include the target file path

5. For each user story phase, include:
   - Story goal and independent test criteria
   - Test tasks (acceptance criteria → automated tests)
   - Implementation tasks (models → services → endpoints → UI)
   - Checkpoint marker for independent validation

6. Generate dependency graph and parallel execution opportunities.

7. Write to `.specify/specs/<feature>/tasks.md`

8. Report: task count, per-story breakdown, MVP scope, parallel opportunities.
