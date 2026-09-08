---
name: receiving-code-review
description: Evaluate review feedback technically, apply valid findings, and explain rejected or deferred advice.
triggers: [review feedback, address comments, fix review findings, evaluate review]
risk: read
category: quality_review
enabled: true
---
# Receiving Code Review

Read each finding in context and verify the referenced behavior in the current revision. Clarify ambiguous feedback before making a materially different change. Prioritize correctness and safety over agreement.

For valid findings, add or update focused coverage and implement the smallest coherent correction. For invalid, incompatible, or out-of-scope feedback, explain the concrete evidence and tradeoff. Re-run the affected verification after changes and keep unresolved risk visible.
