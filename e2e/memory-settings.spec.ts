import { expect, test } from "@playwright/test";

/// 记忆页：候选审核、按作用域查看、删除与清空的确认令牌。
///
/// 这里是 Task 2 推迟到本轮的界面（记忆列表/审核/删除确认）。桩同时钉住命令名与驼峰参数名，
/// 尤其是确认令牌必须是宿主算出的记忆 ID 与规范 scope 串，而不是前端自己编的值。
test("memory page reviews candidates and deletes with the host confirmation token", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 820 });

  await page.addInitScript(() => {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    (window as unknown as { __memoryCalls: typeof calls }).__memoryCalls = calls;

    const memory = {
      id: "memory-1",
      scopeType: "user",
      scopeId: null,
      memoryType: "preference",
      normalizedKey: "prefer pnpm workspaces",
      content: "prefer pnpm workspaces",
      sourceType: "user",
      sourceRef: null,
      confidence: 1,
      sensitivity: "normal",
      status: "active",
      revision: 1,
      expiresAtMs: null,
      createdAtMs: 1,
      updatedAtMs: 2,
    };
    const candidate = {
      id: "candidate-1",
      operation: "create",
      targetMemoryId: null,
      scopeType: "project",
      scopeId: "project-1",
      memoryType: "fact",
      normalizedKey: "deployment target is staging",
      content: "the deployment target is staging",
      reason: "observed in the transcript",
      confidence: 0.9,
      requiresReview: true,
      status: "pending",
      sourceTurnId: "turn-1",
      createdAtMs: 3,
      reviewedAtMs: null,
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback: (_callback: (...args: unknown[]) => void) => 1,
      unregisterCallback: () => undefined,
      invoke: async (command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args: args ?? {} });
        switch (command) {
          case "plugin:event|listen":
            return typeof args?.handler === "number" ? args.handler : 1;
          case "runtime_status":
            return { ready: true, phase: "agent", version: "0.10.0", uptimeSeconds: 1, capabilities: [] };
          case "get_approval_mode":
            return "ask";
          case "get_reasoning_effort":
            return "medium";
          case "get_provider_catalog":
            return {
              schemaVersion: 1,
              activeProviderId: "openai",
              providers: [{
                schemaVersion: 1,
                id: "openai",
                kind: "open_ai_compatible",
                transport: "open_ai_chat_completions",
                name: "OpenAI",
                baseUrl: "https://api.openai.com/v1",
                model: "gpt-4.1",
                models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }],
                endpoints: [],
                hasApiKey: true,
              }],
            };
          case "list_builtin_workflows":
          case "list_scheduled_tasks":
          case "list_threads":
          case "list_subagents":
          case "list_workspace_directory":
            return [];
          case "read_thread":
          case "read_thread_history":
          case "get_plan":
          case "get_goal":
          case "get_workflow_run":
            return null;
          case "read_thread_mailbox":
            return { schemaVersion: 1, threadId: "thread-1", revision: 0, activeTurnId: null, pending: [] };
          case "workspace_state":
            return {
              current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
              recent: [],
            };
          case "git_status":
            return { isRepository: true, branch: "main", upstream: null, ahead: 0, behind: 0, files: [] };
          case "git_branches":
            return { current: "main", branches: ["main"] };
          case "get_memory_settings":
            return { schemaVersion: 1, enabled: true, autoAcceptHighConfidence: false, defaultTtlDays: 0 };
          case "list_memory_candidates":
            return [candidate];
          case "list_memories":
            return { items: [memory], nextCursor: null, total: 1, scope: "user", status: "active" };
          case "review_memory_candidate":
            return { ...candidate, status: args?.decision === "accept" ? "accepted" : "rejected" };
          case "delete_memory":
            return { ...memory, status: "deleted" };
          case "set_memory_settings":
            return { schemaVersion: 1, enabled: true, autoAcceptHighConfidence: true, defaultTtlDays: 30 };
          case "set_memory_enabled":
            return { schemaVersion: 1, enabled: Boolean(args?.enabled), autoAcceptHighConfidence: false, defaultTtlDays: 0 };
          case "clear_memories":
            return { scope: args?.scope, clearedCount: 1 };
          default:
            return null;
        }
      },
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };
  });

  await page.goto("/");
  await page.waitForTimeout(400);
  const settings = page.getByRole("dialog", { name: "设置" });
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: /^记忆/ }).click();

  // 候选审核。
  await expect(settings.getByRole("heading", { name: "待审核候选", exact: true })).toBeVisible();
  await expect(settings.getByText("the deployment target is staging")).toBeVisible();
  await expect(settings.getByText(/project:project-1 · conf 0.90/)).toBeVisible();
  await settings.getByRole("button", { name: "接受" }).click();

  // 生效记忆与按作用域的删除确认令牌。
  await expect(settings.getByText("prefer pnpm workspaces")).toBeVisible();
  await expect(settings.getByText(/preference · rev 1/)).toBeVisible();
  await settings.getByRole("button", { name: "删除记忆 preference" }).click();
  await expect(settings.getByRole("heading", { name: "删除这条记忆", exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "确认", exact: true }).click();

  // 写入策略保存。
  await settings.getByLabel("默认 TTL 天数").fill("30");
  await settings.getByRole("button", { name: "保存", exact: true }).click();

  const commands = await page.evaluate(() =>
    (window as unknown as { __memoryCalls: Array<{ command: string; args: Record<string, unknown> }> }).__memoryCalls,
  );
  const findLast = (command: string) => commands.filter((call) => call.command === command).pop() ?? null;
  expect(findLast("review_memory_candidate")?.args).toMatchObject({ candidateId: "candidate-1", decision: "accept" });
  expect(findLast("delete_memory")?.args).toMatchObject({ memoryId: "memory-1", confirmationToken: "memory-1" });
  expect(findLast("set_memory_settings")?.args).toMatchObject({
    request: { enabled: true, autoAcceptHighConfidence: false, defaultTtlDays: 30 },
  });
  // 列表查询用的是宿主规范 scope 串，而不是任意字符串。
  expect(findLast("list_memories")?.args).toMatchObject({ scope: "user", status: "active" });

  await page.setViewportSize({ width: 700, height: 820 });
  await page.waitForTimeout(100);
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth + 1);
});
