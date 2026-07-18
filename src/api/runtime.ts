import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentEvent,
  ProviderConfigView,
  RuntimeStatus,
  SaveProviderConfigRequest,
  ThreadDetail,
  ThreadSummary,
  TurnOutcome,
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

export function runTurn(threadId: string, input: string) {
  return invoke<TurnOutcome>("run_turn", { request: { threadId, input } });
}

export function retryTurn(threadId: string) {
  return invoke<TurnOutcome>("retry_turn", { threadId });
}

export function cancelTurn(threadId: string) {
  return invoke<boolean>("cancel_turn", { threadId });
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
