import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentEvent,
  ApprovalMode,
  ApprovalResolution,
  ChangeSet,
  FileEntry,
  FilePreview,
  AttachmentContent,
  GitStatusView,
  GitBranchView,
  ExtensionOverview,
  ImageAttachment,
  ProjectRecord,
  CommandOutputPage,
  CommandSessionView,
  ContextCompactionSummary,
  PtyOutputPage,
  PtySessionView,
  PatchPreview,
  ProviderCatalogView,
  ProviderConfigView,
  ProviderConnectionTest,
  ReasoningEffort,
  RuntimeStatus,
  SaveWorkspaceFileRequest,
  SaveProviderConfigRequest,
  StartCommandRequest,
  StartPtyRequest,
  ThreadDetail,
  ThreadSummary,
  TurnOutcome,
  UserInputResolution,
  UsageSummary,
  WorkspaceState,
  CreateSubagentRequest,
  SubagentView,
  BrowserArtifact,
  BrowserAuditEvent,
  BrowserSettings,
  CreateGoalRequest,
  DocumentContent,
  EvaluationReport,
  GoalState,
  GoalView,
  MemorySettings,
  MemoryUpsertRequest,
  MemoryView,
  MetricsSnapshot,
  PlanUpdateRequest,
  PlanView,
  SearchResult,
  LogQuery,
  LogQueryResult,
} from "../types/runtime";

export function getRuntimeStatus() {
  return invoke<RuntimeStatus>("runtime_status");
}

export function getApprovalMode() {
  return invoke<ApprovalMode>("get_approval_mode");
}

export function setApprovalMode(mode: ApprovalMode) {
  return invoke<ApprovalMode>("set_approval_mode", { mode });
}

export function getReasoningEffort() {
  return invoke<ReasoningEffort>("get_reasoning_effort");
}

export function setReasoningEffort(effort: ReasoningEffort) {
  return invoke<ReasoningEffort>("set_reasoning_effort", { effort });
}

export function getProviderConfig() {
  return invoke<ProviderConfigView | null>("get_provider_config");
}

export function getProviderCatalog() {
  return invoke<ProviderCatalogView>("get_provider_catalog");
}

export function saveProviderConfig(request: SaveProviderConfigRequest) {
  return invoke<ProviderConfigView>("save_provider_config", { request });
}

export function activateProvider(providerId: string) {
  return invoke<ProviderCatalogView>("activate_provider", { providerId });
}

export function deleteProvider(providerId: string) {
  return invoke<ProviderCatalogView>("delete_provider", { providerId });
}

export function deleteProviderApiKey(providerId: string) {
  return invoke<void>("delete_provider_api_key", { providerId });
}

export function createThread() {
  return invoke<ThreadSummary>("create_thread");
}

export function listThreads() {
  return invoke<ThreadSummary[]>("list_threads");
}

export function readThread(threadId: string) {
  return invoke<ThreadDetail>("read_thread", { threadId });
}

export function archiveThread(threadId: string) {
  return invoke<void>("archive_thread", { threadId });
}

export function runTurn(threadId: string, input: string, attachments: ImageAttachment[] = [], agentMode?: string) {
  return invoke<TurnOutcome>("run_turn", { request: { threadId, input, agentMode }, attachments });
}

export function retryTurn(threadId: string) {
  return invoke<TurnOutcome>("retry_turn", { threadId });
}

export function cancelTurn(threadId: string) {
  return invoke<boolean>("cancel_turn", { threadId });
}

