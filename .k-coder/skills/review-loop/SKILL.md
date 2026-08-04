---
name: review-loop
description: Run a review-first repair loop in the current workspace. Use when Codex should review local changes, run build or tests, fix safe review findings, re-run validation, and report what remains in a single workflow.
triggers:
  - review loop
risk: write
enabled: true
---

# Review Loop

## Overview

Use this skill when the user wants one pass that reviews code, validates it, fixes deterministic issues, and re-checks the result.

## Loop

### 1. Review first

- Follow the `code-review` workflow and report format.
- Always inspect local diff first.
- Always run `dotnet build` for the most relevant project; use the solution when the scope is broad or unclear.
- Treat current-workspace compile failures as `P0`.

### 2. Classify the outcome

Split findings into:
- auto-fixable now
- requires user confirmation
- informational only

Auto-fixable findings usually include:
- compile errors
- deterministic null checks or signature mismatches
- missing test updates for changed public interfaces
- review findings with one clear implementation path

Report-only by default:
- `P2` suggestions
- `P3` optional improvements

### 3. Fix safe findings

- Apply the `review-fix` workflow to `P0` and deterministic `P1` items.
- Keep fixes minimal and localized.
- Add or update tests when interface changes or branch logic changed.
- Stop and ask the user only when the fix depends on business intent.

### 4. Re-validate

- Re-run `dotnet build`.
- Re-run the most relevant tests for the changed area.
- If the loop introduced tests, run those tests explicitly when possible.

### 5. Report the final state

Use this structure:

```markdown
## Review Loop Result

### Initial Findings
- ...

### Fixed In This Pass
- ...

### Remaining Findings
- ...

### Needs Confirmation
- ...

### Validation
- Initial build/test: ...
- Final build/test: ...
```

## Operating Rules

- Do not skip the review step.
- Default repair scope is `P0/P1` only; `P2/P3` should be reported but not changed unless the user explicitly asks.
- Do not claim “clean” if validation was not rerun after edits.
- If the first build already fails, surface that as `P0` before discussing lower-severity review findings.
- If no review findings are present and build/tests pass, say so explicitly.

## Suggested Command Flow

```bash
git status --short
git diff --cached
git diff
dotnet build path/to/affected.csproj
# optionally
dotnet test path/to/relevant.test.csproj
```

Use `.sln` builds or broader test runs only when the change scope justifies the extra time.
