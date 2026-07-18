export interface RuntimeStatus {
  ready: boolean;
  phase: string;
  version: string;
  uptimeSeconds: number;
  capabilities: string[];
}

export type MessageRole = "user" | "assistant";
export type TurnState =
  | "queued"
  | "streaming"
  | "awaiting_approval"
  | "running_tool"
  | "completed"
  | "failed"
  | "cancelled";

export interface TextContentBlock {
  type: "text";
  text: string;
}

export interface ChatMessage {
  schemaVersion: number;
  id: string;
  role: MessageRole;
  content: TextContentBlock[];
  createdAtMs: number;
}

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
}

export interface ThreadSummary {
  schemaVersion: number;
  id: string;
  title: string;
  createdAtMs: number;
  updatedAtMs: number;
  archived: boolean;
}

export interface TurnSnapshot {
  turnId: string;
  state: TurnState;
  error: string | null;
}

export interface ThreadDetail {
  schemaVersion: number;
  summary: ThreadSummary;
  messages: ChatMessage[];
  lastTurn: TurnSnapshot | null;
}

export type ProviderKind = "open_ai_compatible";

export interface ProviderConfigView {
  schemaVersion: number;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
}

export interface SaveProviderConfigRequest {
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey?: string;
}

export interface TurnOutcome {
  schemaVersion: number;
  threadId: string;
  turnId: string;
  state: TurnState;
  error: string | null;
}

interface EventBase {
  schemaVersion: number;
  threadId: string;
  turnId: string;
}

export type AgentEvent =
  | (EventBase & { type: "turn_started" })
  | (EventBase & { type: "text_delta"; delta: string })
  | (EventBase & { type: "usage_updated"; usage: TokenUsage })
  | (EventBase & {
      type: "turn_completed";
      message: ChatMessage;
      usage: TokenUsage | null;
    })
  | (EventBase & { type: "turn_failed"; message: string })
  | (EventBase & { type: "turn_cancelled" });

export interface ConversationMessage {
  id: string;
  role: MessageRole;
  text: string;
  createdAtMs: number;
  turnId?: string;
  status?: "streaming" | "failed" | "cancelled";
}
