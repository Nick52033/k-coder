import { Fragment, FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  Activity,
  Archive,
  ArrowUp,
  CircleAlert,
  Bot,
  Code2,
  FileDiff,
  Hammer,
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
  ImageIcon,
  Scissors,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getRuntimeStatus, subscribeToAgentEvents, listSubagents, captureScreen } from "./api/runtime";
import { useWorkbenchStore } from "./stores/workbenchStore";
import { PatchReviewDialog } from "./components/PatchReviewDialog";
import { SettingsDialog, type SettingsSection } from "./components/SettingsDialog";
import { WorkbenchPanel, WorkspacePicker } from "./components/WorkbenchPanel";
import { AgentActivityPanel } from "./components/AgentActivityPanel";
import { ModelSelector } from "./components/ModelSelector";
import { ApprovalModeSelector } from "./components/ApprovalModeSelector";
import { ReasoningSelector } from "./components/ReasoningSelector";
import { TodoList } from "./components/TodoList";
import { ConversationTurnActivity } from "./components/ConversationActivity";
import { MarkdownContent } from "./components/MarkdownContent";
import { cn } from "./lib/cn";
import { open } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";
import type { AttachmentContent, GoalView, RuntimeStatus } from "./types/runtime";
import "./App.css";
import "./enhanced-animations.css"; // UI 增强动画
import "./components/ModeSelector.css";

