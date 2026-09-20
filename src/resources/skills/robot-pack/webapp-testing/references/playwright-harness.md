# Server harness and Playwright check script

Read this before starting a server or copying `scripts/with_server.py`.

## 1. Prerequisites the user must provide

Check first, and stop with a precise blocker if a check fails.

```bash
python --version          # fall back to python3, then py -3
python -m playwright --version
```

- Python 3.8+ is required to run the harness.
- The `playwright` Python package and a Chromium build are required only for the
  Playwright check script: `pip install playwright`, then
  `playwright install chromium`.
- The host installs nothing and downloads nothing. Do not run package installs on
  the user's behalf without asking, and never install to work around a missing
  interpreter.

If Playwright is unavailable but the k-Coder browser tools are, run the checks
interactively with those tools instead and record the reduced coverage.

## 2. Getting the harness into the workspace

`scripts/with_server.py` is a read-only Skill resource. The host never executes
it; only the policy-checked `run_command` can.

1. Read it with `skill_resource_read` (`skillId: "webapp-testing"`,
   `path: "scripts/with_server.py"`, raise `limit` until `truncated` is false).
2. Write the exact text into the workspace with `write_file`, for example
   `.k-coder/tmp/with_server.py`. Never commit it and never place it inside
   `src/resources/skills/`.
3. Delete it when the run ends, together with any check script you created.

Do not rewrite, paraphrase, or "fix" the harness before running it. If it must
change, say so explicitly and show the change.

## 3. Harness contract

```bash
python .k-coder/tmp/with_server.py \
  --server "npm run dev" --port 5173 \
  -- python .k-coder/tmp/check.py
```

Multiple servers (backend + frontend) repeat `--server` and `--port` in the same
order and are all stopped at the end.

| Flag | Meaning |
| --- | --- |
| `--server CMD` | Server command to start. Repeatable, required. |
| `--port N` | Port the matching `--server` listens on. Repeatable, required, same count as `--server`. |
| `--cwd DIR` | Working directory for both the servers and the wrapped command. |
| `--timeout N` | Seconds to wait for each server to answer. Default `60`. |
| `--address A` | Loopback address to probe. Repeatable; default `127.0.0.1` and `::1`. |
| `--ready-http PATH` | Probe this HTTP path instead of a raw TCP connection. |
| `--server-log-lines N` | Server log lines printed when a server fails. Default `40`. |
| `-- CMD ...` | The command to run once every server answers. Required. |

Behavior worth relying on:

- **Readiness** is a TCP connect to loopback (or an HTTP probe when
  `--ready-http` is set). Both `127.0.0.1` and `::1` are tried so a server that
  binds only IPv6 is still detected. No remote host is ever contacted.
- **Exit codes**: `0` wrapped command succeeded, `2` usage error, `3` a server
  never became ready, otherwise the wrapped command's own code.
- **Cleanup**: every server is stopped in reverse order, including after a
  failure. Servers are started in their own process group/session and the process
  tree is killed, so `npm run dev` does not survive the run.
- **Failure output**: when a server fails, its captured log tail is printed, which
  is usually enough to diagnose a compile error or a missing dependency.

## 4. Timeouts and approval

- `run_command` defaults to 120000 ms and accepts up to 3600000 ms. Set
  `timeoutMs` above `--timeout` plus the expected duration of the check script, or
  the whole process tree is killed mid-run.
- A Python script is assessed as a writing command and normally needs approval;
  say what it will run so the request is meaningful.
- Keep server commands to the project's own tooling (`npm run dev`, `pnpm dev`,
  `python -m uvicorn ...`). Do not invent build steps to make a check pass.

## 5. Check script skeleton

Write outputs under the workspace, never to container paths such as
`/mnt/user-data/outputs` or `/tmp`.

```python
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:5173"
OUT = ".k-coder/tmp/evidence"
calls = []
messages = []
failures = []

with sync_playwright() as playwright:
    browser = playwright.chromium.launch(headless=True)
    page = browser.new_page(viewport={"width": 1440, "height": 900})
    page.on("console", lambda message: messages.append(f"[{message.type}] {message.text}"))
    page.on("requestfailed", lambda request: failures.append(request.url))

    page.goto(BASE)
    page.wait_for_load_state("networkidle")   # dynamic apps: wait before inspecting

    # Reconnaissance: screenshot and inspect the rendered state before acting.
    page.screenshot(path=f"{OUT}/01-initial.png", full_page=True)
    page.get_by_role("button", name="提交").click()
    page.wait_for_timeout(500)
    page.screenshot(path=f"{OUT}/02-after-submit.png", full_page=True)

    browser.close()

errors = [m for m in messages if m.startswith("[error]")]
print("console messages:", len(messages), "errors:", len(errors))
print("failed requests:", failures)
raise SystemExit(1 if errors or failures else 0)
```

Rules for the script: launch headless Chromium, always close the browser, use
`role=`/`text=`/stable ids rather than brittle positional selectors, wait
explicitly, and exit non-zero when an observed error should fail the run.
Console and `requestfailed` handlers exist here because Playwright provides them;
they are not available through the k-Coder browser tools.

## 6. Reporting

Report the startup command, port, exact check command, exit code, viewport,
screenshots, and any console/request failure that was observed. If a state was
never exercised, say so; do not summarize it as working.

## 7. Failure triage

| Symptom | Cause | Next step |
| --- | --- | --- |
| `python` not found | No interpreter on PATH | Try `python3` / `py -3`; otherwise report the blocker |
| `No module named playwright` | Package not installed | Ask the user; do not install silently |
| `Executable doesn't exist ... chromium` | Browsers not installed | Ask the user to run `playwright install chromium` |
| Exit code 3, `no response from 127.0.0.1, ::1` | Server crashed or binds another host/port | Read the printed log tail and the real port |
| Command approval denied | Policy stopped the script | Explain the intent or run a narrower command |
| `run_command` timeout | `timeoutMs` below the run duration | Raise `timeoutMs` (max 3600000) and retry once |
| Server log shows network access denied | Sandbox isolation for the assessed risk | Report it; do not disable isolation |
