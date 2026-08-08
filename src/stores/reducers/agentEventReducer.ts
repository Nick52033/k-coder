import type {
  AgentEvent,
  AgentActivityStatus,
  ApprovalRequest,
  ChangeSet,
  ConversationMessage,
  ChatMessage,
  TodoItem,
  TokenUsage,
  ToolActivity,
  ToolResult,
  ToolOutputStream,
  TimelineEventKind,
  TurnSnapshot,
  TurnTimelineItem,
  UserInputRequest,
} from "../../types/runtime";

const MAX_REASONING_SUMMARY_CHARS = 64 * 1024;

export interface ConversationProjectionState {
  messages: ConversationMessage[];
  turnTimeline: TurnTimelineItem[];
  turnUserMessageIds: Record<string, string>;
  activeTurnId: string | null;
  activeTurnThreadId: string | null;
  activeTurns: Record<string, string>;
  cancellingTurns: Record<string, string>;
  activityStatus: { turnId: string; status: AgentActivityStatus } | null;
  pendingApproval: ApprovalRequest | null;
  pendingApprovals: ApprovalRequest[];
  pendingUserInput: UserInputRequest | null;
  pendingUserInputs: UserInputRequest[];
  changes: ChangeSet[];
  error: string;
  lastTurn: TurnSnapshot | null;
  usage: TokenUsage | null;
  todos: Map<string, TodoItem[]>;
}

export interface ReducerHelpers {
  toConversationMessage: (message: ChatMessage, turnId?: string) => ConversationMessage;
  appendTimelineEvent: (
    timeline: TurnTimelineItem[],
    itemId: string,
    turnId: string,
    kind: TimelineEventKind,
    title: string,
    detail: string | null,
    durationMs?: number,
  ) => TurnTimelineItem[];
  insertApprovalRequestEvent: (
    timeline: TurnTimelineItem[],
    itemId: string,
    turnId: string,
    kind: TimelineEventKind,
    title: string,
    detail: string | null,
    toolCallId: string,
  ) => TurnTimelineItem[];
  insertApprovalResolutionEvent: (
    timeline: TurnTimelineItem[],
    itemId: string,
    turnId: string,
    kind: TimelineEventKind,
    title: string,
    detail: string | null,
    requestId: string,
  ) => TurnTimelineItem[];
  finishRunningTools: (
    timeline: TurnTimelineItem[],
    turnId: string,
    terminalState: "failed" | "cancelled",
    completedAtMs: number,
  ) => TurnTimelineItem[];
  approvalQueueState: (queue: ApprovalRequest[]) => {
    pendingApprovals: ApprovalRequest[];
    pendingApproval: ApprovalRequest | null;
  };
  userInputQueueState: (queue: UserInputRequest[]) => {
    pendingUserInputs: UserInputRequest[];
    pendingUserInput: UserInputRequest | null;
  };
  enqueueApproval: (queue: ApprovalRequest[], request: ApprovalRequest) => ApprovalRequest[];
  removeApproval: (queue: ApprovalRequest[], requestId: string) => ApprovalRequest[];
  enqueueUserInput: (queue: UserInputRequest[], request: UserInputRequest) => UserInputRequest[];
  removeUserInput: (queue: UserInputRequest[], requestId: string) => UserInputRequest[];
  appendToolOutput: (
    activity: ToolActivity,
    turnId: string,
    callId: string,
    stream: ToolOutputStream,
    cursor: number,
    delta: string,
  ) => ToolActivity;
  resultDurationMs: (result: ToolResult) => number | undefined;
}

export interface AgentEventReduction {
  state: Partial<ConversationProjectionState>;
  /** 需要外部处理的副作用标记 */
  sideEffects?: Array<"processQueue" | "refreshPlan">;
}

