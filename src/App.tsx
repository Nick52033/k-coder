import { FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  Activity,
  Archive,
  ArrowUp,
  CircleAlert,
  Code2,
  FileDiff,
  KeyRound,
  MessageSquare,
  Moon,
  PanelRight,
  Plus,
  RefreshCw,
  Settings,
  Square,
  Sun,
  Undo2,
  X,
} from "lucide-react";
import { getRuntimeStatus, subscribeToAgentEvents } from "./api/runtime";
import { useWorkbenchStore } from "./stores/workbenchStore";
import { PatchReviewDialog } from "./components/PatchReviewDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import type { RuntimeStatus } from "./types/runtime";
import "./App.css";

type Skin = "paper" | "midnight" | "amethyst";
type ThemeMode = "light" | "dark";

const STORAGE_SKIN = "kcoder_skin";
const STORAGE_THEME = "kcoder_theme";

function readStored<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (raw as T) : fallback;
  } catch {
    return fallback;
  }
}

function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [runtimeError, setRuntimeError] = useState("");
  const [draft, setDraft] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [selectedChangeId, setSelectedChangeId] = useState<string | null>(null);
  const [skin, setSkinState] = useState<Skin>(() => readStored(STORAGE_SKIN, "paper"));
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() =>
    readStored(STORAGE_THEME, "light"),
  );
  const messageAreaRef = useRef<HTMLDivElement>(null);
  const {
    threads,
    activeThreadId,
    messages,
    lastTurn,
    activeTurnId,
    usage,
    toolActivities,
    pendingApproval,
    changes,
    providerConfig,
    loading,
    error,
    initialize,
    createThread,
    selectThread,
    archiveActiveThread,
    sendMessage,
    retryLastTurn,
    stopTurn,
    resolvePendingApproval,
    undoAppliedChange,
    saveProvider,
    handleAgentEvent,
    clearError,
  } = useWorkbenchStore();

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    async function connect() {
      try {
        const stopListening = await subscribeToAgentEvents(handleAgentEvent);
        if (disposed) stopListening();
        else unlisten = stopListening;
        await initialize();
      } catch (error) {
        if (!disposed) setRuntimeError(String(error));
      }
    }

    void connect();
    getRuntimeStatus()
      .then((status) => {
        if (!disposed) setRuntime(status);
      })
      .catch((reason: unknown) => {
        if (!disposed) setRuntimeError(String(reason));
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleAgentEvent, initialize]);

  useEffect(() => {
    const area = messageAreaRef.current;
    if (area) area.scrollTop = area.scrollHeight;
  }, [messages]);

  useEffect(() => {
    document.documentElement.setAttribute("data-skin", skin);
    document.documentElement.setAttribute("data-theme", themeMode);
  }, [skin, themeMode]);

  const setSkin = (next: Skin) => {
    setSkinState(next);
    try { localStorage.setItem(STORAGE_SKIN, next); } catch { /* noop */ }
  };

  const toggleTheme = () => {
    const next = themeMode === "light" ? "dark" : "light";
    setThemeModeState(next);
    try { localStorage.setItem(STORAGE_THEME, next); } catch { /* noop */ }
  };

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const selectedChange = changes.find((change) => change.id === selectedChangeId) ?? null;
  const retryable = !activeTurnId && ["failed", "cancelled"].includes(lastTurn?.state ?? "");

  function submitMessage(event: FormEvent) {
    event.preventDefault();
    const message = draft.trim();
    if (!message || activeTurnId) return;
    setDraft("");
    void sendMessage(message);
  }

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (
      event.key === "Enter"
      && !event.shiftKey
      && !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  }

  function openSettings() {
    clearError();
    setSettingsOpen(true);
  }

  return (
    <main className="workbench">
      <header className="titlebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true"><Code2 size={17} /></span>
          <strong>k-Coder</strong>
        </div>
        <div className="titlebar-actions">
          <span className={`runtime-state ${runtimeError ? "runtime-state--error" : ""}`}>
            {runtimeError ? <CircleAlert size={14} /> : <Activity size={14} />}
            {runtimeError ? "运行时不可用" : runtime ? "运行时就绪" : "正在连接"}
          </span>
          <button
            className="icon-button"
            type="button"
            aria-label={themeMode === "light" ? "切换到深色模式" : "切换到浅色模式"}
            title={themeMode === "light" ? "深色模式" : "浅色模式"}
            onClick={toggleTheme}
          >
            {themeMode === "light" ? <Moon size={17} /> : <Sun size={17} />}
          </button>
          <button
            className="icon-button"
            type="button"
            aria-label="打开设置"
            title="设置"
            onClick={openSettings}
          >
            <Settings size={17} />
          </button>
        </div>
      </header>

      <aside className="sidebar">
        <button className="new-thread-button" type="button" onClick={() => void createThread()}>
          <Plus size={16} />
          新建会话
        </button>
        <div className="section-label">会话</div>
        <nav className="thread-list" aria-label="会话列表">
          {threads.map((thread) => (
            <button
              className={`thread-item ${thread.id === activeThreadId ? "thread-item--active" : ""}`}
              key={thread.id}
              type="button"
              onClick={() => void selectThread(thread.id)}
            >
              <MessageSquare size={15} />
              <span>{thread.title}</span>
            </button>
          ))}
        </nav>
        <button
          className={`provider-button ${providerConfig ? "provider-button--ready" : ""}`}
          type="button"
          onClick={openSettings}
        >
          <KeyRound size={16} />
          <span>{providerConfig?.model ?? "配置 Provider"}</span>
        </button>
      </aside>

      <section className="conversation">
        <div className="conversation-header">
          <div>
            <h1>{activeThread?.title ?? "新会话"}</h1>
            <span className="mode-label">
              {activeTurnId ? "正在生成" : usage ? `${usage.totalTokens} tokens` : "纯文本对话"}
            </span>
          </div>
          <div className="conversation-actions">
            <button
              className="icon-button"
              type="button"
              aria-label="归档会话"
              title="归档会话"
              disabled={!activeThread || Boolean(activeTurnId)}
              onClick={() => void archiveActiveThread()}
            >
              <Archive size={17} />
            </button>
            <button className="icon-button" type="button" aria-label="切换活动面板" title="切换活动面板">
              <PanelRight size={17} />
            </button>
          </div>
        </div>

        <div className={`message-area ${messages.length ? "message-area--populated" : ""}`} ref={messageAreaRef}>
          {loading && !messages.length ? (
            <div className="empty-thread"><Activity className="spin" size={24} /><p>正在读取会话</p></div>
          ) : messages.length ? (
            <div className="message-list">
              {messages.map((message) => (
                <article className={`message message--${message.role}`} key={message.id}>
                  <div className="message-role">{message.role === "user" ? "你" : "k-Coder"}</div>
                  <div className="message-content">
                    {message.text || (message.status === "streaming" ? <span className="typing-indicator">•••</span> : null)}
                  </div>
                  {message.status === "failed" && <div className="message-status message-status--error">生成失败</div>}
                  {message.status === "cancelled" && <div className="message-status">已停止</div>}
                </article>
              ))}
              {retryable && (
                <button className="retry-button" type="button" onClick={() => void retryLastTurn()}>
                  <RefreshCw size={15} />
                  重试
                </button>
              )}
            </div>
          ) : (
            <div className="empty-thread">
              <Code2 size={26} />
              <p>开始对话 — 输入消息与 AI 协作</p>
            </div>
          )}
        </div>

        {error && (
          <div className="error-banner" role="alert">
            <CircleAlert size={16} />
            <span>{error}</span>
            <button type="button" aria-label="关闭错误" title="关闭" onClick={clearError}><X size={15} /></button>
          </div>
        )}

        <form className="composer" onSubmit={submitMessage}>
          <textarea
            aria-label="消息"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleComposerKeyDown}
            placeholder="输入消息"
            rows={3}
            disabled={Boolean(activeTurnId)}
          />
          <div className="composer-footer">
            <span className="composer-mode">{providerConfig?.model ?? "未配置模型"}</span>
            {activeTurnId ? (
              <button className="stop-button" type="button" aria-label="停止生成" title="停止生成" onClick={() => void stopTurn()}>
                <Square size={15} fill="currentColor" />
              </button>
            ) : (
              <button
                className="send-button"
                type="submit"
                aria-label="发送消息"
                title="发送消息"
                disabled={!draft.trim()}
              >
                <ArrowUp size={18} strokeWidth={2.2} />
              </button>
            )}
          </div>
        </form>
      </section>

      <aside className="activity-panel">
        <div className="activity-header">
          <Activity size={16} />
          <h2>活动</h2>
        </div>
        <div className="activity-list">
          <div className="activity-row">
            <span className={`activity-dot ${runtime ? "activity-dot--success" : ""}`} />
            <div><strong>运行时</strong><span>{runtime ? `v${runtime.version}` : "等待中"}</span></div>
          </div>
          <div className="activity-row">
            <span className={`activity-dot ${providerConfig ? "activity-dot--success" : ""}`} />
            <div><strong>Provider</strong><span>{providerConfig?.model ?? "未配置"}</span></div>
          </div>
          <div className="activity-row">
            <span className={`activity-dot ${activeTurnId ? "activity-dot--active" : ""}`} />
            <div><strong>当前 Turn</strong><span>{lastTurn ? stateLabel(lastTurn.state) : "空闲"}</span></div>
          </div>
          {toolActivities.slice(-8).map((activity) => (
            <div className="activity-row activity-row--tool" key={`${activity.turnId}-${activity.call.id}`}>
              <span
                className={`activity-dot ${
                  activity.state === "running"
                    ? "activity-dot--active"
                    : activity.state === "completed"
                      ? "activity-dot--success"
                      : "activity-dot--error"
                }`}
              />
              <div>
                <strong>{activity.call.name}</strong>
                <span title={toolActivityDetail(activity)}>{toolActivityDetail(activity)}</span>
              </div>
            </div>
          ))}
          {changes.length > 0 && (
            <div className="activity-changes">
              <div className="activity-section-title">代码变更</div>
              {changes.slice(-4).reverse().map((change) => (
                <div className="activity-change" key={change.id}>
                  <button
                    className="activity-change-main"
                    type="button"
                    title="查看变更"
                    onClick={() => setSelectedChangeId(change.id)}
                  >
                    <FileDiff size={15} />
                    <span>
                      <strong>{change.files.length} 个文件</strong>
                      <small>{change.undone ? "已撤销" : "已应用"}</small>
                    </span>
                  </button>
                  {!change.undone && (
                    <button
                      className="activity-change-undo"
                      type="button"
                      title="撤销变更"
                      aria-label="撤销变更"
                      disabled={Boolean(activeTurnId)}
                      onClick={() => void undoAppliedChange(change.id)}
                    >
                      <Undo2 size={14} />
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
          {usage && (
            <div className="usage-block">
              <div><span>输入</span><strong>{usage.inputTokens}</strong></div>
              <div><span>输出</span><strong>{usage.outputTokens}</strong></div>
              <div><span>总计</span><strong>{usage.totalTokens}</strong></div>
            </div>
          )}
        </div>
      </aside>

      {settingsOpen && (
        <SettingsDialog
          provider={providerConfig}
          error={error}
          skin={skin}
          themeMode={themeMode}
          onClose={() => setSettingsOpen(false)}
          onSetSkin={setSkin}
          onToggleTheme={toggleTheme}
          onSaveProvider={saveProvider}
        />
      )}
      {pendingApproval && (
        <PatchReviewDialog
          request={pendingApproval}
          error={error}
          onResolve={resolvePendingApproval}
        />
      )}
      {!pendingApproval && selectedChange && (
        <PatchReviewDialog
          change={selectedChange}
          error={error}
          onUndo={undoAppliedChange}
          onClose={() => setSelectedChangeId(null)}
        />
      )}
    </main>
  );
}

function stateLabel(state: string) {
  switch (state) {
    case "completed": return "已完成";
    case "failed": return "失败";
    case "cancelled": return "已取消";
    case "streaming": return "响应中";
    case "running_tool": return "执行工具";
    case "awaiting_approval": return "等待审阅";
    default: return state;
  }
}

function toolActivityDetail(activity: {
  state: "running" | "completed" | "failed";
  call: { arguments: Record<string, unknown> };
  result: { output: string } | null;
}) {
  if (activity.state === "running") return "执行中";
  if (activity.state === "failed") return activity.result?.output || "执行失败";
  const path = activity.call.arguments.path;
  return typeof path === "string" ? path : "已完成";
}

export default App;
