import { CircleCheck, CircleX, Clock3, LoaderCircle } from "lucide-react";
import type { SubagentView } from "./types/runtime";
import { RetryWaitingLabel } from "./components/RetryWaitingLabel";

export function SubagentSummary({ agents, taskIndex, onFocus }: {
  agents: SubagentView[];
  taskIndex: Record<string, number>;
  onFocus: (id: string) => void;
}) {
  if (!agents.length) return null;
  return <section className="subagent-summary" aria-label="本会话子智能体状态">
    <strong>子智能体状态 <span>{agents.length}</span></strong>
    <ul>{[...agents].sort((a, b) => (taskIndex[a.id] ?? 0) - (taskIndex[b.id] ?? 0)).map((agent) => {
      const active = ["running", "queued", "blocked"].includes(agent.state);
      const waiting = active && Boolean(agent.retryAtMs);
      const failed = ["failed", "timed_out"].includes(agent.state);
      const Icon = waiting ? Clock3 : active ? LoaderCircle : failed ? CircleX : CircleCheck;
      const status = { queued: "排队中", running: "运行中", blocked: "等待确认", completed: "已完成", failed: "失败", timed_out: "已超时", cancelled: "已停止" }[agent.state];
      return <li key={agent.id}>
        <button type="button" onClick={() => onFocus(agent.id)} className={failed ? "subagent-summary-row subagent-summary-row--failed" : "subagent-summary-row"}>
          <Icon size={14} aria-hidden="true" />
          <code>task{taskIndex[agent.id]}</code>
          <span className="subagent-summary-label">{agent.label}</span>
          <span className="subagent-summary-status">{waiting ? <RetryWaitingLabel retryAtMs={agent.retryAtMs} /> : status}</span>
        </button>
      </li>;
    })}</ul>
  </section>;
}
