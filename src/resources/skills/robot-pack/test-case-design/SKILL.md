---
name: test-case-design
description: Design traceable test cases for normal, boundary, invalid, failure, recovery, and security behavior.
triggers: [test cases, test matrix, acceptance tests, edge cases]
risk: read
category: testing
enabled: true
---
# Test Case Design

Create test cases from requirements, risks, and public contracts. Each case identifies its requirement or risk, preconditions, data, action, expected observable result, cleanup, and automation level.

Use equivalence partitions, boundaries, state transitions, decision tables, and pairwise combinations where they reduce gaps. Include invalid input, empty state, permissions, concurrency, timeout, cancellation, retry, persistence, rollback, and injection or path-escape cases when applicable. Avoid duplicate cases that do not add coverage.
