import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 1;
    let agentEventCallbackId: number | null = null;
    const thread = { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false };
    const openAiProvider = { schemaVersion: 1, id: "openai", kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }, { id: "gpt-4o", displayName: "GPT-4 Omni", contextWindow: 64000, fallback: false }], endpoints: [], hasApiKey: true };
    const ziccProvider = { schemaVersion: 1, id: "zicc", kind: "open_ai_compatible", transport: "open_ai_responses", name: "zicc", baseUrl: "https://zicc.example.com/v1", model: "gpt-5.6-terra", models: [{ id: "gpt-5.6-terra", displayName: "gpt-5.6-terra", contextWindow: 128000, fallback: false }, { id: "gpt-5.5", displayName: "gpt-5.5", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: true };
    const pendingProvider = { schemaVersion: 1, id: "pending", kind: "open_ai_compatible", transport: "anthropic_messages", name: "待配置供应商", baseUrl: "https://pending.example.com/v1", model: "claude-test", models: [{ id: "claude-test", displayName: "Claude Test", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: false };
    let providerCatalog: { schemaVersion: number; activeProviderId: string | null; providers: Array<typeof openAiProvider> } = { schemaVersion: 1, activeProviderId: "openai", providers: [openAiProvider, ziccProvider, pendingProvider] };
    let approvalMode: "ask" | "full_access" = "ask";
    let reasoningEffort: "off" | "minimal" | "low" | "medium" | "high" | "x_high" = "medium";
    const runTurnCalls: unknown[] = [];
    const runTurnResolvers: Array<(value: unknown) => void> = [];
    const invocationArgs: Record<string, unknown> = {};
    const responses: Record<string, unknown> = {
      runtime_status: { ready: true, phase: "advanced-agent", version: "0.10.0", uptimeSeconds: 12, capabilities: ["skills", "mcp-stdio", "tool-hooks", "persistent-plans", "budgeted-goals"] },
      get_approval_mode: "ask",
      get_reasoning_effort: "medium",
      test_provider_connection: { connected: true, latencyMs: 42, usage: null },
      get_plan: { schemaVersion: 1, threadId: "thread-1", revision: 2, updatedAtMs: 3, steps: [
        { id: "step-1", step: "检查工作区", status: "completed", detail: "已读取关键文件" },
        { id: "step-2", step: "验证实现", status: "in_progress", detail: "正在运行测试" },
      ] },
      get_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "完成 Phase 9 高级智能体能力", state: "active", tokenBudget: null, tokensUsed: 24000, timeBudgetMs: 3600000, elapsedMs: 420000, reason: null, createdAtMs: 2, updatedAtMs: 3, revision: 2 },
      transition_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "完成 Phase 9 高级智能体能力", state: "paused", tokenBudget: null, tokensUsed: 24000, timeBudgetMs: 3600000, elapsedMs: 420000, reason: null, createdAtMs: 2, updatedAtMs: 4, revision: 3 },
      get_memory_settings: { enabled: false },
      set_memory_enabled: { enabled: true },
      list_memories: [],
      get_browser_settings: { enabled: false, allowLocalhost: false },
      save_browser_settings: { enabled: true, allowLocalhost: false },
      list_browser_audit: [{ timestampMs: 3, action: "navigate", target: "https://example.com", success: true, detail: "ok" }],
      list_browser_artifacts: [{ id: "shot-1", name: "shot-1.png", mediaType: "image/png", sizeBytes: 2048, createdAtMs: 3 }],
      advanced_metrics: { providerCalls: 2, providerFailures: 0, averageProviderLatencyMs: 120, inputTokens: 100, outputTokens: 20, toolCalls: 2, toolSuccessRate: 1, fallbackCount: 0, completedTasks: 1, failedTasks: 0, estimatedCostUsd: null },
      run_regression_evaluation: { total: 3, passed: 3, passRate: 1, failures: [] },
      cancel_turn: true,
      retry_turn: { schemaVersion: 1, threadId: "thread-1", turnId: "turn-retry", state: "completed", error: null },
      recognize_image: { text: "hidden OCR fixture", lineCount: 1, durationMs: 12 },
      list_threads: [thread],
      read_thread: { schemaVersion: 1, summary: thread, messages: [
        { schemaVersion: 1, id: "message-user", role: "user", content: [{ type: "text", text: "检查工作区" }], createdAtMs: 1 },
        { schemaVersion: 1, id: "message-assistant", role: "assistant", content: [{ type: "text", text: "检查完成。" }], createdAtMs: 2 },
      ], messageTurnIds: { "message-assistant": "turn-1" }, lastTurn: null, toolActivities: [
        { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1000, completedAtMs: 1200, durationMs: 200 },
        { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 },
        { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { program: "pnpm", args: ["build"], cwd: "D:\\code\\k-coder", timeoutMs: 120000 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530 } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 },
      ], turnTimeline: [
        { type: "text", id: "progress-1", turnId: "turn-1", text: "我先检查相关文件并修改实现。" },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1000, completedAtMs: 1200, durationMs: 200 } },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 } },
        { type: "text", id: "progress-2", turnId: "turn-1", text: "修改完成，接着运行验证。" },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { program: "pnpm", args: ["build"], cwd: "D:\\code\\k-coder", timeoutMs: 120000 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530 } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 } },
        { type: "text", id: "message-assistant", turnId: "turn-1", text: "检查完成。" },
        { type: "event", itemId: "turn-completed-turn-1", turnId: "turn-1", kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 1830 },
      ], approvals: [], changes: [] },
      workspace_state: { current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 }, recent: [] },
      list_workspace_directory: [
        { name: "src", path: "src", isDirectory: true, size: null, modifiedAtMs: 2 },
        { name: "README.md", path: "README.md", isDirectory: false, size: 120, modifiedAtMs: 2 },
      ],
      preview_workspace_file: { path: "README.md", name: "README.md", language: "markdown", content: "# k-Coder", dataUrl: null, size: 9, truncated: false, editable: true, contentHash: "hash-readme" },
      save_workspace_file: { path: "README.md", name: "README.md", language: "markdown", content: "# k-Coder\n\nEdited", dataUrl: null, size: 17, truncated: false, editable: true, contentHash: "hash-edited" },
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
        capabilities: ["list_directory", "read_file"], tokenBudget: null, tokensUsed: 420, timeoutMs: 600000,
        createdAtMs: 2, updatedAtMs: 3, summary: "后端检查完成", error: null,
      }],
      create_subagent: {
        schemaVersion: 1, id: "agent-2", parentAgentId: null, parentThreadId: "thread-1", threadId: "thread-agent-2",
        label: "检查测试", task: "检查测试", state: "running", depth: 1, workspaceRoot: "D:\\code\\k-coder",
        capabilities: ["list_directory", "read_file"], tokenBudget: null, tokensUsed: 0, timeoutMs: 600000,
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
          invocationArgs[command] = args ?? {};
          if (command === "plugin:event|listen") {
            if (args?.event === "agent-event" && typeof args.handler === "number") {
              agentEventCallbackId = args.handler;
            }
            return 1;
          }
          if (command === "get_provider_catalog") return providerCatalog;
          if (command === "run_turn") {
            runTurnCalls.push(args ?? null);
            return new Promise((resolve) => runTurnResolvers.push(resolve));
          }
          if (command === "cancel_turn") {
            runTurnResolvers.shift()?.({ schemaVersion: 1, threadId: "thread-1", turnId: "turn-queued", state: "cancelled", error: null });
            return true;
          }
          if (command === "retry_turn" && localStorage.getItem("kcoder_e2e_hold_retry") === "true") {
            return new Promise(() => undefined);
          }
          if (command === "read_thread") {
            const delayMs = Number(localStorage.getItem("kcoder_e2e_read_delay_ms") ?? 0);
            if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
            const recovered = localStorage.getItem("kcoder_e2e_thread_detail");
            if (recovered) return JSON.parse(recovered);
          }
          if (command === "get_provider_config") {
            return providerCatalog.providers.find((provider) => provider.id === providerCatalog.activeProviderId) ?? null;
          }
          if (command === "save_provider_config") {
            (window as unknown as { __lastProviderRequest: unknown }).__lastProviderRequest = args?.request;
            const request = args?.request as Record<string, unknown>;
            const providerId = request.id as string;
            const existing = providerCatalog.providers.find((provider) => provider.id === providerId);
            const { apiKey, activate, ...publicConfig } = request;
            const saved = {
              schemaVersion: 1,
              ...publicConfig,
              hasApiKey: Boolean(apiKey) || existing?.hasApiKey || false,
            } as typeof openAiProvider;
            providerCatalog = {
              ...providerCatalog,
              activeProviderId: activate ? providerId : providerCatalog.activeProviderId,
              providers: existing
                ? providerCatalog.providers.map((provider) => provider.id === providerId ? saved : provider)
                : [...providerCatalog.providers, saved],
            };
            return saved;
          }
          if (command === "activate_provider") {
            const providerId = args?.providerId as string;
            (window as unknown as { __lastActivatedProvider: string | null }).__lastActivatedProvider = providerId;
            providerCatalog = { ...providerCatalog, activeProviderId: providerId };
            return providerCatalog;
          }
          if (command === "delete_provider") {
            const providerId = args?.providerId as string;
            const providers = providerCatalog.providers.filter((provider) => provider.id !== providerId);
            providerCatalog = {
              ...providerCatalog,
              providers,
              activeProviderId: providerCatalog.activeProviderId === providerId ? providers[0]?.id ?? null : providerCatalog.activeProviderId,
            };
            return providerCatalog;
          }
          if (command === "get_approval_mode") return approvalMode;
          if (command === "set_approval_mode") {
            approvalMode = args?.mode as "ask" | "full_access";
            (window as unknown as { __lastApprovalMode: string | null }).__lastApprovalMode = approvalMode;
            return approvalMode;
          }
          if (command === "get_reasoning_effort") return reasoningEffort;
          if (command === "set_reasoning_effort") {
            reasoningEffort = args?.effort as typeof reasoningEffort;
            return reasoningEffort;
          }
          return responses[command] ?? null;
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
      __invoked: [],
      __invocationArgs: invocationArgs,
      __runTurnCalls: runTurnCalls,
      __lastProviderRequest: null,
      __lastActivatedProvider: null,
      __lastApprovalMode: null,
      __emitAgentEvent: (event: unknown) => {
        if (agentEventCallbackId === null) throw new Error("agent-event listener is not ready");
        callbacks.get(agentEventCallbackId)?.({ event: "agent-event", id: 1, payload: event });
      },
    });
  });
});

