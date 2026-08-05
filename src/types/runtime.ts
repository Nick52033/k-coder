export interface RuntimeStatus {
  ready: boolean;
  phase: string;
  version: string;
  uptimeSeconds: number;
  capabilities: string[];
}

export interface ProjectRecord { id: string; name: string; path: string; trusted: boolean; lastOpenedAtMs: number; }
export interface WorkspaceState { current: ProjectRecord; recent: ProjectRecord[]; }
export interface FileEntry { name: string; path: string; isDirectory: boolean; size: number | null; modifiedAtMs: number | null; }
export interface FilePreview { path: string; name: string; language: string; content: string | null; dataUrl: string | null; size: number; truncated: boolean; editable: boolean; contentHash: string | null; }
export interface SaveWorkspaceFileRequest { path: string; content: string; expectedHash: string; }
export type OcrStatus = "processing" | "complete" | "failed";
export interface AttachmentContent {
  path: string;
  name: string;
  kind: "image" | "document";
  content: string;
  size: number;
  truncated: boolean;
  ocrStatus?: OcrStatus;
  ocrText?: string;
  ocrLineCount?: number;
  ocrDurationMs?: number;
  ocrError?: string;
}
export interface ImageAttachment { name: string; dataUrl: string; ocrText?: string; }
export interface GitFileStatus { path: string; indexStatus: string; worktreeStatus: string; }
export interface GitStatusView { isRepository: boolean; branch: string | null; upstream: string | null; ahead: number; behind: number; files: GitFileStatus[]; }
export interface GitBranchView { current: string | null; branches: string[]; }
export interface UsageSummary { inputTokens: number; outputTokens: number; totalTokens: number; providerCalls: number; }
export interface ProviderConnectionTest { connected: boolean; latencyMs: number; usage: TokenUsage | null; }
export interface InstructionSource { path: string; scope: string; priority: number; bytes: number; }
export interface SkillDiagnostic { name: string; description: string; path: string; scope: string; risk: ToolRisk; triggers: string[]; enabled: boolean; }
export interface CredentialDiagnostic { name: string; configured: boolean; }
export interface McpDiagnostic { id: string; transport: string; enabled: boolean; state: string; toolCount: number; credentials: CredentialDiagnostic[]; error: string | null; }
export interface HookDiagnostic { id: string; phase: string; tool: string; enabled: boolean; }
export interface ExtensionAudit { timestampMs: number; event: string; kind: string; id: string; success: boolean; detail: string; }
export interface ExtensionOverview { schemaVersion: number; configPaths: string[]; instructions: InstructionSource[]; skills: SkillDiagnostic[]; mcpServers: McpDiagnostic[]; hooks: HookDiagnostic[]; audit: ExtensionAudit[]; error: string | null; }

export type SubagentState = "queued" | "running" | "blocked" | "completed" | "failed" | "cancelled" | "timed_out";
export interface CreateSubagentRequest {
  parentThreadId: string;
  task: string;
  label?: string;
  capabilities?: string[];
  tokenBudget?: number;
  timeoutMs?: number;
}
export interface SubagentView {
  schemaVersion: number;
  id: string;
  parentAgentId: string | null;
  parentThreadId: string;
  threadId: string;
  label: string;
  task: string;
  state: SubagentState;
  depth: number;
  workspaceRoot: string;
  capabilities: string[];
  tokenBudget: number | null;
  tokensUsed: number;
  timeoutMs: number;
  createdAtMs: number;
  updatedAtMs: number;
  summary: string | null;
  error: string | null;
}

export type CommandMode = "foreground" | "background";
export type CommandState =
  | { state: "running" }
  | { state: "exited"; code: number }
  | { state: "timed_out" }
  | { state: "cancelled" }
  | { state: "failed"; message: string };

export interface StartCommandRequest {
  program: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
  mode: CommandMode;
  timeoutMs?: number;
  bufferBytes?: number;
}

export interface CommandSessionView {
  id: string;
  mode: CommandMode;
  state: CommandState;
  startedAtMs: number;
  finishedAtMs: number | null;
  nextCursor: number;
  oldestCursor: number;
  outputTruncated: boolean;
}

export interface CommandOutputChunk {
  cursor: number;
  stream: "stdout" | "stderr";
  text: string;
}

export interface CommandOutputPage {
  chunks: CommandOutputChunk[];
  nextCursor: number;
  oldestCursor: number;
  truncatedBeforeCursor: boolean;
}

export interface StartPtyRequest {
  program: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
  rows: number;
  cols: number;
  bufferBytes?: number;
}

export interface PtySessionView {
  id: string;
  state: CommandState;
  startedAtMs: number;
  finishedAtMs: number | null;
  rows: number;
  cols: number;
  nextCursor: number;
  oldestCursor: number;
  outputTruncated: boolean;
}

export interface PtyOutputChunk {
  cursor: number;
  text: string;
}

export interface PtyOutputPage {
  chunks: PtyOutputChunk[];
  nextCursor: number;
  oldestCursor: number;
  truncatedBeforeCursor: boolean;
}

