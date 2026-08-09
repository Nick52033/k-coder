import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 1;
    let agentEventCallbackId: number | null = null;
    const thread = { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false };
    const secondThread = { schemaVersion: 1, id: "thread-2", title: "Parallel conversation", createdAtMs: 3, updatedAtMs: 3, archived: false };
    const openAiProvider = { schemaVersion: 1, id: "openai", kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }, { id: "gpt-4o", displayName: "GPT-4 Omni", contextWindow: 64000, fallback: false }], endpoints: [], hasApiKey: true };
    const ziccProvider = { schemaVersion: 1, id: "zicc", kind: "open_ai_compatible", transport: "open_ai_responses", name: "zicc", baseUrl: "https://zicc.example.com/v1", model: "gpt-5.6-terra", models: [{ id: "gpt-5.6-terra", displayName: "gpt-5.6-terra", contextWindow: 128000, fallback: false }, { id: "gpt-5.5", displayName: "gpt-5.5", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: true };
    const pendingProvider = { schemaVersion: 1, id: "pending", kind: "open_ai_compatible", transport: "anthropic_messages", name: "待配置供应商", baseUrl: "https://pending.example.com/v1", model: "claude-test", models: [{ id: "claude-test", displayName: "Claude Test", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: false };
    let providerCatalog: { schemaVersion: number; activeProviderId: string | null; providers: Array<typeof openAiProvider> } = { schemaVersion: 1, activeProviderId: "openai", providers: [openAiProvider, ziccProvider, pendingProvider] };
    let approvalMode: "ask" | "full_access" = "ask";
    let reasoningEffort: "off" | "minimal" | "low" | "medium" | "high" | "x_high" = "medium";
    let workspaceState = { current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 }, recent: [] as Array<{ id: string; name: string; path: string; trusted: boolean; lastOpenedAtMs: number }> };
    const runTurnCalls: unknown[] = [];
    let startedTurnCount = 0;
    const activeTurnIds = new Map<string, string>();
    const mailboxByThread = new Map<string, Array<{
      turnId: string;
      threadId: string;
      kind: "message" | "retry";
      input: string;
      agentMode: string | null;
      attachments: unknown[];
    }>>();
    const invocationArgs: Record<string, unknown> = {};
    const ptyStartRequests: unknown[] = [];
    const ptyWrites: string[] = [];
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
      create_thread: secondThread,
      recognize_image: { text: "hidden OCR fixture", lineCount: 1, durationMs: 12 },
      list_threads: [thread],
      read_thread: { schemaVersion: 1, summary: thread, messages: [
        { schemaVersion: 1, id: "message-user", role: "user", content: [{ type: "text", text: "检查工作区" }], createdAtMs: 1 },
        { schemaVersion: 1, id: "message-assistant", role: "assistant", content: [{ type: "text", text: "检查完成。" }], createdAtMs: 2 },
      ], messageTurnIds: { "message-assistant": "turn-1" }, lastTurn: null, toolActivities: [
        { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1000, completedAtMs: 1200, durationMs: 200 },
        { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 },
        { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { command: "pnpm build", cwd: ".", timeoutMs: 120000 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530, shell: "powershell" } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 },
      ], turnTimeline: [
        { type: "event", itemId: "provider-context-1", turnId: "turn-1", kind: "provider_context", title: "已保留模型上下文", detail: "openai_responses · reasoning · rs_fixture" },
        { type: "event", itemId: "usage-1", turnId: "turn-1", kind: "usage", title: "模型调用 1 用量", detail: "输入 1200 · 输出 80 · 总计 1280 tokens" },
        { type: "text", id: "progress-1", turnId: "turn-1", text: "我先检查相关文件并修改实现。" },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1000, completedAtMs: 1200, durationMs: 200 } },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 } },
        { type: "text", id: "progress-2", turnId: "turn-1", text: "修改完成，接着运行验证。" },
        { type: "tool", activity: { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { command: "pnpm build", cwd: ".", timeoutMs: 120000 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530, shell: "powershell" } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 } },
        { type: "text", id: "message-assistant", turnId: "turn-1", text: "检查完成。" },
        { type: "event", itemId: "turn-completed-turn-1", turnId: "turn-1", kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 1830 },
      ], approvals: [], changes: [] },
      workspace_state: { current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 }, recent: [] },
      list_workspace_directory: [
        { name: "src", path: "src", isDirectory: true, size: null, modifiedAtMs: 2 },
        { name: "README.md", path: "README.md", isDirectory: false, size: 120, modifiedAtMs: 2 },
      ],
      search_workspace_files: [
        { name: "App.tsx", path: "src/App.tsx", isDirectory: false, size: 240, modifiedAtMs: 2 },
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
        skills: [
          { name: "workspace-review", description: "Built-in workspace review", path: "D:\\apps\\k-coder\\resources\\skills\\workspace-review\\SKILL.md", scope: "builtin", risk: "read", triggers: ["workspace review"], enabled: true },
          { name: "review", description: "Review code safely", path: "D:\\code\\k-coder\\.k-coder\\skills\\review\\SKILL.md", scope: "project", risk: "read", triggers: ["review"], enabled: true },
        ],
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
          if (command === "workspace_state") {
            const forcedPath = localStorage.getItem("kcoder_e2e_workspace_path");
            if (forcedPath) {
              workspaceState = {
                ...workspaceState,
                current: {
                  ...workspaceState.current,
                  name: forcedPath.split(/[/\\]/).filter(Boolean).pop() ?? forcedPath,
                  path: forcedPath,
                },
              };
            }
            return workspaceState;
          }
          if (command === "switch_workspace") {
            const path = String(args?.path ?? "");
            const current = {
              id: `project-${path}`,
              name: path.split(/[/\\]/).filter(Boolean).pop() ?? path,
              path,
              trusted: Boolean(args?.trusted),
              lastOpenedAtMs: Date.now(),
            };
            workspaceState = { current, recent: [current, ...workspaceState.recent] };
            localStorage.setItem("kcoder_e2e_workspace_path", path);
            return current;
          }
          if (command === "get_provider_catalog") return providerCatalog;
          if (command === "turn_start") {
            runTurnCalls.push(args ?? null);
            startedTurnCount += 1;
            const request = args?.request as { threadId?: string; input?: string; agentMode?: string } | undefined;
            const threadId = String(request?.threadId ?? "thread-1");
            const turnId = `turn-start-${startedTurnCount}`;
            const queued = activeTurnIds.has(threadId);
            const attachments = (args?.attachments as Array<{
              name: string;
              dataUrl: string;
              ocrText?: string;
            }> | undefined) ?? [];
            if (queued) {
              mailboxByThread.set(threadId, [
                ...(mailboxByThread.get(threadId) ?? []),
                {
                  schemaVersion: 1,
                  turnId,
                  threadId,
                  kind: "message",
                  input: String(request?.input ?? ""),
                  agentMode: request?.agentMode ?? null,
                  attachments,
                },
              ]);
            } else {
              activeTurnIds.set(threadId, turnId);
              const input = String(request?.input ?? "").trim();
              const content: Array<Record<string, unknown>> = input
                ? [{ type: "text", text: input }]
                : [{ type: "context", text: "请分析用户提供的图片。" }];
              for (const attachment of attachments) {
                if (attachment.ocrText?.trim()) {
                  content.push({
                    type: "context",
                    text: `\n\n[图片文字识别: ${attachment.name}]\n${attachment.ocrText.trim()}`,
                  });
                }
                content.push({
                  type: "image",
                  name: attachment.name,
                  dataUrl: attachment.dataUrl,
                });
              }
              if (agentEventCallbackId === null) throw new Error("agent-event listener is not ready");
              callbacks.get(agentEventCallbackId)?.({
                event: "agent-event",
                id: 1,
                payload: {
                  schemaVersion: 4,
                  threadId,
                  turnId,
                  type: "turn_started",
                  phase: "exploring",
                  userMessage: {
                    schemaVersion: 1,
                    id: `user-${turnId}`,
                    role: "user",
                    content,
                    createdAtMs: Date.now(),
                  },
                },
              });
            }
            return {
              schemaVersion: 1,
              threadId,
              turnId,
              state: queued ? "queued" : "streaming",
            };
          }
          if (command === "turn_retry") {
            startedTurnCount += 1;
            const threadId = String(args?.threadId ?? "thread-1");
            const turnId = `turn-retry-${startedTurnCount}`;
            const queued = activeTurnIds.has(threadId);
            if (queued) {
              mailboxByThread.set(threadId, [
                ...(mailboxByThread.get(threadId) ?? []),
                {
                  schemaVersion: 1,
                  turnId,
                  threadId,
                  kind: "retry",
                  input: "",
                  agentMode: null,
                  attachments: [],
                },
              ]);
            } else {
              activeTurnIds.set(threadId, turnId);
            }
            return {
              schemaVersion: 1,
              threadId,
              turnId,
              state: queued ? "queued" : "streaming",
            };
          }
          if (command === "read_thread_mailbox") {
            const threadId = String(args?.threadId ?? "thread-1");
            return {
              schemaVersion: 1,
              threadId,
              activeTurnId: activeTurnIds.get(threadId) ?? null,
              pending: mailboxByThread.get(threadId) ?? [],
            };
          }
          if (command === "remove_queued_turn") {
            const threadId = String(args?.threadId ?? "");
            const turnId = String(args?.turnId ?? "");
            const pending = mailboxByThread.get(threadId) ?? [];
            const next = pending.filter((item) => item.turnId !== turnId);
            mailboxByThread.set(threadId, next);
            return next.length !== pending.length;
          }
          if (command === "clear_thread_mailbox") {
            const threadId = String(args?.threadId ?? "");
            const removed = mailboxByThread.get(threadId)?.length ?? 0;
            mailboxByThread.set(threadId, []);
            return removed;
          }
          if (command === "turn_steer") {
            const request = args?.request as { threadId: string; expectedTurnId: string };
            return { schemaVersion: 1, threadId: request.threadId, turnId: request.expectedTurnId };
          }
          if (command === "turn_steer_queued") {
            const request = args?.request as {
              threadId: string;
              expectedTurnId: string;
              queuedTurnId: string;
            };
            const pending = mailboxByThread.get(request.threadId) ?? [];
            const queued = pending.find((item) => item.turnId === request.queuedTurnId);
            if (!queued || queued.kind !== "message") {
              throw new Error("queued turn is not an available message");
            }
            mailboxByThread.set(
              request.threadId,
              pending.filter((item) => item.turnId !== request.queuedTurnId),
            );
            return {
              schemaVersion: 1,
              threadId: request.threadId,
              turnId: request.expectedTurnId,
            };
          }
          if (command === "turn_interrupt") {
            if (localStorage.getItem("kcoder_e2e_hold_cancel") === "true") {
              return new Promise(() => undefined);
            }
            return null;
          }
          if (command === "cancel_turn") {
            if (localStorage.getItem("kcoder_e2e_hold_cancel") === "true") {
              return new Promise(() => undefined);
            }
            return true;
          }
          if (command === "list_threads") {
            const configured = localStorage.getItem("kcoder_e2e_threads");
            if (configured) return JSON.parse(configured);
          }
          if (
            (command === "get_plan" || command === "get_goal")
            && String(args?.threadId ?? "") === localStorage.getItem("kcoder_e2e_empty_thread_id")
          ) {
            return null;
          }
          if (command === "read_thread_history") {
            const recovered = localStorage.getItem("kcoder_e2e_thread_history");
            if (recovered) return JSON.parse(recovered);
            return null;
          }
          if (command === "list_thread_turns") {
            const page = localStorage.getItem("kcoder_e2e_thread_turns_page");
            if (page) return JSON.parse(page);
          }
          if (command === "read_thread") {
            const delayMs = Number(localStorage.getItem("kcoder_e2e_read_delay_ms") ?? 0);
            if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
            const detailsByThread = localStorage.getItem("kcoder_e2e_thread_detail_by_id");
            if (detailsByThread) {
              const detail = (JSON.parse(detailsByThread) as Record<string, unknown>)[String(args?.threadId ?? "")];
              if (detail) return detail;
            }
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
          if (command === "start_pty") {
            ptyStartRequests.push(args?.request ?? null);
            const exited = localStorage.getItem("kcoder_e2e_pty_state") === "exited";
            return {
              id: `pty-${ptyStartRequests.length}`,
              state: exited ? { state: "exited", code: 0 } : { state: "running" },
              startedAtMs: 1,
              finishedAtMs: exited ? 2 : null,
              rows: 24,
              cols: 80,
              nextCursor: 1,
              oldestCursor: 0,
              outputTruncated: false,
            };
          }
          if (command === "pty_status") {
            const exited = localStorage.getItem("kcoder_e2e_pty_state") === "exited";
            return {
              id: String(args?.sessionId ?? "pty-1"),
              state: exited ? { state: "exited", code: 0 } : { state: "running" },
              startedAtMs: 1,
              finishedAtMs: exited ? 2 : null,
              rows: 24,
              cols: 80,
              nextCursor: 1,
              oldestCursor: 0,
              outputTruncated: false,
            };
          }
          if (command === "read_pty_output") {
            const cursor = Number(args?.cursor ?? 0);
            const chunks = cursor === 0 ? [{ cursor: 0, text: "PS D:\\code\\k-coder> " }] : [];
            return { chunks, nextCursor: cursor === 0 ? 1 : cursor, oldestCursor: 0, truncatedBeforeCursor: false };
          }
          if (command === "write_pty") {
            ptyWrites.push(String(args?.input ?? ""));
            return undefined;
          }
          if (command === "resize_pty" || command === "close_pty") return undefined;
          return responses[command] ?? null;
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
      __invoked: [],
      __invocationArgs: invocationArgs,
      __ptyStartRequests: ptyStartRequests,
      __ptyWrites: ptyWrites,
      __runTurnCalls: runTurnCalls,
      __lastProviderRequest: null,
      __lastActivatedProvider: null,
      __lastApprovalMode: null,
      __emitAgentEvent: (event: unknown) => {
        const agentEvent = event as { type?: string; threadId?: string; turnId?: string };
        if (agentEvent.threadId && agentEvent.turnId && agentEvent.type === "turn_started") {
          activeTurnIds.set(agentEvent.threadId, agentEvent.turnId);
        } else if (
          agentEvent.threadId
          && ["turn_completed", "turn_failed", "turn_cancelled"].includes(agentEvent.type ?? "")
        ) {
          activeTurnIds.delete(agentEvent.threadId);
        }
        if (agentEventCallbackId === null) throw new Error("agent-event listener is not ready");
        callbacks.get(agentEventCallbackId)?.({ event: "agent-event", id: 1, payload: event });
      },
    });
  });
});

