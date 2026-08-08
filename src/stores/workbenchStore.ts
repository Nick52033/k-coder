import { create } from "zustand";
import {
  archiveThread as archiveThreadCommand,
  createThread as createThreadCommand,
  errorMessage,
  getProviderCatalog,
  getApprovalMode,
  getReasoningEffort,
  activateProvider as activateProviderCommand,
  deleteProvider as deleteProviderCommand,
  listThreads,
  listThreadTurns,
  readThread,
  readThreadHistory,
  searchThreads,
  renameThread,
  deleteThread,
  resolveApproval,
  resolveUserInput,
  retryTurn,
  startTurn,
  readThreadMailbox,
  removeQueuedTurn,
  clearThreadMailbox,
  steerQueuedTurn,
  interruptTurn,
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
  ThreadItem,
  ThreadTurn,
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
  UserInputSnapshot,
  TodoItem,
  ThreadMailboxChanged,
} from "../types/runtime";
import {
  reduceAgentEvent,
  reduceTurnLifecycle,
  type ReducerHelpers,
} from "./reducers/agentEventReducer";

interface QueuedMessage {
  id: string;
  messageId: string;
  threadId: string;
  kind: "message" | "retry";
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
  cancellingTurns: Record<string, string>;
  mailboxRevisions: Record<string, number>;
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
  historyNextCursor: string | null;
  historyLoading: boolean;
  loading: boolean;
  error: string;
  initialize: () => Promise<void>;
  reloadThreads: () => Promise<void>;
  searchThreadHistory: (query: string) => Promise<void>;
  renameConversation: (threadId: string, title: string) => Promise<void>;
  deleteConversation: (threadId: string) => Promise<void>;
  createThread: () => Promise<string>;
  selectThread: (threadId: string) => Promise<void>;
  loadOlderHistory: () => Promise<void>;
  archiveActiveThread: () => Promise<void>;
  sendMessage: (input: string, attachments?: ImageAttachment[], agentMode?: string) => Promise<void>;
  processQueue: (threadId?: string, minimumRevision?: number) => Promise<void>;
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
  handleMailboxChanged: (event: ThreadMailboxChanged) => void;
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

interface ProjectedThreadHistory {
  messages: ConversationMessage[];
  turnTimeline: TurnTimelineItem[];
  turnUserMessageIds: Record<string, string>;
  approvals: ApprovalSnapshot[];
  userInputs: UserInputSnapshot[];
  changes: ChangeSet[];
}

function projectHistoryTurns(turns: ThreadTurn[], unscopedItems: ThreadItem[] = []): ProjectedThreadHistory {
  const projected: ProjectedThreadHistory = {
    messages: [],
    turnTimeline: [],
    turnUserMessageIds: {},
    approvals: [],
    userInputs: [],
    changes: [],
  };
  const messageIds = new Set<string>();
  const timelineIds = new Set<string>();
  const approvalIds = new Set<string>();
  const userInputIds = new Set<string>();
  const changeIds = new Set<string>();

  const projectItem = (item: ThreadItem, turnId?: string) => {
    if (item.type === "user_message" && !messageIds.has(item.message.id)) {
      messageIds.add(item.message.id);
      projected.messages.push(toConversationMessage(item.message));
    } else if (item.type === "agent_message" && item.phase === "final_answer" && !messageIds.has(item.message.id)) {
      messageIds.add(item.message.id);
      projected.messages.push(toConversationMessage(item.message, turnId ?? item.turnId ?? undefined));
    } else if (item.type === "approval" && !approvalIds.has(item.approval.request.id)) {
      approvalIds.add(item.approval.request.id);
      projected.approvals.push(item.approval);
    } else if (item.type === "user_input" && !userInputIds.has(item.userInput.request.id)) {
      userInputIds.add(item.userInput.request.id);
      projected.userInputs.push(item.userInput);
    } else if (item.type === "change" && !changeIds.has(item.changeSet.id)) {
      changeIds.add(item.changeSet.id);
      projected.changes.push(item.changeSet);
    }

    for (const timelineItem of item.timelineItems) {
      const key = timelineItemKey(timelineItem);
      if (timelineIds.has(key)) continue;
      timelineIds.add(key);
      projected.turnTimeline.push(timelineItem);
    }
  };

  for (const turn of turns) {
    if (turn.userMessageId) projected.turnUserMessageIds[turn.id] = turn.userMessageId;
    for (const item of turn.items) {
      projectItem(item, turn.id);
    }
  }
  for (const item of unscopedItems) projectItem(item);
  projected.messages.sort((left, right) => left.createdAtMs - right.createdAtMs);
  return projected;
}

function timelineItemKey(item: TurnTimelineItem) {
  if (item.type === "text") return `text:${item.turnId}:${item.id}`;
  if (item.type === "reasoning") return `reasoning:${item.turnId}:${item.itemId}`;
  if (item.type === "tool") return `tool:${item.activity.turnId}:${item.activity.call.id}`;
  return `event:${item.turnId}:${item.itemId}`;
}

function prependUnique<T>(older: T[], current: T[], key: (item: T) => string) {
  const seen = new Set<string>();
  return [...older, ...current].filter((item) => {
    const value = key(item);
    if (seen.has(value)) return false;
    seen.add(value);
    return true;
  });
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
const terminalTurnIds = new Set<string>();

function mailboxMessages(threadId: string, pending: Array<{
  turnId: string;
  kind?: "message" | "retry";
  input: string;
  attachments: ImageAttachment[];
  agentMode: string | null;
}>): QueuedMessage[] {
  return pending.map((item) => ({
    id: item.turnId,
    messageId: item.turnId,
    threadId,
    kind: item.kind ?? "message",
    input: item.input,
    attachments: item.attachments,
    agentMode: item.agentMode ?? undefined,
    status: "pending",
    turnId: item.turnId,
  }));
}

function rememberTerminalTurn(turnId: string) {
  terminalTurnIds.add(turnId);
  if (terminalTurnIds.size > 200) {
    const oldest = terminalTurnIds.values().next().value;
    if (oldest) terminalTurnIds.delete(oldest);
  }
}
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
  cancellingTurns: {},
  mailboxRevisions: {},
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
  historyNextCursor: null,
  historyLoading: false,
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
        historyNextCursor: null,
        historyLoading: false,
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
    set((state) => {
      const todos = new Map(state.todos);
      todos.delete(threadId);
      return {
        activeThreadId: threadId,
        loading: true,
        error: "",
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
        todos,
        plan: null,
        goal: null,
        historyNextCursor: null,
        historyLoading: false,
        messageQueue: state.messageQueue.filter((message) => message.threadId !== threadId),
      };
    });
    try {
      const [history, plan, goal, mailbox] = await Promise.all([
        readThreadHistory(threadId).catch(() => null),
        getPlan(threadId),
        getGoal(threadId),
        readThreadMailbox(threadId).catch(() => null),
      ]);
      const detail = history ? null : await readThread(threadId);
      if (get().activeThreadId !== threadId) return;
      const hydration = hydrationBuffers.get(threadId);
      if (!hydration || hydration.token !== token) return;
      const lastTurn = history ? history.lastTurn : detail!.lastTurn;
      const summary = history ? history.summary : detail!.summary;
      const restoredActiveTurnId = mailbox?.activeTurnId ?? (lastTurn
        && ["queued", "streaming", "running_tool", "awaiting_approval"].includes(lastTurn.state)
        ? lastTurn.turnId
        : null);
      const activeTurns = restoredActiveTurnId
        ? { ...get().activeTurns, [threadId]: restoredActiveTurnId }
        : withoutActiveTurn(get().activeTurns, threadId);
      const cancellingTurns = restoredActiveTurnId
        && get().cancellingTurns[threadId] === restoredActiveTurnId
          ? get().cancellingTurns
          : withoutActiveTurn(get().cancellingTurns, threadId);

      const projected = history
        ? projectHistoryTurns([...history.turns.data].reverse(), history.unscopedItems)
        : null;
      const approvals = projected?.approvals ?? detail!.approvals;
      const userInputs = projected?.userInputs ?? detail!.userInputs ?? [];
      const pendingApprovals = approvals
        .filter((approval) => !approval.resolution)
        .map((approval) => approval.request);
      const pendingUserInputs = userInputs
        .filter((input) => !input.resolution)
        .map((input) => input.request);
      const todos = new Map(get().todos);
      todos.set(threadId, history ? history.todos : detail!.todos ?? []);
      hydrationBuffers.delete(threadId);
      const restoredTimeline = projected?.turnTimeline ?? (detail!.turnTimeline?.length
        ? normalizeApprovalTimeline(detail!.turnTimeline, detail!.approvals)
        : detail!.toolActivities.map((activity) => ({ type: "tool" as const, activity })));
      const terminalTimeline = history
        ? restoredTimeline
        : lastTurn?.state === "cancelled"
        ? finishRunningTools(restoredTimeline, lastTurn.turnId, "cancelled", summary.updatedAtMs)
        : lastTurn?.state === "failed" || lastTurn?.state === "completed"
          ? finishRunningTools(restoredTimeline, lastTurn.turnId, "failed", summary.updatedAtMs)
          : restoredTimeline;
      set({
        messages: projected?.messages ?? detail!.messages.map((message) =>
          toConversationMessage(message, detail!.messageTurnIds?.[message.id]),
        ),
        lastTurn,
        turnTimeline: terminalTimeline,
        turnUserMessageIds: projected?.turnUserMessageIds ?? detail!.turnUserMessageIds ?? {},
        activityStatus: restoredActiveTurnId
          ? { turnId: lastTurn!.turnId, status: statusForTurnState(lastTurn!.state) ?? "thinking" }
          : null,
        ...approvalQueueState(pendingApprovals),
        ...userInputQueueState(pendingUserInputs),
        changes: projected?.changes ?? detail!.changes,
        todos,
        usage: history ? history.lastUsage : detail!.lastUsage ?? null,
        historyNextCursor: history?.turns.nextCursor ?? null,
        plan,
        goal,
        activeTurns,
        cancellingTurns,
        mailboxRevisions: {
          ...get().mailboxRevisions,
          [threadId]: mailbox?.revision ?? 0,
        },
        activeTurnId: restoredActiveTurnId,
        activeTurnThreadId: restoredActiveTurnId ? threadId : null,
        messageQueue: [
          ...get().messageQueue.filter((message) => message.threadId !== threadId),
          ...mailboxMessages(threadId, mailbox?.pending ?? []),
        ],
      });
      for (const event of hydration.events) get().handleAgentEvent(event);
    } catch (error) {
      set({ error: errorMessage(error) });
    } finally {
      if (hydrationBuffers.get(threadId)?.token === token) hydrationBuffers.delete(threadId);
      if (get().activeThreadId === threadId) set({ loading: false });
    }
  },

