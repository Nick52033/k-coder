import { lazy, memo, Suspense, useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownToLine, Braces, ChevronDown, ChevronRight, CircleCheck,
  CodeXml, Database, Eye, File, FileCode2, FileCog, FileText, Folder, FolderOpen,
  GitBranch, Hash, Image, LocateFixed, Paperclip, Plus, RefreshCw, Search,
  RotateCcw, Save, Terminal, Upload, X,
} from "lucide-react";
import "./WorkbenchPanel.css";
import { MarkdownContent } from "./MarkdownContent";
import {
  extractAttachment, getGitBranches, getGitDiff, getGitStatus, getWorkspaceState,
  listWorkspaceDirectory, openWorkspaceFile, previewWorkspaceFile, revealWorkspaceFile,
  runGitAction, saveWorkspaceFile, switchGitBranch, switchWorkspace,
  searchRepository,
} from "../api/runtime";
import type {
  AttachmentContent, FileEntry, FilePreview, GitBranchView, GitStatusView,
  ProjectRecord, WorkspaceState,
  SearchResult,
} from "../types/runtime";

type Tab = "files" | "git" | "terminal";

const CodeEditor = lazy(() => import("./CodeEditor").then((module) => ({ default: module.CodeEditor })));
const TerminalPanel = lazy(() => import("./TerminalPanel").then((module) => ({ default: module.TerminalPanel })));

export function WorkspacePicker({ onChanged, compact = false }: { onChanged: () => void; compact?: boolean }) {
  const [state, setState] = useState<WorkspaceState | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState("");

  const load = () => getWorkspaceState().then(setState).catch(() => undefined);
  useEffect(() => { void load(); }, []);

  async function select(project?: ProjectRecord) {
    try {
      let path = project?.path;
      if (!path) {
        const selected = await open({ directory: true, multiple: false, title: "选择项目工作区" });
        if (typeof selected !== "string") return;
        path = selected;
      }
      const trusted = project?.trusted || window.confirm(`信任并打开此工作区？\n\n${path}\n\n信任后，智能体可以读取文件并在审批后修改内容。`);
      if (!trusted) return;
      await switchWorkspace(path, true);
      setExpanded(false);
      setError("");
      await load();
      onChanged();
    } catch (reason) {
      setError(toReadableError(reason));
    }
  }

  return (
    <div className={`workspace-picker${compact ? " workspace-picker--compact" : ""}`}>
      <button className="workspace-current" type="button" onClick={() => setExpanded(!expanded)} aria-expanded={expanded} title={state?.current.path ?? "工作区路径"}>
        <span className="workspace-glyph"><FolderOpen size={compact ? 13 : 15} /></span>
        <span className="workspace-info">
          <strong>{state?.current.name ?? "工作区"}</strong>
          {!compact && state?.current.path && <small>{state.current.path.replace(/^\\\\\?\\/, '').replace(/\\/g, '/')}</small>}
        </span>
        <ChevronDown size={compact ? 12 : 14} />
      </button>
      {expanded && (
        <div className="workspace-menu">
          <div className="workspace-menu-label">最近项目</div>
          {state?.recent.slice(0, 6).map((project) => (
            <button type="button" key={project.id} onClick={() => void select(project)}>
              <Folder size={14} /><span><strong>{project.name}</strong><small>{project.path.replace(/^\\\\\?\\/, '').replace(/\\/g, '/')}</small></span>
              {project.id === state.current.id && <CircleCheck size={14} />}
            </button>
          ))}
          <button className="workspace-open" type="button" onClick={() => void select()}><FolderOpen size={14} />打开其他文件夹</button>
        </div>
      )}
      {error && <div className="workspace-error" role="alert">{error}</div>}
    </div>
  );
}

export function WorkbenchPanel({ onAttach, open = false }: { onAttach: (attachment: AttachmentContent) => void; open?: boolean }) {
  const [tab, setTab] = useState<Tab>("files");
  const [terminalMounted, setTerminalMounted] = useState(false);
  const openTerminal = () => {
    setTerminalMounted(true);
    setTab("terminal");
  };
  return (
    <aside className={`workbench-panel ${open ? "workbench-panel--open" : ""}`}>
      <div className="workbench-tabs" role="tablist" aria-label="工作台面板">
        <TabButton active={tab === "files"} icon={<FileCode2 size={15} />} label="文件" onClick={() => setTab("files")} />
        <TabButton active={tab === "git"} icon={<GitBranch size={15} />} label="Git" onClick={() => setTab("git")} />
        <TabButton active={tab === "terminal"} icon={<Terminal size={15} />} label="终端" onClick={openTerminal} />
      </div>
      {tab === "files" && <FilesView onAttach={onAttach} />}
      {tab === "git" && <GitView />}
      {terminalMounted && tab === "terminal" && (
        <div className="terminal-tab-host">
          <Suspense fallback={<div className="panel-empty">正在载入终端...</div>}>
            <TerminalPanel visible />
          </Suspense>
        </div>
      )}
    </aside>
  );
}