export type MessageRole = "user" | "assistant" | "system";
export type TurnState =
  | "queued"
  | "streaming"
  | "awaiting_approval"
  | "running_tool"
  | "completed"
  | "failed"
  | "cancelled";

/// Turn 的语义阶段（对应后端 TurnPhase 枚举）
export type TurnPhase =
  | "idle"
  | "exploring"
  | "planning"
  | "executing"
  | "awaiting_input"
  | "complete"
  | "failed"
  | "cancelled";

export interface TextContentBlock {
  type: "text";
  text: string;
}

export interface ImageContentBlock {
  type: "image";
  name: string;
  dataUrl: string;
}

export interface ContextContentBlock {
  type: "context";
  text: string;
}

export type ContentBlock = TextContentBlock | ImageContentBlock | ContextContentBlock;

export interface ChatMessage {
  schemaVersion: number;
  id: string;
  role: MessageRole;
  content: ContentBlock[];
  createdAtMs: number;
}

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
}

export interface ContextCompactionSummary {
  contractVersion: number;
  summary: string;
  userConstraints: string[];
  recentToolResults: unknown[];
  compactedMessageCount: number;
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
  messageTurnIds: Record<string, string>;
  turnUserMessageIds: Record<string, string>;
  lastTurn: TurnSnapshot | null;
  toolActivities: ToolActivity[];
  turnTimeline: TurnTimelineItem[];
  approvals: ApprovalSnapshot[];
  userInputs: UserInputSnapshot[];
  changes: ChangeSet[];
  todos: TodoItem[];
  lastUsage: TokenUsage | null;
}

export type FileOperation = "add" | "modify" | "delete" | "move";
export type ToolRisk = "read" | "write" | "delete" | "external";
export type ApprovalMode = "ask" | "full_access";
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
  autoApproved: boolean;
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
  /** 授权作用域：`once`=仅本次调用；`session`=本会话内同类操作放行。 */
  scope?: "once" | "session";
  /** 拒绝时附带给模型的反馈文本（可选）。 */
  feedback?: string;
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
  state: "pending" | "running" | "completed" | "failed" | "cancelled";
  result: ToolResult | null;
  outputChunks?: ToolOutputDelta[];
  startedAtMs?: number;
  completedAtMs?: number;
  durationMs?: number;
}

export type AgentActivityStatus = "thinking" | "responding" | "running_tool" | "awaiting_approval" | "finalizing";
export type ToolOutputStream = "stdout" | "stderr";
export interface ToolOutputDelta { stream: ToolOutputStream; cursor: number; text: string; }

export type TimelineEventKind =
  | "provider_context"
  | "usage"
  | "compacted"
  | "approval_requested"
  | "approval_resolved"
  | "change_applied"
  | "change_undone"
  | "user_input_requested"
  | "user_input_resolved"
  | "todo_updated"
  | "turn_completed"
  | "turn_failed"
  | "turn_cancelled";

export type TurnTimelineItem =
  | { type: "text"; id: string; turnId: string; text: string }
  | { type: "reasoning"; itemId: string; turnId: string; summary: string; complete?: boolean }
  | { type: "tool"; activity: ToolActivity }
  | { type: "event"; itemId: string; turnId: string; kind: TimelineEventKind; title: string; detail: string | null; durationMs?: number };

export type ProviderKind = "open_ai_compatible";
export type ProviderTransport =
  | "open_ai_chat_completions"
  | "open_ai_responses"
  | "anthropic_messages"
  | "google_gemini";

export interface ProviderModelConfig {
  id: string;
  displayName: string;
  contextWindow: number;
  maxOutputTokens?: number;
  supportsVision?: boolean;
  fallback: boolean;
}

export interface ProviderEndpointConfig {
  id: string;
  name: string;
  baseUrl: string;
  enabled: boolean;
}

export interface ProviderConfigView {
  schemaVersion: number;
  id: string;
  kind: ProviderKind;
  transport: ProviderTransport;
  name: string;
  baseUrl: string;
  model: string;
  models: ProviderModelConfig[];
  endpoints: ProviderEndpointConfig[];
  hasApiKey: boolean;
}

export interface ProviderCatalogView {
  schemaVersion: number;
  activeProviderId: string | null;
  providers: ProviderConfigView[];
}

export interface SaveProviderConfigRequest {
  id: string;
  kind: ProviderKind;
  transport: ProviderTransport;
  name: string;
  baseUrl: string;
  model: string;
  models: ProviderModelConfig[];
  endpoints: ProviderEndpointConfig[];
  apiKey?: string;
  activate: boolean;
}

export type PlanStepState = "pending" | "in_progress" | "completed" | "failed" | "skipped";
export interface PlanStep { id: string; step: string; status: PlanStepState; detail: string | null; }
export interface PlanView { schemaVersion: number; threadId: string; revision: number; updatedAtMs: number; steps: PlanStep[]; }
export interface PlanUpdateRequest { threadId: string; steps: Array<{ id?: string; step: string; status: PlanStepState; detail?: string }>; }

