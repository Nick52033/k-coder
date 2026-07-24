import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 1;
    const thread = { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false };
    const responses: Record<string, unknown> = {
      runtime_status: { ready: true, phase: "advanced-agent", version: "0.10.0", uptimeSeconds: 12, capabilities: ["skills", "mcp-stdio", "tool-hooks", "persistent-plans", "budgeted-goals"] },
      get_provider_config: { schemaVersion: 1, kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }, { id: "gpt-4o", displayName: "GPT-4o", contextWindow: 64000, fallback: false }], endpoints: [], hasApiKey: true },
      save_provider_config: { schemaVersion: 1, kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }, { id: "gpt-4o", displayName: "GPT-4o", contextWindow: 64000, fallback: false }], endpoints: [], hasApiKey: true },
      get_plan: null,
      get_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "完成 Phase 9 高级智能体能力", state: "active", tokenBudget: 100000, tokensUsed: 24000, timeBudgetMs: 3600000, elapsedMs: 420000, reason: null, createdAtMs: 2, updatedAtMs: 3, revision: 2 },
      transition_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "完成 Phase 9 高级智能体能力", state: "paused", tokenBudget: 100000, tokensUsed: 24000, timeBudgetMs: 3600000, elapsedMs: 420000, reason: null, createdAtMs: 2, updatedAtMs: 4, revision: 3 },
      get_memory_settings: { enabled: false },
      set_memory_enabled: { enabled: true },
      list_memories: [],
      get_browser_settings: { enabled: false, allowLocalhost: false },
      save_browser_settings: { enabled: true, allowLocalhost: false },
      list_browser_audit: [{ timestampMs: 3, action: "navigate", target: "https://example.com", success: true, detail: "ok" }],
      list_browser_artifacts: [{ id: "shot-1", name: "shot-1.png", mediaType: "image/png", sizeBytes: 2048, createdAtMs: 3 }],
      advanced_metrics: { providerCalls: 2, providerFailures: 0, averageProviderLatencyMs: 120, inputTokens: 100, outputTokens: 20, toolCalls: 2, toolSuccessRate: 1, fallbackCount: 0, completedTasks: 1, failedTasks: 0, estimatedCostUsd: null },
      run_regression_evaluation: { total: 3, passed: 3, passRate: 1, failures: [] },
      list_threads: [thread],
      read_thread: { schemaVersion: 1, summary: thread, messages: [], lastTurn: null, toolActivities: [
        { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: {}, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} } },
        { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: {}, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: {} } },
      ], approvals: [], changes: [] },
      workspace_state: { current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 }, recent: [] },
      list_workspace_directory: [
        { name: "src", path: "src", isDirectory: true, size: null, modifiedAtMs: 2 },
        { name: "README.md", path: "README.md", isDirectory: false, size: 120, modifiedAtMs: 2 },
      ],
      preview_workspace_file: { path: "README.md", name: "README.md", language: "markdown", content: "# k-Coder", dataUrl: null, size: 12, truncated: false },
      git_status: { isRepository: true, branch: "main", upstream: "origin/main", ahead: 0, behind: 0, files: [{ path: "src/App.tsx", indexStatus: " ", worktreeStatus: "M" }] },
      git_branches: { current: "main", branches: ["main", "feature/workbench"] },
      extension_overview: {
        schemaVersion: 1,
        configPaths: ["D:\\code\\k-coder\\.k-coder\\extensions.json"],
        instructions: [{ path: "D:\\code\\k-coder\\AGENTS.md", scope: "project", priority: 200, bytes: 120 }],
        skills: [{ name: "review", description: "Review code safely", path: "D:\\code\\k-coder\\.k-coder\\skills\\review\\SKILL.md", scope: "project", risk: "read", triggers: ["review"], enabled: true }],
        mcpServers: [{ id: "local", transport: "stdio", enabled: true, state: "ready", toolCount: 2, credentials: [], error: null }],
        hooks: [{ id: "guard", phase: "before", tool: "mcp__local__*", enabled: true }],
        audit: [{ timestampMs: 2, event: "extensions_ready", kind: "runtime", id: "all", success: true, detail: "extensions loaded" }],
        error: null,
      },
      list_subagents: [{
        schemaVersion: 1, id: "agent-1", parentAgentId: null, parentThreadId: "thread-1", threadId: "thread-agent-1",
        label: "检查后端", task: "分析后端接口", state: "completed", depth: 1, workspaceRoot: "D:\\code\\k-coder",
        capabilities: ["list_directory", "read_file"], tokenBudget: 64000, tokensUsed: 420, timeoutMs: 600000,
        createdAtMs: 2, updatedAtMs: 3, summary: "后端检查完成", error: null,
      }],
      create_subagent: {
        schemaVersion: 1, id: "agent-2", parentAgentId: null, parentThreadId: "thread-1", threadId: "thread-agent-2",
        label: "检查测试", task: "检查测试", state: "running", depth: 1, workspaceRoot: "D:\\code\\k-coder",
        capabilities: ["list_directory", "read_file"], tokenBudget: 64000, tokensUsed: 0, timeoutMs: 600000,
        createdAtMs: 4, updatedAtMs: 4, summary: null, error: null,
      },
      "plugin:event|listen": 1,
    };
    Object.assign(window, {
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        transformCallback: (callback: (...args: unknown[]) => void) => { const id = callbackId++; callbacks.set(id, callback); return id; },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, args?: Record<string, unknown>) => {
          (window as unknown as { __invoked: string[] }).__invoked.push(command);
          if (command === "save_provider_config") {
            (window as unknown as { __lastProviderRequest: unknown }).__lastProviderRequest = args?.request;
            return { ...responses[command] as Record<string, unknown>, ...args?.request as Record<string, unknown> };
          }
          return responses[command] ?? null;
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
      __invoked: [],
      __lastProviderRequest: null,
    });
  });
});