test("supports the primary workbench inspection flow", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("textbox", { name: "消息" })).toHaveCSS("font-size", "13px");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_plan)).toEqual({ threadId: "thread-1" });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_goal)).toEqual({ threadId: "thread-1" });
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await expect(page.getByText("检查完成。", { exact: true })).toBeVisible();
  await expect(page.getByText("执行了 1.8s", { exact: true })).toBeVisible();
  await expect(page.locator(".conversation-header").getByText("1280 tokens", { exact: true })).toHaveCount(0);
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeHidden();
  await page.screenshot({ path: testInfo.outputPath("collapsed-turn.png"), fullPage: true });
  await page.getByText("执行了 1.8s", { exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath("collapsed-steps.png"), fullPage: true });
  await expect(page.locator(".turn-event-step--provider_context")).toHaveCount(0);
  await expect(page.locator(".turn-event-step--usage")).toHaveCount(0);
  await expect(page.locator(".turn-plan")).not.toHaveAttribute("open", "");
  await expect(page.locator(".turn-plan").getByText("检查工作区", { exact: true })).toBeHidden();
  await page.locator(".turn-plan > summary").click();
  await expect(page.locator(".turn-plan").getByText("检查工作区", { exact: true })).toBeVisible();
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeVisible();
  const inspectionGroup = page.locator(".turn-tool-group").filter({ hasText: "执行了多个操作" });
  await expect(inspectionGroup).toBeVisible();
  await expect(page.locator(".turn-timeline-tool").getByText("应用补丁 src/App.css", { exact: true })).toBeHidden();
  await inspectionGroup.locator(":scope > summary").click();
  await expect(page.locator(".turn-timeline-tool").getByText("应用补丁 src/App.css", { exact: true })).toBeVisible();
  await page.getByText("查看补丁", { exact: true }).click();
  const patchEditor = page.locator(".turn-tool-details--file").filter({ hasText: "查看补丁" });
  // dev server 下 Monaco 以未优化的原生 ESM 加载（optimizeDeps 排除 monaco-editor），
  // 首个 diff 编辑器需要完成整棵模块树的瀑布请求，放宽首次可见等待时间。
  await expect(patchEditor.locator('.code-editor[data-language="diff"] .monaco-editor')).toBeVisible({ timeout: 20000 });
  await expect(patchEditor.locator(".view-lines")).toContainText("*** Update File: src/App.css");
  const readTool = page.locator(".turn-timeline-tool").filter({ hasText: "读取 src/stores/workbenchStore.ts L42" });
  await expect(readTool).toBeVisible();
  await expect(readTool.getByText("查看读取内容", { exact: true })).toHaveCount(0);
  await expect(readTool.locator(".turn-tool-details")).toHaveCount(0);
  await expect(page.locator(".turn-file-editor")).toHaveCount(0);
  await expect(readTool.locator(".code-editor")).toHaveCount(0);
  await expect(page.getByText("修改完成，接着运行验证。", { exact: true })).toBeVisible();
  const commandGroup = page.locator(".turn-tool-group").filter({ hasText: "运行了命令" }).first();
  await expect(commandGroup).toBeVisible();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeHidden();
  await commandGroup.locator(":scope > summary").click();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeVisible();
  await expect(page.getByText("3 个操作", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("inline-plan-and-tools.png"), fullPage: true });
  await page.evaluate(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.reload();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeHidden();
  await page.getByText("执行了 1.8s", { exact: true }).click();
  await page.locator(".turn-tool-group").filter({ hasText: "运行了命令" }).first().locator(":scope > summary").click();
  await expect(page.locator(".turn-timeline-tool").getByText("执行 pnpm build", { exact: true })).toBeVisible();
  const commandDetails = page.locator(".turn-command-inline").last();
  await expect(commandDetails.locator(".turn-command-inline-header")).toContainText("PowerShell");
  await expect(commandDetails.locator("pre")).toContainText("pnpm build");
  await page.screenshot({ path: testInfo.outputPath("inline-plan-and-tools-dark.png"), fullPage: true });
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  const readmeRow = page.getByRole("button", { name: /README.md/ });
  await expect(readmeRow).toHaveCSS("font-size", "12px");
  await readmeRow.click();
  await expect(readmeRow).toHaveAttribute("aria-current", "true");
  const previewDialog = page.getByRole("dialog", { name: "预览 README.md" });
  await expect(previewDialog).toBeVisible();
  const previewBackdrop = page.locator(".file-preview-backdrop");
  const viewport = page.viewportSize();
  const backdropBox = await previewBackdrop.boundingBox();
  const dialogBox = await previewDialog.boundingBox();
  expect(viewport).not.toBeNull();
  expect(backdropBox).toMatchObject({ x: 0, y: 48, width: viewport!.width, height: viewport!.height - 48 });
  expect(dialogBox).not.toBeNull();
  expect(Math.abs((dialogBox!.x + dialogBox!.width / 2) - (viewport!.width / 2))).toBeLessThanOrEqual(1);
  expect(Math.abs((dialogBox!.y + dialogBox!.height / 2) - ((viewport!.height + 48) / 2))).toBeLessThanOrEqual(1);
  await page.screenshot({ path: testInfo.outputPath("workspace-markdown-preview-centered.png"), fullPage: true });
  await previewDialog.getByRole("button", { name: "源码", exact: true }).click();
  const editor = previewDialog.locator(".monaco-editor");
  await expect(editor).toBeVisible();
  await expect.poll(async () => {
    const box = await previewDialog.locator(".code-editor").boundingBox();
    return box ? { width: Math.round(box.width), height: Math.round(box.height) } : null;
  }).toMatchObject({ width: expect.any(Number), height: expect.any(Number) });
  const editorBox = await previewDialog.locator(".code-editor").boundingBox();
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
  await previewDialog.getByRole("button", { name: "关闭预览" }).click();
  await expect(previewDialog).toHaveCount(0);
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
  await expect(page.getByText("Built-in workspace review")).toBeVisible();
  await expect(page.getByText("内置 · workspace review")).toBeVisible();
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

test("streams thinking, safe reasoning summaries, command output, and file diffs inline", async ({ page, context }, testInfo) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await page.evaluate(() => {
    localStorage.setItem("kcoder_theme", "dark");
    document.documentElement.dataset.theme = "dark";
  });

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "activity_status_changed", phase: "exploring", status: "thinking" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "rs-live", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live", delta: "Planning parallel cargo check and test" });
  });
  await expect(page.getByText("思考中", { exact: true })).toBeVisible();
  await expect(page.getByText("Planning parallel cargo check and test", { exact: true })).toHaveCount(0);
  await expect(page.locator(".turn-reasoning")).toHaveCount(0);
  await expect(page.getByText("等待工具调用…", { exact: true })).toHaveCount(0);
  await expect(page.locator(".turn-timeline--empty")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("compact-thinking-status.png"), fullPage: true });
  await expect(page.locator(".message-avatar")).toHaveCount(0);
  await expect(page.getByText("正在执行", { exact: true })).toHaveCount(0);

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live", summary: "Planning parallel cargo check and test" });
    emit({ ...base, type: "item_completed", phase: "planning", itemId: "rs-live", itemType: "reasoning", status: "completed" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "rs-live-2", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live-2", delta: "已确认工具输出按游标去重，下一步核对脱敏边界。" });
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live-2", summary: "已确认工具输出按游标去重，下一步核对脱敏边界。" });
    emit({ ...base, type: "item_completed", phase: "planning", itemId: "rs-live-2", itemType: "reasoning", status: "completed" });
    emit({ ...base, type: "item_started", phase: "executing", itemId: "call-live", itemType: "tool" });
    emit({ ...base, type: "tool_started", phase: "executing", call: { id: "call-live", name: "run_command", arguments: { command: "pnpm build" }, metadata: {} } });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stdout", cursor: 0, delta: "building client\n" });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stderr", cursor: 1, delta: "warning: fixture\n" });
    emit({ ...base, type: "tool_completed", phase: "executing", callId: "call-live", name: "run_command", result: { success: true, output: "building client\n", metadata: { durationMs: 1234, shell: "powershell" } } });
    emit({ ...base, type: "item_completed", phase: "executing", itemId: "call-live", itemType: "tool", status: "completed" });
    emit({ ...base, type: "item_started", phase: "executing", itemId: "compaction-live", itemType: "context_compaction" });
    emit({ ...base, type: "context_compacted", phase: "executing", itemId: "compaction-live", automatic: true,
      compactedMessageCount: 18, userConstraintCount: 1, recentToolResultCount: 1 });
    emit({ ...base, type: "item_completed", phase: "executing", itemId: "compaction-live", itemType: "context_compaction", status: "completed" });
    emit({ ...base, type: "item_started", phase: "executing", itemId: "change-live", itemType: "change" });
    emit({ ...base, type: "change_applied", phase: "executing", changeSet: {
      id: "change-live", threadId: "thread-1", turnId: "turn-live", toolCallId: "call-edit-live", createdAtMs: 2,
      undone: false, files: [
        { path: "src/App.tsx", destinationPath: null, operation: "modify", beforeHash: "before", afterHash: "after", beforeContent: "const before = true;\n", afterContent: "const after = true;\n", unifiedDiff: "--- a/src/App.tsx\n+++ b/src/App.tsx\n@@ -1 +1 @@\n-const before = true;\n+const after = true;\n" },
        { path: "src/large.ts", destinationPath: null, operation: "modify", beforeHash: "large-before", afterHash: "large-after", beforeContent: null, afterContent: null, unifiedDiff: "--- a/src/large.ts\n+++ b/src/large.ts\n@@ -1 +1 @@\n-const oldLarge = true;\n+const newLarge = true;\n" },
      ],
    } });
    emit({ ...base, type: "item_completed", phase: "executing", itemId: "change-live", itemType: "change", status: "completed" });
  });

  const reasoning = page.locator(".turn-reasoning").last();
  await expect(page.getByText("思考摘要", { exact: true })).toHaveCount(1);
  await expect(page.getByText("思考内容", { exact: true })).toHaveCount(0);
  await expect(reasoning.locator(".turn-reasoning-segment")).toHaveCount(1);
  await expect(reasoning.getByText("已完成", { exact: true })).toBeVisible();
  await expect(reasoning).not.toHaveAttribute("open", "");
  await reasoning.locator(":scope > summary").click();
  await expect(page.getByText("Planning parallel cargo check and test", { exact: true })).toHaveCount(0);
  await expect(page.getByText("已确认工具输出按游标去重，下一步核对脱敏边界。", { exact: true })).toBeVisible();
  const liveExecution = page.locator(".message--assistant").last().locator(".turn-execution--live");
  await expect(liveExecution.locator(":scope > summary .turn-disclosure-chevron")).toHaveCount(0);
  await expect.poll(async () => {
    const toolBox = await liveExecution.locator(".turn-tool-group").last().boundingBox();
    const statusBox = await liveExecution.locator(":scope > summary").boundingBox();
    return toolBox && statusBox ? statusBox.y >= toolBox.y + toolBox.height : false;
  }).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("grouped-reasoning-summaries.png"), fullPage: true });
  const liveCommandGroup = liveExecution.locator(".turn-tool-group").filter({ hasText: "运行了命令" });
  await expect(liveCommandGroup).not.toHaveAttribute("open", "");
  await liveCommandGroup.locator(":scope > summary").click();
  const liveCommandOutput = page.locator(".turn-tool-output").last();
  await expect(liveCommandOutput).not.toHaveAttribute("open", "");
  await liveCommandOutput.locator(":scope > summary").click();
  await expect(page.locator(".turn-tool-output-line--stdout").filter({ hasText: "building client" })).toBeVisible();
  await expect(page.locator(".turn-tool-output-line--stderr").filter({ hasText: "warning: fixture" })).toBeVisible();
  await expect(page.getByText("耗时 1.2s", { exact: true })).toBeVisible();
  const compactionStep = liveExecution.locator(".turn-event-step--compacted");
  await expect(compactionStep.getByText("已自动压缩上下文", { exact: true })).toBeVisible();
  await expect(compactionStep).not.toHaveAttribute("open", "");
  await compactionStep.locator(":scope > summary").click();
  await expect(compactionStep.getByText("压缩了 18 条历史消息，保留 1 项用户约束和 1 项近期工具结果", { exact: true })).toBeVisible();
  const commandDetails = page.locator(".turn-command-inline").last();
  await expect(commandDetails.locator(".turn-command-inline-header")).toContainText("PowerShell");
  await expect(commandDetails.locator("pre")).toContainText("pnpm build");
  await commandDetails.screenshot({ path: testInfo.outputPath("command-inline.png") });
  const changeStep = liveExecution.locator(".turn-event-step--change_applied");
  await expect(changeStep.getByText("编辑了文件", { exact: true })).toBeVisible();
  await expect(changeStep).not.toHaveAttribute("open", "");
  await changeStep.locator(":scope > summary").click();
  await expect(page.getByText("已编辑 src/App.tsx", { exact: true }).last()).toBeVisible();
  const changeFile = page.locator(".turn-change-file").filter({ hasText: "src/App.tsx" });
  const diffEditor = changeFile.locator('.code-diff-editor[data-language="typescript"]');
  await expect(changeFile.getByText("正在载入编辑器...", { exact: true })).toBeVisible();
  await expect(diffEditor.locator(".monaco-diff-editor")).toBeVisible({ timeout: 15_000 });
  await expect(diffEditor.locator(".monaco-editor").last()).toHaveClass(/vs-dark/);
  await expect(changeFile.locator(".turn-change-editor-header")).toContainText("src/App.tsx+1-1");
  await expect(diffEditor.locator(".view-lines").last()).toContainText("const after = true;");
  const copyDiff = changeFile.getByRole("button", { name: "复制 Diff" });
  await copyDiff.click();
  await expect(changeFile.getByRole("button", { name: "已复制 Diff" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("+const after = true;");
  await changeFile.screenshot({ path: testInfo.outputPath("change-diff-editor.png") });
  await expect(page.getByText("已编辑 src/large.ts", { exact: true })).toBeVisible();
  const boundedChangeFile = page.locator(".turn-change-file").filter({ hasText: "src/large.ts" });
  await expect(boundedChangeFile.locator('.code-editor[data-language="diff"] .monaco-editor')).toBeVisible();
  await expect(boundedChangeFile.locator(".view-lines")).toContainText("+const newLarge = true;");
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

test("copies the user message text with the copy button", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();

  const userMessage = page.locator(".message--user").first();
  const copyButton = userMessage.getByRole("button", { name: "复制消息" });
  await expect(copyButton).toBeVisible();
  await copyButton.click();

  await expect(userMessage.getByRole("button", { name: "已复制" })).toBeVisible();
  const clipboardText = await page.evaluate(() => navigator.clipboard.readText());
  expect(clipboardText).toBe("检查工作区");
});

test("wakes workspace files with @ and enabled Skills with /", async ({ page }) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });

  await composer.fill("@src/");
  await expect(page.locator(".composer-suggestions")).toBeVisible();
  await expect(page.getByRole("option", { name: /src\/App\.tsx/ })).toBeVisible();
  await page.getByRole("option", { name: /src\/App\.tsx/ }).click();
  await expect(composer).toHaveValue("@src/App.tsx ");

  await composer.fill("/re");
  await expect(page.getByRole("option", { name: /\/review/ })).toBeVisible();
  await composer.press("Enter");
  await expect(composer).toHaveValue("/review ");
});