function BrandGlyph({ size = 20 }: { size?: number }) {
  return (
    <svg viewBox="0 0 32 32" width={size} height={size} fill="none" xmlns="http://www.w3.org/2000/svg">
      <rect width="32" height="32" rx="8" fill="currentColor" />
      <path
        d="M10 10.5h4.4v6.6L19 10.5h5L16.6 17 24 21.5h-5L14.4 17.1l-2.4 2.7v1.7h-2V10.5Z"
        fill="var(--color-surface)"
      />
    </svg>
  );
}

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
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("providers");
  const [selectedChangeId, setSelectedChangeId] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<AttachmentContent[]>([]);
  const [screenshotDataUrl, setScreenshotDataUrl] = useState<string | null>(null);
  const [screenshotCapturing, setScreenshotCapturing] = useState(false);
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
    activeTurnThreadId,
    usage,
    turnTimeline,
    turnUserMessageIds,
    activityStatus,
    pendingApproval,
    pendingApprovals,
    pendingUserInput,
    changes,
    providerConfig,
    providerConfigs,
    activeProviderId,
    approvalMode,
    reasoningEffort,
    plan,
    goal,
    todos,
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
    resolvePendingUserInput,
    undoAppliedChange,
    saveProvider,
    activateProvider,
    deleteProvider,
    setApprovalMode,
    setReasoningEffort,
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
        setSettingsSection("providers");
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
  // 仅当正在生成的 turn 属于当前线程时才视为"忙"，避免旧线程的 turn 阻塞新对话
  const currentThreadBusy = Boolean(activeTurnId) && activeTurnThreadId === activeThreadId;
  const retryable = !currentThreadBusy && ["failed", "cancelled"].includes(lastTurn?.state ?? "");
  const toolActivities = turnTimeline.flatMap((item) => item.type === "tool" ? [item.activity] : []);
  const activitiesByTurn = new Map<string, typeof toolActivities>();
  for (const activity of toolActivities) {
    const turnActivities = activitiesByTurn.get(activity.turnId) ?? [];
    turnActivities.push(activity);
    activitiesByTurn.set(activity.turnId, turnActivities);
  }
  const timelineByTurn = new Map<string, typeof turnTimeline>();
  for (const item of turnTimeline) {
    const turnId = item.type === "tool" ? item.activity.turnId : item.turnId;
    const turnItems = timelineByTurn.get(turnId) ?? [];
    turnItems.push(item);
    timelineByTurn.set(turnId, turnItems);
  }
  const representedTurnIds = new Set(
    messages.flatMap((message) => message.role === "assistant" && message.turnId ? [message.turnId] : []),
  );
  const latestPlanActivity = [...toolActivities].reverse().find((activity) => activity.call.name === "update_plan");
  const latestAssistantTurnId = [...messages].reverse().find(
    (message) => message.role === "assistant" && message.turnId,
  )?.turnId;
  const planTurnId = plan?.steps.length
    ? latestPlanActivity?.turnId ?? (currentThreadBusy ? activeTurnId : latestAssistantTurnId)
    : null;
  const orphanTurnIds = [...new Set([...activitiesByTurn.keys(), ...timelineByTurn.keys()])]
    .filter((turnId) => !representedTurnIds.has(turnId));
  const orphanTurnsByUserMessage = new Map<string, string[]>();
  const unanchoredOrphanTurnIds: string[] = [];
  for (const turnId of orphanTurnIds) {
    const messageId = turnUserMessageIds[turnId];
    if (!messageId) {
      unanchoredOrphanTurnIds.push(turnId);
      continue;
    }
    const anchored = orphanTurnsByUserMessage.get(messageId) ?? [];
    anchored.push(turnId);
    orphanTurnsByUserMessage.set(messageId, anchored);
  }
  const planIsAttached = Boolean(planTurnId && representedTurnIds.has(planTurnId));
  const planIsAttachedToOrphan = Boolean(
    planTurnId && orphanTurnIds.includes(planTurnId),
  );
  const hasConversationContent = messages.length > 0
    || orphanTurnIds.length > 0
    || Boolean(plan?.steps.length)
    || Boolean(pendingApproval)
    || Boolean(pendingUserInput);

  function renderOrphanTurn(turnId: string) {
    return (
      <article className="message message--assistant message--activity-only" key={`activity-${turnId}`}>
        <div className="message-avatar" aria-hidden="true">
          <span className="message-avatar-mark">
            <BrandGlyph size={18} />
          </span>
        </div>
        <div className="message-body">
          <div className="message-role">k-Coder</div>
          <ConversationTurnActivity
            activities={activitiesByTurn.get(turnId) ?? []}
            timeline={timelineByTurn.get(turnId) ?? []}
            changes={changes}
            plan={turnId === planTurnId ? plan : null}
            streaming={turnId === activeTurnId}
            activityStatus={turnId === activityStatus?.turnId ? activityStatus.status : null}
            renderText={renderMessageText}
          />
        </div>
      </article>
    );
  }

  function submitMessage(event: FormEvent) {
    event.preventDefault();
    const message = draft.trim();
    if (!message) return;
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

  async function startScreenshot() {
    setScreenshotCapturing(true);
    try {
      const result = await captureScreen();
      setScreenshotDataUrl(result.dataUrl);
    } catch (error) {
      setRuntimeError(typeof error === "string" ? error : "截图失败");
    } finally {
      setScreenshotCapturing(false);
    }
  }

  function handleScreenshotCrop(croppedDataUrl: string) {
    const path = `screenshot://${Date.now()}.png`;
    const size = Math.ceil(croppedDataUrl.length * 0.75); // base64 解码后的近似字节数
    setAttachments((items) => {
      if (items.some((a) => a.path === path)) return items;
      return [
        ...items,
        { kind: "image", name: `screenshot-${Date.now()}.png`, path, content: croppedDataUrl, size, truncated: false },
      ];
    });
    setScreenshotDataUrl(null);
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

  // 生成友好的工具描述
  function getFriendlyToolDescription(toolName: string, args: Record<string, unknown>): string {
    switch (toolName) {
      case "run_command":
        const cmdArgs = args.args as string[] | undefined;
        if (cmdArgs && cmdArgs.length > 0) {
          // 提取实际的命令
          const fullCmd = cmdArgs.join(" ");
          // 如果是 findstr/grep，显示搜索内容
          if (fullCmd.includes("findstr") || fullCmd.includes("grep")) {
            const searchPattern = cmdArgs.find(arg => arg.includes("extract_attachment") || arg.includes("import") || !arg.startsWith("/") && !arg.startsWith("-"));
            return searchPattern ? `搜索代码: ${searchPattern}` : "搜索代码文件";
          }
          // 如果是 cd，显示目录切换
          if (fullCmd.includes("cd ")) {
            return "切换工作目录";
          }
          // 其他命令，显示简化版本
          const mainCmd = cmdArgs.find(arg => !arg.startsWith("/") && !arg.startsWith("-") && arg !== "&&");
          return mainCmd ? `执行命令: ${mainCmd}` : "执行命令";
        }
        return "执行命令";

      case "read_file":
        return `读取文件: ${args.path ?? ""}`;

      case "write_file":
        return `写入文件: ${args.path ?? ""}`;

      case "edit_file":
        return `编辑文件: ${args.path ?? ""}`;

      case "list_directory":
        return `列出目录: ${args.path ?? "当前目录"}`;

      default:
        return toolName;
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

  // Handle local file drag over the composer area
  function handleDragOver(event: React.DragEvent) {
    const items = event.dataTransfer.items;
    if (items.length > 0) {
      const hasImage = Array.from(items).some(
        (item) => item.kind === "file" && item.type.startsWith("image/"),
      );
      if (hasImage) {
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }
    }
  }

  function handleDrop(event: React.DragEvent) {
    const files = Array.from(event.dataTransfer.files).filter((file) =>
      file.type.startsWith("image/"),
    );
    if (files.length === 0) return;
    event.preventDefault();
    for (const file of files) {
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = reader.result as string;
        setAttachments((items) => {
          const path = `drop://${file.name}`;
          if (items.some((a) => a.path === path)) return items;
          return [
            ...items,
            {
              path,
              name: file.name,
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

  // Pick local images via Tauri file dialog
  async function handlePickImages() {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          {
            name: "Images",
            extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"],
          },
        ],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      for (const filePath of paths) {
        try {
          const bytes = await readFile(filePath);
          const ext = filePath.split(".").pop()?.toLowerCase() ?? "png";
          const mimeMap: Record<string, string> = {
            png: "image/png",
            jpg: "image/jpeg",
            jpeg: "image/jpeg",
            gif: "image/gif",
            webp: "image/webp",
            bmp: "image/bmp",
            ico: "image/x-icon",
            svg: "image/svg+xml",
          };
          const mime = mimeMap[ext] ?? "image/png";
          const base64 = btoa(
            Array.from(new Uint8Array(bytes))
              .map((b) => String.fromCharCode(b))
              .join(""),
          );
          const dataUrl = `data:${mime};base64,${base64}`;
          const name = filePath.split(/[/\\]/).pop() ?? `image.${ext}`;
          const pathKey = `file://${filePath}`;
          setAttachments((items) => {
            if (items.some((a) => a.path === pathKey)) return items;
            return [
              ...items,
              {
                path: pathKey,
                name,
                kind: "image",
                content: dataUrl,
                size: bytes.length,
                truncated: false,
              },
            ];
          });
        } catch {
          // silently skip unreadable files
        }
      }
    } catch {
      // user cancelled
    }
  }

  function openSettings() {
    clearError();
    setWorkbenchOpen(false);
    setAgentPanelOpen(false);
    setSettingsSection("providers");
    setSettingsOpen(true);
  }

  function openGoalSettings() {
    setSettingsSection("goal");
    setSettingsOpen(true);
  }

  return (
    <main className={cn("workbench", workbenchOpen && "workbench--panel-open")}>
      <header className="titlebar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <span className="brand-mark" aria-hidden="true">
            <BrandGlyph size={20} />
          </span>
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
              {currentThreadBusy ? "正在生成" : usage ? `${usage.totalTokens} tokens` : "纯文本对话"}
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
              disabled={!activeThread || currentThreadBusy}
              onClick={() => void archiveActiveThread()}
            >
              <Archive size={17} />
            </button>
          </div>
        </div>

        <div className={cn("message-area", hasConversationContent && "message-area--populated")} ref={messageAreaRef}>
          {loading && !messages.length ? (
            <div className="empty-thread"><Activity className="spin" size={24} /><p>正在读取会话</p></div>
          ) : hasConversationContent ? (
            <div className="message-list">
              {/* 任务清单 */}
              {activeThreadId && todos.get(activeThreadId) && (
                <TodoList todos={todos.get(activeThreadId)!} />
              )}

              {messages.map((message) => {
                // 查找该消息对应回合的文件变更
                const messageChanges = message.role === "assistant" && message.turnId
                  ? changes.filter(change => change.turnId === message.turnId && !change.undone)
                  : [];
                const messageActivities = message.role === "assistant" && message.turnId
                  ? activitiesByTurn.get(message.turnId) ?? []
                  : [];
                const messageTimeline = message.role === "assistant" && message.turnId
                  ? timelineByTurn.get(message.turnId) ?? []
                  : [];
                const messagePlan = message.role === "assistant" && message.turnId === planTurnId
                  ? plan
                  : null;
                const messageActivityStatus = message.role === "assistant"
                  && activityStatus
                  && message.turnId === activityStatus.turnId
                  ? activityStatus.status
                  : null;

                return (
                  <Fragment key={message.id}>
                  <article className={cn("message", `message--${message.role}`, message.status === "streaming" && "message--streaming")}>
                    {message.role === "assistant" && (
                      <div className="message-avatar" aria-hidden="true">
                        <span className="message-avatar-mark">
                          <BrandGlyph size={18} />
                        </span>
                        <span className="message-avatar-pulse" />
                      </div>
                    )}
                    <div className="message-body">
                      <div className="message-role">{message.role === "user" ? "你" : "k-Coder"}</div>
                      {message.role === "assistant" && (
                        <ConversationTurnActivity
                          activities={messageActivities}
                          timeline={messageTimeline}
                          changes={changes}
                          plan={messagePlan}
                          streaming={message.status === "streaming"}
                          activityStatus={messageActivityStatus}
                          renderText={renderMessageText}
                        />
                      )}
                      {!messageTimeline.length ? <div className="message-content">
                        {message.text
                          ? renderMessageText(message.text)
                          : message.status === "streaming" && !messageActivityStatus ? (
                          <span className="typing-indicator" aria-label="AI 正在响应">
                            <span className="typing-dot" />
                            <span className="typing-dot" />
                            <span className="typing-dot" />
                          </span>
                        ) : null}
                      </div> : null}

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
                            {messageChanges.flatMap((change) => change.files.map((file) => (
                              <button
                                type="button"
                                key={`${change.id}-${file.path}`}
                                className="change-file-item"
                                onClick={() => setSelectedChangeId(change.id)}
                              >
                                <span className="change-file-name">{file.path}</span>
                                <span className="change-operation">{file.operation}</span>
                              </button>
                            )))}
                          </div>
                        )}
                        </div>
                      )}

                      {message.status === "failed" && <div className="message-status message-status--error">生成失败</div>}
                      {message.status === "cancelled" && <div className="message-status">已停止</div>}
                    </div>
                  </article>
                  {message.role === "user"
                    ? (orphanTurnsByUserMessage.get(message.id) ?? []).map(renderOrphanTurn)
                    : null}
                  </Fragment>
                );
              })}

              {unanchoredOrphanTurnIds.map(renderOrphanTurn)}

              {plan?.steps.length && !planIsAttached && !planIsAttachedToOrphan ? (
                <article className="message message--assistant message--activity-only">
                  <div className="message-avatar" aria-hidden="true">
                    <span className="message-avatar-mark">
                      <BrandGlyph size={18} />
                    </span>
                  </div>
                  <div className="message-body">
                    <div className="message-role">k-Coder</div>
                    <ConversationTurnActivity activities={[]} plan={plan} />
                  </div>
                </article>
              ) : null}

              {/* 内嵌授权请求 */}
              {pendingApproval && (
                <article className="message message--approval">
                  <div className="message-avatar" aria-hidden="true">
                    <span className="message-avatar-mark">
                      <BrandGlyph size={18} />
                    </span>
                  </div>
                  <div className="message-body">
                    <div className="message-role">k-Coder</div>
                    <div className="approval-inline">
                      {pendingApprovals.length > 1 ? (
                        <div className="approval-queue-position">
                          待确认 1 / {pendingApprovals.length}
                        </div>
                      ) : null}
                      <div className="approval-prompt">
                        {getFriendlyToolDescription(pendingApproval.toolName, pendingApproval.arguments)}
                      </div>

                      {/* 显示命令/操作详情 */}
                      {(pendingApproval.preview?.patch || Object.keys(pendingApproval.arguments).length > 0) && (
                        <details className="approval-command">
                          <summary>查看详情</summary>
                          <pre>{pendingApproval.preview?.patch ?? JSON.stringify(pendingApproval.arguments, null, 2)}</pre>
                        </details>
                      )}

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

              {/* Plan 模式：模型向用户提问 */}
              {pendingUserInput && (
                <UserInputCard
                  request={pendingUserInput}
                  onResolve={(resolution) => void resolvePendingUserInput(resolution)}
                />
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
              {currentThreadBusy && (
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

        <GoalControl goal={goal} onManage={openGoalSettings} />
        <form
          className="composer"
          onSubmit={submitMessage}
          onDragOver={handleDragOver}
          onDrop={handleDrop}
        >
          {attachments.length > 0 && <div className="attachment-strip">{attachments.map((attachment) => <span key={attachment.path} className={attachment.kind === "image" ? "attachment-tag attachment-tag--image" : "attachment-tag"}>{attachment.kind === "image" ? <img src={attachment.content} alt={attachment.name} className="attachment-thumb" /> : <Paperclip size={12} />}{attachment.name}<button type="button" aria-label={`移除 ${attachment.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== attachment.path))}><X size={12} /></button></span>)}</div>}
          <textarea
            aria-label="消息"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleComposerKeyDown}
            onPaste={handlePaste}
            placeholder="输入消息，可直接粘贴或拖拽图片"
            rows={3}
          />
          <div className="composer-footer">
            <button
              type="button"
              className="composer-pick-image"
              aria-label="从本地选取图片"
              title="从本地选取图片"
              onClick={() => void handlePickImages()}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                width: 32,
                height: 32,
                padding: 0,
                border: "1px solid transparent",
                borderRadius: "var(--radius-sm)",
                background: "transparent",
                cursor: "pointer",
              }}
            ><ImageIcon size={18} /></button>
            <button
              type="button"
              className="composer-pick-image"
              aria-label="屏幕截图"
              title="屏幕截图（框选屏幕任意区域）"
              disabled={screenshotCapturing}
              onClick={() => void startScreenshot()}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                width: 32,
                height: 32,
                padding: 0,
                border: "1px solid transparent",
                borderRadius: "var(--radius-sm)",
                background: "transparent",
                cursor: screenshotCapturing ? "default" : "pointer",
              }}
            >
              {screenshotCapturing ? <Loader2 size={18} className="spin" /> : <Scissors size={18} />}
            </button>
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
                    <span className="mode-glyph mode-glyph--craft" aria-hidden="true">
                      <Hammer size={13} strokeWidth={2.2} />
                    </span>
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
                    <span className="mode-glyph mode-glyph--craft" aria-hidden="true">
                      <Hammer size={14} strokeWidth={2.2} />
                    </span>
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
            <ApprovalModeSelector
              mode={approvalMode}
              disabled={currentThreadBusy}
              onChange={setApprovalMode}
            />
            <ReasoningSelector
              effort={reasoningEffort}
              onChange={setReasoningEffort}
            />
            <ModelSelector
              provider={providerConfig}
              providers={providerConfigs}
              activeProviderId={activeProviderId}
              onSaveProvider={saveProvider}
              onActivateProvider={activateProvider}
            />
            <div className="composer-actions">
              {currentThreadBusy && (
                <button className="stop-button" type="button" aria-label="停止生成" title="停止生成" onClick={() => void stopTurn()}>
                  {lastTurn?.state === "streaming" ? (
                    <Loader2 className="spin" size={16} />
                  ) : (
                    <Square size={15} fill="currentColor" />
                  )}
                </button>
              )}
              <button
                className="send-button"
                type="submit"
                aria-label="发送消息"
                title="发送消息"
                disabled={!draft.trim()}
              >
                <ArrowUp size={18} strokeWidth={2.2} />
              </button>
            </div>
          </div>
        </form>
      </section>

      {screenshotDataUrl && (
        <ScreenshotOverlay
          imageDataUrl={screenshotDataUrl}
          onCancel={() => setScreenshotDataUrl(null)}
          onConfirm={handleScreenshotCrop}
        />
      )}

      <WorkbenchPanel key={workspaceRevision} open={workbenchOpen} onAttach={(attachment) => setAttachments((items) => items.some((item) => item.path === attachment.path) ? items : [...items, attachment])} />
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
            <span className={cn("activity-dot", currentThreadBusy && "activity-dot--active")} />
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
                      disabled={currentThreadBusy}
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
          initialSection={settingsSection}
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

/**
 * Plan 模式下模型向用户提问的卡片。
 * 每个问题有 2-4 个多选选项，用户选择后通过 resolvePendingUserInput 回传给模型。
 */
function UserInputCard({ request, onResolve }: {
  request: import("./types/runtime").UserInputRequest;
  onResolve: (resolution: import("./types/runtime").UserInputResolution) => void;
}) {
  const [answers, setAnswers] = useState<Record<number, string>>({});

  function submit() {
    const resolved = request.questions.map((q, idx) => ({
      question: q.question,
      answer: answers[idx] ?? "",
    }));
    onResolve({ action: "answered", answers: resolved });
  }

  function skip() {
    onResolve({ action: "skipped", answers: [] });
  }

  const allAnswered = request.questions.every((_, idx) => answers[idx]);

  return (
    <article className="message message--approval">
      <div className="message-avatar" aria-hidden="true">
        <span className="message-avatar-mark">
          <BrandGlyph size={18} />
        </span>
      </div>
      <div className="message-body">
        <div className="message-role">k-Coder 提问</div>
        <div className="approval-inline">
          {request.questions.map((q, qIdx) => (
            <div key={qIdx} className="user-input-question">
              <div className="user-input-question-text">{q.question}</div>
              <div className="user-input-options">
                {q.options.map((opt) => (
                  <button
                    key={opt}
                    type="button"
                    className={`user-input-option ${answers[qIdx] === opt ? "user-input-option--selected" : ""}`}
                    onClick={() => setAnswers((prev) => ({ ...prev, [qIdx]: opt }))}
                  >
                    {opt}
                  </button>
                ))}
              </div>
            </div>
          ))}
          <div className="approval-options">
            <button
              type="button"
              className="approval-option"
              disabled={!allAnswered}
              onClick={submit}
            >
              提交回答
            </button>
            <button
              type="button"
              className="approval-option approval-option--danger"
              onClick={skip}
            >
              跳过
            </button>
          </div>
        </div>
      </div>
    </article>
  );
}

function renderMessageText(text: string): React.ReactNode {
  return <MarkdownContent text={text} />;
}

/**
 * 屏幕截图框选遮罩：显示整屏截图，用户拖拽选择一个矩形区域，
 * 确认后把裁剪出的部分作为图片 dataUrl 交回给调用方。
 */
function ScreenshotOverlay({ imageDataUrl, onCancel, onConfirm }: {
  imageDataUrl: string;
  onCancel: () => void;
  onConfirm: (croppedDataUrl: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const [imgSize, setImgSize] = useState<{ width: number; height: number } | null>(null);
  const [selection, setSelection] = useState<{
    x: number;
    y: number;
    width: number;
    height: number;
  } | null>(null);
  const dragStart = useRef<{ x: number; y: number } | null>(null);

  function handleMouseDown(e: React.MouseEvent) {
    const img = imgRef.current;
    if (!img) return;
    const rect = img.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    dragStart.current = { x, y };
    setSelection({ x, y, width: 0, height: 0 });
  }

  function handleMouseMove(e: React.MouseEvent) {
    if (!dragStart.current) return;
    const img = imgRef.current;
    if (!img) return;
    const rect = img.getBoundingClientRect();
    const curX = e.clientX - rect.left;
    const curY = e.clientY - rect.top;
    const x = Math.min(dragStart.current.x, curX);
    const y = Math.min(dragStart.current.y, curY);
    setSelection({
      x,
      y,
      width: Math.abs(curX - dragStart.current.x),
      height: Math.abs(curY - dragStart.current.y),
    });
  }

  function handleMouseUp() {
    dragStart.current = null;
  }

  function confirmCrop() {
    if (!selection || selection.width < 2 || selection.height < 2) return;
    const img = imgRef.current;
    if (!img) return;
    // 考虑图片实际尺寸与显示尺寸的比例
    const scaleX = img.naturalWidth / img.getBoundingClientRect().width;
    const scaleY = img.naturalHeight / img.getBoundingClientRect().height;
    const sx = Math.max(0, Math.floor(selection.x * scaleX));
    const sy = Math.max(0, Math.floor(selection.y * scaleY));
    const sw = Math.min(img.naturalWidth - sx, Math.floor(selection.width * scaleX));
    const sh = Math.min(img.naturalHeight - sy, Math.floor(selection.height * scaleY));
    if (sw < 1 || sh < 1) return;

    const canvas = document.createElement("canvas");
    canvas.width = sw;
    canvas.height = sh;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.drawImage(img, sx, sy, sw, sh, 0, 0, sw, sh);
    onConfirm(canvas.toDataURL("image/png"));
  }

  return (
    <div className="screenshot-overlay" onMouseUp={handleMouseUp}>
      <div
        ref={containerRef}
        className="screenshot-stage"
        style={{
          width: imgSize ? `${imgSize.width}px` : "auto",
          height: imgSize ? `${imgSize.height}px` : "auto",
        }}
      >
        <img
          ref={imgRef}
          src={imageDataUrl}
          alt="屏幕截图预览"
          className="screenshot-stage-img"
          style={{ cursor: selection ? "crosshair" : "crosshair" }}
          onMouseDown={handleMouseDown}
          onMouseMove={handleMouseMove}
          onLoad={() => {
            const el = imgRef.current;
            if (el) setImgSize({ width: el.width, height: el.height });
          }}
        />
        {selection && selection.width > 0 && selection.height > 0 && (
          <div
            className="screenshot-selection"
            style={{
              left: selection.x,
              top: selection.y,
              width: selection.width,
              height: selection.height,
            }}
          />
        )}
      </div>
      <div className="screenshot-toolbar">
        <span className="screenshot-hint">拖拽框选截图区域</span>
        <button type="button" className="screenshot-btn screenshot-btn--danger" onClick={onCancel}>
          取消
        </button>
        <button
          type="button"
          className="screenshot-btn screenshot-btn--primary"
          disabled={!selection || selection.width < 2 || selection.height < 2}
          onClick={confirmCrop}
        >
          确认
        </button>
      </div>
    </div>
  );
}

export default App;