function TabButton({ active, icon, label, onClick }: { active: boolean; icon: React.ReactNode; label: string; onClick: () => void }) {
  return <button className={active ? "active" : ""} role="tab" aria-selected={active} type="button" onClick={onClick}>{icon}<span>{label}</span></button>;
}

function FilesView({ onAttach }: { onAttach: (attachment: AttachmentContent) => void }) {
  const [revision, setRevision] = useState(0);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [hasWorkspace, setHasWorkspace] = useState(true);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState("");
  const [mdView, setMdView] = useState<"preview" | "source">("preview");
  const dirty = Boolean(preview?.editable && draft !== (preview.content ?? ""));

  const select = useCallback(async (path: string) => {
    if (path === preview?.path) return;
    if (dirty && !window.confirm("当前文件有未保存的修改，放弃修改并打开其他文件？")) return;
    try {
      const next = await previewWorkspaceFile(path);
      setPreview(next);
      setDraft(next.content ?? "");
      setMdView("preview");
      setError("");
      setNotice("");
    } catch (error) {
      setError(toReadableError(error));
    }
  }, [preview?.path, dirty]);

  const save = useCallback(async () => {
    if (!preview?.editable || !preview.contentHash || !dirty || saving) return;
    setSaving(true);
    setNotice("");
    try {
      const saved = await saveWorkspaceFile({
        path: preview.path,
        content: draft,
        expectedHash: preview.contentHash,
      });
      setPreview(saved);
      setDraft(saved.content ?? "");
      setError("");
      setNotice("已保存");
      setRevision((value) => value + 1);
    } catch (reason) {
      setError(toReadableError(reason));
    } finally {
      setSaving(false);
    }
  }, [preview?.editable, preview?.contentHash, preview?.path, draft, dirty, saving]);

  const closePreview = useCallback(() => {
    if (dirty && !window.confirm("当前文件有未保存的修改，确定关闭？")) return;
    setPreview(null);
    setDraft("");
    setMdView("preview");
    setNotice("");
    setError("");
  }, [dirty]);

  const attach = useCallback(async () => {
    if (preview) onAttach(await extractAttachment(preview.path));
  }, [preview, onAttach]);

  useEffect(() => {
    if (!preview) return;
    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") closePreview();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [preview, closePreview]);

  useEffect(() => {
    void getWorkspaceState()
      .then((state) => setHasWorkspace(Boolean(state.current)))
      .catch(() => setHasWorkspace(false));
  }, []);

  if (!hasWorkspace) {
    return (
      <div className="files-view">
        <div className="panel-toolbar">
          <WorkspacePicker onChanged={() => setRevision((value) => value + 1)} compact />
        </div>
        <div className="panel-empty">
          <Folder size={48} />
          <div>
            <strong>浏览和附加项目文件</strong>
            <p>打开工作区后可以：</p>
            <ul>
              <li>浏览文件和文件夹</li>
              <li>搜索代码内容</li>
              <li>预览文件</li>
              <li>附加文件到对话</li>
            </ul>
          </div>
        </div>
      </div>
    );
  }

  const isMarkdown = preview?.language === "markdown";
  const showMarkdownPreview = Boolean(isMarkdown && mdView === "preview" && !preview?.dataUrl && preview?.content !== null);

  return (
    <div className="files-view">
      <div className="panel-toolbar"><WorkspacePicker onChanged={() => setRevision((value) => value + 1)} compact /><button type="button" title="刷新" aria-label="刷新文件树" onClick={() => setRevision((value) => value + 1)}><RefreshCw size={14} /></button></div>
      <form className="repository-search" onSubmit={(event) => { event.preventDefault(); if (query.trim()) void searchRepository(query.trim()).then(setResults).catch((reason) => setError(String(reason))); }}>
        <Search size={14} />
        <input value={query} onChange={(event) => { setQuery(event.target.value); if (!event.target.value) setResults([]); }} placeholder="搜索仓库" aria-label="搜索仓库" />
        <button type="submit" aria-label="执行搜索"><Search size={13} /></button>
      </form>
      {results.length > 0 ? <div className="repository-results">{results.map((result) => <button type="button" key={`${result.path}:${result.line}`} onClick={() => void select(result.path)}><strong>{result.path}:{result.line}</strong><span>{result.preview}</span></button>)}</div> : <div className="file-tree" key={revision}><DirectoryNode path="" depth={0} selectedPath={preview?.path ?? null} onSelect={(path) => void select(path)} /></div>}
      {error && <div className="panel-error">{error}</div>}
      {preview && (
        <div className="file-preview-backdrop" onMouseDown={closePreview}>
          <section className="file-preview file-preview-dialog" role="dialog" aria-modal="true" aria-label={`预览 ${preview.name}`} onMouseDown={(event) => event.stopPropagation()}>
          <div className="preview-header"><span title={preview.path}>{preview.name}{dirty && <i className="preview-dirty" title="有未保存的修改" aria-label="有未保存的修改" />}</span><div className="preview-header-actions">{isMarkdown && !preview.dataUrl && (<div className="markdown-view-toggle" role="group" aria-label="Markdown 视图切换"><button className={mdView === "preview" ? "active" : ""} type="button" title="渲染预览" onClick={() => setMdView("preview")}><Eye size={13} />预览</button><button className={mdView === "source" ? "active" : ""} type="button" title="编辑源码" onClick={() => setMdView("source")}><CodeXml size={13} />源码</button></div>)}<button type="button" aria-label="关闭预览" title="关闭 (Esc)" onClick={closePreview}><X size={14} /></button></div></div>
          {preview.dataUrl ? <img src={preview.dataUrl} alt={preview.name} /> : showMarkdownPreview ? <div className="markdown-preview-body"><MarkdownContent text={draft} /></div> : <Suspense fallback={<div className="code-editor-loading">正在载入编辑器...</div>}><CodeEditor key={preview.path} path={preview.path} language={preview.language} value={draft} readOnly={!preview.editable} onChange={setDraft} onSave={save} /></Suspense>}
          {preview.truncated && <small>文件超过 256 KiB，已截断并以只读方式打开</small>}
          {!preview.dataUrl && !preview.truncated && !preview.editable && <small>该文件不是 UTF-8 文本，仅支持查看</small>}
          {notice && <div className="preview-notice" role="status">{notice}</div>}
          <div className="preview-actions">
            <button className="preview-save-action" type="button" disabled={!dirty || saving} title="保存 (Ctrl+S)" onClick={() => void save()}><Save size={14} />{saving ? "保存中" : "保存"}</button>
            <button type="button" disabled={!dirty || saving} title="放弃未保存的修改" onClick={() => setDraft(preview.content ?? "")}><RotateCcw size={14} />放弃</button>
            <button type="button" title="附加到消息" onClick={() => void attach()}><Paperclip size={14} />附加</button>
            <button className="preview-icon-action" type="button" aria-label="使用系统编辑器打开" title="使用系统编辑器打开" onClick={() => void openWorkspaceFile(preview.path)}><Upload size={14} /></button>
            <button className="preview-icon-action" type="button" aria-label="在资源管理器中定位" title="在资源管理器中定位" onClick={() => void revealWorkspaceFile(preview.path)}><LocateFixed size={14} /></button>
          </div>
          </section>
        </div>
      )}
    </div>
  );
}

const DirectoryNode = memo(function DirectoryNode({ path, depth, selectedPath, onSelect }: { path: string; depth: number; selectedPath: string | null; onSelect: (path: string) => void }) {
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    void listWorkspaceDirectory(path)
      .then((value) => { if (!disposed) { setEntries(value); setError(""); } })
      .catch((reason) => { if (!disposed) setError(toReadableError(reason)); });
    return () => { disposed = true; };
  }, [path]);
  return <>{error && <div className="tree-error" role="alert">{error}</div>}{entries.map((entry) => entry.isDirectory ? <FolderNode key={entry.path} entry={entry} depth={depth} selectedPath={selectedPath} onSelect={onSelect} /> : (
    <button className={`tree-row ${selectedPath === entry.path ? "tree-row--selected" : ""}`} aria-current={selectedPath === entry.path ? "true" : undefined} style={{ paddingLeft: 25 + depth * 14 }} type="button" key={entry.path} onClick={() => onSelect(entry.path)}>
      <FileTypeIcon name={entry.name} /><span>{entry.name}</span>
    </button>
  ))}</>;
});