test("paces streamed text and preserves timeline order before tools and completion", async ({ page }) => {
  await page.goto("/");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-paced" };
  const progressText = `${"逐步展示工具前的说明内容。".repeat(18)}\n\n工具前说明终点`;
  const finalText = `${"逐步展示收到的最终回复。".repeat(24)}\n\n流式终点`;

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({ ...base, type: "item_started", phase: "planning", itemId: "message-paced", itemType: "agent_message" });
  await emit({ ...base, type: "text_delta", phase: "responding", itemId: "message-paced-progress", delta: progressText });
  await emit({
    ...base,
    type: "tool_started",
    phase: "executing",
    call: { id: "call-paced", name: "run_command", arguments: { command: "pnpm build" }, metadata: {} },
  });
  await emit({
    ...base,
    type: "tool_completed",
    phase: "executing",
    callId: "call-paced",
    name: "run_command",
    result: { success: true, output: "done", metadata: { durationMs: 120 } },
  });

  const liveMessage = page.locator(".message--assistant").last();
  await expect(liveMessage.locator(".turn-progress-text--typing")).toBeVisible();
  await expect(liveMessage.getByText("工具前说明终点", { exact: true })).toHaveCount(0);
  await expect(liveMessage.getByText("运行了命令", { exact: true })).toHaveCount(0);
  await expect(liveMessage.getByText("生成回复中", { exact: true })).toBeVisible();
  await expect(liveMessage.getByText("工具前说明终点", { exact: true })).toBeVisible({ timeout: 10_000 });
  await expect(liveMessage.getByText("运行了命令", { exact: true })).toBeVisible();

  await emit({ ...base, type: "text_delta", phase: "responding", itemId: "message-paced", delta: finalText });
  await expect(liveMessage.getByText("流式终点", { exact: true })).toHaveCount(0);
  await emit({ ...base, type: "item_completed", phase: "planning", itemId: "message-paced", itemType: "agent_message", status: "completed" });

  await emit({
    ...base,
    type: "turn_completed",
    phase: "complete",
    message: {
      schemaVersion: 1,
      id: "message-paced",
      role: "assistant",
      content: [{ type: "text", text: finalText }],
      createdAtMs: 5,
    },
    usage: null,
    startedAtMs: 1000,
    completedAtMs: 2800,
    durationMs: 1800,
  });

  await expect(liveMessage.locator(".turn-execution--live")).toBeVisible();
  await expect(liveMessage.getByText("生成回复中", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-execution--live")).toHaveCount(0);
  await expect(liveMessage.locator(".turn-final-response").getByText("流式终点", { exact: true })).toBeVisible();
  await expect(liveMessage.getByText("执行了 1.8s", { exact: true })).toBeVisible();
});

test("keeps the active tool open and folds it as soon as it finishes", async ({ page }) => {
  await page.goto("/");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-collapse" };

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({
    ...base,
    type: "tool_started",
    phase: "executing",
    call: { id: "call-collapse", name: "run_command", arguments: { command: "pnpm build" }, metadata: {} },
  });

  const group = page.locator(".turn-tool-group").last();
  await expect(group).toHaveAttribute("open", "");
  const openHeight = await group.evaluate((element) => element.getBoundingClientRect().height);
  expect(openHeight).toBeGreaterThan(0);

  await emit({
    ...base,
    type: "tool_completed",
    phase: "executing",
    callId: "call-collapse",
    name: "run_command",
    result: { success: true, output: "done", metadata: { durationMs: 120 } },
  });

  await expect(group).not.toHaveAttribute("open", "");
  await emit({
    ...base,
    type: "turn_completed",
    phase: "complete",
    message: {
      schemaVersion: 1,
      id: "message-collapse",
      role: "assistant",
      content: [{ type: "text", text: "构建完成。" }],
      createdAtMs: 5,
    },
    usage: null,
    startedAtMs: 1000,
    completedAtMs: 1120,
    durationMs: 120,
  });
  await expect(group).not.toHaveAttribute("open", "");
  await expect(page.locator(".message--assistant").last().locator(".turn-execution")).not.toHaveAttribute("open", "");
});

test("follows streamed growth only while the conversation remains near the latest content", async ({ page }) => {
  await page.goto("/");
  const area = page.locator(".message-area");
  await area.evaluate((element) => {
    const target = element as HTMLElement;
    target.style.height = "220px";
    target.style.minHeight = "220px";
    target.style.maxHeight = "220px";
    target.scrollTop = target.scrollHeight;
    target.dispatchEvent(new Event("scroll"));
  });
  const initialTop = await area.evaluate((element) => element.scrollTop);
  const firstChunk = Array.from(
    { length: 70 },
    (_, index) => `持续输出第 ${index + 1} 段内容，让对话自然向下生长。`,
  ).join("\n\n");
  const emit = (event: Record<string, unknown>) => page.evaluate((payload) => {
    (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent(payload);
  }, event);
  const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-scroll-follow" };

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: firstChunk });
  await expect.poll(() => area.evaluate((element) => element.scrollTop), { timeout: 8_000 })
    .toBeGreaterThan(initialTop + 20);
  await expect.poll(() => area.evaluate((element) =>
    element.scrollHeight - element.clientHeight - element.scrollTop,
  )).toBeLessThanOrEqual(2);

  const paused = await area.evaluate((element) => {
    const target = element as HTMLElement;
    target.scrollTop = Math.max(0, target.scrollTop - 120);
    target.dispatchEvent(new Event("scroll"));
    target.dispatchEvent(new WheelEvent("wheel", { deltaY: -120 }));
    return { top: target.scrollTop, height: target.scrollHeight };
  });
  await expect.poll(() => area.evaluate((element) => element.scrollHeight), { timeout: 5_000 })
    .toBeGreaterThan(paused.height);
  const pausedTop = await area.evaluate((element) => element.scrollTop);
  expect(Math.abs(pausedTop - paused.top)).toBeLessThanOrEqual(2);
  await expect.poll(() => area.evaluate((element) =>
    element.scrollHeight - element.clientHeight - element.scrollTop,
  )).toBeGreaterThan(48);

  const resumeHeight = await area.evaluate((element) => {
    const target = element as HTMLElement;
    target.scrollTop = target.scrollHeight;
    target.dispatchEvent(new Event("scroll"));
    return target.scrollHeight;
  });
  await expect.poll(() => area.evaluate((element) => element.scrollHeight), { timeout: 5_000 })
    .toBeGreaterThan(resumeHeight);
  await expect.poll(() => area.evaluate((element) =>
    element.scrollHeight - element.clientHeight - element.scrollTop,
  )).toBeLessThanOrEqual(2);
});

test("atomically steers and removes a queued message from the backend mailbox", async ({ page }, testInfo) => {
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
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(0);

  await composer.fill("queued second");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();

  await expect(page.locator(".message-queue")).toContainText("队列 (2)");
  await expect(page.locator(".queue-list")).toContainText("queued first");
  await expect(page.locator(".queue-list")).toContainText("queued second");
  await expect(page.locator(".message--user").getByText("queued second", { exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length)).toBe(2);
  await page.screenshot({ path: testInfo.outputPath("queued-message-actions.png"), fullPage: true });

  await page.getByRole("button", { name: "加入当前对话 queued first", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_steer_queued").length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_steer").length)).toBe(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "remove_queued_turn").length)).toBe(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(0);
  await expect(page.locator(".message-queue")).toContainText("队列 (1)");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-live-queue",
      type: "turn_steered",
      phase: "exploring",
      message: {
        schemaVersion: 1,
        id: "steer-queued-first",
        role: "user",
        content: [{ type: "text", text: "queued first" }],
        createdAtMs: Date.now(),
      },
    });
  });
  await expect(page.locator(".message--user").getByText("queued first", { exact: true })).toBeVisible();
  await expect(page.locator(".message--user").getByText("queued second", { exact: true })).toHaveCount(0);
});

