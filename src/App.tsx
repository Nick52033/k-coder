import { Fragment, FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowUp,
  ChevronRight,
  CircleAlert,
  Bot,
  Code2,
  FileDiff,
  Folder,
  FolderPlus,
  Hammer,
  PanelRightOpen,
  PanelRightClose,
  Loader2,
  Maximize2,
  MessageSquare,
  Minus,
  MoreHorizontal,
  Moon,
  Paperclip,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  ScrollText,
  Search,
  Settings,
  Square,
  Sun,
  Trash2,
  Undo2,
  X,
  Target,
  ImageIcon,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getRuntimeStatus, switchWorkspace, subscribeToAgentEvents, listSubagents, recognizeImage } from "./api/runtime";
import { useWorkbenchStore } from "./stores/workbenchStore";
import { PatchReviewDialog } from "./components/PatchReviewDialog";
import { SettingsDialog, type SettingsSection } from "./components/SettingsDialog";
import { LogViewerDialog } from "./components/LogViewerDialog";
import { WorkbenchPanel } from "./components/WorkbenchPanel";
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
  const [logViewerOpen, setLogViewerOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("providers");
  const [selectedChangeId, setSelectedChangeId] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<AttachmentContent[]>([]);
  const ocrTasksRef = useRef(new Map<string, Promise<string>>());
  const ocrResultsRef = useRef(new Map<string, string>());
  const [threadQuery, setThreadQuery] = useState("");
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const [agentPanelOpen, setAgentPanelOpen] = useState(false);
  const [workspaceRevision, setWorkspaceRevision] = useState(0);
  const [skin, setSkinState] = useState<Skin>(() => readStored(STORAGE_SKIN, "paper"));
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() =>
    readStored(STORAGE_THEME, "light"),
  );
  const [subagentThreadIds, setSubagentThreadIds] = useState<Set<string>>(new Set());
  const [sideView, setSideView] = useState<"conversations" | "projects">("conversations");
  const [workspacePath, setWorkspacePath] = useState("");
  const [threadProjectMap, setThreadProjectMap] = useState<Record<string, string>>({});
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [projectMenuOpen, setProjectMenuOpen] = useState<string | null>(null);
  const [pinnedProjects, setPinnedProjects] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem("kcoder_pinned_projects") ?? "[]") as string[]);
    } catch { return new Set<string>(); }
  });
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
    sendQueuedMessageNow,
    removeQueuedMessage,
    clearQueue,
    forceResetState,
  } = useWorkbenchStore();
  const pendingQueueCount = messageQueue.filter((message) => message.status === "pending").length;

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

  // ===== 线程-项目关联管理 =====
  const THREAD_PROJECT_KEY = "kcoder_thread_project_map";

  const readThreadProjectMap = (): Record<string, string> => {
    try {
      const raw = localStorage.getItem(THREAD_PROJECT_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch { return {}; }
  };

  const saveThreadProjectMap = (map: Record<string, string>) => {
    try { localStorage.setItem(THREAD_PROJECT_KEY, JSON.stringify(map)); } catch { /* noop */ }
  };

  useEffect(() => {
    setThreadProjectMap(readThreadProjectMap());
  }, []);

  // 在指定项目（路径）下创建一个新会话。
  // "会话"tab 调用时不传项目路径，会话保持"未分类"；"项目"tab 调用时传入项目路径，
  // store 创建线程后立即把 threadId 关联到该项目，避免被自动绑定到当前工作区。
  const createSessionUnderProject = async (projectPath: string | null) => {
    let newThreadId: string;
    try {
      newThreadId = await createThread();
    } catch {
      return;
    }
    if (!projectPath) return;
    const map = readThreadProjectMap();
    if (map[newThreadId] === projectPath) return;
    const next = { ...map, [newThreadId]: projectPath };
    setThreadProjectMap(next);
    saveThreadProjectMap(next);
  };

  // 弹出"选择项目"对话框（允许多选），把所选目录注册到 known 列表并切换第一个为活动工作区。
  // 供 titlebar 的"切换工作台"和侧边栏的"添加项目"按钮共用。
  const pickAndSwitchWorkspace = async () => {
    try {
      const selected = await open({ directory: true, multiple: true, title: "选择项目工作区（可多选）" });
      if (!selected) return;
      const paths = (Array.isArray(selected) ? selected : [selected])
        .map((p) => typeof p === "string" ? p : null)
        .filter((p): p is string => Boolean(p));
      if (!paths.length) return;
      const summary = paths.length === 1
        ? `信任并打开此工作区？\n\n${paths[0]}\n\n信任后，智能体可以读取文件并在审批后修改内容。`
        : `信任并打开 ${paths.length} 个工作区？\n\n${paths.join("\n")}\n\n信任后，智能体可以读取文件并在审批后修改内容。第一个项目会作为活动工作区，其余仅注册到项目列表（其下尚无会话，会在创建首个会话后显示）。`;
      const trusted = window.confirm(summary);
      if (!trusted) return;
      await switchWorkspace(paths[0], true);
      setWorkspacePath(paths[0]);
      const knownRaw = localStorage.getItem("kcoder_known_projects");
      const known = knownRaw ? (JSON.parse(knownRaw) as string[]) : [];
      const merged = Array.from(new Set([...known, ...paths]));
      localStorage.setItem("kcoder_known_projects", JSON.stringify(merged));
      setThreadProjectMap(readThreadProjectMap());
      setWorkspaceRevision((value) => value + 1);
      void initialize();
    } catch (err) {
      console.error("切换工作区失败:", err);
    }
  };

  // 仅在初始启动时（没有任何线程和映射时）把第一个会话归到当前工作区，
  // 后续用户在"会话"tab 下新建的会话都不自动归到工作区。
  const bootstrappedRef = useRef(false);
  useEffect(() => {
    if (bootstrappedRef.current) return;
    if (!threads.length || !workspacePath) return;
    const map = readThreadProjectMap();
    const hasAnyBinding = threads.some((t) => Boolean(map[t.id]));
    if (hasAnyBinding) {
      bootstrappedRef.current = true;
      return;
    }
    const first = threads[0];
    const next = { ...map, [first.id]: workspacePath };
    setThreadProjectMap(next);
    saveThreadProjectMap(next);
    bootstrappedRef.current = true;
  }, [threads, workspacePath]);

  useEffect(() => {
    function handleShortcut(event: globalThis.KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void createSessionUnderProject(null);
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

  // 点击外部关闭项目菜单
  useEffect(() => {
    if (!projectMenuOpen) return;
    const handler = (event: MouseEvent) => {
      if (!(event.target instanceof Element)) return;
      if (!event.target.closest(".project-group-menu")) setProjectMenuOpen(null);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [projectMenuOpen]);

  const togglePinProject = (projectPath: string) => {
    setPinnedProjects((prev) => {
      const next = new Set(prev);
      if (next.has(projectPath)) next.delete(projectPath);
      else next.add(projectPath);
      localStorage.setItem("kcoder_pinned_projects", JSON.stringify([...next]));
      return next;
    });
  };

  const getProjectsWithThreads = () => {
    const projects = new Map<string, { name: string; threads: typeof threads; updatedAtMs: number }>();
    // 只把显式归到某个项目的会话算作"项目内"会话；没有归类的会话不出现在"项目"tab。
    for (const thread of threads) {
      const projectPath = threadProjectMap[thread.id];
      if (!projectPath) continue;
      if (!projects.has(projectPath)) {
        const name = projectPath.split(/[/\\]/).filter(Boolean).pop() || projectPath;
        projects.set(projectPath, { name, threads: [], updatedAtMs: 0 });
      }
      const entry = projects.get(projectPath)!;
      entry.threads.push(thread);
      entry.updatedAtMs = Math.max(entry.updatedAtMs, thread.updatedAtMs);
    }
    // 合并"已添加但暂无会话"的项目（多选添加但还未创建会话的项目）。
    // 读一次 localStorage 避免重复 IO。
    try {
      const knownRaw = localStorage.getItem("kcoder_known_projects");
      const known = knownRaw ? (JSON.parse(knownRaw) as string[]) : [];
      for (const projectPath of known) {
        if (projects.has(projectPath)) continue;
        const name = projectPath.split(/[/\\]/).filter(Boolean).pop() || projectPath;
        projects.set(projectPath, { name, threads: [], updatedAtMs: 0 });
      }
    } catch { /* noop */ }
    return Array.from(projects.entries())
      .map(([path, data]) => ({ path, name: data.name, threads: data.threads, updatedAtMs: data.updatedAtMs }))
      .sort((a, b) => {
        const aPinned = pinnedProjects.has(a.path) ? 1 : 0;
        const bPinned = pinnedProjects.has(b.path) ? 1 : 0;
        if (aPinned !== bPinned) return bPinned - aPinned;
        return b.updatedAtMs - a.updatedAtMs;
      });
  };

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

  function startAttachmentOcr(path: string, dataUrl: string) {
    const task = recognizeImage(dataUrl)
      .then((result) => {
        ocrResultsRef.current.set(path, result.text);
        setAttachments((items) => items.map((item) => item.path === path
          ? {
              ...item,
              ocrStatus: "complete",
              ocrText: result.text,
              ocrLineCount: result.lineCount,
              ocrDurationMs: result.durationMs,
              ocrError: undefined,
            }
          : item));
        return result.text;
      })
      .catch((error: unknown) => {
        const message = typeof error === "string" ? error : error instanceof Error ? error.message : "图片文字识别失败";
        ocrResultsRef.current.set(path, "");
        setAttachments((items) => items.map((item) => item.path === path
          ? { ...item, ocrStatus: "failed", ocrError: message }
          : item));
        return "";
      });
    ocrTasksRef.current.set(path, task);
    void task.finally(() => {
      if (ocrTasksRef.current.get(path) === task) ocrTasksRef.current.delete(path);
    });
  }

  function addImageAttachment(attachment: AttachmentContent) {
    setAttachments((items) => {
      if (items.some((item) => item.path === attachment.path)) return items;
      return [...items, { ...attachment, ocrStatus: "processing" }];
    });
    startAttachmentOcr(attachment.path, attachment.content);
  }

  async function submitMessage(event: FormEvent) {
    event.preventDefault();
    const message = draft.trim();
    if (!message && attachments.length === 0) return;
    const attachmentContext = attachments.filter((attachment) => attachment.kind === "document").map((attachment) =>
      `\n\n[附件: ${attachment.name}]\n${attachment.content}`,
    ).join("");
    const imageAttachments = await Promise.all(attachments
      .filter((attachment) => attachment.kind === "image")
      .map(async (attachment) => {
        // Prefer the local result cache, then wait for an in-flight OCR task.
        // React state can lag behind a completed promise when the user sends immediately.
        const text = attachment.ocrText
          || ocrResultsRef.current.get(attachment.path)
          || await (ocrTasksRef.current.get(attachment.path) ?? Promise.resolve(""));
        return {
          name: attachment.name,
          dataUrl: attachment.content,
          ocrText: text?.trim() || undefined,
        };
      }));
    attachments.forEach((attachment) => ocrTasksRef.current.delete(attachment.path));
    attachments.forEach((attachment) => ocrResultsRef.current.delete(attachment.path));
    setDraft("");
    setAttachments([]);
    if (currentThreadBusy) setQueueExpanded(true);
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
        addImageAttachment({
          path: `clipboard://${name}`,
          name,
          kind: "image",
          content: dataUrl,
          size: file.size,
          truncated: false,
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
        addImageAttachment({
          path: `drop://${file.name}`,
          name: file.name,
          kind: "image",
          content: dataUrl,
          size: file.size,
          truncated: false,
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
          addImageAttachment({
            path: pathKey,
            name,
            kind: "image",
            content: dataUrl,
            size: bytes.length,
            truncated: false,
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
    setSideView("conversations");
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
          <div className="titlebar-segmented" role="group" aria-label="面板切换">
            <button
              className={cn("segmented-button", workbenchOpen && "segmented-button--active")}
              type="button"
              aria-label="工作台"
              title="工作台"
              aria-pressed={workbenchOpen}
              onClick={() => { setWorkbenchOpen((value) => !value); setAgentPanelOpen(false); }}
            >
              {workbenchOpen ? <PanelRightOpen size={16} /> : <PanelRightClose size={16} />}
            </button>
            <button
              className={cn("segmented-button", agentPanelOpen && "segmented-button--active")}
              type="button"
              aria-label="子智能体"
              title="子智能体"
              aria-pressed={agentPanelOpen}
              onClick={() => { setAgentPanelOpen((value) => !value); setWorkbenchOpen(false); }}
            >
              <Bot size={16} />
            </button>
          </div>
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
              aria-label="最小化到托盘"
              title="最小化到托盘"
              onClick={() => void appWindow.hide()}
            >
              <X size={16} />
            </button>
          </div>
        </div>
      </header>

      <aside className="sidebar">
        <section className="thread-section" aria-labelledby="thread-section-title">
          <div className="thread-section-heading">
            <div className="sidebar-segmented" role="tablist" aria-label="侧边栏视图">
              <button
                type="button"
                role="tab"
                aria-selected={sideView === "conversations"}
                className={cn(sideView === "conversations" && "is-active")}
                onClick={() => setSideView("conversations")}
              >
                <MessageSquare size={15} />
                <span>会话</span>
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={sideView === "projects"}
                className={cn(sideView === "projects" && "is-active")}
                onClick={() => setSideView("projects")}
              >
                <Folder size={15} />
                <span>项目</span>
              </button>
            </div>
            <div className="sidebar-heading-actions">
              <button
                type="button"
                className="sidebar-icon-button"
                title="添加会话（与项目无关）"
                aria-label="添加会话"
                onClick={() => void createSessionUnderProject(null)}
              >
                <Plus size={15} />
              </button>
              <button
                type="button"
                className="sidebar-icon-button"
                title="添加项目（可多选）"
                aria-label="添加项目"
                onClick={() => void pickAndSwitchWorkspace()}
              >
                <FolderPlus size={15} />
              </button>
            </div>
          </div>
          <label className="thread-search">
            <Search size={14} aria-hidden="true" />
            <input
              aria-label="搜索会话"
              placeholder={sideView === "conversations" ? "搜索会话" : "在会话中搜索"}
              value={threadQuery}
              onChange={(event) => { const query = event.target.value; setThreadQuery(query); void searchThreadHistory(query); }}
            />
            {threadQuery && (
              <button type="button" title="清除搜索" aria-label="清除搜索" onClick={() => { setThreadQuery(""); void searchThreadHistory(""); }}>
                <X size={13} />
              </button>
            )}
          </label>
          {sideView === "conversations" ? (
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
          ) : (
            <nav className="thread-list" aria-label="项目列表">
              {(() => {
                const projects = getProjectsWithThreads();
                if (!projects.length) {
                  return (
                    <div className="thread-empty">
                      <Folder size={16} />
                      <span>还没有打开过项目</span>
                    </div>
                  );
                }
                return projects.map((project) => {
                  const isExpanded = expandedProjects.has(project.path);
                  return (
                    <div className="project-group" key={project.path}>
                      <div className={cn("project-group-header", isExpanded && "project-group-header--expanded")}>
                        <button
                          type="button"
                          className="project-group-toggle"
                          aria-label={isExpanded ? "折叠项目" : "展开项目"}
                          aria-expanded={isExpanded}
                          onClick={() => {
                            setExpandedProjects((prev) => {
                              const next = new Set(prev);
                              if (next.has(project.path)) next.delete(project.path);
                              else next.add(project.path);
                              return next;
                            });
                          }}
                        >
                          <ChevronRight size={13} className={cn("project-group-chevron", isExpanded && "is-expanded")} />
                          <Folder size={15} />
                          <span className="project-group-name">{project.name}</span>
                        </button>
                        <span className="project-group-count">{project.threads.length}</span>
                        <span className="project-group-actions">
                          <button
                            type="button"
                            className="project-group-action"
                            title="在项目中新建会话"
                            aria-label="在项目中新建会话"
                            onClick={async (event) => {
                              event.stopPropagation();
                              await createSessionUnderProject(project.path);
                            }}
                          >
                            <Plus size={13} />
                          </button>
                          <div className="project-group-menu-anchor">
                            <button
                              type="button"
                              className="project-group-action"
                              title="更多操作"
                              aria-label="更多操作"
                              onClick={(event) => {
                                event.stopPropagation();
                                setProjectMenuOpen(projectMenuOpen === project.path ? null : project.path);
                              }}
                            >
                              <MoreHorizontal size={13} />
                            </button>
                            {projectMenuOpen === project.path && (
                              <div className="project-group-menu">
                                <button
                                  type="button"
                                  onClick={() => {
                                    togglePinProject(project.path);
                                    setProjectMenuOpen(null);
                                  }}
                                >
                                  {pinnedProjects.has(project.path) ? (
                                    <><PinOff size={13} /><span>取消置顶</span></>
                                  ) : (
                                    <><Pin size={13} /><span>置顶</span></>
                                  )}
                                </button>
                                <button
                                  type="button"
                                  onClick={() => {
                                    if (window.confirm(`确定从项目列表中删除"${project.name}"？\n\n该操作不会删除项目内的会话。`)) {
                                      const map = readThreadProjectMap();
                                      let changed = false;
                                      for (const id of Object.keys(map)) {
                                        if (map[id] === project.path) { delete map[id]; changed = true; }
                                      }
                                      if (changed) {
                                        setThreadProjectMap(map);
                                        saveThreadProjectMap(map);
                                      }
                                      // 同时从 known 列表移除空项目
                                      try {
                                        const knownRaw = localStorage.getItem("kcoder_known_projects");
                                        const known = knownRaw ? (JSON.parse(knownRaw) as string[]) : [];
                                        const filtered = known.filter((p) => p !== project.path);
                                        if (filtered.length !== known.length) {
                                          localStorage.setItem("kcoder_known_projects", JSON.stringify(filtered));
                                        }
                                      } catch { /* noop */ }
                                      setExpandedProjects((prev) => {
                                        const next = new Set(prev);
                                        next.delete(project.path);
                                        return next;
                                      });
                                    }
                                    setProjectMenuOpen(null);
                                  }}
                                >
                                  <Trash2 size={13} /><span>删除</span>
                                </button>
                              </div>
                            )}
                          </div>
                        </span>
                      </div>
                      {isExpanded && (
                        <div className="project-group-children">
                          {project.threads.length ? (
                            project.threads.map((thread) => {
                              const isSubagentThread = subagentThreadIds.has(thread.id);
                              return (
                                <div className={cn("thread-item thread-item--child", thread.id === activeThreadId && "thread-item--active")} key={thread.id}>
                                  <button className="thread-item-main" type="button" onClick={() => void selectThread(thread.id)}>
                                    <MessageSquare size={14} />
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
                            })
                          ) : (
                            <div className="project-group-empty">尚无会话，点击 + 创建</div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                });
              })()}
            </nav>
          )}
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
          <button
            className="sidebar-settings-button"
            type="button"
            onClick={() => setLogViewerOpen(true)}
            aria-label="查看本地日志"
            title="查看本地运行日志"
          >
            <ScrollText size={16} />
            <span>日志</span>
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
        </div>

        <div className={cn("message-area", hasConversationContent && "message-area--populated")} ref={messageAreaRef}>
          {loading && !hasConversationContent ? (
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
                    <div className="message-body">
                      <div className="message-role">{message.role === "user" ? "你" : "k-Coder"}</div>
                      {message.role === "user" && message.attachments?.length ? (
                        <div className="message-attachments" aria-label="图片附件">
                          {message.attachments.map((attachment, index) => (
                            <span className="message-image-attachment" key={`${message.id}-${index}-${attachment.name}`} title={attachment.name}>
                              <ImageIcon size={15} />
                              <span>{attachment.name}</span>
                            </span>
                          ))}
                        </div>
                      ) : null}
                      {message.role === "assistant" && (
                        <ConversationTurnActivity
                          activities={messageActivities}
                          timeline={messageTimeline}
                          changes={changes}
                          plan={messagePlan}
                          streaming={message.status === "streaming"}
                          activityStatus={messageActivityStatus}
                          finalMessageId={message.id}
                          renderText={renderMessageText}
                        />
                      )}
                      {!messageTimeline.length && (message.text || (message.status === "streaming" && !messageActivityStatus)) ? <div className="message-content">
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
                  <div className="message-body">
                    <div className="message-role">k-Coder</div>
                    <ConversationTurnActivity activities={[]} plan={plan} />
                  </div>
                </article>
              ) : null}

              {/* 内嵌授权请求 */}
              {pendingApproval && (
                <article className="message message--approval">
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

              {loading && (
                <div className="message-list-loading">
                  <Activity className="spin" size={20} />
                  <span>正在读取会话</span>
                </div>
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
        {pendingQueueCount > 0 && (
          <div className="message-queue">
            <button
              type="button"
              className="queue-toggle"
              onClick={() => setQueueExpanded(!queueExpanded)}
            >
              <span className={cn("queue-arrow", queueExpanded && "queue-arrow--expanded")}>▶</span>
              <span className="queue-title">队列 ({pendingQueueCount})</span>
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
                    {queueItem.status === "pending" && (
                      <div className="queue-item-actions">
                        <button
                          type="button"
                          aria-label={`立即发送 ${queueItem.input || "图片消息"}`}
                          title="立即发送并打断当前对话"
                          onClick={() => void sendQueuedMessageNow(queueItem.id)}
                        >
                          <ArrowUp size={16} strokeWidth={2.2} />
                        </button>
                        <button
                          type="button"
                          aria-label={`删除队列消息 ${queueItem.input || "图片消息"}`}
                          title="从队列删除"
                          onClick={() => removeQueuedMessage(queueItem.id)}
                        >
                          <Trash2 size={16} />
                        </button>
                      </div>
                    )}
                  </div>
                ))}
                {pendingQueueCount > 0 && (
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
          {attachments.length > 0 && (
            <div className="attachment-strip">
              {attachments.map((attachment) => (
                <span
                  key={attachment.path}
                  className={attachment.kind === "image" ? "attachment-tag attachment-tag--image" : "attachment-tag"}
                  title={attachment.name}
                >
                  {attachment.kind === "image"
                    ? <img src={attachment.content} alt={attachment.name} className="attachment-thumb" />
                    : <Paperclip size={12} />}
                  <span className="attachment-name">{attachment.name}</span>
                  <button type="button" aria-label={`移除 ${attachment.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== attachment.path))}><X size={12} /></button>
                </span>
              ))}
            </div>
          )}
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
                height: 32,
                padding: 0,
                border: "1px solid transparent",
                borderRadius: "var(--radius-sm)",
                background: "transparent",
                cursor: "pointer",
              }}
            ><ImageIcon size={18} /></button>
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
                disabled={!draft.trim() && attachments.length === 0}
              >
                <ArrowUp size={18} strokeWidth={2.2} />
              </button>
            </div>
          </div>
        </form>
      </section>

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
      {logViewerOpen && (
        <LogViewerDialog onClose={() => setLogViewerOpen(false)} />
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
  const percent = goal.tokenBudget
    ? Math.min(100, Math.round((goal.tokensUsed / goal.tokenBudget) * 100))
    : null;
  const paused = goal.state === "paused";
  const blocked = goal.state === "blocked";
  return (
    <div
      className={`goal-slim${percent === null ? " goal-slim--unlimited" : ""}`}
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
      {percent !== null && (
        <span className="goal-slim-track" aria-hidden="true">
          <span style={{ width: `${percent}%` }} />
        </span>
      )}
      <span className="goal-slim-copy">
        {paused ? "已暂停" : blocked ? "已阻塞" : "运行中"} · {formatTokenUsage(goal.tokensUsed, goal.tokenBudget)}
      </span>
      <span className="goal-slim-manage" aria-hidden="true">管理</span>
    </div>
  );
}

function formatTokenUsage(tokensUsed: number, tokenBudget: number | null) {
  return tokenBudget === null
    ? `${tokensUsed.toLocaleString()} / 无上限 tokens`
    : `${tokensUsed.toLocaleString()} / ${tokenBudget.toLocaleString()} tokens`;
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
  state: "running" | "completed" | "failed" | "cancelled";
  call: { name: string; arguments: Record<string, unknown> };
  result: { output: string } | null;
}) {
  if (activity.state === "running") return "执行中";
  if (activity.state === "failed") return activity.result?.output || "执行失败";
  if (activity.state === "cancelled") return "已取消";
  const args = activity.call.arguments ?? {};
  if (activity.call.name === "search_repository" && typeof args.query === "string") {
    return `搜索 ${args.query}`;
  }
  if (activity.call.name === "read_file" && typeof args.path === "string") {
    const startLine = typeof args.startLine === "number" ? args.startLine : null;
    const lineCount = typeof args.lineCount === "number" ? args.lineCount : null;
    const range = startLine !== null
      ? lineCount !== null && lineCount > 1 ? ` L${startLine}-${startLine + lineCount - 1}` : ` L${startLine}`
      : "";
    return `读取 ${args.path}${range}`;
  }
  const path = args.path;
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

export default App;
