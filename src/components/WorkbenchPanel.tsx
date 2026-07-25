import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownToLine, Braces, ChevronDown, ChevronRight, Circle, CircleCheck, CircleDot,
  CodeXml, Database, File, FileCode2, FileCog, FileDiff, FileText, Folder, FolderOpen,
  GitBranch, Hash, Image, ListChecks, LocateFixed, Paperclip, Plus, RefreshCw, Search,
  Terminal, Upload, X,
} from "lucide-react";
import "./WorkbenchPanel.css";
import {
  extractAttachment, getGitBranches, getGitDiff, getGitStatus, getWorkspaceState,
  listWorkspaceDirectory, openWorkspaceFile, previewWorkspaceFile, revealWorkspaceFile,
  runGitAction, switchGitBranch, switchWorkspace,
  searchRepository,
} from "../api/runtime";
import type {
  AttachmentContent, ChangeSet, FileEntry, FilePreview, GitBranchView, GitStatusView,
  ProjectRecord, ToolActivity, WorkspaceState,
  PlanView as PersistentPlan,
  SearchResult,
} from "../types/runtime";

type Tab = "files" | "git" | "plan";

export function WorkspacePicker({ onChanged }: { onChanged: () => void }) {
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
      setError(String(reason));
    }
  }

  return (
    <div className="workspace-picker">
      <button className="workspace-current" type="button" onClick={() => setExpanded(!expanded)} aria-expanded={expanded} title={state?.current.path ?? "工作区路径"}>
        <span className="workspace-glyph"><FolderOpen size={15} /></span>
        <span className="workspace-info">
          <strong>{state?.current.name ?? "工作区"}</strong>
          {state?.current.path && <small>{state.current.path.replace(/^\\\\\?\\/, '').replace(/\\/g, '/')}</small>}
        </span>
        <ChevronDown size={14} />
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

export function WorkbenchPanel({ plan, toolActivities, changes, onAttach, onSelectChange, open = false }: { plan: PersistentPlan | null; toolActivities: ToolActivity[]; changes: ChangeSet[]; onAttach: (attachment: AttachmentContent) => void; onSelectChange: (changeId: string) => void; open?: boolean }) {
  const [tab, setTab] = useState<Tab>("files");
  return (
    <aside className={`workbench-panel ${open ? "workbench-panel--open" : ""}`}>
      <div className="workbench-tabs" role="tablist" aria-label="工作台面板">
        <TabButton active={tab === "files"} icon={<FileCode2 size={15} />} label="文件" onClick={() => setTab("files")} />
        <TabButton active={tab === "git"} icon={<GitBranch size={15} />} label="Git" onClick={() => setTab("git")} />
        <TabButton active={tab === "plan"} icon={<ListChecks size={15} />} label="计划" onClick={() => setTab("plan")} />
      </div>
      {tab === "files" && <FilesView onAttach={onAttach} />}
      {tab === "git" && <GitView />}
      {tab === "plan" && <PlanView plan={plan} activities={toolActivities} changes={changes} onSelectChange={onSelectChange} />}
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
  async function select(path: string) { try { setPreview(await previewWorkspaceFile(path)); setError(""); } catch (error) { setError(String(error)); } }
  async function attach() { if (preview) onAttach(await extractAttachment(preview.path)); }
  return (
    <div className="files-view">
      <div className="panel-toolbar"><strong>资源管理器</strong><button type="button" title="刷新" aria-label="刷新文件树" onClick={() => setRevision((value) => value + 1)}><RefreshCw size={14} /></button></div>
      <form className="repository-search" onSubmit={(event) => { event.preventDefault(); if (query.trim()) void searchRepository(query.trim()).then(setResults).catch((reason) => setError(String(reason))); }}>
        <Search size={14} />
        <input value={query} onChange={(event) => { setQuery(event.target.value); if (!event.target.value) setResults([]); }} placeholder="搜索仓库" aria-label="搜索仓库" />
        <button type="submit" aria-label="执行搜索"><Search size={13} /></button>
      </form>
      {results.length > 0 ? <div className="repository-results">{results.map((result) => <button type="button" key={`${result.path}:${result.line}`} onClick={() => void select(result.path)}><strong>{result.path}:{result.line}</strong><span>{result.preview}</span></button>)}</div> : <div className="file-tree" key={revision}><DirectoryNode path="" depth={0} onSelect={(path) => void select(path)} /></div>}
      {error && <div className="panel-error">{error}</div>}
      {preview && (
        <div className="file-preview">
          <div className="preview-header"><span title={preview.path}>{preview.name}</span><button type="button" aria-label="关闭预览" onClick={() => setPreview(null)}><X size={14} /></button></div>
          {preview.dataUrl ? <img src={preview.dataUrl} alt={preview.name} /> : <pre><code>{preview.content}</code></pre>}
          {preview.truncated && <small>预览已截断</small>}
          <div className="preview-actions">
            <button type="button" title="附加到消息" onClick={() => void attach()}><Paperclip size={14} />附加</button>
            <button type="button" title="使用系统编辑器打开" onClick={() => void openWorkspaceFile(preview.path)}><Upload size={14} /></button>
            <button type="button" title="在资源管理器中定位" onClick={() => void revealWorkspaceFile(preview.path)}><LocateFixed size={14} /></button>
          </div>
        </div>
      )}
    </div>
  );
}

function DirectoryNode({ path, depth, onSelect }: { path: string; depth: number; onSelect: (path: string) => void }) {
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    void listWorkspaceDirectory(path)
      .then((value) => { if (!disposed) { setEntries(value); setError(""); } })
      .catch((reason) => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; };
  }, [path]);
  return <>{error && <div className="tree-error" role="alert">{error}</div>}{entries.map((entry) => entry.isDirectory ? <FolderNode key={entry.path} entry={entry} depth={depth} onSelect={onSelect} /> : (
    <button className="tree-row" style={{ paddingLeft: 25 + depth * 14 }} type="button" key={entry.path} onClick={() => onSelect(entry.path)}>
      <FileTypeIcon name={entry.name} /><span>{entry.name}</span>
    </button>
  ))}</>;
}

