import type {
  ApprovalSnapshot,
  ChangeSet,
  ChatMessage,
  ConversationMessage,
  ThreadItem,
  ThreadTurn,
  TurnTimelineItem,
  UserInputSnapshot,
} from "../../types/runtime";

export interface ProjectedThreadHistory {
  messages: ConversationMessage[];
  turnTimeline: TurnTimelineItem[];
  turnUserMessageIds: Record<string, string>;
  approvals: ApprovalSnapshot[];
  userInputs: UserInputSnapshot[];
  changes: ChangeSet[];
}

export function toConversationMessage(message: ChatMessage, turnId?: string): ConversationMessage {
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

export function reconcileConversationMessages(
  messages: ConversationMessage[],
  timeline: TurnTimelineItem[],
) {
  const textTurnIds = new Map<string, string>();
  for (const item of timeline) {
    if (item.type === "text") textTurnIds.set(item.id, item.turnId);
  }

  const reconciled: ConversationMessage[] = [];
  const messageIndexes = new Map<string, number>();
  for (const message of messages) {
    const inferredTurnId = message.role === "assistant"
      ? message.turnId ?? textTurnIds.get(message.id)
      : message.turnId;
    const normalized = inferredTurnId === message.turnId
      ? message
      : { ...message, turnId: inferredTurnId };
    const key = message.role === "assistant" && inferredTurnId
      ? `assistant-turn:${inferredTurnId}`
      : `${message.role}-message:${message.id}`;
    const existingIndex = messageIndexes.get(key);
    if (existingIndex === undefined) {
      messageIndexes.set(key, reconciled.length);
      reconciled.push(normalized);
      continue;
    }
    reconciled[existingIndex] = preferredConversationMessage(reconciled[existingIndex], normalized);
  }
  return reconciled;
}

function preferredConversationMessage(
  current: ConversationMessage,
  candidate: ConversationMessage,
) {
  const currentRank = conversationMessageRank(current);
  const candidateRank = conversationMessageRank(candidate);
  if (candidateRank !== currentRank) return candidateRank > currentRank ? candidate : current;
  return candidate.createdAtMs >= current.createdAtMs ? candidate : current;
}

function conversationMessageRank(message: ConversationMessage) {
  const lifecycleRank = message.status === "streaming"
    ? 0
    : message.status === "failed" || message.status === "cancelled"
      ? 1
      : 2;
  const hasContent = Boolean(message.text || message.attachments?.length);
  return lifecycleRank * 2 + Number(hasContent);
}

export function normalizeApprovalTimeline(
  timeline: TurnTimelineItem[],
  approvals: ApprovalSnapshot[],
) {
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

export function projectHistoryTurns(
  turns: ThreadTurn[],
  unscopedItems: ThreadItem[] = [],
): ProjectedThreadHistory {
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

export function timelineItemKey(item: TurnTimelineItem) {
  if (item.type === "text") return `text:${item.turnId}:${item.id}`;
  if (item.type === "reasoning") return `reasoning:${item.turnId}:${item.itemId}`;
  if (item.type === "tool") return `tool:${item.activity.turnId}:${item.activity.call.id}`;
  return `event:${item.turnId}:${item.itemId}`;
}

export function prependUnique<T>(older: T[], current: T[], key: (item: T) => string) {
  const seen = new Set<string>();
  return [...older, ...current].filter((item) => {
    const value = key(item);
    if (seen.has(value)) return false;
    seen.add(value);
    return true;
  });
}

function moveTimelineItemBeforeTool(
  timeline: TurnTimelineItem[],
  itemId: string,
  turnId: string,
  toolCallId: string,
) {
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

function moveTimelineItemAfterRequest(
  timeline: TurnTimelineItem[],
  itemId: string,
  requestItemId: string,
) {
  const itemIndex = timeline.findIndex((item) => item.type === "event" && item.itemId === itemId);
  if (itemIndex < 0) return timeline;
  const item = timeline[itemIndex];
  const withoutItem = [...timeline.slice(0, itemIndex), ...timeline.slice(itemIndex + 1)];
  const requestIndex = withoutItem.findIndex((entry) => entry.type === "event" && entry.itemId === requestItemId);
  if (requestIndex < 0) return timeline;
  return [...withoutItem.slice(0, requestIndex + 1), item, ...withoutItem.slice(requestIndex + 1)];
}