export type GoalState = "active" | "paused" | "blocked" | "completed" | "budget_exhausted";
export interface GoalView {
  schemaVersion: number; id: string; threadId: string; objective: string; state: GoalState;
  tokenBudget: number | null; tokensUsed: number; timeBudgetMs: number; elapsedMs: number;
  reason: string | null; createdAtMs: number; updatedAtMs: number; revision: number;
}
export interface CreateGoalRequest { threadId: string; objective: string; tokenBudget: number | null; timeBudgetMs: number; }

export interface SearchResult { path: string; line: number; column: number; preview: string; score: number; }
export interface MemorySettings { enabled: boolean; }
export interface MemoryView { schemaVersion: number; id: string; content: string; source: string; expiresAtMs: number; createdAtMs: number; updatedAtMs: number; deleted: boolean; revision: number; }
export interface MemoryUpsertRequest { id?: string; content: string; source: string; retentionDays: number; }
export interface BrowserSettings { enabled: boolean; allowLocalhost: boolean; }
export interface BrowserAuditEvent { timestampMs: number; action: string; target: string; success: boolean; detail: string; }
export interface BrowserArtifact { id: string; name: string; mediaType: string; sizeBytes: number; createdAtMs: number; }
export interface DocumentContent { path: string; name: string; mediaType: string; content: string; sourceBytes: number; extractedBytes: number; truncated: boolean; }
export interface MetricsSnapshot { providerCalls: number; providerFailures: number; averageProviderLatencyMs: number; inputTokens: number; outputTokens: number; toolCalls: number; toolSuccessRate: number; fallbackCount: number; completedTasks: number; failedTasks: number; estimatedCostUsd: number | null; }
export interface EvaluationReport { total: number; passed: number; passRate: number; failures: string[]; }

export interface TurnOutcome {
  schemaVersion: number;
  threadId: string;
  turnId: string;
  state: TurnState;
  error: string | null;
  startedAtMs: number;
  completedAtMs: number;
  durationMs: number;
}

interface EventBase {
  schemaVersion: number;
  threadId: string;
  turnId: string;
  phase: TurnPhase;
}

export type TodoStatus = "pending" | "in_progress" | "completed";

export interface TodoItem {
  content: string;
  status: TodoStatus;
  activeForm: string;
}

export type AgentEvent =
  | (EventBase & { type: "turn_started" })
  | (EventBase & { type: "activity_status_changed"; status: AgentActivityStatus })
  | (EventBase & { type: "text_delta"; delta: string })
  | (EventBase & { type: "reasoning_summary_delta"; itemId: string; delta: string })
  | (EventBase & { type: "reasoning_summary_completed"; itemId: string; summary: string })
  | (EventBase & { type: "usage_updated"; usage: TokenUsage })
  | (EventBase & {
      type: "context_compacted";
      itemId: string;
      automatic: boolean;
      compactedMessageCount: number;
      userConstraintCount: number;
      recentToolResultCount: number;
    })
  | (EventBase & { type: "tool_started"; call: ToolCall })
  | (EventBase & {
      type: "tool_output_delta";
      callId: string;
      stream: ToolOutputStream;
      cursor: number;
      delta: string;
    })
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
      startedAtMs: number;
      completedAtMs: number;
      durationMs: number;
    })
  | (EventBase & { type: "turn_failed"; message: string; startedAtMs: number; completedAtMs: number; durationMs: number })
  | (EventBase & { type: "turn_cancelled"; startedAtMs: number; completedAtMs: number; durationMs: number })
  | (EventBase & { type: "user_input_requested"; request: UserInputRequest })
  | (EventBase & {
      type: "user_input_resolved";
      requestId: string;
      resolution: UserInputResolution;
    })
  | (EventBase & { type: "todo_updated"; todos: TodoItem[] });

export interface ConversationMessage {
  id: string;
  role: MessageRole;
  text: string;
  attachments?: ImageAttachment[];
  createdAtMs: number;
  turnId?: string;
  status?: "streaming" | "failed" | "cancelled";
}

export type AgentMode = "craft" | "ask" | "plan";
export type ReasoningEffort = "off" | "minimal" | "low" | "medium" | "high" | "x_high";

export interface UserInputQuestion {
  question: string;
  options: string[];
}

export interface UserInputRequest {
  id: string;
  threadId: string;
  turnId: string;
  toolCallId: string;
  questions: UserInputQuestion[];
  createdAtMs: number;
  expiresAtMs: number;
}

export type UserInputAction = "answered" | "skipped" | "cancelled";

export interface UserInputAnswer {
  question: string;
  answer: string;
}

export interface UserInputResolution {
  action: UserInputAction;
  answers: UserInputAnswer[];
}

export interface UserInputSnapshot {
  request: UserInputRequest;
  resolution: UserInputResolution | null;
}

export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface LogRecord {
  timestampMs: number;
  level: string;
  event: string;
  fields: unknown;
}

export interface LogQueryResult {
  records: LogRecord[];
  total: number;
}

export interface LogQuery {
  limit?: number;
  level?: LogLevel;
  event?: string;
  afterTimestampMs?: number;
}
