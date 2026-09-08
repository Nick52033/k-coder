---
name: dispatching-parallel-agents
description: Delegate independent bounded work to concurrent k-Coder subagents with explicit ownership and evidence.
triggers: [parallel agents, delegate tasks, concurrent work, dispatch subagents]
risk: read
category: development_delivery
enabled: true
---
# Dispatching Parallel Agents

Delegate only tasks that can proceed independently without editing the same files or relying on unfinished shared state. Give each subagent one concrete outcome, exact scope, constraints, and expected evidence. Use the k-Coder collaboration interface; do not assume another vendor's task API.

The parent remains responsible for integration, conflicts, verification, and user communication. Do not delegate secrets, approval decisions, destructive operations, or ambiguous product choices. Wait for every required subagent result before claiming completion.
