# k-Coder browser tool contract

Use only the browser tools the runtime actually registers, and only after the
browser session has been enabled in settings.

| Tool | Argument | Notes |
| --- | --- | --- |
| `browser_navigate` | `url` | HTTP(S) only, no credentials in the URL. Loopback and private addresses are refused unless loopback navigation is allowed in settings. |
| `browser_snapshot` | — | Bounded plain-text snapshot of `document.body.innerText`. Not a DOM tree and not an accessibility tree. |
| `browser_click` | `selector` | CSS selector only. Derive it from the rendered page, not from guessed markup. |
| `browser_type` | `selector`, `text` | CSS selector plus bounded text. |
| `browser_screenshot` | `fullPage` | Stored as a bounded artifact, not written into the workspace. |
| `browser_close` | — | Close the session when the workflow ends. |

Behavior worth relying on:

- Snapshot before acting, and re-snapshot after navigation or any state change.
- Capture the screenshot that proves the final state; a verdict without a
  screenshot or a snapshot has no evidence.
- Sessions are cancellable, and navigation is denied when the session is disabled.

Limits that must be reported rather than worked around:

- No page-script evaluation, no injected scripts.
- No console stream, no network interception, no request-failure events.
- No viewport, device, locale, or user-agent control.
- No DOM or accessibility query beyond the text snapshot.
- No downloads, uploads, or file pickers.

When a check requires one of these, either obtain it through the user's own
Playwright script (see `references/playwright-harness.md`) and say which tool
produced the evidence, or report the coverage as not verified. Never describe a
limitation as a pass.
