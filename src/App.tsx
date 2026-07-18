import { FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  Activity,
  Archive,
  CircleAlert,
  Code2,
  KeyRound,
  MessageSquare,
  PanelRight,
  Plus,
  RefreshCw,
  SendHorizontal,
  Settings,
  Square,
  X,
} from "lucide-react";
import { getRuntimeStatus, subscribeToAgentEvents } from "./api/runtime";
import { useWorkbenchStore } from "./stores/workbenchStore";
import type { RuntimeStatus, SaveProviderConfigRequest } from "./types/runtime";
import "./App.css";

const DEFAULT_BASE_URL = "https://api.openai.com/v1";

function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [runtimeError, setRuntimeError] = useState("");
  const [draft, setDraft] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const messageAreaRef = useRef<HTMLDivElement>(null);
  const {
    threads,
    activeThreadId,
    messages,
    lastTurn,
    activeTurnId,
    usage,
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

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;
  const retryable = !activeTurnId && ["failed", "cancelled"].includes(lastTurn?.state ?? "");

  function submitMessage(event: FormEvent) {
    event.preventDefault();
    const message = draft.trim();
    if (!message || activeTurnId) return;
    setDraft("");
    void sendMessage(message);
  }

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
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
            aria-label="设置 Provider"
            title="设置 Provider"
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
              <p>暂无消息</p>
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
                <SendHorizontal size={17} />
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
            <div><strong>当前 Turn</strong><span>{activeTurnId ? "流式响应中" : lastTurn ? stateLabel(lastTurn.state) : "空闲"}</span></div>
          </div>
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
        <ProviderSettings
          initial={providerConfig}
          error={error}
          onClose={() => setSettingsOpen(false)}
          onSave={saveProvider}
        />
      )}
    </main>
  );
}

interface ProviderSettingsProps {
  initial: ReturnType<typeof useWorkbenchStore.getState>["providerConfig"];
  onClose: () => void;
  onSave: (request: SaveProviderConfigRequest) => Promise<boolean>;
  error: string;
}

function ProviderSettings({ initial, onClose, onSave, error }: ProviderSettingsProps) {
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? DEFAULT_BASE_URL);
  const [model, setModel] = useState(initial?.model ?? "");
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    const saved = await onSave({
      kind: "open_ai_compatible",
      baseUrl,
      model,
      ...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
    });
    setSaving(false);
    if (saved) onClose();
  }

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="provider-settings-title">
        <header>
          <div><KeyRound size={18} /><h2 id="provider-settings-title">Provider 设置</h2></div>
          <button className="icon-button" type="button" aria-label="关闭设置" title="关闭" onClick={onClose}><X size={17} /></button>
        </header>
        <form onSubmit={submit}>
          <label>
            <span>类型</span>
            <select value="open_ai_compatible" disabled>
              <option value="open_ai_compatible">OpenAI-compatible</option>
            </select>
          </label>
          <label>
            <span>API 地址</span>
            <input type="url" required value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} />
          </label>
          <label>
            <span>模型</span>
            <input required value={model} onChange={(event) => setModel(event.target.value)} placeholder="模型名称" />
          </label>
          <label>
            <span>API Key</span>
            <input
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder={initial?.hasApiKey ? "已安全保存" : "输入 API Key"}
              required={!initial?.hasApiKey}
              autoComplete="off"
            />
          </label>
          {error && <div className="settings-error" role="alert"><CircleAlert size={15} /><span>{error}</span></div>}
          <footer>
            <button className="secondary-button" type="button" onClick={onClose}>取消</button>
            <button className="primary-button" type="submit" disabled={saving}>{saving ? "保存中" : "保存"}</button>
          </footer>
        </form>
      </section>
    </div>
  );
}

function stateLabel(state: string) {
  switch (state) {
    case "completed": return "已完成";
    case "failed": return "失败";
    case "cancelled": return "已取消";
    case "streaming": return "响应中";
    default: return state;
  }
}

export default App;