test("supports the primary workbench inspection flow", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await page.getByRole("button", { name: "切换工作台面板" }).click();
  await page.getByRole("button", { name: /README.md/ }).click();
  await expect(page.getByText("# k-Coder")).toBeVisible();
  await page.getByRole("tab", { name: "Git" }).click();
  await expect(page.getByLabel("当前分支")).toHaveValue("main");
  await page.getByRole("button", { name: "暂存 src/App.tsx" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("git_action"))).toBe(true);
  await page.getByRole("tab", { name: "计划" }).click();
  await expect(page.locator(".plan-list").getByText("apply_patch", { exact: true })).toBeVisible();
  await expect(page.locator(".plan-list").getByText("run_command", { exact: true })).toBeVisible();
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(page.getByLabel("供应商名称")).toHaveValue("OpenAI");
  await expect(page.locator(".provider-model-row")).toHaveCount(2);
  await expect(page.getByLabel("模型 ID 1")).toHaveValue("gpt-4.1");
  await expect(page.getByLabel("显示名称 1")).toHaveValue("GPT-4.1");
  await expect(page.getByLabel("上下文长度 1")).toHaveValue("128000");
  await expect(page.getByLabel(/设为默认模型：GPT-4.1/)).toBeChecked();
  await page.getByRole("button", { name: /Skills/ }).click();
  await expect(page.getByText("Review code safely")).toBeVisible();
  await page.getByRole("button", { name: /MCP 与 Hooks/ }).click();
  await expect(page.getByText("local", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /规则与审计/ }).click();
  await expect(page.getByText("extensions_ready", { exact: true })).toBeVisible();
});

test("shows and starts bounded subagent activity", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "切换子智能体面板" }).click();
  await expect(page.getByRole("button", { name: /检查后端/ })).toBeVisible();
  await expect(page.getByText("后端检查完成", { exact: true })).toBeVisible();
  await page.getByLabel("子任务").fill("检查测试");
  await page.getByRole("button", { name: "启动" }).click();
  await expect(page.getByRole("button", { name: /检查测试/ })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("create_subagent"))).toBe(true);
});

test("shows a bounded goal with visible budget and controls", async ({ page }, testInfo) => {
  await page.goto("/");
  const goal = page.locator(".goal-bar");
  await expect(goal).toContainText("完成 Phase 9 高级智能体能力");
  await expect(goal).toContainText("24,000 / 100,000 tokens");
  await goal.getByRole("button", { name: "暂停 Goal" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("transition_goal"))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`goal-${testInfo.project.name}.png`), fullPage: true });
});

test("selects the model from the composer footer", async ({ page }) => {
  await page.goto("/");
  const composer = page.locator(".composer");
  const selector = composer.getByRole("button", { name: "选择模型" });

  await expect(selector).toBeVisible();
  await expect(selector).toContainText("OpenAI");
  await expect(selector).toContainText("GPT-4.1");
  await expect(selector).toContainText("gpt-4.1");
  await expect(page.locator(".sidebar").getByRole("button", { name: "选择模型" })).toHaveCount(0);

  await selector.click();
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeVisible();
  await expect(page.locator(".model-selector-provider")).toContainText("OpenAI");
  await expect(page.getByRole("option")).toHaveCount(2);
  await expect(page.getByText("Deepseek-V4-Pro", { exact: true })).toHaveCount(0);
  await page.getByRole("option", { name: /GPT-4o.*gpt-4o/ }).click();
  await expect(selector).toContainText("GPT-4o");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("save_provider_config"))).toBe(true);

  await selector.press("ArrowDown");
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeHidden();
  await expect(selector).toBeFocused();
});

test("adds, edits, deletes, and saves structured provider models", async ({ page }) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();

  await page.getByRole("button", { name: "新增模型" }).click();
  await expect(page.locator(".provider-model-row")).toHaveCount(3);
  await page.getByLabel("模型 ID 3").fill("o3-mini");
  await page.getByLabel("显示名称 3").fill("O3 Mini");
  await page.getByLabel("上下文长度 3").fill("200000");
  await page.getByLabel(/设为默认模型：O3 Mini/).check();

  await page.getByRole("button", { name: "删除模型：GPT-4o" }).click();
  await expect(page.locator(".provider-model-row")).toHaveCount(2);
  await page.getByRole("button", { name: "保存配置" }).click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastProviderRequest: { model: string } | null }).__lastProviderRequest?.model)).toBe("o3-mini");
  const request = await page.evaluate(() => (window as unknown as { __lastProviderRequest: { models: unknown[] } }).__lastProviderRequest);
  expect(request.models).toEqual([
    { id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false },
    { id: "o3-mini", displayName: "O3 Mini", contextWindow: 200000, fallback: false },
  ]);
});

test("exposes opt-in memory, browser audit, and advanced metrics", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  await page.getByRole("button", { name: /^记忆/ }).click();
  await expect(page.getByRole("heading", { name: "记忆" })).toBeVisible();
  await page.getByRole("checkbox", { name: "启用" }).check();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("set_memory_enabled"))).toBe(true);

  await page.getByRole("button", { name: /浏览器自动化/ }).click();
  await expect(page.getByText("shot-1.png", { exact: true })).toBeVisible();
  await expect(page.getByText("navigate", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: /用量追踪/ }).click();
  await expect(page.getByText("120 ms", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "运行回归评估" }).click();
  await expect(page.getByText(/回归评估 3\/3/)).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath(`phase9-${testInfo.project.name}.png`), fullPage: true });
});
