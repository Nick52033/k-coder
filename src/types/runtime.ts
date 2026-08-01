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
export interface FilePreview { path: string; name: string; language: string; content: string | null; dataUrl: string | null; size: number; truncated: boolean; }
export interface AttachmentContent { path: string; name: string; kind: "image" | "document"; content: string; size: number; truncated: boolean; }
export interface ImageAttachment { name: string; dataUrl: string; }
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
  tokenBudget: number;
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

export interface ImageContentBlock {
  type: "image";
  name: string;
  dataUrl: string;
}

export type ContentBlock = TextContentBlock | ImageContentBlock;

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
  state: "running" | "completed" | "failed";
  result: ToolResult | null;
}

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
  tokenBudget: number; tokensUsed: number; timeBudgetMs: number; elapsedMs: number;
  reason: string | null; createdAtMs: number; updatedAtMs: number; revision: number;
}
export interface CreateGoalRequest { threadId: string; objective: string; tokenBudget: number; timeBudgetMs: number; }

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
