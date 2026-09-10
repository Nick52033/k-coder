import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    const tauriEventCallbackIds = new Map<string, number>();
    let callbackId = 1;
    let agentEventCallbackId: number | null = null;
    let mailboxEventCallbackId: number | null = null;
    const localDate = (offsetDays: number) => {
      const date = new Date();
      date.setDate(date.getDate() + offsetDays);
      const year = date.getFullYear();
      const month = String(date.getMonth() + 1).padStart(2, "0");
      const day = String(date.getDate()).padStart(2, "0");
      return `${year}-${month}-${day}`;
    };
    const threadFixture = { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false, inProject: true, workspacePath: "D:\\code\\k-coder" };
    const threadOverride = localStorage.getItem("kcoder_e2e_thread_override");
    const thread = threadOverride ? { ...threadFixture, ...JSON.parse(threadOverride) } : threadFixture;
    const secondThread = { schemaVersion: 1, id: "thread-2", title: "Parallel conversation", createdAtMs: 3, updatedAtMs: 3, archived: false };
    const openAiProvider = { schemaVersion: 1, id: "openai", kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }, { id: "gpt-4o", displayName: "GPT-4 Omni", contextWindow: 64000, fallback: false }], endpoints: [], hasApiKey: true };
    const ziccProvider = { schemaVersion: 1, id: "zicc", kind: "open_ai_compatible", transport: "open_ai_responses", name: "zicc", baseUrl: "https://zicc.example.com/v1", model: "gpt-5.6-terra", models: [{ id: "gpt-5.6-terra", displayName: "gpt-5.6-terra", contextWindow: 128000, fallback: false }, { id: "gpt-5.5", displayName: "gpt-5.5", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: true };
    const pendingProvider = { schemaVersion: 1, id: "pending", kind: "open_ai_compatible", transport: "anthropic_messages", name: "待配置供应商", baseUrl: "https://pending.example.com/v1", model: "claude-test", models: [{ id: "claude-test", displayName: "Claude Test", contextWindow: 128000, fallback: false }], endpoints: [], hasApiKey: false };
    const makeBinding = (declaration: string, kind: "skill" | "plugin_skill") => {
      const slash = declaration.indexOf("/");
      const pluginId = slash > 0 ? declaration.slice(0, slash) : null;
      const skillId = slash > 0 ? declaration.slice(slash + 1) : declaration;
      return {
        kind,
        declaration,
        skillId,
        pluginId,
        fallbackSkillId: kind === "plugin_skill" ? skillId : null,
      };
    };
    const makeWorkflow = ({ id, name, description, rolePrompt, localSkills, pluginSkills, nodes }: {
      id: string;
      name: string;
      description: string;
      rolePrompt: string;
      localSkills: string[];
      pluginSkills: string[];
      nodes: Array<[string, string, string]>;
    }) => ({
      schemaVersion: 1,
      definitionVersion: 2,
      id,
      name,
      description,
      rolePrompt,
      localSkillCount: localSkills.length,
      pluginSkillCount: pluginSkills.length,
      uniqueSkillCount: localSkills.length + pluginSkills.length,
      skillCatalog: [
        ...localSkills.map((skill) => makeBinding(skill, "skill")),
        ...pluginSkills.map((skill) => makeBinding(skill, "plugin_skill")),
      ],
      nodes: nodes.map(([nodeId, title, nodeDescription]) => ({
        id: nodeId,
        title,
        description: nodeDescription,
        localSkillCount: Math.min(localSkills.length, 1),
        pluginSkillCount: Math.min(pluginSkills.length, 1),
        skillDeclarationCount: Math.min(localSkills.length, 1) + Math.min(pluginSkills.length, 1),
        localSkillBindings: localSkills.slice(0, 1).map((skill) => makeBinding(skill, "skill")),
        pluginSkillBindings: pluginSkills.slice(0, 1).map((skill) => makeBinding(skill, "plugin_skill")),
      })),
    });
    const workflowDefinitions = [
      makeWorkflow({
        id: "fullstack-delivery",
        name: "全栈开发机器人",
        description: "覆盖需求分析、界面与架构设计、原型 HTML、前后端开发、全面测试、构建发布和代码交付。",
        rolePrompt: "# 全栈开发机器人 - 角色定义\n\n你是一位经验丰富的全栈开发工程师。",
        localSkills: ["brainstorming", "writing-plans", "executing-plans", "test-driven-development", "requesting-code-review", "verification-before-completion", "dispatching-parallel-agents", "taste-skill", "awesome-design-md"],
        pluginSkills: ["browser/control-in-app-browser", "superpowers/brainstorming", "superpowers/dispatching-parallel-agents", "superpowers/executing-plans", "superpowers/finishing-a-development-branch", "superpowers/receiving-code-review", "superpowers/requesting-code-review", "superpowers/subagent-driven-development", "superpowers/systematic-debugging", "superpowers/test-driven-development", "superpowers/using-git-worktrees", "superpowers/verification-before-completion", "superpowers/writing-plans", "documents/documents"],
        nodes: [
          ["requirements-analysis", "需求理解与分析", "澄清目标、约束、现状与可验证的交付范围。"],
          ["interface-architecture-design", "界面与架构设计", "确定界面体验、模块职责、公共契约和安全边界。"],
          ["html-prototype", "原型 HTML", "以可检查的 HTML 原型验证关键布局和交互。"],
          ["backend-development", "后端开发", "实现后端领域逻辑、边界契约与安全测试。"],
          ["frontend-development", "前端开发", "实现与现有设计一致的完整界面和交互状态。"],
          ["comprehensive-testing", "全面测试", "覆盖功能、边界、失败、恢复和桌面工作流。"],
          ["build-release", "构建与发布", "完成规定构建门槛并准备可审计的发布结果。"],
          ["code-review-delivery", "代码审查与交付", "复查正确性、安全性、兼容性和最终交付说明。"],
        ],
      }),
      makeWorkflow({
        id: "quality-assurance",
        name: "软件测试机器人",
        description: "覆盖测试策略、用例设计、单元测试、集成与 API、E2E/UI、性能、安全和测试报告。",
        rolePrompt: "# 软件测试机器人 - 角色定义\n\n你是一位经验丰富的高级 QA 测试工程师。",
        localSkills: ["test-strategy-planning", "test-case-design", "api-testing", "performance-testing", "security-testing", "test-report-generation", "webapp-testing"],
        pluginSkills: ["superpowers/test-driven-development", "superpowers/systematic-debugging", "superpowers/verification-before-completion", "superpowers/writing-plans", "superpowers/executing-plans", "superpowers/dispatching-parallel-agents", "superpowers/subagent-driven-development", "browser/control-in-app-browser", "documents/documents"],
        nodes: [
          ["test-strategy", "测试策略制定", "确定测试目标、风险分层、范围、环境和验收口径。"],
          ["test-case-design", "测试用例设计", "设计正常、边界、失败、恢复和安全路径的用例。"],
          ["unit-testing", "单元测试", "实现并执行聚焦领域逻辑和公共契约的单元测试。"],
          ["integration-api-testing", "集成/API 测试", "验证模块协作、类型化边界、API 失败与恢复语义。"],
          ["e2e-ui-testing", "E2E/UI 测试", "验证真实用户工作流、响应式布局和交互终态。"],
          ["nonfunctional-testing", "非功能测试", "评估性能、安全、资源边界和故障韧性。"],
          ["test-report-delivery", "测试报告与交付", "汇总覆盖、结果、缺陷、限制与残余风险。"],
        ],
      }),
      makeWorkflow({
        id: "requirements-design",
        name: "需求设计机器人",
        description: "通过需求采集、边界划定、用户故事、交互流程、PRD 审查、文档输出和钉钉发布形成可交付需求文档。",
        rolePrompt: "# 需求设计机器人 - 角色定义\n\n你是一位资深产品经理和需求分析师。",
        localSkills: ["requirements-intake", "prd-story-modeler", "prd-delivery-review", "create-plan", "dingtalk-document"],
        pluginSkills: ["superpowers/brainstorming", "superpowers/verification-before-completion", "superpowers/writing-plans", "documents/documents"],
        nodes: [
          ["requirements-intake", "需求采集与理解", "与用户沟通需求，通过渐进式问答提炼核心需求。"],
          ["business-boundary", "业务边界划定", "明确做什么、不做什么、约束条件和实现路径。"],
          ["user-story-modeling", "用户故事与功能建模", "将需求拆解为带可验证验收标准的用户故事和功能需求。"],
          ["interaction-flow-design", "交互流程设计", "设计核心交互流程、页面跳转和状态机。"],
          ["prd-review", "PRD 整合与审查", "汇总阶段产物并进行覆盖性、一致性、可执行性和风险审查。"],
          ["document-formatting", "文档格式化输出", "输出最终 Markdown 文档和可选结构化文档。"],
          ["dingtalk-publishing", "发布到钉钉知识库", "通过已配置并授权的钉钉能力发布最终 PRD。"],
        ],
      }),
    ];
    const workflowReadiness = (definition: typeof workflowDefinitions[number]) => ({
      schemaVersion: 1,
      workflowId: definition.id,
      definitionVersion: definition.definitionVersion,
      ready: true,
      skillCount: definition.skillCatalog.length,
      localSkillCount: definition.localSkillCount,
      pluginSkillCount: definition.pluginSkillCount,
      blockerCount: 0,
      bindings: definition.skillCatalog.map((binding) => ({
        binding,
        status: binding.kind === "plugin_skill" ? "builtin_fallback" : "builtin",
        resolvedSkillId: binding.skillId,
        resolvedScope: "builtin",
        bodySha256: `fixture-${binding.declaration}`,
        bodyBytes: 100,
        blocker: null,
      })),
      nodes: definition.nodes.map((node) => ({
        nodeId: node.id,
        ready: true,
        declarationCount: node.skillDeclarationCount,
        uniqueBodyCount: node.skillDeclarationCount,
        totalBodyBytes: node.skillDeclarationCount * 100,
        bindings: [],
        blockers: [],
      })),
      blockers: [],
    });
    let workflowRun = JSON.parse(localStorage.getItem("kcoder_e2e_workflow_run") ?? "null") as null | {
      schemaVersion: number;
      id: string;
      threadId: string;
      workflowId: string;
      objective: string;
      state: "active" | "completed" | "cancelled";
      currentNodeId: string | null;
      currentNodeIndex: number;
      nodeCount: number;
      completedNodes: unknown[];
      createdAtMs: number;
      updatedAtMs: number;
      revision: number;
    };
    let providerCatalog: { schemaVersion: number; activeProviderId: string | null; providers: Array<typeof openAiProvider> } = { schemaVersion: 1, activeProviderId: "openai", providers: [openAiProvider, ziccProvider, pendingProvider] };
    let approvalMode: "ask" | "full_access" = "ask";
    let reasoningEffort: "off" | "minimal" | "low" | "medium" | "high" | "x_high" = "medium";
    let workspaceState = {
      current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
      recent: [
        { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
        { id: "project-2", name: "paypro-platform-be", path: "D:\\code\\paypro-platform-be", trusted: true, lastOpenedAtMs: 1 },
      ] as Array<{ id: string; name: string; path: string; trusted: boolean; lastOpenedAtMs: number }>,
    };
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
      workflowId?: string | null;
    }>>();
    const mailboxRevisionByThread = new Map<string, number>();
    function bumpMailboxRevision(threadId: string) {
      const revision = (mailboxRevisionByThread.get(threadId) ?? 0) + 1;
      mailboxRevisionByThread.set(threadId, revision);
      return revision;
    }
    function emitMailboxChanged(threadId: string, revision: number) {
      if (mailboxEventCallbackId === null) return;
      callbacks.get(mailboxEventCallbackId)?.({
        event: "thread-mailbox-changed",
        id: 1,
        payload: { schemaVersion: 1, threadId, revision },
      });
    }
    function restoreMailboxFixture() {
      const restoredMailbox = JSON.parse(localStorage.getItem("kcoder_e2e_mailbox") ?? "null") as null | {
        threadId: string;
        activeTurnId?: string | null;
        pending: Array<{
          turnId: string;
          threadId: string;
          kind: "message" | "retry";
          input: string;
          agentMode: string | null;
          attachments: unknown[];
          workflowId?: string | null;
        }>;
      };
      if (!restoredMailbox) return;
      mailboxByThread.set(restoredMailbox.threadId, restoredMailbox.pending);
      mailboxRevisionByThread.set(restoredMailbox.threadId, 1);
      if (restoredMailbox.activeTurnId) {
        activeTurnIds.set(restoredMailbox.threadId, restoredMailbox.activeTurnId);
      }
      localStorage.removeItem("kcoder_e2e_mailbox");
    }
    restoreMailboxFixture();
    const invocationArgs: Record<string, unknown> = {};
    const ptyStartRequests: unknown[] = [];
    const ptyWrites: string[] = [];
    const ptyWriteMetrics = { active: 0, maxConcurrent: 0 };
    const extensionOverview = {
      schemaVersion: 1,
      configPaths: [
        "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\extensions.json",
        "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\mcp.json",
        "D:\\code\\k-coder\\.k-coder\\extensions.json",
        "D:\\code\\k-coder\\.k-coder\\mcp.json",
      ],
      instructions: [{ path: "D:\\code\\k-coder\\AGENTS.md", scope: "project", priority: 200, bytes: 120 }],
      skills: [
        { name: "workspace-review", description: "Built-in workspace review", path: "D:\\apps\\k-coder\\resources\\skills\\workspace-review\\SKILL.md", scope: "builtin", risk: "read", category: "quality_review", triggers: ["workspace review"], enabled: true, managedByRobot: false },
        { name: "review", description: "Review code safely", path: "D:\\code\\k-coder\\.k-coder\\skills\\review\\SKILL.md", scope: "project", risk: "read", category: "quality_review", triggers: ["review"], enabled: true, managedByRobot: false },
        { name: "requirements-intake", description: "Robot requirements intake", path: "D:\\apps\\k-coder\\resources\\skills\\robot-pack\\requirements-intake\\SKILL.md", scope: "builtin", risk: "read", category: "requirements_planning", triggers: ["requirements"], enabled: true, managedByRobot: true },
      ],
      mcpServers: [{ id: "local", transport: "stdio", enabled: true, state: "ready", toolCount: 2, credentials: [], error: null }],
      hooks: [{ id: "guard", phase: "before", tool: "mcp__local__*", enabled: true }],
      audit: [{ timestampMs: 2, event: "extensions_ready", kind: "runtime", id: "all", success: true, detail: "extensions loaded" }],
      error: null,
    };
    let userRules = {
      schemaVersion: 1,
      path: "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\user-rules.json",
      rules: [{
        id: "11111111-1111-4111-8111-111111111111",
        title: "方法注释",
        content: "公共方法需要注释，实体缺少注释时需要提醒。",
        createdAtMs: 1_785_000_000_000,
        updatedAtMs: 1_785_000_000_000,
      }],
      error: null,
    };
    let mcpConfig = {
      schemaVersion: 2,
      global: {
        scope: "global",
        path: "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\mcp.json",
        exists: true,
        content: `${JSON.stringify({
          mcpServers: {
            local: {
              type: "stdio",
              enabled: true,
              timeoutMs: 30_000,
              command: "npx",
              args: ["-y", "@modelcontextprotocol/server-filesystem", "D:\\code\\k-coder"],
              secret_env: {},
            },
          },
        }, null, 2)}\n`,
        error: null,
      },
      project: {
        scope: "project",
        path: "D:\\code\\k-coder\\.k-coder\\mcp.json",
        exists: false,
        content: `${JSON.stringify({ mcpServers: {} }, null, 2)}\n`,
        error: null,
      },
      overview: extensionOverview,
    };
    const pluginRootPath = "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\plugins";
    let pluginOverview = {
      schemaVersion: 1,
      rootPath: pluginRootPath,
      plugins: [
        {
          id: "review-tools@local",
          name: "review-tools",
          version: "1.2.3",
          description: "Review the workspace with indexed guidance",
          path: `${pluginRootPath}\\review-package-with-a-very-long-folder-name-that-must-wrap`,
          enabled: false,
          state: "disabled",
          deletable: true,
          components: { skillCount: 2, mcpServerCount: 0, mcpToolCount: 0, unsupportedCount: 0 },
          warnings: [],
          error: null,
        },
        {
          id: "ready-tools@local",
          name: "ready-tools",
          version: "2.0.0",
          description: "Ready plugin",
          path: `${pluginRootPath}\\ready`,
          enabled: true,
          state: "loaded",
          deletable: true,
          components: { skillCount: 1, mcpServerCount: 1, mcpToolCount: 2, unsupportedCount: 0 },
          warnings: [],
          error: null,
        },
        {
          id: "partial-tools@local",
          name: "partial-tools",
          version: "1.0.0",
          description: "Skill available while Apps remain unsupported",
          path: `${pluginRootPath}\\partial`,
          enabled: true,
          state: "degraded",
          deletable: true,
          components: { skillCount: 1, mcpServerCount: 0, mcpToolCount: 0, unsupportedCount: 1 },
          warnings: ["1 declared plugin component(s) are not supported and will not run"],
          error: null,
        },
        {
          id: "blocked-tools@local",
          name: "blocked-tools",
          version: "1.0.0",
          description: "Runtime dependency is unavailable",
          path: `${pluginRootPath}\\blocked`,
          enabled: true,
          state: "blocked",
          deletable: true,
          components: { skillCount: 0, mcpServerCount: 1, mcpToolCount: 0, unsupportedCount: 0 },
          warnings: [],
          error: "MCP runtime failed to start",
        },
        {
          id: "invalid:broken-package",
          name: "broken-package",
          version: "",
          description: "",
          path: `${pluginRootPath}\\broken-package`,
          enabled: false,
          state: "invalid",
          deletable: true,
          components: { skillCount: 0, mcpServerCount: 0, mcpToolCount: 0, unsupportedCount: 0 },
          warnings: [],
          error: "plugin manifest JSON is invalid",
        },
      ],
      error: null,
    };
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
      advanced_metrics: { providerCalls: 2, providerFailures: 0, averageProviderLatencyMs: 120, inputTokens: 100, outputTokens: 20, toolCalls: 2, toolSuccessRate: 1, fallbackCount: 0, retryCount: 2, completedTasks: 1, failedTasks: 0, estimatedCostUsd: null },
      usage_summary: {
        schemaVersion: 2,
        trendDays: 30,
        providerCalls: 14,
        inputTokens: 190300,
        outputTokens: 3300,
        totalTokens: 193600,
        cachedInputTokens: 96500,
        uncachedInputTokens: 93800,
        cacheWriteInputTokens: 0,
        reasoningOutputTokens: 2100,
        replyOutputTokens: 1200,
        cacheHitRate: 96500 / 190300,
        estimatedCostUsd: null,
        daily: [
          { date: localDate(-1), providerCalls: 4, inputTokens: 42000, outputTokens: 800, totalTokens: 42800 },
          { date: localDate(0), providerCalls: 10, inputTokens: 148300, outputTokens: 2500, totalTokens: 150800 },
        ],
        models: [{
          provider: "DeepSeek",
          model: "deepseek-v4-pro-0813",
          providerCalls: 14,
          inputTokens: 190300,
          outputTokens: 3300,
          totalTokens: 193600,
          cachedInputTokens: 96500,
          uncachedInputTokens: 93800,
          cacheWriteInputTokens: 0,
          reasoningOutputTokens: 2100,
          replyOutputTokens: 1200,
          cacheHitRate: 96500 / 190300,
          estimatedCostUsd: null,
        }],
      },
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
      ], approvals: [], changes: [{
        id: "change-plan-summary",
        threadId: "thread-1",
        turnId: "turn-1",
        toolCallId: "call-edit",
        createdAtMs: 2,
        undone: false,
        files: [{
          path: "src/App.css",
          destinationPath: null,
          operation: "modify",
          beforeHash: "before-plan-summary",
          afterHash: "after-plan-summary",
          beforeContent: "old\n",
          afterContent: "new\n",
          unifiedDiff: "--- a/src/App.css\n+++ b/src/App.css\n@@ -1 +1 @@\n-old\n+new\n",
        }],
      }] },
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
      extension_overview: extensionOverview,
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
          if (command === "plugin:dialog|open") {
            const selected = localStorage.getItem("kcoder_e2e_attachment_dialog_paths");
            return selected ? JSON.parse(selected) : null;
          }
          if (command === "plugin:fs|stat") {
            const raw = localStorage.getItem("kcoder_e2e_attachment_file_bytes");
            const size = raw ? (JSON.parse(raw) as number[]).length : 0;
            return {
              isFile: true,
              isDirectory: false,
              isSymlink: false,
              size,
              mtime: null,
              atime: null,
              birthtime: null,
              readonly: true,
            };
          }
          if (command === "plugin:fs|read_file") {
            const bytes = localStorage.getItem("kcoder_e2e_attachment_file_bytes");
            return bytes ? JSON.parse(bytes) : [];
          }
          if (command === "plugin:event|listen") {
            if (typeof args?.event === "string" && typeof args.handler === "number") {
              tauriEventCallbackIds.set(args.event, args.handler);
            }
            if (args?.event === "agent-event" && typeof args.handler === "number") {
              agentEventCallbackId = args.handler;
            } else if (args?.event === "thread-mailbox-changed" && typeof args.handler === "number") {
              mailboxEventCallbackId = args.handler;
            }
            return 1;
          }
          if (command === "workspace_state") {
            const forcedPath = localStorage.getItem("kcoder_e2e_workspace_path");
            const forcedRecentProjects = localStorage.getItem("kcoder_e2e_recent_projects");
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
            if (forcedRecentProjects) {
              workspaceState = {
                ...workspaceState,
                recent: JSON.parse(forcedRecentProjects),
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
          if (command === "create_thread") {
            const inProject = args?.inProject !== false;
            return {
              ...secondThread,
              inProject,
              workspacePath: inProject ? workspaceState.current.path : null,
            };
          }
          if (command === "get_provider_catalog") return providerCatalog;
          if (command === "list_builtin_workflows") return workflowDefinitions;
          if (command === "get_workflow_skill_readiness") {
            const workflow = workflowDefinitions.find((item) => item.id === args?.workflowId);
            if (!workflow) throw new Error("workflow not found");
            return workflowReadiness(workflow);
          }
          if (command === "get_workflow_run") {
            const restored = localStorage.getItem("kcoder_e2e_workflow_run");
            if (restored) workflowRun = JSON.parse(restored);
            return workflowRun?.threadId === args?.threadId ? workflowRun : null;
          }
          if (command === "get_plan") {
            const configured = localStorage.getItem("kcoder_e2e_plan");
            if (configured) return JSON.parse(configured);
          }
          if (command === "cancel_workflow_run") {
            const request = args?.request as { threadId?: string; runId?: string } | undefined;
            if (!workflowRun || workflowRun.threadId !== request?.threadId || workflowRun.id !== request?.runId) {
              throw new Error("workflow run was not found for this thread");
            }
            workflowRun = {
              ...workflowRun,
              state: "cancelled",
              updatedAtMs: Date.now(),
              revision: workflowRun.revision + 1,
            };
            return workflowRun;
          }
          if (command === "turn_start") {
            runTurnCalls.push(args ?? null);
            startedTurnCount += 1;
            const request = args?.request as { threadId?: string; input?: string; agentMode?: string } | undefined;
            const threadId = String(request?.threadId ?? "thread-1");
            const turnId = `turn-start-${startedTurnCount}`;
            const queued = activeTurnIds.has(threadId);
            const workflowId = typeof args?.workflowId === "string" ? args.workflowId : null;
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
                  workflowId,
                  attachments,
                },
              ]);
              bumpMailboxRevision(threadId);
            } else {
              if (workflowId && workflowRun?.state !== "active") {
                const definition = workflowDefinitions.find((item) => item.id === workflowId);
                if (!definition) throw new Error(`unknown built-in workflow: ${workflowId}`);
                workflowRun = {
                  schemaVersion: 1,
                  id: `workflow-run-${startedTurnCount}`,
                  threadId,
                  workflowId,
                  objective: String(request?.input ?? ""),
                  state: "active",
                  currentNodeId: definition.nodes[0]?.id ?? null,
                  currentNodeIndex: 0,
                  nodeCount: definition.nodes.length,
                  completedNodes: [],
                  createdAtMs: Date.now(),
                  updatedAtMs: Date.now(),
                  revision: 1,
                };
              }
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
            restoreMailboxFixture();
            const threadId = String(args?.threadId ?? "thread-1");
            return {
              schemaVersion: 1,
              threadId,
              revision: mailboxRevisionByThread.get(threadId) ?? 0,
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
            if (next.length !== pending.length) bumpMailboxRevision(threadId);
            return next.length !== pending.length;
          }
          if (command === "clear_thread_mailbox") {
            const threadId = String(args?.threadId ?? "");
            const removed = mailboxByThread.get(threadId)?.length ?? 0;
            mailboxByThread.set(threadId, []);
            if (removed > 0) bumpMailboxRevision(threadId);
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
            if (queued.workflowId) {
              throw new Error("a queued workflow start must begin as its own turn");
            }
            mailboxByThread.set(
              request.threadId,
              pending.filter((item) => item.turnId !== request.queuedTurnId),
            );
            bumpMailboxRevision(request.threadId);
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
            const forcedThreadWorkspacePath = localStorage.getItem("kcoder_e2e_thread_workspace_path");
            if (forcedThreadWorkspacePath) return [{ ...thread, workspacePath: forcedThreadWorkspacePath }];
          }
          if (command === "list_subagents") {
            const configured = localStorage.getItem("kcoder_e2e_subagents");
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
            const forcedThreadWorkspacePath = localStorage.getItem("kcoder_e2e_thread_workspace_path");
            if (forcedThreadWorkspacePath) {
              const detail = responses.read_thread as { summary: typeof thread };
              return {
                ...detail,
                summary: { ...detail.summary, workspacePath: forcedThreadWorkspacePath },
              };
            }
            const threadId = String(args?.threadId ?? "");
            const activeTurnId = activeTurnIds.get(threadId);
            if (activeTurnId) {
              return {
                ...(responses.read_thread as Record<string, unknown>),
                lastTurn: { turnId: activeTurnId, state: "streaming", error: null },
              };
            }
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
          if (command === "mcp_config") return mcpConfig;
          if (command === "plugin_overview") {
            if (localStorage.getItem("kcoder_e2e_plugin_empty") === "true") {
              return { ...pluginOverview, plugins: [] };
            }
            return pluginOverview;
          }
          if (command === "set_plugin_enabled") {
            const pluginId = String(args?.pluginId ?? "");
            if (localStorage.getItem("kcoder_e2e_plugin_toggle_error") === pluginId) {
              throw new Error("plugin runtime refresh failed");
            }
            pluginOverview = {
              ...pluginOverview,
              plugins: pluginOverview.plugins.map((plugin) => plugin.id === pluginId
                ? {
                    ...plugin,
                    enabled: Boolean(args?.enabled),
                    state: args?.enabled ? "loaded" : "disabled",
                    error: null,
                  }
                : plugin),
            };
            return pluginOverview;
          }
          if (command === "delete_plugin") {
            const pluginId = String(args?.pluginId ?? "");
            pluginOverview = {
              ...pluginOverview,
              plugins: pluginOverview.plugins.filter((plugin) => plugin.id !== pluginId),
            };
            return pluginOverview;
          }
          if (command === "user_rules") return userRules;
          if (command === "save_user_rule") {
            const request = (args?.request ?? {}) as { id?: string | null; title?: string; content?: string };
            const timestamp = 1_785_000_060_000;
            if (request.id) {
              userRules = {
                ...userRules,
                rules: userRules.rules.map((rule) => rule.id === request.id
                  ? {
                      ...rule,
                      title: String(request.title ?? ""),
                      content: String(request.content ?? ""),
                      updatedAtMs: timestamp,
                    }
                  : rule),
              };
            } else {
              userRules = {
                ...userRules,
                rules: [...userRules.rules, {
                  id: "22222222-2222-4222-8222-222222222222",
                  title: String(request.title ?? ""),
                  content: String(request.content ?? ""),
                  createdAtMs: timestamp,
                  updatedAtMs: timestamp,
                }],
              };
            }
            return userRules;
          }
          if (command === "delete_user_rule") {
            userRules = {
              ...userRules,
              rules: userRules.rules.filter((rule) => rule.id !== String(args?.id ?? "")),
            };
            return userRules;
          }
          if (command === "save_mcp_config") {
            const configScope = args?.scope === "project" ? "project" : "global";
            const content = String(args?.content ?? "");
            if (configScope === "project") {
              mcpConfig = {
                ...mcpConfig,
                project: { ...mcpConfig.project, exists: true, content, error: null },
              };
            } else {
              mcpConfig = {
                ...mcpConfig,
                global: { ...mcpConfig.global, exists: true, content, error: null },
              };
            }
            return mcpConfig;
          }
          if (
            command === "set_extension_enabled"
            || command === "save_mcp_secret"
            || command === "delete_mcp_secret"
          ) return mcpConfig.overview;
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
          if (command === "wait_pty") {
            const exited = localStorage.getItem("kcoder_e2e_pty_state") === "exited";
            if (!exited) await new Promise(() => undefined);
            return {
              id: String(args?.sessionId ?? "pty-1"),
              state: { state: "exited", code: 0 },
              startedAtMs: 1,
              finishedAtMs: 2,
              rows: 24,
              cols: 80,
              nextCursor: 1,
              oldestCursor: 0,
              outputTruncated: false,
            };
          }
          if (command === "read_pty_output") {
            const cursor = Number(args?.cursor ?? 0);
            const text = localStorage.getItem("kcoder_e2e_pty_output") ?? "PS D:\\code\\k-coder> ";
            const chunks = cursor === 0 ? [{ cursor: 0, text }] : [];
            return { chunks, nextCursor: cursor === 0 ? 1 : cursor, oldestCursor: 0, truncatedBeforeCursor: false };
          }
          if (command === "write_pty") {
            ptyWriteMetrics.active += 1;
            ptyWriteMetrics.maxConcurrent = Math.max(ptyWriteMetrics.maxConcurrent, ptyWriteMetrics.active);
            try {
              const delayMs = Number(localStorage.getItem("kcoder_e2e_pty_write_delay_ms") ?? 0);
              if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
              ptyWrites.push(String(args?.input ?? ""));
              return undefined;
            } finally {
              ptyWriteMetrics.active -= 1;
            }
          }
          if (command === "resize_pty" || command === "close_pty") return undefined;
          if (
            (command === "open_workspace_file" || command === "reveal_workspace_file")
            && localStorage.getItem("kcoder_e2e_external_open_error")
          ) {
            throw new Error(localStorage.getItem("kcoder_e2e_external_open_error") ?? "external open failed");
          }
          if (command === "extract_local_document") {
            const delayMs = Number(localStorage.getItem("kcoder_e2e_attachment_extract_delay_ms") ?? 0);
            if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
            const name = String(args?.name ?? "attachment.txt");
            const dataUrl = String(args?.dataUrl ?? "");
            const encoded = dataUrl.split(",", 2)[1] ?? "";
            const bytes = Uint8Array.from(atob(encoded), (value) => value.charCodeAt(0));
            const spreadsheetContent = /\.(xlsx|xls|xlsm|xlsb)$/i.test(name)
              ? "[工作表: 预算]\n项目\t金额\n住宿\t128.5\n"
              : null;
            return {
              path: `attachment://fixture/${name}`,
              name,
              kind: "document",
              content: spreadsheetContent ?? new TextDecoder().decode(bytes),
              size: bytes.byteLength,
              truncated: false,
            };
          }
          return responses[command] ?? null;
        },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
      __invoked: [],
      __invocationArgs: invocationArgs,
      __ptyStartRequests: ptyStartRequests,
      __ptyWrites: ptyWrites,
      __ptyWriteMetrics: ptyWriteMetrics,
      __runTurnCalls: runTurnCalls,
      __lastProviderRequest: null,
      __lastActivatedProvider: null,
      __lastApprovalMode: null,
      __registeredTauriEvents: tauriEventCallbackIds,
      __emitTauriEvent: (event: string, payload: unknown) => {
        const handlerId = tauriEventCallbackIds.get(event);
        if (handlerId === undefined) throw new Error(`${event} listener is not ready`);
        callbacks.get(handlerId)?.({ event, id: 1, payload });
      },
      __emitAgentEvent: (event: unknown) => {
        const agentEvent = event as { type?: string; threadId?: string; turnId?: string };
        if (agentEvent.threadId && agentEvent.turnId && agentEvent.type === "turn_started") {
          activeTurnIds.set(agentEvent.threadId, agentEvent.turnId);
          const pending = mailboxByThread.get(agentEvent.threadId) ?? [];
          const next = pending.filter((item) => item.turnId !== agentEvent.turnId);
          if (next.length !== pending.length) {
            mailboxByThread.set(agentEvent.threadId, next);
            const revision = bumpMailboxRevision(agentEvent.threadId);
            emitMailboxChanged(agentEvent.threadId, revision);
          }
        } else if (
          agentEvent.threadId
          && activeTurnIds.get(agentEvent.threadId) === agentEvent.turnId
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

test("keeps composer popover surfaces consistent and closes the mode menu outside", async ({ page }, testInfo) => {
  await page.goto("/");
  if (testInfo.project.name === "narrow") await page.setViewportSize({ width: 420, height: 820 });
  const composer = page.locator(".composer");
  const modeTrigger = page.getByRole("button", { name: "选择模式" });
  const modeMenu = page.locator(".mode-menu");
  const surfaceStyle = (selector: string) => page.locator(selector).evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      border: style.border,
      borderRadius: style.borderRadius,
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
    };
  });
  const expectMenuWithinComposer = async (selector: string) => {
    const [menuBox, composerBox] = await Promise.all([
      page.locator(selector).boundingBox(),
      composer.boundingBox(),
    ]);
    expect(menuBox).not.toBeNull();
    expect(composerBox).not.toBeNull();
    expect(menuBox!.x).toBeGreaterThanOrEqual(composerBox!.x + 11);
    expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(composerBox!.x + composerBox!.width - 11);
    expect(menuBox!.y + menuBox!.height).toBeLessThanOrEqual(composerBox!.y - 7);
  };

  await modeTrigger.click();
  await expect(modeMenu).toBeVisible();
  await expect(modeMenu).toHaveClass(/composer-popover-surface/);
  await expectMenuWithinComposer(".mode-menu");
  const expectedSurface = await surfaceStyle(".mode-menu");

  await page.getByRole("heading", { name: "Phase 6 workbench" }).click();
  await expect(modeMenu).toBeHidden();

  await page.locator(".context-progress-trigger").click();
  const contextPopover = page.locator(".context-progress-popover");
  await expect(contextPopover).toBeVisible();
  await expect(contextPopover).toHaveClass(/composer-popover-surface/);
  expect(await surfaceStyle(".context-progress-popover")).toEqual(expectedSurface);
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: /操作批准方式/ }).click();
  const approvalMenu = page.locator(".approval-mode-menu");
  await expect(approvalMenu).toBeVisible();
  await expect(approvalMenu).toHaveClass(/composer-popover-surface/);
  expect(await surfaceStyle(".approval-mode-menu")).toEqual(expectedSurface);
  await expectMenuWithinComposer(".approval-mode-menu");
  await page.keyboard.press("Escape");

  await modeTrigger.click();
  await page.keyboard.press("Escape");
  await expect(modeMenu).toBeHidden();
  await expect(modeTrigger).toBeFocused();

  await modeTrigger.click();
  await page.waitForTimeout(250);
  await page.screenshot({ path: testInfo.outputPath(`composer-mode-menu-${testInfo.project.name}.png`), fullPage: true });
  await page.keyboard.press("Escape");
});

