import { expect, test } from "@playwright/test";

test("project workflow can be saved, published, and selected in the composer", async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 1;
    let workflowRecord: Record<string, unknown> | null = null;
    let published = false;
    const thread = {
      schemaVersion: 1,
      id: "thread-workflow",
      title: "Project workflow",
      createdAtMs: 1,
      updatedAtMs: 2,
      archived: false,
      inProject: true,
      workspacePath: "D:\\projects\\demo",
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
        hasApiKey: true,
      }],
    };
    const readiness = (workflowId: string) => ({
      schemaVersion: 1,
      workflowId,
      definitionVersion: 1,
      ready: true,
      skillCount: 0,
      localSkillCount: 0,
      pluginSkillCount: 0,
      blockerCount: 0,
      bindings: [],
      nodes: [],
      blockers: [],
    });
    const definition = () => workflowRecord ? ({
      schemaVersion: workflowRecord.schemaVersion,
      definitionVersion: workflowRecord.revision,
      id: workflowRecord.id,
      source: "custom",
      status: published ? "published" : "draft",
      name: workflowRecord.name,
      description: workflowRecord.description,
      rolePrompt: "Follow the ordered project steps.",
      localSkillCount: 0,
      pluginSkillCount: 0,
      uniqueSkillCount: 0,
      skillCatalog: [],
      nodes: (workflowRecord.nodes as Array<Record<string, unknown>>).map((node) => ({
        id: node.id,
        title: node.title,
        description: node.description,
        localSkillCount: 0,
        pluginSkillCount: 0,
        skillDeclarationCount: 0,
        localSkillBindings: [],
        pluginSkillBindings: [],
      })),
    }) : null;
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
        if (command === "list_workflows") return published && definition() ? [definition()] : [];
        if (command === "list_managed_workflows") return workflowRecord ? [workflowRecord] : [];
        if (command === "save_workflow_draft") {
          const request = args?.request as Record<string, unknown>;
          const id = (request.workflowId as string | null) ?? "custom-0123456789abcdef0123456789abcdef";
          workflowRecord = {
            schemaVersion: 1,
            id,
            name: request.name,
            description: request.description,
            status: "draft",
            revision: workflowRecord ? Number(workflowRecord.revision) + 1 : 1,
            nodes: request.nodes,
            createdAtMs: 1,
            updatedAtMs: 2,
          };
          return workflowRecord;
        }
        if (command === "publish_workflow_draft") {
          published = true;
          if (workflowRecord) workflowRecord = { ...workflowRecord, status: "published", revision: Number(workflowRecord.revision) + 1 };
          return workflowRecord;
        }
        if (command === "get_workflow_skill_readiness") return readiness(String(args?.workflowId));
        if (command === "get_workflow_run" || command === "get_plan" || command === "get_goal") return null;
        if (command === "list_threads") return [thread];
        if (command === "read_thread_history") return null;
        if (command === "read_thread") return detail;
        if (command === "read_thread_mailbox") return { schemaVersion: 1, threadId: thread.id, revision: 0, activeTurnId: null, pending: [] };
        if (command === "list_subagents") return [];
        if (command === "workspace_state") return { current: { id: "project-demo", name: "demo", path: thread.workspacePath, trusted: true, lastOpenedAtMs: 2 }, recent: [] };
        if (command === "list_workspace_directory") return [];
        if (command === "git_status") return { isRepository: true, branch: "main", upstream: null, ahead: 0, behind: 0, files: [] };
        if (command === "git_branches") return { current: "main", branches: ["main"] };
        if (command === "extension_overview") return { schemaVersion: 1, configPaths: [], instructions: [], skills: [], mcpServers: [], hooks: [], audit: [], error: null };
        return null;
      },
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  });

  await page.goto("/");
  await expect(page.getByRole("button", { name: "设置" })).toBeVisible();
  await page.getByRole("button", { name: "设置" }).click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "Workflows" }).click();
  await expect(settings.getByRole("heading", { name: "项目 Workflows" })).toBeVisible();
  await settings.getByRole("button", { name: "新建流程" }).click();

  await settings.getByLabel("流程名称").fill("Project review");
  await settings.getByLabel("流程说明").fill("Review the project before a release.");
  await settings.locator(".workflow-settings-form-grid input").nth(0).fill("Inspect changes");
  await settings.locator(".workflow-settings-form-grid input").nth(1).fill("Review the proposed change set.");
  await settings.getByLabel("任务说明").fill("Inspect changed files and identify risks.");
  await settings.getByLabel("完成标准").fill("The key risks and checks are documented.");
  await settings.getByRole("button", { name: "保存草稿" }).click();
  await expect(settings.getByText("草稿已保存。")).toBeVisible();
  await settings.getByRole("button", { name: "发布流程" }).click();
  await expect(settings.getByText("流程已发布，可在聊天输入框旁的流程选择器中启动。"))
    .toBeVisible();

  await settings.getByRole("button", { name: "关闭设置" }).click();
  const selector = page.getByRole("button", { name: "选择机器人" });
  await selector.click();
  await expect(page.getByRole("menuitemradio", { name: /Project review/ })).toBeVisible();
  expect(consoleErrors).toEqual([]);
});
