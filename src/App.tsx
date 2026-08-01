import { FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  Activity,
  Archive,
  ArrowUp,
  CircleAlert,
  Bot,
  Code2,
  FileDiff,
  Loader2,
  Maximize2,
  MessageSquare,
  Minus,
  Moon,
  Paperclip,
  PanelRight,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Square,
  Sun,
  Trash2,
  Undo2,
  X,
  Target,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getRuntimeStatus, subscribeToAgentEvents, listSubagents } from "./api/runtime";
import { useWorkbenchStore } from "./stores/workbenchStore";
import { PatchReviewDialog } from "./components/PatchReviewDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { WorkbenchPanel, WorkspacePicker } from "./components/WorkbenchPanel";
import { AgentActivityPanel } from "./components/AgentActivityPanel";
import { ModelSelector } from "./components/ModelSelector";
import { cn } from "./lib/cn";
import type { AttachmentContent, GoalView, RuntimeStatus } from "./types/runtime";
import "./App.css";
import "./enhanced-animations.css"; // UI 增强动画
import "./components/ModeSelector.css";

type Skin = "paper" | "midnight" | "vscode" | "amber" | "codebuddy";
type ThemeMode = "light" | "dark";

const STORAGE_SKIN = "kcoder_skin";
const STORAGE_THEME = "kcoder_theme";
const appWindow = getCurrentWindow();

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
  const [attachments, setAttachments] = useState<AttachmentContent[]>([]);
  const [threadQuery, setThreadQuery] = useState("");
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const [agentPanelOpen, setAgentPanelOpen] = useState(false);
  const [workspaceRevision, setWorkspaceRevision] = useState(0);
  const [skin, setSkinState] = useState<Skin>(() => readStored(STORAGE_SKIN, "paper"));
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() =>
    readStored(STORAGE_THEME, "light"),
  );
  const [subagentThreadIds, setSubagentThreadIds] = useState<Set<string>>(new Set());
  const [agentMode, setAgentMode] = useState<"craft" | "ask" | "plan">("craft");
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  const [expandedChangeSets, setExpandedChangeSets] = useState<Set<string>>(new Set());
  const [queueExpanded, setQueueExpanded] = useState(false);
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
    providerConfigs,
    activeProviderId,
    plan,
    goal,
    loading,
    error,
    messageQueue,
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
    activateProvider,
    deleteProvider,
    handleAgentEvent,
    clearError,
    searchThreadHistory,
    renameConversation,
    deleteConversation,
    createActiveGoal,
    transitionActiveGoal,
    clearQueue,
    forceResetState,
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

        // Fetch all subagents to mark their threads
        const subagents = await listSubagents();
        if (!disposed) {
          const threadIds = new Set(subagents.map(subagent => subagent.threadId));
          setSubagentThreadIds(threadIds);
        }
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

    // 添加紧急状态重置快捷键 Ctrl+Shift+R
    function handleEmergencyReset(event: globalThis.KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        // 使用新的 forceResetState 函数
        void forceResetState();
        console.log("紧急状态已重置");
      }
    }
    window.addEventListener("keydown", handleEmergencyReset);

    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("keydown", handleEmergencyReset);
    };
  }, [handleAgentEvent, initialize, forceResetState]);

  useEffect(() => {
    const area = messageAreaRef.current;
    if (area) area.scrollTop = area.scrollHeight;
  }, [messages]);

  useEffect(() => {
    document.documentElement.setAttribute("data-skin", skin);
    document.documentElement.setAttribute("data-theme", themeMode);
  }, [skin, themeMode]);

  useEffect(() => {
    function handleShortcut(event: globalThis.KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void createThread();
      }
      if ((event.ctrlKey || event.metaKey) && event.key === ",") {
        event.preventDefault();
        setSettingsOpen(true);
      }
      if (event.key === "Escape") {
        setWorkbenchOpen(false);
        setAgentPanelOpen(false);
        setModeMenuOpen(false);
      }
    }
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [createThread]);

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
    const attachmentContext = attachments.filter((attachment) => attachment.kind === "document").map((attachment) =>
      `\n\n[附件: ${attachment.name}]\n${attachment.content}`,
    ).join("");
    const imageAttachments = attachments
      .filter((attachment) => attachment.kind === "image")
      .map((attachment) => ({ name: attachment.name, dataUrl: attachment.content }));
    setDraft("");
    setAttachments([]);
    void sendMessage(message + attachmentContext, imageAttachments, agentMode);
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

  function handlePaste(event: React.ClipboardEvent<HTMLTextAreaElement>) {
    const items = event.clipboardData.items;
    const imageFiles: File[] = [];
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.type.startsWith("image/")) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length === 0) return; // 没有图片，使用默认粘贴行为（粘贴文本）

    event.preventDefault();
    for (const file of imageFiles) {
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = reader.result as string;
        const name = file.name || `paste-${Date.now()}.png`;
        setAttachments((items) => {
          // 避免重复添加同名图片
          const path = `clipboard://${name}`;
          if (items.some((a) => a.path === path)) return items;
          return [
            ...items,
            {
              path,
              name,
              kind: "image",
              content: dataUrl,
              size: file.size,
              truncated: false,
            },
          ];
        });
      };
      reader.readAsDataURL(file);
    }
  }

  function openSettings() {
    clearError();
    setWorkbenchOpen(false);
    setAgentPanelOpen(false);
    setSettingsOpen(true);
  }

  return (
    <main className={cn("workbench", workbenchOpen && "workbench--panel-open")}>
      <header className="titlebar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <span className="brand-mark" aria-hidden="true">K</span>
          <strong>k-Coder</strong>
        </div>
        <div className="titlebar-actions">
          <span className={cn("runtime-state", runtimeError && "runtime-state--error")}>
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
            className="icon-button mobile-settings-button"
            type="button"
            aria-label="设置"
            title="打开设置"
            onClick={openSettings}
          >
            <Settings size={17} />
          </button>
          <div className="window-controls">
            <button
              className="window-control"
              type="button"
              aria-label="最小化窗口"
              title="最小化"
              onClick={() => void appWindow.minimize()}
            >
              <Minus size={16} />
            </button>
            <button
              className="window-control"
              type="button"
              aria-label="最大化或还原窗口"
              title="最大化或还原"
              onClick={() => void appWindow.toggleMaximize()}
            >
              <Maximize2 size={14} />
            </button>
            <button
              className="window-control window-control--close"
              type="button"
              aria-label="关闭窗口"
              title="关闭"
              onClick={() => void appWindow.close()}
            >
              <X size={16} />
            </button>
          </div>
        </div>
      </header>

      <aside className="sidebar">
        <div className="sidebar-header">
          <WorkspacePicker onChanged={() => { setWorkspaceRevision((value) => value + 1); void initialize(); }} />
          <button className="new-thread-button" type="button" onClick={() => void createThread()} title="新建会话" aria-label="新建会话">
            <Plus size={18} />
          </button>
        </div>
        <section className="thread-section" aria-labelledby="thread-section-title">
          <div className="thread-section-heading">
            <span id="thread-section-title">会话</span>
            <span className="thread-count" aria-label={`${threads.length} 个会话`}>{threads.length}</span>
          </div>
          <label className="thread-search">
            <Search size={14} aria-hidden="true" />
            <input aria-label="搜索会话" placeholder="搜索会话" value={threadQuery} onChange={(event) => { const query = event.target.value; setThreadQuery(query); void searchThreadHistory(query); }} />
            {threadQuery && (
              <button type="button" title="清除搜索" aria-label="清除搜索" onClick={() => { setThreadQuery(""); void searchThreadHistory(""); }}>
                <X size={13} />
              </button>
            )}
          </label>
          <nav className="thread-list" aria-label="会话列表">
            {threads.map((thread) => {
              const isSubagentThread = subagentThreadIds.has(thread.id);
              return (
                <div className={cn("thread-item", thread.id === activeThreadId && "thread-item--active")} key={thread.id}>
                  <button className="thread-item-main" type="button" onClick={() => void selectThread(thread.id)}>
                    <MessageSquare size={15} />
                    <span>{thread.title}</span>
                    {isSubagentThread && (
                      <span className="subagent-badge" title="子智能体线程">
                        <Bot size={12} />
                      </span>
                    )}
                  </button>
                  <span className="thread-actions">
                    <button type="button" title="重命名" aria-label={`重命名会话 ${thread.title}`} onClick={() => { const title = window.prompt("会话名称", thread.title); if (title) void renameConversation(thread.id, title); }}><Pencil size={12} /></button>
                    <button type="button" title="删除" aria-label={`删除会话 ${thread.title}`} onClick={async () => { if (window.confirm(`删除会话"${thread.title}"？`)) await deleteConversation(thread.id); }}><Trash2 size={12} /></button>
                  </span>
                </div>
              );
            })}
            {!threads.length && (
              <div className="thread-empty">
                <MessageSquare size={16} />
                <span>{threadQuery ? "没有匹配的会话" : "还没有会话"}</span>
              </div>
            )}
          </nav>
        </section>

        <div className="sidebar-footer">
          <button
            className="sidebar-settings-button"
            type="button"
            onClick={openSettings}
            aria-label="设置"
            title="打开设置"
          >
            <Settings size={16} />
            <span>设置</span>
          </button>
        </div>
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
            <button className={cn("icon-button", agentPanelOpen && "icon-button--active")} type="button" aria-label="切换子智能体面板" title="子智能体" onClick={() => { setAgentPanelOpen((value) => !value); setWorkbenchOpen(false); }}>
              <Bot size={17} />
            </button>
            <button className={cn("icon-button", workbenchOpen && "icon-button--active")} type="button" aria-label="切换工作台面板" title="切换工作台面板" onClick={() => { setWorkbenchOpen((value) => !value); setAgentPanelOpen(false); }}>
              <PanelRight size={17} />
            </button>
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
          </div>
        </div>

        <div className={cn("message-area", Boolean(messages.length) && "message-area--populated")} ref={messageAreaRef}>
          {loading && !messages.length ? (
            <div className="empty-thread"><Activity className="spin" size={24} /><p>正在读取会话</p></div>
          ) : messages.length ? (
            <div className="message-list">
              {messages.map((message) => {
                // 查找该消息对应回合的文件变更
                const messageChanges = message.role === "assistant" && message.turnId
                  ? changes.filter(change => change.turnId === message.turnId && !change.undone)
                  : [];

                return (
                  <article className={cn("message", `message--${message.role}`, message.status === "streaming" && "message--streaming")} key={message.id}>
                    {message.role === "assistant" && (
                      <div className="message-avatar" aria-hidden="true">
                        <span className="message-avatar-mark">K</span>
                        <span className="message-avatar-pulse" />
                      </div>
                    )}
                    <div className="message-body">
                      <div className="message-role">{message.role === "user" ? "你" : "k-Coder"}</div>
                      <div className="message-content">
                        {message.text || (message.status === "streaming" ? (
                          <span className="typing-indicator" aria-label="AI 正在响应">
                            <span className="typing-dot" />
                            <span className="typing-dot" />
                            <span className="typing-dot" />
                          </span>
                        ) : null)}
                      </div>
                    </div>

                    {/* 文件变更列表 */}
                    {message.role === "assistant" && messageChanges.length > 0 && (
                      <div className="message-changes">
                        <button
                          type="button"
                          className="changes-toggle"
                          onClick={() => {
                            setExpandedChangeSets(prev => {
                              const next = new Set(prev);
                              if (next.has(message.id)) {
                                next.delete(message.id);
                              } else {
                                next.add(message.id);
                              }
                              return next;
                            });
                          }}
                        >
                          <span className={cn("changes-arrow", expandedChangeSets.has(message.id) && "changes-arrow--expanded")}>▶</span>
                          <span>{messageChanges.reduce((sum, change) => sum + change.files.length, 0)} 个文件</span>
                        </button>

                        {expandedChangeSets.has(message.id) && (
                          <div className="changes-list">
                            {messageChanges.flatMap(change => change.files).map((file, idx) => (
                              <div key={idx} className="change-file-item">
                                <span className="change-file-name">{file.path}</span>
                                <span className="change-operation">{file.operation}</span>
                              </div>
                            ))}
                          </div>
                        )}
                      </div>
                    )}

                    {message.status === "failed" && <div className="message-status message-status--error">生成失败</div>}
                    {message.status === "cancelled" && <div className="message-status">已停止</div>}
                  </article>
                );
              })}

              {/* 内嵌授权请求 */}
              {pendingApproval && (
                <article className="message message--approval">
                  <div className="message-avatar" aria-hidden="true">
                    <span className="message-avatar-mark">K</span>
                  </div>
                  <div className="message-body">
                    <div className="message-role">k-Coder</div>
                    <div className="approval-inline">
                      <div className="approval-prompt">
                        确认执行命令？
                      </div>

                      <div className="approval-options">
                        <button
                          type="button"
                          className="approval-option"
                          onClick={() => void resolvePendingApproval({
                            action: "approved",
                            patch: null,
                            selectedPaths: [],
                            expectedHashes: [],
                            scope: "once",
                          })}
                        >
                          运行
                        </button>
                        <button
                          type="button"
                          className="approval-option"
                          onClick={() => void resolvePendingApproval({
                            action: "approved",
                            patch: null,
                            selectedPaths: [],
                            expectedHashes: [],
                            scope: "session",
                          })}
                        >
                          跳过
                        </button>
                        <button
                          type="button"
                          className="approval-option approval-option--danger"
                          onClick={() => void resolvePendingApproval({
                            action: "rejected",
                            patch: null,
                            selectedPaths: [],
                            expectedHashes: [],
                            scope: "once",
                          })}
                        >
                          拒绝
                        </button>
                      </div>
                    </div>
                  </div>
                </article>
              )}

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
            <div style={{ display: 'flex', gap: '4px', alignItems: 'center' }}>
              {activeTurnId && (
                <button
                  type="button"
                  aria-label="强制重置"
                  title="强制重置卡住的状态"
                  onClick={() => void forceResetState()}
                  style={{
                    padding: '2px 6px',
                    fontSize: '0.65rem',
                    borderRadius: '3px',
                    border: '1px solid currentColor',
                    background: 'transparent',
                    cursor: 'pointer',
                    whiteSpace: 'nowrap'
                  }}
                >
                  重置
                </button>
              )}
              <button type="button" aria-label="关闭错误" title="关闭" onClick={clearError}><X size={15} /></button>
            </div>
          </div>
        )}

        {/* 消息队列显示 */}
        {messageQueue.length > 0 && (
          <div className="message-queue">
            <button
              type="button"
              className="queue-toggle"
              onClick={() => setQueueExpanded(!queueExpanded)}
            >
              <span className={cn("queue-arrow", queueExpanded && "queue-arrow--expanded")}>▶</span>
              <span className="queue-title">队列 ({messageQueue.filter(m => m.status === "pending").length})</span>
            </button>

            {queueExpanded && (
              <div className="queue-list">
                {messageQueue.map((queueItem, idx) => (
                  <div key={queueItem.id} className={cn("queue-item", `queue-item--${queueItem.status}`)}>
                    <span className="queue-item-index">{idx + 1}</span>
                    <div className="queue-item-content">
                      <span className="queue-item-text">{queueItem.input.slice(0, 50)}{queueItem.input.length > 50 ? "..." : ""}</span>
                      <span className="queue-item-status">
                        {queueItem.status === "pending" && "等待中"}
                        {queueItem.status === "processing" && "处理中..."}
                        {queueItem.status === "completed" && "已完成"}
                        {queueItem.status === "failed" && "失败"}
                      </span>
                    </div>
                  </div>
                ))}
                {messageQueue.filter(m => m.status === "pending").length > 0 && (
                  <button
                    type="button"
                    className="queue-clear-btn"
                    onClick={clearQueue}
                  >
                    清空队列
                  </button>
                )}
              </div>
            )}
          </div>
        )}

        <GoalControl goal={goal} onManage={() => setSettingsOpen(true)} />
        <form className="composer" onSubmit={submitMessage}>
          {attachments.length > 0 && <div className="attachment-strip">{attachments.map((attachment) => <span key={attachment.path} className={attachment.kind === "image" ? "attachment-tag attachment-tag--image" : "attachment-tag"}>{attachment.kind === "image" ? <img src={attachment.content} alt={attachment.name} className="attachment-thumb" /> : <Paperclip size={12} />}{attachment.name}<button type="button" aria-label={`移除 ${attachment.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== attachment.path))}><X size={12} /></button></span>)}</div>}
          <textarea
            aria-label="消息"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleComposerKeyDown}
            onPaste={handlePaste}
            placeholder={activeTurnId ? "" : "输入消息，可直接粘贴图片"}
            rows={3}
            disabled={Boolean(activeTurnId)}
          />
          <div className="composer-footer">
            <div className="composer-mode-selector">
              <button
                type="button"
                className="mode-toggle"
                onClick={() => setModeMenuOpen(!modeMenuOpen)}
                aria-label="选择模式"
                title="选择交互模式"
              >
                {agentMode === "craft" && (
                  <>
                    <Code2 size={16} />
                    <span>Craft</span>
                  </>
                )}
                {agentMode === "ask" && (
                  <>
                    <MessageSquare size={16} />
                    <span>Ask</span>
                  </>
                )}
                {agentMode === "plan" && (
                  <>
                    <FileDiff size={16} />
                    <span>Plan</span>
                  </>
                )}
              </button>
              {modeMenuOpen && (
                <div className="mode-menu">
                  <button
                    type="button"
                    className={`mode-option ${agentMode === "craft" ? "mode-option--active" : ""}`}
                    onClick={() => {
                      setAgentMode("craft");
                      setModeMenuOpen(false);
                    }}
                  >
                    <Code2 size={16} />
                    <div>
                      <strong>Craft</strong>
                      <span>直接执行，修改代码</span>
                    </div>
                  </button>
                  <button
                    type="button"
                    className={`mode-option ${agentMode === "ask" ? "mode-option--active" : ""}`}
                    onClick={() => {
                      setAgentMode("ask");
                      setModeMenuOpen(false);
                    }}
                  >
                    <MessageSquare size={16} />
                    <div>
                      <strong>Ask</strong>
                      <span>只回答问题，不修改代码</span>
                    </div>
                  </button>
                  <button
                    type="button"
                    className={`mode-option ${agentMode === "plan" ? "mode-option--active" : ""}`}
                    onClick={() => {
                      setAgentMode("plan");
                      setModeMenuOpen(false);
                    }}
                  >
                    <FileDiff size={16} />
                    <div>
                      <strong>Plan</strong>
                      <span>先制定计划，等待确认</span>
                    </div>
                  </button>
                </div>
              )}
            </div>
            <ModelSelector
              provider={providerConfig}
              providers={providerConfigs}
              activeProviderId={activeProviderId}
              onSaveProvider={saveProvider}
              onActivateProvider={activateProvider}
            />
            {activeTurnId ? (
              <button className="stop-button" type="button" aria-label="停止生成" title="停止生成" onClick={() => void stopTurn()}>
                {lastTurn?.state === "streaming" ? (
                  <Loader2 className="spin" size={16} />
                ) : (
                  <Square size={15} fill="currentColor" />
                )}
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

      <WorkbenchPanel key={workspaceRevision} open={workbenchOpen} plan={plan} toolActivities={toolActivities} changes={changes} onSelectChange={setSelectedChangeId} onAttach={(attachment) => setAttachments((items) => items.some((item) => item.path === attachment.path) ? items : [...items, attachment])} />
      <AgentActivityPanel open={agentPanelOpen} parentThreadId={activeThreadId} onClose={() => setAgentPanelOpen(false)} />
      <aside className="activity-panel activity-panel--overlay" aria-hidden="true">
        <div className="activity-list activity-list--hidden">
          <div className="activity-row">
            <span className={cn("activity-dot", runtime && "activity-dot--success")} />
            <div><strong>运行时</strong><span>{runtime ? `v${runtime.version}` : "等待中"}</span></div>
          </div>
          <div className="activity-row">
            <span className={cn("activity-dot", providerConfig && "activity-dot--success")} />
            <div><strong>Provider</strong><span>{providerConfig?.model ?? "未配置"}</span></div>
          </div>
          <div className="activity-row">
            <span className={cn("activity-dot", activeTurnId && "activity-dot--active")} />
            <div><strong>当前 Turn</strong><span>{lastTurn ? stateLabel(lastTurn.state) : "空闲"}</span></div>
          </div>
          {toolActivities.slice(-8).map((activity) => (
            <div className="activity-row activity-row--tool" key={`${activity.turnId}-${activity.call.id}`}>
              <span
                className={cn(
                  "activity-dot",
                  activity.state === "running" && "activity-dot--active",
                  activity.state === "completed" && "activity-dot--success",
                  activity.state === "failed" && "activity-dot--error",
                )}
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
          providers={providerConfigs}
          activeThreadId={activeThreadId}
          goal={goal}
          activeProviderId={activeProviderId}
          error={error}
          skin={skin}
          themeMode={themeMode}
          onClose={() => setSettingsOpen(false)}
          onSetSkin={setSkin}
          onToggleTheme={toggleTheme}
          onSaveProvider={saveProvider}
          onActivateProvider={activateProvider}
          onDeleteProvider={deleteProvider}
          onCreateGoal={createActiveGoal}
          onTransitionGoal={transitionActiveGoal}
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

function GoalControl({
  goal,
  onManage,
}: {
  goal: GoalView | null;
  onManage: () => void;
}) {
  // 无 Goal 或已结束：不占用聊天区空间，全部收敛到设置里管理
  if (!goal || goal.state === "completed" || goal.state === "budget_exhausted") {
    return null;
  }
  const percent = Math.min(100, Math.round((goal.tokensUsed / goal.tokenBudget) * 100));
  const paused = goal.state === "paused";
  const blocked = goal.state === "blocked";
  return (
    <div
      className="goal-slim"
      role="button"
      tabIndex={0}
      aria-label="Goal 状态，点击管理"
      title={paused ? "Goal 已暂停，点击管理" : blocked ? "Goal 已阻塞，点击管理" : "Goal 运行中，点击管理"}
      onClick={onManage}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onManage();
        }
      }}
    >
      <span className={`goal-slim-icon ${paused ? "goal-slim-icon--paused" : blocked ? "goal-slim-icon--blocked" : ""}`}>
        <Target size={13} />
      </span>
      <span className="goal-slim-track" aria-hidden="true">
        <span style={{ width: `${percent}%` }} />
      </span>
      <span className="goal-slim-copy">
        {paused ? "已暂停" : blocked ? "已阻塞" : "运行中"} · {goal.tokensUsed.toLocaleString()} / {goal.tokenBudget.toLocaleString()} tokens
      </span>
      <span className="goal-slim-manage" aria-hidden="true">管理</span>
    </div>
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
