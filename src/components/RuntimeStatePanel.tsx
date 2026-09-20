import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Activity, ChevronDown, CircleAlert } from "lucide-react";
import { cn } from "../lib/cn";
import type { RuntimeStatus } from "../types/runtime";
import "./RuntimeStatePanel.css";

interface RuntimeTurnSummary {
  /** 当前会话最近一次 Turn 的状态，例如「响应中」「已完成」「空闲」。 */
  label: string;
  /** 同一状态的补充说明，通常是模型给出的错误原因。 */
  detail: string;
}

interface RuntimeStatePanelProps {
  runtime: RuntimeStatus | null;
  runtimeError: string;
  /** 当前生效模型；未配置 Provider 时为 null。 */
  model: string | null;
  /** 当前会话的 Turn 摘要；空闲时为 null。 */
  turn: RuntimeTurnSummary | null;
}

/** 能力标识的中文名，未收录的标识按原样展示。 */
const CAPABILITY_LABELS: Record<string, string> = {
  "streaming-chat": "流式对话",
  "persistent-threads": "会话持久化",
  cancellation: "取消 Turn",
  "native-tool-calling": "原生工具调用",
  "workspace-read-tools": "工作区读取工具",
  "workspace-write-tools": "工作区写入工具",
  "reviewable-patches": "可审阅补丁",
  "change-undo": "变更撤销",
  "command-sessions": "命令会话",
  "bounded-command-output": "命令输出截断",
  "process-tree-cancellation": "进程树清理",
  "command-risk-policy": "命令风险策略",
  "pty-terminal": "PTY 终端",
  "sqlite-projections": "SQLite 投影",
  "context-budgeting": "上下文预算",
  "context-compaction": "上下文压缩",
  "crash-recovery": "崩溃恢复",
  "structured-logging": "结构化日志",
  "programming-workbench": "编程工作台",
  "runtime-instructions": "运行时指令",
  skills: "Skill",
  "mcp-stdio": "MCP Stdio",
  "mcp-streamable-http": "MCP Streamable HTTP",
  "tool-hooks": "工具钩子",
  "extension-diagnostics": "扩展诊断",
  "extension-audit": "扩展审计",
  "multi-agent-delegation": "多智能体委派",
  "bounded-subagents": "子智能体上限",
  "subagent-cancellation": "子智能体取消",
  "subagent-persistence": "子智能体持久化",
  "persistent-plans": "计划持久化",
  "plan-mode": "计划模式",
  "user-input-tool": "向用户提问工具",
  "budgeted-goals": "预算目标",
  "builtin-workflows": "内置机器人工作流",
  "browser-automation": "浏览器自动化",
  "repository-search": "仓库搜索",
  "opt-in-memory": "选择性记忆",
  "bounded-document-extraction": "文档抽取",
  "runtime-metrics": "运行时指标",
  "knowledge-fts": "知识库全文检索",
  "knowledge-citations": "知识库引用",
  "embedding-configuration": "向量配置",
};

function formatUptime(uptimeSeconds: number) {
  if (!Number.isFinite(uptimeSeconds) || uptimeSeconds < 0) return "--";
  const total = Math.floor(uptimeSeconds);
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (days > 0) return `${days} 天 ${hours} 小时`;
  if (hours > 0) return `${hours} 小时 ${minutes} 分`;
  if (minutes > 0) return `${minutes} 分 ${seconds} 秒`;
  return `${seconds} 秒`;
}

function capabilityLabel(capability: string) {
  return CAPABILITY_LABELS[capability] ?? capability;
}

/**
 * 标题栏运行时状态入口。静态标签只说明"是否连上"，看不到任何可操作信息，
 * 因此改为可点击面板：展开后给出 phase、版本、运行时长、当前模型与 Turn 状态、
 * 能力清单，以及连接失败时的原始错误详情。
 */
