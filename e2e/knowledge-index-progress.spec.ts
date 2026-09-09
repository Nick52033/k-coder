import { expect, test } from "@playwright/test";

test("knowledge page renders live index progress and metrics", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 820 });

  await page.addInitScript(() => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    const listeners = new Map<string, number>();
    let callbackId = 1;
    const collection = {
      id: "collection-1",
      name: "Docs",
      scope: "workspace",
      scopeKey: "key-1",
      enabled: true,
      sourceCount: 1,
      indexedChunkCount: 0,
      updatedAtMs: 1,
    };
    const source = {
      sourceId: "source-1",
      relativePath: "docs/guide.md",
      sizeBytes: 2048,
      contentHashPrefix: "abcdef123456",
      activeRevisionId: null,
      activeEmbeddingModel: null,
      activeEmbeddingDimension: 0,
      activeEmbeddingEncodingFormat: "float",
      embeddingStatus: "lexical_only",
      state: "indexing",
      chunkCount: 0,
      lastIndexedAtMs: null,
      lastErrorCode: null,
      initialJobId: "job-1",
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
        switch (command) {
          case "plugin:event|listen": {
            const event = args?.event;
            const handler = args?.handler;
            if (typeof event === "string" && typeof handler === "number") {
              listeners.set(event, handler);
            }
            return typeof handler === "number" ? handler : 1;
          }
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
            return [];
          case "list_scheduled_tasks":
            return [];
          case "read_thread":
            return null;
          case "list_threads":
            return [];
          case "read_thread_history":
            return null;
          case "get_plan":
          case "get_goal":
          case "get_workflow_run":
            return null;
          case "read_thread_mailbox":
            return { schemaVersion: 1, threadId: "thread-1", revision: 0, activeTurnId: null, pending: [] };
          case "list_subagents":
            return [];
          case "workspace_state":
            return {
              current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
              recent: [],
            };
          case "list_workspace_directory":
            return [];
          case "git_status":
            return { isRepository: true, branch: "main", upstream: null, ahead: 0, behind: 0, files: [] };
          case "git_branches":
            return { current: "main", branches: ["main"] };
          case "get_knowledge_settings":
            return { enabled: true, autoSearch: false, maxResults: 6, maxChunkTokens: 1500, knowledgeBudgetPercent: 20, semanticEnabled: false, embeddingProvider: "siliconflow", embeddingModel: "BAAI/bge-m3", embeddingDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "get_embedding_settings":
            return { provider: "siliconflow", endpoint: "https://api.siliconflow.cn/v1/embeddings", model: "BAAI/bge-m3", semanticEnabled: false, encodingFormat: "float", batchSize: 16, timeoutMs: 30000, maxVectorScanChunks: 10000, modelMaxInputTokens: 8192, vectorDimension: 0, embeddingConfigured: false, embeddingStatus: "lexical_only" };
          case "list_knowledge_collections":
            return [collection];
          case "list_knowledge_sources":
            return [source];
          case "get_knowledge_metrics":
            return { jobsQueued: 1, jobsCompleted: 2, jobsReused: 1, jobsFailed: 0, jobsCancelled: 0, chunksIndexed: 24, vectorsIndexed: 12, embeddingRequests: 3, embeddingRetries: 1, totalIndexDurationMs: 900, averageIndexDurationMs: 300, lastErrorCode: null, lastCompletedAtMs: 2 };
          default:
            return null;
        }
      },
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };
    (window as unknown as { emitKnowledgeProgress: unknown }).emitKnowledgeProgress = (
      payload: Record<string, unknown>,
    ) => {
      const handler = listeners.get("knowledge-index-progress");
      const callback = handler === undefined ? undefined : callbacks.get(handler);
      callback?.({ event: "knowledge-index-progress", id: handler, payload });
    };
  });

  await page.goto("/");
  await page.waitForTimeout(400);
  const settings = page.getByRole("dialog", { name: "设置" });
  await page.locator('button[aria-label="设置"]:visible').click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: /^知识库/ }).click();
  await expect(settings.getByRole("heading", { name: "运行指标", exact: true })).toBeVisible();
  await expect(settings.getByText("已索引切片")).toBeVisible();

  await page.evaluate(() => {
    const emit = (window as unknown as { emitKnowledgeProgress: (payload: Record<string, unknown>) => void })
      .emitKnowledgeProgress;
    emit({
      schemaVersion: 1,
      jobId: "job-1",
      sourceId: "source-1",
      collectionId: "collection-1",
      state: "running",
      stage: "embedding",
      processedBytes: 1024,
      totalBytes: 2048,
      processedChunks: 6,
      totalChunks: 12,
      percent: 50,
      embeddingRequests: 2,
      retryCount: 1,
      lastHttpStatus: 200,
      errorCode: null,
      elapsedMs: 1200,
      timestampMs: Date.now(),
    });
  });

  await expect(
    settings.getByRole("status").filter({ hasText: "生成向量 · 50%" }),
  ).toBeVisible();
  await expect(settings.getByText("6/12 切片")).toBeVisible();

  await page.setViewportSize({ width: 700, height: 820 });
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth + 1);
});