test("starts a built-in robot workflow from the composer and cancels it after the turn", async ({ page }, testInfo) => {
  await page.goto("/");
  const robotSelector = page.getByRole("button", { name: "选择机器人" });
  await robotSelector.click();
  const robotMenu = page.getByRole("menu", { name: "机器人列表" });
  await expect(robotMenu.getByRole("menuitemradio")).toHaveCount(4);
  await robotMenu.getByRole("menuitemradio", { name: /软件测试机器人/ }).click();
  await expect(robotSelector).toContainText("软件测试机器人");
  await expect(page.getByRole("button", { name: "选择模式" })).toBeDisabled();

  await page.getByRole("textbox", { name: "消息" }).fill("验证机器人工作流");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect.poll(() => page.evaluate(() => {
    const call = (window as unknown as {
      __invocationArgs: Record<string, { workflowId?: string; request?: { agentMode?: string; input?: string } }>;
    }).__invocationArgs.turn_start;
    return { workflowId: call?.workflowId, agentMode: call?.request?.agentMode, input: call?.request?.input };
  })).toEqual({
    workflowId: "quality-assurance",
    agentMode: "craft",
    input: "验证机器人工作流",
  });

  const control = page.getByLabel("机器人工作流 软件测试机器人");
  await expect(control).toBeVisible();
  await expect(control).toContainText("测试策略制定");
  await expect(control).toContainText("1 / 7");
  await expect(control.locator(".workflow-control-skills")).toContainText("test-strategy-planning");
  await expect(control.getByRole("progressbar", { name: "工作流进度" })).toHaveAttribute("aria-valuenow", "0");
  await page.screenshot({ path: testInfo.outputPath("builtin-robot-running.png"), fullPage: true });

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 5,
      threadId: "thread-1",
      turnId: "turn-start-1",
      type: "turn_completed",
      phase: "complete",
      message: { schemaVersion: 1, id: "workflow-answer", role: "assistant", content: [{ type: "text", text: "本轮完成" }], createdAtMs: 20 },
      usage: null,
      startedAtMs: 10,
      completedAtMs: 20,
      durationMs: 10,
    });
  });
  await control.getByRole("button", { name: "停止机器人工作流" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.cancel_workflow_run)).toEqual({
    request: { threadId: "thread-1", runId: "workflow-run-1" },
  });
  await expect(control).toHaveCount(0);
  await expect(robotSelector).toContainText("普通智能体");
});

test("restores persisted robot node progress and lists built-in definitions", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_workflow_run", JSON.stringify({
      schemaVersion: 1,
      definitionVersion: 2,
      id: "workflow-restored",
      threadId: "thread-1",
      workflowId: "requirements-design",
      objective: "形成设计",
      state: "active",
      currentNodeId: "user-story-modeling",
      currentNodeIndex: 2,
      nodeCount: 7,
      completedNodes: [
        { nodeId: "requirements-intake", summary: "done", evidence: ["docs"], completedAtMs: 2 },
        { nodeId: "business-boundary", summary: "done", evidence: ["scope"], completedAtMs: 3 },
      ],
      createdAtMs: 1,
      updatedAtMs: 3,
      revision: 3,
    }));
  });
  await page.goto("/");

  const control = page.getByLabel("机器人工作流 需求设计");
  await expect(control).toContainText("用户故事与功能建模");
  await expect(control).toContainText("3 / 7");
  await expect(control.getByRole("progressbar", { name: "工作流进度" })).toHaveAttribute("aria-valuenow", "2");
  await expect(page.getByRole("button", { name: "选择机器人" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "选择模式" })).toBeDisabled();

  await page.locator('button[aria-label="设置"]:visible').click();
  await page.getByRole("button", { name: "机器人", exact: true }).click();
  await expect(page.getByRole("heading", { name: "内置机器人" })).toBeVisible();
  await expect(page.locator(".robot-row")).toHaveCount(3);
  await expect(page.locator(".robot-row--active")).toContainText("用户故事与功能建模");
  await expect(page.locator(".robot-row--active .robot-skill-summary")).toContainText("5 本地技能");
  await expect(page.locator(".robot-row--active .robot-skill-summary")).toContainText("4 插件技能");
  await expect(page.locator(".robot-row--active")).toContainText("requirements-intake");
  await expect(page.locator(".robot-row--active .robot-system-prompt"))
    .toContainText("需求设计机器人 - 角色定义");
  await page.screenshot({ path: testInfo.outputPath("builtin-robots-settings.png"), fullPage: true });
});

test("groups Skills and locks robot-managed Skills", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  await page.getByRole("button", { name: /Skills/ }).click();

  await expect(page.getByRole("region", { name: "需求与规划" })).toBeVisible();
  await expect(page.getByRole("region", { name: "质量与评审" })).toBeVisible();
  const managedSkill = page.locator(".extension-row").filter({ hasText: "Robot requirements intake" });
  await expect(managedSkill.getByText("机器人必需", { exact: true })).toBeVisible();
  await expect(managedSkill.getByRole("checkbox")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("robot-managed-skills.png"), fullPage: true });
});

test("restores a queued robot identity from the mailbox snapshot", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_mailbox", JSON.stringify({
      threadId: "thread-1",
      activeTurnId: null,
      pending: [{
        schemaVersion: 1,
        turnId: "turn-workflow-queued",
        threadId: "thread-1",
        kind: "message",
        input: "执行回归测试",
        agentMode: "craft",
        workflowId: "quality-assurance",
        attachments: [],
      }],
    }));
  });
  await page.goto("/");

  const selector = page.getByRole("button", { name: "选择机器人" });
  await expect(selector).toContainText("软件测试机器人");
  await expect(selector).toBeDisabled();
  await expect(page.getByRole("button", { name: "选择模式" })).toBeDisabled();
  await page.getByRole("button", { name: "队列 (1)" }).click();
  await expect(page.getByText("执行回归测试")).toBeVisible();
});

