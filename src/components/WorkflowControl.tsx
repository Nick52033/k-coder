import { useState } from "react";
import { Bot, Loader2, X } from "lucide-react";
import type { WorkflowDefinitionView, WorkflowRunView } from "../types/runtime";

interface WorkflowControlProps {
  definition: WorkflowDefinitionView | null;
  run: WorkflowRunView | null;
  turnBusy: boolean;
  onCancel: () => Promise<boolean>;
}

export function WorkflowControl({ definition, run, turnBusy, onCancel }: WorkflowControlProps) {
  const [cancelling, setCancelling] = useState(false);
  if (!definition || !run || run.state !== "active") return null;

  const node = definition.nodes.find((item) => item.id === run.currentNodeId)
    ?? definition.nodes[run.currentNodeIndex];
  const currentStep = Math.min(run.currentNodeIndex + 1, run.nodeCount);
  const progress = run.nodeCount > 0 ? (run.currentNodeIndex / run.nodeCount) * 100 : 0;

  async function cancel() {
    setCancelling(true);
    try {
      await onCancel();
    } finally {
      setCancelling(false);
    }
  }

  return (
    <div className="workflow-control" aria-label={`机器人工作流 ${definition.name}`}>
      <Bot className="workflow-control-icon" size={17} />
      <div className="workflow-control-main">
        <div className="workflow-control-title">
          <strong>{definition.name}</strong>
          <span>{currentStep} / {run.nodeCount}</span>
        </div>
        <div className="workflow-control-node">{node?.title ?? "等待节点"}</div>
        <div
          className="workflow-control-progress"
          role="progressbar"
          aria-label="工作流进度"
          aria-valuemin={0}
          aria-valuemax={run.nodeCount}
          aria-valuenow={run.currentNodeIndex}
        >
          <span style={{ width: `${progress}%` }} />
        </div>
      </div>
      <button
        type="button"
        className="workflow-control-stop"
        aria-label="停止机器人工作流"
        title={turnBusy ? "请先停止当前 Turn" : "停止机器人工作流"}
        disabled={turnBusy || cancelling}
        onClick={() => void cancel()}
      >
        {cancelling ? <Loader2 className="spin" size={15} /> : <X size={15} />}
      </button>
    </div>
  );
}
