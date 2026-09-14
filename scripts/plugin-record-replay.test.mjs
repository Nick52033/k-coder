import assert from "node:assert/strict";
import fs from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import test from "node:test";
import { randomUUID } from "node:crypto";

import {
  compileWorkflow,
  inspectRecordingScript,
  replayScript,
  replayWorkflow,
  record,
  resolveWorkspacePath,
} from "./plugin-record-replay.mjs";

const workspace = path.resolve(import.meta.dirname, "..");

test('recording timeout saves only incomplete parameterized structure and closes Edge', async () => {
  const relative = `.tmp/plugin-record-replay-${randomUUID()}`;
  await withFixture(async url => {
    try {
      await assert.rejects(record({ workspace, output: `${relative}/capture.json`, url: `${url}/?token=synthetic-secret`, timeoutMs: 2000, headless: true }), /timed out/i);
      const contents = await fs.readFile(path.join(workspace, relative, 'capture.json'), 'utf8');
      const captured = JSON.parse(contents);
      assert.equal(captured.truncated, true);
      assert(!contents.includes('synthetic-secret'));
      assert.equal(captured.toolGraph[0].argsTemplate.url, '{{url_1}}');
      await assert.rejects(compileWorkflow({workspace, workflow: captured, output: `${relative}/replay.json`}), /truncated/i);
    } finally { await fs.rm(path.join(workspace, relative), { recursive: true, force: true }); }
  });
});

test("records actual Edge actions without persisting input values or URL tokens", async () => {
  const { createRecording } = await import('./plugin-browser-recorder.mjs');
  const { chromium } = await import('@playwright/test');
  await withFixture(async url => {
    const browser = await chromium.launch({ channel: 'msedge', headless: true });
    try {
      const context = await browser.newContext();
      const recording = await createRecording(context);
      const page = await context.newPage();
      await page.goto(`${url}/?token=synthetic-url-secret`);
      await page.locator('#query').fill('synthetic-input-secret');
      await page.evaluate(() => {
        const input = document.createElement('input'); input.id = 'password'; input.type = 'password'; document.body.append(input);
      });
      await page.locator('#password').fill('synthetic-password-secret');
      await page.locator('#submit').click();
      await page.waitForFunction(() => document.querySelector('#result').textContent.includes('synthetic-input-secret'));
      const result = recording.snapshot();
      assert.deepEqual(result.toolGraph.map(step => step.toolName), ['browser_navigate', 'browser_type', 'browser_click']);
      assert.equal(result.toolGraph[1].argsTemplate.text, '{{input_1}}');
      assert.equal(result.requiresManualSensitiveInput, true);
      const serialized = JSON.stringify(result);
      assert(!serialized.includes('synthetic-input-secret'));
      assert(!serialized.includes('synthetic-password-secret'));
      assert(!serialized.includes('synthetic-url-secret'));
    } finally { await browser.close(); }
  });
});

