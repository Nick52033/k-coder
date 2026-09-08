import { expect, test } from "@playwright/test";

test("scheduled task page supports the main interaction path", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 820 });
  const task = {
    schemaVersion: 1,
    id: "task-1",
    name: "Daily workspace review",
    schedule: { kind: "daily", hour: 9, minute: 30, weekday: null, atMs: null },
    prompt: "Review the workspace and summarize outstanding work.",
    mode: "background",
    threadId: null,
    workspacePath: "D:\\code\\k-coder",
    enabled: true,
    nextRunAtMs: Date.now() + 3_600_000,
    lastRunAtMs: null,
    lastRunState: null,
    lastError: null,
    runCount: 0,
    createdAtMs: Date.now(),
    updatedAtMs: Date.now(),
    revision: 1,
  };

  await page.addInitScript((fixture) => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    const tasks = [fixture];
    let callbackId = 1;
    const thread = {
      schemaVersion: 1,
      id: "thread-1",
      title: "Workspace review",
      createdAtMs: 1,
      updatedAtMs: 2,
      archived: false,
      inProject: true,
      workspacePath: "D:\\code\\k-coder",
    };
    const detail = {
      schemaVersion: 1,
      summary: thread,
      messages: [],
      messageTurnIds: {},
      lastTurn: null,
      toolActivities: [],
      turnTimeline: [],
      approvals: [],
      changes: [],
      todos: [],
      lastUsage: null,
      contextUsage: null,
    };
    const catalog = {
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
        hasApiKey: false,
      }],
    };
    const workspace = {
      current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
      recent: [],
    };
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      transformCallback: (callback: (...args: unknown[]) => void) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args?: Record<string, unknown>) => {
        if (command === "plugin:event|listen") return 1;
        if (command === "runtime_status") return { ready: true, phase: "agent", version: "0.10.0", uptimeSeconds: 1, capabilities: [] };
        if (command === "get_approval_mode") return "ask";
        if (command === "get_reasoning_effort") return "medium";
        if (command === "get_provider_catalog") return catalog;
        if (command === "list_builtin_workflows") return [];
        if (command === "list_threads") return [thread];
        if (command === "read_thread_history") return null;
        if (command === "read_thread") return detail;
        if (command === "get_plan" || command === "get_goal" || command === "get_workflow_run") return null;
        if (command === "read_thread_mailbox") return { schemaVersion: 1, threadId: thread.id, revision: 0, activeTurnId: null, pending: [] };
        if (command === "list_subagents") return [];
        if (command === "workspace_state") return workspace;
        if (command === "list_workspace_directory") return [];
        if (command === "git_status") return { isRepository: true, branch: "main", upstream: null, ahead: 0, behind: 0, files: [] };
        if (command === "git_branches") return { current: "main", branches: ["main"] };
        if (command === "list_scheduled_tasks") return tasks;
        if (command === "set_scheduled_task_enabled") {
          fixture.enabled = Boolean(args?.enabled);
          return fixture;
        }
        if (command === "trigger_scheduled_task" || command === "delete_scheduled_task" || command === "upsert_scheduled_task") return fixture;
        return null;
      },
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  }, task);

  await page.goto("/");
  await page.waitForTimeout(500);
  await page.getByRole("button", { name: "定时任务" }).click();
  await expect(page.getByRole("heading", { name: "定时任务", exact: true })).toBeVisible();
  await expect(page.getByText("Daily workspace review")).toBeVisible();
  await page.getByRole("button", { name: "表格" }).click();
  await expect(page.getByRole("columnheader", { name: "计划" })).toBeVisible();
  await page.getByRole("button", { name: "批量管理" }).click();
  await expect(page.getByRole("checkbox", { name: "选择 Daily workspace review" })).toBeVisible();
  await expect(page.getByRole("button", { name: "停用 Daily workspace review" })).toBeVisible();
  await page.getByRole("button", { name: "批量管理" }).click();
  await page.getByRole("button", { name: "添加自动化" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("heading", { name: "创建任务", exact: true })).toBeVisible();
  await page.getByLabel("计划类型").selectOption("weekly");
  await expect(page.getByLabel("星期")).toBeVisible();
  await page.getByLabel("计划类型").selectOption("once");
  await expect(page.locator('input[type="datetime-local"]')).toBeVisible();
  await page.getByRole("button", { name: "关闭" }).click();
  await expect(page.getByRole("dialog")).toBeHidden();

  await page.setViewportSize({ width: 700, height: 820 });
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth + 1);
});
