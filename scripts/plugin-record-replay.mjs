#!/usr/bin/env node

import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";
import { createRecording } from "./plugin-browser-recorder.mjs";

const require = createRequire(import.meta.url);
const ownPath = fileURLToPath(import.meta.url);
const DEFAULT_TIMEOUT_MS = 30 * 60 * 1000;
const MAX_TIMEOUT_MS = 60 * 60 * 1000;
const MAX_CAPTURE_CHARS = 64_000;
const SUPPORTED_TOOLS = new Set([
  "browser_navigate",
  "browser_snapshot",
  "browser_click",
  "browser_type",
  "browser_screenshot",
  "browser_close",
]);

function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return (
    relative === "" ||
    (!path.isAbsolute(relative) && !relative.startsWith(`..${path.sep}`) && relative !== "..")
  );
}

async function rejectLinkedAncestors(root, candidate) {
  const relative = path.relative(root, candidate);
  let current = root;
  for (const part of relative.split(path.sep).filter(Boolean)) {
    current = path.join(current, part);
    try {
      const metadata = await fs.lstat(current);
      if (metadata.isSymbolicLink()) {
        throw new Error(`workspace path contains a link or junction: ${current}`);
      }
      const canonical = await fs.realpath(current);
      if (!isWithin(root, canonical)) {
        throw new Error(`workspace path escapes through a link or junction: ${current}`);
      }
    } catch (error) {
      if (error?.code === "ENOENT") return;
      throw error;
    }
  }
}

export async function resolveWorkspacePath(workspace, relativePath, options = {}) {
  if (typeof relativePath !== "string" || relativePath.trim() === "") {
    throw new Error("workspace path must be a non-empty relative path");
  }
  if (path.isAbsolute(relativePath)) {
    throw new Error("workspace path must be relative");
  }
  if (path.parse(relativePath).root !== "" || /^[a-zA-Z]:/.test(relativePath)) {
    throw new Error("workspace path must not contain a drive-qualified root");
  }
  if (relativePath.includes(":")) throw new Error("workspace path must not contain a colon or alternate data stream");
  const root = await fs.realpath(path.resolve(workspace));
  const candidate = path.resolve(root, relativePath);
  if (!isWithin(root, candidate)) {
    throw new Error("workspace path escapes the workspace");
  }
  await rejectLinkedAncestors(root, candidate);
  if (!options.allowMissing) {
    const canonical = await fs.realpath(candidate);
    if (!isWithin(root, canonical)) {
      throw new Error("workspace path escapes the workspace");
    }
  }
  return candidate;
}

function normalizeLegacyStep(step) {
  if (step.toolName !== "browser_run") return step;
  const args = step.argsTemplate ?? {};
  const action = args.action;
  if (action === "navigate") {
    return { ...step, toolName: "browser_navigate", argsTemplate: { url: args.url } };
  }
  if (action === "type") {
    return {
      ...step,
      toolName: "browser_type",
      argsTemplate: { selector: args.selector, text: args.text },
    };
  }
  if (action === "click") {
    return {
      ...step,
      toolName: "browser_click",
      argsTemplate: { selector: args.selector },
    };
  }
  throw new Error(`unsupported legacy browser_run action: ${String(action)}`);
}

function compileStep(step, index) {
  const normalized = normalizeLegacyStep(step);
  const tool = normalized.toolName;
  if (!SUPPORTED_TOOLS.has(tool)) {
    throw new Error(`unsupported browser tool at step ${index + 1}: ${String(tool)}`);
  }
  const args = normalized.argsTemplate ?? {};
  if (tool === "browser_navigate" && typeof args.url !== "string") {
    throw new Error(`browser_navigate step ${index + 1} requires url`);
  }
  if (
    (tool === "browser_click" || tool === "browser_type") &&
    typeof args.selector !== "string"
  ) {
    throw new Error(`${tool} step ${index + 1} requires selector`);
  }
  if (tool === "browser_type" && typeof args.text !== "string") {
    throw new Error(`browser_type step ${index + 1} requires text`);
  }
  return {
    id: normalized.stepId || `step_${index + 1}`,
    tool,
    args,
    verification: normalized.verification || null,
    fallback: normalized.fallback || null,
  };
}

function resolveTemplate(value, variables) {
  if (typeof value === "string") {
    return value.replace(/\{\{([a-zA-Z0-9_]+)\}\}/g, (_, name) => {
      if (!(name in variables)) throw new Error(`recorded workflow variable is unresolved: ${name}`);
      return String(variables[name]);
    });
  }
  if (Array.isArray(value)) return value.map((item) => resolveTemplate(item, variables));
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, resolveTemplate(item, variables)]),
    );
  }
  return value;
}