  loadOlderHistory: async () => {
    const { activeThreadId: threadId, historyNextCursor: cursor, historyLoading } = get();
    if (!threadId || !cursor || historyLoading) return;
    set({ historyLoading: true, error: "" });
    try {
      const page = await listThreadTurns(threadId, {
        cursor,
        limit: 50,
        sortDirection: "desc",
        itemsView: "full",
      });
      if (get().activeThreadId !== threadId) return;
      const older = projectHistoryTurns([...page.data].reverse());
      set((state) => ({
        messages: prependUnique(older.messages, state.messages, (message) => message.id),
        turnTimeline: prependUnique(older.turnTimeline, state.turnTimeline, timelineItemKey),
        turnUserMessageIds: { ...older.turnUserMessageIds, ...state.turnUserMessageIds },
        changes: prependUnique(older.changes, state.changes, (change) => change.id),
        historyNextCursor: page.nextCursor,
      }));
    } catch (error) {
      set({ error: errorMessage(error) });
    } finally {
      if (get().activeThreadId === threadId) set({ historyLoading: false });
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
    const { activeThreadId: threadId } = get();
    const text = input.trim();
    if (!threadId || (!text && attachments.length === 0)) return;
    set({ error: "" });
    try {
      const handle = await startTurn(threadId, text, attachments, agentMode);
      if (handle.state === "queued") {
        await get().processQueue();
      } else if (!terminalTurnIds.has(handle.turnId)) {
        set((state) => ({
          activeTurns: { ...state.activeTurns, [threadId]: handle.turnId },
          cancellingTurns: withoutActiveTurn(state.cancellingTurns, threadId),
        }));
      }
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  processQueue: async (targetThreadId, minimumRevision = 0) => {
    const threadId = targetThreadId ?? get().activeThreadId;
    if (!threadId) return;
    try {
      const mailbox = await readThreadMailbox(threadId);
      set((state) => {
        const expectedRevision = Math.max(
          minimumRevision,
          state.mailboxRevisions[threadId] ?? 0,
        );
        if (mailbox.revision < expectedRevision) return {};
        return {
          mailboxRevisions: {
            ...state.mailboxRevisions,
            [threadId]: mailbox.revision,
          },
          messageQueue: [
            ...state.messageQueue.filter((message) => message.threadId !== threadId),
            ...mailboxMessages(threadId, mailbox.pending),
          ],
        };
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  handleMailboxChanged: (event) => {
    const knownRevision = get().mailboxRevisions[event.threadId] ?? 0;
    if (event.revision <= knownRevision) return;
    set((state) => ({
      mailboxRevisions: {
        ...state.mailboxRevisions,
        [event.threadId]: event.revision,
      },
    }));
    void get().processQueue(event.threadId, event.revision);
  },

  sendQueuedMessageNow: async (messageId) => {
    const queuedMessage = get().messageQueue.find((message) =>
      message.id === messageId && message.status === "pending"
    );
    if (!queuedMessage || queuedMessage.kind !== "message") return;

    const activeTurnId = get().activeTurns[queuedMessage.threadId];
    if (activeTurnId) {
      try {
        await steerQueuedTurn(
          queuedMessage.threadId,
          activeTurnId,
          queuedMessage.turnId ?? queuedMessage.id,
        );
        await get().processQueue();
      } catch (error) {
        set({ error: errorMessage(error) });
        await get().processQueue();
      }
    }
  },

  removeQueuedMessage: (messageId) => {
    const queued = get().messageQueue.find((message) => message.id === messageId);
    if (!queued) return;
    void removeQueuedTurn(queued.threadId, queued.turnId ?? queued.id)
      .then(() => get().processQueue())
      .catch((error) => set({ error: errorMessage(error) }));
  },

  clearQueue: () => {
    const threadId = get().activeThreadId;
    if (!threadId) return;
    void clearThreadMailbox(threadId)
      .then(() => get().processQueue())
      .catch((error) => set({ error: errorMessage(error) }));
  },

  retryLastTurn: async () => {
    const threadId = get().activeThreadId;
    if (!threadId || get().activeTurns[threadId]) return;
    set({ error: "", usage: null });
    try {
      const handle = await retryTurn(threadId);
      if (handle.state === "queued") {
        await get().processQueue();
      } else if (!terminalTurnIds.has(handle.turnId)) {
        set((state) => ({
          activeTurns: { ...state.activeTurns, [threadId]: handle.turnId },
          cancellingTurns: withoutActiveTurn(state.cancellingTurns, threadId),
        }));
      }
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  stopTurn: async (targetThreadId) => {
    const threadId = targetThreadId ?? get().activeThreadId;
    const turnId = threadId ? get().activeTurns[threadId] : null;
    if (!threadId || !turnId || get().cancellingTurns[threadId] === turnId) return;
    set((state) => ({
      cancellingTurns: { ...state.cancellingTurns, [threadId]: turnId },
      error: state.activeThreadId === threadId ? "" : state.error,
    }));
    try {
      const accepted = await Promise.race([
        interruptTurn(threadId, turnId).then(() => "accepted" as const),
        new Promise<"timeout">((resolve) => window.setTimeout(() => resolve("timeout"), 3_000)),
      ]);
      if (accepted === "timeout") {
        set((state) => ({
          cancellingTurns: withoutActiveTurn(state.cancellingTurns, threadId),
          error: state.activeThreadId === threadId && state.activeTurns[threadId] === turnId
            ? "停止请求超时，Turn 仍在运行，可以重试停止"
            : state.error,
        }));
      }
    } catch (error) {
      set((state) => ({
        cancellingTurns: withoutActiveTurn(state.cancellingTurns, threadId),
        error: state.activeThreadId === threadId ? errorMessage(error) : state.error,
      }));
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
    if (event.type === "turn_started") {
      terminalTurnIds.delete(event.turnId);
    } else if (terminalEvent) {
      rememberTerminalTurn(event.turnId);
    }
    const lifecyclePatch = reduceTurnLifecycle(event, {
      activeTurns: get().activeTurns,
      cancellingTurns: get().cancellingTurns,
    });
    if (lifecyclePatch) {
      set(lifecyclePatch);
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

    const helpers: ReducerHelpers = {
      toConversationMessage,
      appendTimelineEvent,
      insertApprovalRequestEvent,
      insertApprovalResolutionEvent,
      finishRunningTools,
      approvalQueueState,
      userInputQueueState,
      enqueueApproval,
      removeApproval,
      enqueueUserInput,
      removeUserInput,
      appendToolOutput,
      resultDurationMs,
    };
    const state = get();
    const reduction = reduceAgentEvent(event, state, helpers);
    if (!reduction) return;
    if (Object.keys(reduction.state).length > 0) {
      set(reduction.state);
    }
    if (reduction.sideEffects?.includes("processQueue")) {
      void get().processQueue();
    }
    if (reduction.sideEffects?.includes("refreshPlan") && event.type === "tool_completed") {
      void getPlan(event.threadId)
        .then((updatedPlan) => {
          if (get().activeThreadId === event.threadId) set({ plan: updatedPlan });
        })
        .catch(() => undefined);
    }
  },

  clearError: () => set({ error: "" }),

  forceResetState: async () => {
    const threadId = get().activeThreadId;
    const turnId = threadId ? get().activeTurns[threadId] : null;
    let recoveryError = "";

    if (threadId && turnId) {
      set((state) => ({
        cancellingTurns: { ...state.cancellingTurns, [threadId]: turnId },
      }));
      try {
        const result = await Promise.race([
          interruptTurn(threadId, turnId).then(() => "accepted" as const),
          new Promise<"timeout">((resolve) => window.setTimeout(() => resolve("timeout"), 3_000)),
        ]);
        if (result === "timeout") {
          recoveryError = "精确停止请求超时，已重新同步运行时状态";
        }
      } catch (error) {
        recoveryError = errorMessage(error);
      }
    }

    if (threadId) {
      await get().selectThread(threadId);
      if (recoveryError) set({ error: recoveryError });
      return;
    }
    set({
      activeTurnId: null,
      activeTurnThreadId: null,
      pendingApproval: null,
      pendingApprovals: [],
      pendingUserInput: null,
      pendingUserInputs: [],
      turnTimeline: [],
      turnUserMessageIds: {},
      activityStatus: null,
      error: recoveryError,
      loading: false,
    });
  },
}));