test("does not render a queued message in another conversation", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-thread-1-active",
      type: "turn_started",
      phase: "exploring",
    });
  });

  const composer = page.getByRole("textbox", { name: "消息" });
  await composer.fill("thread one queued message");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect(page.locator(".queue-list")).toContainText("thread one queued message");

  await page.keyboard.press("Control+n");
  await expect(page.getByRole("heading", { name: "Parallel conversation" })).toBeVisible();
  await expect(page.getByText("thread one queued message", { exact: true })).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-thread-1-active",
      type: "turn_completed",
      message: { schemaVersion: 1, id: "done-a", role: "assistant", content: [{ type: "text", text: "done" }], createdAtMs: 4 },
      usage: null,
      phase: "completed",
    });
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length
  )).toBe(1);
  await expect(page.getByText("thread one queued message", { exact: true })).toHaveCount(0);
});

test("runs different conversations concurrently while keeping each conversation sequential", async ({ page }) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });

  await composer.fill("first conversation work");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length
  )).toBe(1);
  await expect.poll(() => page.evaluate(() => {
    const invoked = (window as unknown as { __invoked: string[] }).__invoked;
    return {
      asyncStarts: invoked.filter((command) => command === "turn_start").length,
      blockingRuns: invoked.filter((command) => command === "run_turn").length,
    };
  })).toEqual({ asyncStarts: 1, blockingRuns: 0 });

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-parallel-1",
      type: "turn_started",
      phase: "exploring",
    });
  });

  await page.keyboard.press("Control+n");
  await expect(page.getByRole("heading", { name: "Parallel conversation" })).toBeVisible();
  await composer.fill("second conversation work");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length
  )).toBe(2);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __runTurnCalls: Array<{ request: { threadId: string } }> })
      .__runTurnCalls.map((call) => call.request.threadId)
  )).toEqual(["thread-1", "thread-2"]);
  await expect(page.locator(".message-queue")).toHaveCount(0);
});