test("supports the primary workbench inspection flow", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_plan)).toEqual({ threadId: "thread-1" });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_goal)).toEqual({ threadId: "thread-1" });
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await expect(page.getByText("检查完成。", { exact: true })).toBeVisible();
  await expect(page.getByText("执行了 1.8s", { exact: true })).toBeVisible();
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeHidden();
  await page.screenshot({ path: testInfo.outputPath("collapsed-turn.png"), fullPage: true });
  await page.getByText("执行了 1.8s", { exact: true }).click();
  await expect(page.locator(".turn-plan").getByText("检查工作区", { exact: true })).toBeVisible();
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeVisible();
  await expect(page.locator(".turn-timeline-tool").getByText("应用补丁 src/App.css", { exact: true })).toBeVisible();
  await page.getByText("查看补丁", { exact: true }).click();
  await expect(page.locator(".turn-command-details pre").filter({ hasText: "*** Update File: src/App.css" })).toBeVisible();
  const readTool = page.locator(".turn-timeline-tool").filter({ hasText: "读取 src/stores/workbenchStore.ts L42" });
  await expect(readTool).toBeVisible();
  await expect(readTool.getByText("查看读取内容", { exact: true })).toHaveCount(0);
  await expect(readTool.locator(".turn-tool-details")).toHaveCount(0);
  await expect(page.locator(".turn-file-editor")).toHaveCount(0);
  await expect(page.locator(".turn-command-details pre").filter({ hasText: "export const fixture = true;" })).toHaveCount(0);
  await expect(page.getByText("修改完成，接着运行验证。", { exact: true })).toBeVisible();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeVisible();
  await expect(page.getByText("3 个操作", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("inline-plan-and-tools.png"), fullPage: true });
  await page.evaluate(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.reload();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeHidden();
  await page.getByText("执行了 1.8s", { exact: true }).click();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("inline-plan-and-tools-dark.png"), fullPage: true });
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  const readmeRow = page.getByRole("button", { name: /README.md/ });
  await expect(readmeRow).toHaveCSS("font-size", "12px");
  await readmeRow.click();
  await expect(readmeRow).toHaveAttribute("aria-current", "true");
  const editor = page.locator(".monaco-editor");
  await expect(editor).toBeVisible();
  await expect.poll(async () => {
    const box = await page.locator(".code-editor").boundingBox();
    return box ? { width: Math.round(box.width), height: Math.round(box.height) } : null;
  }).toMatchObject({ width: expect.any(Number), height: expect.any(Number) });
  const editorBox = await page.locator(".code-editor").boundingBox();
  expect(editorBox?.width ?? 0).toBeGreaterThan(340);
  expect(editorBox?.height ?? 0).toBeGreaterThan(380);
  await expect(editor.locator(".line-numbers").first()).toHaveText("1");
  await expect(editor.locator(".view-lines")).toContainText("# k-Coder");
  await expect(editor.locator('[class*="mtk"]')).not.toHaveCount(0);
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText("# k-Coder\n\nEdited");
  const saveButton = page.getByRole("button", { name: "保存", exact: true });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();
  await expect(page.getByText("已保存", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.save_workspace_file)).toEqual({
    request: { path: "README.md", content: "# k-Coder\n\nEdited", expectedHash: "hash-readme" },
  });
  await page.screenshot({ path: testInfo.outputPath("workspace-code-editor.png"), fullPage: true });
  await page.getByRole("button", { name: "最大化编辑器" }).click();
  await expect(page.getByPlaceholder("搜索仓库")).toBeHidden();
  await expect.poll(async () => (await page.locator(".code-editor").boundingBox())?.height ?? 0).toBeGreaterThan(600);
  await page.screenshot({ path: testInfo.outputPath("workspace-code-editor-maximized.png"), fullPage: true });
  await page.getByRole("tab", { name: "Git" }).click();
  await expect(page.getByLabel("当前分支")).toHaveValue("main");
  await page.getByRole("button", { name: "暂存 src/App.tsx" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("git_action"))).toBe(true);
  await expect(page.getByRole("tab", { name: "计划" })).toHaveCount(0);
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(page.locator(".provider-list-item")).toHaveCount(3);
  await expect(page.getByLabel("供应商名称")).toHaveValue("OpenAI");
  await expect(page.locator(".provider-model-card")).toHaveCount(2);
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