const FolderNode = memo(function FolderNode({ entry, depth, selectedPath, onSelect }: { entry: FileEntry; depth: number; selectedPath: string | null; onSelect: (path: string) => void }) {
  const [open, setOpen] = useState(false);
  return <div>
    <button className="tree-row" style={{ paddingLeft: 8 + depth * 14 }} type="button" onClick={() => setOpen(!open)} aria-expanded={open}>
      {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}<Folder size={14} /><span>{entry.name}</span>
    </button>
    {open && <DirectoryNode path={entry.path} depth={depth + 1} selectedPath={selectedPath} onSelect={onSelect} />}
  </div>;
});

function toReadableError(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  try {
    const parsed = JSON.stringify(reason);
    return parsed === undefined || parsed === '"undefined"' ? "未知错误" : parsed;
  } catch {
    return "未知错误";
  }
}

function GitView() {
  const [status, setStatus] = useState<GitStatusView | null>(null);
  const [branches, setBranches] = useState<GitBranchView | null>(null);
  const [diff, setDiff] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busyAction, setBusyAction] = useState<"stage" | "unstage" | "commit" | "pull" | "push" | null>(null);
  const refresh = async () => {
    const [statusResult, branchesResult] = await Promise.allSettled([getGitStatus(), getGitBranches()]);
    if (statusResult.status === "fulfilled") { setStatus(statusResult.value); setError(""); }
    else setError(toReadableError(statusResult.reason));
    if (branchesResult.status === "fulfilled") setBranches(branchesResult.value);
  };
  useEffect(() => { void refresh(); }, []);
  async function action(name: "stage" | "unstage" | "commit" | "pull" | "push", paths: string[] = []) {
    const confirmation = name === "commit" ? `提交当前已暂存的更改？\n\n${message}`
      : name === "pull" ? "从远程拉取并快进当前分支？"
      : name === "push" ? "将当前分支推送到远程？"
      : null;
    if (confirmation && !window.confirm(confirmation)) return;
    setBusyAction(name);
    setNotice("");
    try {
      await runGitAction(name, paths, name === "commit" ? message : undefined, Boolean(confirmation));
      if (name === "commit") setMessage("");
      setError("");
      setNotice(gitActionSuccessLabel(name));
      await refresh();
    } catch (error) {
      setError(toReadableError(error));
    } finally {
      setBusyAction(null);
    }
  }
  async function changeBranch(branch: string, create = false) {
    if (!branch || (!create && branch === branches?.current)) return;
    if (!window.confirm(`${create ? "创建并切换到" : "切换到"}分支"${branch}"？\n\n未提交的更改会保留；如有冲突，Git 将拒绝切换。`)) return;
    try { await switchGitBranch(branch, create, true); setDiff(""); await refresh(); } catch (reason) { setError(toReadableError(reason)); }
  }
  if (status && !status.isRepository) return <div className="panel-empty"><GitBranch size={22} /><span>当前工作区不是 Git 仓库</span></div>;
  const hasUnstagedChanges = Boolean(status?.files.some(isGitFileStageable));
  const hasStagedChanges = Boolean(status?.files.some(isGitFileStaged));
  return <div className="git-view">
    <div className="panel-toolbar"><span title={status?.upstream ?? "尚未关联远程分支"}><GitBranch size={14} /><strong>{status?.branch ?? "Git"}</strong>{status && (status.ahead || status.behind) ? <small>↑{status.ahead} ↓{status.behind}</small> : null}</span><button type="button" title="刷新" aria-label="刷新 Git 状态" onClick={() => void refresh()}><RefreshCw size={14} /></button></div>
    <div className="branch-controls"><select aria-label="当前分支" value={branches?.current ?? ""} onChange={(event) => void changeBranch(event.target.value)}>{branches?.branches.map((branch) => <option key={branch} value={branch}>{branch}</option>)}</select><button type="button" title="新建分支" aria-label="新建分支" onClick={() => { const branch = window.prompt("新分支名称"); if (branch?.trim()) void changeBranch(branch.trim(), true); }}><Plus size={14} /></button></div>
    <div className="git-actions">
      <button type="button" disabled={busyAction !== null || !status?.upstream} title={status?.upstream ? "从远程快进拉取" : "当前分支尚未关联远程分支"} onClick={() => void action("pull")}><GitActionIcon active={busyAction === "pull"} icon={<ArrowDownToLine size={14} />} />拉取</button>
      <button type="button" disabled={busyAction !== null || !status?.branch} title={status?.upstream ? "推送当前分支" : "首次推送将关联 origin"} onClick={() => void action("push")}><GitActionIcon active={busyAction === "push"} icon={<Upload size={14} />} />推送</button>
      <button type="button" disabled={busyAction !== null || !hasUnstagedChanges} title="暂存全部更改" onClick={() => void action("stage")}><GitActionIcon active={busyAction === "stage"} icon={<Plus size={14} />} />全部暂存</button>
    </div>
    <div className="git-files">{status?.files.map((file) => {
      const nextAction = isGitFileStageable(file) ? "stage" : "unstage";
      const actionLabel = nextAction === "stage" ? "暂存" : "取消暂存";
      return <div className="git-file" key={file.path}><button type="button" title="查看 Diff" onClick={() => void getGitDiff(file.path, isGitFileStaged(file) && !isGitFileStageable(file)).then(setDiff).catch((reason) => setError(String(reason)))}><span>{file.path}</span><code>{file.indexStatus}{file.worktreeStatus}</code></button><button type="button" disabled={busyAction !== null} title={actionLabel} aria-label={`${actionLabel} ${file.path}`} onClick={() => void action(nextAction, [file.path])}>{nextAction === "stage" ? "+" : "−"}</button></div>;
    })}</div>
    {diff && <pre className="git-diff">{diff}</pre>}
    <div className="commit-box"><input value={message} onChange={(event) => setMessage(event.target.value)} placeholder="提交说明" aria-label="提交说明" /><button type="button" disabled={busyAction !== null || !message.trim() || !hasStagedChanges} title={hasStagedChanges ? "提交已暂存的更改" : "请先暂存更改"} onClick={() => void action("commit")}>{busyAction === "commit" ? "提交中" : "提交"}</button></div>
    {notice && <div className="panel-notice" role="status">{notice}</div>}
    {error && <div className="panel-error">{error}</div>}
  </div>;
}

