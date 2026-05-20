---
description: Create a technical implementation plan from a feature specification. Generates research, data model, contracts, and architecture decisions.
---

## User Input

```text
$ARGUMENTS
```

You **MUST** consider the user input before proceeding (if not empty).

## Outline

1. Find the feature spec directory. If user provides a feature name, look in `.specify/specs/<name>/`. Otherwise, find the most recently modified spec.

2. Load context:
   - **Required:** spec.md from the feature directory
   - **Required:** `.specify/memory/constitution.md` for project principles
   - **Reference:** AGENTS.md, CLAUDE.md, relevant source files, existing data models

3. Fill the implementation plan (`plan.md`):
   - Technical Context: language, dependencies, storage, testing, platform
   - Constitution Check: verify all principles are respected
   - Project structure: map to project's existing structure

4. Phase 0 — Research:
   - Resolve all NEEDS CLARIFICATION items
   - Research best practices for each technology choice
   - Write `research.md` with decisions, rationale, alternatives

5. Phase 1 — Data Model:
   - Extract entities → `data-model.md`
   - Define API contracts → `contracts/` directory
   - Create `quickstart.md` for integration scenarios

6. Write all artifacts to the feature directory.

7. Report: plan path, generated artifacts, readiness for `/speckit.tasks`
