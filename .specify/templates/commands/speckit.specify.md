---
description: Create or update a feature specification from a natural language description. Focus on WHAT and WHY, not HOW.
---

## User Input

```text
$ARGUMENTS
```

You **MUST** consider the user input before proceeding (if not empty).

## Outline

1. Generate a concise short name (2-4 words) for the feature.
2. Create a feature directory: `.specify/specs/<short-name>/`
3. Load `.specify/templates/spec-template.md` for required sections.
4. Load `.specify/memory/constitution.md` for project principles.
5. Read existing codebase context (relevant `AGENTS.md`, `CLAUDE.md`, and relevant source files) to understand what already exists.

6. Fill the specification:
   - Parse user description, extract actors, actions, data, constraints
   - Generate User Stories with priorities (P1, P2, P3)
   - Each story must be independently testable
   - Write GIVEN/WHEN/THEN acceptance scenarios
   - Generate functional requirements (each must be testable)
   - Define measurable success criteria (technology-agnostic)
   - Identify edge cases
   - Maximum 3 [NEEDS CLARIFICATION] markers — make informed guesses for everything else

7. Write to `.specify/specs/<short-name>/spec.md`

8. Generate a quality checklist at `.specify/specs/<short-name>/checklists/requirements.md`
   - Validate: no implementation details, testable requirements, measurable success criteria
   - If items fail, fix the spec (max 3 iterations)

9. Report: feature directory path, spec summary, readiness for `/speckit.plan`
