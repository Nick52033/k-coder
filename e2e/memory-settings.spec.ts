import { expect, test } from "@playwright/test";

/// 记忆页：候选审核、按作用域查看、删除与清空的确认令牌。
///
/// 这里是 Task 2 推迟到本轮的界面（记忆列表/审核/删除确认）。桩同时钉住命令名与驼峰参数名，
/// 尤其是确认令牌必须是宿主算出的记忆 ID 与规范 scope 串，而不是前端自己编的值。
type MemoryStubOptions = { enabled?: boolean; legacyConsent?: boolean; consentVersion?: number; scopeDelayMs?: number; listenDelayMs?: number; recoveryError?: string; providerUnavailableReason?: string; dreamEnabled?: boolean; maintenanceDelayMs?: number };

async function installMemoryStub(page: import("@playwright/test").Page, options: MemoryStubOptions = {}) {
  await page.addInitScript((options) => {
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

    let memorySettings = { schemaVersion: 1, enabled: options.enabled ?? true, autoAcceptHighConfidence: false, defaultTtlDays: 0, ...(options.legacyConsent ? {} : { autoExtractionConsentVersion: options.consentVersion ?? 2, captureAfterMs: 1000 }) };
    let maintenanceSettings = { schemaVersion: 1, enabled: options.dreamEnabled ?? false, dreamEnabled: options.dreamEnabled ?? false, remoteDisclosureAccepted: options.dreamEnabled ?? false, tokenBudget: 4096, intervalMs: 86_400_000, idleAfterMs: 300_000, lastRunAtMs: null as number | null, lastOutcome: "never", runningSinceMs: null as number | null, threadId: null };
    let finishMaintenance: (() => void) | undefined;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    let callbackId = 0;
    let eventRevision = 1;
    const harness = window as unknown as {
      __emitMemoryChanged: () => void;
      __releaseScope: () => void;
      __delayScope: boolean;
      __delayScopeOptions: boolean;
      __releaseScopeOptions: () => void;
      __savedMemory: (Omit<typeof memory, "expiresAtMs" | "scopeId"> & { expiresAtMs: number | null; scopeId: string | null }) | null;
    };
    harness.__delayScope = false;
    harness.__delayScopeOptions = false;
    harness.__releaseScopeOptions = () => undefined;
    harness.__savedMemory = null;
    harness.__releaseScope = () => undefined;
    harness.__emitMemoryChanged = () => {
      candidate.content = "updated candidate from repository";
      for (const [id, listener] of listeners) {
        if (listener.event === "memory:changed") callbacks.get(listener.handler)?.({ event: "memory:changed", id, payload: { revision: ++eventRevision } });
      }
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback: (callback: (event: unknown) => void) => { const id = ++callbackId; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => { callbacks.delete(id); },
      invoke: async (command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args: args ?? {} });
        switch (command) {
          case "plugin:event|listen": {
            const id = typeof args?.handler === "number" ? args.handler : ++callbackId;
            listeners.set(id, { event: String(args?.event), handler: id });
            if (args?.event === "memory:changed" && options.listenDelayMs) await new Promise((resolve) => setTimeout(resolve, options.listenDelayMs));
            return id;
          }
          case "plugin:event|unlisten":
            listeners.delete(Number(args?.eventId));
            return null;
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
            return memorySettings;
          case "get_memory_scopes": {
            if (!args || Object.keys(args).length !== 1 || !("threadId" in args) || (args.threadId !== null && typeof args.threadId !== "string")) throw new Error("get_memory_scopes expects camelCase threadId");
            if (harness.__delayScopeOptions) {
              harness.__delayScopeOptions = false;
              await new Promise<void>((resolve) => { harness.__releaseScopeOptions = resolve; });
              return [{ scope: "user", label: "用户（旧结果）" }, { scope: "project:outdated-host-project", label: "旧项目" }];
            }
            return [{ scope: "user", label: "用户（全局）" }, { scope: "project:host-project", label: "当前项目（宿主）" }, { scope: "workspace:host-workspace", label: "当前工作区（宿主）" }];
          }
          case "get_memory_maintenance_settings":
            return maintenanceSettings;
          case "accept_memory_maintenance_disclosure":
            maintenanceSettings = { ...maintenanceSettings, remoteDisclosureAccepted: true };
            return maintenanceSettings;
          case "set_memory_maintenance_settings": {
            const request = args?.request as { enabled: boolean; dreamEnabled: boolean; remoteDisclosureAccepted: boolean; tokenBudget: number; idleAfterMs: number };
            maintenanceSettings = { ...maintenanceSettings, ...request };
            return maintenanceSettings;
          }
          case "run_memory_maintenance": {
            maintenanceSettings = { ...maintenanceSettings, runningSinceMs: Date.now() };
            if (options.maintenanceDelayMs) await new Promise<void>((resolve) => { finishMaintenance = resolve; setTimeout(resolve, options.maintenanceDelayMs); });
            maintenanceSettings = { ...maintenanceSettings, runningSinceMs: null, lastRunAtMs: Date.now(), lastOutcome: "completed" };
            return { trigger: "manual", outcome: "completed", offline: { expiredIds: [], mergedGroups: [] }, dream: { status: "completed", proposals: 1, accepted: 0, pending: 1, error: null }, startedAtMs: 1000, completedAtMs: Date.now() };
          }
          case "cancel_memory_maintenance":
            maintenanceSettings = { ...maintenanceSettings, runningSinceMs: null, lastOutcome: "cancelled" };
            finishMaintenance?.();
            return true;
          case "get_memory_diagnostics":
            return { schemaVersion: 1, queuedJobs: 2, runningJobs: 0, completedJobs: 3, failedJobs: 1, skippedJobs: 4, dreamPendingSummaries: 2, lastCaptureAtMs: 1000, lastExtractionAtMs: null, lastCandidateCount: 4, lastAcceptedCount: 1, lastPendingCount: 2, lastSuppressedCount: 1, lastReason: "no_new_turns", providerUnavailableReason: options.providerUnavailableReason ?? null, recoveryError: options.recoveryError ?? null };
          case "list_memory_candidates":
            return [candidate];
          case "list_memories": {
            if (args?.scope === "project:host-project") {
              if (harness.__delayScope) await new Promise<void>((resolve) => { harness.__releaseScope = resolve; });
              else if (options.scopeDelayMs) await new Promise((resolve) => setTimeout(resolve, options.scopeDelayMs));
            }
            const stored = harness.__savedMemory;
            const items = stored ? [stored] : [{ ...memory, content: args?.scope === "user" ? memory.content : `memory in ${String(args?.scope)}` }];
            return { items, nextCursor: null, total: 1, scope: args?.scope, status: "active" };
          }
          case "upsert_memory": {
            const request = args?.request as { content: string; memoryType: string; scope: string; expiresAtMs: number | null };
            const saved = { ...memory, content: request.content, memoryType: request.memoryType, scopeType: request.scope.split(":")[0], scopeId: request.scope.split(":")[1] ?? null, expiresAtMs: request.expiresAtMs };
            harness.__savedMemory = saved;
            return { memory: saved, deduplicated: false };
          }
          case "review_memory_candidate":
            return { ...candidate, status: args?.decision === "accept" ? "accepted" : "rejected" };
          case "delete_memory":
            return { ...memory, status: "deleted" };
          case "set_memory_settings": {
            const request = args?.request as { enabled: boolean; autoAcceptHighConfidence: boolean; defaultTtlDays: number; autoExtractionDisclosureAccepted?: boolean } | undefined;
            if (!request || Object.keys(request).some((key) => key.includes("_")) || typeof request.enabled !== "boolean" || typeof request.autoAcceptHighConfidence !== "boolean" || typeof request.defaultTtlDays !== "number" || (request.autoExtractionDisclosureAccepted !== undefined && typeof request.autoExtractionDisclosureAccepted !== "boolean")) throw new Error("set_memory_settings expects camelCase request fields");
            memorySettings = { ...memorySettings, enabled: request.enabled, autoAcceptHighConfidence: request.autoAcceptHighConfidence, defaultTtlDays: request.defaultTtlDays, ...(request.autoExtractionDisclosureAccepted === true ? { autoExtractionConsentVersion: 2, captureAfterMs: Date.now() } : {}) };
            return memorySettings;
          }
          case "set_memory_enabled":
            memorySettings = { ...memorySettings, enabled: Boolean(args?.enabled) };
            return memorySettings;
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
  }, options);
}

async function openMemoryPage(page: import("@playwright/test").Page) {
  await page.setViewportSize({ width: 1280, height: 820 });
  await page.goto("/");
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: /^记忆/ }).click();
  await expect(settings.getByRole("heading", { name: "手动新增记忆" })).toBeVisible();
  await expect(settings.getByRole("button", { name: "刷新记忆" })).toBeEnabled();
  return settings;
}

