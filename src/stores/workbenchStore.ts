import { create } from "zustand";
import {
  archiveThread as archiveThreadCommand,
  cancelTurn,
  createThread as createThreadCommand,
  errorMessage,
  getProviderCatalog,
  getApprovalMode,
  getReasoningEffort,
  activateProvider as activateProviderCommand,
  deleteProvider as deleteProviderCommand,
  listThreads,
  readThread,
  searchThreads,
  renameThread,
  deleteThread,
  resolveApproval,
  resolveUserInput,
  retryTurn,
  runTurn,
  saveProviderConfig,
  setApprovalMode as setApprovalModeCommand,
  setReasoningEffort as setReasoningEffortCommand,
  undoChange,
  createGoal,
  getGoal,
  getPlan,
  transitionGoal,
} from "../api/runtime";
import type {
  AgentEvent,
  AgentActivityStatus,
  ApprovalMode,
  ReasoningEffort,
  ApprovalRequest,
  ApprovalSnapshot,
  ApprovalResolution,
  ChatMessage,
  ChangeSet,
  ConversationMessage,
  ImageAttachment,
  ProviderCatalogView,
  ProviderConfigView,
  SaveProviderConfigRequest,
  ThreadSummary,
  ToolActivity,
  ToolResult,
  ToolOutputStream,
  TimelineEventKind,
  TurnTimelineItem,
  TokenUsage,
  TurnSnapshot,
  GoalState,
  GoalView,
  PlanView,
  UserInputRequest,
  UserInputResolution,
  TodoItem,
} from "../types/runtime";

interface QueuedMessage {
  id: string;
  messageId: string;
  threadId: string;
  input: string;
  attachments: ImageAttachment[];
  agentMode?: string;
  status: "pending" | "processing" | "completed" | "failed";
  turnId?: string;
  error?: string;
}

interface WorkbenchState {
  threads: ThreadSummary[];
  activeThreadId: string | null;
  messages: ConversationMessage[];
  lastTurn: TurnSnapshot | null;
  activeTurnId: string | null;
  activeTurnThreadId: string | null;
  activeTurns: Record<string, string>;
  messageQueue: QueuedMessage[];
  usage: TokenUsage | null;
  turnTimeline: TurnTimelineItem[];
  turnUserMessageIds: Record<string, string>;
  activityStatus: { turnId: string; status: AgentActivityStatus } | null;
  pendingApproval: ApprovalRequest | null;
  pendingApprovals: ApprovalRequest[];
  pendingUserInput: UserInputRequest | null;
  pendingUserInputs: UserInputRequest[];
  changes: ChangeSet[];
  providerConfig: ProviderConfigView | null;
  providerConfigs: ProviderConfigView[];
  activeProviderId: string | null;
  approvalMode: ApprovalMode;
  reasoningEffort: ReasoningEffort;
  plan: PlanView | null;
  goal: GoalView | null;
  todos: Map<string, TodoItem[]>; // key: threadId
  loading: boolean;
  error: string;
  initialize: () => Promise<void>;
  reloadThreads: () => Promise<void>;
  searchThreadHistory: (query: string) => Promise<void>;
  renameConversation: (threadId: string, title: string) => Promise<void>;
  deleteConversation: (threadId: string) => Promise<void>;
  createThread: () => Promise<string>;
  selectThread: (threadId: string) => Promise<void>;
  archiveActiveThread: () => Promise<void>;
  sendMessage: (input: string, attachments?: ImageAttachment[], agentMode?: string) => Promise<void>;
  processQueue: () => Promise<void>;
  sendQueuedMessageNow: (messageId: string) => Promise<void>;
  removeQueuedMessage: (messageId: string) => void;
  clearQueue: () => void;
  retryLastTurn: () => Promise<void>;
  stopTurn: (threadId?: string) => Promise<void>;
  loadProviderCatalog: () => Promise<void>;
  saveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
  activateProvider: (providerId: string) => Promise<boolean>;
  deleteProvider: (providerId: string) => Promise<boolean>;
  setApprovalMode: (mode: ApprovalMode) => Promise<boolean>;
  setReasoningEffort: (effort: ReasoningEffort) => Promise<boolean>;
  createActiveGoal: (objective: string, tokenBudget: number | null, timeBudgetMs: number) => Promise<boolean>;
  transitionActiveGoal: (state: GoalState, reason?: string) => Promise<boolean>;
  resolvePendingApproval: (resolution: ApprovalResolution) => Promise<boolean>;
  resolvePendingUserInput: (resolution: UserInputResolution) => Promise<boolean>;
  undoAppliedChange: (changeId: string) => Promise<boolean>;
  handleAgentEvent: (event: AgentEvent) => void;
  clearError: () => void;
  forceResetState: () => Promise<void>;
}

function toConversationMessage(message: ChatMessage, turnId?: string): ConversationMessage {
  return {
    id: message.id,
    role: message.role,
    text: message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join(""),
    attachments: message.content
      .filter((block) => block.type === "image")
      .map((block) => ({ name: block.name, dataUrl: block.dataUrl })),
    createdAtMs: message.createdAtMs,
    turnId,
  };
}

const MAX_LIVE_TOOL_OUTPUT_CHARS = 64 * 1024;
const MAX_REASONING_SUMMARY_CHARS = 64 * 1024;

function appendToolOutput(
  activity: ToolActivity,
  turnId: string,
  callId: string,
  stream: ToolOutputStream,
  cursor: number,
  delta: string,
) {
  if (activity.turnId !== turnId || activity.call.id !== callId) return activity;
  if (activity.outputChunks?.some((chunk) => chunk.stream === stream && chunk.cursor === cursor)) {
    return activity;
  }
  const chunks = [...(activity.outputChunks ?? [])];
  chunks.push({ stream, cursor, text: delta });
  let total = chunks.reduce((sum, chunk) => sum + chunk.text.length, 0);
  while (chunks.length > 1 && total > MAX_LIVE_TOOL_OUTPUT_CHARS) {
    total -= chunks.shift()!.text.length;
  }
  if (chunks[0] && total > MAX_LIVE_TOOL_OUTPUT_CHARS) {
    chunks[0] = { ...chunks[0], text: chunks[0].text.slice(-MAX_LIVE_TOOL_OUTPUT_CHARS) };
  }
  return { ...activity, outputChunks: chunks };
}

