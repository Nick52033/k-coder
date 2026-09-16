import type { SubagentState, ToolActivity } from "../types/runtime";

/**
 * 仍在推进中的子智能体状态。其余状态（已完成、失败、超时、已停止）都是终态。
 */
const LIVE_SUBAGENT_STATES: ReadonlySet<SubagentState> = new Set(["queued", "running", "blocked"]);

/** 子智能体仍在推进（排队、运行或等待确认）。 */
export function isLiveSubagentState(state: SubagentState): boolean {
  return LIVE_SUBAGENT_STATES.has(state);
}

/** Delegation tools from `multi_agent`; waits may target multiple subagents. */
export const DELEGATION_TOOL_NAMES = new Set([
  "create_agent",
  "wait_agent",
  "send_agent_message",
  "resume_agent",
  "close_agent",
]);

/**
 * Resolves the subagent a delegation tool acted on. Most tools carry `agentId` in
 * their arguments; `create_agent` only learns the id from its structured result.
 */
export function subagentIdsOf(activity: ToolActivity): string[] {
  if (!DELEGATION_TOOL_NAMES.has(activity.call.name)) return [];
  const args = activity.call.arguments ?? {};
  if (activity.call.name === "wait_agent" && Array.isArray(args.agentIds)) {
    return [...new Set(args.agentIds.filter((id): id is string => typeof id === "string" && !!id.trim()).map((id) => id.trim()))];
  }
  const argumentId = args.agentId;
  if (typeof argumentId === "string" && argumentId.trim()) return [argumentId.trim()];
  const output = activity.result?.output;
  if (typeof output !== "string" || !output.trim()) return [];
  try {
    const parsed = JSON.parse(output) as { id?: unknown };
    return typeof parsed.id === "string" && parsed.id.trim() ? [parsed.id.trim()] : [];
  } catch {
    return [];
  }
}

/**
 * 子智能体 id → 最早引用它的轮次。引用来自委派工具调用（`create_agent` 结果或
 * `agentId` 参数），因此"谁创建/等待了这个子任务"与"它属于哪一轮"是同一个事实，
 * 对话里的 `taskN` chip 也依赖同一份解析。
 */
export function buildSubagentTurnIndex(
  activitiesByTurn: Map<string, ToolActivity[]>,
): Record<string, string> {
  const turnByAgent: Record<string, string> = {};
  const referencedAtMs: Record<string, number> = {};
  for (const [turnId, activities] of activitiesByTurn) {
    for (const activity of activities) {
      const ids = subagentIdsOf(activity);
      if (!ids.length) continue;
      const at = activity.startedAtMs ?? activity.completedAtMs ?? Number.MAX_SAFE_INTEGER;
      for (const id of ids) {
        const current = referencedAtMs[id];
        if (current !== undefined && current <= at) continue;
        turnByAgent[id] = turnId;
        referencedAtMs[id] = at;
      }
    }
  }
  return turnByAgent;
}