test("uses composer quick actions for files, attachments, Skills, and extension entry points", async ({ page }, testInfo) => {
  await page.goto("/");
  const toolbar = page.getByRole("toolbar", { name: "输入快捷操作" });
  const fileAction = toolbar.getByRole("button", { name: "引用工作区文件" });
  const attachmentAction = toolbar.getByRole("button", { name: "添加附件" });
  const skillAction = toolbar.getByRole("button", { name: "使用 Skill" });
  const moreAction = toolbar.getByRole("button", { name: "更多操作" });
  const agentSelector = toolbar.getByRole("button", { name: "选择机器人" });
  const composer = page.getByRole("textbox", { name: "消息" });
  const subagentToggle = page.getByRole("button", { name: "子智能体", exact: true });

  await expect(toolbar.getByRole("button")).toHaveCount(5);
  await expect(subagentToggle).toHaveClass(/segmented-button--subagent/);
  await expect(subagentToggle.locator('[data-icon="subagent"]')).toHaveCount(1);
  await expect(agentSelector).toHaveAttribute("aria-label", "选择机器人");
  await expect(agentSelector.locator('[data-icon="robot"]')).toHaveCount(1);
  await expect(toolbar.locator(".workflow-selector--compact")).toHaveCount(1);
  await expect(attachmentAction).toHaveAttribute("title", "添加附件");
  await expect(page.locator(".composer .project-selector")).toHaveCount(1);
  await fileAction.click();
  const fileSuggestions = page.getByRole("listbox", { name: "文件引用" });
  await expect(fileSuggestions).toBeVisible();
  await expect(composer).toHaveValue("@");
  await fileSuggestions.getByRole("option", { name: /src\/App\.tsx/ }).click();
  await expect(composer).toHaveValue("@src/App.tsx ");

  await composer.fill("");
  await skillAction.click();
  const skillSuggestions = page.getByRole("listbox", { name: "Skills" });
  await expect(skillSuggestions).toBeVisible();
  await expect(composer).toHaveValue("/");
  await skillSuggestions.getByRole("option", { name: /\/workspace-review/ }).click();
  await expect(composer).toHaveValue("/workspace-review ");

  await attachmentAction.click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.includes("plugin:dialog|open"))).toBe(true);
  expect(await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs["plugin:dialog|open"])).toEqual({ options: { multiple: true } });

  await moreAction.click();
  const menu = page.getByRole("menu", { name: "添加内容" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitem")).toHaveText([
    "添加插件",
    "添加 MCP",
    "添加小程序",
    "添加 Workflow",
  ]);
  await expect(menu.getByRole("menuitem", { name: "添加插件" })).toBeFocused();

  const [menuBox, triggerBox] = await Promise.all([menu.boundingBox(), moreAction.boundingBox()]);
  expect(menuBox).not.toBeNull();
  expect(triggerBox).not.toBeNull();
  expect(menuBox!.x).toBeGreaterThanOrEqual(0);
  expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(menuBox!.y).toBeGreaterThanOrEqual(0);
  expect(menuBox!.y + menuBox!.height).toBeLessThanOrEqual(triggerBox!.y);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`composer-add-menu-${testInfo.project.name}.png`), fullPage: true });

  await page.keyboard.press("End");
  await expect(menu.getByRole("menuitem", { name: "添加 Workflow" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "添加插件" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(moreAction).toBeFocused();

  for (const entry of [
    { menuLabel: "添加 MCP", heading: "MCP 配置", pending: false },
    { menuLabel: "添加插件", heading: "本地插件", pending: false },
    { menuLabel: "添加小程序", heading: "小程序", pending: true },
    { menuLabel: "添加 Workflow", heading: "Workflows", pending: true },
  ]) {
    await moreAction.click();
    await menu.getByRole("menuitem", { name: entry.menuLabel }).click();
    const settings = page.getByRole("dialog", { name: "设置" });
    await expect(settings.getByRole("heading", { name: entry.heading, exact: true })).toBeVisible();
    if (entry.pending) await expect(settings.getByText("尚未接入", { exact: true })).toBeVisible();
    await settings.getByRole("button", { name: "关闭设置" }).click();
  }
});

test("polls, cancels, and refreshes knowledge sources without layout overflow", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.evaluate(() => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
      __knowledgeActions: string[];
      __knowledgeAdds: Array<{ collectionId?: string; workspaceRelativePath?: string }>;
    };
    const originalInvoke = host.__TAURI_INTERNALS__.invoke;
    host.__knowledgeActions = [];
    host.__knowledgeAdds = [];
    let primaryCollectionExists = true;
    let sourceExists = true;
    let sourceState = "queued";
    let activeJobId: string | null = "knowledge-job-1";
    let createdCollectionName: string | null = null;
    const collection = (id: string, name: string, sourceCount: number, indexedChunkCount: number) => ({
      id,
      name,
      scope: "workspace",
      scopeKey: "D:\\code\\k-coder",
      enabled: true,
      sourceCount,
      indexedChunkCount,
      updatedAtMs: Date.now(),
    });
    const source = () => ({
      sourceId: "knowledge-source-1",
      relativePath: "docs/knowledge/architecture-and-operations-guide.md",
      sizeBytes: 4096,
      contentHashPrefix: "0123456789ab",
      activeRevisionId: sourceState === "cancelled" ? null : "knowledge-revision-1",
      activeEmbeddingModel: null,
      activeEmbeddingDimension: 0,
      activeEmbeddingEncodingFormat: "float",
      embeddingStatus: "lexical_only",
      state: sourceState,
      chunkCount: sourceState === "cancelled" ? 0 : 3,
      lastIndexedAtMs: null,
      lastErrorCode: sourceState === "cancelled" ? "KC_CANCELLED" : null,
      initialJobId: activeJobId,
    });
    host.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "get_knowledge_settings") {
        return {
          enabled: true,
          autoSearch: false,
          maxResults: 6,
          maxChunkTokens: 1500,
          knowledgeBudgetPercent: 8,
          semanticEnabled: false,
          embeddingProvider: "siliconflow",
          embeddingModel: "BAAI/bge-m3",
          embeddingDimension: 0,
          embeddingConfigured: false,
          embeddingStatus: "lexical_only",
        };
      }
      if (command === "get_embedding_settings") {
        return {
          provider: "siliconflow",
          endpoint: "https://api.siliconflow.cn/v1/embeddings",
          model: "BAAI/bge-m3",
          semanticEnabled: false,
          encodingFormat: "float",
          batchSize: 16,
          timeoutMs: 30000,
          maxVectorScanChunks: 10000,
          modelMaxInputTokens: 8192,
          vectorDimension: 0,
          embeddingConfigured: false,
          embeddingStatus: "lexical_only",
        };
      }
      if (command === "list_knowledge_collections") {
        return [
          ...(primaryCollectionExists ? [collection("knowledge-collection-1", "项目知识", sourceExists ? 1 : 0, !sourceExists || sourceState === "cancelled" ? 0 : 3)] : []),
          ...(createdCollectionName ? [collection("knowledge-collection-2", createdCollectionName, 0, 0)] : []),
        ];
      }
      if (command === "upsert_knowledge_collection") {
        host.__knowledgeActions.push(command);
        const request = args?.request as { name?: string } | undefined;
        createdCollectionName = String(request?.name ?? "");
        return collection("knowledge-collection-2", createdCollectionName, 0, 0);
      }
      if (command === "list_knowledge_sources") {
        return sourceExists && args?.collectionId === "knowledge-collection-1" ? [source()] : [];
      }
      if (command === "add_knowledge_source") {
        const request = (args?.request ?? {}) as { collectionId?: string; workspaceRelativePath?: string };
        if (request.workspaceRelativePath?.startsWith("../")) {
          throw { code: "KC_PATH_OUTSIDE_WORKSPACE", message: "source path is outside workspace" };
        }
        host.__knowledgeActions.push(command);
        host.__knowledgeAdds.push(request);
        sourceExists = true;
        return source();
      }
      if (command === "cancel_knowledge_index_job") {
        host.__knowledgeActions.push(command);
        sourceState = "cancelled";
        activeJobId = null;
        return { jobId: String(args?.jobId), sourceId: "knowledge-source-1", state: "cancelled" };
      }
      if (command === "refresh_knowledge_source") {
        host.__knowledgeActions.push(command);
        sourceState = "indexing";
        activeJobId = "knowledge-job-2";
        return { jobId: activeJobId, sourceId: "knowledge-source-1", state: "running" };
      }
      if (command === "delete_knowledge_source") {
        host.__knowledgeActions.push(command);
        sourceExists = false;
        activeJobId = null;
        return { deletedSourceId: "knowledge-source-1" };
      }
      if (command === "delete_knowledge_collection") {
        host.__knowledgeActions.push(command);
        primaryCollectionExists = false;
        return { deletedCollectionId: "knowledge-collection-1" };
      }
      return originalInvoke(command, args);
    };
  });

  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: /^知识库/ }).click();
  await expect(settings.getByRole("heading", { name: "知识库", exact: true })).toBeVisible();
  await expect(settings.getByText("等待索引", { exact: false })).toBeVisible();

  const collectionName = settings.getByRole("textbox", { name: "Collection 名称" });
  const createCollection = settings.getByRole("button", { name: "创建", exact: true });
  await createCollection.click();
  await expect(settings.getByText("请输入 Collection 名称", { exact: true })).toBeVisible();
  await expect(collectionName).toBeFocused();
  await expect(collectionName).toHaveAttribute("aria-invalid", "true");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __knowledgeActions: string[] }
  ).__knowledgeActions.filter((command) => command === "upsert_knowledge_collection").length)).toBe(0);

  await collectionName.fill("  团队文档  ");
  await expect(settings.getByText("请输入 Collection 名称", { exact: true })).toHaveCount(0);
  await createCollection.click();
  await expect(settings.getByText("团队文档", { exact: true })).toBeVisible();
  await expect(collectionName).toHaveValue("");
  await expect(collectionName).toHaveAttribute("aria-invalid", "false");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __knowledgeActions: string[] }
  ).__knowledgeActions.filter((command) => command === "upsert_knowledge_collection").length)).toBe(1);

  const cancel = settings.getByRole("button", { name: /取消 .* 的索引/ });
  const refresh = settings.getByRole("button", { name: /刷新 .*architecture-and-operations-guide\.md/ });
  await expect(cancel).toBeVisible();
  await expect(refresh).toBeDisabled();
  await cancel.click();
  await expect(settings.getByText("已取消", { exact: false })).toBeVisible();
  await expect(cancel).toHaveCount(0);
  await expect(refresh).toBeEnabled();

  await page.evaluate(() => {
    localStorage.setItem(
      "kcoder_e2e_attachment_dialog_paths",
      JSON.stringify(["D:\\code\\k-coder\\docs\\knowledge\\picked.md"]),
    );
  });
  await settings.getByRole("button", { name: "选择文件" }).first().click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __knowledgeAdds: Array<{ workspaceRelativePath?: string }> }
  ).__knowledgeAdds.length)).toBe(1);
  expect(await page.evaluate(() => {
    return (window as unknown as { __knowledgeAdds: Array<{ workspaceRelativePath?: string }> })
      .__knowledgeAdds[0].workspaceRelativePath;
  })).toBe("docs/knowledge/picked.md");

  const createdCard = settings.locator(".knowledge-collection-card").filter({ hasText: "团队文档" });
  await createdCard.getByLabel("来源路径").fill("../outside.md");
  await createdCard.getByRole("button", { name: "添加路径" }).click();
  await expect(settings.getByRole("alert")).toHaveText("KC_PATH_OUTSIDE_WORKSPACE：source path is outside workspace");

  await settings.getByRole("button", { name: "关闭设置" }).click();
  await expect(settings).toBeHidden();
  await page.locator('button[aria-label="设置"]:visible').click();
  const reopenedSettings = page.getByRole("dialog", { name: "设置" });
  await reopenedSettings.getByRole("button", { name: /^知识库/ }).click();
  await expect(reopenedSettings.getByText("docs/knowledge/architecture-and-operations-guide.md", { exact: true })).toBeVisible();
  await expect(reopenedSettings.getByText("已取消", { exact: false })).toBeVisible();
  await expect(reopenedSettings.getByRole("button", { name: "选择文件" }).first()).toBeVisible();

  await refresh.click();
  await expect(settings.getByText("正在索引", { exact: false })).toBeVisible();
  await expect(settings.getByRole("button", { name: /取消 .* 的索引/ })).toBeVisible();
  await expect(refresh).toBeDisabled();
  await expect.poll(() => page.evaluate(() => {
    const invoked = (window as unknown as { __knowledgeActions: string[] }).__knowledgeActions;
    return {
      cancelled: invoked.filter((command) => command === "cancel_knowledge_index_job").length,
      refreshed: invoked.filter((command) => command === "refresh_knowledge_source").length,
    };
  })).toEqual({ cancelled: 1, refreshed: 1 });

  const layout = await settings.evaluate((element) => {
    const content = element.querySelector<HTMLElement>(".settings-content")!;
    const sourceRow = element.querySelector<HTMLElement>(".knowledge-source-row")!;
    const actions = element.querySelector<HTMLElement>(".knowledge-source-actions")!;
    const rowBounds = sourceRow.getBoundingClientRect();
    const actionBounds = actions.getBoundingClientRect();
    return {
      contentFits: content.scrollWidth <= content.clientWidth,
      rowFits: sourceRow.scrollWidth <= sourceRow.clientWidth,
      actionsInsideRow: actionBounds.left >= rowBounds.left && actionBounds.right <= rowBounds.right + 1,
    };
  });
  expect(layout).toEqual({ contentFits: true, rowFits: true, actionsInsideRow: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`knowledge-settings-${testInfo.project.name}.png`), fullPage: true });

  const deleteSource = settings.getByRole("button", { name: /删除知识来源 .*architecture-and-operations-guide\.md/ });
  await deleteSource.click();
  const sourceConfirmation = page.getByRole("dialog", { name: "删除知识来源" });
  await expect(sourceConfirmation).toContainText("工作区原文件不会被删除");
  await page.keyboard.press("Escape");
  await expect(sourceConfirmation).toBeHidden();
  await expect(settings).toBeVisible();
  await deleteSource.click();
  await page.getByRole("dialog", { name: "删除知识来源" }).getByRole("button", { name: "删除索引" }).click();
  await expect(settings.getByText("docs/knowledge/architecture-and-operations-guide.md", { exact: true })).toHaveCount(0);

  await settings.getByRole("button", { name: "删除 Collection 项目知识" }).click();
  const collectionConfirmation = page.getByRole("dialog", { name: "删除 Collection" });
  await expect(collectionConfirmation).toContainText("全部本地索引");
  await collectionConfirmation.getByRole("button", { name: "删除索引" }).click();
  await expect(settings.getByText("项目知识", { exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => {
    const invoked = (window as unknown as { __knowledgeActions: string[] }).__knowledgeActions;
    return {
      deletedSource: invoked.filter((command) => command === "delete_knowledge_source").length,
      deletedCollection: invoked.filter((command) => command === "delete_knowledge_collection").length,
    };
  })).toEqual({ deletedSource: 1, deletedCollection: 1 });
});

test("disables project-only composer quick actions outside a project", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("kcoder_e2e_thread_override", JSON.stringify({
    inProject: false,
    workspacePath: null,
  })));
  await page.reload();

  const toolbar = page.getByRole("toolbar", { name: "输入快捷操作" });
  const fileAction = toolbar.getByRole("button", { name: "引用工作区文件" });
  const skillAction = toolbar.getByRole("button", { name: "使用 Skill" });
  await expect(fileAction).toBeDisabled();
  await expect(fileAction).toHaveAttribute("title", "独立会话不能引用工作区文件");
  await expect(skillAction).toBeDisabled();
  await expect(skillAction).toHaveAttribute("title", "独立会话不能使用 Skill");
});

test("renders the composer add menu in dark mode", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "One dark-theme visual check is sufficient");
  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  await page.getByRole("button", { name: "更多操作" }).click();
  const menu = page.getByRole("menu", { name: "添加内容" });
  await expect(menu).toBeVisible();
  const colors = await menu.evaluate((element) => {
    const probe = document.createElement("span");
    probe.style.color = "var(--color-ink)";
    document.body.appendChild(probe);
    const expectedText = getComputedStyle(probe).color;
    probe.remove();
    return {
      background: getComputedStyle(element).backgroundColor,
      text: getComputedStyle(element.querySelector("button")!).color,
      expectedText,
    };
  });
  expect(colors.background).not.toBe("rgba(0, 0, 0, 0)");
  expect(colors.text).toBe(colors.expectedText);
  await page.screenshot({ path: testInfo.outputPath("composer-add-menu-dark.png"), fullPage: true });
});

test("supports the primary workbench inspection flow", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("textbox", { name: "消息" })).toHaveCSS("font-size", "13px");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_plan)).toEqual({ threadId: "thread-1" });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invocationArgs: Record<string, unknown> }).__invocationArgs.get_goal)).toEqual({ threadId: "thread-1" });
  await expect(page.getByRole("heading", { name: "Phase 6 workbench" })).toBeVisible();
  await expect(page.getByText("检查完成。", { exact: true })).toBeVisible();
  await expect(page.getByText("执行了 1.8s", { exact: true })).toBeVisible();
  await expect(page.locator(".turn-execution > summary > svg.lucide-circle-check")).toHaveCount(1);
  await expect(page.locator(".conversation-header").getByText("1280 tokens", { exact: true })).toHaveCount(0);
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeHidden();
  await page.screenshot({ path: testInfo.outputPath("collapsed-turn.png"), fullPage: true });
  await page.getByText("执行了 1.8s", { exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath("collapsed-steps.png"), fullPage: true });
  await expect(page.locator(".turn-event-step--provider_context")).toHaveCount(0);
  await expect(page.locator(".turn-event-step--usage")).toHaveCount(0);
  const planProgress = page.locator(".plan-progress").first();
  const planProgressTrigger = planProgress.getByRole("button", { name: /执行计划：第 2\/2 步/ });
  await expect(planProgressTrigger).toContainText("第 2/2 步");
  await expect(planProgressTrigger).toContainText("1 个文件已更新");
  await expect(planProgressTrigger).toContainText("+1");
  await expect(planProgressTrigger).toContainText("-1");
  await planProgressTrigger.hover();
  const planPopover = page.getByRole("dialog", { name: "执行计划详情" });
  await expect(planPopover).toBeVisible();
  await expect(planPopover.locator(".plan-progress-step--completed").getByText("检查工作区", { exact: true })).toBeVisible();
  await expect(planPopover.locator(".plan-progress-step--completed .plan-progress-step-status")).toContainText("已完成");
  await expect(planPopover.locator(".plan-progress-step--in_progress").getByText("验证实现", { exact: true })).toBeVisible();
  await expect(planPopover.locator(".plan-progress-step--in_progress .plan-progress-step-status")).toContainText("进行中");
  await page.screenshot({ path: testInfo.outputPath("plan-progress-hover.png"), fullPage: true });
  await planProgressTrigger.click();
  await page.mouse.move(0, 0);
  await expect(planPopover).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(planPopover).toBeHidden();
  await expect(page.getByText("我先检查相关文件并修改实现。", { exact: true })).toBeVisible();
  const inspectionGroup = page.locator(".turn-tool-group").filter({ hasText: "执行了多个操作" });
  await expect(inspectionGroup).toBeVisible();
  const inspectionSummary = inspectionGroup.locator(":scope > summary");
  await expect(inspectionSummary.locator("svg.lucide-wrench, svg.lucide-square-terminal")).toHaveCount(0);
  await expect(inspectionSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(1);
  await inspectionSummary.screenshot({ path: testInfo.outputPath("operation-group-chevron.png") });
  await expect(page.locator(".turn-timeline-tool").getByText("应用补丁 src/App.css", { exact: true })).toBeHidden();
  await inspectionSummary.click();
  await expect(inspectionSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-down")).toHaveCount(1);
  await expect(inspectionSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(0);
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
  const commandSummary = commandGroup.locator(":scope > summary");
  await expect(commandSummary.locator("svg.lucide-wrench, svg.lucide-square-terminal")).toHaveCount(0);
  await expect(commandSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(1);
  await expect(page.locator(".turn-timeline-tool--command").getByText("pnpm build", { exact: true })).toBeHidden();
  await commandSummary.click();
  await expect(commandSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-down")).toHaveCount(1);
  await expect(commandSummary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(0);
  const commandRow = page.locator(".turn-timeline-tool--command").filter({ hasText: "pnpm build" }).last();
  await expect(commandRow.getByText("pnpm build", { exact: true })).toBeVisible();
  await expect(commandRow.getByText("已运行", { exact: true })).toBeVisible();
  await expect(commandRow.locator(".turn-command-inline, .turn-tool-output")).toHaveCount(0);
  await expect(page.getByText("3 个操作", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("inline-plan-and-tools.png"), fullPage: true });
  await page.evaluate(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.reload();
  await expect(page.locator(".turn-timeline-tool--command").getByText("pnpm build", { exact: true })).toBeHidden();
  await page.getByText("执行了 1.8s", { exact: true }).click();
  const restoredCommandSummary = page.locator(".turn-tool-group").filter({ hasText: "运行了命令" }).first().locator(":scope > summary");
  await expect(restoredCommandSummary.locator(".turn-tool-group-copy > svg.lucide-chevron-right")).toHaveCount(1);
  await restoredCommandSummary.click();
  await expect(restoredCommandSummary.locator(".turn-tool-group-copy > svg.lucide-chevron-down")).toHaveCount(1);
  const restoredCommandRow = page.locator(".turn-timeline-tool--command").filter({ hasText: "pnpm build" }).last();
  await expect(restoredCommandRow.getByText("pnpm build", { exact: true })).toBeVisible();
  await expect(restoredCommandRow.getByText("已运行", { exact: true })).toBeVisible();
  await expect(restoredCommandRow.locator(".turn-command-inline, .turn-tool-output")).toHaveCount(0);
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
  await page.getByRole("button", { name: "MCP", exact: true }).click();
  await expect(page.getByRole("heading", { name: "MCP 配置", exact: true })).toBeVisible();
  await expect(page.locator(".mcp-runtime-server").filter({ hasText: "local" })).toBeVisible();
  await page.getByRole("button", { name: "Rules", exact: true }).click();
  await page.getByText("运行时来源与审计", { exact: true }).click();
  await expect(page.getByText("extensions_ready", { exact: true })).toBeVisible();
});

test("creates edits and deletes custom user rules", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  await page.getByRole("button", { name: "Rules", exact: true }).click();

  const settings = page.getByRole("dialog", { name: "设置" });
  await expect(settings.getByRole("heading", { name: "自定义规则", exact: true })).toBeVisible();
  await expect(settings.getByText("方法注释", { exact: true })).toBeVisible();
  await expect(settings.locator(".rule-row")).toHaveCount(1);

  await settings.getByRole("button", { name: "新建", exact: true }).click();
  await settings.getByLabel("名称").fill("😀".repeat(80));
  await expect(settings.locator(".rule-field").filter({ hasText: "名称" }).getByText("80/80", { exact: true })).toBeVisible();
  await expect(settings.getByLabel("名称")).toHaveAttribute("aria-invalid", "false");
  await settings.getByLabel("名称").fill("审批流程");
  await settings.getByLabel("规则内容").fill("审批通过后结束流程，并记录最终状态。");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect(settings.getByText("审批流程", { exact: true })).toBeVisible();
  await expect(settings.locator(".rule-row")).toHaveCount(2);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.save_user_rule)).toEqual({
    request: {
      id: null,
      title: "审批流程",
      content: "审批通过后结束流程，并记录最终状态。",
    },
  });

  await settings.getByRole("button", { name: "编辑 审批流程" }).click();
  await settings.getByLabel("名称").fill("审批完成规则");
  await settings.getByLabel("规则内容").fill("审批通过并完成审计后结束流程。");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect(settings.getByText("审批完成规则", { exact: true })).toBeVisible();
  await expect(settings.getByText("审批流程", { exact: true })).toHaveCount(0);

  await settings.getByRole("button", { name: "删除 审批完成规则" }).click();
  const confirmation = page.getByRole("dialog", { name: "删除规则" });
  await expect(confirmation.getByText("审批完成规则", { exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(settings).toBeVisible();
  await expect(settings.getByText("审批完成规则", { exact: true })).toBeVisible();

  await settings.getByRole("button", { name: "删除 审批完成规则" }).click();
  await confirmation.getByRole("button", { name: "删除", exact: true }).click();
  await expect(settings.getByText("审批完成规则", { exact: true })).toHaveCount(0);
  await expect(settings.locator(".rule-row")).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, { id?: string }> }
  ).__invocationArgs.delete_user_rule?.id)).toBe("22222222-2222-4222-8222-222222222222");

  expect(await settings.locator(".settings-content").evaluate((element) => (
    element.scrollWidth <= element.clientWidth + 1
  ))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("custom-user-rules.png"), fullPage: true });
});