function resultDurationMs(result: ToolResult): number | undefined {
  const value = result.metadata?.durationMs;
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}

function statusForTurnState(state: TurnSnapshot["state"]): AgentActivityStatus | null {
  if (state === "running_tool") return "running_tool";
  if (state === "awaiting_approval") return "awaiting_approval";
  if (state === "streaming" || state === "queued") return "thinking";
  return null;
}

function enqueueApproval(queue: ApprovalRequest[], request: ApprovalRequest) {
  return queue.some((approval) => approval.id === request.id) ? queue : [...queue, request];
}

function removeApproval(queue: ApprovalRequest[], requestId: string) {
  return queue.filter((approval) => approval.id !== requestId);
}

function approvalQueueState(queue: ApprovalRequest[]) {
  return { pendingApprovals: queue, pendingApproval: queue[0] ?? null };
}

function enqueueUserInput(queue: UserInputRequest[], request: UserInputRequest) {
  return queue.some((input) => input.id === request.id) ? queue : [...queue, request];
}

function removeUserInput(queue: UserInputRequest[], requestId: string) {
  return queue.filter((input) => input.id !== requestId);
}

function userInputQueueState(queue: UserInputRequest[]) {
  return { pendingUserInputs: queue, pendingUserInput: queue[0] ?? null };
}

function appendTimelineEvent(
  timeline: TurnTimelineItem[],
  itemId: string,
  turnId: string,
  kind: TimelineEventKind,
  title: string,
  detail: string | null = null,
  durationMs?: number,
) {
  if (timeline.some((item) => item.type === "event" && item.itemId === itemId)) return timeline;
  return [...timeline, { type: "event" as const, itemId, turnId, kind, title, detail, durationMs }];
}

function insertApprovalRequestEvent(
  timeline: TurnTimelineItem[],
  itemId: string,
  turnId: string,
  kind: TimelineEventKind,
  title: string,
  detail: string | null,
  toolCallId: string,
) {
  if (timeline.some((item) => item.type === "event" && item.itemId === itemId)) return timeline;
  const event = { type: "event" as const, itemId, turnId, kind, title, detail };
  const toolIndex = timeline.findIndex((item) => item.type === "tool"
    && item.activity.turnId === turnId
    && item.activity.call.id === toolCallId);
  if (toolIndex < 0) return [...timeline, event];
  return [...timeline.slice(0, toolIndex), event, ...timeline.slice(toolIndex)];
}

function insertApprovalResolutionEvent(
  timeline: TurnTimelineItem[],
  itemId: string,
  turnId: string,
  kind: TimelineEventKind,
  title: string,
  detail: string | null,
  requestId: string,
) {
  if (timeline.some((item) => item.type === "event" && item.itemId === itemId)) return timeline;
  const event = { type: "event" as const, itemId, turnId, kind, title, detail };
  const requestIndex = timeline.findIndex((item) => item.type === "event" && item.itemId === `approval-requested-${requestId}`);
  if (requestIndex >= 0) return [...timeline.slice(0, requestIndex + 1), event, ...timeline.slice(requestIndex + 1)];
  return [...timeline, event];
}

function normalizeApprovalTimeline(timeline: TurnTimelineItem[], approvals: ApprovalSnapshot[]) {
  let normalized = timeline;
  for (const approval of approvals) {
    normalized = moveTimelineItemBeforeTool(
      normalized,
      `approval-requested-${approval.request.id}`,
      approval.request.turnId,
      approval.request.toolCallId,
    );
    normalized = moveTimelineItemAfterRequest(
      normalized,
      `approval-resolved-${approval.request.id}`,
      `approval-requested-${approval.request.id}`,
    );
  }
  return normalized;
}

function finishRunningTools(
  timeline: TurnTimelineItem[],
  turnId: string,
  terminalState: "failed" | "cancelled",
  completedAtMs: number,
) {
  return timeline.map((item) => item.type === "tool"
    && item.activity.turnId === turnId
    && (item.activity.state === "pending" || item.activity.state === "running")
    ? {
        ...item,
        activity: {
          ...item.activity,
          state: terminalState,
          completedAtMs,
          durationMs: item.activity.startedAtMs
            ? Math.max(0, completedAtMs - item.activity.startedAtMs)
            : undefined,
        },
      }
    : item);
}

function moveTimelineItemBeforeTool(timeline: TurnTimelineItem[], itemId: string, turnId: string, toolCallId: string) {
  const itemIndex = timeline.findIndex((item) => item.type === "event" && item.itemId === itemId);
  if (itemIndex < 0) return timeline;
  const item = timeline[itemIndex];
  const withoutItem = [...timeline.slice(0, itemIndex), ...timeline.slice(itemIndex + 1)];
  const toolIndex = withoutItem.findIndex((entry) => entry.type === "tool"
    && entry.activity.turnId === turnId
    && entry.activity.call.id === toolCallId);
  if (toolIndex < 0) return timeline;
  return [...withoutItem.slice(0, toolIndex), item, ...withoutItem.slice(toolIndex)];
}

function moveTimelineItemAfterRequest(timeline: TurnTimelineItem[], itemId: string, requestItemId: string) {
  const itemIndex = timeline.findIndex((item) => item.type === "event" && item.itemId === itemId);
  if (itemIndex < 0) return timeline;
  const item = timeline[itemIndex];
  const withoutItem = [...timeline.slice(0, itemIndex), ...timeline.slice(itemIndex + 1)];
  const requestIndex = withoutItem.findIndex((entry) => entry.type === "event" && entry.itemId === requestItemId);
  if (requestIndex < 0) return timeline;
  return [...withoutItem.slice(0, requestIndex + 1), item, ...withoutItem.slice(requestIndex + 1)];
}

let initializationPromise: Promise<void> | null = null;
let hydrationSequence = 0;
const hydrationBuffers = new Map<string, { token: number; events: AgentEvent[] }>();
const processingQueueThreads = new Set<string>();
const cancellationRequestedTurns = new Map<string, string>();

