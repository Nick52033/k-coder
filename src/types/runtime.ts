export interface RuntimeStatus {
  ready: boolean;
  phase: string;
  version: string;
  uptimeSeconds: number;
  capabilities: string[];
}

export interface CommandError<TDetails = unknown> {
  code: string;
  message: string;
  details?: TDetails;
}

export interface ProjectRecord { id: string; name: string; path: string; trusted: boolean; lastOpenedAtMs: number; }
export interface WorkspaceState { current: ProjectRecord; recent: ProjectRecord[]; }
export interface KnowledgeSettings { enabled: boolean; autoSearch: boolean; maxResults: number; maxChunkTokens: number; knowledgeBudgetPercent: number; semanticEnabled: boolean; embeddingProvider: string; embeddingModel: string; embeddingDimension: number; embeddingConfigured: boolean; embeddingStatus: string; }
export interface EmbeddingSettings { provider: string; endpoint: string; model: string; semanticEnabled: boolean; encodingFormat: string; batchSize: number; timeoutMs: number; maxVectorScanChunks: number; modelMaxInputTokens: number; vectorDimension: number; embeddingConfigured: boolean; embeddingStatus: string; }
export interface KnowledgeCollection { id: string; name: string; scope: string; scopeKey: string; enabled: boolean; sourceCount: number; indexedChunkCount: number; updatedAtMs: number; }
export interface UpsertKnowledgeCollectionRequest { id?: string | null; name: string; scope?: string; enabled: boolean; }
export interface AddKnowledgeSourceRequest { collectionId: string; workspaceRelativePath: string; }
export interface KnowledgeSource { sourceId: string; relativePath: string; sizeBytes: number; contentHashPrefix: string | null; activeRevisionId: string | null; activeEmbeddingModel: string | null; activeEmbeddingDimension: number; activeEmbeddingEncodingFormat: string; embeddingStatus: string; state: string; chunkCount: number; lastIndexedAtMs: number | null; lastErrorCode: string | null; initialJobId: string | null; }
export interface KnowledgeIndexJob { jobId: string; sourceId: string; state: string; stage: string; embeddingMode: string; processedBytes: number; totalBytes: number; processedChunks: number; totalChunks: number; embeddingRequests: number; retryCount: number; lastHttpStatus: number | null; chunkCount: number; vectorCount: number; errorCode: string | null; createdAtMs: number; completedAtMs: number | null; }
export interface KnowledgeSearchResult { citationId: string; title: string; path: string; locator: string; preview: string; revision: string; score: number; lexicalRank: number; semanticRank: number | null; }
export interface KnowledgeSearchResponse { success: boolean; results: KnowledgeSearchResult[]; metadata: Record<string, unknown>; }
export interface KnowledgeCitation { citationId: string; path: string; locator: string; text: string; revision: string; isCurrentRevision: boolean; }
export interface SetEmbeddingSettingsRequest { semanticEnabled: boolean; batchSize: number; timeoutMs: number; maxVectorScanChunks: number; }
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
export interface DailyUsageSummary {
  date: string;
  providerCalls: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
}
export interface UsageTokenBreakdown {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedInputTokens: number | null;
  uncachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  reasoningOutputTokens: number | null;
  replyOutputTokens: number | null;
  cacheHitRate: number | null;
  estimatedCostUsd: number | null;
}
export interface ModelUsageSummary extends UsageTokenBreakdown {
  provider: string | null;
  model: string | null;
  providerCalls: number;
}
export interface UsageSummary extends UsageTokenBreakdown {
  schemaVersion: number;
  trendDays: number;
  providerCalls: number;
  daily: DailyUsageSummary[];
  models: ModelUsageSummary[];
}
export interface ProviderConnectionTest { connected: boolean; latencyMs: number; usage: TokenUsage | null; }
export interface InstructionSource { path: string; scope: string; priority: number; bytes: number; }
export type SkillCategory = "requirements_planning" | "development_delivery" | "quality_review" | "testing" | "design_experience" | "data_documents" | "observability" | "integration_automation" | "extension_platform" | "other";
export interface SkillDiagnostic { name: string; description: string; path: string; scope: string; risk: ToolRisk; category: SkillCategory; triggers: string[]; enabled: boolean; managedByRobot: boolean; }
export interface CredentialDiagnostic { name: string; configured: boolean; }
export interface McpDiagnostic { id: string; transport: string; enabled: boolean; state: string; toolCount: number; credentials: CredentialDiagnostic[]; error: string | null; }
export interface HookDiagnostic { id: string; phase: string; tool: string; enabled: boolean; }
export interface ExtensionAudit { timestampMs: number; event: string; kind: string; id: string; success: boolean; detail: string; }
export interface ExtensionOverview { schemaVersion: number; configPaths: string[]; instructions: InstructionSource[]; skills: SkillDiagnostic[]; mcpServers: McpDiagnostic[]; hooks: HookDiagnostic[]; audit: ExtensionAudit[]; error: string | null; }
export interface UserRule { id: string; title: string; content: string; createdAtMs: number; updatedAtMs: number; }
export interface SaveUserRuleRequest { id: string | null; title: string; content: string; }
export interface UserRulesView { schemaVersion: number; path: string; rules: UserRule[]; error: string | null; }
export interface McpConfigDocumentView { scope: "global" | "project"; path: string; exists: boolean; content: string; error: string | null; }
export interface McpConfigView { schemaVersion: number; global: McpConfigDocumentView; project: McpConfigDocumentView; overview: ExtensionOverview; }
export type PluginState = "disabled" | "loaded" | "degraded" | "blocked" | "invalid";
export interface PluginComponentSummary {
  skillCount: number;
  mcpServerCount: number;
  mcpToolCount: number;
  unsupportedCount: number;
}
export interface PluginDiagnostic {
  id: string;
  name: string;
  version: string;
  description: string;
  path: string;
  enabled: boolean;
  state: PluginState;
  deletable: boolean;
  components: PluginComponentSummary;
  warnings: string[];
  error: string | null;
}
export interface PluginOverview {
  schemaVersion: number;
  rootPath: string;
  plugins: PluginDiagnostic[];
  error: string | null;
}

