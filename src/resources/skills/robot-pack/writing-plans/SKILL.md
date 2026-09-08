---
name: writing-plans
description: Convert an approved design or concrete request into an implementation plan with traceable, verifiable steps.
triggers: [write plan, implementation plan, development plan, task breakdown]
risk: read
category: requirements_planning
enabled: true
---
# Writing Plans

Create a plan only after reading the relevant repository instructions, roadmap, architecture, and affected contracts. Describe the outcome and current facts before decomposing the work.

Each step must name the owned area, behavior to change, important failure or security branches, and verification evidence. Order steps by dependency. Keep UI, command boundary, runtime, persistence, and protocol responsibilities within their existing architecture.

Use the repository's normal plan location and format. Do not invent paths, promise tests that cannot run, or hide unresolved decisions in implementation steps. A plan never grants permission to commit, push, deploy, delete data, or invoke external services.