const FileTypeIcon = memo(function FileTypeIcon({ name }: { name: string }) {
  const visual = fileVisual(name);
  const Icon = visual.icon;
  return <Icon className={`file-type-icon file-type-icon--${visual.kind}`} size={14} aria-hidden="true" />;
});

function fileVisual(name: string) {
  const lower = name.toLowerCase();
  const extension = lower.includes(".") ? lower.slice(lower.lastIndexOf(".") + 1) : "";
  if (/^(package|tsconfig|jsconfig).*\.json$/.test(lower) || extension === "json") return { kind: "json", icon: Braces };
  if (["ts", "tsx"].includes(extension)) return { kind: "typescript", icon: FileCode2 };
  if (["js", "jsx", "mjs", "cjs"].includes(extension)) return { kind: "javascript", icon: FileCode2 };
  if (["html", "htm", "xml", "svg"].includes(extension)) return { kind: "markup", icon: CodeXml };
  if (["css", "scss", "sass", "less"].includes(extension)) return { kind: "style", icon: Hash };
  if (["md", "mdx", "txt", "rtf"].includes(extension)) return { kind: "document", icon: FileText };
  if (["rs", "toml"].includes(extension) || lower === "cargo.lock") return { kind: "rust", icon: FileCog };
  if (["py", "pyi", "ipynb"].includes(extension)) return { kind: "python", icon: FileCode2 };
  if (["yaml", "yml"].includes(extension)) return { kind: "yaml", icon: FileCog };
  if (["sql", "db", "sqlite", "sqlite3"].includes(extension)) return { kind: "database", icon: Database };
  if (["sh", "bash", "zsh", "ps1", "bat", "cmd"].includes(extension)) return { kind: "shell", icon: Terminal };
  if (["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"].includes(extension)) return { kind: "image", icon: Image };
  if (lower.startsWith(".env") || ["gitignore", "gitattributes", "editorconfig"].includes(extension)) return { kind: "config", icon: FileCog };
  return { kind: "default", icon: File };
}

function isGitFileStageable(file: GitStatusView["files"][number]) {
  return file.indexStatus === "?" || Boolean(file.worktreeStatus.trim());
}

function isGitFileStaged(file: GitStatusView["files"][number]) {
  return file.indexStatus !== " " && file.indexStatus !== "?";
}

function gitActionSuccessLabel(action: "stage" | "unstage" | "commit" | "pull" | "push") {
  return action === "stage" ? "更改已暂存" : action === "unstage" ? "已取消暂存" : action === "commit" ? "提交完成" : action === "pull" ? "拉取完成" : "推送完成";
}

function GitActionIcon({ active, icon }: { active: boolean; icon: React.ReactNode }) {
  return active ? <RefreshCw className="git-action-spinner" size={14} /> : icon;
}
