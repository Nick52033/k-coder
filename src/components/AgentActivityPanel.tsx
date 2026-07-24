import { FormEvent, useEffect, useMemo, useState } from "react";
import {
  Bot,
  CheckCircle2,
  Clock3,
  Loader2,
  MessageSquareMore,
  Play,
  RotateCcw,
  Send,
  Square,
  X,
  XCircle,
} from "lucide-react";
import {
  closeSubagent,
  createSubagent,
  errorMessage,
  listSubagents,
  resumeSubagent,
  sendSubagentMessage,
  subscribeToSubagentEvents,
} from "../api/runtime";
import { cn } from "../lib/cn";
import type { SubagentState, SubagentView } from "../types/runtime";

interface AgentActivityPanelProps {
  open: boolean;
  parentThreadId: string | null;
  onClose: () => void;
}

const activeStates = new Set<SubagentState>(["queued", "running", "blocked"]);

export function AgentActivityPanel({ open, parentThreadId, onClose }: AgentActivityPanelProps) {
  const [agents, setAgents] = useState<SubagentView[]>([]);
  const [task, setTask] = useState("");
  const [allowEdits, setAllowEdits] = useState(false);
  const [allowCommands, setAllowCommands] = useState(false);
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [followup, setFollowup] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void subscribeToSubagentEvents((agent) => {
      if (disposed || agent.parentThreadId !== parentThreadId) return;
      setAgents((current) => sortAgents(upsert(current, agent)));
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; });
    return () => { disposed = true; unlisten?.(); };
  }, [parentThreadId]);

  useEffect(() => {
    if (!open || !parentThreadId) return;
    void listSubagents(parentThreadId)
      .then((items) => { setAgents(sortAgents(items)); setError(""); })
      .catch((reason) => setError(errorMessage(reason)));
  }, [open, parentThreadId]);

  const selected = useMemo(
    () => agents.find((agent) => agent.id === selectedId) ?? agents[0] ?? null,
    [agents, selectedId],
  );

  async function handleCreate(event: FormEvent) {
    event.preventDefault();
    if (!parentThreadId || !task.trim() || creating) return;
    const capabilities = ["list_directory", "read_file"];
    if (allowEdits) capabilities.push("apply_patch", "write_file");
    if (allowCommands) capabilities.push("run_command");
    setCreating(true);
    try {
      const created = await createSubagent({
        parentThreadId,
        task: task.trim(),
        capabilities,
        tokenBudget: 64_000,
        timeoutMs: 600_000,
      });
      setAgents((current) => sortAgents(upsert(current, created)));
      setSelectedId(created.id);
      setTask("");
      setError("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setCreating(false);
    }
  }

  async function stop(agent: SubagentView) {
    try {
      const updated = await closeSubagent(agent.id);
      setAgents((current) => sortAgents(upsert(current, updated)));
    } catch (reason) { setError(errorMessage(reason)); }
  }

  async function resume(agent: SubagentView) {
    try {
      const updated = await resumeSubagent(agent.id);
      setAgents((current) => sortAgents(upsert(current, updated)));
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  async function sendFollowup(event: FormEvent) {
    event.preventDefault();
    if (!selected || !followup.trim()) return;
    try {
      const updated = await sendSubagentMessage(selected.id, followup.trim());
      setAgents((current) => sortAgents(upsert(current, updated)));
      setFollowup("");
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  if (!open) return null;
  return (
    <aside className="agent-panel" aria-label="多智能体活动">
      <header className="agent-panel-header">
        <div><Bot size={17} /><strong>子智能体</strong><span>{agents.filter((agent) => activeStates.has(agent.state)).length} 运行中</span></div>
        <button className="icon-button" type="button" onClick={onClose} aria-label="关闭子智能体面板" title="关闭"><X size={16} /></button>
      </header>

      <form className="agent-create" onSubmit={handleCreate}>
        <textarea aria-label="子任务" rows={3} value={task} onChange={(event) => setTask(event.target.value)} placeholder="输入独立子任务" disabled={!parentThreadId || creating} />
        <div className="agent-capabilities">
          <label><input type="checkbox" checked={allowEdits} onChange={(event) => setAllowEdits(event.target.checked)} />编辑</label>
          <label><input type="checkbox" checked={allowCommands} onChange={(event) => setAllowCommands(event.target.checked)} />命令</label>
          <button type="submit" disabled={!parentThreadId || !task.trim() || creating}>
            {creating ? <Loader2 className="spin" size={14} /> : <Play size={14} fill="currentColor" />}启动
          </button>
        </div>
      </form>

      {error && <div className="agent-error" role="alert">{error}</div>}
      <div className="agent-list">
        {agents.length === 0 && <div className="agent-empty"><Bot size={20} /><span>暂无子任务</span></div>}
        {agents.map((agent) => (
          <button key={agent.id} className={cn("agent-row", selected?.id === agent.id && "agent-row--selected")} type="button" onClick={() => setSelectedId(agent.id)}>
            <StateIcon state={agent.state} />
            <span><strong>{agent.label}</strong><small>{stateLabel(agent.state)} · {agent.tokensUsed}/{agent.tokenBudget} tokens</small></span>
          </button>
        ))}
      </div>

      {selected && (
        <section className="agent-detail">
          <div className="agent-detail-heading"><strong>{selected.label}</strong><span>{selected.capabilities.join(" · ")}</span></div>
          <p className="agent-task">{selected.task}</p>
          {selected.summary && <div className="agent-summary">{selected.summary}</div>}
          {selected.error && <div className="agent-error">{selected.error}</div>}
          <div className="agent-detail-actions">
            {activeStates.has(selected.state) ? (
              <button type="button" onClick={() => void stop(selected)}><Square size={13} fill="currentColor" />停止</button>
            ) : ["failed", "cancelled", "timed_out"].includes(selected.state) ? (
              <button type="button" onClick={() => void resume(selected)}><RotateCcw size={14} />恢复</button>
            ) : null}
          </div>
          {!activeStates.has(selected.state) && (
            <form className="agent-followup" onSubmit={sendFollowup}>
              <input aria-label="发送子智能体消息" value={followup} onChange={(event) => setFollowup(event.target.value)} placeholder="继续这个子任务" />
              <button type="submit" disabled={!followup.trim()} aria-label="发送" title="发送"><Send size={14} /></button>
            </form>
          )}
        </section>
      )}
    </aside>
  );
}

function upsert(agents: SubagentView[], incoming: SubagentView) {
  const existing = agents.some((agent) => agent.id === incoming.id);
  return existing ? agents.map((agent) => agent.id === incoming.id ? incoming : agent) : [incoming, ...agents];
}
function sortAgents(agents: SubagentView[]) { return [...agents].sort((a, b) => b.updatedAtMs - a.updatedAtMs); }
function stateLabel(state: SubagentState) {
  return { queued: "排队", running: "运行中", blocked: "等待审批", completed: "已完成", failed: "失败", cancelled: "已取消", timed_out: "已超时" }[state];
}
function StateIcon({ state }: { state: SubagentState }) {
  if (state === "completed") return <CheckCircle2 className="agent-state agent-state--success" size={16} />;
  if (["failed", "cancelled", "timed_out"].includes(state)) return <XCircle className="agent-state agent-state--error" size={16} />;
  if (state === "blocked") return <Clock3 className="agent-state agent-state--blocked" size={16} />;
  return state === "running" ? <Loader2 className="agent-state spin" size={16} /> : <MessageSquareMore className="agent-state" size={16} />;
}
