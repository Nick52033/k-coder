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
  SaveUserRuleRequest,
  UserRulesView,
  PluginOverview,
  McpConfigView,
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
  ThreadHistorySnapshot,
  ThreadItemsPage,
  ThreadSummary,
  ThreadTurnsPage,
  HistorySortDirection,
  TurnItemsView,
  TurnHandle,
  ThreadMailboxSnapshot,
  ThreadMailboxChanged,
  TurnSteerResponse,
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
  MemoryUpsertOutcome,
  MemoryRecord,
  MemoryPage,
  MemoryScope,
  MemoryStatus,
  MemoryCandidate,
  MemoryCandidateStatus,
  MemoryCandidateDecision,
  MemoryClearOutcome,
  SetMemorySettingsRequest,
  MaintenanceSettings,
  MaintenanceReport,
  SetMemoryMaintenanceSettingsRequest,
  MetricsSnapshot,
  PlanUpdateRequest,
  PlanView,
  SearchResult,
  CancelWorkflowRunRequest,
  WorkflowDefinitionView,
  WorkflowRunView,
  WorkflowSkillReadinessView,
  LogQuery,
  LogQueryResult,
  MobileCapability,
  MobileDeviceView,
  MobilePairingView,
  MobileStatus,
  KnowledgeSettings,
  EmbeddingSettings,
  KnowledgeCollection,
  UpsertKnowledgeCollectionRequest,
  AddKnowledgeSourceRequest,
  KnowledgeSource,
  KnowledgeIndexJob,
  KnowledgeIndexProgress,
  KnowledgeIndexMetrics,
  SetEmbeddingSettingsRequest,
  KnowledgeSearchResponse,
  KnowledgeCitation,
  KnowledgeFeedbackType,
  KnowledgeFeedbackRecord,
  KnowledgeRetrievalEventRecord,
  KnowledgeEntityType,
  KnowledgeEntityRecord,
  KnowledgeFactCandidateRecord,
  KnowledgeFactRecord,
  KnowledgeRelationQueryResult,
  ScheduledTaskView,
  UpsertScheduledTaskRequest,
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

export function createThread(inProject = true) {
  return invoke<ThreadSummary>("create_thread", { inProject });
}

export function listThreads() {
  return invoke<ThreadSummary[]>("list_threads");
}

export function readThread(threadId: string) {
  return invoke<ThreadDetail>("read_thread", { threadId });
}

export function readThreadHistory(threadId: string) {
  return invoke<ThreadHistorySnapshot>("read_thread_history", { threadId });
}

export function listThreadTurns(
  threadId: string,
  options: {
    cursor?: string | null;
    limit?: number;
    sortDirection?: HistorySortDirection;
    itemsView?: TurnItemsView;
  } = {},
) {
  return invoke<ThreadTurnsPage>("list_thread_turns", {
    threadId,
    cursor: options.cursor ?? null,
    limit: options.limit ?? null,
    sortDirection: options.sortDirection ?? null,
    itemsView: options.itemsView ?? null,
  });
}

export function listThreadItems(
  threadId: string,
  options: {
    turnId?: string | null;
    cursor?: string | null;
    limit?: number;
    sortDirection?: HistorySortDirection;
  } = {},
) {
  return invoke<ThreadItemsPage>("list_thread_items", {
    threadId,
    turnId: options.turnId ?? null,
    cursor: options.cursor ?? null,
    limit: options.limit ?? null,
    sortDirection: options.sortDirection ?? null,
  });
}

export function archiveThread(threadId: string) {
  return invoke<void>("archive_thread", { threadId });
}

export function runTurn(threadId: string, input: string, attachments: ImageAttachment[] = [], agentMode?: string, workflowId?: string) {
  return invoke<TurnOutcome>("run_turn", { request: { threadId, input, agentMode }, attachments, workflowId });
}