async function withFixture(run) {
  const server = http.createServer((request, response) => {
    response.setHeader("content-type", "text/html; charset=utf-8");
    response.end(`<!doctype html>
      <html><body>
        <label for="query">Query</label>
        <input id="query" />
        <button id="submit" type="button">Submit</button>
        <p id="result">Waiting</p>
        <script>
          document.querySelector('#submit').addEventListener('click', () => {
            document.querySelector('#result').textContent =
              'Submitted: ' + document.querySelector('#query').value;
          });
        </script>
      </body></html>`);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  try {
    await run(`http://127.0.0.1:${address.port}`);
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

test("compiles existing browser tools and replays them in system Edge", async () => {
  await withFixture(async (url) => {
    const relativeRoot = `.tmp/plugin-record-replay-${randomUUID()}`;
    const root = path.join(workspace, relativeRoot);
    await fs.mkdir(root, { recursive: true });
    const workflow = {
      name: "loopback-form",
      objective: "Fill and submit the loopback form",
      toolGraph: [
        { stepId: "navigate", toolName: "browser_navigate", argsTemplate: { url } },
        {
          stepId: "type",
          toolName: "browser_type",
          argsTemplate: { selector: "#query", text: "k-coder" },
        },
        {
          stepId: "click",
          toolName: "browser_click",
          argsTemplate: { selector: "#submit" },
        },
        { stepId: "snapshot", toolName: "browser_snapshot", argsTemplate: {} },
        {
          stepId: "screenshot",
          toolName: "browser_screenshot",
          argsTemplate: { fullPage: false },
        },
        { stepId: "close", toolName: "browser_close", argsTemplate: {} },
      ],
    };

    try {
      const compiledPath = await compileWorkflow({
        workspace,
        workflow,
        output: `${relativeRoot}/loopback.replay.json`,
      });
      const compiled = JSON.parse(await fs.readFile(compiledPath, "utf8"));
      assert.deepEqual(
        compiled.steps.map((step) => step.tool),
        [
          "browser_navigate",
          "browser_type",
          "browser_click",
          "browser_snapshot",
          "browser_screenshot",
          "browser_close",
        ],
      );

      const reportPath = `${relativeRoot}/loopback.report.json`;
      const report = await replayWorkflow({
        workspace,
        workflowPath: path.relative(workspace, compiledPath),
        reportPath,
        timeoutMs: 20_000,
        headless: true,
      });
      assert.equal(report.ok, true);
      assert.match(report.snapshot, /Submitted: k-coder/);
      assert.equal(report.steps.length, 6);
      assert.ok(report.screenshotBytes > 0);
      assert.equal(
        JSON.parse(await fs.readFile(path.join(workspace, reportPath), "utf8")).ok,
        true,
      );
    } finally {
      await fs.rm(root, { recursive: true, force: true });
    }
  });
});

test("workspace paths reject parent traversal and directory junction escapes", async () => {
  await assert.rejects(resolveWorkspacePath(workspace, 'recording.json:secret', { allowMissing: true }), /stream|colon|workspace/i);
  await assert.rejects(
    resolveWorkspacePath(workspace, "../outside.json", { allowMissing: true }),
    /workspace/i,
  );
  await assert.rejects(
    resolveWorkspacePath(workspace, "C:escape.json", { allowMissing: true }),
    /workspace|relative/i,
  );

  const relativeRoot = `.tmp/plugin-record-replay-${randomUUID()}`;
  const root = path.join(workspace, relativeRoot);
  const outside = path.join(path.dirname(workspace), `plugin-record-replay-${randomUUID()}`);
  const junction = path.join(root, "junction");
  await fs.mkdir(root, { recursive: true });
  await fs.mkdir(outside, { recursive: true });
  try {
    await fs.symlink(outside, junction, "junction");
    await assert.rejects(
      resolveWorkspacePath(workspace, `${relativeRoot}/junction/escape.json`, {
        allowMissing: true,
      }),
      /link|workspace/i,
    );
  } finally {
    await fs.rm(junction, { recursive: true, force: true });
    await fs.rm(root, { recursive: true, force: true });
    await fs.rm(outside, { recursive: true, force: true });
  }
});

test('refuses workflows that skipped sensitive inputs or contain unknown replay actions', async () => {
  await assert.rejects(compileWorkflow({ workspace, output: '.tmp/unused.json', workflow: { requiresManualSensitiveInput: true, toolGraph: [{ toolName: 'browser_click', argsTemplate: { selector: '#submit' } }] } }), /sensitive|manual/i);
  const relative = `.tmp/plugin-record-replay-${randomUUID()}`;
  await fs.mkdir(path.join(workspace, relative), { recursive: true });
  try {
    await fs.writeFile(path.join(workspace, relative, 'bad.json'), JSON.stringify({schemaVersion: 1, steps: [{tool: 'invented', args: {}}]}));
    await assert.rejects(replayWorkflow({workspace, workflowPath: `${relative}/bad.json`, reportPath: `${relative}/report.json`, headless: true}), /unsupported/i);
  } finally { await fs.rm(path.join(workspace, relative), {recursive: true, force: true}); }
});

test("recording inspection rejects literal password input", () => {
  assert.throws(
    () =>
      inspectRecordingScript(`
        await page.locator('input[type="password"]').fill('secret123');
      `),
    /sensitive|password/i,
  );
  assert.doesNotThrow(() =>
    inspectRecordingScript(`await page.locator('#query').fill('weather');`),
  );
});

test('imported workflows normalize tools, resolve supplied variables and refuse login replay', async () => {
  const relative = `.tmp/plugin-record-replay-${randomUUID()}`;
  const readWorkflow = async name => JSON.parse(await fs.readFile(path.join(workspace, '.k-coder/plugins/record-replay/workflows', `${name}.workflow.json`), 'utf8'));
  try {
    const baidu = await readWorkflow('百度搜索关键词');
    const output = await compileWorkflow({workspace, workflow: baidu, variables: {keyword: 'plugin validation'}, output: `${relative}/baidu.json`});
    assert.equal(JSON.parse(await fs.readFile(output, 'utf8')).steps[1].args.text, 'plugin validation');
    const multi = await readWorkflow('complete-a-multi-step-workflow');
    const legacy = await compileWorkflow({workspace, workflow: multi, output: `${relative}/multi.json`});
    const steps = JSON.parse(await fs.readFile(legacy, 'utf8')).steps;
    assert.equal(steps[0].tool, 'browser_navigate');
    assert(!steps.some(step => step.tool === 'browser_run'));
    await assert.rejects(compileWorkflow({workspace, workflow: await readWorkflow('login-to-application'), output: `${relative}/login.json`}), /sensitive|manual/i);
  } finally { await fs.rm(path.join(workspace, relative), { recursive: true, force: true }); }
});

test("script replay stops on timeout and cancellation and writes reports", async () => {
  const relativeRoot = `.tmp/plugin-record-replay-${randomUUID()}`;
  const root = path.join(workspace, relativeRoot);
  await fs.mkdir(root, { recursive: true });
  await fs.writeFile(path.join(root, "hang.cjs"), "setInterval(() => {}, 1000);\n", "utf8");
  try {
    await assert.rejects(
      replayScript({
        workspace,
        scriptPath: `${relativeRoot}/hang.cjs`,
        reportPath: `${relativeRoot}/timeout.report.json`,
        timeoutMs: 1000,
      }),
      /failed/i,
    );
    assert.match(
      JSON.parse(await fs.readFile(path.join(root, "timeout.report.json"), "utf8")).stderr,
      /timed out/i,
    );

    const controller = new AbortController();
    setTimeout(() => controller.abort(), 100);
    await assert.rejects(
      replayScript({
        workspace,
        scriptPath: `${relativeRoot}/hang.cjs`,
        reportPath: `${relativeRoot}/cancel.report.json`,
        timeoutMs: 20_000,
        signal: controller.signal,
      }),
      /failed/i,
    );
    assert.match(
      JSON.parse(await fs.readFile(path.join(root, "cancel.report.json"), "utf8")).stderr,
      /cancelled/i,
    );
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("replays a codegen-style script and writes its process report", async () => {
  await withFixture(async (url) => {
    const relativeRoot = `.tmp/plugin-record-replay-${randomUUID()}`;
    const root = path.join(workspace, relativeRoot);
    await fs.mkdir(root, { recursive: true });
    const scriptPath = path.join(root, "recording.cjs");
    await fs.writeFile(
      scriptPath,
      `const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ channel: 'msedge', headless: true });
  const page = await browser.newPage();
  await page.goto(${JSON.stringify(url)});
  await page.locator('#query').fill('recorded');
  await page.locator('#submit').click();
  console.log(await page.locator('#result').innerText());
  await browser.close();
})().catch((error) => { console.error(error); process.exit(1); });
`,
      "utf8",
    );
    try {
      const reportPath = `${relativeRoot}/script.report.json`;
      const report = await replayScript({
        workspace,
        scriptPath: `${relativeRoot}/recording.cjs`,
        reportPath,
        timeoutMs: 20_000,
      });
      assert.equal(report.ok, true);
      assert.match(report.stdout, /Submitted: recorded/);
      assert.equal(
        JSON.parse(await fs.readFile(path.join(workspace, reportPath), "utf8")).exitCode,
        0,
      );
    } finally {
      await fs.rm(root, { recursive: true, force: true });
    }
  });
});