test("hides failed patch attempts after a matching retry succeeds", async ({ page }, testInfo) => {
  await page.goto("/");
  const retryPatch = "*** Begin Patch\n*** Update File: src/App.tsx\n@@\n-old\n+new\n*** End Patch";
  const unparseableRetryPatch = "*** Begin Patch\n@@\n-old\n+new\n*** End Patch";
  const ambiguousSuccessPatchA = "*** Begin Patch\n*** Update File: src/Alpha.ts\n@@\n-old\n+new\n*** End Patch";
  const ambiguousSuccessPatchB = "*** Begin Patch\n*** Update File: src/Beta.ts\n@@\n-old\n+new\n*** End Patch";
  const finalFailurePatch = "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch";
  const calls = [
    { id: "call-patch-invalid", name: "apply_patch", arguments: { patch: unparseableRetryPatch }, metadata: {} },
    { id: "call-patch-conflict", name: "apply_patch", arguments: { patch: retryPatch }, metadata: {} },
    { id: "call-patch-success", name: "apply_patch", arguments: { patch: retryPatch }, metadata: {} },
    { id: "call-patch-ambiguous-failure", name: "apply_patch", arguments: { patch: unparseableRetryPatch }, metadata: {} },
    { id: "call-patch-ambiguous-success-a", name: "apply_patch", arguments: { patch: ambiguousSuccessPatchA }, metadata: {} },
    { id: "call-patch-ambiguous-success-b", name: "apply_patch", arguments: { patch: ambiguousSuccessPatchB }, metadata: {} },
    { id: "call-patch-final-failure", name: "apply_patch", arguments: { patch: finalFailurePatch }, metadata: {} },
  ];
  const results = [
    { success: false, output: "patch syntax is invalid", metadata: { error: true } },
    { success: false, output: "patch conflicts with the workspace", metadata: { error: true } },
    { success: true, output: "applied", metadata: {} },
    { success: false, output: "ambiguous patch failure remains visible", metadata: { error: true } },
    { success: true, output: "applied alpha", metadata: {} },
    { success: true, output: "applied beta", metadata: {} },
    { success: false, output: "final patch failure", metadata: { error: true } },
  ];
  const activities = calls.map((call, index) => ({
    turnId: "turn-patch-retry",
    call,
    state: [2, 4, 5].includes(index) ? "completed" : "failed",
    result: results[index],
    startedAtMs: 1000 + index * 10,
    completedAtMs: 1006 + index * 10,
    durationMs: 6,
  }));

  await page.evaluate(({ calls: eventCalls, results: eventResults }) => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-patch-retry" };
    emit({ ...base, type: "turn_started", phase: "executing" });
    for (let index = 0; index < 3; index += 1) {
      emit({ ...base, type: "tool_started", phase: "executing", call: eventCalls[index] });
      emit({
        ...base,
        type: "tool_completed",
        phase: "executing",
        callId: eventCalls[index].id,
        name: eventCalls[index].name,
        result: eventResults[index],
      });
    }
    emit({ ...base, type: "text_delta", phase: "responding", delta: "继续处理另一个文件。" });
    for (let index = 3; index < 6; index += 1) {
      emit({ ...base, type: "tool_started", phase: "executing", call: eventCalls[index] });
      emit({
        ...base,
        type: "tool_completed",
        phase: "executing",
        callId: eventCalls[index].id,
        name: eventCalls[index].name,
        result: eventResults[index],
      });
    }
    emit({ ...base, type: "text_delta", phase: "responding", delta: "继续验证最终失败。" });
    emit({ ...base, type: "tool_started", phase: "executing", call: eventCalls[6] });
    emit({
      ...base,
      type: "tool_completed",
      phase: "executing",
      callId: eventCalls[6].id,
      name: eventCalls[6].name,
      result: eventResults[6],
    });
  }, { calls, results });

  const assertProjection = async () => {
    const message = page.locator(".message--assistant").last();
    const groups = message.locator(".turn-tool-group");
    await expect(groups).toHaveCount(3);

    const successfulGroup = groups.nth(0);
    const successfulSummary = successfulGroup.locator(":scope > summary");
    await expect(successfulSummary).toContainText("执行了操作");
    await expect(successfulSummary).not.toContainText("包含失败");
    await successfulSummary.click();
    await expect(successfulGroup.locator(".turn-timeline-tool")).toHaveCount(1);
    await expect(successfulGroup.locator(".turn-timeline-tool--completed")).toContainText("应用补丁 src/App.tsx");
    await expect(successfulGroup.locator(".turn-timeline-tool--failed")).toHaveCount(0);
    await expect(successfulGroup).not.toContainText("patch syntax is invalid");
    await expect(successfulGroup).not.toContainText("patch conflicts with the workspace");

    const ambiguousGroup = groups.nth(1);
    const ambiguousSummary = ambiguousGroup.locator(":scope > summary");
    await expect(ambiguousSummary).toContainText("包含失败");
    await ambiguousSummary.click();
    await expect(ambiguousGroup.locator(".turn-timeline-tool")).toHaveCount(3);
    await expect(ambiguousGroup.locator(".turn-timeline-tool--failed")).toContainText("ambiguous patch failure remains visible");

    const failedGroup = groups.nth(2);
    const failedSummary = failedGroup.locator(":scope > summary");
    await expect(failedSummary).toContainText("包含失败");
    await failedSummary.click();
    await expect(failedGroup.locator(".turn-timeline-tool--failed")).toContainText("final patch failure");
    return message;
  };

  const liveMessage = await assertProjection();
  await liveMessage.screenshot({ path: testInfo.outputPath(`patch-retry-live-${testInfo.project.name}.png`) });

  await page.evaluate(({ activities: storedActivities }) => {
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 2, archived: false },
      messages: [
        { schemaVersion: 1, id: "message-patch-user", role: "user", content: [{ type: "text", text: "应用补丁" }], createdAtMs: 1 },
        { schemaVersion: 1, id: "message-patch-final", role: "assistant", content: [{ type: "text", text: "补丁重试处理完成。" }], createdAtMs: 2 },
      ],
      messageTurnIds: { "message-patch-final": "turn-patch-retry" },
      turnUserMessageIds: { "turn-patch-retry": "message-patch-user" },
      lastTurn: null,
      toolActivities: storedActivities,
      turnTimeline: [
        ...storedActivities.slice(0, 3).map((activity) => ({ type: "tool", activity })),
        { type: "text", id: "progress-next-file", turnId: "turn-patch-retry", text: "继续处理另一个文件。" },
        ...storedActivities.slice(3, 6).map((activity) => ({ type: "tool", activity })),
        { type: "text", id: "progress-final-failure", turnId: "turn-patch-retry", text: "继续验证最终失败。" },
        { type: "tool", activity: storedActivities[6] },
        { type: "text", id: "message-patch-final", turnId: "turn-patch-retry", text: "补丁重试处理完成。" },
        { type: "event", itemId: "turn-completed-patch-retry", turnId: "turn-patch-retry", kind: "turn_completed", title: "Turn 已完成", detail: null, durationMs: 36 },
      ],
      approvals: [],
      userInputs: [],
      changes: [],
      todos: [],
      lastUsage: null,
      contextUsage: null,
    }));
  }, { activities });
  await page.reload();
  await page.getByText("执行了 36ms", { exact: true }).click();
  await expect(page.getByText("5 个操作", { exact: true })).toBeVisible();
  const restoredMessage = await assertProjection();
  await restoredMessage.screenshot({ path: testInfo.outputPath(`patch-retry-restored-${testInfo.project.name}.png`) });
});

test("edits global and project mcp.json directly", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "MCP", exact: true }).click();

  await expect(dialog.getByRole("heading", { name: "MCP 配置" })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "全局", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(dialog.getByText("runtime-data\\mcp.json", { exact: false })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "新增服务器", exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "可视化", exact: true })).toHaveCount(0);
  await expect(dialog.locator(".mcp-runtime-server").filter({ hasText: "local" })).toContainText("2 个工具");

  const globalEditor = dialog.getByLabel(/runtime-data\\mcp\.json JSON$/);
  await globalEditor.fill(JSON.stringify({
    mcpServers: {
      local: {
        type: "stdio",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-filesystem", "D:\\code\\k-coder"],
      },
      github: {
        enabled: true,
        timeoutMs: 45000,
        type: "streamable-http",
        url: "https://example.com/mcp",
        headers: { Accept: "application/json, text/event-stream" },
        secret_headers: { Authorization: "github-token" },
      },
    },
  }));
  await dialog.getByRole("button", { name: "格式化 JSON", exact: true }).click();
  await expect(globalEditor).toHaveValue(/"github"/);
  await dialog.getByRole("button", { name: "保存 JSON", exact: true }).click();

  await expect(page.getByText("全局 MCP 配置已保存", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => {
    const args = (window as unknown as {
      __invocationArgs: Record<string, { scope?: string; content?: string }>;
    }).__invocationArgs.save_mcp_config;
    const document = JSON.parse(args?.content ?? "{}") as {
      mcpServers?: Record<string, Record<string, unknown>>;
    };
    return { scope: args?.scope, server: document.mcpServers?.github };
  })).toEqual({
    scope: "global",
    server: {
      enabled: true,
      timeoutMs: 45000,
      type: "streamable-http",
      url: "https://example.com/mcp",
      headers: { Accept: "application/json, text/event-stream" },
      secret_headers: { Authorization: "github-token" },
    },
  });

  const workspaceBox = await dialog.locator(".mcp-json-workspace").boundingBox();
  const dialogBox = await dialog.boundingBox();
  expect(workspaceBox).not.toBeNull();
  expect(dialogBox).not.toBeNull();
  expect((workspaceBox?.x ?? 0) + (workspaceBox?.width ?? 0)).toBeLessThanOrEqual(
    (dialogBox?.x ?? 0) + (dialogBox?.width ?? 0) + 1,
  );
  await page.screenshot({ path: testInfo.outputPath(`mcp-settings-${testInfo.project.name}.png`), fullPage: true });

  await dialog.getByRole("button", { name: "当前项目", exact: true }).click();
  const projectPath = dialog.locator(".mcp-config-path");
  await expect(projectPath).toHaveAttribute("title", /\.k-coder\\mcp\.json$/);
  await expect(projectPath).toContainText("尚未创建");
  const jsonEditor = dialog.getByLabel(/\.k-coder\\mcp\.json JSON$/);
  await expect(jsonEditor).toHaveValue(/"mcpServers": \{\}/);
  await jsonEditor.fill(JSON.stringify({
    mcpServers: [{ id: "legacy", transport: "stdio", command: ["node"] }],
  }));
  await expect(dialog.locator(".mcp-json-error")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "保存 JSON", exact: true })).toBeEnabled();
  await jsonEditor.fill("{");
  await expect(dialog.locator(".mcp-json-error")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "保存 JSON", exact: true })).toBeDisabled();
});

test("keeps the MCP settings layout bounded at responsive breakpoints", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "One responsive sweep is sufficient");
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "MCP", exact: true }).click();
  await expect(dialog.getByRole("heading", { name: "MCP 配置" })).toBeVisible();

  for (const viewport of [
    { width: 375, height: 812 },
    { width: 768, height: 900 },
    { width: 1024, height: 800 },
    { width: 1440, height: 900 },
  ]) {
    await page.setViewportSize(viewport);
    const bounds = await dialog.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return {
        left: box.left,
        right: box.right,
        clientWidth: element.clientWidth,
        scrollWidth: element.scrollWidth,
        pageClientWidth: document.documentElement.clientWidth,
        pageScrollWidth: document.documentElement.scrollWidth,
      };
    });
    expect(bounds.left).toBeGreaterThanOrEqual(-1);
    expect(bounds.right).toBeLessThanOrEqual(viewport.width + 1);
    expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.clientWidth + 1);
    expect(bounds.pageScrollWidth).toBeLessThanOrEqual(bounds.pageClientWidth + 1);
    if (viewport.width === 375) {
      await page.screenshot({ path: testInfo.outputPath("mcp-settings-phone.png") });
    }
  }
});

test("renders the MCP settings tool in dark mode", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "One dark-theme visual check is sufficient");
  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.locator('button[aria-label="设置"]:visible').click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "MCP", exact: true }).click();
  await expect(dialog.getByRole("heading", { name: "MCP 配置" })).toBeVisible();
  await expect.poll(() => dialog.locator(".mcp-json-workspace").evaluate((element) => ({
    background: getComputedStyle(element).backgroundColor,
    surface: getComputedStyle(document.documentElement).getPropertyValue("--color-surface").trim(),
  }))).toEqual({ background: "rgb(27, 32, 48)", surface: "#1B2030" });
  await page.screenshot({ path: testInfo.outputPath("mcp-settings-dark.png") });
});

test("manages local plugins from backend facts", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "插件管理" }).click();

  await expect(settings.getByRole("heading", { name: "本地插件" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.plugin_overview)).toEqual({ refresh: true });
  await expect(settings.locator(".plugin-root")).toContainText(/runtime-data\\plugins/);
  for (const state of ["未启用", "已加载", "部分可用", "已阻止", "无效"]) {
    await expect(settings.getByText(state, { exact: true }).first()).toBeVisible();
  }

  const invalid = settings.locator(".plugin-row").filter({ hasText: "broken-package" });
  await expect(invalid.getByRole("checkbox", { name: "启用 broken-package" })).toBeDisabled();
  const review = settings.locator(".plugin-row").filter({ hasText: "review-tools" });
  await review.getByRole("checkbox", { name: "启用 review-tools" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.set_plugin_enabled)).toEqual({
    pluginId: "review-tools@local",
    enabled: true,
  });
  await expect(review.getByRole("checkbox", { name: "启用 review-tools" })).toBeChecked();
  await expect(review.getByText("已加载", { exact: true })).toBeVisible();

  await review.getByRole("button", { name: "删除 review-tools" }).click();
  const confirm = page.getByRole("dialog", { name: "删除插件" });
  await expect(confirm.getByText("review-tools", { exact: true })).toBeVisible();
  await confirm.getByRole("button", { name: "取消", exact: true }).click();
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "delete_plugin").length)).toBe(0);

  await review.getByRole("button", { name: "删除 review-tools" }).click();
  await page.getByRole("dialog", { name: "删除插件" }).getByRole("button", { name: "删除", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.delete_plugin)).toEqual({ pluginId: "review-tools@local" });
  await expect(settings.locator(".plugin-row").filter({ hasText: "review-tools" })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath(`plugin-settings-${testInfo.project.name}.png`), fullPage: true });
});

test("restores local plugin facts after a rejected toggle", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_plugin_toggle_error", "review-tools@local");
  });
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "插件管理" }).click();
  const review = settings.locator(".plugin-row").filter({ hasText: "review-tools" });

  await review.getByRole("checkbox", { name: "启用 review-tools" }).click();

  await expect(settings.getByRole("alert")).toContainText("plugin runtime refresh failed");
  await expect(review.getByRole("checkbox", { name: "启用 review-tools" })).not.toBeChecked();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "plugin_overview").length)).toBeGreaterThanOrEqual(2);
});

test("keeps local plugin settings bounded and renders the empty state", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "One responsive sweep is sufficient");
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "插件管理" }).click();
  await expect(settings.getByRole("heading", { name: "本地插件" })).toBeVisible();

  for (const viewport of [
    { width: 375, height: 812 },
    { width: 768, height: 900 },
    { width: 1024, height: 800 },
    { width: 1440, height: 900 },
  ]) {
    await page.setViewportSize(viewport);
    const bounds = await settings.evaluate((element) => ({
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
      pageClientWidth: document.documentElement.clientWidth,
      pageScrollWidth: document.documentElement.scrollWidth,
    }));
    expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.clientWidth + 1);
    expect(bounds.pageScrollWidth).toBeLessThanOrEqual(bounds.pageClientWidth + 1);
  }

  await page.evaluate(() => localStorage.setItem("kcoder_e2e_plugin_empty", "true"));
  await settings.getByRole("button", { name: "刷新插件" }).click();
  await expect(settings.getByText("未发现插件", { exact: true })).toBeVisible();
});

test("renders local plugin settings in dark mode", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "One dark-theme visual check is sufficient");
  await page.addInitScript(() => localStorage.setItem("kcoder_theme", "dark"));
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "插件管理" }).click();

  await expect(settings.getByRole("heading", { name: "本地插件" })).toBeVisible();
  await expect.poll(() => settings.locator(".plugin-row-icon").first().evaluate((element) => ({
    background: getComputedStyle(element).backgroundColor,
    panel: getComputedStyle(document.documentElement).getPropertyValue("--color-surface-panel").trim(),
  }))).toEqual({ background: "rgb(22, 26, 38)", panel: "#161A26" });
  await page.screenshot({ path: testInfo.outputPath("plugin-settings-dark.png") });
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

test("selects and persists the extended appearance themes", async ({ page }) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();

  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "外观" }).click();
  const picker = settings.getByRole("radiogroup", { name: "选择主题" });
  await expect(picker.getByRole("radio", { name: "Synthwave：霓虹夜色" })).toBeVisible();

  const themes = [
    ["arctic", "Arctic：冰川蓝白", "#1687a7"],
    ["crt-green", "CRT Green：荧光绿屏", "#78f6a5"],
    ["ember", "Ember：炭火橙红", "#f97316"],
    ["miami", "Miami：海盐珊瑚", "#e66064"],
    ["synthwave", "Synthwave：霓虹夜色", "#f472b6"],
    ["terminal", "Terminal：琥珀终端", "#f5b94c"],
    ["vapor", "Vapor：雾紫柔光", "#8b73d6"],
  ] as const;
  for (const [id, name, brand] of themes) {
    await picker.getByRole("radio", { name }).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", id);
    await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--color-brand").trim())).toBe(brand);
  }

  await picker.getByRole("radio", { name: "Synthwave：霓虹夜色" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "synthwave");
  await expect(page.locator("html")).toHaveAttribute("data-theme-preference", "synthwave");

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "synthwave");
  await expect(page.locator("html")).toHaveAttribute("data-theme-preference", "synthwave");
});

test("resolves the system appearance and follows system changes", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.addInitScript(() => localStorage.setItem("kcoder_theme", "system"));
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme-preference", "system");

  await page.emulateMedia({ colorScheme: "light" });
  await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe("light");
});

test("keeps the K brand and renders a distinct command-pulse welcome mark", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const emptyThread = {
      schemaVersion: 1,
      id: "thread-1",
      title: "新会话",
      createdAtMs: 1,
      updatedAtMs: 1,
      archived: false,
    };
    localStorage.setItem("kcoder_e2e_threads", JSON.stringify([emptyThread]));
    localStorage.setItem("kcoder_e2e_empty_thread_id", emptyThread.id);
    localStorage.setItem("kcoder_e2e_read_delay_ms", "900");
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: emptyThread,
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

  const welcome = page.locator(".empty-thread--welcome");
  await expect(page.getByText("正在读取会话", { exact: true })).toBeVisible();
  await expect(welcome).toHaveCount(0);
  await expect(welcome).toBeVisible();
  await expect(page.getByRole("heading", { name: "新会话" })).toBeVisible();
  await expect(welcome.getByRole("heading", { name: "从一个明确的任务开始" })).toBeVisible();
  await expect(welcome.locator("svg[data-welcome-mark='command-pulse']")).toHaveCount(1);
  await expect(welcome.locator("svg[data-brand-mark='k-letter']")).toHaveCount(0);
  await expect(page.locator(".brand-mark svg[data-brand-mark='k-letter']")).toHaveCount(1);
  await expect(page.locator("svg[data-brand-mark='command-pulse']")).toHaveCount(0);
  await expect(page.locator(".message-list")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`command-pulse-welcome-k-brand-${testInfo.project.name}.png`), fullPage: true });
});

test("uses the CodeBuddy blue K and rebuilds the native Windows icon resource", () => {
  const iconSource = readFileSync(new URL("../assets/app-icon.svg", import.meta.url), "utf8");
  const buildScript = readFileSync(new URL("../src-tauri/build.rs", import.meta.url), "utf8");
  expect(iconSource).toContain('fill="#2F6FE4"');
  expect(iconSource).toMatch(/<path\s+d="M154 112h72v119l104-119h88L298 247l128 153h-91L250 294l-24 27v79h-72V112Z"\s+fill="#fff"\s*\/>/);
  expect(buildScript).toContain('println!("cargo:rerun-if-changed=icons/icon.ico");');
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

test("selects the active workspace project from the composer", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const currentPath = "\\\\?\\D:\\code\\k-coder";
    // A stale presentation-only project group must not override the persisted thread workspace.
    localStorage.setItem("kcoder_thread_project_map", JSON.stringify({ "thread-1": "D:\\code\\paypro-platform-be" }));
    localStorage.setItem("kcoder_known_projects", JSON.stringify([
      "\\\\?\\D:\\code\\paypro-platform-be",
      "\\\\?\\UNC\\server\\share\\repo",
    ]));
    localStorage.setItem("kcoder_e2e_workspace_path", currentPath);
    localStorage.setItem("kcoder_e2e_thread_workspace_path", currentPath);
    localStorage.setItem("kcoder_e2e_recent_projects", JSON.stringify([
      { id: "project-1", name: "k-coder", path: currentPath, trusted: true, lastOpenedAtMs: 3 },
      { id: "project-2", name: "paypro-platform-be", path: "\\\\?\\D:\\code\\paypro-platform-be", trusted: true, lastOpenedAtMs: 2 },
      { id: "project-3", name: "shared-repo", path: "\\\\?\\UNC\\server\\share\\repo", trusted: true, lastOpenedAtMs: 1 },
    ]));
  });
  await page.goto("/");
  const composer = page.locator(".composer");
  const initialTrigger = composer.getByRole("button", { name: "当前项目 k-coder" });
  await expect(initialTrigger).toBeVisible();
  await expect(initialTrigger).toHaveAttribute("title", "D:\\code\\k-coder");
  await expect(page.locator(".workspace-current")).toHaveAttribute("title", "D:\\code\\k-coder");

  await initialTrigger.click();
  const dialog = page.getByRole("dialog", { name: "选择项目" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("option")).toHaveCount(3);
  await expect(dialog.getByText("D:\\code\\k-coder", { exact: true })).toBeVisible();
  await expect(dialog.getByText("\\\\server\\share\\repo", { exact: true })).toBeVisible();
  expect(await dialog.textContent()).not.toContain("\\\\?\\");
  const search = dialog.getByRole("textbox", { name: "搜索项目" });
  await expect(search).toBeFocused();
  const [dialogBox, composerBox, searchBox] = await Promise.all([
    dialog.boundingBox(),
    composer.boundingBox(),
    search.boundingBox(),
  ]);
  const viewport = page.viewportSize();
  expect(dialogBox).not.toBeNull();
  expect(composerBox).not.toBeNull();
  expect(searchBox).not.toBeNull();
  expect(dialogBox?.x ?? 0).toBeGreaterThanOrEqual(0);
  expect((dialogBox?.x ?? 0) + (dialogBox?.width ?? 0)).toBeLessThanOrEqual(viewport?.width ?? 0);
  expect(dialogBox?.y ?? 0).toBeGreaterThanOrEqual(0);
  expect((dialogBox?.y ?? 0) + (dialogBox?.height ?? 0)).toBeLessThanOrEqual((composerBox?.y ?? 0) - 7);
  expect(searchBox?.y ?? 0).toBeGreaterThanOrEqual(dialogBox?.y ?? 0);
  expect((searchBox?.y ?? 0) + (searchBox?.height ?? 0)).toBeLessThanOrEqual(
    (dialogBox?.y ?? 0) + (dialogBox?.height ?? 0),
  );
  expect(searchBox?.height ?? 0).toBeLessThanOrEqual(32);
  await expect(search).toHaveCSS("box-shadow", "none");
  await expect(page.locator(".composer:focus-within")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("composer-project-menu.png"), fullPage: true });
  await search.fill("\\\\server\\share");
  await expect(dialog.getByRole("option", { name: /shared-repo/ })).toBeVisible();
  await search.fill("paypro");
  const projectOption = dialog.getByRole("option", { name: /paypro-platform-be/ });
  await expect(projectOption).toBeVisible();
  await projectOption.click();

  await expect(composer.getByRole("button", { name: "当前项目 paypro-platform-be" })).toBeVisible();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "switch_workspace").length,
  )).toBeGreaterThanOrEqual(1);
  expect(await page.evaluate(() =>
    (window as unknown as { __invocationArgs: Record<string, { path?: string }> }).__invocationArgs.switch_workspace?.path,
  )).toBe("\\\\?\\D:\\code\\paypro-platform-be");
  const calls = await page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked);
  expect(calls.lastIndexOf("create_thread")).toBeGreaterThan(calls.lastIndexOf("switch_workspace"));
  await expect.poll(() => composer.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
  await page.screenshot({ path: testInfo.outputPath("composer-project-selector.png"), fullPage: true });
});

test("reports system opener failures inside the file preview", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => localStorage.setItem("kcoder_e2e_external_open_error", "系统无法打开该文件"));
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  await page.getByRole("button", { name: /README\.md/ }).click();

  const preview = page.getByRole("dialog", { name: "预览 README.md" });
  await preview.getByRole("button", { name: "使用系统编辑器打开" }).click();
  await expect(preview.getByRole("alert")).toContainText("系统无法打开该文件");

  await page.evaluate(() => localStorage.removeItem("kcoder_e2e_external_open_error"));
  await preview.getByRole("button", { name: "在资源管理器中定位" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "reveal_workspace_file").length)).toBe(1);
});

test("creates a standalone conversation from the lower-left project menu", async ({ page }, testInfo) => {
  await page.goto("/");
  const composer = page.locator(".composer");
  await composer.getByRole("button", { name: "当前项目 k-coder" }).click();

  const dialog = page.getByRole("dialog", { name: "选择项目" });
  const standalone = dialog.getByRole("button", { name: "不在项目中", exact: true });
  await expect(standalone).toBeVisible();
  await expect(standalone).toHaveAttribute("aria-pressed", "false");
  const [dialogBox, standaloneBox] = await Promise.all([dialog.boundingBox(), standalone.boundingBox()]);
  expect(dialogBox).not.toBeNull();
  expect(standaloneBox).not.toBeNull();
  expect((standaloneBox?.x ?? 0) - (dialogBox?.x ?? 0)).toBeLessThanOrEqual(12);
  expect(
    (dialogBox?.y ?? 0) + (dialogBox?.height ?? 0)
      - ((standaloneBox?.y ?? 0) + (standaloneBox?.height ?? 0)),
  ).toBeLessThanOrEqual(12);
  await dialog.screenshot({ path: testInfo.outputPath("project-menu-standalone-option.png") });

  await page.evaluate(() => {
    (window as unknown as { __invoked: string[] }).__invoked.length = 0;
  });
  await standalone.click();

  await expect(composer.getByRole("button", { name: "当前不在项目中" })).toContainText("不在项目中");
  await expect(composer.getByRole("button", { name: "选择机器人" })).toBeDisabled();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.create_thread)).toEqual({ inProject: false });
  const calls = await page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked);
  expect(calls).toContain("create_thread");
  expect(calls).not.toContain("switch_workspace");

  await composer.getByRole("button", { name: "当前不在项目中" }).click();
  await expect(page.getByRole("dialog", { name: "选择项目" }).getByRole("button", { name: "不在项目中", exact: true }))
    .toHaveAttribute("aria-pressed", "true");
  await page.screenshot({ path: testInfo.outputPath("standalone-conversation-selected.png"), fullPage: true });

  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    (window as unknown as { __invoked: string[] }).__invoked.length = 0;
  });
  const message = composer.getByRole("textbox", { name: "消息" });
  await message.fill("@src/");
  await expect(page.locator(".composer-suggestions")).toHaveCount(0);
  await message.fill("/review");
  await expect(page.locator(".composer-suggestions")).toHaveCount(0);
  const standaloneCalls = await page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked);
  expect(standaloneCalls).not.toContain("search_workspace_files");
  expect(standaloneCalls).not.toContain("get_extension_overview");
});

