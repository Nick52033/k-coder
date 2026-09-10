(() => {
  const callbacks = /* @__PURE__ */ new Map();
  const tauriEventCallbackIds = /* @__PURE__ */ new Map();
  let callbackId = 1;
  let agentEventCallbackId = null;
  let mailboxEventCallbackId = null;
  const localDate = (offsetDays) => {
    const date = /* @__PURE__ */ new Date();
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
  const openAiProvider = { schemaVersion: 1, id: "openai", kind: "open_ai_compatible", transport: "open_ai_chat_completions", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1", models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128e3, fallback: false }, { id: "gpt-4o", displayName: "GPT-4 Omni", contextWindow: 64e3, fallback: false }], endpoints: [], hasApiKey: true };
  const ziccProvider = { schemaVersion: 1, id: "zicc", kind: "open_ai_compatible", transport: "open_ai_responses", name: "zicc", baseUrl: "https://zicc.example.com/v1", model: "gpt-5.6-terra", models: [{ id: "gpt-5.6-terra", displayName: "gpt-5.6-terra", contextWindow: 128e3, fallback: false }, { id: "gpt-5.5", displayName: "gpt-5.5", contextWindow: 128e3, fallback: false }], endpoints: [], hasApiKey: true };
  const pendingProvider = { schemaVersion: 1, id: "pending", kind: "open_ai_compatible", transport: "anthropic_messages", name: "\u5F85\u914D\u7F6E\u4F9B\u5E94\u5546", baseUrl: "https://pending.example.com/v1", model: "claude-test", models: [{ id: "claude-test", displayName: "Claude Test", contextWindow: 128e3, fallback: false }], endpoints: [], hasApiKey: false };
  const makeBinding = (declaration, kind) => {
    const slash = declaration.indexOf("/");
    const pluginId = slash > 0 ? declaration.slice(0, slash) : null;
    const skillId = slash > 0 ? declaration.slice(slash + 1) : declaration;
    return {
      kind,
      declaration,
      skillId,
      pluginId,
      fallbackSkillId: kind === "plugin_skill" ? skillId : null
    };
  };
  const makeWorkflow = ({ id, name, description, rolePrompt, localSkills, pluginSkills, nodes }) => ({
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
      ...pluginSkills.map((skill) => makeBinding(skill, "plugin_skill"))
    ],
    nodes: nodes.map(([nodeId, title, nodeDescription]) => ({
      id: nodeId,
      title,
      description: nodeDescription,
      localSkillCount: Math.min(localSkills.length, 1),
      pluginSkillCount: Math.min(pluginSkills.length, 1),
      skillDeclarationCount: Math.min(localSkills.length, 1) + Math.min(pluginSkills.length, 1),
      localSkillBindings: localSkills.slice(0, 1).map((skill) => makeBinding(skill, "skill")),
      pluginSkillBindings: pluginSkills.slice(0, 1).map((skill) => makeBinding(skill, "plugin_skill"))
    }))
  });
  const workflowDefinitions = [
    makeWorkflow({
      id: "fullstack-delivery",
      name: "\u5168\u6808\u5F00\u53D1\u673A\u5668\u4EBA",
      description: "\u8986\u76D6\u9700\u6C42\u5206\u6790\u3001\u754C\u9762\u4E0E\u67B6\u6784\u8BBE\u8BA1\u3001\u539F\u578B HTML\u3001\u524D\u540E\u7AEF\u5F00\u53D1\u3001\u5168\u9762\u6D4B\u8BD5\u3001\u6784\u5EFA\u53D1\u5E03\u548C\u4EE3\u7801\u4EA4\u4ED8\u3002",
      rolePrompt: "# \u5168\u6808\u5F00\u53D1\u673A\u5668\u4EBA - \u89D2\u8272\u5B9A\u4E49\n\n\u4F60\u662F\u4E00\u4F4D\u7ECF\u9A8C\u4E30\u5BCC\u7684\u5168\u6808\u5F00\u53D1\u5DE5\u7A0B\u5E08\u3002",
      localSkills: ["brainstorming", "writing-plans", "executing-plans", "test-driven-development", "requesting-code-review", "verification-before-completion", "dispatching-parallel-agents", "taste-skill", "awesome-design-md"],
      pluginSkills: ["browser/control-in-app-browser", "superpowers/brainstorming", "superpowers/dispatching-parallel-agents", "superpowers/executing-plans", "superpowers/finishing-a-development-branch", "superpowers/receiving-code-review", "superpowers/requesting-code-review", "superpowers/subagent-driven-development", "superpowers/systematic-debugging", "superpowers/test-driven-development", "superpowers/using-git-worktrees", "superpowers/verification-before-completion", "superpowers/writing-plans", "documents/documents"],
      nodes: [
        ["requirements-analysis", "\u9700\u6C42\u7406\u89E3\u4E0E\u5206\u6790", "\u6F84\u6E05\u76EE\u6807\u3001\u7EA6\u675F\u3001\u73B0\u72B6\u4E0E\u53EF\u9A8C\u8BC1\u7684\u4EA4\u4ED8\u8303\u56F4\u3002"],
        ["interface-architecture-design", "\u754C\u9762\u4E0E\u67B6\u6784\u8BBE\u8BA1", "\u786E\u5B9A\u754C\u9762\u4F53\u9A8C\u3001\u6A21\u5757\u804C\u8D23\u3001\u516C\u5171\u5951\u7EA6\u548C\u5B89\u5168\u8FB9\u754C\u3002"],
        ["html-prototype", "\u539F\u578B HTML", "\u4EE5\u53EF\u68C0\u67E5\u7684 HTML \u539F\u578B\u9A8C\u8BC1\u5173\u952E\u5E03\u5C40\u548C\u4EA4\u4E92\u3002"],
        ["backend-development", "\u540E\u7AEF\u5F00\u53D1", "\u5B9E\u73B0\u540E\u7AEF\u9886\u57DF\u903B\u8F91\u3001\u8FB9\u754C\u5951\u7EA6\u4E0E\u5B89\u5168\u6D4B\u8BD5\u3002"],
        ["frontend-development", "\u524D\u7AEF\u5F00\u53D1", "\u5B9E\u73B0\u4E0E\u73B0\u6709\u8BBE\u8BA1\u4E00\u81F4\u7684\u5B8C\u6574\u754C\u9762\u548C\u4EA4\u4E92\u72B6\u6001\u3002"],
        ["comprehensive-testing", "\u5168\u9762\u6D4B\u8BD5", "\u8986\u76D6\u529F\u80FD\u3001\u8FB9\u754C\u3001\u5931\u8D25\u3001\u6062\u590D\u548C\u684C\u9762\u5DE5\u4F5C\u6D41\u3002"],
        ["build-release", "\u6784\u5EFA\u4E0E\u53D1\u5E03", "\u5B8C\u6210\u89C4\u5B9A\u6784\u5EFA\u95E8\u69DB\u5E76\u51C6\u5907\u53EF\u5BA1\u8BA1\u7684\u53D1\u5E03\u7ED3\u679C\u3002"],
        ["code-review-delivery", "\u4EE3\u7801\u5BA1\u67E5\u4E0E\u4EA4\u4ED8", "\u590D\u67E5\u6B63\u786E\u6027\u3001\u5B89\u5168\u6027\u3001\u517C\u5BB9\u6027\u548C\u6700\u7EC8\u4EA4\u4ED8\u8BF4\u660E\u3002"]
      ]
    }),
    makeWorkflow({
      id: "quality-assurance",
      name: "\u8F6F\u4EF6\u6D4B\u8BD5\u673A\u5668\u4EBA",
      description: "\u8986\u76D6\u6D4B\u8BD5\u7B56\u7565\u3001\u7528\u4F8B\u8BBE\u8BA1\u3001\u5355\u5143\u6D4B\u8BD5\u3001\u96C6\u6210\u4E0E API\u3001E2E/UI\u3001\u6027\u80FD\u3001\u5B89\u5168\u548C\u6D4B\u8BD5\u62A5\u544A\u3002",
      rolePrompt: "# \u8F6F\u4EF6\u6D4B\u8BD5\u673A\u5668\u4EBA - \u89D2\u8272\u5B9A\u4E49\n\n\u4F60\u662F\u4E00\u4F4D\u7ECF\u9A8C\u4E30\u5BCC\u7684\u9AD8\u7EA7 QA \u6D4B\u8BD5\u5DE5\u7A0B\u5E08\u3002",
      localSkills: ["test-strategy-planning", "test-case-design", "api-testing", "performance-testing", "security-testing", "test-report-generation", "webapp-testing"],
      pluginSkills: ["superpowers/test-driven-development", "superpowers/systematic-debugging", "superpowers/verification-before-completion", "superpowers/writing-plans", "superpowers/executing-plans", "superpowers/dispatching-parallel-agents", "superpowers/subagent-driven-development", "browser/control-in-app-browser", "documents/documents"],
      nodes: [
        ["test-strategy", "\u6D4B\u8BD5\u7B56\u7565\u5236\u5B9A", "\u786E\u5B9A\u6D4B\u8BD5\u76EE\u6807\u3001\u98CE\u9669\u5206\u5C42\u3001\u8303\u56F4\u3001\u73AF\u5883\u548C\u9A8C\u6536\u53E3\u5F84\u3002"],
        ["test-case-design", "\u6D4B\u8BD5\u7528\u4F8B\u8BBE\u8BA1", "\u8BBE\u8BA1\u6B63\u5E38\u3001\u8FB9\u754C\u3001\u5931\u8D25\u3001\u6062\u590D\u548C\u5B89\u5168\u8DEF\u5F84\u7684\u7528\u4F8B\u3002"],
        ["unit-testing", "\u5355\u5143\u6D4B\u8BD5", "\u5B9E\u73B0\u5E76\u6267\u884C\u805A\u7126\u9886\u57DF\u903B\u8F91\u548C\u516C\u5171\u5951\u7EA6\u7684\u5355\u5143\u6D4B\u8BD5\u3002"],
        ["integration-api-testing", "\u96C6\u6210/API \u6D4B\u8BD5", "\u9A8C\u8BC1\u6A21\u5757\u534F\u4F5C\u3001\u7C7B\u578B\u5316\u8FB9\u754C\u3001API \u5931\u8D25\u4E0E\u6062\u590D\u8BED\u4E49\u3002"],
        ["e2e-ui-testing", "E2E/UI \u6D4B\u8BD5", "\u9A8C\u8BC1\u771F\u5B9E\u7528\u6237\u5DE5\u4F5C\u6D41\u3001\u54CD\u5E94\u5F0F\u5E03\u5C40\u548C\u4EA4\u4E92\u7EC8\u6001\u3002"],
        ["nonfunctional-testing", "\u975E\u529F\u80FD\u6D4B\u8BD5", "\u8BC4\u4F30\u6027\u80FD\u3001\u5B89\u5168\u3001\u8D44\u6E90\u8FB9\u754C\u548C\u6545\u969C\u97E7\u6027\u3002"],
        ["test-report-delivery", "\u6D4B\u8BD5\u62A5\u544A\u4E0E\u4EA4\u4ED8", "\u6C47\u603B\u8986\u76D6\u3001\u7ED3\u679C\u3001\u7F3A\u9677\u3001\u9650\u5236\u4E0E\u6B8B\u4F59\u98CE\u9669\u3002"]
      ]
    }),
    makeWorkflow({
      id: "requirements-design",
      name: "\u9700\u6C42\u8BBE\u8BA1\u673A\u5668\u4EBA",
      description: "\u901A\u8FC7\u9700\u6C42\u91C7\u96C6\u3001\u8FB9\u754C\u5212\u5B9A\u3001\u7528\u6237\u6545\u4E8B\u3001\u4EA4\u4E92\u6D41\u7A0B\u3001PRD \u5BA1\u67E5\u3001\u6587\u6863\u8F93\u51FA\u548C\u9489\u9489\u53D1\u5E03\u5F62\u6210\u53EF\u4EA4\u4ED8\u9700\u6C42\u6587\u6863\u3002",
      rolePrompt: "# \u9700\u6C42\u8BBE\u8BA1\u673A\u5668\u4EBA - \u89D2\u8272\u5B9A\u4E49\n\n\u4F60\u662F\u4E00\u4F4D\u8D44\u6DF1\u4EA7\u54C1\u7ECF\u7406\u548C\u9700\u6C42\u5206\u6790\u5E08\u3002",
      localSkills: ["requirements-intake", "prd-story-modeler", "prd-delivery-review", "create-plan", "dingtalk-document"],
      pluginSkills: ["superpowers/brainstorming", "superpowers/verification-before-completion", "superpowers/writing-plans", "documents/documents"],
      nodes: [
        ["requirements-intake", "\u9700\u6C42\u91C7\u96C6\u4E0E\u7406\u89E3", "\u4E0E\u7528\u6237\u6C9F\u901A\u9700\u6C42\uFF0C\u901A\u8FC7\u6E10\u8FDB\u5F0F\u95EE\u7B54\u63D0\u70BC\u6838\u5FC3\u9700\u6C42\u3002"],
        ["business-boundary", "\u4E1A\u52A1\u8FB9\u754C\u5212\u5B9A", "\u660E\u786E\u505A\u4EC0\u4E48\u3001\u4E0D\u505A\u4EC0\u4E48\u3001\u7EA6\u675F\u6761\u4EF6\u548C\u5B9E\u73B0\u8DEF\u5F84\u3002"],
        ["user-story-modeling", "\u7528\u6237\u6545\u4E8B\u4E0E\u529F\u80FD\u5EFA\u6A21", "\u5C06\u9700\u6C42\u62C6\u89E3\u4E3A\u5E26\u53EF\u9A8C\u8BC1\u9A8C\u6536\u6807\u51C6\u7684\u7528\u6237\u6545\u4E8B\u548C\u529F\u80FD\u9700\u6C42\u3002"],
        ["interaction-flow-design", "\u4EA4\u4E92\u6D41\u7A0B\u8BBE\u8BA1", "\u8BBE\u8BA1\u6838\u5FC3\u4EA4\u4E92\u6D41\u7A0B\u3001\u9875\u9762\u8DF3\u8F6C\u548C\u72B6\u6001\u673A\u3002"],
        ["prd-review", "PRD \u6574\u5408\u4E0E\u5BA1\u67E5", "\u6C47\u603B\u9636\u6BB5\u4EA7\u7269\u5E76\u8FDB\u884C\u8986\u76D6\u6027\u3001\u4E00\u81F4\u6027\u3001\u53EF\u6267\u884C\u6027\u548C\u98CE\u9669\u5BA1\u67E5\u3002"],
        ["document-formatting", "\u6587\u6863\u683C\u5F0F\u5316\u8F93\u51FA", "\u8F93\u51FA\u6700\u7EC8 Markdown \u6587\u6863\u548C\u53EF\u9009\u7ED3\u6784\u5316\u6587\u6863\u3002"],
        ["dingtalk-publishing", "\u53D1\u5E03\u5230\u9489\u9489\u77E5\u8BC6\u5E93", "\u901A\u8FC7\u5DF2\u914D\u7F6E\u5E76\u6388\u6743\u7684\u9489\u9489\u80FD\u529B\u53D1\u5E03\u6700\u7EC8 PRD\u3002"]
      ]
    })
  ];
  const workflowReadiness = (definition) => ({
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
      blocker: null
    })),
    nodes: definition.nodes.map((node) => ({
      nodeId: node.id,
      ready: true,
      declarationCount: node.skillDeclarationCount,
      uniqueBodyCount: node.skillDeclarationCount,
      totalBodyBytes: node.skillDeclarationCount * 100,
      bindings: [],
      blockers: []
    })),
    blockers: []
  });
  let workflowRun = JSON.parse(localStorage.getItem("kcoder_e2e_workflow_run") ?? "null");
  let providerCatalog = { schemaVersion: 1, activeProviderId: "openai", providers: [openAiProvider, ziccProvider, pendingProvider] };
  let approvalMode = "ask";
  let reasoningEffort = "medium";
  let workspaceState = {
    current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
    recent: [
      { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
      { id: "project-2", name: "paypro-platform-be", path: "D:\\code\\paypro-platform-be", trusted: true, lastOpenedAtMs: 1 }
    ]
  };
  const runTurnCalls = [];
  let startedTurnCount = 0;
  const activeTurnIds = /* @__PURE__ */ new Map();
  const mailboxByThread = /* @__PURE__ */ new Map();
  const mailboxRevisionByThread = /* @__PURE__ */ new Map();
  function bumpMailboxRevision(threadId) {
    const revision = (mailboxRevisionByThread.get(threadId) ?? 0) + 1;
    mailboxRevisionByThread.set(threadId, revision);
    return revision;
  }
  function emitMailboxChanged(threadId, revision) {
    if (mailboxEventCallbackId === null) return;
    callbacks.get(mailboxEventCallbackId)?.({
      event: "thread-mailbox-changed",
      id: 1,
      payload: { schemaVersion: 1, threadId, revision }
    });
  }
  function restoreMailboxFixture() {
    const restoredMailbox = JSON.parse(localStorage.getItem("kcoder_e2e_mailbox") ?? "null");
    if (!restoredMailbox) return;
    mailboxByThread.set(restoredMailbox.threadId, restoredMailbox.pending);
    mailboxRevisionByThread.set(restoredMailbox.threadId, 1);
    if (restoredMailbox.activeTurnId) {
      activeTurnIds.set(restoredMailbox.threadId, restoredMailbox.activeTurnId);
    }
    localStorage.removeItem("kcoder_e2e_mailbox");
  }
  restoreMailboxFixture();
  const invocationArgs = {};
  const ptyStartRequests = [];
  const ptyWrites = [];
  const ptyWriteMetrics = { active: 0, maxConcurrent: 0 };
  const extensionOverview = {
    schemaVersion: 1,
    configPaths: [
      "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\extensions.json",
      "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\mcp.json",
      "D:\\code\\k-coder\\.k-coder\\extensions.json",
      "D:\\code\\k-coder\\.k-coder\\mcp.json"
    ],
    instructions: [{ path: "D:\\code\\k-coder\\AGENTS.md", scope: "project", priority: 200, bytes: 120 }],
    skills: [
      { name: "workspace-review", description: "Built-in workspace review", path: "D:\\apps\\k-coder\\resources\\skills\\workspace-review\\SKILL.md", scope: "builtin", risk: "read", category: "quality_review", triggers: ["workspace review"], enabled: true, managedByRobot: false },
      { name: "review", description: "Review code safely", path: "D:\\code\\k-coder\\.k-coder\\skills\\review\\SKILL.md", scope: "project", risk: "read", category: "quality_review", triggers: ["review"], enabled: true, managedByRobot: false },
      { name: "requirements-intake", description: "Robot requirements intake", path: "D:\\apps\\k-coder\\resources\\skills\\robot-pack\\requirements-intake\\SKILL.md", scope: "builtin", risk: "read", category: "requirements_planning", triggers: ["requirements"], enabled: true, managedByRobot: true }
    ],
    mcpServers: [{ id: "local", transport: "stdio", enabled: true, state: "ready", toolCount: 2, credentials: [], error: null }],
    hooks: [{ id: "guard", phase: "before", tool: "mcp__local__*", enabled: true }],
    audit: [{ timestampMs: 2, event: "extensions_ready", kind: "runtime", id: "all", success: true, detail: "extensions loaded" }],
    error: null
  };
  let userRules = {
    schemaVersion: 1,
    path: "C:\\Users\\demo\\AppData\\Local\\k-coder\\runtime-data\\user-rules.json",
    rules: [{
      id: "11111111-1111-4111-8111-111111111111",
      title: "\u65B9\u6CD5\u6CE8\u91CA",
      content: "\u516C\u5171\u65B9\u6CD5\u9700\u8981\u6CE8\u91CA\uFF0C\u5B9E\u4F53\u7F3A\u5C11\u6CE8\u91CA\u65F6\u9700\u8981\u63D0\u9192\u3002",
      createdAtMs: 1785e9,
      updatedAtMs: 1785e9
    }],
    error: null
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
            timeoutMs: 3e4,
            command: "npx",
            args: ["-y", "@modelcontextprotocol/server-filesystem", "D:\\code\\k-coder"],
            secret_env: {}
          }
        }
      }, null, 2)}
