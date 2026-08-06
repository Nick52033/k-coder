---
name: review-fix
description: Repair deterministic issues from a recent code review in the current workspace. Use when Codex should read the latest code-review result, fix concrete P0/P1 issues, add or update related tests when public interfaces or branches changed, then rebuild and re-check what remains.
triggers:
  - review fix
risk: write
enabled: true
---

# Review Fix

## Overview

Use this skill after a `code-review` result exists in the current conversation. Fix only concrete, well-scoped issues from that review, then rebuild and rerun the most relevant tests.

## Workflow

### 1. Load the review result

- Read the most recent `code-review` output in the current thread.
- Extract issues by level: `P0`, `P1`, `P2`, `P3`.
- If no recent review result exists, stop and ask the user to run `/code-review` first or paste the review text.

### 2. Decide what can be auto-fixed

Fix by default:
- `P0` compile failures, missing files, broken signatures, deterministic test failures
- `P1` clear logic bugs, missing null handling, missing test updates for changed public interfaces, obvious build/test regressions

Report only by default:
- `P2` suggestions
- `P3` optional improvements

Do not auto-fix without user confirmation:
- business-rule ambiguity
- changes requiring product or workflow policy decisions
- broad refactors not required to resolve the review findings
- speculative optimizations

If a reported issue is not reproducible, say so and leave it unresolved rather than forcing a code change.

### 3. Apply the fix

- Fix highest severity first.
- Keep edits minimal and targeted to the review finding.
- If a public interface, manager/service method, controller action, or DTO changed, check whether related tests must be added or updated.
- Prefer fixing the root cause instead of only silencing the symptom.

### 4. Re-validate

- Run `dotnet build` for the most relevant `.csproj`; use `.sln` when the change spans modules or when the review finding concerns cross-project integration.
- Run the most relevant tests when behavior changed.
- If the original finding was “missing tests”, verify the new tests compile and run.

### 5. Report the outcome

Use this structure:

```markdown
## Fix Result

### Fixed
- ...

### Not Fixed
- ...

### Needs Confirmation
- ...

### Validation
- Build: ...
- Tests: ...
```

## Rules

- Do not invent review findings; only fix issues present in the latest review result or clearly discovered while reproducing those issues.
- Default scope is `P0/P1` only. Leave `P2/P3` in the report unless the user explicitly asks to fix them.
- Do not silently downgrade behavior to make tests pass.
- Do not mark a finding fixed unless code changed or validation proved the issue no longer reproduces.
- If validation cannot run, say exactly what was not run.
- If build still fails after the fix, keep the unresolved compile failure in the report.

## Priorities

1. Restore buildability.
2. Resolve `P0` findings.
3. Resolve deterministic `P1` findings.
4. Update or add tests for interface and branch changes.
5. Leave `P2/P3` reported but unchanged by default.