export function startTurn(
  threadId: string,
  input: string,
  attachments: ImageAttachment[] = [],
  agentMode?: string,
  workflowId?: string,
) {
  return invoke<TurnHandle>("turn_start", {
    request: { threadId, input, agentMode },
    attachments,
    workflowId,
  });
}

export function listBuiltinWorkflows() {
  return invoke<WorkflowDefinitionView[]>("list_builtin_workflows");
}

export function getWorkflowSkillReadiness(workflowId: string) {
  return invoke<WorkflowSkillReadinessView>("get_workflow_skill_readiness", { workflowId });
}

export function getWorkflowRun(threadId: string) {
  return invoke<WorkflowRunView | null>("get_workflow_run", { threadId });
}

export function cancelWorkflowRun(request: CancelWorkflowRunRequest) {
  return invoke<WorkflowRunView>("cancel_workflow_run", { request });
}

export function listScheduledTasks() {
  return invoke<ScheduledTaskView[]>("list_scheduled_tasks");
}

export function upsertScheduledTask(request: UpsertScheduledTaskRequest) {
  return invoke<ScheduledTaskView>("upsert_scheduled_task", { request });
}

export function deleteScheduledTask(taskId: string) {
  return invoke<void>("delete_scheduled_task", { taskId });
}

export function setScheduledTaskEnabled(taskId: string, enabled: boolean) {
  return invoke<ScheduledTaskView>("set_scheduled_task_enabled", { taskId, enabled });
}

export function triggerScheduledTask(taskId: string) {
  return invoke<ScheduledTaskView>("trigger_scheduled_task", { taskId });
}

export function readThreadMailbox(threadId: string) {
  return invoke<ThreadMailboxSnapshot>("read_thread_mailbox", { threadId });
}

export function removeQueuedTurn(threadId: string, turnId: string) {
  return invoke<boolean>("remove_queued_turn", { threadId, turnId });
}

export function clearThreadMailbox(threadId: string) {
  return invoke<number>("clear_thread_mailbox", { threadId });
}

export function steerTurn(
    threadId: string,
  expectedTurnId: string,
  input: string,
  attachments: ImageAttachment[] = [],
) {
  return invoke<TurnSteerResponse>("turn_steer", {
    request: { threadId, expectedTurnId, input, attachments },
  });
}

export function steerQueuedTurn(threadId: string, expectedTurnId: string, queuedTurnId: string) {
  return invoke<TurnSteerResponse>("turn_steer_queued", {
    request: { threadId, expectedTurnId, queuedTurnId },
  });
}

export function interruptTurn(threadId: string, turnId: string) {
  return invoke<void>("turn_interrupt", { threadId, turnId });
}

export function forkThread(threadId: string, lastTurnId?: string) {
  return invoke<ThreadSummary>("thread_fork", {
    request: { threadId, lastTurnId: lastTurnId ?? null },
  });
}

export function resumeThread(threadId: string) {
  return invoke<ThreadHistorySnapshot>("thread_resume", { threadId });
}

export function rollbackThread(threadId: string, numTurns: number) {
  return invoke<ThreadHistorySnapshot>("thread_rollback", {
    request: { threadId, numTurns },
  });
}