`,
      error: null
    },
    project: {
      scope: "project",
      path: "D:\\code\\k-coder\\.k-coder\\mcp.json",
      exists: false,
      content: `${JSON.stringify({ mcpServers: {} }, null, 2)}
`,
      error: null
    },
    overview: extensionOverview
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
        error: null
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
        error: null
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
        error: null
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
        error: "MCP runtime failed to start"
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
        error: "plugin manifest JSON is invalid"
      }
    ],
    error: null
  };
  const responses = {
    runtime_status: { ready: true, phase: "advanced-agent", version: "0.10.0", uptimeSeconds: 12, capabilities: ["skills", "mcp-stdio", "tool-hooks", "persistent-plans", "budgeted-goals"] },
    get_approval_mode: "ask",
    get_reasoning_effort: "medium",
    test_provider_connection: { connected: true, latencyMs: 42, usage: null },
    get_plan: { schemaVersion: 1, threadId: "thread-1", revision: 2, updatedAtMs: 3, steps: [
      { id: "step-1", step: "\u68C0\u67E5\u5DE5\u4F5C\u533A", status: "completed", detail: "\u5DF2\u8BFB\u53D6\u5173\u952E\u6587\u4EF6" },
      { id: "step-2", step: "\u9A8C\u8BC1\u5B9E\u73B0", status: "in_progress", detail: "\u6B63\u5728\u8FD0\u884C\u6D4B\u8BD5" }
    ] },
    get_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "\u5B8C\u6210 Phase 9 \u9AD8\u7EA7\u667A\u80FD\u4F53\u80FD\u529B", state: "active", tokenBudget: null, tokensUsed: 24e3, timeBudgetMs: 36e5, elapsedMs: 42e4, reason: null, createdAtMs: 2, updatedAtMs: 3, revision: 2 },
    transition_goal: { schemaVersion: 1, id: "goal-1", threadId: "thread-1", objective: "\u5B8C\u6210 Phase 9 \u9AD8\u7EA7\u667A\u80FD\u4F53\u80FD\u529B", state: "paused", tokenBudget: null, tokensUsed: 24e3, timeBudgetMs: 36e5, elapsedMs: 42e4, reason: null, createdAtMs: 2, updatedAtMs: 4, revision: 3 },
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
        { date: localDate(-1), providerCalls: 4, inputTokens: 42e3, outputTokens: 800, totalTokens: 42800 },
        { date: localDate(0), providerCalls: 10, inputTokens: 148300, outputTokens: 2500, totalTokens: 150800 }
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
        estimatedCostUsd: null
      }]
    },
    run_regression_evaluation: { total: 3, passed: 3, passRate: 1, failures: [] },
    cancel_turn: true,
    create_thread: secondThread,
    recognize_image: { text: "hidden OCR fixture", lineCount: 1, durationMs: 12 },
    list_threads: [thread],
    read_thread: { schemaVersion: 1, summary: thread, messages: [
      { schemaVersion: 1, id: "message-user", role: "user", content: [{ type: "text", text: "\u68C0\u67E5\u5DE5\u4F5C\u533A" }], createdAtMs: 1 },
      { schemaVersion: 1, id: "message-assistant", role: "assistant", content: [{ type: "text", text: "\u68C0\u67E5\u5B8C\u6210\u3002" }], createdAtMs: 2 }
    ], messageTurnIds: { "message-assistant": "turn-1" }, lastTurn: null, toolActivities: [
      { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1e3, completedAtMs: 1200, durationMs: 200 },
      { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 },
      { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { command: "pnpm build", cwd: ".", timeoutMs: 12e4 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530, shell: "powershell" } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 }
    ], turnTimeline: [
      { type: "event", itemId: "provider-context-1", turnId: "turn-1", kind: "provider_context", title: "\u5DF2\u4FDD\u7559\u6A21\u578B\u4E0A\u4E0B\u6587", detail: "openai_responses \xB7 reasoning \xB7 rs_fixture" },
      { type: "event", itemId: "usage-1", turnId: "turn-1", kind: "usage", title: "\u6A21\u578B\u8C03\u7528 1 \u7528\u91CF", detail: "\u8F93\u5165 1200 \xB7 \u8F93\u51FA 80 \xB7 \u603B\u8BA1 1280 tokens" },
      { type: "text", id: "progress-1", turnId: "turn-1", text: "\u6211\u5148\u68C0\u67E5\u76F8\u5173\u6587\u4EF6\u5E76\u4FEE\u6539\u5B9E\u73B0\u3002" },
      { type: "tool", activity: { turnId: "turn-1", call: { id: "call-edit", name: "apply_patch", arguments: { patch: "*** Begin Patch\n*** Update File: src/App.css\n@@\n-old\n+new\n*** End Patch" }, metadata: {} }, state: "completed", result: { success: true, output: "applied", metadata: {} }, startedAtMs: 1e3, completedAtMs: 1200, durationMs: 200 } },
      { type: "tool", activity: { turnId: "turn-1", call: { id: "call-read", name: "read_file", arguments: { path: "src/stores/workbenchStore.ts", startLine: 42, lineCount: 1 }, metadata: {} }, state: "completed", result: { success: true, output: "export const fixture = true;\n", metadata: { path: "src/stores/workbenchStore.ts", offset: 920, bytesReturned: 29, totalBytes: 4096, startLine: 42, endLine: 42, linesReturned: 1, totalLines: 200, truncated: true } }, startedAtMs: 1210, completedAtMs: 1224, durationMs: 14 } },
      { type: "text", id: "progress-2", turnId: "turn-1", text: "\u4FEE\u6539\u5B8C\u6210\uFF0C\u63A5\u7740\u8FD0\u884C\u9A8C\u8BC1\u3002" },
      { type: "tool", activity: { turnId: "turn-1", call: { id: "call-test", name: "run_command", arguments: { command: "pnpm build", cwd: ".", timeoutMs: 12e4 }, metadata: {} }, state: "completed", result: { success: true, output: "tests passed", metadata: { durationMs: 1530, shell: "powershell" } }, startedAtMs: 1300, completedAtMs: 2830, durationMs: 1530 } },
      { type: "text", id: "message-assistant", turnId: "turn-1", text: "\u68C0\u67E5\u5B8C\u6210\u3002" },
      { type: "event", itemId: "turn-completed-turn-1", turnId: "turn-1", kind: "turn_completed", title: "Turn \u5DF2\u5B8C\u6210", detail: null, durationMs: 1830 }
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
        unifiedDiff: "--- a/src/App.css\n+++ b/src/App.css\n@@ -1 +1 @@\n-old\n+new\n"
      }]
    }] },
    workspace_state: { current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 }, recent: [] },
    list_workspace_directory: [
      { name: "src", path: "src", isDirectory: true, size: null, modifiedAtMs: 2 },
      { name: "README.md", path: "README.md", isDirectory: false, size: 120, modifiedAtMs: 2 }
    ],
    search_workspace_files: [
      { name: "App.tsx", path: "src/App.tsx", isDirectory: false, size: 240, modifiedAtMs: 2 },
      { name: "README.md", path: "README.md", isDirectory: false, size: 120, modifiedAtMs: 2 }
    ],
    preview_workspace_file: { path: "README.md", name: "README.md", language: "markdown", content: "# k-Coder", dataUrl: null, size: 9, truncated: false, editable: true, contentHash: "hash-readme" },
    save_workspace_file: { path: "README.md", name: "README.md", language: "markdown", content: "# k-Coder\n\nEdited", dataUrl: null, size: 17, truncated: false, editable: true, contentHash: "hash-edited" },
    git_status: { isRepository: true, branch: "main", upstream: "origin/main", ahead: 0, behind: 0, files: [{ path: "src/App.tsx", indexStatus: " ", worktreeStatus: "M" }] },
    git_branches: { current: "main", branches: ["main", "feature/workbench"] },
    extension_overview: extensionOverview,
    list_subagents: [{
      schemaVersion: 1,
      id: "agent-1",
      parentAgentId: null,
      parentThreadId: "thread-1",
      threadId: "thread-agent-1",
      label: "\u68C0\u67E5\u540E\u7AEF",
      task: "\u5206\u6790\u540E\u7AEF\u63A5\u53E3",
      state: "completed",
      depth: 1,
      workspaceRoot: "D:\\code\\k-coder",
      capabilities: ["list_directory", "read_file"],
      tokenBudget: null,
      tokensUsed: 420,
      timeoutMs: 6e5,
      createdAtMs: 2,
      updatedAtMs: 3,
      summary: "\u540E\u7AEF\u68C0\u67E5\u5B8C\u6210",
      error: null
    }],
    create_subagent: {
      schemaVersion: 1,
      id: "agent-2",
      parentAgentId: null,
      parentThreadId: "thread-1",
      threadId: "thread-agent-2",
      label: "\u68C0\u67E5\u6D4B\u8BD5",
      task: "\u68C0\u67E5\u6D4B\u8BD5",
      state: "running",
      depth: 1,
      workspaceRoot: "D:\\code\\k-coder",
      capabilities: ["list_directory", "read_file"],
      tokenBudget: null,
      tokensUsed: 0,
      timeoutMs: 6e5,
      createdAtMs: 4,
      updatedAtMs: 4,
      summary: null,
      error: null
    },
    "plugin:event|listen": 1
  };
  Object.assign(window, {
    __TAURI_INTERNALS__: {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      transformCallback: (callback) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id) => callbacks.delete(id),
      invoke: async (command, args) => {
        window.__invoked.push(command);
        invocationArgs[command] = args ?? {};
        if (command === "plugin:dialog|open") {
          const selected = localStorage.getItem("kcoder_e2e_attachment_dialog_paths");
          return selected ? JSON.parse(selected) : null;
        }
        if (command === "plugin:fs|stat") {
          const raw = localStorage.getItem("kcoder_e2e_attachment_file_bytes");
          const size = raw ? JSON.parse(raw).length : 0;
          return {
            isFile: true,
            isDirectory: false,
            isSymlink: false,
            size,
            mtime: null,
            atime: null,
            birthtime: null,
            readonly: true
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
                path: forcedPath
              }
            };
          }
          if (forcedRecentProjects) {
            workspaceState = {
              ...workspaceState,
              recent: JSON.parse(forcedRecentProjects)
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
            lastOpenedAtMs: Date.now()
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
            workspacePath: inProject ? workspaceState.current.path : null
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
          const request = args?.request;
          if (!workflowRun || workflowRun.threadId !== request?.threadId || workflowRun.id !== request?.runId) {
            throw new Error("workflow run was not found for this thread");
          }
          workflowRun = {
            ...workflowRun,
            state: "cancelled",
            updatedAtMs: Date.now(),
            revision: workflowRun.revision + 1
          };
          return workflowRun;
        }
        if (command === "turn_start") {
          runTurnCalls.push(args ?? null);
          startedTurnCount += 1;
          const request = args?.request;
          const threadId = String(request?.threadId ?? "thread-1");
          const turnId = `turn-start-${startedTurnCount}`;
          const queued = activeTurnIds.has(threadId);
          const workflowId = typeof args?.workflowId === "string" ? args.workflowId : null;
          const attachments = args?.attachments ?? [];
          if (queued) {
            mailboxByThread.set(threadId, [
              ...mailboxByThread.get(threadId) ?? [],
              {
                schemaVersion: 1,
                turnId,
                threadId,
                kind: "message",
                input: String(request?.input ?? ""),
                agentMode: request?.agentMode ?? null,
                workflowId,
                attachments
              }
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
                revision: 1
              };
            }
            activeTurnIds.set(threadId, turnId);
            const input = String(request?.input ?? "").trim();
            const content = input ? [{ type: "text", text: input }] : [{ type: "context", text: "\u8BF7\u5206\u6790\u7528\u6237\u63D0\u4F9B\u7684\u56FE\u7247\u3002" }];
            for (const attachment of attachments) {
              if (attachment.ocrText?.trim()) {
                content.push({
                  type: "context",
                  text: `

