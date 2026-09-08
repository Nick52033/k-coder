---
name: brainstorming
description: Explore an idea, expose uncertainty, compare approaches, and converge on a bounded design before implementation.
triggers: [brainstorm, explore idea, compare approaches, clarify design]
risk: read
category: requirements_planning
enabled: true
---
# Brainstorming

Turn an unclear idea into an actionable design. Inspect available project context first, then identify the goal, users, constraints, success criteria, and material unknowns. Ask only questions whose answers would change the design; prefer one focused question with concrete options.

Offer two or three viable approaches when a real tradeoff exists. State a recommendation and explain its consequences. Keep assumptions, risks, decisions, and out-of-scope items explicit. Scale the design to the task: a small change needs a concise design, while a cross-cutting feature needs contracts, data flow, failure handling, migration, and verification.

Do not turn brainstorming into implementation unless the current host-managed workflow node permits it. Do not create commits, push code, contact external systems, or expand scope without user authorization.