export async function compileWorkflow({ workspace, workflow, output, variables = {} }) {
  if (workflow?.requiresManualSensitiveInput) throw new Error("workflow skipped sensitive input; complete authentication manually and record the subsequent actions separately");
  if (workflow?.truncated) throw new Error("recording was truncated; record a shorter complete workflow");
  if (!workflow || !Array.isArray(workflow.toolGraph) || workflow.toolGraph.length === 0) {
    throw new Error("recorded workflow must contain at least one toolGraph step");
  }
  const defaults = Object.fromEntries(
    Object.entries(workflow.variables ?? {})
      .filter(([, definition]) => definition && "default" in definition)
      .map(([name, definition]) => [name, definition.default]),
  );
  const compiled = resolveTemplate({
    schemaVersion: 1,
    name: workflow.name,
    objective: workflow.objective,
    sourceTrace: workflow.sourceTrace ?? null,
    steps: workflow.toolGraph.map(compileStep),
  }, { ...defaults, ...variables });
  const outputPath = await resolveWorkspacePath(workspace, output, { allowMissing: true });
  await fs.mkdir(path.dirname(outputPath), { recursive: true });
  await rejectLinkedAncestors(await fs.realpath(path.resolve(workspace)), outputPath);
  await fs.writeFile(outputPath, `${JSON.stringify(compiled, null, 2)}\n`, "utf8");
  return outputPath;
}

function remainingTimeout(deadline) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw new Error("record/replay timed out");
  return Math.min(remaining, 30_000);
}

async function runBrowserStep(page, step, deadline, state) {
  const timeout = remainingTimeout(deadline);
  if (step.tool === "browser_navigate") {
    await page.goto(step.args.url, { waitUntil: "domcontentloaded", timeout });
  } else if (step.tool === "browser_type") {
    const target = page.locator(step.args.selector);
    await target.waitFor({ state: "visible", timeout });
    await target.fill(step.args.text, { timeout });
  } else if (step.tool === "browser_click") {
    const target = page.locator(step.args.selector);
    await target.waitFor({ state: "visible", timeout });
    await target.click({ timeout });
  } else if (step.tool === "browser_snapshot") {
    state.snapshot = await page.locator("body").innerText({ timeout });
  } else if (step.tool === "browser_screenshot") {
    const bytes = await page.screenshot({
      fullPage: Boolean(step.args.fullPage),
      timeout,
    });
    state.screenshotBytes = bytes.byteLength;
  } else if (step.tool === "browser_close") {
    return "close";
  }
  return "continue";
}

export async function replayWorkflow({
  workspace,
  workflowPath,
  reportPath,
  timeoutMs = DEFAULT_TIMEOUT_MS,
  headless = false,
}) {
  const boundedTimeout = Number(timeoutMs);
  if (!Number.isInteger(boundedTimeout) || boundedTimeout < 1000 || boundedTimeout > MAX_TIMEOUT_MS) {
    throw new Error(`timeout must be an integer from 1000 to ${MAX_TIMEOUT_MS}`);
  }
  const inputPath = await resolveWorkspacePath(workspace, workflowPath);
  const outputPath = await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
  const workflow = JSON.parse(await fs.readFile(inputPath, "utf8"));
  if (workflow.schemaVersion !== 1 || !Array.isArray(workflow.steps)) {
    throw new Error("replay workflow must use schemaVersion 1");
  }
  workflow.steps = workflow.steps.map((step, index) => compileStep({
    stepId: step.id, toolName: step.tool, argsTemplate: step.args,
  }, index));

  const startedAt = Date.now();
  const deadline = startedAt + boundedTimeout;
  const report = {
    schemaVersion: 1,
    workflow: workflow.name,
    ok: false,
    startedAt: new Date(startedAt).toISOString(),
    finishedAt: null,
    durationMs: 0,
    snapshot: "",
    screenshotBytes: 0,
    steps: [],
  };
  let browser;
  try {
    browser = await chromium.launch({ channel: "msedge", headless });
    const page = await browser.newPage();
    for (const step of workflow.steps) {
      const stepStartedAt = Date.now();
      const action = await runBrowserStep(page, step, deadline, report);
      report.steps.push({
        id: step.id,
        tool: step.tool,
        ok: true,
        durationMs: Date.now() - stepStartedAt,
      });
      if (action === "close") {
        await browser.close();
        browser = undefined;
      }
    }
    report.ok = true;
  } catch (error) {
    report.error = error instanceof Error ? error.message : String(error);
    throw error;
  } finally {
    if (browser) await browser.close().catch(() => {});
    report.finishedAt = new Date().toISOString();
    report.durationMs = Date.now() - startedAt;
    await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
    await fs.mkdir(path.dirname(outputPath), { recursive: true });
    await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
    await fs.writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  }
  return report;
}