test("keeps a migrated unbound project thread in the current project", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem("kcoder_e2e_thread_override", JSON.stringify({
      title: "Legacy project conversation",
      inProject: true,
      workspacePath: null,
    }));
    localStorage.removeItem("kcoder_thread_project_map");
  });
  await page.reload();

  await expect(page.locator(".composer").getByRole("button", { name: "当前项目 k-coder" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => {
    const raw = localStorage.getItem("kcoder_thread_project_map");
    return raw ? (JSON.parse(raw) as Record<string, string>)["thread-1"] : null;
  })).toBe("D:\\code\\k-coder");
});

test("keeps project selection in sync and removes groups without deleting conversations", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const currentPath = "D:\\code\\k-coder";
    const codexPath = "D:\\code\\codex";
    localStorage.setItem("kcoder_thread_project_map", JSON.stringify({ "thread-1": currentPath }));
    localStorage.setItem("kcoder_known_projects", JSON.stringify([currentPath, codexPath]));
    localStorage.setItem("kcoder_e2e_workspace_path", currentPath);
    localStorage.setItem("kcoder_e2e_recent_projects", JSON.stringify([
      { id: "project-1", name: "k-coder", path: currentPath, trusted: true, lastOpenedAtMs: 4 },
      { id: "project-2", name: "codex", path: codexPath, trusted: true, lastOpenedAtMs: 3 },
      { id: "project-3", name: "Nick", path: "D:\\code\\Nick", trusted: true, lastOpenedAtMs: 2 },
      { id: "project-4", name: "src-tauri", path: "D:\\code\\Nick\\k-coder\\src-tauri", trusted: true, lastOpenedAtMs: 1 },
    ]));
  });
  await page.goto("/");

  const composer = page.locator(".composer");
  const projectTrigger = composer.getByRole("button", { name: "当前项目 k-coder" });
  await expect(projectTrigger).toBeVisible();
  await projectTrigger.click();
  const dialog = page.getByRole("dialog", { name: "选择项目" });
  await expect(dialog.getByRole("option")).toHaveCount(2);
  await expect(dialog.getByRole("option", { name: /k-coder/ })).toBeVisible();
  const codexOption = dialog.getByRole("option", { name: /codex/ });
  await expect(codexOption).toBeVisible();
  await expect(dialog.getByText("Nick", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("src-tauri", { exact: true })).toHaveCount(0);

  if (testInfo.project.name !== "narrow") {
    await page.keyboard.press("Escape");
    await page.getByRole("tab", { name: "项目" }).click();
    const projectList = page.getByRole("navigation", { name: "项目列表" });
    await expect(projectList.locator(".project-group")).toHaveCount(2);
    const kCoderGroup = projectList.locator(".project-group").filter({ hasText: "k-coder" });
    await expect(kCoderGroup.locator(".project-group-toggle")).toHaveAttribute("aria-current", "page");
    await kCoderGroup.getByRole("button", { name: "更多操作" }).click();
    const removeBoundProject = kCoderGroup.getByRole("button", { name: "删除分组", exact: true });
    await expect(removeBoundProject).toBeEnabled();
    await expect(removeBoundProject).toHaveAttribute("title", "删除项目分组");
    await kCoderGroup.getByRole("button", { name: "更多操作" }).click();
    await projectTrigger.click();
  }

  await page.getByRole("dialog", { name: "选择项目" }).getByRole("option", { name: /codex/ }).click();
  await expect(composer.getByRole("button", { name: "当前项目 codex" })).toBeVisible();

  if (testInfo.project.name !== "narrow") {
    const projectList = page.getByRole("navigation", { name: "项目列表" });
    const codexGroup = projectList.locator(".project-group").filter({ hasText: "codex" });
    await expect(codexGroup.locator(".project-group-toggle")).toHaveAttribute("aria-current", "page");
    const kCoderGroup = projectList.locator(".project-group").filter({ hasText: "k-coder" });
    await kCoderGroup.getByRole("button", { name: "展开项目" }).click();
    await kCoderGroup.getByText("Phase 6 workbench", { exact: true }).click();
    await expect(composer.getByRole("button", { name: "当前项目 k-coder" })).toBeVisible();
    await expect(kCoderGroup.locator(".project-group-toggle")).toHaveAttribute("aria-current", "page");

    let deletePrompt = "";
    page.once("dialog", async (dialog) => {
      deletePrompt = dialog.message();
      expect(dialog.type()).toBe("confirm");
      await dialog.accept();
    });
    await kCoderGroup.getByRole("button", { name: "更多操作" }).click();
    await kCoderGroup.getByRole("button", { name: "删除分组", exact: true }).click();

    expect(deletePrompt).toContain("1 个会话不会被删除");
    expect(deletePrompt).toContain("项目文件不会被删除");
    await expect(kCoderGroup).toHaveCount(0);
    await expect(projectList.locator(".project-group")).toHaveCount(1);
    await page.getByRole("tab", { name: "会话" }).click();
    await expect(page.getByRole("navigation", { name: "会话列表" }).getByText("Phase 6 workbench", { exact: true })).toBeVisible();
    await expect(composer.getByRole("button", { name: "当前项目 k-coder" })).toBeVisible();
    await expect.poll(() => page.evaluate(() => JSON.parse(
      localStorage.getItem("kcoder_hidden_project_groups") ?? "[]",
    ))).toEqual(["D:\\code\\k-coder"]);
    await expect.poll(() => page.evaluate(() => JSON.parse(
      localStorage.getItem("kcoder_known_projects") ?? "[]",
    ))).toEqual(["D:\\code\\codex"]);
    await expect.poll(() => page.evaluate(() => JSON.parse(
      localStorage.getItem("kcoder_thread_project_map") ?? "{}",
    )["thread-1"])).toBe("D:\\code\\k-coder");
    expect(await page.evaluate(() => (
      window as unknown as { __invoked: string[] }
    ).__invoked.filter((command) => command === "delete_thread").length)).toBe(0);

    await page.reload();
    await expect(page.getByRole("navigation", { name: "会话列表" }).getByText("Phase 6 workbench", { exact: true })).toBeVisible();
    await page.getByRole("tab", { name: "项目" }).click();
    const restoredProjectList = page.getByRole("navigation", { name: "项目列表" });
    await expect(restoredProjectList.locator(".project-group")).toHaveCount(1);
    await expect(restoredProjectList.getByText("k-coder", { exact: true })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath("project-group-deleted.png"), fullPage: true });

    await composer.getByRole("button", { name: "当前项目 k-coder" }).click();
    const activeProjectOption = page.getByRole("dialog", { name: "选择项目" })
      .getByRole("option", { name: /k-coder/ });
    await expect(activeProjectOption).toHaveAttribute("aria-selected", "true");
    await activeProjectOption.click();
    await expect(restoredProjectList.locator(".project-group")).toHaveCount(2);
    await expect(restoredProjectList.getByText("k-coder", { exact: true })).toBeVisible();
    await expect.poll(() => page.evaluate(() => JSON.parse(
      localStorage.getItem("kcoder_hidden_project_groups") ?? "[]",
    ))).toEqual([]);
  }

  await page.screenshot({ path: testInfo.outputPath("linked-project-selection.png"), fullPage: true });
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

test("streams thinking, safe reasoning summaries, compact command states, and file diffs inline", async ({ page, context }, testInfo) => {
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
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live", delta: "**Fixing context mismatch in patch**" });
  });
  await expect(page.getByText("思考中", { exact: true })).toBeVisible();
  await expect(page.getByText("Fixing context mismatch in patch", { exact: true })).toHaveCount(0);
  await expect(page.locator(".turn-reasoning")).toHaveCount(0);
  await expect(page.getByText("等待工具调用…", { exact: true })).toHaveCount(0);
  await expect(page.locator(".turn-timeline--empty")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("compact-thinking-status.png"), fullPage: true });
  await expect(page.locator(".message-avatar")).toHaveCount(0);
  await expect(page.getByText("正在执行", { exact: true })).toHaveCount(0);

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-live" };
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live", summary: "**Fixing context mismatch in patch**" });
    emit({ ...base, type: "item_completed", phase: "planning", itemId: "rs-live", itemType: "reasoning", status: "completed" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "rs-live-mixed", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live-mixed", delta: "The user asked why 思考摘要 is visible, so I should explain the provider response." });
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live-mixed", summary: "The user asked why 思考摘要 is visible, so I should explain the provider response." });
    emit({ ...base, type: "item_completed", phase: "planning", itemId: "rs-live-mixed", itemType: "reasoning", status: "completed" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "rs-live-2", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live-2", delta: "已确认工具输出按游标去重，下一步核对脱敏边界。" });
    emit({ ...base, type: "reasoning_summary_completed", phase: "planning", itemId: "rs-live-2", summary: "已确认工具输出按游标去重，下一步核对脱敏边界。" });
    emit({ ...base, type: "item_completed", phase: "planning", itemId: "rs-live-2", itemType: "reasoning", status: "completed" });
    emit({ ...base, type: "item_started", phase: "planning", itemId: "rs-live-3", itemType: "reasoning" });
    emit({ ...base, type: "reasoning_summary_delta", phase: "planning", itemId: "rs-live-3", delta: "已完成扁平布局，正在核对窄屏边界。" });
    emit({ ...base, type: "item_started", phase: "executing", itemId: "call-live", itemType: "tool" });
    emit({ ...base, type: "tool_started", phase: "executing", call: { id: "call-live", name: "run_command", arguments: { command: "pnpm build" }, metadata: {} } });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stdout", cursor: 0, delta: "building client\n" });
    emit({ ...base, type: "tool_output_delta", phase: "executing", callId: "call-live", stream: "stderr", cursor: 1, delta: "warning: fixture\n" });
    emit({ ...base, type: "tool_completed", phase: "executing", callId: "call-live", name: "run_command", result: { success: true, output: "building client\n", metadata: { durationMs: 1234, shell: "powershell" } } });
    emit({ ...base, type: "item_completed", phase: "executing", itemId: "call-live", itemType: "tool", status: "completed" });
    emit({ ...base, type: "item_started", phase: "executing", itemId: "compaction-live", itemType: "context_compaction" });
    emit({ ...base, type: "context_compacted", phase: "executing", itemId: "compaction-live", automatic: true,
      compactedMessageCount: 18, userConstraintCount: 1, recentUserMessageCount: 2, recentToolResultCount: 1 });
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
  await expect(reasoning.locator(":scope > .turn-reasoning-heading > svg.lucide-lightbulb")).toHaveCount(1);
  await expect(page.getByText("思考内容", { exact: true })).toHaveCount(0);
  await expect(reasoning.locator(".turn-reasoning-segment")).toHaveCount(2);
  await expect(reasoning.locator(":scope > summary, .turn-disclosure-status, .turn-disclosure-chevron")).toHaveCount(0);
  await expect(reasoning).toHaveCSS("border-top-width", "0px");
  await expect(reasoning).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(page.getByText("Fixing context mismatch in patch", { exact: true })).toHaveCount(0);
  await expect(page.getByText("The user asked why 思考摘要 is visible, so I should explain the provider response.", { exact: true })).toHaveCount(0);
  await expect(page.getByText("已确认工具输出按游标去重，下一步核对脱敏边界。", { exact: true })).toBeVisible();
  await expect(page.getByText("已完成扁平布局，正在核对窄屏边界。", { exact: true })).toBeVisible();
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
  const liveCommandRow = liveCommandGroup.locator(".turn-timeline-tool--command").filter({ hasText: "pnpm build" });
  await expect(liveCommandRow.getByText("pnpm build", { exact: true })).toBeVisible();
  await expect(liveCommandRow.getByText("已运行", { exact: true })).toBeVisible();
  await expect(liveCommandRow.locator(".turn-command-inline, .turn-tool-output, .turn-tool-duration")).toHaveCount(0);
  const compactionStep = liveExecution.locator(".turn-event-step--compacted");
  await expect(compactionStep.getByText("已自动压缩上下文", { exact: true })).toBeVisible();
  await expect(compactionStep).not.toHaveAttribute("open", "");
  await compactionStep.locator(":scope > summary").click();
  await expect(compactionStep.getByText("压缩了 18 条历史消息，保留 1 项用户约束、2 项近期用户请求和 1 项近期工具结果", { exact: true })).toBeVisible();
  await liveCommandRow.screenshot({ path: testInfo.outputPath("command-summary.png") });
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
  await liveCommandGroup.scrollIntoViewIfNeeded();
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

test("uses an unframed disclosure and scrolls long multi-command groups", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (value: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "turn-command-scroll" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    for (let index = 0; index < 8; index += 1) {
      const callId = `call-command-scroll-${index}`;
      const command = `pnpm exec playwright test e2e/workbench.spec.ts --project command-scroll-${index} --grep a-very-long-command-name-that-wraps-in-narrow-layouts`;
      emit({
        ...base,
        type: "tool_started",
        phase: "executing",
        call: { id: callId, name: "run_command", arguments: { command }, metadata: {} },
      });
      emit({
        ...base,
        type: "tool_completed",
        phase: "executing",
        callId,
        name: "run_command",
        result: { success: true, output: "done", metadata: { durationMs: 120 } },
      });
    }
  });

  const group = page.locator(".turn-tool-group--multiple-commands").last();
  const summary = group.locator(":scope > summary");
  await expect(summary.getByText("运行了多个命令", { exact: true })).toBeVisible();
  await expect(summary.locator("svg.lucide-wrench, svg.lucide-square-terminal")).toHaveCount(0);
  await expect(summary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(1);
  const summaryAlignment = await summary.evaluate((element) => {
    const title = element.querySelector<HTMLElement>(".turn-disclosure-title")?.getBoundingClientRect();
    const arrow = element.querySelector<SVGElement>(".turn-tool-group-copy > .turn-tool-group-chevron")?.getBoundingClientRect();
    if (!title || !arrow) return null;
    return {
      gap: arrow.left - title.right,
      centerDelta: Math.abs((arrow.top + arrow.height / 2) - (title.top + title.height / 2)),
    };
  });
  expect(summaryAlignment).not.toBeNull();
  expect(summaryAlignment!.gap).toBeGreaterThanOrEqual(0);
  expect(summaryAlignment!.gap).toBeLessThanOrEqual(16);
  expect(summaryAlignment!.centerDelta).toBeLessThanOrEqual(4);
  await expect(summary).toHaveCSS("border-top-width", "0px");
  await expect(summary).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await group.screenshot({ path: testInfo.outputPath("unframed-collapsed-command-group.png") });
  await summary.click();
  await expect(summary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-down")).toHaveCount(1);
  await expect(summary.locator(".turn-tool-group-copy > svg.turn-tool-group-chevron.lucide-chevron-right")).toHaveCount(0);

  const content = group.locator(".turn-tool-group-content");
  await expect(content.locator(".turn-timeline-tool--command")).toHaveCount(8);
  const scrollState = await content.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    overflowY: getComputedStyle(element).overflowY,
  }));
  expect(scrollState.clientHeight).toBeLessThanOrEqual(168);
  expect(scrollState.scrollHeight).toBeGreaterThan(scrollState.clientHeight);
  expect(scrollState.overflowY).toBe("auto");
  await content.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect.poll(() => content.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await group.screenshot({ path: testInfo.outputPath("unframed-scrollable-command-group.png") });
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

test("primary send queues behind the active turn and the queued send steers it", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    emit({
      schemaVersion: 1,
      threadId: "thread-1",
      turnId: "turn-primary-active",
      type: "turn_started",
      phase: "exploring",
      userMessage: {
        schemaVersion: 1,
        id: "primary-active-user",
        role: "user",
        content: [{ type: "text", text: "请检查当前发送逻辑" }],
        createdAtMs: Date.now(),
      },
    });
    emit({
      schemaVersion: 4,
      threadId: "thread-1",
      turnId: "turn-primary-active",
      type: "text_delta",
      phase: "responding",
      itemId: "assistant-before-steer",
      delta: "这是引导前的助手回复。",
    });
  });
  await expect(page.getByText("这是引导前的助手回复。", { exact: true })).toBeVisible();

  const composer = page.getByRole("textbox", { name: "消息" });
  await expect(composer).toBeEnabled();
  await composer.fill("改为先修复发送逻辑，再继续验证");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();

  await expect(page.getByRole("button", { name: "停止生成" })).toBeEnabled();
  await expect(page.locator(".mode-label")).toHaveText("正在生成");
  await expect(page.getByRole("button", { name: "发送消息", exact: true })).toHaveAttribute("title", "加入消息队列");
  await expect(page.locator(".message-queue")).toContainText("队列 (1)");
  await expect(page.locator(".queue-list")).toContainText("改为先修复发送逻辑，再继续验证");
  const steerQueuedButton = page.getByRole("button", { name: "发送到当前对话 改为先修复发送逻辑，再继续验证" });
  await expect(steerQueuedButton).toBeEnabled();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<Record<string, unknown>> }
  ).__runTurnCalls[0])).toMatchObject({
    request: {
      threadId: "thread-1",
      input: "改为先修复发送逻辑，再继续验证",
    },
  });
  await expect.poll(() => page.evaluate(() => Object.prototype.hasOwnProperty.call(
    (window as unknown as { __runTurnCalls: Array<Record<string, unknown>> }).__runTurnCalls[0] ?? {},
    "interruptActiveTurnId",
  ))).toBe(false);
  await expect.poll(() => page.evaluate(() => {
    const invoked = (window as unknown as { __invoked: string[] }).__invoked;
    return {
      interrupt: invoked.filter((command) => command === "turn_interrupt").length,
      steer: invoked.filter((command) => command === "turn_steer").length,
      steerQueued: invoked.filter((command) => command === "turn_steer_queued").length,
    };
  })).toEqual({ interrupt: 0, steer: 0, steerQueued: 0 });
  await page.screenshot({ path: testInfo.outputPath("primary-send-queued-current-turn.png"), fullPage: true });

  await steerQueuedButton.click();
  await expect.poll(() => page.evaluate(() => {
    const invoked = (window as unknown as { __invoked: string[] }).__invoked;
    return {
      interrupt: invoked.filter((command) => command === "turn_interrupt").length,
      steer: invoked.filter((command) => command === "turn_steer").length,
      steerQueued: invoked.filter((command) => command === "turn_steer_queued").length,
    };
  })).toEqual({ interrupt: 0, steer: 0, steerQueued: 1 });
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.turn_steer_queued)).toEqual({
    request: {
      threadId: "thread-1",
      expectedTurnId: "turn-primary-active",
      queuedTurnId: "turn-start-1",
    },
  });
  await expect(page.locator(".message-queue")).toHaveCount(0);

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    emit({
      schemaVersion: 4,
      threadId: "thread-1",
      turnId: "turn-primary-active",
      type: "turn_steered",
      phase: "exploring",
      message: {
        schemaVersion: 1,
        id: "steer-turn-primary-active",
        role: "user",
        content: [{ type: "text", text: "改为先修复发送逻辑，再继续验证" }],
        createdAtMs: Date.now(),
      },
    });
    emit({
      schemaVersion: 4,
      threadId: "thread-1",
      turnId: "turn-primary-active",
      type: "text_delta",
      phase: "responding",
      itemId: "assistant-after-steer",
      delta: "这是根据引导产生的新回复。",
    });
  });
  await expect(page.locator(".message--user").getByText("改为先修复发送逻辑，再继续验证", { exact: true })).toBeVisible();
  await expect(page.getByText("这是引导前的助手回复。", { exact: true })).toBeVisible();
  await expect(page.getByText("这是根据引导产生的新回复。", { exact: true })).toBeVisible();
  await expect(page.locator('article.message--assistant[data-turn-id="turn-primary-active"]')).toHaveCount(2);
  const preSteerAssistant = page
    .locator('article.message--assistant[data-turn-id="turn-primary-active"]')
    .filter({ hasText: "这是引导前的助手回复。" });
  await expect(preSteerAssistant.locator(".turn-final-response")).toContainText("这是引导前的助手回复。");
  const visibleOrder = await page.locator(".message-list").innerText();
  expect(visibleOrder.indexOf("这是引导前的助手回复。")).toBeLessThan(
    visibleOrder.indexOf("改为先修复发送逻辑，再继续验证"),
  );
  expect(visibleOrder.indexOf("改为先修复发送逻辑，再继续验证")).toBeLessThan(
    visibleOrder.indexOf("这是根据引导产生的新回复。"),
  );
  await page.screenshot({ path: testInfo.outputPath("primary-send-steered-current-turn.png"), fullPage: true });
  await expect(page.locator(".mode-label")).toHaveText("正在生成");

  await page.getByRole("button", { name: "停止生成" }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: unknown[] }
  ).__runTurnCalls.length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs.turn_interrupt)).toEqual({
    threadId: "thread-1",
    turnId: "turn-primary-active",
  });

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 4,
      threadId: "thread-1",
      turnId: "turn-primary-active",
      type: "turn_cancelled",
      phase: "cancelled",
    });
  });
  await expect(page.locator(".message-queue")).toHaveCount(0);
  await expect(page.locator(".message--user").getByText("改为先修复发送逻辑，再继续验证", { exact: true })).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: unknown[] }
  ).__runTurnCalls.length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(1);
});

test("atomically steers and removes a queued message from the backend mailbox", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_mailbox", JSON.stringify({
      threadId: "thread-1",
      activeTurnId: "turn-live-queue",
      pending: [
        {
          schemaVersion: 1,
          turnId: "queued-first",
          threadId: "thread-1",
          kind: "message",
          input: "queued first",
          agentMode: "craft",
          workflowId: null,
          attachments: [],
        },
        {
          schemaVersion: 1,
          turnId: "queued-second",
          threadId: "thread-1",
          kind: "message",
          input: "queued second",
          agentMode: "craft",
          workflowId: null,
          attachments: [],
        },
      ],
    }));
  });
  await page.goto("/");

  await expect(page.locator(".message-queue")).toContainText("队列 (2)");
  await page.locator(".queue-toggle").click();
  await expect(page.locator(".queue-list")).toContainText("queued first");
  await expect(page.locator(".queue-list")).toContainText("queued second");
  await expect(page.locator(".message--user").getByText("queued first", { exact: true })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("queued-message-actions.png"), fullPage: true });

  await page.getByRole("button", { name: "发送到当前对话 queued first", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_steer_queued").length)).toBe(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_steer").length)).toBe(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "remove_queued_turn").length)).toBe(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.filter((command) => command === "turn_interrupt").length)).toBe(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __runTurnCalls: unknown[] }).__runTurnCalls.length)).toBe(0);
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