test("supports a light CodeBuddy appearance", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem("kcoder_skin", "codebuddy");
    localStorage.setItem("kcoder_theme", "light");
  });
  await page.reload();

  await expect(page.locator("html")).toHaveAttribute("data-skin", "codebuddy");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect.poll(() => page.evaluate(() => {
    const styles = getComputedStyle(document.documentElement);
    return {
      surface: styles.getPropertyValue("--color-surface").trim(),
      background: styles.backgroundColor,
      colorScheme: styles.colorScheme,
    };
  })).toEqual({ surface: "#f8fafc", background: "rgb(242, 245, 250)", colorScheme: "light" });
  await expect(page.getByRole("button", { name: "切换到深色模式" })).toBeVisible();

  await page.getByRole("button", { name: "切换到深色模式" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--color-surface").trim())).toBe("#1B2030");

  await page.getByRole("button", { name: "切换到浅色模式" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--color-surface").trim())).toBe("#f8fafc");
});

test("keeps the mid-width workbench bounded without toolbar overflow", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1097, height: 820 });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await page.getByRole("button", { name: "工作台", exact: true }).click();

  const panel = page.locator(".workbench-panel");
  const conversation = page.locator(".conversation");
  const toolbar = panel.locator(".panel-toolbar");
  const workspaceButton = toolbar.locator(".workspace-current");
  await expect(panel).toBeVisible();
  await expect(workspaceButton).toContainText("k-coder", { ignoreCase: true });
  await expect.poll(() => panel.evaluate((element) => element.getBoundingClientRect().width)).toBeLessThanOrEqual(400);

  const panelBox = await panel.evaluate((element) => { const rect = element.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; });
  const conversationBox = await conversation.evaluate((element) => { const rect = element.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; });
  const toolbarBox = await toolbar.boundingBox();
  const workspaceBox = await workspaceButton.boundingBox();
  expect(panelBox?.width ?? 0).toBeLessThanOrEqual(400);
  expect(panelBox?.width ?? 0).toBeLessThan(1097 * 0.45);
  expect((conversationBox?.x ?? 0) + (conversationBox?.width ?? 0)).toBeLessThanOrEqual(panelBox?.x ?? 0);
  expect(workspaceBox?.x ?? 0).toBeGreaterThanOrEqual(toolbarBox?.x ?? 0);
  expect(workspaceBox?.y ?? 0).toBeGreaterThanOrEqual(toolbarBox?.y ?? 0);
  expect((workspaceBox?.y ?? 0) + (workspaceBox?.height ?? 0)).toBeLessThanOrEqual((toolbarBox?.y ?? 0) + (toolbarBox?.height ?? 0));
  await page.screenshot({ path: testInfo.outputPath("mid-width-workbench.png"), fullPage: true });
});

test("keeps messages, composer, and send action inside the conversation when the workbench opens", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1536, height: 900 });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await page.getByRole("button", { name: "工作台", exact: true }).click();

  const panel = page.locator(".workbench-panel");
  const conversation = page.locator(".conversation");
  const messageArea = page.locator(".message-area");
  const userMessage = page.locator(".message--user").first();
  const composer = page.locator(".composer");
  const sendButton = page.getByRole("button", { name: "发送消息" });
  await expect(panel).toBeVisible();
  await expect(composer).toBeVisible();
  await expect(sendButton).toBeVisible();

  const panelBox = await panel.boundingBox();
  const conversationBox = await conversation.boundingBox();
  const messageBox = await userMessage.boundingBox();
  const composerBox = await composer.boundingBox();
  const sendBox = await sendButton.boundingBox();
  expect(panelBox).not.toBeNull();
  expect(conversationBox).not.toBeNull();
  expect(messageBox).not.toBeNull();
  expect(composerBox).not.toBeNull();
  expect(sendBox).not.toBeNull();

  const conversationRight = (conversationBox?.x ?? 0) + (conversationBox?.width ?? 0);
  const composerRight = (composerBox?.x ?? 0) + (composerBox?.width ?? 0);
  expect(panelBox?.width ?? 0).toBeLessThanOrEqual(440);
  expect(panelBox?.width ?? 0).toBeGreaterThanOrEqual(360);
  expect(conversationRight).toBeLessThanOrEqual((panelBox?.x ?? 0) + 1);
  expect((messageBox?.x ?? 0) + (messageBox?.width ?? 0)).toBeLessThanOrEqual(conversationRight + 1);
  expect(composerRight).toBeLessThanOrEqual(conversationRight + 1);
  expect((sendBox?.x ?? 0) + (sendBox?.width ?? 0)).toBeLessThanOrEqual(composerRight + 1);
  await expect.poll(() => composer.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
  await expect.poll(() => messageArea.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
  await page.screenshot({ path: testInfo.outputPath("workbench-composer-bounds.png"), fullPage: true });
});

test("selects and persists the global reasoning effort", async ({ page }) => {
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "选择推理强度" });
  await expect(trigger).toContainText("推理 中");
  await trigger.click();
  await expect(page.getByText("设置模型推理强度", { exact: false })).toBeVisible();
  await expect(page.getByRole("menuitemradio", { name: "中" })).toHaveAttribute("aria-checked", "true");
  await page.getByRole("menuitemradio", { name: "高", exact: true }).click();
  await expect(trigger).toContainText("推理 高");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "set_reasoning_effort").length)).toBe(1);
});

test("streams thinking, safe reasoning summaries, and command output inline", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "activity_status_changed", phase: "exploring", status: "thinking" });
  });
  await expect(page.getByText("思考中", { exact: true })).toBeVisible();
  await expect(page.getByText("等待工具调用…", { exact: true })).toHaveCount(0);
  await expect(page.locator(".turn-timeline--empty")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("compact-thinking-status.png"), fullPage: true });
  await expect(page.locator(".message-avatar")).toHaveCount(0);
  await expect(page.getByText("正在执行", { exact: true })).toHaveCount(0);

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live", delta: "正在检查公开事件契约。" });
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live", summary: "正在检查公开事件契约。" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live-2", delta: "正在核对工具输出边界。" });
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live-2", summary: "正在核对工具输出边界。" });
    emit({ ...base, type: "tool_started", phase: "executing", call: { id: "call-live", name: "run_command", arguments: { program: "pnpm", args: ["build"] }, metadata: {} } });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stdout", cursor: 0, delta: "building client\n" });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stderr", cursor: 1, delta: "warning: fixture\n" });
    emit({ ...base, type: "tool_completed", phase: "executing", callId: "call-live", name: "run_command", result: { success: true, output: "building client\n", metadata: { durationMs: 1234 } } });
    emit({ ...base, type: "change_applied", phase: "executing", changeSet: {
      id: "change-live", threadId: "thread-1", turnId: "turn-live", toolCallId: "call-edit-live", createdAtMs: 2,
      undone: false, files: [{ path: "src/App.tsx", destinationPath: null, operation: "modify", beforeHash: "before", afterHash: "after", beforeContent: "before\n", afterContent: "after\n", unifiedDiff: "-before\n+after\n" }],
    } });
  });

  const reasoning = page.locator(".turn-reasoning").last();
  await expect(page.getByText("思考内容", { exact: true })).toHaveCount(1);
  await expect(reasoning.locator(".turn-reasoning-segment")).toHaveCount(2);
  await expect(reasoning.getByText("2 段", { exact: true })).toBeVisible();
  await expect(reasoning).toHaveAttribute("open", "");
  await expect(page.getByText("正在检查公开事件契约。", { exact: true })).toBeVisible();
  await expect(page.getByText("正在核对工具输出边界。", { exact: true })).toBeVisible();
  const liveExecution = page.locator(".message--assistant").last().locator(".turn-execution--live");
  await expect(liveExecution.locator(":scope > summary .turn-disclosure-chevron")).toHaveCount(0);
  await expect.poll(async () => {
    const toolBox = await liveExecution.locator(".turn-timeline-tool").last().boundingBox();
    const statusBox = await liveExecution.locator(":scope > summary").boundingBox();
    return toolBox && statusBox ? statusBox.y >= toolBox.y + toolBox.height : false;
  }).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("grouped-reasoning-summaries.png"), fullPage: true });
  await page.locator(".turn-tool-output").last().locator("summary").click();
  await expect(page.locator(".turn-tool-output-line--stdout").filter({ hasText: "building client" })).toBeVisible();
  await expect(page.locator(".turn-tool-output-line--stderr").filter({ hasText: "warning: fixture" })).toBeVisible();
  await expect(page.getByText("耗时 1.2s", { exact: true })).toBeVisible();
  await page.getByText("查看命令", { exact: true }).last().click();
  await expect(page.locator(".turn-command-details pre").last()).toContainText("pnpm build");
  await page.getByText("查看变更", { exact: true }).last().click();
  await expect(page.getByText("修改 src/App.tsx", { exact: true }).last()).toBeVisible();
  await page.getByText("修改 src/App.tsx", { exact: true }).last().click();
  await expect(page.locator(".turn-change-file pre").last()).toContainText("+after");
  await page.locator(".turn-tool-output").last().scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("live-agent-timeline.png"), fullPage: true });
});

