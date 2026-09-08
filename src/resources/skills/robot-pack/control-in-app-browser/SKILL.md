---
name: control-in-app-browser
description: Inspect and operate a supported in-app browser with bounded, auditable actions.
triggers: [in app browser, browser control, inspect page, browser automation]
risk: external
category: integration_automation
enabled: true
---
# Control In-App Browser

Use only the browser tools exposed by the current k-Coder runtime. Navigate to an approved target, inspect the current snapshot before acting, identify controls by accessible labels or stable evidence, and perform one bounded interaction at a time. Re-snapshot after navigation or state changes.

Do not bypass approval, authentication, origin restrictions, or localhost policy. Never assume JavaScript evaluation, console, network interception, download, or upload capabilities unless the tool registry explicitly provides them. Avoid irreversible submissions and external side effects without explicit user authorization. Read references/k-coder-browser-tools.md for the supported interaction contract.