async function memoryCalls(page: import("@playwright/test").Page) {
  return page.evaluate(() => (window as unknown as { __memoryCalls: Array<{ command: string; args: Record<string, unknown> }> }).__memoryCalls);
}

test("memory page reviews candidates and deletes with the host confirmation token", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);

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

test("manual memory uses host scopes and an explicit expiry", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);
  const scopes = settings.getByLabel("新增记忆作用域", { exact: true });
  await expect(scopes.locator("option")).toHaveText(["用户（全局）", "当前项目（宿主）", "当前工作区（宿主）"]);
  await scopes.selectOption("project:host-project");
  await expect(settings.getByRole("button", { name: "刷新记忆" })).toBeEnabled();
  await settings.getByLabel("新增记忆类型", { exact: true }).selectOption("constraint");
  await settings.getByLabel("新增记忆有效期天数", { exact: true }).fill("30");
  await settings.getByLabel("新增记忆内容", { exact: true }).fill("Project builds must remain reproducible");
  const before = Date.now();
  await settings.getByRole("button", { name: "新增记忆", exact: true }).click();
  await expect(settings.getByRole("status")).toContainText("记忆已保存");
  await expect(settings.getByText("Project builds must remain reproducible", { exact: true })).toBeVisible();
  const call = (await memoryCalls(page)).filter((entry) => entry.command === "upsert_memory").pop();
  expect(call?.args).toMatchObject({ request: { scope: "project:host-project", memoryType: "constraint", content: "Project builds must remain reproducible" } });
  const request = call?.args.request as { expiresAtMs: number };
  expect(request.expiresAtMs).toBeGreaterThanOrEqual(before + 30 * 86_400_000);
  expect(request.expiresAtMs).toBeLessThanOrEqual(Date.now() + 30 * 86_400_000);
  await expect(settings.getByLabel("新增记忆内容", { exact: true })).toHaveValue("");
});