test("renders streamed assistant markdown as structured content", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await page.evaluate(() => {
    localStorage.setItem("kcoder_skin", "codebuddy");
    localStorage.setItem("kcoder_theme", "dark");
  });
  await page.reload();

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-markdown" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({
      ...base,
      type: "text_delta",
      phase: "responding",
      delta: "## 实时渲染\n\n这是 **结构化正文**，包含 `turnTimeline`。\n\n| 字段 | 作用 |\n| --- | --- |\n| messages | 对话消息 |\n| plan | 执行计划 |\n\n```ts\nconst ready = true;\n```\n\n<img src=x onerror=alert(1)>\n\n![远程图片](https://example.com/tracker.png)",
    });
  });

  const liveMessage = page.locator(".message--assistant").last();
  await expect(liveMessage.getByRole("heading", { level: 2, name: "实时渲染" })).toBeVisible();
  await expect(liveMessage.locator("strong").getByText("结构化正文", { exact: true })).toBeVisible();
  await expect(liveMessage.locator("code").getByText("turnTimeline", { exact: true })).toBeVisible();
  await expect(liveMessage.locator("table")).toBeVisible();
  await expect(liveMessage.locator("th")).toHaveText(["字段", "作用"]);
  await expect(liveMessage.locator(".markdown-code-block code")).toContainText("const ready = true;");
  await expect(liveMessage.getByRole("button", { name: "复制代码" })).toBeVisible();
  await expect(liveMessage.locator("img")).toHaveCount(0);
  await expect(liveMessage.locator(".markdown-image-placeholder")).toHaveText("远程图片");
  await liveMessage.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("assistant-markdown.png"), fullPage: true });
});

test("queues messages during thinking and interrupts only from the queued send action", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.locator(".message-queue")).toHaveCount(0);
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-live-queue",
      type: "turn_started",
      phase: "exploring",
    });
  });

  const composer = page.getByRole("textbox", { name: "消息" });
  await expect(composer).toBeEnabled();
  await composer.fill("queued first");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect(page.locator(".message-queue")).toContainText("队列 (1)");
  await expect(page.locator(".message--user").getByText("queued first", { exact: true })).toHaveCount(0);
  await expect(page.locator(".queue-list")).toContainText("queued first");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "cancel_turn").length)).toBe(0);

  await composer.fill("queued second");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();

  await expect(page.locator(".message-queue")).toContainText("队列 (2)");
  await expect(page.locator(".queue-list")).toContainText("queued first");
  await expect(page.locator(".queue-list")).toContainText("queued second");
  await expect(page.locator(".message--user").getByText("queued second", { exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length)).toBe(0);
  await page.screenshot({ path: testInfo.outputPath("queued-message-actions.png"), fullPage: true });

  await page.getByRole("button", { name: "立即发送 queued first", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "cancel_turn").length)).toBe(1);
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-live-queue",
      type: "turn_cancelled",
      phase: "cancelled",
    });
  });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length)).toBe(1);
  await expect(page.locator(".message--user").getByText("queued first", { exact: true })).toBeVisible();
  await expect(page.locator(".message--user").getByText("queued second", { exact: true })).toHaveCount(0);
});

test("keeps OCR text hidden while adding it to the model context", async ({ page }, testInfo) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });
  await composer.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File([new Uint8Array([137, 80, 78, 71])], "ocr-fixture.png", { type: "image/png" }));
    element.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, clipboardData: transfer }));
  });

  await expect(page.getByAltText("ocr-fixture.png")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "recognize_image").length)).toBe(1);
  await expect(page.getByText("hidden OCR fixture", { exact: true })).toHaveCount(0);
  await expect(page.getByText("查看识别文字", { exact: true })).toHaveCount(0);
  await expect(page.locator(".attachment-ocr-state, .attachment-ocr-details")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("ocr-hidden-from-composer.png"), fullPage: true });

  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  const imageMessage = page.locator(".message--user").filter({ has: page.locator(".message-image-attachment") });
  await expect(imageMessage.getByText("ocr-fixture.png", { exact: true })).toBeVisible();
  await expect(imageMessage.locator(".message-content")).toHaveCount(0);
  await expect(page.getByText("hidden OCR fixture", { exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: Array<{ request?: { input?: string } }> }).__runTurnCalls[0]?.request?.input)).toBe("");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: Array<{ attachments?: unknown[] }> }).__runTurnCalls[0]?.attachments?.length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: Array<{ attachments?: Array<{ ocrText?: string }> }> }).__runTurnCalls[0]?.attachments?.[0]?.ocrText)).toBe("hidden OCR fixture");
  await page.screenshot({ path: testInfo.outputPath("image-attachment-in-user-message.png"), fullPage: true });

  await page.evaluate(() => localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
    schemaVersion: 1,
    summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false },
    messages: [{
      schemaVersion: 1,
      id: "message-image-history",
      role: "user",
      content: [
        { type: "context", text: "请分析用户提供的图片。" },
        { type: "context", text: "[图片文字识别: ocr-fixture.png]\nhidden OCR fixture" },
        { type: "image", name: "ocr-fixture.png", dataUrl: "data:image/png;base64,iVBORw0KGgo=" },
      ],
      createdAtMs: 1,
    }],
    messageTurnIds: {},
    turnUserMessageIds: {},
    lastTurn: null,
    toolActivities: [],
    turnTimeline: [],
    approvals: [],
    userInputs: [],
    changes: [],
    todos: [],
    lastUsage: null,
  })));
  await page.reload();
  await expect(imageMessage.getByText("ocr-fixture.png", { exact: true })).toBeVisible();
  await expect(imageMessage.locator(".message-content")).toHaveCount(0);
  await expect(page.getByText("hidden OCR fixture", { exact: true })).toHaveCount(0);
});

test("queues concurrent approvals and drops an expired request", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-approval" };
    const request = (id: string, callId: string, file: string) => ({
      id,
      threadId: "thread-1",
      turnId: "turn-approval",
      toolCallId: callId,
      toolName: "run_command",
      reason: "fixture approval",
      risk: "external",
      arguments: { program: "powershell", args: ["-Command", `Get-Content '${file}'`] },
      preview: null,
      createdAtMs: 1,
      expiresAtMs: Date.now() + 300_000,
    });
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: request("approval-1", "call-1", "docs/架构.md") });
    emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: request("approval-2", "call-2", "docs/开发路线图.md") });
  });

  await expect(page.getByText("待确认 1 / 2", { exact: true })).toBeVisible();
  await expect(page.locator(".approval-prompt")).toContainText("docs/架构.md");
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await expect(page.locator(".approval-prompt")).toContainText("docs/开发路线图.md");

  await page.evaluate(() => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
    };
    const originalInvoke = host.__TAURI_INTERNALS__.invoke;
    host.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "resolve_approval") {
        throw new Error(`approval request was not found: ${String(args?.requestId ?? "")}`);
      }
      return originalInvoke(command, args);
    };
  });
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await expect(page.locator(".message--approval")).toHaveCount(0);
  await expect(page.getByText(/approval request was not found/)).toHaveCount(0);
});

