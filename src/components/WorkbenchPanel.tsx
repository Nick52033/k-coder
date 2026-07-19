import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDownToLine, ChevronDown, ChevronRight, CircleCheck, File, FileCode2, FileDiff, Folder,
  FolderOpen, GitBranch, Image, ListChecks, LocateFixed, Paperclip, RefreshCw,
  Plus, Upload, X,
} from "lucide-react";
import {
  extractAttachment, getGitBranches, getGitDiff, getGitStatus, getWorkspaceState,
  listWorkspaceDirectory, openWorkspaceFile, previewWorkspaceFile, revealWorkspaceFile,
  runGitAction, switchGitBranch, switchWorkspace,
} from "../api/runtime";
import type {
  AttachmentContent, ChangeSet, FileEntry, FilePreview, GitBranchView, GitStatusView,
  ProjectRecord, ToolActivity, WorkspaceState,
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
      <button className="workspace-current" type="button" onClick={() => setExpanded(!expanded)} aria-expanded={expanded}>
        <span className="workspace-glyph"><FolderOpen size={15} /></span>
        <span><strong>{state?.current.name ?? "工作区"}</strong><small>{state?.current.path ?? "正在读取"}</small></span>
        <ChevronDown size={14} />
      </button>
      {expanded && (
        <div className="workspace-menu">
          <div className="workspace-menu-label">最近项目</div>
          {state?.recent.slice(0, 6).map((project) => (
            <button type="button" key={project.id} onClick={() => void select(project)}>
              <Folder size={14} /><span><strong>{project.name}</strong><small>{project.path}</small></span>
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

export function WorkbenchPanel({ toolActivities, changes, onAttach, onSelectChange, open = false }: { toolActivities: ToolActivity[]; changes: ChangeSet[]; onAttach: (attachment: AttachmentContent) => void; onSelectChange: (changeId: string) => void; open?: boolean }) {
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
      {tab === "plan" && <PlanView activities={toolActivities} changes={changes} onSelectChange={onSelectChange} />}
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
  async function select(path: string) { try { setPreview(await previewWorkspaceFile(path)); setError(""); } catch (error) { setError(String(error)); } }
  async function attach() { if (preview) onAttach(await extractAttachment(preview.path)); }
  return (
    <div className="files-view">
      <div className="panel-toolbar"><strong>资源管理器</strong><button type="button" title="刷新" aria-label="刷新文件树" onClick={() => setRevision((value) => value + 1)}><RefreshCw size={14} /></button></div>
      <div className="file-tree" key={revision}><DirectoryNode path="" depth={0} onSelect={(path) => void select(path)} /></div>
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
      {isImage(entry.name) ? <Image size={14} /> : <File size={14} />}<span>{entry.name}</span>
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
    try { await runGitAction(name, paths, name === "commit" ? message : undefined, Boolean(confirmation)); setMessage(""); setError(""); void refresh(); } catch (error) { setError(String(error)); }
  }
  async function changeBranch(branch: string, create = false) {
    if (!branch || (!create && branch === branches?.current)) return;
    if (!window.confirm(`${create ? "创建并切换到" : "切换到"}分支“${branch}”？\n\n未提交的更改会保留；如有冲突，Git 将拒绝切换。`)) return;
    try { await switchGitBranch(branch, create, true); setDiff(""); await refresh(); } catch (reason) { setError(String(reason)); }
  }
  if (status && !status.isRepository) return <div className="panel-empty"><GitBranch size={22} /><span>当前工作区不是 Git 仓库</span></div>;
  return <div className="git-view">
    <div className="panel-toolbar"><span><GitBranch size={14} /><strong>{status?.branch ?? "Git"}</strong>{status && (status.ahead || status.behind) ? <small>↑{status.ahead} ↓{status.behind}</small> : null}</span><button type="button" title="刷新" aria-label="刷新 Git 状态" onClick={() => void refresh()}><RefreshCw size={14} /></button></div>
    <div className="branch-controls"><select aria-label="当前分支" value={branches?.current ?? ""} onChange={(event) => void changeBranch(event.target.value)}>{branches?.branches.map((branch) => <option key={branch} value={branch}>{branch}</option>)}</select><button type="button" title="新建分支" aria-label="新建分支" onClick={() => { const branch = window.prompt("新分支名称"); if (branch?.trim()) void changeBranch(branch.trim(), true); }}><Plus size={14} /></button></div>
    <div className="git-actions"><button type="button" onClick={() => void action("pull")}><ArrowDownToLine size={14} />拉取</button><button type="button" onClick={() => void action("push")}><Upload size={14} />推送</button></div>
    <div className="git-files">{status?.files.map((file) => <div className="git-file" key={file.path}><button type="button" title="查看 Diff" onClick={() => void getGitDiff(file.path, Boolean(file.indexStatus.trim() && !file.worktreeStatus.trim())).then(setDiff).catch((reason) => setError(String(reason)))}><span>{file.path}</span><code>{file.indexStatus}{file.worktreeStatus}</code></button><button type="button" title={file.indexStatus.trim() ? "取消暂存" : "暂存"} aria-label={`${file.indexStatus.trim() ? "取消暂存" : "暂存"} ${file.path}`} onClick={() => void action(file.indexStatus.trim() ? "unstage" : "stage", [file.path])}>{file.indexStatus.trim() ? "−" : "+"}</button></div>)}</div>
    {diff && <pre className="git-diff">{diff}</pre>}
    <div className="commit-box"><input value={message} onChange={(event) => setMessage(event.target.value)} placeholder="提交说明" aria-label="提交说明" /><button type="button" disabled={!message.trim()} onClick={() => void action("commit")}>提交</button></div>
    {error && <div className="panel-error">{error}</div>}
  </div>;
}

function PlanView({ activities, changes, onSelectChange }: { activities: ToolActivity[]; changes: ChangeSet[]; onSelectChange: (changeId: string) => void }) {
  if (!activities.length && !changes.length) return <div className="panel-empty"><ListChecks size={22} /><span>工具步骤会在这里形成执行计划</span></div>;
  return <div className="plan-list">
    {activities.map((activity, index) => <div className="plan-step" key={`${activity.turnId}-${activity.call.id}`}><span>{activity.state === "completed" ? <CircleCheck size={14} /> : <span className="plan-index">{index + 1}</span>}</span><div><strong>{activity.call.name}</strong><small>{activity.state === "running" ? "执行中" : activity.state === "failed" ? "失败" : "已完成"}</small></div></div>)}
    {changes.length > 0 && <div className="plan-changes"><div className="plan-section-label">代码变更</div>{changes.slice().reverse().map((change) => <button type="button" key={change.id} onClick={() => onSelectChange(change.id)}><FileDiff size={14} /><span><strong>{change.files.length} 个文件</strong><small>{change.undone ? "已撤销" : "查看 Diff"}</small></span></button>)}</div>}
  </div>;
}

function isImage(name: string) { return /\.(png|jpe?g|gif|webp|bmp)$/i.test(name); }