function withoutActiveTurn(activeTurns: Record<string, string>, threadId: string) {
  if (!(threadId in activeTurns)) return activeTurns;
  const next = { ...activeTurns };
  delete next[threadId];
  return next;
}

const LEGACY_PROVIDER_STORAGE_KEY = "k-coder-providers";

function providerSignature(provider: Partial<ProviderConfigView>) {
  return [provider.name, provider.baseUrl, provider.transport, provider.model].join("\u0000");
}

function providerCatalogState(catalog: ProviderCatalogView) {
  return {
    providerConfigs: catalog.providers,
    activeProviderId: catalog.activeProviderId,
    providerConfig: catalog.providers.find((provider) => provider.id === catalog.activeProviderId) ?? null,
  };
}

async function migrateLegacyProviderCatalog(catalog: ProviderCatalogView) {
  const raw = localStorage.getItem(LEGACY_PROVIDER_STORAGE_KEY);
  if (!raw) return catalog;

  try {
    const legacyProviders = JSON.parse(raw) as Array<Partial<ProviderConfigView> & { isDefault?: boolean }>;
    if (!Array.isArray(legacyProviders)) return catalog;
    const knownIds = new Set(catalog.providers.map((provider) => provider.id));
    const knownSignatures = new Set(catalog.providers.map(providerSignature));
    let migrationComplete = true;
    for (const legacy of legacyProviders) {
      const signature = providerSignature(legacy);
      if (!legacy.id || knownIds.has(legacy.id) || knownSignatures.has(signature) || !legacy.name || !legacy.baseUrl || !legacy.model) continue;
      try {
        await saveProviderConfig({
          id: legacy.id,
          kind: legacy.kind ?? "open_ai_compatible",
          transport: legacy.transport ?? "open_ai_chat_completions",
          name: legacy.name,
          baseUrl: legacy.baseUrl,
          model: legacy.model,
          models: legacy.models ?? [],
          endpoints: legacy.endpoints ?? [],
          activate: false,
        });
        knownIds.add(legacy.id);
        knownSignatures.add(signature);
      } catch {
        migrationComplete = false;
      }
    }
    if (migrationComplete) localStorage.removeItem(LEGACY_PROVIDER_STORAGE_KEY);
    return await getProviderCatalog();
  } catch {
    return catalog;
  }
}