test("streams progress and tools in event order before the turn completes", async ({ page }, testInfo) => {
  await page.goto("/");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({ ...base, type: "text_delta", phase: "planning", delta: "我先读取入口文件。" });
  await emit({
    ...base,
    type: "tool_started",
    phase: "executing",
    call: { id: "call-live", name: "read_file", arguments: { path: "src/App.tsx", startLine: 3370, lineCount: 50 }, metadata: {} },
  });

  const liveMessage = page.locator(".message--assistant").last();
  await expect(liveMessage.getByText("我先读取入口文件。", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline-tool--running").getByText("读取 src/App.tsx L3370-3419", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-tool-meta > span").getByText("执行中", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline-tool--running .turn-tool-running")).toBeVisible();
  const liveExecution = liveMessage.locator(".turn-execution--live");
  await expect(liveExecution).toHaveAttribute("open", "");
  await expect(liveExecution.locator("summary").getByText("处理工具结果中", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".message-avatar")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("active-file-read.png"), fullPage: true });

  await emit({
    ...base,
    type: "tool_completed",
    phase: "executing",
    callId: "call-live",
    name: "read_file",
    result: { success: true, output: "export function App() {}", metadata: { path: "src/App.tsx", bytesReturned: 24, startLine: 3370, endLine: 3382 } },
  });
  await expect(liveMessage.locator(".turn-timeline-tool--completed").getByText("读取 src/App.tsx L3370-3382", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-tool-meta > span").getByText("已完成", { exact: true })).toBeVisible();
  await expect(liveMessage.getByText("思考中", { exact: true })).toBeVisible();
  await emit({ ...base, type: "text_delta", phase: "planning", delta: "入口文件已读取。" });
  await emit({
    ...base,
    type: "turn_completed",
    phase: "complete",
    message: {
      schemaVersion: 1,
      id: "message-live",
      role: "assistant",
      content: [{ type: "text", text: "入口文件已读取。" }],
      createdAtMs: 5,
    },
    usage: null,
    startedAtMs: 1000,
    completedAtMs: 5200,
    durationMs: 4200,
  });

  await expect(liveMessage.getByText("入口文件已读取。", { exact: true })).toBeVisible();
  await expect(liveMessage.getByText("执行了 4.2s", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline-tool--completed").getByText("读取 src/App.tsx L3370-3382", { exact: true })).toBeHidden();
  await liveMessage.getByText("执行了 4.2s", { exact: true }).click();
  await expect(liveMessage.locator(".turn-timeline-tool--completed").getByText("读取 src/App.tsx L3370-3382", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline > *")).toHaveCount(3);
});

test("shows the backend error instead of failed tool arguments", async ({ page }) => {
  await page.goto("/");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-tool-failure" };
  const error = "tool execution denied: the tool is not allowed by the workspace policy";

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({
    ...base,
    type: "tool_started",
    phase: "executing",
    call: {
      id: "call-search-failure",
      name: "search_repository",
      arguments: { query: "workbench--panel-open" },
      metadata: {},
    },
  });
  await emit({
    ...base,
    type: "tool_completed",
    phase: "executing",
    callId: "call-search-failure",
    name: "search_repository",
    result: { success: false, output: error, metadata: { error: true } },
  });

  const failedTool = page.locator(".message--assistant").last().locator(".turn-timeline-tool--failed");
  await expect(failedTool.getByText("搜索代码", { exact: true })).toBeVisible();
  await expect(failedTool.getByText(error, { exact: true })).toBeVisible();
  await expect(failedTool.getByText("workbench--panel-open", { exact: true })).toHaveCount(0);
});

test("completes and restores an approved edit test repair workflow", async ({ page }, testInfo) => {
  await page.goto("/");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-self-edit" };
  const call = (id: string, name: string, args: Record<string, unknown> = {}) => ({ id, name, arguments: args, metadata: {} });
  const result = (success: boolean, output: string, metadata: Record<string, unknown> = {}) => ({ success, output, metadata });
  const approval = (id: string, callId: string) => ({
    id,
    threadId: "thread-1",
    turnId: "turn-self-edit",
    toolCallId: callId,
    toolName: "apply_patch",
    reason: "review the proposed file change",
    risk: "write",
    arguments: { patch: "*** Begin Patch\n*** End Patch" },
    preview: null,
    createdAtMs: 1,
    expiresAtMs: Date.now() + 300_000,
  });
  const change = (id: string, callId: string, before: string, after: string) => ({
    id,
    threadId: "thread-1",
    turnId: "turn-self-edit",
    toolCallId: callId,
    createdAtMs: 2,
    undone: false,
    files: [{
      path: "src/example.ts",
      destinationPath: null,
      operation: "modify",
      beforeHash: "before",
      afterHash: "after",
      beforeContent: before,
      afterContent: after,
      unifiedDiff: `-${before}\n+${after}`,
    }],
  });

  const readCall = call("call-read", "read_file", { path: "src/example.ts" });
  const firstPatchCall = call("call-patch-1", "apply_patch");
  const failedTestCall = call("call-test-1", "run_command", { program: "pnpm", args: ["test"] });
  const repairPatchCall = call("call-patch-2", "apply_patch");
  const passedTestCall = call("call-test-2", "run_command", { program: "pnpm", args: ["test"] });
  const firstChange = change("change-1", "call-patch-1", "before", "broken");
  const repairedChange = change("change-2", "call-patch-2", "broken", "fixed");

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "先读取目标文件。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: readCall });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: readCall.id, name: readCall.name, result: result(true, "before") });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "开始应用第一版修改。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: firstPatchCall });
  await emit({ ...base, type: "tool_started", phase: "executing", call: firstPatchCall });
  await emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: approval("approval-edit-1", firstPatchCall.id) });
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await emit({ ...base, type: "approval_resolved", phase: "executing", requestId: "approval-edit-1", resolution: { action: "approved", patch: null, selectedPaths: [], expectedHashes: [] } });
  await emit({ ...base, type: "change_applied", phase: "executing", changeSet: firstChange });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: firstPatchCall.id, name: firstPatchCall.name, result: result(true, "applied") });
  await emit({ ...base, type: "tool_started", phase: "executing", call: failedTestCall });
  await emit({ ...base, type: "tool_output_delta", phase: "executing", callId: failedTestCall.id, stream: "stderr", cursor: 1, delta: "test failed\n" });
  await emit({ ...base, type: "tool_output_delta", phase: "executing", callId: failedTestCall.id, stream: "stderr", cursor: 1, delta: "test failed\n" });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: failedTestCall.id, name: failedTestCall.name, result: result(false, "test failed") });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "测试失败，修正实现后重新验证。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: repairPatchCall });
  await emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: approval("approval-edit-2", repairPatchCall.id) });
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await emit({ ...base, type: "approval_resolved", phase: "executing", requestId: "approval-edit-2", resolution: { action: "approved", patch: null, selectedPaths: [], expectedHashes: [] } });
  await emit({ ...base, type: "change_applied", phase: "executing", changeSet: repairedChange });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: repairPatchCall.id, name: repairPatchCall.name, result: result(true, "applied") });
  await emit({ ...base, type: "tool_started", phase: "executing", call: passedTestCall });
  await emit({ ...base, type: "tool_output_delta", phase: "executing", callId: passedTestCall.id, stream: "stdout", cursor: 2, delta: "all tests passed\n" });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: passedTestCall.id, name: passedTestCall.name, result: result(true, "all tests passed") });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "修复完成，测试已经通过。" });
  await emit({
    ...base,
    type: "turn_completed",
    phase: "complete",
    message: { schemaVersion: 1, id: "message-self-edit", role: "assistant", content: [{ type: "text", text: "修复完成，测试已经通过。" }], createdAtMs: 3 },
    usage: null,
    startedAtMs: 1000,
    completedAtMs: 126000,
    durationMs: 125000,
  });

  const liveMessage = page.locator(".message--assistant").last();
  await expect(liveMessage.locator(".turn-timeline-tool")).toHaveCount(5);
  await expect(liveMessage.locator(".turn-timeline-tool").first()).toBeHidden();
  await liveMessage.getByText("执行了 2分05秒", { exact: true }).click();
  const timelineOrder = await liveMessage.locator(".turn-timeline").first().evaluate((timeline) =>
    Array.from(timeline.children).map((child) => child.className),
  );
  const approvalRequestIndex = timelineOrder.findIndex((className) => className.includes("approval_requested"));
  const approvalResolvedIndex = timelineOrder.findIndex((className) => className.includes("approval_resolved"));
  const toolIndexes = timelineOrder
    .map((className, index) => className.includes("turn-timeline-tool") ? index : -1)
    .filter((index) => index >= 0);
  expect(approvalRequestIndex).toBeGreaterThanOrEqual(0);
  expect(approvalResolvedIndex).toBeGreaterThan(approvalRequestIndex);
  expect(approvalResolvedIndex).toBeLessThan(toolIndexes[1]);
  await expect(liveMessage.getByText("测试失败，修正实现后重新验证。", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-tool-output-line--stderr")).toHaveText("test failed\n");
  await expect(liveMessage.getByText("修复完成，测试已经通过。", { exact: true })).toHaveCount(1);
  await expect(liveMessage.locator(".changes-toggle")).toContainText("2 个文件");

  const persistedTimeline = [
    { type: "text", id: "progress-read", turnId: "turn-self-edit", text: "先读取目标文件。" },
    { type: "tool", activity: { turnId: "turn-self-edit", call: readCall, state: "completed", result: result(true, "before") } },
    { type: "text", id: "progress-edit", turnId: "turn-self-edit", text: "开始应用第一版修改。" },
    { type: "tool", activity: { turnId: "turn-self-edit", call: firstPatchCall, state: "completed", result: result(true, "applied") } },
    { type: "event", itemId: "approval-requested-approval-edit-1", turnId: "turn-self-edit", kind: "approval_requested", title: "已请求操作确认", detail: "apply_patch · review the proposed file change" },
    { type: "event", itemId: "approval-resolved-approval-edit-1", turnId: "turn-self-edit", kind: "approval_resolved", title: "操作确认已处理", detail: "approved" },
    { type: "tool", activity: { turnId: "turn-self-edit", call: failedTestCall, state: "failed", result: result(false, "test failed", { outputChunks: [{ stream: "stderr", cursor: 1, text: "test failed\n" }] }) } },
    { type: "text", id: "progress-repair", turnId: "turn-self-edit", text: "测试失败，修正实现后重新验证。" },
    { type: "tool", activity: { turnId: "turn-self-edit", call: repairPatchCall, state: "completed", result: result(true, "applied") } },
    { type: "tool", activity: { turnId: "turn-self-edit", call: passedTestCall, state: "completed", result: result(true, "all tests passed") } },
    { type: "text", id: "message-self-edit", turnId: "turn-self-edit", text: "修复完成，测试已经通过。" },
    { type: "event", itemId: "turn-completed-turn-self-edit", turnId: "turn-self-edit", kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 125000 },
  ];
  await page.evaluate((detail) => localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify(detail)), {
    schemaVersion: 1,
    summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 3, archived: false },
    messages: [
      { schemaVersion: 1, id: "message-user-self-edit", role: "user", content: [{ type: "text", text: "修改并测试" }], createdAtMs: 1 },
      { schemaVersion: 1, id: "message-self-edit", role: "assistant", content: [{ type: "text", text: "修复完成，测试已经通过。" }], createdAtMs: 3 },
    ],
    messageTurnIds: { "message-self-edit": "turn-self-edit" },
    lastTurn: { turnId: "turn-self-edit", state: "completed", error: null },
    toolActivities: [],
    turnTimeline: persistedTimeline,
    approvals: [{ request: approval("approval-edit-1", firstPatchCall.id), resolution: { action: "approved", patch: null, selectedPaths: [], expectedHashes: [] } }],
    changes: [firstChange, repairedChange],
  });
  await page.reload();

  const restoredMessage = page.locator(".message--assistant").last();
  await expect(restoredMessage.locator(".turn-timeline-tool")).toHaveCount(5);
  await expect(restoredMessage.locator(".turn-timeline-tool").first()).toBeHidden();
  await restoredMessage.getByText("执行了 2分05秒", { exact: true }).click();
  const restoredTimelineOrder = await restoredMessage.locator(".turn-timeline").first().evaluate((timeline) =>
    Array.from(timeline.children).map((child) => child.className),
  );
  const restoredRequestIndex = restoredTimelineOrder.findIndex((className) => className.includes("approval_requested"));
  const restoredResolutionIndex = restoredTimelineOrder.findIndex((className) => className.includes("approval_resolved"));
  const restoredToolIndexes = restoredTimelineOrder
    .map((className, index) => className.includes("turn-timeline-tool") ? index : -1)
    .filter((index) => index >= 0);
  expect(restoredRequestIndex).toBeLessThan(restoredToolIndexes[1]);
  expect(restoredResolutionIndex).toBeLessThan(restoredToolIndexes[1]);
  await expect(restoredMessage.locator(".turn-tool-output-line--stderr")).toHaveText("test failed\n");
  await expect(restoredMessage.getByText("修复完成，测试已经通过。", { exact: true })).toHaveCount(1);
  await expect(restoredMessage.locator(".changes-toggle")).toContainText("2 个文件");
  await restoredMessage.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("self-edit-recovery.png"), fullPage: true });
});