test("enabling memory requires disclosure and never enables auto accept implicitly", async ({ page }) => {
  await installMemoryStub(page, { enabled: false, legacyConsent: true });
  const settings = await openMemoryPage(page);
  await settings.getByRole("checkbox", { name: /记忆已停用/ }).click();
  const disclosure = settings.getByRole("dialog", { name: "确认开启自动记忆提取" });
  await expect(disclosure).toContainText("不扫描历史聊天");
  await expect(disclosure).toContainText("脱敏摘要将外发");
  await expect(disclosure).toContainText("额外 Token 费用");
  await disclosure.getByRole("button", { name: "取消", exact: true }).click();
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_settings" || call.command === "set_memory_enabled")).toHaveLength(0);
  await expect(settings.getByRole("checkbox", { name: /记忆已停用/ })).not.toBeChecked();
  await settings.getByRole("checkbox", { name: /记忆已停用/ }).click();
  await disclosure.getByRole("button", { name: "同意外发摘要并开启" }).click();
  await expect(disclosure).not.toBeVisible();
  await expect(settings.getByRole("checkbox", { name: /记忆已启用/ })).toBeChecked();
  await expect(settings.getByRole("checkbox", { name: "自动接受高置信候选" })).not.toBeChecked();
  const calls = await memoryCalls(page);
  expect(calls.filter((call) => call.command === "set_memory_enabled")).toHaveLength(0);
  expect(calls.filter((call) => call.command === "set_memory_settings").pop()?.args).toEqual({ request: { enabled: true, autoAcceptHighConfidence: false, defaultTtlDays: 0, autoExtractionDisclosureAccepted: true } });
  expect(calls.filter((call) => call.command === "get_memory_scopes").pop()?.args).toEqual({ threadId: null });
  expect(calls.filter((call) => call.command === "get_memory_diagnostics").pop()?.args).toEqual({});
});