function appendBounded(current, chunk) {
  const next = current + chunk.toString();
  return next.length <= MAX_CAPTURE_CHARS ? next : next.slice(next.length - MAX_CAPTURE_CHARS);
}

async function terminateProcessTree(child) {
  if (!child.pid || child.exitCode !== null) return;
  if (process.platform === "win32") {
    await new Promise((resolve) => {
      const killer = spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
        windowsHide: true,
        stdio: "ignore",
      });
      killer.once("exit", resolve);
      killer.once("error", resolve);
    });
  } else {
    child.kill("SIGTERM");
  }
}

async function runChildWithLifecycle(
  command,
  args,
  { cwd, timeoutMs, stdio = "pipe", signal },
) {
  if (signal?.aborted) throw new Error("process cancelled");
  const child = spawn(command, args, { cwd, shell: false, stdio, windowsHide: true });
  let stdout = "";
  let stderr = "";
  child.stdout?.on("data", (chunk) => (stdout = appendBounded(stdout, chunk)));
  child.stderr?.on("data", (chunk) => (stderr = appendBounded(stderr, chunk)));
  const stop = () => void terminateProcessTree(child);
  let rejectCancellation;
  let forcedTermination = false;
  const cancellation = new Promise((_, reject) => {
    rejectCancellation = reject;
  });
  const cancel = () => {
    forcedTermination = true;
    rejectCancellation(new Error("process cancelled"));
  };
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  signal?.addEventListener("abort", cancel, { once: true });
  let timer;
  try {
    const exit = await Promise.race([
      new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("exit", (code, signal) => resolve({ code, signal }));
      }),
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          forcedTermination = true;
          reject(new Error(`process timed out after ${timeoutMs} ms`));
        }, timeoutMs);
      }),
      cancellation,
    ]);
    return { ...exit, stdout, stderr };
  } finally {
    clearTimeout(timer);
    if (forcedTermination) await terminateProcessTree(child);
    process.off("SIGINT", stop);
    process.off("SIGTERM", stop);
    signal?.removeEventListener("abort", cancel);
  }
}

export function inspectRecordingScript(source) {
  const literalSensitiveInput =
    /(?:password|passcode|api[-_ ]?key|authorization)[\s\S]{0,240}\.(?:fill|type)\(\s*(['"`])[^'"`\r\n]+\1/i;
  if (literalSensitiveInput.test(source)) {
    throw new Error(
      "recording contains literal sensitive input near a password or credential field; replace it before replay",
    );
  }
}

export async function replayScript({
  workspace,
  scriptPath,
  reportPath,
  timeoutMs = DEFAULT_TIMEOUT_MS,
  signal,
}) {
  const boundedTimeout = Number(timeoutMs);
  if (!Number.isInteger(boundedTimeout) || boundedTimeout < 1000 || boundedTimeout > MAX_TIMEOUT_MS) {
    throw new Error(`timeout must be an integer from 1000 to ${MAX_TIMEOUT_MS}`);
  }
  const inputPath = await resolveWorkspacePath(workspace, scriptPath);
  if (!inputPath.endsWith(".cjs") && !inputPath.endsWith(".mjs")) {
    throw new Error("recording script must use a .cjs or .mjs extension");
  }
  inspectRecordingScript(await fs.readFile(inputPath, "utf8"));
  const outputPath = await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
  const startedAt = Date.now();
  let result;
  try {
    result = await runChildWithLifecycle(process.execPath, [inputPath], {
      cwd: workspace,
      timeoutMs: boundedTimeout,
      signal,
    });
  } catch (error) {
    result = {
      code: null,
      signal: null,
      stdout: "",
      stderr: error instanceof Error ? error.message : String(error),
    };
  }
  const report = {
    schemaVersion: 1,
    script: path.relative(workspace, inputPath),
    ok: result.code === 0,
    exitCode: result.code,
    signal: result.signal,
    stdout: result.stdout,
    stderr: result.stderr,
    startedAt: new Date(startedAt).toISOString(),
    finishedAt: new Date().toISOString(),
    durationMs: Date.now() - startedAt,
  };
  await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
  await fs.mkdir(path.dirname(outputPath), { recursive: true });
  await resolveWorkspacePath(workspace, reportPath, { allowMissing: true });
  await fs.writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  if (!report.ok) {
    const error = new Error(`recording script failed; report: ${path.relative(workspace, outputPath)}`);
    error.report = report;
    throw error;
  }
  return report;
}

function parseArgs(argv) {
  const [command, ...tokens] = argv;
  const options = {};
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (!token.startsWith("--")) throw new Error(`unexpected argument: ${token}`);
    const name = token.slice(2);
    if (name === "headed" || name === "headless") {
      options[name] = true;
    } else {
      const value = tokens[index + 1];
      if (!value || value.startsWith("--")) throw new Error(`missing value for --${name}`);
      options[name] = value;
      index += 1;
    }
  }
  return { command, options };
}

async function readJsonWithin(workspace, relativePath) {
  const input = await resolveWorkspacePath(workspace, relativePath);
  return JSON.parse(await fs.readFile(input, "utf8"));
}

export async function record({ workspace, output, url, timeoutMs = DEFAULT_TIMEOUT_MS, headless = false, signal }) {
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > MAX_TIMEOUT_MS) throw new Error("invalid recording timeout");
  if (!/^https?:\/\//i.test(url)) throw new Error("record requires an HTTP(S) URL");
  if (signal?.aborted) throw new Error("recording cancelled");
  const outputPath = await resolveWorkspacePath(workspace, output, { allowMissing: true });
  let browser, recording, timer, stopReason;
  const stop = () => { stopReason = "recording cancelled"; void browser?.close(); };
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  signal?.addEventListener("abort", stop, { once: true });
  const deadline = Date.now() + timeoutMs;
  try {
    browser = await chromium.launch({ channel: "msedge", headless, timeout: timeoutMs });
    const disconnected = new Promise(resolve => browser.once("disconnected", resolve));
    timer = setTimeout(() => { stopReason = "recording timed out"; void browser.close(); }, Math.max(1, deadline - Date.now()));
    if (stopReason || signal?.aborted) throw new Error("recording cancelled");
    const context = await browser.newContext();
    recording = await createRecording(context);
    const page = await context.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: remainingTimeout(deadline) });
    await disconnected;
    if (stopReason) throw new Error(stopReason);
  } catch (error) {
    stopReason ||= "recording interrupted";
    throw error;
  } finally {
    clearTimeout(timer);
    await browser?.close().catch(() => {});
    process.off("SIGINT", stop);
    process.off("SIGTERM", stop);
    signal?.removeEventListener("abort", stop);
    if (recording) {
      const snapshot = recording.snapshot();
      if (stopReason) snapshot.truncated = true;
      await resolveWorkspacePath(workspace, output, { allowMissing: true });
      await fs.mkdir(path.dirname(outputPath), { recursive: true });
      await resolveWorkspacePath(workspace, output, { allowMissing: true });
      await fs.writeFile(outputPath, `${JSON.stringify(snapshot, null, 2)}\n`, "utf8");
    }
  }
  return { ok: true, output: path.relative(workspace, outputPath) };
}

