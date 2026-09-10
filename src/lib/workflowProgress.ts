import type { PlanView, WorkflowDefinitionView, WorkflowRunView } from "../types/runtime";

/** Display projection only: workflow completion is owned by the backend. */
export function workflowProgressPlan(
  threadId: string | null,
  run: WorkflowRunView | null,
  definitions: WorkflowDefinitionView[],
): PlanView | null {
  if (!run || run.threadId !== threadId) return null;
  const definition = definitions.find((item) => item.id === run.workflowId
    && item.definitionVersion === run.definitionVersion);
  if (!definition || definition.nodes.length !== run.nodeCount) return null;
  if (run.state === "active" && !definition.nodes.some((node) => node.id === run.currentNodeId)) return null;
  const completed = new Map(run.completedNodes.map((node) => [node.nodeId, node]));
  return {
    schemaVersion: 1,
    threadId: run.threadId,
    revision: run.revision,
    updatedAtMs: run.updatedAtMs,
    steps: definition.nodes.map((node) => ({
      id: node.id,
      step: node.title,
      status: completed.has(node.id)
        ? "completed"
        : run.state === "active" && node.id === run.currentNodeId
          ? "in_progress"
          : "pending",
      detail: completed.get(node.id)?.summary ?? node.description,
    })),
  };
}
