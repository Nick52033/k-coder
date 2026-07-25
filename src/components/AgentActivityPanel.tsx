import { FormEvent, useEffect, useMemo, useState } from "react";
import {
  Bot,
  CheckCircle2,
  Clock,
  Loader2,
  Pause,
  Play,
  RotateCcw,
  Square,
  Timer,
  X,
  XCircle,
  ChevronDown,
  ChevronRight,
} from "lucide-react";
import {
  closeSubagent,
  createSubagent,
  errorMessage,
  listSubagents,
  resumeSubagent,
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
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);
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

  useEffect(() => {
    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    if (open) {
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }
  }, [open, onClose]);

  const runningCount = useMemo(
    () => agents.filter((agent) => activeStates.has(agent.state)).length,
    [agents],
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
      setTask("");
      setAllowEdits(false);
      setAllowCommands(false);
      setShowCreateForm(false);
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

  function toggleExpand(agentId: string) {
    setExpandedId((current) => current === agentId ? null : agentId);
  }

  if (!open) return null;

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="agent-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="agent-dialog-title"
      >
        <header className="agent-dialog-header">
          <div className="agent-dialog-title">
            <Bot size={18} />
            <h2 id="agent-dialog-title">子智能体</h2>
            {runningCount > 0 && <span className="agent-count-badge">{runningCount} 运行中</span>}
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            aria-label="关闭子智能体面板"
            title="关闭"
          >
            <X size={16} />
          </button>
        </header>

        <div className="agent-dialog-body">
          <div className="agent-quick-actions">
            {!showCreateForm ? (
              <button
                className="agent-create-button"
                type="button"
                onClick={() => setShowCreateForm(true)}
                disabled={!parentThreadId}
              >
                <Play size={14} fill="currentColor" />
                创建新任务
              </button>
            ) : (
              <form className="agent-create-form" onSubmit={handleCreate}>
                <textarea
                  aria-label="子任务描述"
                  rows={3}
                  value={task}
                  onChange={(event) => setTask(event.target.value)}
                  placeholder="描述需要并行执行的子任务..."
                  disabled={creating}
                  autoFocus
                />
                <div className="agent-create-options">
                  <div className="agent-capabilities">
                    <label>
                      <input
                        type="checkbox"
                        checked={allowEdits}
                        onChange={(event) => setAllowEdits(event.target.checked)}
                      />
                      <span>编辑权限</span>
                    </label>
                    <label>
                      <input
                        type="checkbox"
                        checked={allowCommands}
                        onChange={(event) => setAllowCommands(event.target.checked)}
                      />
                      <span>命令权限</span>
                    </label>
                  </div>
                  <div className="agent-create-actions">
                    <button
                      type="button"
                      className="agent-button agent-button--secondary"
                      onClick={() => { setShowCreateForm(false); setTask(""); setAllowEdits(false); setAllowCommands(false); }}
                      disabled={creating}
                    >
                      取消
                    </button>
                    <button
                      type="submit"
                      className="agent-button agent-button--primary"
                      disabled={!task.trim() || creating}
                    >
                      {creating ? <Loader2 className="spin" size={14} /> : <Play size={14} fill="currentColor" />}
                      启动
                    </button>
                  </div>
                </div>
              </form>
            )}
          </div>

          {error && <div className="agent-error" role="alert">{String(error)}</div>}

          <div className="agent-list">
            {agents.length === 0 && (
              <div className="agent-empty">
                <Bot size={32} className="agent-empty-icon" />
                <strong>暂无子任务</strong>
                <p>创建子任务来并行处理工作<br />每个任务独立运行，互不干扰</p>
                {!showCreateForm && (
                  <button
                    className="agent-button agent-button--primary"
                    type="button"
                    onClick={() => setShowCreateForm(true)}
                    disabled={!parentThreadId}
                  >
                    <Play size={14} fill="currentColor" />
                    创建第一个任务
                  </button>
                )}
              </div>
            )}

            {agents.map((agent) => (
              <AgentCard
                key={agent.id}
                agent={agent}
                expanded={expandedId === agent.id}
                onToggleExpand={() => toggleExpand(agent.id)}
                onStop={() => void stop(agent)}
                onResume={() => void resume(agent)}
              />
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}

interface AgentCardProps {
  agent: SubagentView;
  expanded: boolean;
  onToggleExpand: () => void;
  onStop: () => void;
  onResume: () => void;
}

function AgentCard({ agent, expanded, onToggleExpand, onStop, onResume }: AgentCardProps) {
  const statusInfo = getStatusInfo(agent.state);
  const progress = agent.tokenBudget > 0 ? (agent.tokensUsed / agent.tokenBudget) * 100 : 0;

  return (
    <div className={cn("agent-card", `agent-card--${agent.state}`)}>
      <button
        className="agent-card-header"
        type="button"
        onClick={onToggleExpand}
        aria-expanded={expanded}
      >
        <div className="agent-card-status">
          <statusInfo.Icon
            size={16}
            className={cn("agent-status-icon", statusInfo.spinning && "spin")}
            style={{ color: statusInfo.color }}
          />
          <div className="agent-card-info">
            <strong className="agent-card-label">{String(agent.label)}</strong>
            <span className="agent-card-meta">
              {statusInfo.label} · {agent.tokensUsed.toLocaleString()}/{agent.tokenBudget.toLocaleString()} tokens
            </span>
          </div>
        </div>
        <div className="agent-card-expand">
          {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        </div>
      </button>

      {agent.tokenBudget > 0 && (
        <div className="agent-progress-container">
          <div className="agent-progress-bar">
            <div
              className="agent-progress-fill"
              style={{
                width: `${Math.min(progress, 100)}%`,
                backgroundColor: statusInfo.color
              }}
            />
          </div>
          <span className="agent-progress-text">{Math.round(progress)}%</span>
        </div>
      )}

      {expanded && (
        <div className="agent-card-details">
          <div className="agent-detail-section">
            <h4>📝 任务描述</h4>
            <p className="agent-task-text">{agent.task}</p>
          </div>

          <div className="agent-detail-section">
            <h4>📊 执行信息</h4>
            <dl className="agent-info-list">
              <dt>权限</dt>
              <dd>{Array.isArray(agent.capabilities) ? agent.capabilities.join(" · ") : String(agent.capabilities)}</dd>
              <dt>已用 Token</dt>
              <dd>{agent.tokensUsed.toLocaleString()} / {agent.tokenBudget.toLocaleString()}</dd>
              <dt>创建时间</dt>
              <dd>{new Date(agent.createdAtMs).toLocaleTimeString()}</dd>
            </dl>
          </div>

          {agent.summary && (
            <div className="agent-detail-section">
              <h4>💬 执行摘要</h4>
              <p className="agent-summary-text">{agent.summary}</p>
            </div>
          )}

          {agent.error && (
            <div className="agent-detail-section agent-detail-section--error">
              <h4>⚠️ 错误信息</h4>
              <p className="agent-error-text">{agent.error}</p>
            </div>
          )}

          <div className="agent-card-actions">
            {activeStates.has(agent.state) ? (
              <button className="agent-button agent-button--danger" type="button" onClick={onStop}>
                <Square size={13} fill="currentColor" />
                停止
              </button>
            ) : ["failed", "cancelled", "timed_out"].includes(agent.state) ? (
              <button className="agent-button agent-button--secondary" type="button" onClick={onResume}>
                <RotateCcw size={14} />
                恢复
              </button>
            ) : null}
          </div>
        </div>
      )}
    </div>
  );
}

function upsert(agents: SubagentView[], incoming: SubagentView) {
  const existing = agents.some((agent) => agent.id === incoming.id);
  return existing ? agents.map((agent) => agent.id === incoming.id ? incoming : agent) : [incoming, ...agents];
}

function sortAgents(agents: SubagentView[]) {
  return [...agents].sort((a, b) => b.updatedAtMs - a.updatedAtMs);
}

function getStatusInfo(state: SubagentState) {
  const statusMap = {
    queued: {
      label: "排队中",
      Icon: Clock,
      color: "#3B82F6",
      spinning: false
    },
    running: {
      label: "运行中",
      Icon: Loader2,
      color: "#22C55E",
      spinning: true
    },
    blocked: {
      label: "等待审批",
      Icon: Pause,
      color: "#F59E0B",
      spinning: false
    },
    completed: {
      label: "已完成",
      Icon: CheckCircle2,
      color: "#10B981",
      spinning: false
    },
    failed: {
      label: "失败",
      Icon: XCircle,
      color: "#EF4444",
      spinning: false
    },
    cancelled: {
      label: "已取消",
      Icon: XCircle,
      color: "#6B7280",
      spinning: false
    },
    timed_out: {
      label: "已超时",
      Icon: Timer,
      color: "#F97316",
      spinning: false
    },
  };

  return statusMap[state] || statusMap.queued;
}