test("sends images without frontend OCR and opens the conversation preview", async ({ page }, testInfo) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });
  await composer.evaluate(async (element) => {
    const transfer = new DataTransfer();
    const canvas = document.createElement("canvas");
    canvas.width = 320;
    canvas.height = 180;
    const context = canvas.getContext("2d")!;
    context.fillStyle = "#166534";
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = "#f8fafc";
    context.font = "600 30px sans-serif";
    context.fillText("Image preview", 56, 100);
    const blob = await new Promise<Blob>((resolve) => canvas.toBlob((value) => resolve(value!), "image/png"));
    transfer.items.add(new File([blob], "ocr-fixture.png", { type: "image/png" }));
    element.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, clipboardData: transfer }));
  });

  await expect(page.getByAltText("ocr-fixture.png")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "recognize_image").length)).toBe(0);
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
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: Array<{ attachments?: Array<{ ocrText?: string }> }> }).__runTurnCalls[0]?.attachments?.[0]?.ocrText)).toBeUndefined();
  await imageMessage.getByRole("button", { name: "查看图片 ocr-fixture.png" }).click();
  const imagePreview = page.getByRole("dialog", { name: "ocr-fixture.png" });
  await expect(imagePreview).toBeVisible();
  await expect(imagePreview.locator("img")).toHaveAttribute("src", /^data:image\/png;base64,/);
  await page.screenshot({ path: testInfo.outputPath("conversation-image-preview.png"), fullPage: true });
  await page.keyboard.press("Escape");
  await expect(imagePreview).toHaveCount(0);
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
  await imageMessage.getByRole("button", { name: "查看图片 ocr-fixture.png" }).click();
  await expect(page.getByRole("dialog", { name: "ocr-fixture.png" })).toBeVisible();
  await page.getByRole("button", { name: "关闭图片预览" }).click();
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
      arguments: { command: `Get-Content '${file}'` },
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
  const completedReadGroup = liveMessage.locator(".turn-tool-group").filter({ hasText: "执行了操作" });
  await expect(completedReadGroup).toBeVisible();
  await expect(completedReadGroup).not.toHaveAttribute("open", "");
  await completedReadGroup.locator(":scope > summary").click();
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
  await liveMessage.locator(".turn-tool-group").filter({ hasText: "执行了操作" }).locator(":scope > summary").click();
  await expect(liveMessage.locator(".turn-timeline-tool--completed").getByText("读取 src/App.tsx L3370-3382", { exact: true })).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline > *")).toHaveCount(2);
  await expect(liveMessage.getByText("Turn 已完成", { exact: true })).toHaveCount(0);
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
  const failedTestCall = call("call-test-1", "run_command", { command: "pnpm test" });
  const repairPatchCall = call("call-patch-2", "apply_patch");
  const passedTestCall = call("call-test-2", "run_command", { command: "pnpm test" });
  const firstChange = change("change-1", "call-patch-1", "before", "broken");
  const repairedChange = change("change-2", "call-patch-2", "broken", "fixed");

  await emit({ ...base, type: "turn_started", phase: "exploring" });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "先读取目标文件。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: readCall });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: readCall.id, name: readCall.name, result: result(true, "before") });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "开始应用第一版修改。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: firstPatchCall });
  await emit({ ...base, type: "tool_started", phase: "executing", call: firstPatchCall });
  await emit({ ...base, type: "item_started", phase: "awaiting_input", itemId: "approval-edit-1", itemType: "approval" });
  await emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: approval("approval-edit-1", firstPatchCall.id) });
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await emit({ ...base, type: "approval_resolved", phase: "executing", requestId: "approval-edit-1", resolution: { action: "approved", patch: null, selectedPaths: [], expectedHashes: [] } });
  await emit({ ...base, type: "item_completed", phase: "executing", itemId: "approval-edit-1", itemType: "approval", status: "completed" });
  await emit({ ...base, type: "item_started", phase: "executing", itemId: firstChange.id, itemType: "change" });
  await emit({ ...base, type: "change_applied", phase: "executing", changeSet: firstChange });
  await emit({ ...base, type: "item_completed", phase: "executing", itemId: firstChange.id, itemType: "change", status: "completed" });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: firstPatchCall.id, name: firstPatchCall.name, result: result(true, "applied") });
  await emit({ ...base, type: "tool_started", phase: "executing", call: failedTestCall });
  await emit({ ...base, type: "tool_output_delta", phase: "executing", callId: failedTestCall.id, stream: "stderr", cursor: 1, delta: "test failed\n" });
  await emit({ ...base, type: "tool_output_delta", phase: "executing", callId: failedTestCall.id, stream: "stderr", cursor: 1, delta: "test failed\n" });
  await emit({ ...base, type: "tool_completed", phase: "executing", callId: failedTestCall.id, name: failedTestCall.name, result: result(false, "test failed") });
  await emit({ ...base, type: "text_delta", phase: "responding", delta: "测试失败，修正实现后重新验证。" });
  await emit({ ...base, type: "tool_started", phase: "executing", call: repairPatchCall });
  await emit({ ...base, type: "item_started", phase: "awaiting_input", itemId: "approval-edit-2", itemType: "approval" });
  await emit({ ...base, type: "approval_requested", phase: "awaiting_input", request: approval("approval-edit-2", repairPatchCall.id) });
  await page.getByRole("button", { name: "运行", exact: true }).click();
  await emit({ ...base, type: "approval_resolved", phase: "executing", requestId: "approval-edit-2", resolution: { action: "approved", patch: null, selectedPaths: [], expectedHashes: [] } });
  await emit({ ...base, type: "item_completed", phase: "executing", itemId: "approval-edit-2", itemType: "approval", status: "completed" });
  await emit({ ...base, type: "item_started", phase: "executing", itemId: repairedChange.id, itemType: "change" });
  await emit({ ...base, type: "change_applied", phase: "executing", changeSet: repairedChange });
  await emit({ ...base, type: "item_completed", phase: "executing", itemId: repairedChange.id, itemType: "change", status: "completed" });
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
  await expect(liveMessage.locator(".turn-event-step--approval_requested")).toHaveCount(0);
  await expect(liveMessage.locator(".turn-event-step--approval_resolved")).toHaveCount(0);
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
  await expect(restoredMessage.locator(".turn-event-step--approval_requested")).toHaveCount(0);
  await expect(restoredMessage.locator(".turn-event-step--approval_resolved")).toHaveCount(0);
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
      call: { id: "call-cancel", name: "run_command", arguments: { command: "pnpm build" }, metadata: {} },
    });
  });

  await page.getByRole("button", { name: "停止生成" }).click();
  await expect(page.getByRole("button", { name: "正在停止" })).toBeVisible();
  await expect(page.getByRole("button", { name: "正在停止" })).toBeDisabled();
  await expect(page.locator(".mode-label")).toHaveText("正在停止");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(1);

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
  const cancelledExecution = cancelledMessage.locator(".turn-execution");
  const cancelledToolGroup = cancelledMessage.locator(".turn-tool-group--cancelled");
  await expect(cancelledExecution).not.toHaveAttribute("open", "");
  await expect(cancelledExecution.locator(":scope > summary .turn-disclosure-title")).toHaveText("已停止");
  await expect(cancelledExecution.getByRole("button", { name: "重试" })).not.toBeVisible();
  await cancelledExecution.locator(":scope > summary").click();
  await expect(cancelledToolGroup).not.toHaveAttribute("open", "");
  await cancelledToolGroup.locator(":scope > summary").click();
  await expect(cancelledMessage.locator(".turn-timeline-tool--cancelled")).toContainText("已取消");
  await expect(cancelledExecution.getByRole("button", { name: "重试" })).toBeVisible();
  await cancelledExecution.getByRole("button", { name: "重试" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_retry").length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "retry_turn").length)).toBe(0);
});

