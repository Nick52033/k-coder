---
name: record-replay
description: "Record browser action structure locally without saving typed values or navigation URLs, compile parameterized workflows to k-Coder browser tools, or replay them in system Edge. Use for record, replay, browser macro, or workflow capture requests."
---

# Record & Replay

k-Coder has no `recording_control` tool and no Replay panel. Use the repository
helper through the normal `run_command` tool, so command approval, timeout,
cancellation, output limits, audit, and process-tree cleanup remain owned by the
host. Files are written only below the current workspace.

The helper is `scripts/plugin-record-replay.mjs` in the k-Coder source
workspace. Run its commands from the current workspace root. It uses the
workspace's installed Playwright package and the system Microsoft Edge channel;
it does not install browsers or dependencies.

## Check Availability

```powershell
node scripts/plugin-record-replay.mjs doctor --workspace .
```

A failed doctor result means the local flow is unavailable until the repository
dependencies and system Edge exist. Do not substitute a cloud browser or claim
recording is active.

## Interactive Recording

Start the structural recorder in a foreground `run_command` session:

```powershell
node scripts/plugin-record-replay.mjs record --workspace . --url http://127.0.0.1:3000 --output .k-coder/recordings/example.workflow.json --timeout-ms 1800000
```

The user demonstrates the flow in the external Edge window and then closes the
recorder. Keep the command session attached. Use the host's command cancellation
when the user asks to stop; the helper also handles Ctrl+C and bounded timeout.
Do not start a detached process.

The recorder stores selectors and action order only. It never reads input values
and replaces every navigation URL with a parameter. Fill `url_N` and `input_N`
in a separate workspace JSON object using only non-sensitive replay data. Never
put credentials, URL tokens, or authorization headers into commands or files.
If a credential field was touched, compilation refuses that recording: have the
user finish authentication manually and record subsequent actions separately.
Timed out, cancelled, or truncated recordings cannot be compiled. Recording
covers top-level pages, ordinary text fields and button/link clicks; review
navigation transitions and selectors before replaying side effects. Frames,
uploads, drag-and-drop and browser dialogs need explicit manual steps.

The generated JSON is a local workflow artifact. k-Coder does not have
an automatic script card, repair conversation, publish step, or one-click panel.
Run and revise it through audited workspace file and command tools.

## Imported Workflow JSON

Imported `workflows/*.workflow.json` files can be normalized to k-Coder's real
browser tool contract:

```powershell
node scripts/plugin-record-replay.mjs compile --workspace . --workflow .k-coder/recordings/example.workflow.json --variables .k-coder/recordings/example.variables.json --output .k-coder/recordings/example.replay.json
```

Compilation accepts the six current tools:

- `browser_navigate { url }`
- `browser_snapshot {}`
- `browser_click { selector }`
- `browser_type { selector, text }`
- `browser_screenshot { fullPage? }`
- `browser_close {}`

Legacy `browser_run` navigate/type/click steps are normalized during import, but
new workflow files must use the current tool names.

Replay a compiled workflow and save an auditable report:

```powershell
node scripts/plugin-record-replay.mjs replay --workspace . --workflow .k-coder/recordings/example.replay.json --report .k-coder/recordings/example.report.json --timeout-ms 120000
```

Replay defaults to headless system Edge. Add `--headed` only when the user needs
to watch the local replay. A failure exits nonzero and still writes a bounded
report with the error and completed steps. Fix the workspace workflow or site,
then explicitly run the replay again; there is no automatic five-round repair.
The report confirms executed actions; textual `verification` and `fallback`
annotations are guidance for the agent, not executable assertions. Check the
final page against the user's intended result. Imported login examples are
marked as requiring manual sensitive input and cannot be compiled for replay.

## Path and Lifecycle Boundary

Inputs, generated recordings, and reports must use workspace-relative paths.
The helper resolves the workspace, rejects `..`, absolute targets, symbolic
links, and Windows directory junction escapes before access. Recording and
replay timeouts are limited to one hour. Browser and child processes are closed
on completion, cancellation, failure, or timeout.

Use k-Coder's `browser_*` tools when an agent should operate a page directly.
Use the structural recorder when the user wants to demonstrate a repeatable flow.
These both control external browser sessions and are separate from the workbench
preview iframe.
