---
name: webapp-testing
description: Validate real browser workflows, responsive layout, accessibility, persistence, and failure states.
triggers: [webapp testing, browser test, e2e ui, responsive testing]
risk: read
category: testing
enabled: true
---
# Web App Testing

Run the supported application and exercise the user's primary workflow through the available browser or desktop UI tools. Verify visible loading, empty, success, validation, disabled, error, cancellation, and recovery states. Check keyboard access, focus, accessible names, text overflow, responsive dimensions, and persisted state where applicable.

Use screenshots and DOM or accessibility snapshots only when the runtime exposes them. Do not claim console, network, JavaScript evaluation, mobile device, or native desktop coverage that was not actually available. Record the exact viewport and environment with evidence.