test("allows retrying a stop request after the IPC timeout", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("kcoder_e2e_hold_cancel", "true"));
  await page.goto("/");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-stop-timeout",
      type: "turn_started",
      phase: "exploring",
    });
  });

  await page.getByRole("button", { name: "停止生成" }).click();
  await expect(page.getByRole("button", { name: "正在停止" })).toBeDisabled();
  await expect(page.getByRole("alert")).toContainText("停止请求超时");
  await expect(page.getByRole("button", { name: "停止生成" })).toBeEnabled();
  await page.getByRole("button", { name: "停止生成" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(2);
});

test("presents a failed turn as one actionable error disclosure", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_hold_retry", "true");
    localStorage.setItem("kcoder_theme", "dark");
  });
  await page.goto("/");
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-rate-limited" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "reasoning-rate-limited", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "reasoning-rate-limited", delta: "正在检查上游响应。" });
    emit({
      ...base,
      type: "turn_failed",
      phase: "failed",
      message: "provider returned HTTP 429: Upstream rate limit exceeded, please retry later",
      startedAtMs: 1_000,
      completedAtMs: 5_600,
      durationMs: 4_600,
    });
  });

  const failedExecution = page.locator(".message--assistant").last().locator(".turn-execution--failed");
  await expect(failedExecution).not.toHaveAttribute("open", "");
  await expect(failedExecution.locator(":scope > summary .turn-disclosure-title")).toHaveText("请求未完成");
  await expect(failedExecution.locator(":scope > summary .turn-disclosure-status")).toHaveText("耗时 4.6s");
  await expect(failedExecution.getByText("错误原因", { exact: true })).not.toBeVisible();
  await expect(failedExecution.getByText("Turn 执行失败", { exact: true })).toHaveCount(0);
  await expect(failedExecution.getByText("执行失败", { exact: true })).toHaveCount(0);
  await expect(page.locator(".error-banner")).toHaveCount(0);
  await expect(failedExecution.getByRole("button", { name: "重试", exact: true })).not.toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("failed-turn-agent-ui-collapsed.png"), fullPage: true });

  await failedExecution.locator(":scope > summary").click();
  await expect(failedExecution).toHaveAttribute("open", "");
  await expect(failedExecution.getByText("错误原因", { exact: true })).toBeVisible();
  await expect(failedExecution.getByText("provider returned HTTP 429: Upstream rate limit exceeded, please retry later", { exact: true })).toBeVisible();
  await expect(failedExecution.locator(".turn-reasoning")).toHaveCount(0);
  await expect(failedExecution.getByText("正在检查上游响应。", { exact: true })).toHaveCount(0);
  await expect(failedExecution.getByText("生成中", { exact: true })).toHaveCount(0);
  await expect(failedExecution.locator(".turn-plan")).not.toHaveAttribute("open", "");
  await expect(failedExecution.getByRole("button", { name: "重试", exact: true })).toBeVisible();
  await page.waitForTimeout(450);
  await page.screenshot({ path: testInfo.outputPath("failed-turn-agent-ui-expanded.png"), fullPage: true });
  await failedExecution.getByRole("button", { name: "重试", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "turn_retry").length)).toBe(1);
});

