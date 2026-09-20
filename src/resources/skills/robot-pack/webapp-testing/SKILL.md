---
name: webapp-testing
description: Validate real browser workflows, responsive layout, accessibility, persistence, and failure states.
triggers: [webapp testing, browser test, e2e ui, responsive testing]
risk: external
category: testing
enabled: true
---
# Web App Testing

Test the application while it runs. Drive the user's primary workflow, judge only states you actually observed, and record the evidence behind every verdict.

## Compatibility

- Running the bundled harness `scripts/with_server.py` requires a user-provided Python 3.8+ interpreter. The host never installs Python.
- Driving a page from a Python script also requires the user's `playwright` package and a Chromium build: `pip install playwright`, then `playwright install chromium`. The host installs neither.
- When a prerequisite is missing, either use the k-Coder browser tools instead or report the blocked coverage. Never claim coverage that could not be executed.
- The bundled harness uses only the Python standard library and installs nothing. Prefer it over adding new helpers.

## Choose a harness

```
Is the page static HTML (no server, no build)?
├─ Yes → read the file and derive selectors from the source
└─ No  → is the application already running?
    ├─ No  → run the bundled harness for one self-contained pass
    └─ Yes → reconnaissance, then action:
             1. navigate and wait for the network to settle
             2. snapshot or screenshot the rendered state
             3. derive selectors from what actually rendered
             4. interact using the discovered selectors
```

- **k-Coder browser tools** (`browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_screenshot`, `browser_close`) are interactive and resumable across tool calls. Read `references/k-coder-browser-tools.md` for the exact contract and its limits.
- **Bundled harness** runs the whole pass in one `run_command` call. Read `references/playwright-harness.md` for the dependency, copy, invocation, timeout, and cleanup contract.

## Required preconditions

- The browser session must be enabled in settings and loopback navigation must be allowed; localhost targets are refused otherwise.
- Confirm the startup command, port, dependency install step, and whether login is required before starting anything.
- If the port is already in use, decide whether that instance is the application under test instead of starting a second copy.
- Waiting on a dynamic app means waiting for its network activity to settle before inspecting anything. Do not inspect a page that is still hydrating.

## Coverage

Verify visible loading, empty, success, validation, disabled, error, cancellation, and recovery states. Check keyboard access, focus order, accessible names, text overflow, responsive dimensions, and persisted state where the workflow relies on them.

Cover the happy path and the failure paths that matter. Confirm persistence and side effects instead of trusting rendered text alone, and report uncertainty instead of rounding it to a pass.

## Evidence and limits

- Record the exact startup command, port, viewport, and the exit code or final state that produced each verdict.
- k-Coder's browser tools expose no page-script evaluation, no console stream, no network interception, and no viewport control. Do not report console, network, device, or native-desktop coverage that was not actually available.
- Console errors, failed requests, or a non-zero exit code count as failures only when some tool actually observed them.
- Never fabricate a result, a screenshot, or a passing state. State precisely which checks ran and which did not.
- Always close the browser session and stop every server you started. Delete any harness or script you copied into the workspace when the run is over.

## Host boundary

This Skill grants no tools and changes no approvals. Policy assessment, sandbox rules, command timeouts, and user approval still decide what may run. Do not bypass authentication, use unapproved credentials, target production or external services, or perform irreversible submissions without explicit user authorization.
