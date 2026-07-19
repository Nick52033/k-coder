import { create } from "zustand";
import {
  archiveThread as archiveThreadCommand,
  cancelTurn,
  createThread as createThreadCommand,
  errorMessage,
  getProviderConfig,
  listThreads,
  readThread,
  searchThreads,
  renameThread,
  deleteThread,
  resolveApproval,
  retryTurn,
  runTurn,
  saveProviderConfig,
  undoChange,
} from "../api/runtime";
import type {
  AgentEvent,
  ApprovalRequest,
  ApprovalResolution,
  ChatMessage,
  ChangeSet,
  ConversationMessage,
  ImageAttachment,
  ProviderConfigView,
  SaveProviderConfigRequest,
  ThreadSummary,
  ToolActivity,
  TokenUsage,
  TurnSnapshot,
} from "../types/runtime";

interface WorkbenchState {
  threads: ThreadSummary[];
  activeThreadId: string | null;
  messages: ConversationMessage[];
  lastTurn: TurnSnapshot | null;
  activeTurnId: string | null;
  usage: TokenUsage | null;
  toolActivities: ToolActivity[];
  pendingApproval: ApprovalRequest | null;
  changes: ChangeSet[];
  providerConfig: ProviderConfigView | null;
  loading: boolean;
  error: string;
  initialize: () => Promise<void>;
  reloadThreads: () => Promise<void>;
  searchThreadHistory: (query: string) => Promise<void>;
  renameConversation: (threadId: string, title: string) => Promise<void>;
  deleteConversation: (threadId: string) => Promise<void>;
  createThread: () => Promise<void>;
  selectThread: (threadId: string) => Promise<void>;
  archiveActiveThread: () => Promise<void>;
  sendMessage: (input: string, attachments?: ImageAttachment[]) => Promise<void>;
  retryLastTurn: () => Promise<void>;
  stopTurn: () => Promise<void>;
  loadProviderConfig: () => Promise<void>;
  saveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
  resolvePendingApproval: (resolution: ApprovalResolution) => Promise<boolean>;
  undoAppliedChange: (changeId: string) => Promise<boolean>;
  handleAgentEvent: (event: AgentEvent) => void;
  clearError: () => void;
}

function toConversationMessage(message: ChatMessage): ConversationMessage {
  return {
    id: message.id,
    role: message.role,
    text: message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join(""),
    createdAtMs: message.createdAtMs,
  };
}

let initializationPromise: Promise<void> | null = null;

