import { expect, test } from "@playwright/test";

/// 验证知识库设置页的「检索控制台」：多路召回结果、引用展开、反馈与检索事件。
///
/// 这里的 `__TAURI_INTERNALS__.invoke` mock 是唯一的契约来源：它同时断言了命令名、驼峰参数名
/// 与返回字段名，所以「命令没在 lib.rs 注册」或「前端参数名拼错」都会在这里失败，而不是等到
/// 桌面端点击时才发现。检索与反馈共用宿主生成的 `settings` 伪 Turn，因此 citation 可被反馈绑定。
test("knowledge retrieval console searches, expands a citation and records feedback", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 820 });

  await page.addInitScript(() => {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    (window as unknown as { __knowledgeCalls: typeof calls }).__knowledgeCalls = calls;

    const callbackId = { value: 1 };
    const collection = {
      id: "collection-1",
      name: "Docs",
      scope: "workspace",
      scopeKey: "key-1",
      enabled: true,
      sourceCount: 1,
      indexedChunkCount: 12,
      updatedAtMs: 1,
    };
    const source = {
      sourceId: "source-1",
      relativePath: "docs/schema.md",
      sizeBytes: 2048,
      contentHashPrefix: "abcdef123456",
      activeRevisionId: "rev-1",
      activeEmbeddingModel: null,
      activeEmbeddingDimension: 0,
      activeEmbeddingEncodingFormat: "float",
      embeddingStatus: "lexical_only",
      state: "indexed",
      chunkCount: 12,
      lastIndexedAtMs: 2,
      lastErrorCode: null,
      initialJobId: null,
    };
    const searchResponse = {
      success: true,
      results: [
        {
          citationId: "citation-1",
          title: "Schema 迁移",
          path: "docs/schema.md",
          locator: "L1-4",
          preview: "提升 DATABASE_SCHEMA_VERSION",
          revision: "rev-1",
          score: 0.7123,
          lexicalRank: 1,
          semanticRank: null,
        },
      ],
      metadata: {
        retrievalMode: "lexical_only",
        channels: ["lexical", "path"],
        rewriteCount: 2,
        budgetChars: 30720,
        fallbackCode: "KC_EMBEDDING_NOT_CONFIGURED",
      },
    };
    const retrievalEvent = {
      id: "event-1",
      threadId: "settings",
      turnId: "settings",
      queryHash: "abc123def456",
      retrievalMode: "lexical_only",
      resultCount: 4,
      selectedCitationCount: 1,
      latencyMs: 12,
      createdAtMs: 1_700_000_000_000,
    };
    const activeEntity = {
      id: "entity-1",
      collectionId: "collection-1",
      entityType: "concept",
      name: "MemoryService",
      normalizedName: "memoryservice",
      description: null,
      confidence: 0.5,
      status: "active",
      createdAtMs: 1,
      updatedAtMs: 1,
    };
    const fact = {
      id: "fact-1",
      subjectEntityId: "entity-1",
      predicate: "depends_on",
      objectEntityId: null,
      objectText: "knowledge store",
      sourceChunkId: "chunk-1",
      sourceRevisionId: "rev-1",
      confidence: 0.5,
      validFromMs: null,
      validToMs: null,
      status: "candidate",
      createdAtMs: 1,
      updatedAtMs: 1,
    };
    const factCandidate = {
      fact,
      collectionId: "collection-1",
      subjectName: "MemoryService",
      objectEntityName: null,
      sourcePath: "docs/graph.md",
      locator: "L1-4",
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback: (_callback: (...args: unknown[]) => void) => callbackId.value++,
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
          case "get_knowledge_settings":
            return { enabled: true, autoSearch: false, maxResults: 6, maxChunkTokens: 1500, knowledgeBudgetPercent: 8, semanticEnabled: false, embeddingProvider: "siliconflow", embeddingModel: "BAAI/bge-m3", embeddingDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "get_embedding_settings":
            return { provider: "siliconflow", endpoint: "https://api.siliconflow.cn/v1/embeddings", model: "BAAI/bge-m3", semanticEnabled: false, encodingFormat: "float", batchSize: 16, timeoutMs: 30000, maxVectorScanChunks: 10000, modelMaxInputTokens: 8192, vectorDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "list_knowledge_collections":
            return [collection];
          case "list_knowledge_sources":
            return [source];
          case "get_knowledge_metrics":
            return { jobsQueued: 0, jobsCompleted: 2, jobsReused: 1, jobsFailed: 0, jobsCancelled: 0, chunksIndexed: 24, vectorsIndexed: 0, embeddingRequests: 0, embeddingRetries: 0, totalIndexDurationMs: 900, averageIndexDurationMs: 300, lastErrorCode: null, lastCompletedAtMs: 2 };
          case "search_knowledge":
            return searchResponse;
          case "read_knowledge_citation":
            return {
              citationId: "citation-1",
              path: "docs/schema.md",
              locator: "L1-6",
              text: "# Schema 迁移\n\n第一步：提升 DATABASE_SCHEMA_VERSION。",
              revision: "rev-1",
              isCurrentRevision: true,
            };
          case "record_knowledge_feedback":
            return {
              id: "feedback-1",
              citationId: "citation-1",
              feedbackType: typeof args?.feedbackType === "string" ? args.feedbackType : "",
              createdAtMs: 3,
              chunkId: "chunk-1",
              sourceRevisionId: "rev-1",
            };
          case "list_knowledge_retrieval_events":
            return [retrievalEvent];
          case "list_knowledge_facts":
            return [factCandidate];
          case "list_knowledge_entities":
            return [activeEntity];
          case "review_knowledge_fact":
            return {
              ...fact,
              id: typeof args?.factId === "string" ? args.factId : fact.id,
              status: args?.decision === "accept" ? "active" : "rejected",
            };
          case "query_knowledge_relations":
            return {
              subject: activeEntity,
              relations: [{
                factId: "fact-1",
                subjectEntityId: "entity-1",
                subjectName: "MemoryService",
                predicate: "depends_on",
                objectEntityId: null,
                objectEntityName: null,
                objectText: "knowledge graph",
                confidence: 0.5,
                sourceChunkId: "chunk-1",
                sourceRevisionId: "rev-1",
                sourcePath: "docs/relations.md",
                locator: "L2-3",
                updatedAtMs: 1,
              }],
            };
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
  await settings.getByRole("button", { name: /^知识库/ }).click();

  // 面板本身与挂载时读到的检索事件（验证 list_knowledge_retrieval_events 的契约与字段命名）。
  await expect(settings.getByRole("heading", { name: "检索控制台", exact: true })).toBeVisible();
  await expect(settings.getByText("最多 6 个切片 · 预算 8%")).toBeVisible();
  await expect(settings.getByText("abc123def456")).toBeVisible();
  await expect(settings.getByText(/候选 4 · 返回引用 1 · 12 ms/)).toBeVisible();

  await settings.getByLabel("知识库检索关键词").fill("schema 迁移");
  await settings.getByLabel("模型改写").check();
  await settings.getByRole("button", { name: "检索", exact: true }).click();

  // 结果、融合元数据与六路信号的展示。
  await expect(settings.getByText("Schema 迁移", { exact: true })).toBeVisible();
  await expect(settings.getByText("docs/schema.md · L1-4 · rev rev-1")).toBeVisible();
  await expect(settings.getByText("score 0.712")).toBeVisible();
  await expect(settings.getByText("lexical #1")).toBeVisible();
  await expect(settings.getByText("semantic —")).toBeVisible();
  await expect(settings.getByText("模式 lexical_only")).toBeVisible();
  await expect(settings.getByText("通道 lexical + path")).toBeVisible();
  await expect(settings.getByText("改写 2")).toBeVisible();
  await expect(settings.getByText("预算 30720 字符")).toBeVisible();
  await expect(settings.getByText("降级 KC_EMBEDDING_NOT_CONFIGURED")).toBeVisible();

  const searchCall = await page.evaluate(() => {
    const calls = (window as unknown as { __knowledgeCalls: Array<{ command: string; args: Record<string, unknown> }> })
      .__knowledgeCalls;
    return calls.find((call) => call.command === "search_knowledge") ?? null;
  });
  expect(searchCall?.args).toMatchObject({
    query: "schema 迁移",
    limit: 6,
    threadId: "settings",
    turnId: "settings",
    modelRewrite: true,
  });

  // 引用展开走的是同一个伪 Turn，因此 citation 能被读到而不是 KC_CITATION_FORBIDDEN。
  await settings.getByRole("button", { name: "展开引用" }).click();
  await expect(settings.getByText("第一步：提升 DATABASE_SCHEMA_VERSION。")).toBeVisible();
  await expect(settings.getByText("仍是当前版本")).toBeVisible();
  await expect(settings.getByRole("button", { name: "引用已展开" })).toBeVisible();

  // 反馈：按钮进入选中态，并且请求带上 citation 与宿主给出的 scope 绑定。
  const useful = settings.getByRole("button", { name: "有用", exact: true });
  await expect(useful).toHaveAttribute("aria-pressed", "false");
  await useful.click();
  await expect(useful).toHaveAttribute("aria-pressed", "true");

  const feedbackCall = await page.evaluate(() => {
    const calls = (window as unknown as { __knowledgeCalls: Array<{ command: string; args: Record<string, unknown> }> })
      .__knowledgeCalls;
    return calls.find((call) => call.command === "record_knowledge_feedback") ?? null;
  });
  expect(feedbackCall?.args).toMatchObject({
    citationId: "citation-1",
    feedbackType: "useful",
    threadId: "settings",
    turnId: "settings",
  });

  await page.setViewportSize({ width: 700, height: 820 });
  await page.waitForTimeout(100);
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth + 1);
});

/// 关系审核面板：模型提出的读法只有「通过」才生效，实体类型由审核者从宿主词表里选。
///
/// 这里同时钉住 `list_knowledge_facts` / `list_knowledge_entities` 的挂载期调用、`review_knowledge_fact`
/// 的三个参数以及只读的 `query_knowledge_relations`，因此命令漏注册或参数改名都会失败。
test("knowledge graph panel reviews a fact candidate and queries active relations", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 820 });

  await page.addInitScript(() => {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    (window as unknown as { __knowledgeCalls: typeof calls }).__knowledgeCalls = calls;

    const collection = {
      id: "collection-1",
      name: "Docs",
      scope: "workspace",
      scopeKey: "key-1",
      enabled: true,
      sourceCount: 1,
      indexedChunkCount: 12,
      updatedAtMs: 1,
    };
    const entity = {
      id: "entity-1",
      collectionId: "collection-1",
      entityType: "concept",
      name: "MemoryService",
      normalizedName: "memoryservice",
      description: null,
      confidence: 0.5,
      status: "active",
      createdAtMs: 1,
      updatedAtMs: 1,
    };
    const candidate = {
      fact: {
        id: "fact-1",
        subjectEntityId: "entity-1",
        predicate: "depends_on",
        objectEntityId: null,
        objectText: "knowledge store",
        sourceChunkId: "chunk-1",
        sourceRevisionId: "rev-1",
        confidence: 0.5,
        validFromMs: null,
        validToMs: null,
        status: "candidate",
        createdAtMs: 1,
        updatedAtMs: 1,
      },
      collectionId: "collection-1",
      subjectName: "MemoryService",
      objectEntityName: null,
      sourcePath: "docs/graph.md",
      locator: "L1-4",
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
          case "get_knowledge_settings":
            return { enabled: true, autoSearch: false, maxResults: 6, maxChunkTokens: 1500, knowledgeBudgetPercent: 8, semanticEnabled: false, embeddingProvider: "siliconflow", embeddingModel: "BAAI/bge-m3", embeddingDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "get_embedding_settings":
            return { provider: "siliconflow", endpoint: "https://api.siliconflow.cn/v1/embeddings", model: "BAAI/bge-m3", semanticEnabled: false, encodingFormat: "float", batchSize: 16, timeoutMs: 30000, maxVectorScanChunks: 10000, modelMaxInputTokens: 8192, vectorDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "list_knowledge_collections":
            return [collection];
          case "list_knowledge_sources":
            return [];
          case "get_knowledge_metrics":
            return { jobsQueued: 0, jobsCompleted: 0, jobsReused: 0, jobsFailed: 0, jobsCancelled: 0, chunksIndexed: 0, vectorsIndexed: 0, embeddingRequests: 0, embeddingRetries: 0, totalIndexDurationMs: 0, averageIndexDurationMs: 0, lastErrorCode: null, lastCompletedAtMs: null };
          case "list_knowledge_retrieval_events":
            return [];
          case "list_knowledge_facts":
            return [candidate];
          case "list_knowledge_entities":
            return [entity];
          case "review_knowledge_fact":
            return {
              ...candidate.fact,
              id: typeof args?.factId === "string" ? args.factId : candidate.fact.id,
              status: args?.decision === "accept" ? "active" : "rejected",
            };
          case "query_knowledge_relations":
            return {
              subject: entity,
              relations: [{
                factId: "fact-1",
                subjectEntityId: "entity-1",
                subjectName: "MemoryService",
                predicate: "depends_on",
                objectEntityId: null,
                objectEntityName: null,
                objectText: "knowledge graph",
                confidence: 0.5,
                sourceChunkId: "chunk-1",
                sourceRevisionId: "rev-1",
                sourcePath: "docs/relations.md",
                locator: "L2-3",
                updatedAtMs: 1,
              }],
            };
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
  await settings.getByRole("button", { name: /^知识库/ }).click();

  await expect(settings.getByRole("heading", { name: "关系审核", exact: true })).toBeVisible();
  await expect(settings.getByText("1 个已生效实体 · 1 条待审核")).toBeVisible();
  await expect(settings.getByText("knowledge store")).toBeVisible();
  await expect(settings.getByText("docs/graph.md · L1-4 · rev rev-1")).toBeVisible();

  // 通过时必须把审核者选定的宿主词表类型一起送出去。
  await settings.getByLabel("实体类型").selectOption("module");
  await settings.getByRole("button", { name: "通过" }).click();

  const reviewCall = await page.evaluate(() => {
    const calls = (window as unknown as { __knowledgeCalls: Array<{ command: string; args: Record<string, unknown> }> })
      .__knowledgeCalls;
    return calls.find((call) => call.command === "review_knowledge_fact") ?? null;
  });
  expect(reviewCall?.args).toMatchObject({
    factId: "fact-1",
    decision: "accept",
    entityType: "module",
  });

  // 只读的关系查询。
  await settings.getByLabel("实体名称").fill("MemoryService");
  await settings.getByRole("button", { name: "查询", exact: true }).click();
  await expect(settings.getByText("knowledge graph")).toBeVisible();
  await expect(settings.getByText("docs/relations.md · L2-3 · rev rev-1")).toBeVisible();

  const relationCall = await page.evaluate(() => {
    const calls = (window as unknown as { __knowledgeCalls: Array<{ command: string; args: Record<string, unknown> }> })
      .__knowledgeCalls;
    return calls.find((call) => call.command === "query_knowledge_relations") ?? null;
  });
  expect(relationCall?.args).toMatchObject({ name: "MemoryService", limit: 50 });

  await settings.getByRole("button", { name: "驳回" }).click();
  const rejectCall = await page.evaluate(() => {
    const calls = (window as unknown as { __knowledgeCalls: Array<{ command: string; args: Record<string, unknown> }> })
      .__knowledgeCalls;
    return calls.filter((call) => call.command === "review_knowledge_fact").pop() ?? null;
  });
  expect(rejectCall?.args).toMatchObject({ factId: "fact-1", decision: "reject" });
  expect(rejectCall?.args).not.toHaveProperty("entityType", "module");

  await page.setViewportSize({ width: 700, height: 820 });
  await page.waitForTimeout(100);
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth + 1);
});