test("uses the exact active turn when recovering a stuck conversation", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-recovery-exact",
      type: "turn_started",
      phase: "exploring",
    });
  });

  await page.keyboard.press("Control+Shift+r");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.turn_interrupt)).toEqual({
    threadId: "thread-1",
    turnId: "turn-recovery-exact",
  });
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "cancel_turn").length)).toBe(0);
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

  const failedExecution = page.locator(".message--activity-only .turn-execution--failed").filter({ hasText: "provider failed" });
  await expect(failedExecution).not.toHaveAttribute("open", "");
  await expect(failedExecution.getByText("请求未完成", { exact: true })).toBeVisible();
  await expect(failedExecution.getByText("错误原因", { exact: true })).not.toBeVisible();
  await expect(failedExecution.getByText("Turn 已失败", { exact: true })).toHaveCount(0);
  await failedExecution.locator(":scope > summary").click();
  await expect(failedExecution.getByText("错误原因", { exact: true })).toBeVisible();
  await expect(failedExecution.getByText("provider failed", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_retry").length)).toBe(1);
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
    localStorage.setItem("kcoder_theme", "dark");
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
          kind: "model_question",
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
  await expect.poll(() => page.locator(".user-input-question-text").evaluate((element) => getComputedStyle(element).color)).toBe("rgb(227, 232, 240)");
  await expect.poll(() => page.getByRole("button", { name: "Fast", exact: true }).evaluate((element) => getComputedStyle(element).color)).toBe("rgb(227, 232, 240)");
  await page.getByRole("button", { name: "Fast", exact: true }).click();
  await page.getByRole("button", { name: "提交回答", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "resolve_user_input").length)).toBe(1);
});

test("restores a soft turn continuation gate with direct actions", async ({ page }) => {
  const question = "当前执行段已调用模型 30 次、累计消耗 920000 tokens、运行 480 秒。";
  await page.addInitScript((continuationQuestion) => {
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Long running turn", createdAtMs: 1, updatedAtMs: 2, archived: false },
      messages: [{ schemaVersion: 1, id: "continuation-user", role: "user", content: [{ type: "text", text: "Complete the task" }], createdAtMs: 1 }],
      messageTurnIds: {},
      turnUserMessageIds: { "turn-continuation": "continuation-user" },
      lastTurn: { turnId: "turn-continuation", state: "awaiting_approval", error: null },
      toolActivities: [],
      turnTimeline: [{ type: "event", itemId: "user-input-requested-continuation-1", turnId: "turn-continuation", kind: "user_input_requested", title: "User input requested", detail: continuationQuestion }],
      approvals: [],
      userInputs: [{
        request: {
          id: "continuation-1",
          threadId: "thread-1",
          turnId: "turn-continuation",
          toolCallId: "runtime-turn-continuation",
          kind: "turn_continuation",
          questions: [{ question: continuationQuestion, options: ["continue", "compact_and_continue", "stop"] }],
          createdAtMs: 1,
          expiresAtMs: Date.now() + 300000,
        },
        resolution: null,
      }],
      changes: [],
      todos: [],
      lastUsage: null,
    }));
  }, question);

  await page.goto("/");
  await expect(page.getByText("执行额度已用完", { exact: true })).toBeVisible();
  await expect(page.locator(".user-input-question-text").getByText(question, { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "继续执行", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "停止执行", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "压缩后继续", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.resolve_user_input)).toEqual({
    requestId: "continuation-1",
    resolution: {
      action: "answered",
      answers: [{ question, answer: "compact_and_continue" }],
    },
  });
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
  const trigger = page.getByRole("button", { name: /操作批准方式/ });

  await expect(trigger).toHaveAttribute("aria-label", "操作批准方式：请求批准");
  await expect(trigger).not.toContainText("请求批准");
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
        arguments: { command: "pnpm test" },
        preview: null,
        createdAtMs: 1,
        expiresAtMs: 2,
      },
    });
  });
  await expect(page.locator(".turn-event-step--approval_requested")).toHaveCount(0);
  await expect(page.getByText("已自动批准操作", { exact: true })).toHaveCount(0);
  await expect(page.locator(".message--approval")).toHaveCount(0);
  await expect(page.locator(".message-role").getByText("k-Coder", { exact: true })).toHaveCount(1);
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
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(page.getByRole("dialog", { name: "设置" })).toBeVisible();
  await page.getByRole("button", { name: "关闭设置" }).click();
  await expect(page.getByRole("tab", { name: "项目" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("navigation", { name: "项目列表" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "会话列表" })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("project-session-list.png"), fullPage: true });
});

test("switches to the project workspace before creating a project session", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === "narrow", "窄屏隐藏项目侧边栏");
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_thread_project_map", JSON.stringify({ "thread-1": "D:\\code\\k-coder" }));
    localStorage.setItem("kcoder_known_projects", JSON.stringify(["D:\\code\\k-coder"]));
    localStorage.setItem("kcoder_e2e_workspace_path", "D:\\code\\codex");
  });
  await page.goto("/");

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.switch_workspace
  )).toEqual({ path: "D:\\code\\k-coder", trusted: true });

  await page.evaluate(() => {
    localStorage.setItem("kcoder_e2e_workspace_path", "D:\\code\\codex");
    (window as unknown as { __invoked: string[] }).__invoked.length = 0;
  });
  await page.getByRole("tab", { name: "项目" }).click();
  const project = page.locator(".project-group").filter({ hasText: "k-coder" });
  await project.getByRole("button", { name: "在项目中新建会话" }).click();

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invoked: string[] }).__invoked
  )).toContain("create_thread");
  const calls = await page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked);
  expect(calls.indexOf("switch_workspace")).toBeGreaterThanOrEqual(0);
  expect(calls.indexOf("create_thread")).toBeGreaterThan(calls.indexOf("switch_workspace"));
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.switch_workspace
  )).toEqual({ path: "D:\\code\\k-coder", trusted: true });
});