async function main(argv) {
  const { command, options } = parseArgs(argv);
  const workspace = await fs.realpath(path.resolve(options.workspace ?? "."));
  const timeoutMs = Number(options["timeout-ms"] ?? DEFAULT_TIMEOUT_MS);
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > MAX_TIMEOUT_MS) {
    throw new Error(`timeout must be an integer from 1000 to ${MAX_TIMEOUT_MS}`);
  }
  if (command === "compile") {
    const workflow = await readJsonWithin(workspace, options.workflow);
    const variables = options.variables ? await readJsonWithin(workspace, options.variables) : {};
    const outputPath = await compileWorkflow({ workspace, workflow, output: options.output, variables });
    process.stdout.write(`${JSON.stringify({ ok: true, output: path.relative(workspace, outputPath) })}\n`);
  } else if (command === "replay") {
    const report = await replayWorkflow({
      workspace,
      workflowPath: options.workflow,
      reportPath: options.report,
      timeoutMs,
      headless: !options.headed,
    });
    process.stdout.write(`${JSON.stringify(report)}\n`);
  } else if (command === "replay-script") {
    const report = await replayScript({
      workspace,
      scriptPath: options.script,
      reportPath: options.report,
      timeoutMs,
    });
    process.stdout.write(`${JSON.stringify(report)}\n`);
  } else if (command === "record") {
    if (!options.url || !/^https?:\/\//i.test(options.url)) {
      throw new Error("record requires an HTTP(S) --url");
    }
    const result = await record({
      workspace,
      output: options.output,
      url: options.url,
      timeoutMs,
      headless: Boolean(options.headless),
    });
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } else if (command === "doctor") {
    const edgeCandidates = [
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
      "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
    ];
    const edge = (await Promise.all(edgeCandidates.map(async (value) => {
      try { await fs.access(value); return value; } catch { return null; }
    }))).find(Boolean);
    process.stdout.write(`${JSON.stringify({ ok: Boolean(edge), edge, playwrightModule: require.resolve("@playwright/test") })}\n`);
    if (!edge) process.exitCode = 1;
  } else {
    throw new Error("usage: plugin-record-replay.mjs <doctor|record|compile|replay|replay-script> [options]");
  }
}

if (path.resolve(process.argv[1] ?? "") === path.resolve(ownPath)) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