test("keeps a cancelled turn busy until the terminal event and then retries", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-cancel" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "text_delta", phase: "responding", delta: "正在执行长任务。" });
    emit({
      ...base,
      type: "tool_started",
      phase: "executing",
      call: { id: "call-cancel", name: "run_command", arguments: { program: "pnpm", args: ["build"] }, metadata: {} },
    });
  });

  await page.getByRole("button", { name: "停止生成" }).click();
  await expect(page.getByRole("button", { name: "停止生成" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "cancel_turn").length)).toBe(1);

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-cancel",
      type: "turn_cancelled",
      phase: "cancelled",
    });
  });
  const cancelledMessage = page.locator(".message--assistant").last();
  await expect(cancelledMessage.locator(".turn-timeline-tool--running")).toHaveCount(0);
  await expect(cancelledMessage.locator(".turn-timeline-tool--cancelled")).toContainText("已取消");
  await expect(page.getByRole("button", { name: "重试" })).toBeVisible();
  await page.getByRole("button", { name: "重试" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "retry_turn").length)).toBe(1);
});

test("keeps retry attempts in one assistant reply before and after recovery", async ({ page }, testInfo) => {
  const failedDetail = {
    schemaVersion: 1,
    summary: { schemaVersion: 1, id: "thread-1", title: "Retry grouping", createdAtMs: 1, updatedAtMs: 2, archived: false },
    messages: [{ schemaVersion: 1, id: "message-retry-user", role: "user", content: [{ type: "text", text: "修复这个问题" }], createdAtMs: 1 }],
    messageTurnIds: {},
    turnUserMessageIds: { "turn-first": "message-retry-user" },
    lastTurn: { turnId: "turn-first", state: "failed", error: "provider failed" },
    toolActivities: [],
    turnTimeline: [{
      type: "event",
      itemId: "turn-failed-turn-first",
      turnId: "turn-first",
      kind: "turn_failed",
      title: "Turn 已失败",
      detail: "provider failed",
      durationMs: 120,
    }],
    approvals: [],
    userInputs: [],
    changes: [],
    todos: [],
    lastUsage: null,
  };
  await page.addInitScript((detail) => {
    localStorage.setItem("kcoder_e2e_hold_retry", "true");
    if (!localStorage.getItem("kcoder_e2e_thread_detail")) {
      localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify(detail));
    }
  }, failedDetail);
  await page.goto("/");

  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "retry_turn").length)).toBe(1);
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-second" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "text_delta", phase: "responding", delta: "继续检查并修复。" });
  });

  const liveGroup = page.locator(".message--retry-group");
  await expect(liveGroup).toHaveCount(1);
  await expect(liveGroup.locator(".message-role")).toHaveText("k-Coder");
  await expect(liveGroup.locator(".message-retry-attempt")).toHaveCount(2);
  await expect(page.locator(".message--assistant .message-role")).toHaveCount(1);
  await expect(liveGroup.getByText("继续检查并修复。", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("retry-group-live.png"), fullPage: true });

  const completedDetail = {
    ...failedDetail,
    summary: { ...failedDetail.summary, updatedAtMs: 4 },
    messages: [
      ...failedDetail.messages,
      { schemaVersion: 1, id: "message-retry-assistant", role: "assistant", content: [{ type: "text", text: "问题已经修复。" }], createdAtMs: 4 },
    ],
    messageTurnIds: { "message-retry-assistant": "turn-second" },
    turnUserMessageIds: { "turn-first": "message-retry-user", "turn-second": "message-retry-user" },
    lastTurn: { turnId: "turn-second", state: "completed", error: null },
    turnTimeline: [
      ...failedDetail.turnTimeline,
      { type: "text", id: "message-retry-assistant", turnId: "turn-second", text: "问题已经修复。" },
      { type: "event", itemId: "turn-completed-turn-second", turnId: "turn-second", kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 240 },
    ],
  };
  await page.evaluate((detail) => localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify(detail)), completedDetail);
  await page.reload();

  const restoredGroup = page.locator(".message--retry-group");
  await expect(restoredGroup).toHaveCount(1);
  await expect(restoredGroup.locator(".message-role")).toHaveText("k-Coder");
  await expect(restoredGroup.locator(".message-retry-attempt")).toHaveCount(2);
  await expect(page.locator(".message--assistant .message-role")).toHaveCount(1);
  await expect(restoredGroup.getByText("问题已经修复。", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("retry-group-restored.png"), fullPage: true });
});

