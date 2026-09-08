---
name: requesting-code-review
description: Request a focused review of a concrete diff, contract, or implementation before delivery.
triggers: [request code review, review my changes, inspect diff, implementation review]
risk: read
category: quality_review
enabled: true
---
# Requesting Code Review

Provide the reviewer with the accepted requirements, repository constraints, changed files or revisions, and verification already performed. Ask for bugs, regressions, security issues, contract drift, concurrency and recovery risks, and missing tests, ordered by severity.

Use a subagent only when available and useful; otherwise perform the same evidence-driven review locally. Validate every finding against the current code before changing it. A review request never authorizes committing, pushing, or broad refactoring.