test("hydrates the unified thread item page and loads older turns", async ({ page }) => {
  await page.addInitScript(() => {
    const item = (
      id: string,
      turnId: string,
      role: "user" | "assistant",
      text: string,
      createdAtMs: number,
    ) => ({
      schemaVersion: 1,
      id,
      turnId,
      status: "completed",
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs,
      timelineItems: role === "assistant"
        ? [{ type: "text", id, turnId, text }]
        : [],
      type: role === "user" ? "user_message" : "agent_message",
      message: {
        schemaVersion: 1,
        id,
        role,
        content: [{ type: "text", text }],
        createdAtMs,
      },
      ...(role === "assistant" ? { phase: "final_answer" } : {}),
    });
    const turn = (id: string, userText: string, answerText: string, createdAtMs: number) => ({
      schemaVersion: 1,
      id,
      userMessageId: `${id}-user`,
      state: "completed",
      error: null,
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs + 20,
      durationMs: 20,
      itemsView: "full",
      items: [
        item(`${id}-user`, id, "user", userText, createdAtMs),
        item(`${id}-answer`, id, "assistant", answerText, createdAtMs + 10),
      ],
    });
    const summary = {
      schemaVersion: 1,
      id: "thread-1",
      title: "Phase 6 workbench",
      createdAtMs: 1,
      updatedAtMs: 300,
      archived: false,
      workspacePath: "D:\\code\\k-coder",
    };
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary,
      lastTurn: { turnId: "turn-new", state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [turn("turn-new", "recent question", "recent answer", 200)],
        nextCursor: "older-cursor",
        backwardsCursor: "newer-cursor",
      },
      unscopedItems: [],
    }));
    localStorage.setItem("kcoder_e2e_thread_turns_page", JSON.stringify({
      data: [turn("turn-old", "older question", "older answer", 100)],
      nextCursor: null,
      backwardsCursor: "older-backwards-cursor",
    }));
  });
  await page.goto("/");

  await expect(page.getByText("recent answer", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "read_thread_history").length
  )).toBe(1);
  expect(await page.evaluate(() =>
    (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "read_thread").length
  )).toBe(0);

  await page.getByRole("button", { name: "加载更早记录" }).click();
  await expect(page.getByText("older question", { exact: true })).toBeVisible();
  await expect(page.getByText("older answer", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "加载更早记录" })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.list_thread_turns
  )).toEqual({
    threadId: "thread-1",
    cursor: "older-cursor",
    limit: 50,
    sortDirection: "desc",
    itemsView: "full",
  });
});

test("renders a recovered assistant item only once when its turn comes from the timeline", async ({ page }) => {
  await page.addInitScript(() => {
    const turnId = "turn-recovered";
    const answerId = "answer-recovered";
    const answer = "代码已提交并推送成功。";
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Phase 6 workbench",
        createdAtMs: 1,
        updatedAtMs: 4,
        archived: false,
        workspacePath: "D:\\code\\k-coder",
      },
      lastTurn: { turnId, state: "completed", error: null },
      todos: [],
      lastUsage: { inputTokens: 172555, outputTokens: 2683, totalTokens: 175238 },
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: "push-user",
          state: "completed",
          error: null,
          startedAtMs: 2,
          completedAtMs: 4,
          durationMs: 2,
          itemsView: "full",
          items: [{
            schemaVersion: 1,
            id: "push-user",
            turnId,
            status: "completed",
            startedAtMs: 2,
            completedAtMs: 2,
            timelineItems: [],
            type: "user_message",
            message: {
              schemaVersion: 1,
              id: "push-user",
              role: "user",
              content: [{ type: "text", text: "推送代码" }],
              createdAtMs: 2,
            },
          }],
        }],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [{
        schemaVersion: 1,
        id: answerId,
        turnId: null,
        status: "completed",
        startedAtMs: 3,
        completedAtMs: 3,
        timelineItems: [
          { type: "text", id: answerId, turnId, text: answer },
          { type: "event", itemId: `turn-completed-${turnId}`, turnId, kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 2 },
        ],
        type: "agent_message",
        phase: "final_answer",
        message: {
          schemaVersion: 1,
          id: answerId,
          role: "assistant",
          content: [{ type: "text", text: answer }],
          createdAtMs: 3,
        },
      }],
    }));
  });
  await page.goto("/");

  await expect(page.getByText("代码已提交并推送成功。", { exact: true })).toHaveCount(1);
  await expect(page.locator(".message--assistant")).toHaveCount(1);
  await expect(page.locator(".message--assistant .turn-execution")).toHaveCount(1);
});

test("coalesces multiple assistant projections for the same turn", async ({ page }) => {
  await page.addInitScript(() => {
    const turnId = "turn-overlap";
    const item = (id: string, text: string, createdAtMs: number) => ({
      schemaVersion: 1,
      id,
      turnId,
      status: "completed",
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs,
      timelineItems: [{ type: "text", id, turnId, text }],
      type: "agent_message",
      phase: "final_answer",
      message: {
        schemaVersion: 1,
        id,
        role: "assistant",
        content: [{ type: "text", text }],
        createdAtMs,
      },
    });
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 5, archived: false },
      lastTurn: { turnId, state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: null,
          state: "completed",
          error: null,
          startedAtMs: 2,
          completedAtMs: 5,
          durationMs: 3,
          itemsView: "full",
          items: [item("answer-overlap-old", "阶段性答复", 3), item("answer-overlap-final", "最终答复", 4)],
        }],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  await expect(page.locator(".message--assistant")).toHaveCount(1);
  await expect(page.getByText("最终答复", { exact: true })).toHaveCount(1);
});

test("clears the previous thread view while the selected thread is loading", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === "narrow", "窄屏隐藏会话侧边栏");
  await page.addInitScript(() => {
    const secondThread = {
      schemaVersion: 1,
      id: "thread-2",
      title: "Parallel conversation",
      createdAtMs: 3,
      updatedAtMs: 3,
      archived: false,
    };
    localStorage.setItem("kcoder_e2e_threads", JSON.stringify([
      { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false },
      secondThread,
    ]));
    localStorage.setItem("kcoder_e2e_empty_thread_id", secondThread.id);
    localStorage.setItem("kcoder_e2e_thread_detail_by_id", JSON.stringify({
      [secondThread.id]: {
        schemaVersion: 1,
        summary: secondThread,
        messages: [],
        messageTurnIds: {},
        lastTurn: null,
        toolActivities: [],
        turnTimeline: [],
        approvals: [],
        changes: [],
      },
    }));
  });
  await page.goto("/");

  await expect(page.getByText("检查完成。", { exact: true })).toBeVisible();
  await page.evaluate(() => localStorage.setItem("kcoder_e2e_read_delay_ms", "2000"));
  await page.getByRole("button", { name: "Parallel conversation", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Parallel conversation", exact: true })).toBeVisible();

  expect(await page.getByText("检查完成。", { exact: true }).isVisible()).toBe(false);
  expect(await page.getByText("检查工作区", { exact: true }).isVisible()).toBe(false);
  await expect(page.getByText("正在读取会话", { exact: true })).toBeVisible();
});

test("starts a workspace terminal from the workbench tab and forwards keystrokes", async ({ page }) => {
  const startCount = () => page.evaluate(() => (window as unknown as { __ptyStartRequests: unknown[] }).__ptyStartRequests.length);
  await page.goto("/");
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  await page.getByRole("tab", { name: "终端" }).click();

  await expect(page.locator(".terminal-view .xterm")).toBeVisible();
  await expect.poll(startCount).toBeGreaterThanOrEqual(1);
  const request = await page.evaluate(() => {
    const list = (window as unknown as { __ptyStartRequests: Array<{ program?: string; cwd?: string }> }).__ptyStartRequests;
    return list[list.length - 1];
  });
  expect(request.program).toBe("");
  expect(request.cwd ?? "").toBe("");

  await page.locator(".terminal-view .xterm-helper-textarea").focus();
  await page.keyboard.type("pnpm tauri build");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __ptyWrites: string[] }).__ptyWrites.join(""))).toContain("pnpm tauri build");

  const started = await startCount();
  await page.getByRole("tab", { name: "文件" }).click();
  await expect(page.locator(".terminal-view")).toBeHidden();
  await page.getByRole("tab", { name: "终端" }).click();
  await expect(page.locator(".terminal-view .xterm")).toBeVisible();
  expect(await startCount()).toBe(started);
});

test("shows the terminal exit state and restarts the session", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_pty_state", "exited");
  });
  await page.goto("/");
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  await page.getByRole("tab", { name: "终端" }).click();

  await expect(page.getByText("进程已退出（代码 0）", { exact: true })).toBeVisible();
  const started = await page.evaluate(() => (window as unknown as { __ptyStartRequests: unknown[] }).__ptyStartRequests.length);
  await page.getByRole("button", { name: "重启终端" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __ptyStartRequests: unknown[] }).__ptyStartRequests.length)).toBe(started + 1);
});