export type SubagentState = "queued" | "running" | "blocked" | "completed" | "failed" | "cancelled" | "timed_out";
export interface CreateSubagentRequest {
  parentThreadId: string;
  task: string;
  label?: string;
  capabilities?: string[];
  tokenBudget?: number;
  timeoutMs?: number;
  /** `"none"` (default) | `"all"` | positive integer of parent turns to replay. */
  forkTurns?: string;
  /** Omit for a direct child; set to a subagent id to delegate one level deeper. */
  parentAgentId?: string;
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
  /** Canonical path in the delegation tree, e.g. `/root/1a2b3c`. */
  agentPath: string;
  /** How the thread was seeded from the parent: `null`, `"all"`, or `"<n>"`. */
  forkMode: string | null;
  turnCount: number;
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
  recentUserMessages?: string[];
  currentUserRequest?: string;
  importantToolObservations?: string[];
  recentToolResults: unknown[];
  compactedMessageCount: number;
  estimatedBeforeTokens?: number;
  estimatedAfterTokens?: number;
}

export interface ThreadSummary {
  schemaVersion: number;
  id: string;
  title: string;
  createdAtMs: number;
  updatedAtMs: number;
  archived: boolean;
  inProject?: boolean;
  workspacePath?: string | null;
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
  contextUsage: TokenUsage | null;
}

export type HistorySortDirection = "asc" | "desc";
export type TurnItemsView = "not_loaded" | "summary" | "full";
export type AgentMessagePhase = "commentary" | "final_answer";

interface ThreadItemBase {
  schemaVersion: number;
  id: string;
  turnId: string | null;
  status: AgentItemStatus | null;
  startedAtMs: number | null;
  completedAtMs: number | null;
  timelineItems: TurnTimelineItem[];
}

export type ThreadItem = ThreadItemBase & (
  | { type: "user_message"; message: ChatMessage }
  | { type: "agent_message"; message: ChatMessage; phase: AgentMessagePhase }
  | { type: "reasoning"; summary: string }
  | { type: "tool"; activity: ToolActivity }
  | { type: "approval"; approval: ApprovalSnapshot }
  | { type: "user_input"; userInput: UserInputSnapshot }
  | { type: "change"; changeSet: ChangeSet }
  | {
      type: "context_compaction";
      automatic: boolean;
      compactedMessageCount: number;
      userConstraintCount: number;
      recentToolResultCount: number;
      recentUserMessageCount?: number;
    }
  | { type: "event" }
);

export interface ThreadTurn {
  schemaVersion: number;
  id: string;
  userMessageId: string | null;
  state: TurnState;
  error: string | null;
  startedAtMs: number | null;
  completedAtMs: number | null;
  durationMs: number | null;
  itemsView: TurnItemsView;
  items: ThreadItem[];
}