test("restores an active conversation without replaying existing text or restarting its turn", async ({ page }) => {
  const progressText = "已确认会话切换只影响展示状态，当前任务仍在后台继续执行。".repeat(24);
  const laterDelta = "切回后收到的新进度仍然按照实时节奏继续展示。".repeat(12);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.addInitScript(({ progress }) => {
    const firstThread = {
      schemaVersion: 1,
      id: "thread-1",
      title: "Restored active turn",
      createdAtMs: 1,
      updatedAtMs: 2,
      archived: false,
    };
    const secondThread = {
      schemaVersion: 1,
      id: "thread-2",
      title: "Parallel conversation",
      createdAtMs: 3,
      updatedAtMs: 3,
      archived: false,
    };
    const emptyDetail = {
      schemaVersion: 1,
      summary: secondThread,
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
    };
    localStorage.setItem("kcoder_e2e_threads", JSON.stringify([firstThread, secondThread]));
    localStorage.setItem("kcoder_e2e_empty_thread_id", secondThread.id);
    localStorage.setItem("kcoder_e2e_thread_detail_by_id", JSON.stringify({
      [firstThread.id]: {
        ...emptyDetail,
        summary: firstThread,
        messages: [{
          schemaVersion: 1,
          id: "switch-user",
          role: "user",
          content: [{ type: "text", text: "检查会话切换" }],
          createdAtMs: 1,
        }],
      },
      [secondThread.id]: emptyDetail,
    }));
    localStorage.setItem("kcoder_e2e_switch_progress", progress);
  }, { progress: progressText });

  await page.goto("/");
  await page.evaluate((progress) => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 4, threadId: "thread-1", turnId: "turn-switch-active" };
    emit({ ...base, type: "turn_started", phase: "exploring" });
    emit({
      ...base,
      type: "text_delta",
      phase: "responding",
      itemId: "switch-progress",
      delta: progress,
    });
  }, progressText);
  await expect(page.getByText(progressText, { exact: true })).toBeVisible({ timeout: 10_000 });

  await page.evaluate((progress) => {
    const details = JSON.parse(localStorage.getItem("kcoder_e2e_thread_detail_by_id") ?? "{}") as Record<string, Record<string, unknown>>;
    details["thread-1"] = {
      ...details["thread-1"],
      messageTurnIds: {},
      turnUserMessageIds: { "turn-switch-active": "switch-user" },
      lastTurn: { turnId: "turn-switch-active", state: "streaming", error: null },
      turnTimeline: [{
        type: "text",
        id: "switch-progress",
        turnId: "turn-switch-active",
        text: progress,
      }],
    };
    localStorage.setItem("kcoder_e2e_thread_detail_by_id", JSON.stringify(details));
  }, progressText);

  await page.locator(".thread-item-main").filter({ hasText: "Parallel conversation" })
    .evaluate((element) => (element as HTMLButtonElement).click());
  await expect(page.getByRole("heading", { name: "Parallel conversation", exact: true })).toBeVisible();

  await page.locator(".sidebar-segmented [role='tab']").nth(1)
    .evaluate((element) => (element as HTMLButtonElement).click());
  const projectToggle = page.locator(".project-group-toggle").first();
  await projectToggle.evaluate((element) => (element as HTMLButtonElement).click());
  await page.locator(".thread-item--child .thread-item-main").filter({ hasText: "Restored active turn" })
    .evaluate((element) => (element as HTMLButtonElement).click());
  await expect(page.getByRole("heading", { name: "Restored active turn", exact: true })).toBeVisible();

  const progress = page.locator(".turn-progress-text").last();
  expect(await progress.textContent()).toBe(progressText);
  expect(await page.evaluate(() => (
    window as unknown as { __runTurnCalls: unknown[] }
  ).__runTurnCalls)).toHaveLength(0);

  await page.evaluate((delta) => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 4,
      threadId: "thread-1",
      turnId: "turn-switch-active",
      type: "text_delta",
      phase: "responding",
      itemId: "switch-progress",
      delta,
    });
  }, laterDelta);
  const completeText = progressText + laterDelta;
  expect(await progress.textContent()).not.toBe(completeText);
  await expect(page.getByText(completeText, { exact: true })).toBeVisible({ timeout: 10_000 });
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "turn_start" || command === "turn_retry"))).toHaveLength(0);
});

test("pastes local documents into the composer without intercepting plain text", async ({ page }, testInfo) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });

  const plainTextPrevented = await composer.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "保留普通文本粘贴");
    const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: transfer });
    element.dispatchEvent(event);
    return event.defaultPrevented;
  });
  expect(plainTextPrevented).toBe(false);

  await composer.fill("请总结附件");
  await page.evaluate(() => localStorage.setItem("kcoder_e2e_attachment_extract_delay_ms", "150"));
  await composer.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File(
      ["# Release notes\n\n- Paste files directly"],
      "release-notes.md",
      { type: "text/markdown", lastModified: 42 },
    ));
    element.dispatchEvent(new ClipboardEvent("paste", {
      bubbles: true,
      cancelable: true,
      clipboardData: transfer,
    }));
  });

  await expect(page.getByRole("button", { name: "发送消息", exact: true })).toBeDisabled();
  const pendingAttachment = page.getByLabel("待发送附件");
  await expect(pendingAttachment.getByText("release-notes.md", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "发送消息", exact: true })).toBeEnabled();
  await expect(composer).toHaveAttribute("placeholder", "输入消息，可直接粘贴或拖入文件");
  const extractionArgs = await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, Record<string, unknown>> }
  ).__invocationArgs.extract_local_document);
  expect(extractionArgs).toMatchObject({ name: "release-notes.md" });
  expect(extractionArgs).not.toHaveProperty("path");
  expect(extractionArgs.dataUrl).toMatch(/^data:text\/markdown;base64,/);
  await page.screenshot({ path: testInfo.outputPath("pasted-document-in-composer.png"), fullPage: true });

  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<{ request?: { input?: string } }> }
  ).__runTurnCalls[0]?.request?.input)).toContain("[附件: release-notes.md]");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<{ request?: { input?: string } }> }
  ).__runTurnCalls[0]?.request?.input)).toContain("Paste files directly");
  expect(await page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<{ attachments?: unknown[] }> }
  ).__runTurnCalls[0]?.attachments)).toEqual([]);
});

test("rejects unsupported pasted files with visible feedback", async ({ page }) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });
  await composer.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File([new Uint8Array([0, 1, 2])], "archive.zip", { type: "application/zip" }));
    element.dispatchEvent(new ClipboardEvent("paste", {
      bubbles: true,
      cancelable: true,
      clipboardData: transfer,
    }));
  });

  await expect(page.getByRole("status").filter({ hasText: "archive.zip：已选择，但暂不支持解析此文件类型" })).toBeVisible();
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "extract_local_document"))).toHaveLength(0);
});

test("allows selecting any file before reporting unsupported formats", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem(
      "kcoder_e2e_attachment_dialog_paths",
      JSON.stringify(["C:\\fixtures\\archive.zip"]),
    );
  });

  await page.getByRole("toolbar", { name: "输入快捷操作" })
    .getByRole("button", { name: "添加附件" })
    .click();

  await expect(page.getByRole("status").filter({
    hasText: "archive.zip：已选择，但暂不支持解析此文件类型",
  })).toBeVisible();
  expect(await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, unknown> }
  ).__invocationArgs["plugin:dialog|open"])).toEqual({ options: { multiple: true } });
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "plugin:fs|read_file"))).toHaveLength(0);
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "plugin:fs|stat"))).toHaveLength(0);
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "extract_local_document"))).toHaveLength(0);
});

test("keeps local attachment reads limited to dynamically scoped fs commands", async () => {
  const capability = JSON.parse(readFileSync(
    new URL("../src-tauri/capabilities/default.json", import.meta.url),
    "utf8",
  )) as { permissions: unknown[] };

  expect(capability.permissions).toEqual(expect.arrayContaining([
    "fs:allow-stat",
    "fs:allow-read-file",
  ]));
  expect(capability.permissions).not.toContain("fs:default");
  expect(capability.permissions.filter((permission) => {
    if (!permission || typeof permission !== "object") return false;
    const identifier = (permission as { identifier?: unknown }).identifier;
    return typeof identifier === "string" && identifier.startsWith("fs:");
  })).toEqual([]);
});

test("imports Excel through Tauri native file drop without duplicate DOM handling", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem(
      "kcoder_e2e_attachment_file_bytes",
      JSON.stringify([0x50, 0x4b, 0x03, 0x04]),
    );
  });
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __registeredTauriEvents: Map<string, number> }
  ).__registeredTauriEvents.has("tauri://drag-drop"))).toBe(true);

  const composer = page.locator(".composer");
  await composer.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const scaleFactor = window.devicePixelRatio || 1;
    const emit = (
      window as unknown as { __emitTauriEvent: (event: string, payload: unknown) => void }
    ).__emitTauriEvent;
    emit("tauri://drag-enter", {
      paths: ["C:\\fixtures\\native-budget.xlsx"],
      position: {
        x: (bounds.left + bounds.width / 2) * scaleFactor,
        y: (bounds.top + bounds.height / 2) * scaleFactor,
      },
    });
  });
  await expect(composer).toHaveClass(/composer--drag-active/);

  await page.evaluate(() => {
    (
      window as unknown as { __emitTauriEvent: (event: string, payload: unknown) => void }
    ).__emitTauriEvent("tauri://drag-over", { position: { x: 1, y: 1 } });
  });
  await expect(composer).not.toHaveClass(/composer--drag-active/);

  await composer.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const scaleFactor = window.devicePixelRatio || 1;
    const emit = (
      window as unknown as { __emitTauriEvent: (event: string, payload: unknown) => void }
    ).__emitTauriEvent;
    const position = {
      x: (bounds.left + bounds.width / 2) * scaleFactor,
      y: (bounds.top + bounds.height / 2) * scaleFactor,
    };
    emit("tauri://drag-over", { position });
    emit("tauri://drag-drop", {
      paths: ["C:\\fixtures\\native-budget.xlsx"],
      position,
    });
  });

  await expect(page.getByLabel("待发送附件")
    .getByText("native-budget.xlsx", { exact: true })).toBeVisible();
  await expect(composer).not.toHaveClass(/composer--drag-active/);

  await page.getByRole("textbox", { name: "消息" }).evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File(
      [new Uint8Array([0x50, 0x4b, 0x03, 0x04])],
      "native-budget.xlsx",
      { type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" },
    ));
    element.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  expect(await page.evaluate(() => (
    window as unknown as { __invoked: string[] }
  ).__invoked.filter((command) => command === "extract_local_document"))).toHaveLength(1);
});

test("selects supported Excel files after a metadata size preflight", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem(
      "kcoder_e2e_attachment_dialog_paths",
      JSON.stringify(["C:\\fixtures\\budget.xlsx"]),
    );
    localStorage.setItem(
      "kcoder_e2e_attachment_file_bytes",
      JSON.stringify([0x50, 0x4b, 0x03, 0x04]),
    );
  });

  await page.getByRole("toolbar", { name: "输入快捷操作" })
    .getByRole("button", { name: "添加附件" })
    .click();

  await expect(page.getByLabel("待发送附件").getByText("budget.xlsx", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, Record<string, unknown>> }
  ).__invocationArgs["plugin:fs|stat"])).toMatchObject({
    path: "C:\\fixtures\\budget.xlsx",
  });
  expect(await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, Record<string, unknown>> }
  ).__invocationArgs["plugin:fs|read_file"])).toMatchObject({
    path: "C:\\fixtures\\budget.xlsx",
  });
  const extractionArgs = await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, Record<string, unknown>> }
  ).__invocationArgs.extract_local_document);
  expect(extractionArgs).toMatchObject({ name: "budget.xlsx" });
  expect(extractionArgs.dataUrl).toMatch(
    /^data:application\/vnd\.openxmlformats-officedocument\.spreadsheetml\.sheet;base64,/,
  );
});

test("pastes common Excel workbooks and sends extracted sheet content", async ({ page }) => {
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "消息" });
  await composer.fill("请汇总预算");
  await composer.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File(
      [new Uint8Array([0x50, 0x4b, 0x03, 0x04])],
      "budget.xlsx",
      {
        type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        lastModified: 43,
      },
    ));
    element.dispatchEvent(new ClipboardEvent("paste", {
      bubbles: true,
      cancelable: true,
      clipboardData: transfer,
    }));
  });

  await expect(page.getByLabel("待发送附件").getByText("budget.xlsx", { exact: true })).toBeVisible();
  const extractionArgs = await page.evaluate(() => (
    window as unknown as { __invocationArgs: Record<string, Record<string, unknown>> }
  ).__invocationArgs.extract_local_document);
  expect(extractionArgs).toMatchObject({ name: "budget.xlsx" });
  expect(extractionArgs.dataUrl).toMatch(
    /^data:application\/vnd\.openxmlformats-officedocument\.spreadsheetml\.sheet;base64,/,
  );

  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<{ request?: { input?: string } }> }
  ).__runTurnCalls[0]?.request?.input)).toContain("[附件: budget.xlsx]");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __runTurnCalls: Array<{ request?: { input?: string } }> }
  ).__runTurnCalls[0]?.request?.input)).toContain("[工作表: 预算]\n项目\t金额\n住宿\t128.5");
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
  await expect(liveMessage.locator(".turn-tool-output")).toHaveCount(0);
  await expect(liveMessage.locator(".turn-command-summary")).toHaveCount(2);
  await expect(liveMessage.locator(".turn-command-summary").filter({ hasText: "运行失败" })).toHaveCount(1);
  await expect(liveMessage.locator(".turn-command-summary").filter({ hasText: "test failed" })).toHaveCount(1);
  await expect(liveMessage.locator(".turn-command-summary").filter({ hasText: "已运行" })).toHaveCount(1);
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
  await expect(restoredMessage.locator(".turn-tool-output")).toHaveCount(0);
  await expect(restoredMessage.locator(".turn-command-summary")).toHaveCount(2);
  await expect(restoredMessage.locator(".turn-command-summary").filter({ hasText: "运行失败" })).toHaveCount(1);
  await expect(restoredMessage.locator(".turn-command-summary").filter({ hasText: "test failed" })).toHaveCount(1);
  await expect(restoredMessage.locator(".turn-command-summary").filter({ hasText: "已运行" })).toHaveCount(1);
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

  const runningCommandRow = page.locator(".message--assistant").last().locator(".turn-timeline-tool--command");
  await expect(runningCommandRow.getByText("pnpm build", { exact: true })).toBeVisible();
  await expect(runningCommandRow.getByText("运行中", { exact: true })).toBeVisible();
  await expect(runningCommandRow.locator(".turn-command-inline, .turn-tool-output, .turn-tool-duration")).toHaveCount(0);

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
  const cancelledCommandRow = cancelledMessage.locator(".turn-timeline-tool--cancelled");
  await expect(cancelledCommandRow).toContainText("已取消");
  await expect(cancelledCommandRow.locator(".turn-command-inline, .turn-tool-output, .turn-tool-duration")).toHaveCount(0);
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
  await expect(page.locator(".plan-progress-popover").last()).not.toBeVisible();
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
  const question = "当前执行段已调用模型 100 次、累计消耗 920000 tokens、运行 480 秒。如需继续，请发送“继续”（点击“继续执行”即可）。";
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
  await expect(page.getByText(/420 tokens/)).toBeVisible();
  await page.getByRole("complementary", { name: "子智能体", exact: true }).getByRole("button", { name: /检查后端/ }).click();
  await expect(page.getByText("后端检查完成", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "返回列表" }).click();
  await page.getByRole("button", { name: "新建子任务" }).click();
  await page.getByLabel("子任务描述").fill("检查测试");
  await page.getByRole("button", { name: "启动" }).click();
  await expect(page.locator(".subagent-detail-title")).toContainText("检查测试");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __invoked: string[] }).__invoked.includes("create_subagent"))).toBe(true);
  await expect.poll(() => page.evaluate(() => {
    const args = (window as unknown as { __invocationArgs: Record<string, { request?: Record<string, unknown> }> }).__invocationArgs.create_subagent;
    return args?.request && Object.prototype.hasOwnProperty.call(args.request, "tokenBudget");
  })).toBe(false);
});

test("focuses only the clicked subagent and scopes the list to its conversation", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const threads = ["thread-1", "thread-2", "thread-empty"].map((id, index) => ({
      schemaVersion: 1, id, title: ["Phase 6 workbench", "Parallel conversation", "Empty conversation"][index],
      createdAtMs: index + 1, updatedAtMs: index + 1, archived: false,
      inProject: true, workspacePath: "D:\\code\\k-coder",
    }));
    const agents = ["agent-1", "agent-2", "agent-old"].map((id, index) => ({
      schemaVersion: 1, id, parentAgentId: null, parentThreadId: index === 2 ? "thread-2" : "thread-1",
      threadId: `thread-${id}`, label: ["检查后端", "检查文档", "历史任务"][index], task: `任务 ${id}`,
      state: "running", depth: 1, workspaceRoot: "D:\\code\\k-coder", capabilities: ["read_file"],
      tokenBudget: null, tokensUsed: 420, timeoutMs: 600000, createdAtMs: index + 2,
      updatedAtMs: index + 2, summary: null, error: null, turnCount: 1,
    }));
    localStorage.setItem("kcoder_e2e_threads", JSON.stringify(threads));
    localStorage.setItem("kcoder_e2e_subagents", JSON.stringify(agents));
    localStorage.setItem("kcoder_e2e_thread_detail_by_id", JSON.stringify(Object.fromEntries([
      ...threads.map((summary) => [summary.id, {
        schemaVersion: 1, summary, messages: [], messageTurnIds: {}, turnUserMessageIds: {}, lastTurn: null,
        toolActivities: [], turnTimeline: [], lastUsage: null, contextUsage: null,
        approvals: [], userInputs: [], changes: [], todos: [],
      }]),
      ...agents.map((agent) => [agent.threadId, {
        schemaVersion: 1, summary: { ...threads[0], id: agent.threadId, title: agent.label },
        messages: [{ id: `message-${agent.id}`, role: "assistant", content: [{ type: "text", text: `${agent.label}独立历史` }] }],
      }]),
    ])));
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench", exact: true })).toBeVisible();
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 6, threadId: "thread-1", turnId: "turn-focus", phase: "executing" };
    emit({ ...base, type: "turn_started" });
    for (const agentId of ["agent-1", "agent-2"]) {
      emit({ ...base, type: "tool_queued", call: { id: `wait-${agentId}`, name: "wait_agent", arguments: { agentId }, metadata: {} } });
    }
  });
  const drawer = page.getByRole("complementary", { name: "子智能体", exact: true });
  await page.getByRole("button", { name: "查看子智能体 task1", exact: true }).click();
  await expect(drawer.locator(".subagent-detail-title")).toContainText("检查后端");
  await expect(drawer.getByText("检查后端独立历史", { exact: true })).toBeVisible();
  await expect(drawer.locator(".subagent-row")).toHaveCount(0);
  await page.evaluate(() => {
    (window as unknown as { __emitTauriEvent: (name: string, event: unknown) => void }).__emitTauriEvent("agent-event", {
      schemaVersion: 6, threadId: "thread-agent-1", turnId: "child-turn-1", phase: "answering",
      type: "text_delta", itemId: "child-item-1", delta: "仅 task1 的实时输出",
    });
  });
  await expect(drawer.getByText("仅 task1 的实时输出", { exact: true })).toBeVisible();
  await drawer.getByPlaceholder("向该子智能体发送消息…").fill("仅发给 task1 的草稿");
  // On narrow windows the panel occupies the conversation area; close it before selecting another chip.
  if (testInfo.project.name === "narrow") await page.getByRole("button", { name: "子智能体", exact: true }).click();
  await page.getByRole("button", { name: "查看子智能体 task2", exact: true }).click();
  await expect(drawer.locator(".subagent-detail-title")).toContainText("检查文档");
  await expect(drawer.getByText("检查文档独立历史", { exact: true })).toBeVisible();
  await expect(drawer.getByText("检查后端独立历史", { exact: true })).toHaveCount(0);
  await expect(drawer.getByText("仅 task1 的实时输出", { exact: true })).toHaveCount(0);
  await expect(drawer.getByPlaceholder("向该子智能体发送消息…")).toHaveValue("");
  await page.screenshot({ path: testInfo.outputPath(`focused-task-${testInfo.project.name}.png`), fullPage: true });
  await drawer.getByRole("button", { name: "返回列表" }).click();
  await expect(drawer.locator(".subagent-row")).toHaveCount(2);
  await expect(drawer.locator(".agent-count-badge")).toHaveText("2 运行中");
  await expect(drawer.getByText("历史任务", { exact: true })).toHaveCount(0);
  await page.evaluate(() => {
    const agents = JSON.parse(localStorage.getItem("kcoder_e2e_subagents")!) as Array<Record<string, unknown>>;
    const emit = (window as unknown as { __emitTauriEvent: (name: string, event: unknown) => void }).__emitTauriEvent;
    emit("subagent-event", { ...agents[0], state: "completed", updatedAtMs: 10 });
    emit("subagent-event", { ...agents[2], label: "其他会话实时任务", updatedAtMs: 11 });
  });
  await expect(drawer.locator(".agent-count-badge")).toHaveText("1 运行中");
  await expect(drawer.locator(".subagent-row")).toHaveCount(2);
  await expect(drawer.getByText("其他会话实时任务", { exact: true })).toHaveCount(0);

  // The sidebar is intentionally hidden at the narrow breakpoint; validate conversation switching at desktop width.
  await page.setViewportSize({ width: 1500, height: 900 });
  await page.getByRole("tab", { name: "项目", exact: true }).click();
  await page.getByRole("button", { name: "展开项目", exact: true }).click();
  await drawer.getByRole("button", { name: /检查后端/ }).click();
  await page.getByRole("button", { name: "Parallel conversation", exact: true }).click();
  await expect(drawer.locator(".subagent-detail")).toHaveCount(0);
  await expect(drawer.locator(".subagent-row")).toHaveCount(1);
  await expect(drawer.getByText("其他会话实时任务", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Empty conversation", exact: true }).click();
  await expect(drawer.getByText("当前会话暂无子任务", { exact: true })).toBeVisible();
  await expect(drawer.locator(".agent-count-badge")).toHaveCount(0);
  await page.locator(".thread-item-main").filter({ hasText: "Phase 6 workbench" }).click();
  await expect(drawer.locator(".subagent-detail")).toHaveCount(0);
  await expect(drawer.locator(".subagent-row")).toHaveCount(2);
});

