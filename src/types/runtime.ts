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
  toolActivities: ToolActivity[];
  approvals: ApprovalSnapshot[];
  changes: ChangeSet[];
}

export type FileOperation = "add" | "modify" | "delete" | "move";
export type ToolRisk = "read" | "write" | "delete" | "external";
export type ApprovalAction = "approved" | "rejected" | "timed_out" | "cancelled";

export interface PatchFilePreview {
  path: string;
  destinationPath: string | null;
  operation: FileOperation;
  beforeHash: string | null;
  afterHash: string | null;
  beforeContent: string | null;
  afterContent: string | null;
  unifiedDiff: string;
}

export interface PatchPreview {
  patch: string;
  files: PatchFilePreview[];
  totalSnapshotBytes: number;
}

export interface ExpectedFileHash {
  path: string;
  beforeHash: string | null;
}

export interface ApprovalRequest {
  id: string;
  threadId: string;
  turnId: string;
  toolCallId: string;
  toolName: string;
  reason: string;
  risk: ToolRisk;
  arguments: Record<string, unknown>;
  preview: PatchPreview | null;
  createdAtMs: number;
  expiresAtMs: number;
}

export interface ApprovalResolution {
  action: ApprovalAction;
  patch: string | null;
  selectedPaths: string[];
  expectedHashes: ExpectedFileHash[];
}

export interface ApprovalSnapshot {
  request: ApprovalRequest;
  resolution: ApprovalResolution | null;
}

export interface ChangeFileSnapshot extends PatchFilePreview {}

export interface ChangeSet {
  id: string;
  threadId: string;
  turnId: string;
  toolCallId: string;
  createdAtMs: number;
  files: ChangeFileSnapshot[];
  undone: boolean;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
  metadata: Record<string, unknown>;
}

export interface ToolResult {
  success: boolean;
  output: string;
  metadata: Record<string, unknown>;
}

export interface ToolActivity {
  turnId: string;
  call: ToolCall;
  state: "running" | "completed" | "failed";
  result: ToolResult | null;
}

export type ProviderKind = "open_ai_compatible";
export type ProviderTransport =
  | "open_ai_chat_completions"
  | "open_ai_responses"
  | "anthropic_messages"
  | "google_gemini";

export interface ProviderConfigView {
  schemaVersion: number;
  kind: ProviderKind;
  transport: ProviderTransport;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
}

export interface SaveProviderConfigRequest {
  kind: ProviderKind;
  transport: ProviderTransport;
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
  | (EventBase & { type: "tool_started"; call: ToolCall })
  | (EventBase & {
      type: "tool_completed";
      callId: string;
      name: string;
      result: ToolResult;
    })
  | (EventBase & { type: "approval_requested"; request: ApprovalRequest })
  | (EventBase & {
      type: "approval_resolved";
      requestId: string;
      resolution: ApprovalResolution;
    })
  | (EventBase & { type: "change_applied"; changeSet: ChangeSet })
  | (EventBase & { type: "change_undone"; changeId: string })
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