export function retryTurn(threadId: string) {
  return invoke<TurnHandle>("turn_retry", { threadId });
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
export function getKnowledgeSettings() { return invoke<KnowledgeSettings>("get_knowledge_settings"); }
export function setKnowledgeEnabled(enabled: boolean) { return invoke<KnowledgeSettings>("set_knowledge_enabled", { enabled }); }
export function listKnowledgeCollections() { return invoke<KnowledgeCollection[]>("list_knowledge_collections"); }
export function upsertKnowledgeCollection(request: UpsertKnowledgeCollectionRequest) { return invoke<KnowledgeCollection>("upsert_knowledge_collection", { request }); }
export function deleteKnowledgeCollection(collectionId: string, confirmationToken: string) { return invoke<Record<string, unknown>>("delete_knowledge_collection", { collectionId, confirmationToken }); }
export function addKnowledgeSource(request: AddKnowledgeSourceRequest) { return invoke<KnowledgeSource>("add_knowledge_source", { request }); }
export function listKnowledgeSources(collectionId: string) { return invoke<KnowledgeSource[]>("list_knowledge_sources", { collectionId }); }
export function deleteKnowledgeSource(sourceId: string, confirmationToken: string) { return invoke<Record<string, unknown>>("delete_knowledge_source", { sourceId, confirmationToken }); }
export function refreshKnowledgeSource(sourceId: string) { return invoke<KnowledgeIndexJob>("refresh_knowledge_source", { sourceId }); }
export function getKnowledgeIndexJob(jobId: string) { return invoke<KnowledgeIndexJob>("get_knowledge_index_job", { jobId }); }
export function cancelKnowledgeIndexJob(jobId: string) { return invoke<KnowledgeIndexJob>("cancel_knowledge_index_job", { jobId }); }
export function getKnowledgeMetrics() { return invoke<KnowledgeIndexMetrics>("get_knowledge_metrics"); }
export function getEmbeddingSettings() { return invoke<EmbeddingSettings>("get_embedding_settings"); }
export function setEmbeddingSettings(request: SetEmbeddingSettingsRequest) { return invoke<EmbeddingSettings>("set_embedding_settings", { request }); }
export function setEmbeddingApiKey(apiKey: string) { return invoke<Record<string, unknown>>("set_embedding_api_key", { apiKey }); }
export function deleteEmbeddingApiKey() { return invoke<Record<string, unknown>>("delete_embedding_api_key"); }
export function testEmbeddingConnection() { return invoke<{ connected: boolean; latencyMs: number; httpStatus: number | null; model: string; vectorDimension: number; usage: Record<string, unknown> | null; traceId: string | null; errorCode: string | null }>("test_embedding_connection"); }
export function searchKnowledge(query: string, limit = 6, threadId?: string, turnId?: string, modelRewrite = false) { return invoke<KnowledgeSearchResponse>("search_knowledge", { query, limit, threadId, turnId, modelRewrite }); }
export function readKnowledgeCitation(citationId: string, threadId: string, turnId: string, before = 0, after = 0) { return invoke<KnowledgeCitation>("read_knowledge_citation", { citationId, threadId, turnId, before, after }); }
export function recordKnowledgeFeedback(citationId: string, feedbackType: KnowledgeFeedbackType, threadId: string, turnId: string) { return invoke<KnowledgeFeedbackRecord>("record_knowledge_feedback", { citationId, feedbackType, threadId, turnId }); }
export function listKnowledgeRetrievalEvents(threadId: string, limit = 20) { return invoke<KnowledgeRetrievalEventRecord[]>("list_knowledge_retrieval_events", { threadId, limit }); }
export function listKnowledgeEntities(collectionId: string, status: string = "active", limit?: number) { return invoke<KnowledgeEntityRecord[]>("list_knowledge_entities", { collectionId, status, limit }); }
export function listKnowledgeFacts(collectionId: string, status: string = "candidate", limit?: number) { return invoke<KnowledgeFactCandidateRecord[]>("list_knowledge_facts", { collectionId, status, limit }); }
export function reviewKnowledgeFact(factId: string, decision: "accept" | "reject", entityType?: KnowledgeEntityType) { return invoke<KnowledgeFactRecord>("review_knowledge_fact", { factId, decision, entityType }); }
export function queryKnowledgeRelations(name: string, limit?: number) { return invoke<KnowledgeRelationQueryResult>("query_knowledge_relations", { name, limit }); }
export function getMemorySettings() { return invoke<MemorySettings>("get_memory_settings"); }
export function setMemorySettings(request: SetMemorySettingsRequest) { return invoke<MemorySettings>("set_memory_settings", { request }); }
export function setMemoryEnabled(enabled: boolean) { return invoke<MemorySettings>("set_memory_enabled", { enabled }); }
export function listMemories(scope: MemoryScope, status?: MemoryStatus, cursor?: string, limit?: number) { return invoke<MemoryPage>("list_memories", { scope, status, cursor, limit }); }
export function upsertMemory(request: MemoryUpsertRequest) { return invoke<MemoryUpsertOutcome>("upsert_memory", { request }); }
export function listMemoryCandidates(status?: MemoryCandidateStatus, limit?: number) { return invoke<MemoryCandidate[]>("list_memory_candidates", { status, limit }); }
export function reviewMemoryCandidate(candidateId: string, decision: MemoryCandidateDecision) { return invoke<MemoryCandidate>("review_memory_candidate", { candidateId, decision }); }
export function deleteMemory(memoryId: string, confirmationToken: string) { return invoke<MemoryRecord>("delete_memory", { memoryId, confirmationToken }); }
export function clearMemories(scope: MemoryScope, confirmationToken: string) { return invoke<MemoryClearOutcome>("clear_memories", { scope, confirmationToken }); }
export function getMemoryMaintenanceSettings() { return invoke<MaintenanceSettings>("get_memory_maintenance_settings"); }
export function setMemoryMaintenanceSettings(request: SetMemoryMaintenanceSettingsRequest) { return invoke<MaintenanceSettings>("set_memory_maintenance_settings", { request }); }
export function acceptMemoryMaintenanceDisclosure() { return invoke<MaintenanceSettings>("accept_memory_maintenance_disclosure"); }
export function runMemoryMaintenance() { return invoke<MaintenanceReport>("run_memory_maintenance"); }
export function cancelMemoryMaintenance() { return invoke<boolean>("cancel_memory_maintenance"); }
export function getBrowserSettings() { return invoke<BrowserSettings>("get_browser_settings"); }
export function saveBrowserSettings(settings: BrowserSettings) { return invoke<BrowserSettings>("save_browser_settings", { settings }); }
export function listBrowserAudit() { return invoke<BrowserAuditEvent[]>("list_browser_audit"); }
export function listBrowserArtifacts() { return invoke<BrowserArtifact[]>("list_browser_artifacts"); }
export function readBrowserArtifact(name: string) { return invoke<string>("read_browser_artifact", { name }); }
export function readMessageImage(threadId: string, path: string) { return invoke<string>("read_message_image", { threadId, path }); }
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

export function sendSubagentMessage(
  agentId: string,
  message: string,
  triggerTurn = true,
) {
  return invoke<SubagentView>("send_subagent_message", { agentId, message, triggerTurn });
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
export function getUserRules(refresh = false) { return invoke<UserRulesView>("user_rules", { refresh }); }
export function saveUserRule(request: SaveUserRuleRequest) { return invoke<UserRulesView>("save_user_rule", { request }); }
export function deleteUserRule(id: string) { return invoke<UserRulesView>("delete_user_rule", { id }); }
export function getPluginOverview(refresh = false) { return invoke<PluginOverview>("plugin_overview", { refresh }); }
export function setPluginEnabled(pluginId: string, enabled: boolean) { return invoke<PluginOverview>("set_plugin_enabled", { pluginId, enabled }); }
export function deletePlugin(pluginId: string) { return invoke<PluginOverview>("delete_plugin", { pluginId }); }
export function getMcpConfig(refresh = false) { return invoke<McpConfigView>("mcp_config", { refresh }); }
export function saveMcpConfig(scope: "global" | "project", content: string) { return invoke<McpConfigView>("save_mcp_config", { scope, content }); }
export function setExtensionEnabled(kind: "skill" | "mcp" | "hook", id: string, enabled: boolean) { return invoke<ExtensionOverview>("set_extension_enabled", { kind, id, enabled }); }
export function saveMcpSecret(server: string, name: string, value: string) { return invoke<ExtensionOverview>("save_mcp_secret", { server, name, value }); }
export function deleteMcpSecret(server: string, name: string) { return invoke<ExtensionOverview>("delete_mcp_secret", { server, name }); }
export function getWorkspaceState() { return invoke<WorkspaceState>("workspace_state"); }
export function switchWorkspace(path: string, trusted: boolean) { return invoke<ProjectRecord>("switch_workspace", { path, trusted }); }
/** 只登记项目（服务端事实），不切换活动工作区。 */
export function registerProjectPaths(paths: string[]) { return invoke<ProjectRecord[]>("register_project_paths", { paths }); }
/** 从项目清单移除；不删除会话与文件。 */
export function removeProjectPath(path: string) { return invoke<void>("remove_project_path", { path }); }
export function listWorkspaceDirectory(path = "") { return invoke<FileEntry[]>("list_workspace_directory", { path }); }
export function searchWorkspaceFiles(query: string, limit = 50) { return invoke<FileEntry[]>("search_workspace_files", { query, limit }); }
export function previewWorkspaceFile(path: string) { return invoke<FilePreview>("preview_workspace_file", { path }); }
export function saveWorkspaceFile(request: SaveWorkspaceFileRequest) { return invoke<FilePreview>("save_workspace_file", { request }); }
export function extractAttachment(path: string) { return invoke<AttachmentContent>("extract_attachment", { path }); }
export function extractLocalDocument(name: string, dataUrl: string) { return invoke<AttachmentContent>("extract_local_document", { name, dataUrl }); }
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

export function clearLogs(confirmed: boolean): Promise<void> {
  return invoke<void>("clear_logs", { confirmed });
}

export function subscribeToAgentEvents(
  handler: (event: AgentEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", ({ payload }) => handler(payload));
}

export function subscribeToMailboxEvents(
  handler: (event: ThreadMailboxChanged) => void,
): Promise<UnlistenFn> {
  return listen<ThreadMailboxChanged>("thread-mailbox-changed", ({ payload }) => handler(payload));
}

export function subscribeToKnowledgeProgress(
  handler: (event: KnowledgeIndexProgress) => void,
): Promise<UnlistenFn> {
  return listen<KnowledgeIndexProgress>("knowledge-index-progress", ({ payload }) => handler(payload));
}

export function subscribeToSubagentEvents(
  handler: (event: SubagentView) => void,
): Promise<UnlistenFn> {
  return listen<SubagentView>("subagent-event", ({ payload }) => handler(payload));
}

export function mobileStatus(): Promise<MobileStatus> {
  return invoke<MobileStatus>("mobile_status");
}

export function startMobileGateway(
  bindAddress: string | null,
  port: number | null = null,
): Promise<MobileStatus> {
  return invoke<MobileStatus>("mobile_start", { bindAddress, port });
}

export function stopMobileGateway(): Promise<MobileStatus> {
  return invoke<MobileStatus>("mobile_stop");
}

export function createMobilePairing(): Promise<MobilePairingView> {
  return invoke<MobilePairingView>("mobile_create_pairing");
}

export function approveMobilePairing(pendingId: string): Promise<MobileDeviceView> {
  return invoke<MobileDeviceView>("mobile_approve_pairing", { pendingId });
}

export function denyMobilePairing(pendingId: string): Promise<void> {
  return invoke<void>("mobile_deny_pairing", { pendingId });
}

export function revokeMobileDevice(deviceId: string): Promise<MobileDeviceView> {
  return invoke<MobileDeviceView>("mobile_revoke_device", { deviceId });
}

export function setMobileCapabilities(
  capabilities: MobileCapability[],
): Promise<MobileStatus> {
  return invoke<MobileStatus>("mobile_set_capabilities", { capabilities });
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message);
  }
  return String(error);
}