test("shows every queued subagent wait before serial execution reaches it", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => {
    try {
      (window as unknown as { __emitTauriEvent: (event: string, payload: unknown) => void }).__emitTauriEvent(
        "subagent-event",
        {
          schemaVersion: 1,
          id: "agent-2",
          parentAgentId: null,
          parentThreadId: "thread-1",
          threadId: "thread-agent-2",
          label: "检查文档",
          task: "分析文档结构",
          state: "running",
          depth: 1,
          workspaceRoot: "D:\\code\\k-coder",
          capabilities: ["list_directory", "read_file"],
          tokenBudget: null,
          tokensUsed: 0,
          timeoutMs: 600000,
          createdAtMs: 4,
          updatedAtMs: 4,
          summary: null,
          error: null,
        },
      );
      return true;
    } catch {
      return false;
    }
  })).toBe(true);

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 6, threadId: "thread-1", turnId: "turn-subagent-waits" };
    const calls = [
      { id: "call-wait-1", name: "wait_agent", arguments: { agentId: "agent-1" }, metadata: {} },
      { id: "call-wait-2", name: "wait_agent", arguments: { agentId: "agent-2" }, metadata: {} },
    ];
    emit({ ...base, type: "turn_started", phase: "exploring" });
    for (const call of calls) {
      emit({ ...base, type: "item_started", phase: "executing", itemId: call.id, itemType: "tool" });
      emit({ ...base, type: "tool_queued", phase: "executing", call });
    }
    emit({ ...base, type: "tool_started", phase: "executing", call: calls[0] });
  });

  const liveMessage = page.locator(".message--assistant").last();
  await expect(liveMessage.locator(".turn-timeline-tool")).toHaveCount(2);
  await expect(
    liveMessage.locator(".turn-timeline-tool--running").getByRole("button", { name: "查看子智能体 task1" }),
  ).toBeVisible();
  await expect(
    liveMessage.locator(".turn-timeline-tool--pending").getByRole("button", { name: "查看子智能体 task2" }),
  ).toBeVisible();
  await expect(
    liveMessage.locator(".turn-timeline-tool--running .turn-tool-meta > span:not(.turn-tool-duration)"),
  ).toHaveText("执行中");
  await expect(
    liveMessage.locator(".turn-timeline-tool--pending .turn-tool-meta > span:not(.turn-tool-duration)"),
  ).toHaveText("等待执行");
  await page.screenshot({ path: testInfo.outputPath(`queued-subagent-waits-${testInfo.project.name}.png`), fullPage: true });

  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 6, threadId: "thread-1", turnId: "turn-subagent-waits", phase: "executing" };
    for (const callId of ["call-wait-1", "call-wait-2"]) {
      emit({
        ...base,
        type: "tool_completed",
        callId,
        name: "wait_agent",
        result: { success: false, output: "tool execution was cancelled", metadata: {} },
      });
      emit({
        ...base,
        type: "item_completed",
        itemId: callId,
        itemType: "tool",
        status: "cancelled",
      });
    }
  });

  await expect(liveMessage.locator(".turn-timeline-tool")).toHaveCount(2);
  await expect(
    liveMessage.locator(".turn-timeline-tool--cancelled").getByRole("button", { name: "查看子智能体 task1" }),
  ).toBeVisible();
  await expect(
    liveMessage.locator(".turn-timeline-tool--cancelled").getByRole("button", { name: "查看子智能体 task2" }),
  ).toBeVisible();
  await expect(liveMessage.locator(".turn-timeline-tool--failed")).toHaveCount(0);
});

test("batch subagent wait shows all targets and distinguishes timeout from task completion", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_subagents", JSON.stringify([1, 2, 3, 4].map(n => ({
      schemaVersion: 1, id: `batch-${n}`, parentAgentId: null, parentThreadId: "thread-1",
      threadId: `child-batch-${n}`, label: `批量任务${n}`, task: "检查文档", state: "running", depth: 1,
      workspaceRoot: "D:\\code\\k-coder", capabilities: ["read_file"], tokenBudget: null,
      tokensUsed: 0, timeoutMs: 600000, createdAtMs: n, updatedAtMs: 30000, summary: null, error: null,
    }))));
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench", exact: true })).toBeVisible();
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 7, threadId: "thread-1", turnId: "turn-batch-wait", phase: "executing" };
    const call = { id: "batch-wait", name: "wait_agent", arguments: { agentIds: ["batch-1", "batch-2", "batch-3", "batch-4"] }, metadata: {} };
    emit({ ...base, type: "turn_started" });
    emit({ ...base, type: "tool_queued", call });
    emit({ ...base, type: "tool_started", call });
  });
  const row = page.locator('.turn-timeline-tool').filter({ hasText: "等待任一子智能体结束" }).first();
  await expect(row).toHaveCount(1);
  await expect(row.locator('.subagent-task-chip')).toHaveCount(4);
  await expect(row).toContainText("等待耗时");
  for (let n = 1; n <= 4; n++) {
    await expect(row.getByRole('button', { name: `查看子智能体 task${n}`, exact: true })).toBeVisible();
  }
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 7, threadId: "thread-1", turnId: "turn-batch-wait", phase: "executing" };
    emit({ ...base, type: "tool_completed", callId: "batch-wait", name: "wait_agent", result: {
      success: true, output: JSON.stringify({ schemaVersion: 1, agents: [], finishedAgentIds: [], timedOut: true }), metadata: {},
    }});
    emit({ ...base, type: "item_completed", itemId: "batch-wait", itemType: "tool", status: "completed" });
  });
  await expect(row).toContainText("本次等待结束，子任务仍在运行");
  const group = page.locator('.turn-tool-group').filter({ has: row });
  await expect(group).not.toHaveAttribute('open', '');
  await group.locator(':scope > summary').click();
  await page.evaluate(() => {
    const bridge = window as unknown as { __emitTauriEvent: (name: string, event: unknown) => void };
    const emit = (event: unknown) => bridge.__emitTauriEvent("agent-event", event);
    const base = { schemaVersion: 7, threadId: "thread-1", turnId: "turn-batch-wait", phase: "executing" };
    const call = { id: "batch-next", name: "wait_agent", arguments: { agentIds: ["batch-1", "batch-2"] }, metadata: {} };
    emit({ ...base, type: "tool_started", call });
    emit({ ...base, type: "tool_completed", callId: call.id, name: call.name, result: {
      success: true, output: JSON.stringify({ schemaVersion: 1, agents: [{ id: 'batch-2', state: 'completed' }], finishedAgentIds: ['batch-2'], timedOut: false }), metadata: {},
    }});
    emit({ ...base, type: "item_completed", itemId: call.id, itemType: "tool", status: "completed" });
  });
  await expect(page.locator('.turn-timeline-tool').filter({ hasText: '已获取结果' })).toHaveCount(1);
  // Open the child detail only after emitting parent events: the mock bridge
  // keeps one callback per event name, unlike the real Tauri event bus.
  const finalGroup = page.locator('.turn-tool-group').filter({ has: page.locator('.subagent-task-chip[aria-label="查看子智能体 task3"]') });
  await expect(finalGroup).not.toHaveAttribute('open', '');
  await finalGroup.locator(':scope > summary').click();
  await row.getByRole('button', { name: '查看子智能体 task3', exact: true }).click();
  const drawer = page.getByRole('complementary', { name: '子智能体', exact: true });
  await expect(drawer.locator('.subagent-detail-title')).toContainText('批量任务3');
  await expect(drawer.locator('.subagent-detail-meta')).toContainText('子任务耗时');
  await expect(drawer.locator('.subagent-detail-meta')).toContainText('30s');
  await page.screenshot({ path: testInfo.outputPath(`batch-subagent-wait-${testInfo.project.name}.png`), fullPage: true });
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

test("shows recovered and live context-window progress in the composer", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_theme", "dark");
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Context progress",
        createdAtMs: 1,
        updatedAtMs: 2,
        archived: false,
        inProject: true,
        workspacePath: "D:\\code\\k-coder",
      },
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
      lastUsage: { inputTokens: 400_000, outputTokens: 20_000, totalTokens: 420_000 },
      contextUsage: { inputTokens: 14_000, outputTokens: 1_360, totalTokens: 15_360 },
    }));
  });
  await page.goto("/");

  const trigger = page.locator(".context-progress-trigger");
  const sendButton = page.getByRole("button", { name: "发送消息" });
  await expect(trigger).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-label", "上下文 12%，15K / 128K");

  const initialViewport = page.viewportSize()!;
  const validationWidths = testInfo.project.name === "desktop" ? [1280] : [700, 596, 375];
  for (const width of validationWidths) {
    await page.setViewportSize({ width, height: 820 });
    const quickActionLayout = await page.locator(".composer").evaluate((composer) => {
      const toolbar = composer.querySelector<HTMLElement>(".composer-quick-actions")!;
      const textarea = composer.querySelector<HTMLElement>("textarea")!;
      const workflow = toolbar.querySelector<HTMLElement>(".workflow-toggle")!;
      const rect = (element: HTMLElement) => {
        const box = element.getBoundingClientRect();
        return { top: box.top, right: box.right, bottom: box.bottom, left: box.left };
      };
      return {
        composer: rect(composer as HTMLElement),
        toolbar: rect(toolbar),
        textarea: rect(textarea),
        workflow: rect(workflow),
        buttons: Array.from(toolbar.querySelectorAll<HTMLElement>("button")).map(rect),
      };
    });
    const composerLayout = await page.locator(".composer-footer").evaluate((footer) => {
      const controls = footer.querySelector<HTMLElement>(".composer-controls")!;
      const options = footer.querySelector<HTMLElement>(".composer-options")!;
      const actions = footer.querySelector<HTMLElement>(".composer-actions")!;
      const project = footer.querySelector<HTMLElement>(".project-selector-trigger")!;
      const mode = footer.querySelector<HTMLElement>(".mode-toggle")!;
      const rect = (element: HTMLElement) => {
        const box = element.getBoundingClientRect();
        return { top: box.top, right: box.right, bottom: box.bottom, left: box.left };
      };
      const interactiveBounds = Array.from(footer.querySelectorAll<HTMLElement>("button")).map(rect);
      return {
        footer: rect(footer as HTMLElement),
        controls: rect(controls),
        options: rect(options),
        actions: rect(actions),
        project: rect(project),
        mode: rect(mode),
        interactiveBounds,
        approvalInActions: Boolean(actions.querySelector(".approval-mode-selector")),
      };
    });
    expect(quickActionLayout.toolbar.bottom).toBeLessThanOrEqual(quickActionLayout.textarea.top + 1);
    expect(quickActionLayout.workflow.bottom).toBeLessThanOrEqual(quickActionLayout.textarea.top + 1);
    expect(quickActionLayout.textarea.bottom).toBeLessThanOrEqual(composerLayout.footer.top + 1);
    for (const bounds of quickActionLayout.buttons) {
      expect(bounds.left).toBeGreaterThanOrEqual(quickActionLayout.composer.left - 1);
      expect(bounds.right).toBeLessThanOrEqual(quickActionLayout.composer.right + 1);
      expect(bounds.top).toBeGreaterThanOrEqual(quickActionLayout.toolbar.top - 1);
      expect(bounds.bottom).toBeLessThanOrEqual(quickActionLayout.toolbar.bottom + 1);
    }
    expect(composerLayout.options.right).toBeLessThanOrEqual(composerLayout.actions.left + 1);
    expect(composerLayout.actions.right).toBeLessThanOrEqual(composerLayout.footer.right + 1);
    expect(composerLayout.project.right).toBeLessThanOrEqual(composerLayout.mode.left + 1);
    expect(Math.abs(composerLayout.project.top - composerLayout.mode.top)).toBeLessThanOrEqual(1);
    expect(composerLayout.approvalInActions).toBe(true);
    for (const bounds of composerLayout.interactiveBounds) {
      expect(bounds.left).toBeGreaterThanOrEqual(composerLayout.footer.left - 1);
      expect(bounds.right).toBeLessThanOrEqual(composerLayout.footer.right + 1);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`composer-${width}.png`) });
  }
  await page.setViewportSize(initialViewport);

  await trigger.click();

  const popover = page.getByRole("dialog", { name: "上下文用量" });
  await expect(popover).toBeVisible();
  await expect(popover).toContainText("15K / 128K");
  await expect(popover).toContainText("12%");
  await expect(popover.getByRole("progressbar", { name: "上下文占用" })).toHaveAttribute("aria-valuenow", "12");

  const geometry = await Promise.all([trigger.boundingBox(), sendButton.boundingBox(), popover.boundingBox()]);
  expect(geometry.every(Boolean)).toBe(true);
  expect(geometry[0]!.x + geometry[0]!.width).toBeLessThanOrEqual(geometry[1]!.x);
  expect(geometry[2]!.x).toBeGreaterThanOrEqual(0);
  expect(geometry[2]!.x + geometry[2]!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({
    path: testInfo.outputPath(`context-progress-${testInfo.project.name}.png`),
    fullPage: true,
  });

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 5,
      type: "usage_updated",
      phase: "executing",
      threadId: "thread-1",
      turnId: "turn-context",
      usage: { inputTokens: 230_000, outputTokens: 20_000, totalTokens: 250_000 },
      contextUsage: { inputTokens: 84_000, outputTokens: 5_600, totalTokens: 89_600 },
    });
  });
  await expect(trigger).toHaveAttribute("aria-label", "上下文 70%，90K / 128K");
  await expect(popover).toContainText("90K / 128K");
  await expect(popover.getByRole("progressbar", { name: "上下文占用" })).toHaveAttribute("aria-valuenow", "70");

  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({
      schemaVersion: 5,
      type: "context_compacted",
      phase: "executing",
      threadId: "thread-1",
      turnId: "turn-context",
      itemId: "compaction-context",
      automatic: true,
      compactedMessageCount: 12,
      userConstraintCount: 1,
      recentToolResultCount: 2,
      recentUserMessageCount: 2,
    });
  });
  await expect(trigger).toHaveAttribute("aria-label", "上下文用量等待更新");
  await expect(popover).toContainText("-- / 128K");
  await expect(popover).toContainText("模型返回下一次用量后更新");
  await expect(popover.getByRole("progressbar", { name: "上下文占用" })).not.toHaveAttribute("aria-valuenow");
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
  const menu = page.getByRole("menu", { name: "操作批准方式" });
  const expectMenuWithinComposer = async () => {
    const geometry = await page.evaluate(() => {
      const composer = document.querySelector<HTMLElement>(".composer")!;
      const approvalMenu = document.querySelector<HTMLElement>(".approval-mode-menu")!;
      const menuRect = approvalMenu.getBoundingClientRect();
      const composerRect = composer.getBoundingClientRect();
      const descriptions = [...approvalMenu.querySelectorAll<HTMLElement>("small")].map((element) => {
        const rect = element.getBoundingClientRect();
        return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom };
      });
      return {
        menu: { left: menuRect.left, right: menuRect.right, top: menuRect.top, bottom: menuRect.bottom },
        composer: { left: composerRect.left, right: composerRect.right, top: composerRect.top },
        descriptions,
        clientWidth: approvalMenu.clientWidth,
        scrollWidth: approvalMenu.scrollWidth,
      };
    });
    expect(geometry.menu.left).toBeGreaterThanOrEqual(geometry.composer.left + 11);
    expect(geometry.menu.right).toBeLessThanOrEqual(geometry.composer.right - 11);
    expect(geometry.menu.top).toBeGreaterThanOrEqual(11);
    expect(geometry.menu.bottom).toBeLessThanOrEqual(geometry.composer.top - 7);
    expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.clientWidth);
    for (const description of geometry.descriptions) {
      expect(description.left).toBeGreaterThanOrEqual(geometry.menu.left);
      expect(description.right).toBeLessThanOrEqual(geometry.menu.right);
      expect(description.top).toBeGreaterThanOrEqual(geometry.menu.top);
      expect(description.bottom).toBeLessThanOrEqual(geometry.menu.bottom);
    }
  };

  await expect(trigger).toHaveAttribute("aria-label", "操作批准方式：请求批准");
  await expect(trigger).not.toContainText("请求批准");
  await trigger.click();
  await expect(menu).toBeVisible();
  await expectMenuWithinComposer();
  await expect(menu.getByRole("menuitemradio", { name: /请求批准/ })).toHaveAttribute("aria-checked", "true");
  await menu.getByRole("menuitemradio", { name: /完整访问/ }).click();

  await expect(trigger).toContainText("完整访问");
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __lastApprovalMode: string | null }).__lastApprovalMode,
  )).toBe("full_access");
  await expect(trigger.locator("span")).toBeVisible();
  if (testInfo.project.name === "narrow") {
    await page.setViewportSize({ width: 596, height: 820 });
    await expect(trigger.locator("span")).toBeVisible();
    const groups = await Promise.all([
      page.locator(".composer-options").boundingBox(),
      page.locator(".composer-actions").boundingBox(),
    ]);
    expect(groups.every(Boolean)).toBe(true);
    expect(groups[0]!.x + groups[0]!.width).toBeLessThanOrEqual(groups[1]!.x + 1);
    await page.setViewportSize({ width: 420, height: 820 });
    await expect(trigger.locator("span")).toBeHidden();
  }
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
  await expectMenuWithinComposer();
  await page.waitForTimeout(250);
  await page.screenshot({ path: testInfo.outputPath(`approval-mode-${testInfo.project.name}.png`), fullPage: true });
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("adds, edits, deletes, and saves structured provider models", async ({ page }) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();

  await page.getByRole("button", { name: "新增模型" }).click();
  await expect(page.locator(".provider-model-card")).toHaveCount(3);
  await page.getByLabel("模型 ID 3").fill("gpt-5.6-sol");
  await page.getByLabel("显示名称 3").fill("GPT-5.6 Sol");
  await page.getByLabel("上下文长度 3").fill("200000");
  await expect(page.locator(".provider-model-card").nth(2).getByRole("checkbox", { name: "支持图片" })).toBeChecked();
  await page.getByLabel(/设为默认模型：GPT-5.6 Sol/).check();

  await page.getByRole("button", { name: "删除模型：GPT-4 Omni" }).click();
  await expect(page.locator(".provider-model-card")).toHaveCount(2);
  await page.getByRole("button", { name: "保存配置" }).click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastProviderRequest: { model: string } | null }).__lastProviderRequest?.model)).toBe("gpt-5.6-sol");
  const request = await page.evaluate(() => (window as unknown as { __lastProviderRequest: { models: unknown[] } }).__lastProviderRequest);
  expect(request.models).toEqual([
    { id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, maxOutputTokens: undefined, supportsVision: true, fallback: false },
    { id: "gpt-5.6-sol", displayName: "GPT-5.6 Sol", contextWindow: 200000, maxOutputTokens: undefined, supportsVision: true, fallback: false },
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
  await expect(page.locator(".usage-summary-grid > div").filter({ hasText: "自动重试" }).getByText("2", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "运行回归评估" }).click();
  await expect(page.getByText(/回归评估 3\/3/)).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath(`phase9-${testInfo.project.name}.png`), fullPage: true });
});

test("shows detailed usage tracking by token, day, and model", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  await page.getByRole("button", { name: /用量追踪/ }).click();

  await expect(page.getByRole("heading", { name: "用量追踪" })).toBeVisible();
  const details = page.getByRole("region", { name: "Token 明细" });
  await expect(details.getByText("193.6K", { exact: true })).toBeVisible();
  await expect(details.getByText("96.5K", { exact: true })).toBeVisible();
  await expect(details.getByText("50.7%", { exact: true })).toBeVisible();

  const trend = page.getByRole("img", { name: "最近 30 天 Token 趋势" });
  await expect(trend.locator(".usage-trend-bar")).toHaveCount(30);
  await expect(trend.locator('.usage-trend-bar[data-has-usage="true"]')).toHaveCount(2);

  const modelTable = page.getByRole("table", { name: "按模型统计" });
  await expect(modelTable.getByText("deepseek-v4-pro-0813", { exact: true })).toBeVisible();
  await expect(modelTable.getByText("DeepSeek", { exact: true })).toBeVisible();
  await expect(modelTable.getByText("未知", { exact: true })).toBeVisible();
  await expect(modelTable.getByText("14", { exact: true })).toBeVisible();

  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath(`usage-tracking-${testInfo.project.name}.png`), fullPage: true });
  await modelTable.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath(`usage-models-${testInfo.project.name}.png`), fullPage: true });
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

test("restores a steered turn around the user guidance boundary", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const turnId = "turn-restored-steer";
    const messageItem = (
      id: string,
      role: "user" | "assistant",
      text: string,
      createdAtMs: number,
      image?: { name: string; dataUrl: string },
    ) => ({
      schemaVersion: 1,
      id,
      turnId,
      status: "completed",
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs,
      timelineItems: role === "assistant" ? [{ type: "text", id, turnId, text }] : [],
      type: role === "user" ? "user_message" : "agent_message",
      message: {
        schemaVersion: 1,
        id,
        role,
        content: [
          { type: "text", text },
          ...(image ? [{ type: "image", name: image.name, dataUrl: image.dataUrl }] : []),
        ],
        createdAtMs,
      },
      ...(role === "assistant" ? { phase: "final_answer" } : {}),
    });
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Steered conversation",
        createdAtMs: 1,
        updatedAtMs: 6,
        archived: false,
        workspacePath: "D:\\code\\k-coder",
      },
      lastTurn: { turnId, state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: "steer-initial-user",
          state: "completed",
          error: null,
          startedAtMs: 2,
          completedAtMs: 6,
          durationMs: 4,
          itemsView: "full",
          items: [
            messageItem("steer-initial-user", "user", "开始检查实现。", 2),
            messageItem(
              "assistant-before-restored-steer",
              "assistant",
              "这是刷新前的阶段回复。",
              3,
              {
                name: "pre-steer-result.png",
                dataUrl: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
              },
            ),
            messageItem("restored-steer-user", "user", "改为先检查子智能体实现。", 4),
            messageItem(
              "assistant-after-restored-steer",
              "assistant",
              "这是根据新引导继续的回复。",
              5,
              {
                name: "steered-result.png",
                dataUrl: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
              },
            ),
            {
              schemaVersion: 1,
              id: `turn-completed-${turnId}`,
              turnId,
              status: "completed",
              startedAtMs: 6,
              completedAtMs: 6,
              timelineItems: [{
                type: "event",
                itemId: `turn-completed-${turnId}`,
                turnId,
                kind: "turn_completed",
                title: "Turn 已完成",
                detail: null,
                durationMs: 4,
              }],
              type: "event",
            },
          ],
        }],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  await expect(page.getByText("这是刷新前的阶段回复。", { exact: true })).toHaveCount(1);
  await expect(page.locator(".message--user").getByText("改为先检查子智能体实现。", { exact: true })).toHaveCount(1);
  await expect(page.getByText("这是根据新引导继续的回复。", { exact: true })).toBeVisible();
  await expect(page.locator('article.message--assistant[data-turn-id="turn-restored-steer"]')).toHaveCount(2);
  const restoredPreSteerAssistant = page
    .locator('article.message--assistant[data-turn-id="turn-restored-steer"]')
    .filter({ hasText: "这是刷新前的阶段回复。" });
  await expect(restoredPreSteerAssistant.locator(".turn-final-response")).toContainText("这是刷新前的阶段回复。");
  await expect(restoredPreSteerAssistant.getByRole("button", { name: "查看图片 pre-steer-result.png" })).toBeVisible();
  await expect(restoredPreSteerAssistant.getByRole("button", { name: "查看图片 steered-result.png" })).toHaveCount(0);
  const restoredPostSteerAssistant = page
    .locator('article.message--assistant[data-turn-id="turn-restored-steer"]')
    .filter({ hasText: "这是根据新引导继续的回复。" });
  await expect(restoredPostSteerAssistant.getByRole("button", { name: "查看图片 pre-steer-result.png" })).toHaveCount(0);
  const restoredAttachment = restoredPostSteerAssistant.getByRole("button", { name: "查看图片 steered-result.png" });
  await expect(restoredAttachment).toBeVisible();
  await expect.poll(() => restoredAttachment.locator("img").evaluate((image) => (
    (image as HTMLImageElement).complete && (image as HTMLImageElement).naturalWidth > 0
  ))).toBe(true);
  const restoredOrder = await page.locator(".message-list").textContent() ?? "";
  expect(restoredOrder.indexOf("这是刷新前的阶段回复。")).toBeLessThan(
    restoredOrder.indexOf("改为先检查子智能体实现。"),
  );
  expect(restoredOrder.indexOf("改为先检查子智能体实现。")).toBeLessThan(
    restoredOrder.indexOf("这是根据新引导继续的回复。"),
  );
  await page.screenshot({ path: testInfo.outputPath("restored-steered-turn.png"), fullPage: true });
});