test("legacy enabled memory stays enabled while upgrading consent", async ({ page }) => {
  await installMemoryStub(page, { legacyConsent: true });
  const settings = await openMemoryPage(page);
  await expect(settings.getByText("自动提取：不可运行", { exact: true })).toBeVisible();
  await expect(settings.getByText("采集起点：尚未确认", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "升级确认自动提取" }).click();
  await settings.getByRole("dialog", { name: "确认开启自动记忆提取" }).getByRole("button", { name: "同意外发摘要并开启" }).click();
  await expect(settings.getByRole("button", { name: "升级确认自动提取" })).toHaveCount(0);
  await expect(settings.getByText("自动提取：已授权，等待宿主调度", { exact: true })).toBeVisible();
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_settings").pop()?.args).toMatchObject({ request: { enabled: true, autoExtractionDisclosureAccepted: true } });
  await settings.getByRole("checkbox", { name: /记忆已启用/ }).uncheck();
  await expect(settings.getByText("自动提取：不可运行", { exact: true })).toBeVisible();
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_enabled").pop()?.args).toEqual({ enabled: false });
});

test("v1 consent must upgrade even when Dream disclosure was previously accepted", async ({ page }) => {
  await installMemoryStub(page, { consentVersion: 1, dreamEnabled: true });
  const settings = await openMemoryPage(page);
  const maintenance = settings.locator("section").filter({ has: page.getByRole("heading", { name: "自动维护与 Dream", exact: true }) });
  await expect(settings.getByRole("checkbox", { name: /记忆已启用/ })).toBeChecked();
  await expect(settings.getByText("自动提取：不可运行", { exact: true })).toBeVisible();
  await expect(maintenance.getByText("Dream：不可运行", { exact: false })).toContainText("记忆授权需要升级到 v2");
  await expect(maintenance.getByRole("checkbox", { name: "启用 Dream", exact: true })).toBeChecked();
  await maintenance.getByRole("button", { name: "升级记忆授权以运行 Dream" }).click();
  const disclosure = settings.getByRole("dialog", { name: "确认开启自动记忆提取" });
  await expect(disclosure).toContainText("授权升级到 v2");
  await disclosure.getByRole("button", { name: "取消", exact: true }).click();
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_settings" || call.command === "accept_memory_maintenance_disclosure")).toHaveLength(0);
  await maintenance.getByRole("button", { name: "升级记忆授权以运行 Dream" }).click();
  await disclosure.getByRole("button", { name: "同意外发摘要并开启" }).click();
  await expect(disclosure).not.toBeVisible();
  await expect(maintenance.getByText("Dream：已授权，等待宿主调度", { exact: false })).toBeVisible();
  await expect(settings.getByRole("checkbox", { name: "自动接受高置信候选", exact: true })).not.toBeChecked();
  const calls = await memoryCalls(page);
  expect(calls.filter((call) => call.command === "set_memory_settings").pop()?.args).toEqual({ request: { enabled: true, autoAcceptHighConfidence: false, defaultTtlDays: 0, autoExtractionDisclosureAccepted: true } });
  expect(calls.filter((call) => call.command === "accept_memory_maintenance_disclosure" || call.command === "set_memory_maintenance_settings")).toHaveLength(0);
});

