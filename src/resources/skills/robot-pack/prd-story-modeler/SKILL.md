---
name: prd-story-modeler
description: Model actors, user stories, functional requirements, rules, states, and testable acceptance criteria.
triggers: [user story, functional model, acceptance criteria, interaction flow]
risk: read
category: requirements_planning
enabled: true
---
# PRD Story Modeler

Translate accepted requirements into traceable user stories and functional requirements. Use stable IDs such as US-001 and FR-001. Every story identifies the actor, intent, value, prerequisites, normal flow, relevant exceptions, and acceptance criteria.

Acceptance criteria must be observable and decidable as yes or no. Cover empty, loading, permission, invalid input, cancellation, retry, and recovery states where relevant. Model important state transitions and cross-page or cross-service flows with Mermaid when a diagram improves precision.

Do not invent features to fill perceived gaps. Mark unresolved rules and return them to requirements intake instead of silently choosing product behavior.
