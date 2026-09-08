---
name: verification-before-completion
description: Require fresh, relevant evidence before claiming that work is complete, fixed, passing, or ready.
triggers: [verify completion, final verification, prove fix, quality gate]
risk: read
category: quality_review
enabled: true
---
# Verification Before Completion

Identify the exact command or observable workflow that proves each completion claim. Run the current repository-required gates after implementation, inspect their exit status and relevant output, and distinguish passed, failed, skipped, and unavailable checks.

Match evidence to the claim: compilation does not prove behavior, unit tests do not prove desktop interaction, and mocked output does not prove an external integration. Review the final diff for accidental churn and secret leakage. Never claim completion based on expectation, stale output, or another agent's assertion.

Verification does not authorize commits, pushes, deployments, destructive cleanup, or external publication.