export interface ThreadTurnsPage {
  data: ThreadTurn[];
  nextCursor: string | null;
  backwardsCursor: string | null;
}

export interface ThreadItemEntry {
  turnId: string | null;
  item: ThreadItem;
}

export interface ThreadItemsPage {
  data: ThreadItemEntry[];
  nextCursor: string | null;
  backwardsCursor: string | null;
}

export interface ThreadHistorySnapshot {
  schemaVersion: number;
  summary: ThreadSummary;
  lastTurn: TurnSnapshot | null;
  todos: TodoItem[];
  lastUsage: TokenUsage | null;
  contextUsage: TokenUsage | null;
  turns: ThreadTurnsPage;
  unscopedItems: ThreadItem[];
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
  | "deep_seek_chat_completions"
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

export type WorkflowSkillBindingKind = "skill" | "plugin_skill";
export interface WorkflowSkillBindingView {
  kind: WorkflowSkillBindingKind;
  declaration: string;
  skillId: string;
  pluginId: string | null;
  fallbackSkillId: string | null;
}
export type WorkflowSkillReadinessStatus =
  | "builtin"
  | "global"
  | "project"
  | "plugin"
  | "builtin_fallback"
  | "disabled"
  | "missing"
  | "oversized"
  | "limit_exceeded";
export interface WorkflowSkillBindingReadinessView {
  binding: WorkflowSkillBindingView;
  status: WorkflowSkillReadinessStatus;
  resolvedSkillId: string | null;
  resolvedScope: string | null;
  bodySha256: string | null;
  bodyBytes: number | null;
  blocker: string | null;
}
export interface WorkflowSkillReadinessBlocker {
  nodeId: string | null;
  declaration: string | null;
  status: WorkflowSkillReadinessStatus;
  message: string;
}
export interface WorkflowNodeSkillReadinessView {
  nodeId: string;
  ready: boolean;
  declarationCount: number;
  uniqueBodyCount: number;
  totalBodyBytes: number;
  bindings: WorkflowSkillBindingReadinessView[];
  blockers: WorkflowSkillReadinessBlocker[];
}
export interface WorkflowSkillReadinessView {
  schemaVersion: number;
  workflowId: string;
  definitionVersion: number;
  ready: boolean;
  skillCount: number;
  localSkillCount: number;
  pluginSkillCount: number;
  blockerCount: number;
  bindings: WorkflowSkillBindingReadinessView[];
  nodes: WorkflowNodeSkillReadinessView[];
  blockers: WorkflowSkillReadinessBlocker[];
}
export type WorkflowSkillPreflightCommandError = CommandError<
  WorkflowSkillReadinessView | { workflowId: string; ready: false }
>;
export type WorkflowRunState = "active" | "completed" | "cancelled";
export interface WorkflowNodeView {
  id: string;
  title: string;
  description: string;
  localSkillCount: number;
  pluginSkillCount: number;
  skillDeclarationCount: number;
  localSkillBindings: WorkflowSkillBindingView[];
  pluginSkillBindings: WorkflowSkillBindingView[];
}
export interface WorkflowDefinitionView {
  schemaVersion: number;
  definitionVersion: number;
  id: string;
  name: string;
  description: string;
  localSkillCount: number;
  pluginSkillCount: number;
  uniqueSkillCount: number;
  skillCatalog: WorkflowSkillBindingView[];
  nodes: WorkflowNodeView[];
}
export interface WorkflowNodeCompletion {
  nodeId: string;
  summary: string;
  evidence: string[];
  completedAtMs: number;
}
export interface WorkflowRunView {
  schemaVersion: number;
  definitionVersion: number;
  id: string;
  threadId: string;
  workflowId: string;
  objective: string;
  state: WorkflowRunState;
  currentNodeId: string | null;
  currentNodeIndex: number;
  nodeCount: number;
  completedNodes: WorkflowNodeCompletion[];
  createdAtMs: number;
  updatedAtMs: number;
  revision: number;
}
export interface CancelWorkflowRunRequest { threadId: string; runId: string; }

export interface SearchResult { path: string; line: number; column: number; preview: string; score: number; }
export interface MemorySettings { enabled: boolean; }
export interface MemoryView { schemaVersion: number; id: string; content: string; source: string; expiresAtMs: number; createdAtMs: number; updatedAtMs: number; deleted: boolean; revision: number; }
export interface MemoryUpsertRequest { id?: string; content: string; source: string; retentionDays: number; }
export interface BrowserSettings { enabled: boolean; allowLocalhost: boolean; }
export interface BrowserAuditEvent { timestampMs: number; action: string; target: string; success: boolean; detail: string; }
export interface BrowserArtifact { id: string; name: string; mediaType: string; sizeBytes: number; createdAtMs: number; }
export interface DocumentContent { path: string; name: string; mediaType: string; content: string; sourceBytes: number; extractedBytes: number; truncated: boolean; }
export interface MetricsSnapshot { providerCalls: number; providerFailures: number; averageProviderLatencyMs: number; inputTokens: number; outputTokens: number; compactionCount: number; compactedMessages: number; estimatedContextTokensSaved: number; toolCalls: number; toolSuccessRate: number; fallbackCount: number; retryCount: number; completedTasks: number; failedTasks: number; estimatedCostUsd: number | null; }
export interface EvaluationReport { total: number; passed: number; passRate: number; failures: string[]; }

export type ScheduledTaskKind = "once" | "daily" | "weekly";
export interface ScheduledTaskSchedule {
  kind: ScheduledTaskKind;
  atMs?: number | null;
  hour?: number | null;
  minute?: number | null;
  weekday?: number | null;
}
export type ScheduledTaskMode = "background" | "thread";
export interface ScheduledTaskView {
  schemaVersion: number;
  id: string;
  name: string;
  schedule: ScheduledTaskSchedule;
  prompt: string;
  mode: ScheduledTaskMode;
  threadId: string | null;
  workspacePath: string;
  enabled: boolean;
  nextRunAtMs: number | null;
  lastRunAtMs: number | null;
  lastRunState: "running" | "completed" | "failed" | null;
  lastError: string | null;
  runCount: number;
  createdAtMs: number;
  updatedAtMs: number;
  revision: number;
}
export interface UpsertScheduledTaskRequest {
  id?: string | null;
  name: string;
  schedule: ScheduledTaskSchedule;
  prompt: string;
  mode: ScheduledTaskMode;
  threadId?: string | null;
  workspacePath?: string | null;
  enabled?: boolean;
}

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

export interface TurnHandle {
  schemaVersion: number;
  threadId: string;
  turnId: string;
  state: TurnState;
}

export interface QueuedTurn {
  schemaVersion: number;
  turnId: string;
  threadId: string;
  kind: "message" | "retry";
  input: string;
  agentMode: string | null;
  workflowId?: string | null;
  attachments: ImageAttachment[];
}

export interface ThreadMailboxSnapshot {
  schemaVersion: number;
  threadId: string;
  revision: number;
  activeTurnId: string | null;
  pending: QueuedTurn[];
}

export interface ThreadMailboxChanged {
  schemaVersion: number;
  threadId: string;
  revision: number;
}

export interface TurnSteerResponse { schemaVersion: number; threadId: string; turnId: string; }

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

export type AgentItemType =
  | "agent_message"
  | "reasoning"
  | "tool"
  | "approval"
  | "change"
  | "context_compaction"
  | "user_input";
export type AgentItemStatus = "completed" | "failed" | "cancelled";

export type AgentEvent =
  | (EventBase & { type: "turn_started"; userMessage: ChatMessage | null })
  | (EventBase & { type: "turn_steered"; message: ChatMessage })
  | (EventBase & { type: "turn_rejected"; message: string })
  | (EventBase & { type: "item_started"; itemId: string; itemType: AgentItemType })
  | (EventBase & { type: "item_completed"; itemId: string; itemType: AgentItemType; status: AgentItemStatus })
  | (EventBase & { type: "activity_status_changed"; status: AgentActivityStatus })
  | (EventBase & { type: "text_delta"; itemId: string; delta: string })
  | (EventBase & { type: "reasoning_summary_delta"; itemId: string; delta: string })
  | (EventBase & { type: "reasoning_summary_completed"; itemId: string; summary: string })
  | (EventBase & {
      type: "usage_updated";
      usage: TokenUsage;
      contextUsage: TokenUsage;
    })
  | (EventBase & {
      type: "context_compacted";
      itemId: string;
      automatic: boolean;
      compactedMessageCount: number;
      userConstraintCount: number;
      recentToolResultCount: number;
      recentUserMessageCount?: number;
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

export type UserInputRequestKind = "model_question" | "turn_continuation";

export interface UserInputRequest {
  id: string;
  threadId: string;
  turnId: string;
  toolCallId: string;
  kind: UserInputRequestKind;
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