function FolderNode({ entry, depth, onSelect }: { entry: FileEntry; depth: number; onSelect: (path: string) => void }) {
  const [open, setOpen] = useState(false);
  return <div>
    <button className="tree-row" style={{ paddingLeft: 8 + depth * 14 }} type="button" onClick={() => setOpen(!open)} aria-expanded={open}>
      {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}<Folder size={14} /><span>{entry.name}</span>
    </button>
    {open && <DirectoryNode path={entry.path} depth={depth + 1} onSelect={onSelect} />}
  </div>;
}

function GitView() {
  const [status, setStatus] = useState<GitStatusView | null>(null);
  const [branches, setBranches] = useState<GitBranchView | null>(null);
  const [diff, setDiff] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busyAction, setBusyAction] = useState<"stage" | "unstage" | "commit" | "pull" | "push" | null>(null);
  const refresh = () => Promise.all([getGitStatus(), getGitBranches()])
    .then(([nextStatus, nextBranches]) => { setStatus(nextStatus); setBranches(nextBranches); setError(""); })
    .catch((reason) => setError(String(reason)));
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
      setError(String(error));
    } finally {
      setBusyAction(null);
    }
  }
  async function changeBranch(branch: string, create = false) {
    if (!branch || (!create && branch === branches?.current)) return;
    if (!window.confirm(`${create ? "创建并切换到" : "切换到"}分支“${branch}”？\n\n未提交的更改会保留；如有冲突，Git 将拒绝切换。`)) return;
    try { await switchGitBranch(branch, create, true); setDiff(""); await refresh(); } catch (reason) { setError(String(reason)); }
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

function PlanView({ plan, activities, changes, onSelectChange }: { plan: PersistentPlan | null; activities: ToolActivity[]; changes: ChangeSet[]; onSelectChange: (changeId: string) => void }) {
  if (!plan?.steps.length && !activities.length && !changes.length) return <div className="panel-empty"><ListChecks size={22} /><span>智能体创建计划后会显示在这里</span></div>;
  return <div className="plan-list">
    {plan?.steps.map((step, index) => <div className={`plan-step plan-step--${step.status}`} key={step.id}><span>{step.status === "completed" ? <CircleCheck size={14} /> : step.status === "in_progress" ? <CircleDot size={14} /> : step.status === "failed" ? <X size={14} /> : <Circle size={14} />}</span><div><strong>{index + 1}. {step.step}</strong><small>{step.detail ?? (step.status === "in_progress" ? "执行中" : step.status === "completed" ? "已完成" : step.status === "failed" ? "失败" : step.status === "skipped" ? "已跳过" : "待处理")}</small></div></div>)}
    {!plan?.steps.length && activities.map((activity, index) => <div className="plan-step" key={`${activity.turnId}-${activity.call.id}`}><span>{activity.state === "completed" ? <CircleCheck size={14} /> : <span className="plan-index">{index + 1}</span>}</span><div><strong>{activity.call.name}</strong><small>{activity.state}</small></div></div>)}
    {changes.length > 0 && <div className="plan-changes"><div className="plan-section-label">代码变更</div>{changes.slice().reverse().map((change) => <button type="button" key={change.id} onClick={() => onSelectChange(change.id)}><FileDiff size={14} /><span><strong>{change.files.length} 个文件</strong><small>{change.undone ? "已撤销" : "查看 Diff"}</small></span></button>)}</div>}
  </div>;
}

function FileTypeIcon({ name }: { name: string }) {
  const visual = fileVisual(name);
  const Icon = visual.icon;
  return <Icon className={`file-type-icon file-type-icon--${visual.kind}`} size={14} aria-hidden="true" />;
}

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