test("restores a pending user question after reopening the thread", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false },
      messages: [{ schemaVersion: 1, id: "question-user", role: "user", content: [{ type: "text", text: "Plan this change" }], createdAtMs: 1 }],
      messageTurnIds: {},
      turnUserMessageIds: { "turn-question": "question-user" },
      lastTurn: { turnId: "turn-question", state: "awaiting_approval", error: null },
      toolActivities: [],
      turnTimeline: [{ type: "event", itemId: "user-input-requested-input-1", turnId: "turn-question", kind: "user_input_requested", title: "User input requested", detail: "Choose an approach" }],
      approvals: [],
      userInputs: [{
        request: {
          id: "input-1",
          threadId: "thread-1",
          turnId: "turn-question",
          toolCallId: "call-input",
          questions: [{ question: "Choose an approach", options: ["Conservative", "Fast"] }],
          createdAtMs: 1,
          expiresAtMs: Date.now() + 300000,
        },
        resolution: null,
      }],
      changes: [],
      todos: [],
      lastUsage: null,
    }));
  });
  await page.goto("/");
  await expect(page.locator(".user-input-question-text").getByText("Choose an approach", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Fast", exact: true }).click();
  await page.getByRole("button", { name: "提交回答", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "resolve_user_input").length)).toBe(1);
});

test("replays live events received while a thread snapshot is loading", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_read_delay_ms", "500");
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false },
      messages: [],
      messageTurnIds: {},
      turnUserMessageIds: {},
      lastTurn: null,
      toolActivities: [],
      turnTimeline: [],
      approvals: [],
      userInputs: [],
      changes: [],
      todos: [],
      lastUsage: null,
    }));
  });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => {
    try {
      (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
        schemaVersion: 1,
        threadId: "thread-1",
        turnId: "turn-hydration",
        type: "turn_started",
        phase: "exploring",
      });
      return true;
    } catch {
      return false;
    }
  })).toBe(true);
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-hydration",
      type: "text_delta",
      phase: "responding",
      delta: "live event survived snapshot hydration",
    });
  });
  await expect(page.getByText("live event survived snapshot hydration", { exact: true })).toBeVisible();
});

test("shows and starts subagent activity without a default token budget", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "子智能体", exact: true }).click();
  await page.getByRole("button", { name: /检查后端/ }).click();
  await expect(page.getByText("后端检查完成", { exact: true })).toBeVisible();
  await expect(page.getByText("420 / 无上限 tokens", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "创建新任务" }).click();
  await page.getByLabel("子任务描述").fill("检查测试");
  await page.getByRole("button", { name: "启动" }).click();
  await expect(page.getByRole("button", { name: /检查测试/ })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("create_subagent"))).toBe(true);
  await expect.poll(() => page.evaluate(() => {
    const args = (window as unknown as { __invocationArgs: Record<string, { request?: Record<string, unknown> }> }).__invocationArgs.create_subagent;
    return args?.request && Object.prototype.hasOwnProperty.call(args.request, "tokenBudget");
  })).toBe(false);
});

test("shows unbounded goal token consumption and controls", async ({ page }, testInfo) => {
  await page.goto("/");
  const goal = page.locator(".goal-slim");
  await expect(goal).toContainText("24,000 / 无上限 tokens");
  await expect(goal.locator(".goal-slim-track")).toHaveCount(0);
  await goal.click();

  const dialog = page.getByRole("dialog", { name: "设置" });
  await expect(dialog.getByRole("heading", { name: "目标与预算" })).toBeVisible();
  await expect(dialog).toContainText("完成 Phase 9 高级智能体能力");
  await expect(dialog).toContainText("24,000 / 无上限 tokens");
  await expect(dialog.locator(".goal-progress")).toHaveCount(0);
  await dialog.getByRole("button", { name: "暂停" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("transition_goal"))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`goal-${testInfo.project.name}.png`), fullPage: true });
});