test("slow scope results cannot overwrite a newer selection", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);
  await page.evaluate(() => { (window as unknown as { __delayScope: boolean }).__delayScope = true; });
  await settings.getByLabel("记忆作用域", { exact: true }).selectOption("project:host-project");
  await expect.poll(async () => (await memoryCalls(page)).filter((call) => call.command === "list_memories" && call.args.scope === "project:host-project").length).toBeGreaterThan(0);
  await settings.getByLabel("记忆作用域", { exact: true }).selectOption("workspace:host-workspace");
  await expect(settings.getByText("memory in workspace:host-workspace", { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as unknown as { __releaseScope: () => void }).__releaseScope(); });
  await expect(settings.getByLabel("记忆作用域", { exact: true })).toHaveValue("workspace:host-workspace");
  await expect(settings.getByText("memory in project:host-project", { exact: true })).toHaveCount(0);
  await expect(settings.getByText("memory in workspace:host-workspace", { exact: true })).toBeVisible();
});

test("an older host scope response cannot replace the current options", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);
  const before = (await memoryCalls(page)).filter((call) => call.command === "get_memory_scopes").length;
  await page.evaluate(() => { (window as unknown as { __delayScopeOptions: boolean }).__delayScopeOptions = true; });
  await settings.getByRole("button", { name: "刷新记忆" }).click();
  await expect.poll(async () => (await memoryCalls(page)).filter((call) => call.command === "get_memory_scopes").length).toBeGreaterThan(before);
  await settings.getByLabel("记忆作用域", { exact: true }).selectOption("project:host-project");
  await expect(settings.getByText("memory in project:host-project", { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as unknown as { __releaseScopeOptions: () => void }).__releaseScopeOptions(); });
  await expect(settings.getByLabel("记忆作用域", { exact: true })).toHaveValue("project:host-project");
  await expect(settings.getByLabel("记忆作用域", { exact: true }).locator("option")).toHaveText(["用户（全局）", "当前项目（宿主）", "当前工作区（宿主）"]);
});

test("revision only changes refresh repository data and unsubscribe on unmount", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);
  await expect(settings.getByText("排队 2 · 运行中 0 · 已完成 3 · 失败 1 · 已跳过 4", { exact: true })).toBeVisible();
  await expect(settings.getByText("Dream 待处理摘要：2", { exact: true })).toBeVisible();
  await expect(settings.getByText("候选 4 · 已接受 1 · 待审核 2 · 已抑制 1", { exact: true })).toBeVisible();
  await expect(settings.getByText("最近原因：no_new_turns", { exact: true })).toBeVisible();
  await expect(settings.getByText("候选尚未保存为生效记忆", { exact: false })).toBeVisible();
  const before = (await memoryCalls(page)).filter((call) => call.command === "get_memory_diagnostics").length;
  await settings.getByRole("button", { name: "刷新记忆" }).click();
  await expect.poll(async () => (await memoryCalls(page)).filter((call) => call.command === "get_memory_diagnostics").length).toBeGreaterThan(before);
  await page.evaluate(() => { (window as unknown as { __emitMemoryChanged: () => void }).__emitMemoryChanged(); });
  await expect(settings.getByText("updated candidate from repository", { exact: true })).toBeVisible();
  const listenCalls = (await memoryCalls(page)).filter((call) => call.command === "plugin:event|listen" && call.args.event === "memory:changed");
  const listener = listenCalls[listenCalls.length - 1];
  expect(listener).toBeTruthy();
  await settings.getByRole("button", { name: /^外观/ }).click();
  await expect.poll(async () => (await memoryCalls(page)).some((call) => call.command === "plugin:event|unlisten" && call.args.eventId === listener.args.handler)).toBe(true);
});

test("a subscription resolving after unmount is still released", async ({ page }) => {
  await installMemoryStub(page, { listenDelayMs: 600 });
  const settings = await openMemoryPage(page);
  await settings.getByRole("button", { name: /^外观/ }).click();
  await expect.poll(async () => {
    const calls = await memoryCalls(page);
    const listens = calls.filter((call) => call.command === "plugin:event|listen" && call.args.event === "memory:changed");
    return listens.length > 0 && listens.every((listener) => calls.some((call) => call.command === "plugin:event|unlisten" && call.args.eventId === listener.args.handler));
  }).toBe(true);
});

test("capture recovery failures remain visible instead of appearing as an empty store", async ({ page }) => {
  await installMemoryStub(page, { recoveryError: "capture_recovery_failed" });
  const settings = await openMemoryPage(page);
  await expect(settings.getByRole("alert")).toContainText("摘要作业恢复失败：capture_recovery_failed");
  await expect(settings.getByText("自动提取：不可运行", { exact: true })).toBeVisible();
});