[\u56FE\u7247\u6587\u5B57\u8BC6\u522B: ${attachment.name}]
${attachment.ocrText.trim()}`
                });
              }
              content.push({
                type: "image",
                name: attachment.name,
                dataUrl: attachment.dataUrl
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
                  createdAtMs: Date.now()
                }
              }
            });
          }
          return {
            schemaVersion: 1,
            threadId,
            turnId,
            state: queued ? "queued" : "streaming"
          };
        }
        if (command === "turn_retry") {
          startedTurnCount += 1;
          const threadId = String(args?.threadId ?? "thread-1");
          const turnId = `turn-retry-${startedTurnCount}`;
          const queued = activeTurnIds.has(threadId);
          if (queued) {
            mailboxByThread.set(threadId, [
              ...mailboxByThread.get(threadId) ?? [],
              {
                schemaVersion: 1,
                turnId,
                threadId,
                kind: "retry",
                input: "",
                agentMode: null,
                attachments: []
              }
            ]);
          } else {
            activeTurnIds.set(threadId, turnId);
          }
          return {
            schemaVersion: 1,
            threadId,
            turnId,
            state: queued ? "queued" : "streaming"
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
            pending: mailboxByThread.get(threadId) ?? []
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
          const request = args?.request;
          return { schemaVersion: 1, threadId: request.threadId, turnId: request.expectedTurnId };
        }
        if (command === "turn_steer_queued") {
          const request = args?.request;
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
            pending.filter((item) => item.turnId !== request.queuedTurnId)
          );
          bumpMailboxRevision(request.threadId);
          return {
            schemaVersion: 1,
            threadId: request.threadId,
            turnId: request.expectedTurnId
          };
        }
        if (command === "turn_interrupt") {
          if (localStorage.getItem("kcoder_e2e_hold_cancel") === "true") {
            return new Promise(() => void 0);
          }
          return null;
        }
        if (command === "cancel_turn") {
          if (localStorage.getItem("kcoder_e2e_hold_cancel") === "true") {
            return new Promise(() => void 0);
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
        if ((command === "get_plan" || command === "get_goal") && String(args?.threadId ?? "") === localStorage.getItem("kcoder_e2e_empty_thread_id")) {
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
            const detail = JSON.parse(detailsByThread)[String(args?.threadId ?? "")];
            if (detail) return detail;
          }
          const recovered = localStorage.getItem("kcoder_e2e_thread_detail");
          if (recovered) return JSON.parse(recovered);
          const forcedThreadWorkspacePath = localStorage.getItem("kcoder_e2e_thread_workspace_path");
          if (forcedThreadWorkspacePath) {
            const detail = responses.read_thread;
            return {
              ...detail,
              summary: { ...detail.summary, workspacePath: forcedThreadWorkspacePath }
            };
          }
          const threadId = String(args?.threadId ?? "");
          const activeTurnId = activeTurnIds.get(threadId);
          if (activeTurnId) {
            return {
              ...responses.read_thread,
              lastTurn: { turnId: activeTurnId, state: "streaming", error: null }
            };
          }
        }
        if (command === "get_provider_config") {
          return providerCatalog.providers.find((provider) => provider.id === providerCatalog.activeProviderId) ?? null;
        }
        if (command === "save_provider_config") {
          window.__lastProviderRequest = args?.request;
          const request = args?.request;
          const providerId = request.id;
          const existing = providerCatalog.providers.find((provider) => provider.id === providerId);
          const { apiKey, activate, ...publicConfig } = request;
          const saved = {
            schemaVersion: 1,
            ...publicConfig,
            hasApiKey: Boolean(apiKey) || existing?.hasApiKey || false
          };
          providerCatalog = {
            ...providerCatalog,
            activeProviderId: activate ? providerId : providerCatalog.activeProviderId,
            providers: existing ? providerCatalog.providers.map((provider) => provider.id === providerId ? saved : provider) : [...providerCatalog.providers, saved]
          };
          return saved;
        }
        if (command === "activate_provider") {
          const providerId = args?.providerId;
          window.__lastActivatedProvider = providerId;
          providerCatalog = { ...providerCatalog, activeProviderId: providerId };
          return providerCatalog;
        }
        if (command === "delete_provider") {
          const providerId = args?.providerId;
          const providers = providerCatalog.providers.filter((provider) => provider.id !== providerId);
          providerCatalog = {
            ...providerCatalog,
            providers,
            activeProviderId: providerCatalog.activeProviderId === providerId ? providers[0]?.id ?? null : providerCatalog.activeProviderId
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
            plugins: pluginOverview.plugins.map((plugin) => plugin.id === pluginId ? {
              ...plugin,
              enabled: Boolean(args?.enabled),
              state: args?.enabled ? "loaded" : "disabled",
              error: null
            } : plugin)
          };
          return pluginOverview;
        }
        if (command === "delete_plugin") {
          const pluginId = String(args?.pluginId ?? "");
          pluginOverview = {
            ...pluginOverview,
            plugins: pluginOverview.plugins.filter((plugin) => plugin.id !== pluginId)
          };
          return pluginOverview;
        }
        if (command === "user_rules") return userRules;
        if (command === "save_user_rule") {
          const request = args?.request ?? {};
          const timestamp = 178500006e4;
          if (request.id) {
            userRules = {
              ...userRules,
              rules: userRules.rules.map((rule) => rule.id === request.id ? {
                ...rule,
                title: String(request.title ?? ""),
                content: String(request.content ?? ""),
                updatedAtMs: timestamp
              } : rule)
            };
          } else {
            userRules = {
              ...userRules,
              rules: [...userRules.rules, {
                id: "22222222-2222-4222-8222-222222222222",
                title: String(request.title ?? ""),
                content: String(request.content ?? ""),
                createdAtMs: timestamp,
                updatedAtMs: timestamp
              }]
            };
          }
          return userRules;
        }
        if (command === "delete_user_rule") {
          userRules = {
            ...userRules,
            rules: userRules.rules.filter((rule) => rule.id !== String(args?.id ?? ""))
          };
          return userRules;
        }
        if (command === "save_mcp_config") {
          const configScope = args?.scope === "project" ? "project" : "global";
          const content = String(args?.content ?? "");
          if (configScope === "project") {
            mcpConfig = {
              ...mcpConfig,
              project: { ...mcpConfig.project, exists: true, content, error: null }
            };
          } else {
            mcpConfig = {
              ...mcpConfig,
              global: { ...mcpConfig.global, exists: true, content, error: null }
            };
          }
          return mcpConfig;
        }
        if (command === "set_extension_enabled" || command === "save_mcp_secret" || command === "delete_mcp_secret") return mcpConfig.overview;
        if (command === "get_approval_mode") return approvalMode;
        if (command === "set_approval_mode") {
          approvalMode = args?.mode;
          window.__lastApprovalMode = approvalMode;
          return approvalMode;
        }
        if (command === "get_reasoning_effort") return reasoningEffort;
        if (command === "set_reasoning_effort") {
          reasoningEffort = args?.effort;
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
            outputTruncated: false
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
            outputTruncated: false
          };
        }
        if (command === "wait_pty") {
          const exited = localStorage.getItem("kcoder_e2e_pty_state") === "exited";
          if (!exited) await new Promise(() => void 0);
          return {
            id: String(args?.sessionId ?? "pty-1"),
            state: { state: "exited", code: 0 },
            startedAtMs: 1,
            finishedAtMs: 2,
            rows: 24,
            cols: 80,
            nextCursor: 1,
            oldestCursor: 0,
            outputTruncated: false
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
            return void 0;
          } finally {
            ptyWriteMetrics.active -= 1;
          }
        }
        if (command === "resize_pty" || command === "close_pty") return void 0;
        if ((command === "open_workspace_file" || command === "reveal_workspace_file") && localStorage.getItem("kcoder_e2e_external_open_error")) {
          throw new Error(localStorage.getItem("kcoder_e2e_external_open_error") ?? "external open failed");
        }
        if (command === "extract_local_document") {
          const delayMs = Number(localStorage.getItem("kcoder_e2e_attachment_extract_delay_ms") ?? 0);
          if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
          const name = String(args?.name ?? "attachment.txt");
          const dataUrl = String(args?.dataUrl ?? "");
          const encoded = dataUrl.split(",", 2)[1] ?? "";
          const bytes = Uint8Array.from(atob(encoded), (value) => value.charCodeAt(0));
          const spreadsheetContent = /\.(xlsx|xls|xlsm|xlsb)$/i.test(name) ? "[\u5DE5\u4F5C\u8868: \u9884\u7B97]\n\u9879\u76EE	\u91D1\u989D\n\u4F4F\u5BBF	128.5\n" : null;
          return {
            path: `attachment://fixture/${name}`,
            name,
            kind: "document",
            content: spreadsheetContent ?? new TextDecoder().decode(bytes),
            size: bytes.byteLength,
            truncated: false
          };
        }
        return responses[command] ?? null;
      }
    },
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => void 0 },
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
    __emitTauriEvent: (event, payload) => {
      const handlerId = tauriEventCallbackIds.get(event);
      if (handlerId === void 0) throw new Error(`${event} listener is not ready`);
      callbacks.get(handlerId)?.({ event, id: 1, payload });
    },
    __emitAgentEvent: (event) => {
      const agentEvent = event;
      if (agentEvent.threadId && agentEvent.turnId && agentEvent.type === "turn_started") {
        activeTurnIds.set(agentEvent.threadId, agentEvent.turnId);
        const pending = mailboxByThread.get(agentEvent.threadId) ?? [];
        const next = pending.filter((item) => item.turnId !== agentEvent.turnId);
        if (next.length !== pending.length) {
          mailboxByThread.set(agentEvent.threadId, next);
          const revision = bumpMailboxRevision(agentEvent.threadId);
          emitMailboxChanged(agentEvent.threadId, revision);
        }
      } else if (agentEvent.threadId && activeTurnIds.get(agentEvent.threadId) === agentEvent.turnId && ["turn_completed", "turn_failed", "turn_cancelled"].includes(agentEvent.type ?? "")) {
        activeTurnIds.delete(agentEvent.threadId);
      }
      if (agentEventCallbackId === null) throw new Error("agent-event listener is not ready");
      callbacks.get(agentEventCallbackId)?.({ event: "agent-event", id: 1, payload: event });
    }
  });
})();