test("switches providers and models from the composer footer", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.getByRole("button", { name: "切换到深色模式" }).click();
  const composer = page.locator(".composer");
  const selector = composer.getByRole("button", { name: "选择模型" });

  await expect(selector).toBeVisible();
  await expect(selector).toContainText("OpenAI");
  await expect(selector).toContainText("GPT-4.1");
  await expect(selector.locator("em")).toHaveCount(0);
  await expect(page.locator(".sidebar").getByRole("button", { name: "选择模型" })).toHaveCount(0);

  await selector.click();
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeVisible();
  const providerOptions = page.locator(".model-selector-provider-options");
  await expect(providerOptions.getByRole("button")).toHaveCount(3);
  await expect(providerOptions.getByRole("button", { name: /待配置供应商/ })).toBeDisabled();
  await expect(page.getByRole("option")).toHaveCount(2);
  await expect(page.getByText("Deepseek-V4-Pro", { exact: true })).toHaveCount(0);
  await page.getByRole("option", { name: /GPT-4 Omni.*gpt-4o/ }).click();
  await expect(selector).toContainText("GPT-4 Omni");
  await expect(selector.locator("em")).toHaveText("gpt-4o");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("save_provider_config"))).toBe(true);

  await selector.click();
  await providerOptions.getByRole("button", { name: /zicc/ }).click();
  await expect(selector).toContainText("zicc");
  await expect(selector).toContainText("gpt-5.6-terra");
  await expect(selector.locator("em")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastActivatedProvider: string | null }).__lastActivatedProvider)).toBe("zicc");

  await selector.press("ArrowDown");
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeVisible();
  await expect(page.getByRole("option")).toHaveCount(2);
  await expect(page.getByRole("option", { name: /gpt-5.5/ })).toBeVisible();
  await page.waitForTimeout(250);
  await page.screenshot({ path: testInfo.outputPath(`provider-selector-${testInfo.project.name}.png`), fullPage: true });
  await page.keyboard.press("Escape");
  await expect(page.getByRole("listbox", { name: "可用模型" })).toBeHidden();
  await expect(selector).toBeFocused();
});

test("switches the runtime approval mode from the composer", async ({ page }, testInfo) => {
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "选择操作批准方式" });

  await expect(trigger).toContainText("请求批准");
  await trigger.click();
  const menu = page.getByRole("menu", { name: "操作批准方式" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitemradio", { name: /请求批准/ })).toHaveAttribute("aria-checked", "true");
  await menu.getByRole("menuitemradio", { name: /完整访问/ }).click();

  await expect(trigger).toContainText("完整访问");
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __lastApprovalMode: string | null }).__lastApprovalMode,
  )).toBe("full_access");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      type: "approval_requested",
      phase: "awaiting_input",
      threadId: "thread-1",
      turnId: "turn-auto-approved",
      request: {
        id: "approval-auto",
        threadId: "thread-1",
        turnId: "turn-auto-approved",
        toolCallId: "call-auto",
        toolName: "run_command",
        reason: "full-access mode automatically approved: fixture",
        autoApproved: true,
        risk: "external",
        arguments: { program: "pnpm", args: ["test"] },
        preview: null,
        createdAtMs: 1,
        expiresAtMs: 2,
      },
    });
  });
  await page.locator(".message--activity-only").last().locator(".turn-execution > summary").click();
  await expect(page.getByText("已自动批准操作", { exact: true })).toBeVisible();
  await expect(page.locator(".message--approval")).toHaveCount(0);
  await trigger.click();
  await expect(menu.getByRole("menuitemradio", { name: /完整访问/ })).toHaveAttribute("aria-checked", "true");
  await page.waitForTimeout(250);
  await page.screenshot({ path: testInfo.outputPath(`approval-mode-${testInfo.project.name}.png`), fullPage: true });
});

test("adds, edits, deletes, and saves structured provider models", async ({ page }) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();

  await page.getByRole("button", { name: "新增模型" }).click();
  await expect(page.locator(".provider-model-card")).toHaveCount(3);
  await page.getByLabel("模型 ID 3").fill("o3-mini");
  await page.getByLabel("显示名称 3").fill("O3 Mini");
  await page.getByLabel("上下文长度 3").fill("200000");
  await page.getByLabel(/设为默认模型：O3 Mini/).check();

  await page.getByRole("button", { name: "删除模型：GPT-4 Omni" }).click();
  await expect(page.locator(".provider-model-card")).toHaveCount(2);
  await page.getByRole("button", { name: "保存配置" }).click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastProviderRequest: { model: string } | null }).__lastProviderRequest?.model)).toBe("o3-mini");
  const request = await page.evaluate(() => (window as unknown as { __lastProviderRequest: { models: unknown[] } }).__lastProviderRequest);
  expect(request.models).toEqual([
    { id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, maxOutputTokens: undefined, supportsVision: false, fallback: false },
    { id: "o3-mini", displayName: "O3 Mini", contextWindow: 200000, maxOutputTokens: undefined, supportsVision: false, fallback: false },
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

test("colors file formats and wires complete Git actions", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.evaluate(() => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
      __gitActions: Array<Record<string, unknown>>;
    };
    const originalInvoke = host.__TAURI_INTERNALS__.invoke;
    host.__gitActions = [];
    host.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "list_workspace_directory") {
        return [
          { name: "app.ts", path: "app.ts", isDirectory: false, size: 10, modifiedAtMs: 2 },
          { name: "package.json", path: "package.json", isDirectory: false, size: 10, modifiedAtMs: 2 },
          { name: "README.md", path: "README.md", isDirectory: false, size: 10, modifiedAtMs: 2 },
          { name: "styles.css", path: "styles.css", isDirectory: false, size: 10, modifiedAtMs: 2 },
        ];
      }
      if (command === "git_status") {
        return {
          isRepository: true,
          branch: "main",
          upstream: "origin/main",
          ahead: 0,
          behind: 0,
          files: [
            { path: "new.ts", indexStatus: "?", worktreeStatus: "?" },
            { path: "staged.ts", indexStatus: "M", worktreeStatus: " " },
          ],
        };
      }
      if (command === "git_action") {
        host.__gitActions.push(args ?? {});
        return "ok";
      }
      return originalInvoke(command, args);
    };
  });

  await page.getByRole("button", { name: "工作台", exact: true }).click();
  await page.getByRole("button", { name: "刷新文件树" }).click();
  await expect(page.locator(".file-type-icon--typescript")).toBeVisible();
  await expect(page.locator(".file-type-icon--json")).toBeVisible();
  await expect(page.locator(".file-type-icon--document")).toBeVisible();
  await expect(page.locator(".file-type-icon--style")).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("colored-file-tree.png"), fullPage: true });

  await page.getByRole("tab", { name: "Git" }).click();
  await expect(page.getByRole("button", { name: "暂存 new.ts" })).toBeVisible();
  await page.getByRole("button", { name: "暂存 new.ts" }).click();
  page.on("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "拉取" }).click();
  await page.getByRole("button", { name: "推送" }).click();
  await page.getByLabel("提交说明").fill("test workbench Git actions");
  await page.screenshot({ path: testInfo.outputPath("git-actions.png"), fullPage: true });
  await page.getByRole("button", { name: "提交", exact: true }).click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __gitActions: Array<{ action: string }> }).__gitActions.map(({ action }) => action))).toEqual([
    "stage",
    "pull",
    "push",
    "commit",
  ]);
});

test("hides project-bound sessions from the plain conversation list", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === "narrow", "窄屏隐藏侧边栏，项目会话分组只在桌面侧边栏展示");
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_thread_project_map", JSON.stringify({ "thread-1": "D:\\code\\k-coder" }));
    localStorage.setItem("kcoder_known_projects", JSON.stringify(["D:\\code\\k-coder"]));
  });
  await page.goto("/");

  // 绑定到项目的会话不再出现在"会话"tab 的普通列表中
  const conversationList = page.getByRole("navigation", { name: "会话列表" });
  await expect(conversationList).toBeVisible();
  await expect(conversationList.getByText("Phase 6 workbench", { exact: true })).toHaveCount(0);
  await expect(conversationList.getByText("还没有会话", { exact: true })).toBeVisible();

  // 同一会话仍在"项目"tab 的项目分组中展示
  await page.getByRole("tab", { name: "项目" }).click();
  const projectList = page.getByRole("navigation", { name: "项目列表" });
  await expect(projectList.getByText("k-coder", { exact: true })).toBeVisible();
  await expect(projectList.locator(".project-group-count")).toHaveText("1");
  await projectList.getByRole("button", { name: "展开项目" }).click();
  await expect(projectList.getByText("Phase 6 workbench", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("project-session-list.png"), fullPage: true });
});