export const useWorkbenchStore = create<WorkbenchState>((set, get) => ({
  threads: [],
  activeThreadId: null,
  messages: [],
  lastTurn: null,
  activeTurnId: null,
  usage: null,
  toolActivities: [],
  pendingApproval: null,
  changes: [],
  providerConfig: null,
  loading: true,
  error: "",

  initialize: () => {
    if (initializationPromise) return initializationPromise;
    initializationPromise = (async () => {
      set({ loading: true, error: "" });
      try {
        await Promise.all([get().reloadThreads(), get().loadProviderConfig()]);
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
        usage: null,
        toolActivities: [],
        pendingApproval: null,
        changes: [],
        error: "",
      }));
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  selectThread: async (threadId) => {
    set({ activeThreadId: threadId, loading: true, error: "" });
    try {
      const detail = await readThread(threadId);
      if (get().activeThreadId !== threadId) return;
      set({
        messages: detail.messages.map(toConversationMessage),
        lastTurn: detail.lastTurn,
        toolActivities: detail.toolActivities,
        pendingApproval:
          [...detail.approvals].reverse().find((approval) => !approval.resolution)?.request ?? null,
        changes: detail.changes,
        activeTurnId:
          detail.lastTurn
          && ["queued", "streaming", "running_tool", "awaiting_approval"].includes(detail.lastTurn.state)
            ? detail.lastTurn.turnId
            : null,
      });
    } catch (error) {
      set({ error: errorMessage(error) });
    } finally {
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

  sendMessage: async (input, attachments = []) => {
    const threadId = get().activeThreadId;
    const text = input.trim();
    if (!threadId || !text || get().activeTurnId) return;
    const optimisticId = `pending-${crypto.randomUUID()}`;
    set((state) => ({
      messages: [
        ...state.messages,
        {
          id: optimisticId,
          role: "user",
          text,
          createdAtMs: Date.now(),
        },
      ],
      error: "",
      usage: null,
    }));
    try {
      const outcome = await runTurn(threadId, text, attachments);
      await Promise.all([get().reloadThreads(), get().selectThread(threadId)]);
      if (outcome.error) set({ error: outcome.error });
    } catch (error) {
      set({ error: errorMessage(error), activeTurnId: null });
      await get().selectThread(threadId);
    }
  },

  retryLastTurn: async () => {
    const threadId = get().activeThreadId;
    if (!threadId || get().activeTurnId) return;
    set({ error: "", usage: null });
    try {
      const outcome = await retryTurn(threadId);
      await Promise.all([get().reloadThreads(), get().selectThread(threadId)]);
      if (outcome.error) set({ error: outcome.error });
    } catch (error) {
      set({ error: errorMessage(error), activeTurnId: null });
    }
  },

  stopTurn: async () => {
    const threadId = get().activeThreadId;
    if (!threadId || !get().activeTurnId) return;
    try {
      await cancelTurn(threadId);
    } catch (error) {
      set({ error: errorMessage(error) });
    }
  },

  loadProviderConfig: async () => {
    const providerConfig = await getProviderConfig();
    set({ providerConfig });
  },

  saveProvider: async (request) => {
    try {
      const providerConfig = await saveProviderConfig(request);
      set({ providerConfig, error: "" });
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
      set({ pendingApproval: null, error: "" });
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  },

  undoAppliedChange: async (changeId) => {
    const threadId = get().activeThreadId;
    if (!threadId || get().activeTurnId) return false;
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
    if (event.threadId !== get().activeThreadId) {
      if (event.type === "turn_completed") void get().reloadThreads();
      return;
    }

    switch (event.type) {
      case "turn_started":
        set((state) => ({
          activeTurnId: event.turnId,
          lastTurn: { turnId: event.turnId, state: "streaming", error: null },
          pendingApproval: null,
          messages: [
            ...state.messages,
            {
              id: `turn-${event.turnId}`,
              role: "assistant",
              text: "",
              createdAtMs: Date.now(),
              turnId: event.turnId,
              status: "streaming",
            },
          ],
        }));
        break;
      case "text_delta":
        set((state) => ({
          messages: state.messages.map((message) =>
            message.turnId === event.turnId
              ? { ...message, text: message.text + event.delta }
              : message,
          ),
        }));
        break;
      case "usage_updated":
        set({ usage: event.usage });
        break;
      case "tool_started":
        set((state) => ({
          lastTurn: { turnId: event.turnId, state: "running_tool", error: null },
          toolActivities: [
            ...state.toolActivities,
            {
              turnId: event.turnId,
              call: event.call,
              state: "running",
              result: null,
            },
          ],
        }));
        break;
      case "tool_completed":
        set((state) => ({
          lastTurn: { turnId: event.turnId, state: "streaming", error: null },
          toolActivities: state.toolActivities.map((activity) =>
            activity.turnId === event.turnId && activity.call.id === event.callId
              ? {
                  ...activity,
                  state: event.result.success ? "completed" : "failed",
                  result: event.result,
                }
              : activity,
          ),
        }));
        break;
      case "approval_requested":
        set({
          pendingApproval: event.request,
          lastTurn: { turnId: event.turnId, state: "awaiting_approval", error: null },
        });
        break;
      case "approval_resolved":
        set((state) => ({
          pendingApproval:
            state.pendingApproval?.id === event.requestId ? null : state.pendingApproval,
          lastTurn: { turnId: event.turnId, state: "streaming", error: null },
        }));
        break;
      case "change_applied":
        set((state) => ({ changes: [...state.changes, event.changeSet] }));
        break;
      case "change_undone":
        set((state) => ({
          changes: state.changes.map((change) =>
            change.id === event.changeId ? { ...change, undone: true } : change,
          ),
        }));
        break;
      case "turn_completed":
        set((state) => ({
          activeTurnId: null,
          pendingApproval: null,
          usage: event.usage,
          lastTurn: { turnId: event.turnId, state: "completed", error: null },
          messages: state.messages.map((message) =>
            message.turnId === event.turnId
              ? toConversationMessage(event.message)
              : message,
          ),
        }));
        break;
      case "turn_failed":
        set((state) => ({
          activeTurnId: null,
          pendingApproval: null,
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
        }));
        break;
      case "turn_cancelled":
        set((state) => ({
          activeTurnId: null,
          pendingApproval: null,
          lastTurn: { turnId: event.turnId, state: "cancelled", error: null },
          messages: state.messages.map((message) =>
            message.turnId === event.turnId
              ? { ...message, status: "cancelled" as const }
              : message,
          ),
        }));
        break;
    }
  },

  clearError: () => set({ error: "" }),
}));