export const useWorkbenchStore = create<WorkbenchState>((set, get) => ({
  threads: [],
  activeThreadId: null,
  messages: [],
  lastTurn: null,
  activeTurnId: null,
  activeTurnThreadId: null,
  activeTurns: {},
  messageQueue: [],
  usage: null,
  turnTimeline: [],
  turnUserMessageIds: {},
  activityStatus: null,
  pendingApproval: null,
  pendingApprovals: [],
  pendingUserInput: null,
  pendingUserInputs: [],
  changes: [],
  providerConfig: null,
  providerConfigs: [],
  activeProviderId: null,
  approvalMode: "ask",
  reasoningEffort: "medium",
  plan: null,
  goal: null,
  todos: new Map(),
  loading: true,
  error: "",

  initialize: () => {
    if (initializationPromise) return initializationPromise;
    initializationPromise = (async () => {
      set({ loading: true, error: "" });
      try {
        const [, , approvalMode, reasoningEffort] = await Promise.all([
          get().reloadThreads(),
          get().loadProviderCatalog(),
          getApprovalMode(),
          getReasoningEffort(),
        ]);
        set({ approvalMode, reasoningEffort });
        let threadId = get().activeThreadId ?? get().threads[0]?.id ?? null;
        if (!threadId) {
          const thread = await createThreadCommand();
          set({ threads: [thread], activeThreadId: thread.id });
          threadId = thread.id;
        }
        await get().selectThread(threadId);
      } catch (error) {
        set({ error: errorMessage(error) });
      } finally {
        set({ loading: false });
        initializationPromise = null;
      }
    })();
    return initializationPromise;
  },

  reloadThreads: async () => {
    const threads = await listThreads();
    set({ threads });
  },

  searchThreadHistory: async (query) => {
    try { set({ threads: await searchThreads(query), error: "" }); }
    catch (error) { set({ error: errorMessage(error) }); }
  },

  renameConversation: async (threadId, title) => {
    try {
      const updated = await renameThread(threadId, title);
      set((state) => ({ threads: state.threads.map((thread) => thread.id === threadId ? updated : thread), error: "" }));
    } catch (error) { set({ error: errorMessage(error) }); }
  },

  deleteConversation: async (threadId) => {
    try {
      await deleteThread(threadId);
      const threads = await listThreads();
      set({ threads });
      if (get().activeThreadId === threadId) {
        if (threads[0]) await get().selectThread(threads[0].id); else await get().createThread();
      }
    } catch (error) { set({ error: errorMessage(error) }); }
  },

  createThread: async () => {
    try {
      const thread = await createThreadCommand();
      set((state) => ({
        threads: [thread, ...state.threads],
        activeThreadId: thread.id,
        messages: [],
        lastTurn: null,
        activeTurnId: null,
        activeTurnThreadId: null,
        usage: null,
        turnTimeline: [],
        turnUserMessageIds: {},
        activityStatus: null,
        pendingApproval: null,
        pendingApprovals: [],
        pendingUserInput: null,
        pendingUserInputs: [],
        changes: [],
        plan: null,
        goal: null,
        error: "",
      }));
      return thread.id;
    } catch (error) {
      set({ error: errorMessage(error) });
      throw error;
    }
  },

  selectThread: async (threadId) => {
    const token = ++hydrationSequence;
    hydrationBuffers.set(threadId, { token, events: [] });
    set({
      activeThreadId: threadId,
      loading: true,
      error: "",
      usage: null,
      activityStatus: null,
      pendingApproval: null,
      pendingApprovals: [],
      pendingUserInput: null,
      pendingUserInputs: [],
    });
    try {
      const [detail, plan, goal] = await Promise.all([
        readThread(threadId),
        getPlan(threadId),
        getGoal(threadId),
      ]);
      if (get().activeThreadId !== threadId) return;
      const hydration = hydrationBuffers.get(threadId);
      if (!hydration || hydration.token !== token) return;
      const restoredActiveTurnId = detail.lastTurn
        && ["queued", "streaming", "running_tool", "awaiting_approval"].includes(detail.lastTurn.state)
        ? detail.lastTurn.turnId
        : null;
      const activeTurns = restoredActiveTurnId
        ? { ...get().activeTurns, [threadId]: restoredActiveTurnId }
        : withoutActiveTurn(get().activeTurns, threadId);

      const pendingApprovals = detail.approvals
        .filter((approval) => !approval.resolution)
        .map((approval) => approval.request);
      const pendingUserInputs = (detail.userInputs ?? [])
        .filter((input) => !input.resolution)
        .map((input) => input.request);
      const todos = new Map(get().todos);
      todos.set(threadId, detail.todos ?? []);
      hydrationBuffers.delete(threadId);
      const restoredTimeline = detail.turnTimeline?.length
        ? normalizeApprovalTimeline(detail.turnTimeline, detail.approvals)
        : detail.toolActivities.map((activity) => ({ type: "tool" as const, activity }));
      const terminalTimeline = detail.lastTurn?.state === "cancelled"
        ? finishRunningTools(restoredTimeline, detail.lastTurn.turnId, "cancelled", detail.summary.updatedAtMs)
        : detail.lastTurn?.state === "failed" || detail.lastTurn?.state === "completed"
          ? finishRunningTools(restoredTimeline, detail.lastTurn.turnId, "failed", detail.summary.updatedAtMs)
          : restoredTimeline;
      set({
        messages: detail.messages.map((message) =>
          toConversationMessage(message, detail.messageTurnIds?.[message.id]),
        ),
        lastTurn: detail.lastTurn,
        turnTimeline: terminalTimeline,
        turnUserMessageIds: detail.turnUserMessageIds ?? {},
        activityStatus: restoredActiveTurnId
          ? { turnId: detail.lastTurn!.turnId, status: statusForTurnState(detail.lastTurn!.state) ?? "thinking" }
          : null,
        ...approvalQueueState(pendingApprovals),
        ...userInputQueueState(pendingUserInputs),
        changes: detail.changes,
        todos,
        usage: detail.lastUsage ?? null,
        plan,
        goal,
        activeTurns,
        activeTurnId: restoredActiveTurnId,
        activeTurnThreadId: restoredActiveTurnId ? threadId : null,
      });
      for (const event of hydration.events) get().handleAgentEvent(event);
    } catch (error) {
      set({ error: errorMessage(error) });
    } finally {
      if (hydrationBuffers.get(threadId)?.token === token) hydrationBuffers.delete(threadId);
      if (get().activeThreadId === threadId) set({ loading: false });
    }
  },

  archiveActiveThread: async () => {
    const threadId = get().activeThreadId;
    if (!threadId) return;
    try {
      await archiveThreadCommand(threadId);
      const threads = await listThreads();
      if (threads.length === 0) {
        await get().createThread();
        return;
      }
      set({ threads });
      await get().selectThread(threads[0].id);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  sendMessage: async (input, attachments = [], agentMode) => {
    const { activeThreadId: threadId, activeTurns } = get();
    const text = input.trim();
    if (!threadId || (!text && attachments.length === 0)) return;
    const shouldQueueOnly = Boolean(activeTurns[threadId] || processingQueueThreads.has(threadId));

    // 创建队列项
    const queuedMessage: QueuedMessage = {
      id: `queue-${crypto.randomUUID()}`,
      messageId: `pending-${crypto.randomUUID()}`,
      threadId,
      input: text,
      attachments,
      agentMode,
      status: "pending",
    };

    // 添加到队列
    set((state) => ({
      messageQueue: [...state.messageQueue, queuedMessage],
      error: "",
    }));

    if (shouldQueueOnly) return;

    void get().processQueue();
  },

  processQueue: async () => {
    let nextMessage: QueuedMessage | undefined;
    try {
      const { messageQueue, activeTurns } = get();
      nextMessage = messageQueue.find((message) => message.status === "pending"
        && !activeTurns[message.threadId]
        && !processingQueueThreads.has(message.threadId));
      if (!nextMessage) return;
      const queuedMessage = nextMessage;
      processingQueueThreads.add(queuedMessage.threadId);

      console.log("[Queue] 开始处理消息:", queuedMessage.id);
      set((state) => ({
        messageQueue: state.messageQueue.map(msg =>
          msg.id === queuedMessage.id ? { ...msg, status: "processing" as const } : msg
        ),
        messages: state.activeThreadId !== queuedMessage.threadId
          ? state.messages
          : state.messages.some((message) => message.id === queuedMessage.messageId)
          ? state.messages
          : [
              ...state.messages,
              {
                id: queuedMessage.messageId,
                role: "user" as const,
                text: queuedMessage.input,
                attachments: queuedMessage.attachments.map(({ name, dataUrl }) => ({ name, dataUrl })),
                createdAtMs: Date.now(),
              },
            ],
      }));

      const outcome = await runTurn(
        queuedMessage.threadId,
        queuedMessage.input,
        queuedMessage.attachments,
        queuedMessage.agentMode
      );

      console.log("[Queue] Turn 完成:", outcome.turnId);

      // 标记为完成并清理
      set((state) => ({
        messageQueue: state.messageQueue.map(msg =>
          msg.id === queuedMessage.id
            ? { ...msg, status: "completed" as const, turnId: outcome.turnId }
            : msg
        ).filter(
          // 立即清理已完成的消息，只保留最近3条
          (msg, idx, arr) => msg.status !== "completed" || idx >= arr.length - 3
        ),
        activeTurns: state.activeTurns[queuedMessage.threadId] === outcome.turnId
          ? withoutActiveTurn(state.activeTurns, queuedMessage.threadId)
          : state.activeTurns,
      }));
      
      const outcomeError = outcome.error;

      // 重新加载线程，但不要重新设置 activeTurnId
      await get().reloadThreads();
      if (get().activeThreadId === queuedMessage.threadId) {
        await get().selectThread(queuedMessage.threadId);
      }

      if (outcomeError) {
        if (get().activeThreadId === queuedMessage.threadId) set({ error: outcomeError });
        // 如果有错误，标记为失败
        set((state) => ({
          messageQueue: state.messageQueue.map(msg =>
            msg.id === queuedMessage.id
              ? { ...msg, status: "failed" as const, error: outcomeError ?? undefined }
              : msg
          ),
        }));
      }

    } catch (error) {
      console.error("[Queue] 处理消息失败:", error);

      if (!nextMessage) {
        set({ error: errorMessage(error) });
        return;
      }
      const failedMessage = nextMessage;
      const errorMsg = errorMessage(error);

      // 标记为失败
      set((state) => ({
        messageQueue: state.messageQueue.map(msg =>
          msg.id === failedMessage.id
            ? { ...msg, status: "failed" as const, error: errorMsg }
            : msg
        ),
        activeTurns: withoutActiveTurn(state.activeTurns, failedMessage.threadId),
        activeTurnId: state.activeThreadId === failedMessage.threadId ? null : state.activeTurnId,
        activeTurnThreadId: state.activeThreadId === failedMessage.threadId ? null : state.activeTurnThreadId,
      }));

      if (get().activeThreadId === failedMessage.threadId) {
        await get().selectThread(failedMessage.threadId);
      }

      // selectThread 会清除 error，所以在之后重新设置
      if (get().activeThreadId === failedMessage.threadId) set({ error: errorMsg });
    } finally {
      if (nextMessage) {
        processingQueueThreads.delete(nextMessage.threadId);
        setTimeout(() => get().processQueue(), 0);
      }
    }
  },

  sendQueuedMessageNow: async (messageId) => {
    const queuedMessage = get().messageQueue.find((message) =>
      message.id === messageId && message.status === "pending"
    );
    if (!queuedMessage) return;

    set((state) => {
      const target = state.messageQueue.find((message) => message.id === messageId);
      if (!target) return state;
      const messageQueue = state.messageQueue.filter((message) => message.id !== messageId);
      const firstPendingIndex = messageQueue.findIndex((message) =>
        message.threadId === target.threadId && message.status === "pending"
      );
      messageQueue.splice(firstPendingIndex < 0 ? messageQueue.length : firstPendingIndex, 0, target);
      return { messageQueue };
    });

    const activeTurnId = get().activeTurns[queuedMessage.threadId];
    if (activeTurnId) {
      await get().stopTurn(queuedMessage.threadId);
      return;
    }
    void get().processQueue();
  },

  removeQueuedMessage: (messageId) => {
    set((state) => ({
      messageQueue: state.messageQueue.filter((message) =>
        message.id !== messageId || message.status !== "pending"
      ),
    }));
  },

  clearQueue: () => {
    set((state) => ({
      messageQueue: state.messageQueue.filter((message) =>
        message.threadId !== state.activeThreadId || message.status !== "pending"
      ),
    }));
  },

  retryLastTurn: async () => {
    const threadId = get().activeThreadId;
    if (!threadId || get().activeTurns[threadId]) return;
    set({ error: "", usage: null });
    try {
      const outcome = await retryTurn(threadId);
      set((state) => ({
        activeTurns: state.activeTurns[threadId] === outcome.turnId
          ? withoutActiveTurn(state.activeTurns, threadId)
          : state.activeTurns,
      }));
      await get().reloadThreads();
      if (get().activeThreadId === threadId) {
        await get().selectThread(threadId);
        if (outcome.error) set({ error: outcome.error });
      }
    } catch (error) {
      // 确保错误发生时清除 activeTurnId，防止界面卡住
      set((state) => ({
        error: state.activeThreadId === threadId ? errorMessage(error) : state.error,
        activeTurns: withoutActiveTurn(state.activeTurns, threadId),
        activeTurnId: state.activeThreadId === threadId ? null : state.activeTurnId,
        activeTurnThreadId: state.activeThreadId === threadId ? null : state.activeTurnThreadId,
      }));
    }
  },

  stopTurn: async (targetThreadId) => {
    const threadId = targetThreadId ?? get().activeThreadId;
    const turnId = threadId ? get().activeTurns[threadId] : null;
    if (!threadId || !turnId || cancellationRequestedTurns.get(threadId) === turnId) return;
    cancellationRequestedTurns.set(threadId, turnId);
    try {
      const accepted = await Promise.race([
        cancelTurn(threadId),
        new Promise<"timeout">((resolve) => window.setTimeout(() => resolve("timeout"), 3_000)),
      ]);
      if (accepted !== true) {
        if (accepted === false) cancellationRequestedTurns.delete(threadId);
        if (get().activeThreadId === threadId) await get().selectThread(threadId);
      }
    } catch (error) {
      cancellationRequestedTurns.delete(threadId);
      if (get().activeThreadId === threadId) set({ error: errorMessage(error) });
    }
  },

  loadProviderCatalog: async () => {
    let catalog = await getProviderCatalog();
    catalog = await migrateLegacyProviderCatalog(catalog);
    set(providerCatalogState(catalog));
  },

  saveProvider: async (request) => {
    try {
      await saveProviderConfig(request);
      const catalog = await getProviderCatalog();
      set({ ...providerCatalogState(catalog), error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  createActiveGoal: async (objective, tokenBudget, timeBudgetMs) => {
    const threadId = get().activeThreadId;
    if (!threadId) return false;
    try {
      const goal = await createGoal({ threadId, objective, tokenBudget, timeBudgetMs });
      set({ goal, error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  activateProvider: async (providerId) => {
    try {
      const catalog = await activateProviderCommand(providerId);
      set({ ...providerCatalogState(catalog), error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  deleteProvider: async (providerId) => {
    try {
      const catalog = await deleteProviderCommand(providerId);
      set({ ...providerCatalogState(catalog), error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  setApprovalMode: async (mode) => {
    try {
      const approvalMode = await setApprovalModeCommand(mode);
      set({ approvalMode, error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  setReasoningEffort: async (effort) => {
    try {
      const reasoningEffort = await setReasoningEffortCommand(effort);
      set({ reasoningEffort, error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  transitionActiveGoal: async (state, reason) => {
    const goal = get().goal;
    if (!goal) return false;
    try {
      const updated = await transitionGoal(goal.id, state, reason);
      set({ goal: updated, error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  resolvePendingApproval: async (resolution) => {
    const approval = get().pendingApproval;
    if (!approval) return false;
    try {
      await resolveApproval(approval.id, resolution);
      set((state) => ({
        ...approvalQueueState(removeApproval(state.pendingApprovals, approval.id)),
        error: "",
      }));
      return true;
    } catch (error) {
      const message = errorMessage(error);
      if (message.includes("approval request was not found")) {
        set((state) => ({
          ...approvalQueueState(removeApproval(state.pendingApprovals, approval.id)),
          error: "",
        }));
        return false;
      }
      set({ error: message });
      return false;
    }
  },

  resolvePendingUserInput: async (resolution) => {
    const request = get().pendingUserInput;
    if (!request) return false;
    try {
      await resolveUserInput(request.id, resolution);
      set((state) => ({
        ...userInputQueueState(removeUserInput(state.pendingUserInputs, request.id)),
        error: "",
      }));
      return true;
    } catch (error) {
      const message = errorMessage(error);
      if (message.includes("user input request was not found")) {
        set((state) => ({
          ...userInputQueueState(removeUserInput(state.pendingUserInputs, request.id)),
          error: "",
        }));
        return false;
      }
      set({ error: message });
      return false;
    }
  },

  undoAppliedChange: async (changeId) => {
    const threadId = get().activeThreadId;
    if (!threadId || get().activeTurns[threadId]) return false;
    try {
      const change = await undoChange(threadId, changeId);
      set((state) => ({
        changes: state.changes.map((item) =>
          item.id === change.id ? { ...item, undone: true } : item,
        ),
        error: "",
      }));
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  handleAgentEvent: (event) => {
    const terminalEvent = ["turn_completed", "turn_failed", "turn_cancelled"].includes(event.type);
    if (terminalEvent && cancellationRequestedTurns.get(event.threadId) === event.turnId) {
      cancellationRequestedTurns.delete(event.threadId);
    }
    if (event.type === "turn_started") {
      set((state) => ({ activeTurns: { ...state.activeTurns, [event.threadId]: event.turnId } }));
    } else if (terminalEvent) {
      set((state) => ({
        activeTurns: state.activeTurns[event.threadId] === event.turnId
          ? withoutActiveTurn(state.activeTurns, event.threadId)
          : state.activeTurns,
      }));
    }
    const hydration = hydrationBuffers.get(event.threadId);
    if (hydration) {
      hydration.events.push(event);
      return;
    }
    if (event.threadId !== get().activeThreadId) {
      if (terminalEvent) {
        setTimeout(() => get().processQueue(), 0);
        void get().reloadThreads();
      }
      return;
    }

    switch (event.type) {
      case "turn_started":
        set((state) => {
          const latestUserMessage = [...state.messages].reverse().find((message) => message.role === "user");
          return {
            activeTurnId: event.turnId,
            activeTurnThreadId: event.threadId,
            lastTurn: { turnId: event.turnId, state: "streaming", error: null },
            pendingApproval: null,
            pendingApprovals: [],
            activityStatus: { turnId: event.turnId, status: "thinking" },
            turnUserMessageIds: state.turnUserMessageIds[event.turnId] || !latestUserMessage
              ? state.turnUserMessageIds
              : { ...state.turnUserMessageIds, [event.turnId]: latestUserMessage.id },
            messages: state.messages.some((message) => message.turnId === event.turnId)
              ? state.messages
              : [...state.messages, {
                id: `turn-${event.turnId}`,
                role: "assistant",
                text: "",
                createdAtMs: Date.now(),
                turnId: event.turnId,
                status: "streaming",
              }],
          };
        });
        break;
      case "activity_status_changed":
        set({ activityStatus: { turnId: event.turnId, status: event.status } });
        break;
      case "text_delta":
        set((state) => {
          const lastItem = state.turnTimeline[state.turnTimeline.length - 1];
          const turnTimeline = lastItem?.type === "text" && lastItem.turnId === event.turnId
            ? state.turnTimeline.map((item, index) => index === state.turnTimeline.length - 1
              ? { ...lastItem, text: lastItem.text + event.delta }
              : item)
            : [...state.turnTimeline, {
                type: "text" as const,
                id: `stream-${event.turnId}-${crypto.randomUUID()}`,
                turnId: event.turnId,
                text: event.delta,
              }];
          return {
            activityStatus: { turnId: event.turnId, status: "responding" },
            turnTimeline,
          };
        });
        break;
      case "reasoning_summary_delta":
        set((state) => {
          const existing = state.turnTimeline.findIndex((item) =>
            item.type === "reasoning" && item.turnId === event.turnId && item.itemId === event.itemId,
          );
          if (existing >= 0) {
            return {
              turnTimeline: state.turnTimeline.map((item, index) => index === existing && item.type === "reasoning"
                ? { ...item, summary: (item.summary + event.delta).slice(0, MAX_REASONING_SUMMARY_CHARS), complete: false }
                : item),
            };
          }
          return {
            turnTimeline: [...state.turnTimeline, {
              type: "reasoning" as const,
              itemId: event.itemId,
              turnId: event.turnId,
              summary: event.delta.slice(0, MAX_REASONING_SUMMARY_CHARS),
              complete: false,
            }],
          };
        });
        break;
      case "reasoning_summary_completed":
        set((state) => {
          const summary = event.summary.slice(0, MAX_REASONING_SUMMARY_CHARS);
          const exists = state.turnTimeline.some((item) =>
            item.type === "reasoning" && item.turnId === event.turnId && item.itemId === event.itemId,
          );
          return {
            turnTimeline: exists
              ? state.turnTimeline.map((item) => item.type === "reasoning"
                  && item.turnId === event.turnId
                  && item.itemId === event.itemId
                ? { ...item, summary, complete: true }
                : item)
              : [...state.turnTimeline, {
                  type: "reasoning" as const,
                  itemId: event.itemId,
                  turnId: event.turnId,
                  summary,
                  complete: true,
                }],
          };
        });
        break;
      case "usage_updated":
        set({ usage: event.usage });
        break;
      case "tool_started":
        set((state) => {
          const startedAtMs = Date.now();
          const activity: ToolActivity = {
            turnId: event.turnId,
            call: event.call,
            state: "running",
            result: null,
            startedAtMs,
          };
          const hasExisting = state.turnTimeline.some((item) => item.type === "tool"
            && item.activity.turnId === event.turnId
            && item.activity.call.id === event.call.id);
          return {
            lastTurn: { turnId: event.turnId, state: "running_tool", error: null },
            activityStatus: { turnId: event.turnId, status: "running_tool" },
            turnTimeline: hasExisting
              ? state.turnTimeline.map((item) => item.type === "tool"
                && item.activity.turnId === event.turnId
                && item.activity.call.id === event.call.id
                ? {
                    ...item,
                    activity: {
                      ...item.activity,
                      call: event.call,
                      state: "running",
                      result: null,
                      startedAtMs: item.activity.startedAtMs ?? startedAtMs,
                      completedAtMs: undefined,
                      durationMs: undefined,
                    },
                  }
                : item)
              : [...state.turnTimeline, { type: "tool", activity }],
          };
        });
        break;
      case "tool_output_delta":
        set((state) => ({
          turnTimeline: state.turnTimeline.map((item) => item.type === "tool"
            ? { ...item, activity: appendToolOutput(
                item.activity,
                event.turnId,
                event.callId,
                event.stream,
                event.cursor,
                event.delta,
              ) }
            : item),
        }));
        break;
      case "tool_completed":
        set((state) => {
          const completedAtMs = Date.now();
          return {
            lastTurn: { turnId: event.turnId, state: "streaming", error: null },
            activityStatus: { turnId: event.turnId, status: "thinking" },
            turnTimeline: state.turnTimeline.map((item) =>
              item.type === "tool"
                && item.activity.turnId === event.turnId
                && item.activity.call.id === event.callId
                ? {
                    ...item,
                    activity: {
                      ...item.activity,
                      state: event.result.success ? "completed" : "failed",
                      result: event.result,
                      completedAtMs,
                      durationMs: resultDurationMs(event.result)
                        ?? (item.activity.startedAtMs
                          ? Math.max(0, completedAtMs - item.activity.startedAtMs)
                          : undefined),
                    },
                  }
                : item,
            ),
          };
        });
        if (event.name === "update_plan") {
          void getPlan(event.threadId)
            .then((updatedPlan) => {
              if (get().activeThreadId === event.threadId) set({ plan: updatedPlan });
            })
            .catch(() => undefined);
        }
        break;
      case "approval_requested":
        set((state) => {
          if (event.request.autoApproved) {
            return {
              turnTimeline: insertApprovalRequestEvent(
                state.turnTimeline,
                `approval-requested-${event.request.id}`,
                event.turnId,
                "approval_requested",
                "已自动批准操作",
                `${event.request.toolName} · ${event.request.reason}`,
                event.request.toolCallId,
              ),
            };
          }
          const queue = enqueueApproval(state.pendingApprovals, event.request);
          return {
            ...approvalQueueState(queue),
            activityStatus: { turnId: event.turnId, status: "awaiting_approval" },
            lastTurn: { turnId: event.turnId, state: "awaiting_approval", error: null },
            turnTimeline: insertApprovalRequestEvent(
              state.turnTimeline,
              `approval-requested-${event.request.id}`,
              event.turnId,
              "approval_requested",
              "已请求操作确认",
              `${event.request.toolName} · ${event.request.reason}`,
              event.request.toolCallId,
            ),
          };
        });
        break;
      case "approval_resolved":
        set((state) => {
          const queue = removeApproval(state.pendingApprovals, event.requestId);
          return {
            ...approvalQueueState(queue),
            lastTurn: {
              turnId: event.turnId,
              state: queue.length ? "awaiting_approval" : "streaming",
              error: null,
            },
            activityStatus: {
              turnId: event.turnId,
              status: queue.length ? "awaiting_approval" : "thinking",
            },
            turnTimeline: insertApprovalResolutionEvent(
              state.turnTimeline,
              `approval-resolved-${event.requestId}`,
              event.turnId,
              "approval_resolved",
              "操作确认已处理",
              event.resolution.action,
              event.requestId,
            ),
          };
        });
        break;
      case "user_input_requested":
        set((state) => {
          const queue = enqueueUserInput(state.pendingUserInputs, event.request);
          return {
            ...userInputQueueState(queue),
            activityStatus: { turnId: event.turnId, status: "awaiting_approval" },
            lastTurn: { turnId: event.turnId, state: "awaiting_approval", error: null },
            turnTimeline: appendTimelineEvent(
              state.turnTimeline,
              `user-input-requested-${event.request.id}`,
              event.turnId,
              "user_input_requested",
              "已请求用户输入",
              event.request.questions.map((question) => question.question).join("；"),
            ),
          };
        });
        break;
      case "user_input_resolved":
        set((state) => {
          const queue = removeUserInput(state.pendingUserInputs, event.requestId);
          return {
            ...userInputQueueState(queue),
            lastTurn: {
              turnId: event.turnId,
              state: queue.length ? "awaiting_approval" : "streaming",
              error: null,
            },
            activityStatus: {
              turnId: event.turnId,
              status: queue.length ? "awaiting_approval" : "thinking",
            },
            turnTimeline: appendTimelineEvent(
              state.turnTimeline,
              `user-input-resolved-${event.requestId}`,
              event.turnId,
              "user_input_resolved",
              "用户输入已处理",
              event.resolution.action,
            ),
          };
        });
        break;
      case "change_applied":
        set((state) => ({
          changes: state.changes.some((change) => change.id === event.changeSet.id)
            ? state.changes
            : [...state.changes, event.changeSet],
          turnTimeline: appendTimelineEvent(
            state.turnTimeline,
            `change-applied-${event.changeSet.id}`,
            event.turnId,
            "change_applied",
            `已应用 ${event.changeSet.files.length} 个文件变更`,
            event.changeSet.files.map((file) => file.path).join("、"),
          ),
        }));
        break;
      case "change_undone":
        set((state) => ({
          changes: state.changes.map((change) =>
            change.id === event.changeId ? { ...change, undone: true } : change,
          ),
          turnTimeline: appendTimelineEvent(
            state.turnTimeline,
            `change-undone-${event.changeId}`,
            event.turnId,
            "change_undone",
            "已撤销文件变更",
            event.changeId,
          ),
        }));
        break;
      case "turn_completed":
        set((state) => {
          const completedAtMs = event.completedAtMs ?? Date.now();
          const finalText = toConversationMessage(event.message, event.turnId).text;
          let lastTextItemIndex = -1;
          let lastToolItemIndex = -1;
          for (let index = state.turnTimeline.length - 1; index >= 0; index -= 1) {
            const item = state.turnTimeline[index];
            const itemTurnId = item.type === "tool" ? item.activity.turnId : item.turnId;
            if (itemTurnId !== event.turnId) continue;
            if (lastTextItemIndex < 0 && item.type === "text") lastTextItemIndex = index;
            if (lastToolItemIndex < 0 && item.type === "tool") lastToolItemIndex = index;
            if (lastTextItemIndex >= 0 && lastToolItemIndex >= 0) break;
          }
          let turnTimeline = lastTextItemIndex > lastToolItemIndex
            ? state.turnTimeline.map((item, index) => index === lastTextItemIndex
              ? { type: "text" as const, id: event.message.id, turnId: event.turnId, text: finalText }
              : item)
            : finalText
              ? [...state.turnTimeline, { type: "text" as const, id: event.message.id, turnId: event.turnId, text: finalText }]
              : state.turnTimeline;
          turnTimeline = finishRunningTools(turnTimeline, event.turnId, "failed", completedAtMs);
          turnTimeline = appendTimelineEvent(
            turnTimeline,
            `turn-completed-${event.turnId}`,
            event.turnId,
            "turn_completed",
            "任务完成",
            null,
            event.durationMs,
          );
          return {
          activeTurnId: null,
          activeTurnThreadId: null,
          activityStatus: null,
          pendingApproval: null,
          pendingApprovals: [],
          pendingUserInput: null,
          pendingUserInputs: [],
          usage: event.usage,
          lastTurn: { turnId: event.turnId, state: "completed", error: null },
          turnTimeline,
          messages: state.messages.some((message) => message.turnId === event.turnId)
            ? state.messages.map((message) => message.turnId === event.turnId
                ? toConversationMessage(event.message, event.turnId)
                : message)
            : [...state.messages, toConversationMessage(event.message, event.turnId)],
          };
        });
        // Turn 完成后，触发队列处理下一条消息
        setTimeout(() => get().processQueue(), 500);
        break;
      case "turn_failed":
        set((state) => {
          const failedAtMs = event.completedAtMs ?? Date.now();
          return {
          activeTurnId: null,
          activeTurnThreadId: null,
          activityStatus: null,
          pendingApproval: null,
          pendingApprovals: [],
          pendingUserInput: null,
          pendingUserInputs: [],
          error: event.message,
          lastTurn: {
            turnId: event.turnId,
            state: "failed",
            error: event.message,
          },
          messages: state.messages.map((message) =>
            message.turnId === event.turnId
              ? { ...message, status: "failed" as const }
              : message,
          ),
          turnTimeline: appendTimelineEvent(
            finishRunningTools(state.turnTimeline, event.turnId, "failed", failedAtMs),
            `turn-failed-${event.turnId}`,
            event.turnId,
            "turn_failed",
            "Turn 执行失败",
            event.message,
            event.durationMs,
          ),
          };
        });
        setTimeout(() => get().processQueue(), 500);
        break;
      case "turn_cancelled":
        set((state) => {
          const cancelledAtMs = Date.now();
          return {
            activeTurnId: null,
            activeTurnThreadId: null,
            activityStatus: null,
            pendingApproval: null,
            pendingApprovals: [],
            pendingUserInput: null,
            pendingUserInputs: [],
            lastTurn: { turnId: event.turnId, state: "cancelled", error: null },
            messages: state.messages.map((message) =>
              message.turnId === event.turnId
                ? { ...message, status: "cancelled" as const }
                : message,
            ),
            turnTimeline: appendTimelineEvent(
              finishRunningTools(state.turnTimeline, event.turnId, "cancelled", cancelledAtMs),
              `turn-cancelled-${event.turnId}`,
              event.turnId,
              "turn_cancelled",
              "Turn 已取消",
              null,
              event.durationMs,
            ),
          };
        });
        setTimeout(() => get().processQueue(), 500);
        break;
      case "todo_updated":
        set((state) => {
          const newTodos = new Map(state.todos);
          newTodos.set(event.threadId, event.todos);
          return {
            todos: newTodos,
          };
        });
        break;
    }
  },

  clearError: () => set({ error: "" }),

  forceResetState: async () => {
    const threadId = get().activeThreadId;
    console.log("强制重置状态...");
    if (threadId) cancellationRequestedTurns.delete(threadId);

    // 尝试取消任何正在运行的 turn
    if (threadId && get().activeTurns[threadId]) {
      try {
        await cancelTurn(threadId);
      } catch (e) {
        console.warn("取消 turn 失败（可能已完成）", e);
      }
    }

    // 清除所有可能导致卡住的状态
    set((state) => ({
      activeTurnId: null,
      activeTurnThreadId: null,
      activeTurns: threadId ? withoutActiveTurn(state.activeTurns, threadId) : state.activeTurns,
      pendingApproval: null,
      pendingApprovals: [],
      pendingUserInput: null,
      pendingUserInputs: [],
      turnTimeline: [],
      turnUserMessageIds: {},
      activityStatus: null,
      error: "",
      loading: false,
    }));

    // 重新加载当前线程的状态
    if (threadId) {
      await get().selectThread(threadId);
    }

    console.log("状态已重置");
  },
}));