export function getPlan(threadId: string) { return invoke<PlanView | null>("get_plan", { threadId }); }
export function updatePlan(request: PlanUpdateRequest) { return invoke<PlanView>("update_plan", { request }); }
export function getGoal(threadId: string) { return invoke<GoalView | null>("get_goal", { threadId }); }
export function createGoal(request: CreateGoalRequest) { return invoke<GoalView>("create_goal", { request }); }
export function transitionGoal(goalId: string, state: GoalState, reason?: string) { return invoke<GoalView>("transition_goal", { request: { goalId, state, reason } }); }
export function searchRepository(query: string, limit = 50) { return invoke<SearchResult[]>("search_repository", { query, limit }); }
export function getMemorySettings() { return invoke<MemorySettings>("get_memory_settings"); }
export function setMemoryEnabled(enabled: boolean) { return invoke<MemorySettings>("set_memory_enabled", { enabled }); }
export function listMemories() { return invoke<MemoryView[]>("list_memories"); }
export function upsertMemory(request: MemoryUpsertRequest) { return invoke<MemoryView>("upsert_memory", { request }); }
export function deleteMemory(memoryId: string) { return invoke<MemoryView>("delete_memory", { memoryId }); }
export function getBrowserSettings() { return invoke<BrowserSettings>("get_browser_settings"); }
export function saveBrowserSettings(settings: BrowserSettings) { return invoke<BrowserSettings>("save_browser_settings", { settings }); }
export function listBrowserAudit() { return invoke<BrowserAuditEvent[]>("list_browser_audit"); }
export function listBrowserArtifacts() { return invoke<BrowserArtifact[]>("list_browser_artifacts"); }
export function closeBrowserSession() { return invoke<void>("close_browser_session"); }
export function extractDocumentContent(relativePath: string) { return invoke<DocumentContent>("extract_document_content", { relativePath }); }
export function getAdvancedMetrics() { return invoke<MetricsSnapshot>("advanced_metrics"); }
export function runRegressionEvaluation() { return invoke<EvaluationReport>("run_regression_evaluation"); }

export function createSubagent(request: CreateSubagentRequest) {
  return invoke<SubagentView>("create_subagent", { request });
}

export function listSubagents(parentThreadId?: string) {
  return invoke<SubagentView[]>("list_subagents", { parentThreadId });
}

export function waitSubagent(agentId: string, timeoutMs = 30_000) {
  return invoke<SubagentView>("wait_subagent", { agentId, timeoutMs });
}

export function sendSubagentMessage(agentId: string, message: string) {
  return invoke<SubagentView>("send_subagent_message", { agentId, message });
}

export function resumeSubagent(agentId: string, message?: string) {
  return invoke<SubagentView>("resume_subagent", { agentId, message });
}

export function closeSubagent(agentId: string) {
  return invoke<SubagentView>("close_subagent", { agentId });
}

export function previewPatch(patch: string) {
  return invoke<PatchPreview>("preview_patch", { patch });
}

export function resolveApproval(requestId: string, resolution: ApprovalResolution) {
  return invoke<void>("resolve_approval", { requestId, resolution });
}

export function resolveUserInput(requestId: string, resolution: UserInputResolution) {
  return invoke<void>("resolve_user_input", { requestId, resolution });
}

export function undoChange(threadId: string, changeId: string) {
  return invoke<ChangeSet>("undo_change", { threadId, changeId });
}
export function testProviderConnection(providerId?: string) { return invoke<ProviderConnectionTest>("test_provider_connection", { providerId }); }

