import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  Bot,
  CheckCircle2,
  Clock,
  Copy,
  Loader2,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Send,
  Square,
  Timer,
  X,
  XCircle,
  ChevronRight,
} from "lucide-react";
import {
  closeSubagent,
  createSubagent,
  errorMessage,
  listSubagents,
  readThread,
  resumeSubagent,
  sendSubagentMessage,
  subscribeToAgentEvents,
  subscribeToSubagentEvents,
} from "../api/runtime";
import { cn } from "../lib/cn";
import type { AgentEvent, ChatMessage, SubagentState, SubagentView } from "../types/runtime";
import { MarkdownContent } from "./MarkdownContent";
import "./AgentActivityPanel.css";

interface AgentActivityPanelProps {
  open: boolean;
  parentThreadId: string | null;
  /** Controlled selection so the conversation view can focus a subagent from anywhere. */
  selectedId: string | null;
  onSelectId: (id: string | null) => void;
  onClose: () => void;
}

const activeStates = new Set<SubagentState>(["queued", "running", "blocked"]);

interface DetailLine {
  id: string;
  kind: "text" | "tool" | "note";
  text: string;
  pending?: boolean;
}

export function AgentActivityPanel({
  open,
  parentThreadId,
  selectedId,
  onSelectId,
  onClose,
}: AgentActivityPanelProps) {
  const [agents, setAgents] = useState<SubagentView[]>([]);
  const [task, setTask] = useState("");
  const [tokenBudget, setTokenBudget] = useState("");
  const [forkTurns, setForkTurns] = useState("none");
  const [allowEdits, setAllowEdits] = useState(false);
  const [allowCommands, setAllowCommands] = useState(false);
  const [creating, setCreating] = useState(false);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void subscribeToSubagentEvents((agent) => {
      if (disposed) return;
      setAgents((current) => sortAgents(upsert(current, agent)));
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    // Load every subagent, not just the ones owned by the active thread, so the
    // conversation can focus a subagent created from another session.
    void listSubagents()
      .then((items) => {
        setAgents(sortAgents(items));
        setError("");
      })
      .catch((reason) => setError(errorMessage(reason)));
  }, [open]);

  useEffect(() => {
    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key !== "Escape") return;
      if (selectedId) onSelectId(null);
      else onClose();
    }
    if (open) {
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }
  }, [open, onClose, selectedId]);

  const runningCount = useMemo(
    () => agents.filter((agent) => activeStates.has(agent.state)).length,
    [agents],
  );
  const selected = useMemo(
    () => agents.find((agent) => agent.id === selectedId) ?? null,
    [agents, selectedId],
  );

  async function handleCreate(event: FormEvent) {
    event.preventDefault();
    if (!parentThreadId || !task.trim() || creating) return;
    const capabilities = ["list_directory", "read_file"];
    if (allowEdits) capabilities.push("apply_patch", "write_file");
    if (allowCommands) capabilities.push("run_command");
    const parsedTokenBudget = tokenBudget.trim() ? Number(tokenBudget) : undefined;
    if (
      parsedTokenBudget !== undefined &&
      (!Number.isSafeInteger(parsedTokenBudget) || parsedTokenBudget <= 0)
    ) {
      setError("Token 预算必须是正整数");
      return;
    }
    setCreating(true);
    try {
      const created = await createSubagent({
        parentThreadId,
        task: task.trim(),
        capabilities,
        ...(parsedTokenBudget === undefined ? {} : { tokenBudget: parsedTokenBudget }),
        timeoutMs: 600_000,
        forkTurns: forkTurns.trim() || "none",
      });
      setAgents((current) => sortAgents(upsert(current, created)));
      onSelectId(created.id);
      setTask("");
      setTokenBudget("");
      setForkTurns("none");
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
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  async function resume(agent: SubagentView) {
    try {
      const updated = await resumeSubagent(agent.id);
      setAgents((current) => sortAgents(upsert(current, updated)));
      setError("");
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }

  return (
    <aside
      className={cn("subagent-drawer", open && "subagent-drawer--open")}
      aria-hidden={!open}
      aria-label="子智能体"
    >
      {!open ? null : selected ? (
        <SubagentDetail
          agent={selected}
          onBack={() => onSelectId(null)}
          onStop={() => void stop(selected)}
          onResume={() => void resume(selected)}
          onError={setError}
        />
      ) : (
        <>
          <header className="subagent-drawer-header">
            <div className="subagent-drawer-title">
              <Bot size={16} />
              <span>子智能体</span>
              {runningCount > 0 && <span className="agent-count-badge">{runningCount} 运行中</span>}
            </div>
            <div className="subagent-drawer-actions">
              <button
                className="icon-button"
                type="button"
                onClick={() => setShowCreateForm((value) => !value)}
                disabled={!parentThreadId}
                aria-label="新建子任务"
                title="新建子任务"
              >
                <Plus size={14} />
              </button>
              <button
                className="icon-button"
                type="button"
                onClick={onClose}
                aria-label="关闭子智能体面板"
                title="关闭"
              >
                <X size={15} />
              </button>
            </div>
          </header>

          {showCreateForm && (
            <form className="subagent-create-form" onSubmit={handleCreate}>
              <textarea
                aria-label="子任务描述"
                rows={3}
                value={task}
                onChange={(event) => setTask(event.target.value)}
                placeholder="描述需要并行执行的子任务..."
                disabled={creating}
                autoFocus
              />
              <div className="subagent-create-options">
                <label className="subagent-field">
                  <span>Token 预算（可选）</span>
                  <input
                    type="number"
                    min={1}
                    step={10_000}
                    value={tokenBudget}
                    placeholder="默认不限制"
                    disabled={creating}
                    onChange={(event) => setTokenBudget(event.target.value)}
                  />
                </label>
                <label className="subagent-field">
                  <span>继承上下文（fork_turns）</span>
                  <input
                    type="text"
                    value={forkTurns}
                    placeholder="none / all / 3"
                    disabled={creating}
                    onChange={(event) => setForkTurns(event.target.value)}
                  />
                </label>
                <div className="subagent-capabilities">
                  <label>
                    <input
                      type="checkbox"
                      checked={allowEdits}
                      disabled={creating}
                      onChange={(event) => setAllowEdits(event.target.checked)}
                    />
                    <span>编辑权限</span>
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      checked={allowCommands}
                      disabled={creating}
                      onChange={(event) => setAllowCommands(event.target.checked)}
                    />
                    <span>命令权限</span>
                  </label>
                </div>
                <div className="subagent-create-actions">
                  <button
                    type="button"
                    className="agent-button agent-button--secondary"
                    onClick={() => {
                      setShowCreateForm(false);
                      setTask("");
                      setTokenBudget("");
                      setForkTurns("none");
                      setAllowEdits(false);
                      setAllowCommands(false);
                    }}
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

          {error && <div className="agent-error" role="alert">{String(error)}</div>}

          <div className="subagent-list">
            {agents.length === 0 && (
              <div className="agent-empty">
                <Bot size={28} />
                <strong>暂无子任务</strong>
                <p>创建子任务来并行处理工作</p>
              </div>
            )}
            {agents.map((agent) => (
              <SubagentRow
                key={agent.id}
                agent={agent}
                selected={agent.id === selectedId}
                onSelect={() => onSelectId(agent.id)}
                onStop={() => void stop(agent)}
                onResume={() => void resume(agent)}
              />
            ))}
          </div>
        </>
      )}
    </aside>
  );
}

interface SubagentRowProps {
  agent: SubagentView;
  selected: boolean;
  onSelect: () => void;
  onStop: () => void;
  onResume: () => void;
}

function SubagentRow({ agent, selected, onSelect, onStop, onResume }: SubagentRowProps) {
  const statusInfo = getStatusInfo(agent.state);
  const elapsed = formatElapsed(agent.createdAtMs, agent.updatedAtMs);

  return (
    <div className={cn("subagent-row", `subagent-row--${agent.state}`, selected && "subagent-row--selected")}>
      <button className="subagent-row-main" type="button" onClick={onSelect}>
        <span className="subagent-row-icon" style={{ color: statusInfo.color }}>
          <statusInfo.Icon size={14} className={cn(statusInfo.spinning && "spin")} />
        </span>
        <span className="subagent-row-body">
          <span className="subagent-row-label">{String(agent.label)}</span>
          <span className="subagent-row-meta">
            {statusInfo.label} · 已处理 {elapsed} · {formatTokenUsage(agent.tokensUsed, agent.tokenBudget)}
          </span>
          <span className="subagent-row-summary">{summarize(agent)}</span>
        </span>
        <ChevronRight size={14} className="subagent-row-chevron" />
      </button>
      <div className="subagent-row-actions">
        {activeStates.has(agent.state) ? (
          <button
            className="agent-button agent-button--danger"
            type="button"
            onClick={onStop}
            title="停止"
          >
            <Square size={12} fill="currentColor" />
          </button>
        ) : ["failed", "cancelled", "timed_out"].includes(agent.state) ? (
          <button
            className="agent-button agent-button--secondary"
            type="button"
            onClick={onResume}
            title="恢复"
          >
            <RotateCcw size={13} />
          </button>
        ) : null}
      </div>
    </div>
  );
}

interface SubagentDetailProps {
  agent: SubagentView;
  onBack: () => void;
  onStop: () => void;
  onResume: () => void;
  onError: (message: string) => void;
}

function SubagentDetail({ agent, onBack, onStop, onResume, onError }: SubagentDetailProps) {
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [lines, setLines] = useState<DetailLine[]>([]);
  const [draft, setDraft] = useState("");
  const [triggerTurn, setTriggerTurn] = useState(true);
  const [sending, setSending] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const statusInfo = getStatusInfo(agent.state);

  useEffect(() => {
    let disposed = false;
    void readThread(agent.threadId)
      .then((detail) => {
        if (!disposed) setHistory(Array.isArray(detail.messages) ? detail.messages : []);
      })
      .catch(() => {
        /* history is best effort; the live stream still renders. */
      });
    return () => {
      disposed = true;
    };
  }, [agent.threadId, agent.turnCount]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void subscribeToAgentEvents((event: AgentEvent) => {
      if (disposed || event.threadId !== agent.threadId) return;
      setLines((current) => applyEvent(current, event));
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [agent.threadId]);

  useEffect(() => {
    const node = scrollRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [lines.length, history.length]);

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    setSending(true);
    try {
      await sendSubagentMessage(agent.id, text, triggerTurn);
      setLines((current) => [
        ...current,
        { id: `note-${current.length}`, kind: "note", text: `已投递消息（triggerTurn=${triggerTurn}）` },
      ]);
      setDraft("");
    } catch (reason) {
      onError(errorMessage(reason));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="subagent-detail">
      <header className="subagent-detail-header">
        <button className="icon-button" type="button" onClick={onBack} aria-label="返回列表" title="返回">
          <ArrowLeft size={15} />
        </button>
        <div className="subagent-detail-title">
          <strong>{String(agent.label)}</strong>
          <span className="subagent-detail-state" style={{ color: statusInfo.color }}>
            <statusInfo.Icon size={12} className={cn(statusInfo.spinning && "spin")} />
            {statusInfo.label}
          </span>
        </div>
        {activeStates.has(agent.state) ? (
          <button className="agent-button agent-button--danger" type="button" onClick={onStop}>
            <Square size={12} fill="currentColor" />
            停止
          </button>
        ) : ["failed", "cancelled", "timed_out"].includes(agent.state) ? (
          <button className="agent-button agent-button--secondary" type="button" onClick={onResume}>
            <RotateCcw size={13} />
            恢复
          </button>
        ) : null}
      </header>

      <dl className="subagent-detail-meta">
        <div>
          <dt>路径</dt>
          <dd className="subagent-path">{agent.agentPath ?? "-"}</dd>
        </div>
        <div>
          <dt>层级</dt>
          <dd>{agent.depth}</dd>
        </div>
        <div>
          <dt>继承</dt>
          <dd>{agent.forkMode ?? "none"}</dd>
        </div>
        <div>
          <dt>轮次</dt>
          <dd>{agent.turnCount ?? 0}</dd>
        </div>
      </dl>

      <div className="subagent-detail-stream" ref={scrollRef}>
        {history.map((message) => (
          <article key={message.id} className={cn("subagent-message", `subagent-message--${message.role}`)}>
            <span className="subagent-message-role">{message.role === "user" ? "任务" : "智能体"}</span>
            <div className="subagent-message-body">
              <MarkdownContent text={messageText(message)} />
            </div>
          </article>
        ))}
        {lines.map((line) =>
          line.kind === "text" ? (
            <article key={line.id} className="subagent-message subagent-message--assistant">
              <span className="subagent-message-role">智能体</span>
              <div className="subagent-message-body">
                <MarkdownContent text={line.text} />
              </div>
            </article>
          ) : (
            <div key={line.id} className={cn("subagent-stream-note", line.kind === "tool" && "subagent-stream-note--tool")}>
              {line.text}
            </div>
          ),
        )}
        {activeStates.has(agent.state) && lines.length === 0 && (
          <div className="subagent-stream-note">等待子智能体输出…</div>
        )}
        {agent.summary && (
          <div className="subagent-detail-summary">
            <strong>执行摘要</strong>
            <p>{agent.summary}</p>
          </div>
        )}
        {agent.error && <div className="agent-error">{agent.error}</div>}
      </div>

      <form
        className="subagent-detail-composer"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <textarea
          rows={2}
          value={draft}
          placeholder="向该子智能体发送消息…"
          onChange={(event) => setDraft(event.target.value)}
        />
        <div className="subagent-detail-composer-actions">
          <label>
            <input
              type="checkbox"
              checked={triggerTurn}
              onChange={(event) => setTriggerTurn(event.target.checked)}
            />
            <span>立即打断并转向</span>
          </label>
          <button
            type="submit"
            className="agent-button agent-button--primary"
            disabled={!draft.trim() || sending}
          >
            {sending ? <Loader2 className="spin" size={13} /> : <Send size={13} />}
            发送
          </button>
        </div>
      </form>
    </div>
  );
}

function applyEvent(current: DetailLine[], event: AgentEvent): DetailLine[] {
  switch (event.type) {
    case "text_delta": {
      const next = [...current];
      const last = next[next.length - 1];
      if (last && last.kind === "text" && last.pending) {
        next[next.length - 1] = { ...last, text: last.text + event.delta };
      } else {
        next.push({ id: `text-${next.length}`, kind: "text", text: event.delta, pending: true });
      }
      return next;
    }
    case "tool_started":
      return [
        ...current.map((line) => (line.pending ? { ...line, pending: false } : line)),
        { id: `tool-${current.length}`, kind: "tool", text: `调用工具 ${event.call?.name ?? "tool"}` },
      ];
    case "tool_completed":
      return [
        ...current,
        {
          id: `tool-done-${current.length}`,
          kind: "tool",
          text: `${event.name} ${event.result?.success ? "完成" : "失败"}`,
        },
      ];
    case "turn_completed":
    case "turn_failed":
    case "turn_cancelled":
      return current.map((line) => (line.pending ? { ...line, pending: false } : line));
    default:
      return current;
  }
}

function messageText(message: ChatMessage): string {
  const blocks = (message.content ?? []) as unknown as Array<{ text?: string }>;
  return blocks
    .map((block) => (typeof block?.text === "string" ? block.text : ""))
    .join("");
}

function summarize(agent: SubagentView): string {
  if (agent.error) return agent.error;
  if (agent.summary) return agent.summary;
  return agent.task;
}

function formatElapsed(createdAtMs: number, updatedAtMs: number): string {
  const ms = Math.max(updatedAtMs - createdAtMs, 0);
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m${seconds % 60}s`;
}

function formatTokenUsage(tokensUsed: number, tokenBudget: number | null) {
  return tokenBudget === null
    ? `${tokensUsed.toLocaleString()} tokens`
    : `${tokensUsed.toLocaleString()} / ${tokenBudget.toLocaleString()} tokens`;
}

function upsert(agents: SubagentView[], incoming: SubagentView) {
  const existing = agents.some((agent) => agent.id === incoming.id);
  return existing
    ? agents.map((agent) => (agent.id === incoming.id ? incoming : agent))
    : [incoming, ...agents];
}

function sortAgents(agents: SubagentView[]) {
  return [...agents].sort((a, b) => b.updatedAtMs - a.updatedAtMs);
}

function getStatusInfo(state: SubagentState) {
  const statusMap = {
    queued: { label: "排队中", Icon: Clock, color: "#3B82F6", spinning: false },
    running: { label: "运行中", Icon: Loader2, color: "#22C55E", spinning: true },
    blocked: { label: "等待审批", Icon: Pause, color: "#F59E0B", spinning: false },
    completed: { label: "已完成", Icon: CheckCircle2, color: "#10B981", spinning: false },
    failed: { label: "失败", Icon: XCircle, color: "#EF4444", spinning: false },
    cancelled: { label: "已取消", Icon: XCircle, color: "#6B7280", spinning: false },
    timed_out: { label: "已超时", Icon: Timer, color: "#F97316", spinning: false },
  };

  return statusMap[state] || statusMap.queued;
}
