import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentEvent,
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
  ProviderConfigView,
  ProviderConnectionTest,
  RuntimeStatus,
  SaveProviderConfigRequest,
  StartCommandRequest,
  StartPtyRequest,
  ThreadDetail,
  ThreadSummary,
  TurnOutcome,
  UsageSummary,
  WorkspaceState,
} from "../types/runtime";

export function getRuntimeStatus() {
  return invoke<RuntimeStatus>("runtime_status");
}

export function getProviderConfig() {
  return invoke<ProviderConfigView | null>("get_provider_config");
}

export function saveProviderConfig(request: SaveProviderConfigRequest) {
  return invoke<ProviderConfigView>("save_provider_config", { request });
}

export function deleteProviderApiKey() {
  return invoke<void>("delete_provider_api_key");
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

export function runTurn(threadId: string, input: string, attachments: ImageAttachment[] = []) {
  return invoke<TurnOutcome>("run_turn", { request: { threadId, input }, attachments });
}

export function retryTurn(threadId: string) {
  return invoke<TurnOutcome>("retry_turn", { threadId });
}

export function cancelTurn(threadId: string) {
  return invoke<boolean>("cancel_turn", { threadId });
}

export function previewPatch(patch: string) {
  return invoke<PatchPreview>("preview_patch", { patch });
}

export function resolveApproval(requestId: string, resolution: ApprovalResolution) {
  return invoke<void>("resolve_approval", { requestId, resolution });
}

export function undoChange(threadId: string, changeId: string) {
  return invoke<ChangeSet>("undo_change", { threadId, changeId });
}
export function testProviderConnection() { return invoke<ProviderConnectionTest>("test_provider_connection"); }

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
export function previewWorkspaceFile(path: string) { return invoke<FilePreview>("preview_workspace_file", { path }); }
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

export function subscribeToAgentEvents(
  handler: (event: AgentEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", ({ payload }) => handler(payload));
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message);
  }
  return String(error);
}