test("provider diagnostics explain why authorized extraction and Dream cannot run", async ({ page }) => {
  await installMemoryStub(page, { providerUnavailableReason: "provider_unavailable", dreamEnabled: true });
  const settings = await openMemoryPage(page);
  await expect(settings.getByText("自动提取：不可运行", { exact: true })).toBeVisible();
  await expect(settings.getByText("Dream：不可运行", { exact: false })).toContainText("模型供应商不可用：provider_unavailable");
  await expect(settings.getByText("模型供应商不可用：provider_unavailable；不会启动后台模型调用。", { exact: true })).toBeVisible();
  await expect(settings.getByRole("button", { name: "升级确认自动提取" })).toHaveCount(0);
});

test("refresh preserves an unsaved independent auto acceptance choice", async ({ page }) => {
  await installMemoryStub(page);
  const settings = await openMemoryPage(page);
  const autoAccept = settings.getByRole("checkbox", { name: "自动接受高置信候选" });
  await autoAccept.check();
  await settings.getByRole("spinbutton", { name: "默认有效期天数" }).fill("30");
  await settings.getByRole("button", { name: "刷新记忆" }).click();
  await expect(settings.getByRole("button", { name: "刷新记忆" })).toBeEnabled();
  await expect(autoAccept).toBeChecked();
  await expect(settings.getByRole("spinbutton", { name: "默认有效期天数" })).toHaveValue("30");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect.poll(async () => (await memoryCalls(page)).filter((call) => call.command === "set_memory_settings").length).toBe(1);
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_settings").pop()?.args).toEqual({ request: { enabled: true, autoAcceptHighConfidence: true, defaultTtlDays: 30 } });
});

test("Dream disclosure keeps the existing maintenance APIs and cancellation", async ({ page }) => {
  await installMemoryStub(page, { maintenanceDelayMs: 1500 });
  const settings = await openMemoryPage(page);
  await expect(settings.getByText("Token 预算不是价格或费用上限", { exact: false })).toBeVisible();
  await settings.getByRole("checkbox", { name: "启用 Dream", exact: true }).click();
  const disclosure = settings.getByRole("dialog", { name: "确认 Dream 摘要外发" });
  await expect(disclosure).toContainText("任务摘要及已保存记忆");
  await expect(disclosure).toContainText("额外 Token 费用");
  await disclosure.getByRole("button", { name: "取消", exact: true }).click();
  expect((await memoryCalls(page)).filter((call) => call.command === "accept_memory_maintenance_disclosure")).toHaveLength(0);
  await settings.getByRole("checkbox", { name: "启用 Dream", exact: true }).click();
  await disclosure.getByRole("button", { name: "同意 Dream 外发披露" }).click();
  await expect(disclosure).not.toBeVisible();
  await settings.getByRole("checkbox", { name: "自动维护", exact: true }).check();
  await settings.getByLabel("Dream Token 预算", { exact: true }).fill("2048");
  await settings.getByLabel("空闲后维护分钟", { exact: true }).fill("10");
  await settings.getByRole("button", { name: "保存维护设置" }).click();
  await expect(settings.getByRole("button", { name: "保存维护设置" })).toBeEnabled();
  expect((await memoryCalls(page)).filter((call) => call.command === "set_memory_maintenance_settings").pop()?.args).toEqual({ request: { enabled: true, dreamEnabled: true, remoteDisclosureAccepted: true, tokenBudget: 2048, idleAfterMs: 600_000 } });
  await settings.getByRole("button", { name: "立即运行维护" }).click();
  await expect(settings.getByRole("button", { name: "取消维护" })).toBeEnabled();
  await settings.getByRole("button", { name: "取消维护" }).click();
  await expect.poll(async () => (await memoryCalls(page)).some((call) => call.command === "cancel_memory_maintenance")).toBe(true);
  await expect(settings.getByRole("status")).toContainText("待审核 1（尚未保存）");
});