test("does not repeat a pre-steer answer when the steered tail has no text", async ({ page }) => {
  await page.addInitScript(() => {
    const turnId = "turn-steer-empty-tail";
    const messageItem = (
      id: string,
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
      timelineItems: role === "assistant" ? [{ type: "text", id, turnId, text }] : [],
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
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Steered empty tail",
        createdAtMs: 1,
        updatedAtMs: 5,
        archived: false,
      },
      lastTurn: { turnId, state: "cancelled", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: "empty-tail-initial-user",
          state: "cancelled",
          error: null,
          startedAtMs: 2,
          completedAtMs: 5,
          durationMs: 3,
          itemsView: "full",
          items: [
            messageItem("empty-tail-initial-user", "user", "先给出初步结论。", 2),
            messageItem("empty-tail-assistant", "assistant", "这是引导前唯一的回复。", 3),
            messageItem("empty-tail-steer-user", "user", "先停一下，不要继续回答。", 4),
            {
              schemaVersion: 1,
              id: `turn-cancelled-${turnId}`,
              turnId,
              status: "cancelled",
              startedAtMs: 5,
              completedAtMs: 5,
              timelineItems: [{
                type: "event",
                itemId: `turn-cancelled-${turnId}`,
                turnId,
                kind: "turn_cancelled",
                title: "Turn 已停止",
                detail: null,
                durationMs: 3,
              }],
              type: "event",
            },
          ],
        }],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  await expect(page.getByText("这是引导前唯一的回复。", { exact: true })).toHaveCount(1);
  const listText = await page.locator(".message-list").textContent() ?? "";
  expect(listText.indexOf("这是引导前唯一的回复。")).toBeLessThan(
    listText.indexOf("先停一下，不要继续回答。"),
  );
});

test("does not duplicate tool-only steered turns as orphan activity", async ({ page }) => {
  await page.addInitScript(() => {
    const turnId = "turn-steer-tools-only";
    const userItem = (id: string, text: string, createdAtMs: number) => ({
      schemaVersion: 1,
      id,
      turnId,
      status: "completed",
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs,
      timelineItems: [],
      type: "user_message",
      message: {
        schemaVersion: 1,
        id,
        role: "user",
        content: [{ type: "text", text }],
        createdAtMs,
      },
    });
    const toolItem = (id: string, path: string, createdAtMs: number) => {
      const activity = {
        turnId,
        call: { id, name: "read_file", arguments: { path }, metadata: {} },
        state: "completed",
        result: { success: true, output: path, metadata: {} },
        startedAtMs: createdAtMs,
        completedAtMs: createdAtMs + 1,
        durationMs: 1,
      };
      return {
        schemaVersion: 1,
        id,
        turnId,
        status: "completed",
        startedAtMs: createdAtMs,
        completedAtMs: createdAtMs + 1,
        timelineItems: [{ type: "tool", activity }],
        type: "tool",
        activity,
      };
    };
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Steered tools only",
        createdAtMs: 1,
        updatedAtMs: 7,
        archived: false,
      },
      lastTurn: { turnId, state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: "tools-initial-user",
          state: "completed",
          error: null,
          startedAtMs: 2,
          completedAtMs: 7,
          durationMs: 5,
          itemsView: "full",
          items: [
            userItem("tools-initial-user", "检查两个文件。", 2),
            toolItem("tool-before-steer", "src/before.ts", 3),
            userItem("tools-steer-user", "改为先检查后一个文件。", 5),
            toolItem("tool-after-steer", "src/after.ts", 6),
          ],
        }],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  await expect(page.locator('article.message--assistant[data-turn-id="turn-steer-tools-only"]')).toHaveCount(2);
  await expect(page.locator(".turn-timeline-tool")).toHaveCount(2);
  const listText = await page.locator(".message-list").textContent() ?? "";
  expect(listText.indexOf("src/before.ts")).toBeLessThan(listText.indexOf("改为先检查后一个文件。"));
  expect(listText.indexOf("改为先检查后一个文件。")).toBeLessThan(listText.indexOf("src/after.ts"));
});

test("keeps the pre-steer segment when the original retry owner is on an older page", async ({ page }) => {
  await page.addInitScript(() => {
    const turnId = "turn-steer-newer-page";
    const messageItem = (
      id: string,
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
      timelineItems: role === "assistant" ? [{ type: "text", id, turnId, text }] : [],
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
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Paginated steered retry",
        createdAtMs: 1,
        updatedAtMs: 105,
        archived: false,
      },
      lastTurn: { turnId, state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [{
          schemaVersion: 1,
          id: turnId,
          userMessageId: "retry-owner-on-older-page",
          state: "completed",
          error: null,
          startedAtMs: 101,
          completedAtMs: 105,
          durationMs: 4,
          itemsView: "full",
          items: [
            messageItem("newer-page-pre-steer", "assistant", "分页中引导前的回复。", 102),
            messageItem("newer-page-steer-user", "user", "分页中改用另一种方法。", 103),
            messageItem("newer-page-post-steer", "assistant", "分页中引导后的回复。", 104),
          ],
        }],
        nextCursor: "older-page-cursor",
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  await expect(page.getByText("分页中引导前的回复。", { exact: true })).toHaveCount(1);
  await expect(page.getByText("分页中引导后的回复。", { exact: true })).toHaveCount(1);
  const listText = await page.locator(".message-list").textContent() ?? "";
  expect(listText.indexOf("分页中引导前的回复。")).toBeLessThan(listText.indexOf("分页中改用另一种方法。"));
  expect(listText.indexOf("分页中改用另一种方法。")).toBeLessThan(listText.indexOf("分页中引导后的回复。"));
});

test("keeps retry attempts on the correct side of steer boundaries", async ({ page }) => {
  await page.addInitScript(() => {
    const messageItem = (
      turnId: string,
      id: string,
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
      timelineItems: role === "assistant" ? [{ type: "text", id, turnId, text }] : [],
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
    const terminalItem = (turnId: string, kind: "turn_failed" | "turn_completed", createdAtMs: number) => ({
      schemaVersion: 1,
      id: `${kind}-${turnId}`,
      turnId,
      status: kind === "turn_failed" ? "failed" : "completed",
      startedAtMs: createdAtMs,
      completedAtMs: createdAtMs,
      timelineItems: [{
        type: "event",
        itemId: `${kind}-${turnId}`,
        turnId,
        kind,
        title: kind === "turn_failed" ? "Turn 已失败" : "Turn 已完成",
        detail: kind === "turn_failed" ? "provider failed" : null,
        durationMs: 1,
      }],
      type: "event",
    });
    const initialMessageId = "steered-retry-initial-user";
    const firstTurnId = "turn-steered-retry-first";
    const secondTurnId = "turn-steered-retry-second";
    const thirdTurnId = "turn-steered-retry-third";
    localStorage.setItem("kcoder_e2e_thread_history", JSON.stringify({
      schemaVersion: 1,
      summary: {
        schemaVersion: 1,
        id: "thread-1",
        title: "Steered retry grouping",
        createdAtMs: 1,
        updatedAtMs: 10,
        archived: false,
      },
      lastTurn: { turnId: thirdTurnId, state: "completed", error: null },
      todos: [],
      lastUsage: null,
      turns: {
        data: [
          {
            schemaVersion: 1,
            id: thirdTurnId,
            userMessageId: initialMessageId,
            state: "completed",
            error: null,
            startedAtMs: 8,
            completedAtMs: 10,
            durationMs: 2,
            itemsView: "full",
            items: [
              messageItem(thirdTurnId, "steered-retry-third-answer", "assistant", "第三次尝试在引导结束后开始。", 9),
              terminalItem(thirdTurnId, "turn_completed", 10),
            ],
          },
          {
            schemaVersion: 1,
            id: secondTurnId,
            userMessageId: initialMessageId,
            state: "completed",
            error: null,
            startedAtMs: 4,
            completedAtMs: 7,
            durationMs: 3,
            itemsView: "full",
            items: [
              messageItem(secondTurnId, "steered-retry-before", "assistant", "第二次尝试在引导前的回复。", 4),
              messageItem(secondTurnId, "steered-retry-user", "user", "第二次尝试请改用另一种方法。", 5),
              messageItem(secondTurnId, "steered-retry-after", "assistant", "第二次尝试在引导后的回复。", 7),
              terminalItem(secondTurnId, "turn_completed", 7),
            ],
          },
          {
            schemaVersion: 1,
            id: firstTurnId,
            userMessageId: initialMessageId,
            state: "failed",
            error: "provider failed",
            startedAtMs: 1,
            completedAtMs: 3,
            durationMs: 2,
            itemsView: "full",
            items: [
              messageItem(firstTurnId, initialMessageId, "user", "请修复这个问题。", 1),
              messageItem(firstTurnId, "steered-retry-first-answer", "assistant", "第一次尝试的回复。", 2),
              terminalItem(firstTurnId, "turn_failed", 3),
            ],
          },
        ],
        nextCursor: null,
        backwardsCursor: null,
      },
      unscopedItems: [],
    }));
  });
  await page.goto("/");

  const retryGroups = page.locator(".message--retry-group");
  await expect(retryGroups).toHaveCount(2);
  await expect(page.locator(".message-retry-attempt")).toHaveCount(3);
  const listText = await page.locator(".message-list").textContent() ?? "";
  expect(listText.indexOf("第一次尝试的回复。")).toBeLessThan(listText.indexOf("第二次尝试在引导前的回复。"));
  expect(listText.indexOf("第二次尝试在引导前的回复。")).toBeLessThan(
    listText.indexOf("第二次尝试请改用另一种方法。"),
  );
  expect(listText.indexOf("第二次尝试请改用另一种方法。")).toBeLessThan(
    listText.indexOf("第二次尝试在引导后的回复。"),
  );
  expect(listText.indexOf("第二次尝试在引导后的回复。")).toBeLessThan(
    listText.indexOf("第三次尝试在引导结束后开始。"),
  );
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

test("keeps workspace terminal input ordered and preserves the session across tabs", async ({ page }, testInfo) => {
  const startCount = () => page.evaluate(() => (window as unknown as { __ptyStartRequests: unknown[] }).__ptyStartRequests.length);
  if (testInfo.project.name === "narrow") {
    await page.setViewportSize({ width: 381, height: 420 });
  }
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_theme", "dark");
    localStorage.setItem("kcoder_e2e_pty_write_delay_ms", "25");
    localStorage.setItem(
      "kcoder_e2e_pty_output",
      "\u001b[34mPS D:\\code\\Nick\\k-coder>\u001b[0m pnpm tauri dev\r\nReady\r\nPS D:\\code\\Nick\\k-coder> ",
    );
  });
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
  await page.keyboard.press("Enter");
  await page.keyboard.type("pnpm tauri dev");
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __ptyWrites: string[] }).__ptyWrites.join(""))).toBe(
    "pnpm tauri build\rpnpm tauri dev\r",
  );
  const writeMetrics = await page.evaluate(() => (window as unknown as {
    __ptyWriteMetrics: { active: number; maxConcurrent: number };
  }).__ptyWriteMetrics);
  expect(writeMetrics.active).toBe(0);
  expect(writeMetrics.maxConcurrent).toBe(1);
  await page.locator(".terminal-view").screenshot({ path: testInfo.outputPath("terminal-input-stable.png") });

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

test("opens the embedded browser panel and keeps the page across tab switches", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "工作台", exact: true }).click();
  await page.getByRole("tab", { name: "浏览器" }).click();

  await expect(page.locator(".browser-empty")).toBeVisible();

  const address = page.getByRole("textbox", { name: "网址" });
  await address.fill("localhost:1420");
  await address.press("Enter");
  await expect(page.locator(".browser-host iframe")).toHaveAttribute("src", "http://localhost:1420");
  await expect(address).toHaveValue("http://localhost:1420");

  await address.fill("example.com/docs");
  await address.press("Enter");
  await expect(page.locator(".browser-host iframe")).toHaveAttribute("src", "https://example.com/docs");

  await page.getByRole("button", { name: "后退" }).click();
  await expect(page.locator(".browser-host iframe")).toHaveAttribute("src", "http://localhost:1420");
  await expect(address).toHaveValue("http://localhost:1420");

  await page.getByRole("button", { name: "前进" }).click();
  await expect(page.locator(".browser-host iframe")).toHaveAttribute("src", "https://example.com/docs");

  await page.getByRole("tab", { name: "文件", exact: true }).click();
  await expect(page.locator(".browser-host")).toBeHidden();
  await page.getByRole("tab", { name: "浏览器" }).click();
  await expect(page.locator(".browser-host iframe")).toBeVisible();
  await expect(page.locator(".browser-host iframe")).toHaveAttribute("src", "https://example.com/docs");
});


test("rate limit waiting and all child states are visible with only one wait call", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench", exact: true })).toBeVisible();
  await page.evaluate(() => {
    const bridge = window as unknown as { __emitAgentEvent: (event: unknown) => void; __emitTauriEvent: (name: string, event: unknown) => void };
    const child = { schemaVersion: 1, parentAgentId: null, parentThreadId: "thread-1", state: "running", depth: 1,
      workspaceRoot: "D:\\code\\k-coder", capabilities: ["read_file"], tokenBudget: null, tokensUsed: 0,
      timeoutMs: 600000, summary: null, error: null, turnCount: 1, task: "检查文档" };
    bridge.__emitTauriEvent("subagent-event", { ...child, id: "agent-1", threadId: "child-1", label: "项目文档", createdAtMs: 1, updatedAtMs: 10 });
    bridge.__emitTauriEvent("subagent-event", { ...child, id: "agent-2", threadId: "child-2", label: "架构文档", createdAtMs: 2, updatedAtMs: 10, retryAtMs: Date.now() + 60000 });
    const base = { schemaVersion: 7, threadId: "thread-1", turnId: "quota-turn", phase: "executing" };
    bridge.__emitAgentEvent({ ...base, type: "turn_started" });
    const call = { id: "wait-only-task2", name: "wait_agent", arguments: { agentId: "agent-2" }, metadata: {} };
    bridge.__emitAgentEvent({ ...base, type: "tool_queued", call });
    bridge.__emitAgentEvent({ ...base, type: "tool_started", call });
  });
  const summary = page.getByRole("region", { name: "本会话子智能体状态" });
  await expect(summary.locator("li")).toHaveCount(2);
  await expect(summary).toContainText("项目文档运行中");
  await expect(summary).toContainText("架构文档限流等待");
  await expect(page.locator(".message--assistant").last().locator(".turn-timeline-tool")).toHaveCount(1);
  const chip = page.getByRole("button", { name: "查看子智能体 task2", exact: true });
  await expect(chip).toBeVisible();
  const box = await chip.boundingBox();
  expect(box!.width).toBeLessThan(90);
  await page.evaluate(() => {
    const bridge = window as unknown as { __emitAgentEvent: (event: unknown) => void };
    bridge.__emitAgentEvent({ schemaVersion: 7, threadId: "thread-1", turnId: "quota-turn", phase: "exploring", type: "provider_retry_waiting", retryAtMs: Date.now() + 60000 });
  });
  await expect(page.locator(".turn-execution--live > summary > .turn-disclosure-title").last()).toContainText("限流等待");
  await page.evaluate(() => {
    (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({ schemaVersion: 7, threadId: "thread-1", turnId: "quota-turn", phase: "exploring", type: "activity_status_changed", status: "thinking" });
  });
  await expect(page.locator(".turn-execution--live > summary > .turn-disclosure-title").last()).not.toContainText("限流等待");
  await chip.click();
  const drawer = page.getByRole("complementary", { name: "子智能体", exact: true });
  await expect(drawer.locator(".agent-retry-wait")).toContainText("限流等待");
  await page.screenshot({ path: testInfo.outputPath("child-rate-limit.png"), fullPage: true });

  await page.evaluate(() => {
    const bridge = window as unknown as { __emitAgentEvent: (event: unknown) => void; __emitTauriEvent: (name: string, event: unknown) => void };
    bridge.__emitAgentEvent({ schemaVersion: 7, threadId: "thread-1", turnId: "quota-turn", phase: "exploring", type: "activity_status_changed", status: "thinking" });
    bridge.__emitTauriEvent("subagent-event", { schemaVersion: 1, id: "agent-2", parentAgentId: null, parentThreadId: "thread-1", threadId: "child-2", label: "架构文档", task: "检查文档", state: "failed", depth: 1, workspaceRoot: "D:\\code\\k-coder", capabilities: ["read_file"], tokenBudget: null, tokensUsed: 0, timeoutMs: 600000, createdAtMs: 2, updatedAtMs: 20, summary: null, error: "provider returned HTTP 429", turnCount: 1 });
  });
  await expect(page.locator(".turn-execution--live > summary > .turn-disclosure-title").last()).not.toContainText("限流等待");
  await expect(summary).toContainText("架构文档失败");
  await expect(drawer.locator(".agent-retry-wait")).toHaveCount(0);
});


test("shows direct subagent creation in a conversation without messages", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_subagents", "[]");
    localStorage.setItem("kcoder_e2e_thread_detail_by_id", JSON.stringify({ "thread-1": {
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "Phase 6 workbench", createdAtMs: 1, updatedAtMs: 1, archived: false, inProject: true, workspacePath: "D:\\code\\k-coder" },
      messages: [], messageTurnIds: {}, turnUserMessageIds: {}, lastTurn: null,
      toolActivities: [], turnTimeline: [], lastUsage: null, contextUsage: null, approvals: [], userInputs: [], changes: [], todos: [],
    } }));
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Phase 6 workbench", exact: true })).toBeVisible();
  await page.evaluate(() => {
    (window as unknown as { __emitTauriEvent: (name: string, payload: unknown) => void }).__emitTauriEvent("subagent-event", {
      schemaVersion: 1, id: "direct-child", parentAgentId: null, parentThreadId: "thread-1", threadId: "child-thread",
      label: "直接创建的任务", task: "检查文档", state: "running", depth: 1, workspaceRoot: "D:\\code\\k-coder", capabilities: [],
      tokenBudget: null, tokensUsed: 0, timeoutMs: 600000, createdAtMs: 2, updatedAtMs: 2, summary: null, error: null, turnCount: 0,
    });
  });
  const summary = page.getByRole("region", { name: "本会话子智能体状态" });
  await expect(summary).toBeVisible();
  await expect(summary).toContainText("直接创建的任务运行中");
});

test("robot progress follows authoritative nodes across failure retry and reload", async ({ page }) => {
  await page.addInitScript(() => {
    if (localStorage.getItem("kcoder_e2e_workflow_run")) return;
    const nodes = ["requirements-analysis", "interface-architecture-design", "html-prototype", "backend-development", "frontend-development", "comprehensive-testing", "build-release", "code-review-delivery"];
    localStorage.setItem("kcoder_e2e_plan", JSON.stringify({ schemaVersion: 1, threadId: "thread-1", revision: 1, updatedAtMs: 1, steps: nodes.map((id, index) => ({ id, step: `旧计划 ${index + 1}`, status: index === 0 ? "in_progress" : "pending" })) }));
    localStorage.setItem("kcoder_e2e_workflow_run", JSON.stringify({ schemaVersion: 1, definitionVersion: 2, id: "progress-run", threadId: "thread-1", workflowId: "fullstack-delivery", objective: "进度回归", state: "active", currentNodeId: nodes[1], currentNodeIndex: 1, nodeCount: 8, completedNodes: [{ nodeId: nodes[0], summary: "需求已确认", evidence: ["docs"], completedAtMs: 2 }], createdAtMs: 1, updatedAtMs: 2, revision: 2 }));
  });
  await page.goto("/");
  const progress = page.locator(".plan-progress-trigger");
  const control = page.getByLabel("机器人工作流 全栈开发机器人");
  await expect(progress).toHaveCount(1);
  await expect(progress).toContainText("第 2/8 步");
  await expect(control).toContainText("2 / 8");
  await page.evaluate(() => {
    const emit = (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent;
    const base = { schemaVersion: 1, threadId: "thread-1", turnId: "retry-progress", phase: "executing" };
    emit({ ...base, type: "turn_started" });
    emit({ ...base, type: "tool_started", call: { id: "advance", name: "complete_workflow_node", arguments: {}, metadata: {} } });
    const run = JSON.parse(localStorage.getItem("kcoder_e2e_workflow_run")!);
    run.completedNodes.push({ nodeId: run.currentNodeId, summary: "设计已完成", evidence: ["docs/design"], completedAtMs: 3 });
    Object.assign(run, { currentNodeId: "html-prototype", currentNodeIndex: 2, revision: 3, updatedAtMs: 3 });
    localStorage.setItem("kcoder_e2e_workflow_run", JSON.stringify(run));
    emit({ ...base, type: "tool_completed", callId: "advance", name: "complete_workflow_node", result: { success: true, output: "done", metadata: {} } });
  });
  await expect(progress).toHaveCount(1);
  await expect(progress).toContainText("第 3/8 步");
  await expect(control).toContainText("3 / 8");
  await progress.click();
  const details = page.getByRole("dialog", { name: "执行计划详情" });
  await expect(details.locator(".plan-progress-step--completed")).toHaveCount(2);
  await expect(details.locator(".plan-progress-step--in_progress")).toContainText("原型 HTML");
  await expect(details).not.toContainText("旧计划");
  await page.keyboard.press("Escape");
  await page.evaluate(() => (window as unknown as { __emitAgentEvent: (event: unknown) => void }).__emitAgentEvent({ schemaVersion: 1, threadId: "thread-1", turnId: "retry-progress", phase: "responding", type: "turn_failed", message: "provider request failed" }));
  await expect(progress).toContainText("第 3/8 步");
  await page.reload();
  await expect(progress).toContainText("第 3/8 步");
  await expect(control).toContainText("3 / 8");
});

test("ordinary conversation keeps its independent plan progress", async ({ page }) => {
  await page.goto("/");
  const progress = page.locator(".plan-progress-trigger");
  await expect(progress).toHaveCount(1);
  await expect(progress).toContainText("第 2/2 步");
  await expect(progress).toContainText("1 个文件已更新");
  await progress.click();
  const details = page.getByRole("dialog", { name: "执行计划详情" });
  await expect(details.locator(".plan-progress-step--completed")).toContainText("检查工作区");
  await expect(details.locator(".plan-progress-step--in_progress")).toContainText("验证实现");
  await page.reload();
  await expect(progress).toContainText("第 2/2 步");
});