export function RuntimeStatePanel({ runtime, runtimeError, model, turn }: RuntimeStatePanelProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [popoverPosition, setPopoverPosition] = useState({ top: 0, left: 0, width: 320 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();

  const failed = Boolean(runtimeError);
  const triggerLabel = failed ? "运行时不可用" : runtime ? "运行时就绪" : "正在连接";
  const capabilities = runtime?.capabilities ?? [];

  function updatePopoverPosition() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const viewportPadding = 12;
    const width = Math.min(320, Math.max(260, window.innerWidth - viewportPadding * 2));
    setPopoverPosition({
      top: Math.min(rect.bottom + 7, window.innerHeight - viewportPadding),
      left: Math.max(
        viewportPadding,
        Math.min(rect.right - width, window.innerWidth - width - viewportPadding),
      ),
      width,
    });
  }

  useEffect(() => {
    if (!isOpen) return;

    function handlePointerDown(event: MouseEvent) {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !popoverRef.current?.contains(target)) {
        setIsOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      setIsOpen(false);
      triggerRef.current?.focus();
    }

    updatePopoverPosition();
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("resize", updatePopoverPosition);
    window.addEventListener("scroll", updatePopoverPosition, true);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("resize", updatePopoverPosition);
      window.removeEventListener("scroll", updatePopoverPosition, true);
    };
  }, [isOpen]);

  return (
    <div className="runtime-state-panel">
      <button
        ref={triggerRef}
        className={cn("runtime-state-trigger", failed && "runtime-state-trigger--error")}
        type="button"
        aria-label="运行时状态"
        aria-expanded={isOpen}
        aria-haspopup="dialog"
        aria-controls={isOpen ? popoverId : undefined}
        title="查看运行时状态"
        onClick={() => {
          if (!isOpen) updatePopoverPosition();
          setIsOpen((open) => !open);
        }}
      >
        {failed ? <CircleAlert size={14} aria-hidden="true" /> : <Activity size={14} aria-hidden="true" />}
        <span className="runtime-state-label">{triggerLabel}</span>
        <ChevronDown className="runtime-state-chevron" size={13} aria-hidden="true" />
      </button>

      {isOpen && createPortal(
        <div
          ref={popoverRef}
          id={popoverId}
          className="runtime-state-popover composer-popover-surface"
          role="dialog"
          aria-label="运行时状态"
          style={popoverPosition}
        >
          <div className="runtime-state-popover-heading">
            <span className="runtime-state-popover-dot" aria-hidden="true" />
            <div>
              <strong>运行时</strong>
              <span>{failed ? "连接失败" : runtime ? runtime.phase : "尚未连接"}</span>
            </div>
          </div>

          <dl className="runtime-state-grid">
            <div>
              <dt>状态</dt>
              <dd>{triggerLabel}</dd>
            </div>
            <div>
              <dt>版本</dt>
              <dd>{runtime ? `v${runtime.version}` : "--"}</dd>
            </div>
            <div>
              <dt>运行时长</dt>
              <dd>{runtime ? formatUptime(runtime.uptimeSeconds) : "--"}</dd>
            </div>
            <div>
              <dt>当前模型</dt>
              <dd>{model ?? "未配置"}</dd>
            </div>
            <div>
              <dt>当前 Turn</dt>
              <dd>{turn?.label ?? "空闲"}</dd>
            </div>
          </dl>

          {turn?.detail ? <p className="runtime-state-turn-detail">{turn.detail}</p> : null}

          {failed ? (
            <div className="runtime-state-error">
              <strong>错误详情</strong>
              <pre>{runtimeError}</pre>
            </div>
          ) : null}

          <div className="runtime-state-capabilities">
            <div className="runtime-state-capabilities-heading">
              <strong>能力</strong>
              <span>{capabilities.length} 项</span>
            </div>
            {capabilities.length > 0 ? (
              <ul>
                {capabilities.map((capability) => (
                  <li key={capability} title={capability}>
                    {capabilityLabel(capability)}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="runtime-state-capabilities-empty">连接后展示运行时能力清单</p>
            )}
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}