export function reduceAgentEvent(
  event: AgentEvent,
  state: ConversationProjectionState,
  helpers: ReducerHelpers,
): AgentEventReduction | null {
  const {
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
  } = helpers;

  switch (event.type) {
    case "turn_started": {
      const persistedUserMessage = event.userMessage
        ? toConversationMessage(event.userMessage, event.turnId)
        : null;
      const withUserMessage = persistedUserMessage
        && !state.messages.some((message) => message.id === persistedUserMessage.id)
          ? [...state.messages, persistedUserMessage]
          : state.messages;
      const latestUserMessage = persistedUserMessage
        ?? [...withUserMessage].reverse().find((message) => message.role === "user");
      return {
        state: {
          activeTurnId: event.turnId,
          activeTurnThreadId: event.threadId,
          lastTurn: { turnId: event.turnId, state: "streaming", error: null },
          pendingApproval: null,
          pendingApprovals: [],
          activityStatus: { turnId: event.turnId, status: "thinking" },
          turnUserMessageIds: state.turnUserMessageIds[event.turnId] || !latestUserMessage
            ? state.turnUserMessageIds
            : { ...state.turnUserMessageIds, [event.turnId]: latestUserMessage.id },
          messages: withUserMessage.some((message) =>
            message.turnId === event.turnId && message.role === "assistant")
            ? withUserMessage
            : [...withUserMessage, {
              id: `turn-${event.turnId}`,
              role: "assistant",
              text: "",
              createdAtMs: Date.now(),
              turnId: event.turnId,
              status: "streaming",
            }],
        },
      };
    }
    case "turn_steered":
      return {
        state: {
          messages: state.messages.some((message) => message.id === event.message.id)
            ? state.messages
            : [...state.messages, toConversationMessage(event.message, event.turnId)],
        },
      };
    case "turn_rejected":
      return {
        state: { error: event.message },
        sideEffects: ["processQueue"],
      };
    case "item_started":
      if (event.itemType === "agent_message") {
        const hasStreamingAssistant = state.messages.some((message) =>
          message.turnId === event.turnId
          && message.role === "assistant"
          && message.status === "streaming",
        );
        return hasStreamingAssistant
          ? {
              state: {
                messages: state.messages.map((message) => message.turnId === event.turnId
                  && message.role === "assistant"
                  && message.status === "streaming"
                  ? { ...message, id: event.itemId }
                  : message),
              },
            }
          : null;
      }
      return null;
    case "item_completed":
      return null;
    case "activity_status_changed":
      return {
        state: { activityStatus: { turnId: event.turnId, status: event.status } },
      };
    case "text_delta": {
      const lastItem = state.turnTimeline[state.turnTimeline.length - 1];
      const itemId = event.itemId ?? (lastItem?.type === "text" && lastItem.turnId === event.turnId
        ? lastItem.id
        : `legacy-stream-${event.turnId}-${crypto.randomUUID()}`);
      const existing = state.turnTimeline.findIndex((item) =>
        item.type === "text" && item.turnId === event.turnId && item.id === itemId,
      );
      const turnTimeline = existing >= 0
        ? state.turnTimeline.map((item, index) => index === existing && item.type === "text"
          ? { ...item, text: item.text + event.delta }
          : item)
        : [...state.turnTimeline, {
            type: "text" as const,
            id: itemId,
            turnId: event.turnId,
            text: event.delta,
          }];
      return {
        state: {
          activityStatus: { turnId: event.turnId, status: "responding" },
          turnTimeline,
        },
      };
    }
    case "reasoning_summary_delta": {
      const existing = state.turnTimeline.findIndex((item) =>
        item.type === "reasoning" && item.turnId === event.turnId && item.itemId === event.itemId,
      );
      if (existing >= 0) {
        return {
          state: {
            turnTimeline: state.turnTimeline.map((item, index) => index === existing && item.type === "reasoning"
              ? { ...item, summary: (item.summary + event.delta).slice(0, MAX_REASONING_SUMMARY_CHARS), complete: false }
              : item),
          },
        };
      }
      return {
        state: {
          turnTimeline: [...state.turnTimeline, {
            type: "reasoning" as const,
            itemId: event.itemId,
            turnId: event.turnId,
            summary: event.delta.slice(0, MAX_REASONING_SUMMARY_CHARS),
            complete: false,
          }],
        },
      };
    }
    case "reasoning_summary_completed": {
      const summary = event.summary.slice(0, MAX_REASONING_SUMMARY_CHARS);
      const exists = state.turnTimeline.some((item) =>
        item.type === "reasoning" && item.turnId === event.turnId && item.itemId === event.itemId,
      );
      return {
        state: {
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
        },
      };
    }
    case "usage_updated":
      return { state: { usage: event.usage } };
    case "context_compacted":
      return {
        state: {
          turnTimeline: appendTimelineEvent(
            state.turnTimeline,
            event.itemId,
            event.turnId,
            "compacted",
            event.automatic ? "已自动压缩上下文" : "已手动压缩上下文",
            `压缩了 ${event.compactedMessageCount} 条历史消息，保留 ${event.userConstraintCount} 项用户约束和 ${event.recentToolResultCount} 项近期工具结果`,
          ),
        },
      };
    case "tool_started": {
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
        state: {
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
        },
      };
    }
    case "tool_output_delta":
      return {
        state: {
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
        },
      };
    case "tool_completed": {
      const completedAtMs = Date.now();
      const sideEffects: Array<"refreshPlan"> = [];
      if (event.name === "update_plan") {
        sideEffects.push("refreshPlan");
      }
      return {
        state: {
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
        },
        sideEffects: sideEffects.length > 0 ? sideEffects : undefined,
      };
    }
    case "approval_requested": {
      if (event.request.autoApproved) {
        return {
          state: {
            turnTimeline: insertApprovalRequestEvent(
              state.turnTimeline,
              `approval-requested-${event.request.id}`,
              event.turnId,
              "approval_requested",
              "已自动批准操作",
              `${event.request.toolName} · ${event.request.reason}`,
              event.request.toolCallId,
            ),
          },
        };
      }
      const queue = enqueueApproval(state.pendingApprovals, event.request);
      return {
        state: {
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
        },
      };
    }
    case "approval_resolved": {
      const queue = removeApproval(state.pendingApprovals, event.requestId);
      return {
        state: {
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
        },
      };
    }
    case "user_input_requested": {
      const queue = enqueueUserInput(state.pendingUserInputs, event.request);
      return {
        state: {
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
        },
      };
    }
    case "user_input_resolved": {
      const queue = removeUserInput(state.pendingUserInputs, event.requestId);
      return {
        state: {
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
        },
      };
    }
    case "change_applied":
      return {
        state: {
          changes: state.changes.some((change) => change.id === event.changeSet.id)
            ? state.changes
            : [...state.changes, event.changeSet],
          turnTimeline: appendTimelineEvent(
            state.turnTimeline,
            `change-applied-${event.changeSet.id}`,
            event.turnId,
            "change_applied",
            "编辑了文件",
            event.changeSet.files.map((file) => file.path).join("、"),
          ),
        },
      };
    case "change_undone":
      return {
        state: {
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
        },
      };
    case "turn_completed": {
      const completedAtMs = event.completedAtMs ?? Date.now();
      const finalText = toConversationMessage(event.message, event.turnId).text;
      const completedTextItemIndex = state.turnTimeline.findIndex((item) =>
        item.type === "text" && item.turnId === event.turnId && item.id === event.message.id,
      );
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
      let turnTimeline = completedTextItemIndex >= 0
        ? state.turnTimeline.map((item, index) => index === completedTextItemIndex
          ? { type: "text" as const, id: event.message.id, turnId: event.turnId, text: finalText }
          : item)
        : lastTextItemIndex > lastToolItemIndex
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
        state: {
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
        },
      };
    }
    case "turn_failed": {
      const failedAtMs = event.completedAtMs ?? Date.now();
      return {
        state: {
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
        },
      };
    }
    case "turn_cancelled": {
      const cancelledAtMs = Date.now();
      return {
        state: {
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
        },
      };
    }
    case "todo_updated": {
      const newTodos = new Map(state.todos);
      newTodos.set(event.threadId, event.todos);
      return { state: { todos: newTodos } };
    }
    default:
      return null;
  }
}