export function searchThreads(query: string) { return invoke<ThreadSummary[]>("search_threads", { query }); }
export function renameThread(threadId: string, title: string) { return invoke<ThreadSummary>("rename_thread", { threadId, title }); }
export function deleteThread(threadId: string) { return invoke<void>("delete_thread", { threadId }); }
export function getUsageSummary() { return invoke<UsageSummary>("usage_summary"); }
export function getExtensionOverview(refresh = false) { return invoke<ExtensionOverview>("extension_overview", { refresh }); }
export function setExtensionEnabled(kind: "skill" | "mcp" | "hook", id: string, enabled: boolean) { return invoke<ExtensionOverview>("set_extension_enabled", { kind, id, enabled }); }
export function saveMcpSecret(server: string, name: string, value: string) { return invoke<ExtensionOverview>("save_mcp_secret", { server, name, value }); }
export function deleteMcpSecret(server: string, name: string) { return invoke<ExtensionOverview>("delete_mcp_secret", { server, name }); }
export function getWorkspaceState() { return invoke<WorkspaceState>("workspace_state"); }
export function switchWorkspace(path: string, trusted: boolean) { return invoke<ProjectRecord>("switch_workspace", { path, trusted }); }
export function listWorkspaceDirectory(path = "") { return invoke<FileEntry[]>("list_workspace_directory", { path }); }
export function searchWorkspaceFiles(query: string, limit = 50) { return invoke<FileEntry[]>("search_workspace_files", { query, limit }); }
export function previewWorkspaceFile(path: string) { return invoke<FilePreview>("preview_workspace_file", { path }); }
export function saveWorkspaceFile(request: SaveWorkspaceFileRequest) { return invoke<FilePreview>("save_workspace_file", { request }); }
export function extractAttachment(path: string) { return invoke<AttachmentContent>("extract_attachment", { path }); }
export function openWorkspaceFile(path: string) { return invoke<void>("open_workspace_file", { path }); }
export function revealWorkspaceFile(path: string) { return invoke<void>("reveal_workspace_file", { path }); }
export function getGitStatus() { return invoke<GitStatusView>("git_status"); }
export function getGitDiff(path?: string, staged = false) { return invoke<string>("git_diff", { path, staged }); }
export function getGitBranches() { return invoke<GitBranchView>("git_branches"); }
export function switchGitBranch(branch: string, create: boolean, confirmed: boolean) {
  return invoke<string>("git_switch_branch", { branch, create, confirmed });
}
export function runGitAction(action: "stage" | "unstage" | "commit" | "pull" | "push", paths: string[] = [], message?: string, confirmed = false) {
  return invoke<string>("git_action", { action, paths, message, confirmed });
}

export function compactThread(threadId: string) {
  return invoke<ContextCompactionSummary>("compact_thread", { threadId });
}

export function rebuildSessionProjection() {
  return invoke<void>("rebuild_session_projection");
}

export function startCommand(request: StartCommandRequest) {
  return invoke<CommandSessionView>("start_command", { request });
}

export function commandStatus(sessionId: string) {
  return invoke<CommandSessionView>("command_status", { sessionId });
}

export function readCommandOutput(sessionId: string, cursor = 0, limit = 200) {
  return invoke<CommandOutputPage>("read_command_output", { sessionId, cursor, limit });
}

export function waitCommand(sessionId: string) {
  return invoke<CommandSessionView>("wait_command", { sessionId });
}

export function writeCommandStdin(sessionId: string, input: string) {
  return invoke<void>("write_command_stdin", { sessionId, input });
}

export function cancelCommand(sessionId: string) {
  return invoke<boolean>("cancel_command", { sessionId });
}

export function closeCommand(sessionId: string) {
  return invoke<void>("close_command", { sessionId });
}

export function startPty(request: StartPtyRequest) {
  return invoke<PtySessionView>("start_pty", { request });
}

export function ptyStatus(sessionId: string) {
  return invoke<PtySessionView>("pty_status", { sessionId });
}

export function readPtyOutput(sessionId: string, cursor = 0, limit = 200) {
  return invoke<PtyOutputPage>("read_pty_output", { sessionId, cursor, limit });
}

export function writePty(sessionId: string, input: string) {
  return invoke<void>("write_pty", { sessionId, input });
}

export function resizePty(sessionId: string, rows: number, cols: number) {
  return invoke<void>("resize_pty", { sessionId, rows, cols });
}

export function waitPty(sessionId: string) {
  return invoke<PtySessionView>("wait_pty", { sessionId });
}

export function closePty(sessionId: string) {
  return invoke<void>("close_pty", { sessionId });
}

export interface OcrResult {
  text: string;
  lineCount: number;
  durationMs: number;
}

export function recognizeImage(dataUrl: string): Promise<OcrResult> {
  return invoke<OcrResult>("recognize_image", { dataUrl });
}

export function readLogs(query: LogQuery = {}): Promise<LogQueryResult> {
  return invoke<LogQueryResult>("read_logs", {
    limit: query.limit,
    level: query.level,
    event: query.event,
    afterTimestampMs: query.afterTimestampMs,
  });
}

export function subscribeToAgentEvents(
  handler: (event: AgentEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", ({ payload }) => handler(payload));
}

export function subscribeToSubagentEvents(
  handler: (event: SubagentView) => void,
): Promise<UnlistenFn> {
  return listen<SubagentView>("subagent-event", ({ payload }) => handler(payload));
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message);
  }
  return String(error);
}
