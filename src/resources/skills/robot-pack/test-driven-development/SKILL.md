---
name: test-driven-development
description: Implement behavior through focused failing tests, minimal production changes, and evidence-based refactoring.
triggers: [test driven development, tdd, implement with tests, regression test]
risk: read
category: development_delivery
enabled: true
---
# Test-Driven Development

Define the observable contract first. Add a focused test that would fail for the missing behavior or regression, then implement the smallest production change that satisfies it. Refactor only after the behavior is covered.

Prefer real boundaries and existing test harnesses over implementation-detail mocks. Cover the normal path plus material invalid input, boundary, cancellation, recovery, and security branches. When execution is deliberately deferred by the user or host workflow, record that evidence gap and run the agreed consolidated gate later.

Never weaken assertions, delete coverage, or change unrelated behavior to make a test pass. Testing does not authorize commits, external calls, or destructive fixtures.