export function reduceTurnLifecycle(
  event: AgentEvent,
  state: Pick<ConversationProjectionState, "activeTurns" | "cancellingTurns">,
): Partial<Pick<ConversationProjectionState, "activeTurns" | "cancellingTurns">> | null {
  const terminalEvent = ["turn_completed", "turn_failed", "turn_cancelled"].includes(event.type);
  if (event.type === "turn_started") {
    return {
      activeTurns: { ...state.activeTurns, [event.threadId]: event.turnId },
      cancellingTurns: state.cancellingTurns[event.threadId] === event.turnId
        ? state.cancellingTurns
        : withoutActiveTurn(state.cancellingTurns, event.threadId),
    };
  }
  if (terminalEvent) {
    return {
      activeTurns: state.activeTurns[event.threadId] === event.turnId
        ? withoutActiveTurn(state.activeTurns, event.threadId)
        : state.activeTurns,
      cancellingTurns: state.cancellingTurns[event.threadId] === event.turnId
        ? withoutActiveTurn(state.cancellingTurns, event.threadId)
        : state.cancellingTurns,
    };
  }
  return null;
}

function withoutActiveTurn(activeTurns: Record<string, string>, threadId: string) {
  if (!(threadId in activeTurns)) return activeTurns;
  const next = { ...activeTurns };
  delete next[threadId];
  return next;
}
