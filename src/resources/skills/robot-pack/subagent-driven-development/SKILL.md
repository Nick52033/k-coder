---
name: subagent-driven-development
description: Execute suitable plan tasks through bounded subagents while one parent owns integration and final quality.
triggers: [subagent development, delegate implementation, multi agent implementation, parallel development]
risk: read
category: development_delivery
enabled: true
---
# Subagent-Driven Development

Use subagents only when the active runtime exposes them and the plan contains separable work. Assign non-overlapping files or read-only investigations. Include relevant repository rules, accepted design decisions, and a precise completion contract in each task.

Review returned work against the live workspace; do not accept a summary as proof. Resolve shared-contract changes centrally, integrate deliberately, and run the final repository gates once implementation is complete. The parent owns the result and cannot transfer approval or safety decisions to a child.
