import { FormEvent, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlertCircle,
  CalendarClock,
  Check,
  CheckCircle2,
  ChevronDown,
  Clock3,
  Folder,
  Grid2X2,
  List,
  Loader2,
  MessageSquare,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  Square,
  TimerReset,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import {
  deleteScheduledTask,
  getWorkspaceState,
  listScheduledTasks,
  listThreads,
  setScheduledTaskEnabled,
  triggerScheduledTask,
  upsertScheduledTask,
} from "../api/runtime";
import type {
  ScheduledTaskKind,
  ScheduledTaskMode,
  ScheduledTaskSchedule,
  ScheduledTaskView,
  ThreadSummary,
  WorkspaceState,
} from "../types/runtime";
import "./ScheduledTasksPage.css";

type ViewMode = "cards" | "table";

interface FormState {
  name: string;
  kind: ScheduledTaskKind;
  onceAt: string;
  hour: string;
  minute: string;
  weekday: string;
  mode: ScheduledTaskMode;
  threadId: string;
  prompt: string;
  enabled: boolean;
}

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function nextHourInput(): string {
  const date = new Date(Date.now() + 60 * 60 * 1000);
  date.setMinutes(0, 0, 0);
  const offset = date.getTimezoneOffset();
  const local = new Date(date.getTime() - offset * 60 * 1000);
  return local.toISOString().slice(0, 16);
}

function taskToForm(task: ScheduledTaskView | null): FormState {
  const schedule = task?.schedule;
  const hour = schedule?.hour ?? 9;
  const minute = schedule?.minute ?? 0;
  const onceAt = schedule?.atMs
    ? (() => {
        const date = new Date(schedule.atMs);
        const offset = date.getTimezoneOffset();
        return new Date(date.getTime() - offset * 60 * 1000).toISOString().slice(0, 16);
      })()
    : nextHourInput();
  return {
    name: task?.name ?? "",
    kind: schedule?.kind ?? "daily",
    onceAt,
    hour: pad(hour),
    minute: pad(minute),
    weekday: String(schedule?.weekday ?? 1),
    mode: task?.mode ?? "background",
    threadId: task?.threadId ?? "",
    prompt: task?.prompt ?? "",
    enabled: task?.enabled ?? true,
  };
}

function formatClock(hour: number | null | undefined, minute: number | null | undefined): string {
  return `${pad(hour ?? 0)}:${pad(minute ?? 0)}`;
}

function formatSchedule(schedule: ScheduledTaskSchedule): string {
  if (schedule.kind === "once") {
    return schedule.atMs
      ? new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(schedule.atMs)
      : "一次性任务";
  }
  const clock = formatClock(schedule.hour, schedule.minute);
  return schedule.kind === "weekly"
    ? `每${WEEKDAYS[schedule.weekday ?? 0]} ${clock}`
    : `每天 ${clock}`;
}

function relativeTime(timestamp: number | null): string {
  if (!timestamp) return "未安排";
  const delta = timestamp - Date.now();
  if (delta <= 0) return "正在排队";
  const minutes = Math.floor(delta / 60_000);
  if (minutes < 60) return `${Math.max(1, minutes)} 分钟后`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时 ${minutes % 60} 分钟后`;
  return `${Math.floor(hours / 24)} 天后`;
}

function formatLastRun(task: ScheduledTaskView): string {
  if (!task.lastRunAtMs) return "尚未运行";
  const date = new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(task.lastRunAtMs);
  if (task.lastRunState === "running") return "运行中";
  if (task.lastRunState === "failed") return `${date} · 失败`;
  return `${date} · 已完成`;
}

function readableError(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  try {
    const value = JSON.stringify(reason);
    return value && value !== "{}" ? value : "操作失败";
  } catch {
    return "操作失败";
  }
}

export function ScheduledTasksPage() {
  const [tasks, setTasks] = useState<ScheduledTaskView[]>([]);
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [workspace, setWorkspace] = useState<WorkspaceState | null>(null);
  const [query, setQuery] = useState("");
  const [viewMode, setViewMode] = useState<ViewMode>("cards");
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState("");
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<ScheduledTaskView | null>(null);
  const [form, setForm] = useState<FormState>(() => taskToForm(null));
  const [saving, setSaving] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [bulkMode, setBulkMode] = useState(false);

  const load = async (quiet = false) => {
    if (quiet) setRefreshing(true);
    else setLoading(true);
    try {
      const [nextTasks, nextThreads, nextWorkspace] = await Promise.all([
        listScheduledTasks(),
        listThreads(),
        getWorkspaceState(),
      ]);
      setTasks(nextTasks);
      setThreads(nextThreads.filter((thread) => !thread.archived));
      setWorkspace(nextWorkspace);
      setError("");
      setSelectedIds((current) => new Set([...current].filter((id) => nextTasks.some((task) => task.id === id))));
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  };

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(true), 10_000);
    return () => window.clearInterval(timer);
  }, []);

  const filteredTasks = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return tasks;
    return tasks.filter((task) => `${task.name} ${task.prompt} ${task.workspacePath}`.toLowerCase().includes(normalized));
  }, [query, tasks]);

  const enabledCount = tasks.filter((task) => task.enabled).length;
  const runningCount = tasks.filter((task) => task.lastRunState === "running").length;
  const recentRuns = tasks.filter((task) => task.lastRunAtMs && Date.now() - task.lastRunAtMs < 24 * 60 * 60 * 1000).length;
  const nextTask = tasks.filter((task) => task.enabled && task.nextRunAtMs).sort((left, right) => (left.nextRunAtMs ?? Infinity) - (right.nextRunAtMs ?? Infinity))[0];

  function openCreate() {
    setEditing(null);
    setForm(taskToForm(null));
    setError("");
    setModalOpen(true);
  }

  function openEdit(task: ScheduledTaskView) {
    setEditing(task);
    setForm(taskToForm(task));
    setError("");
    setModalOpen(true);
  }

  function updateForm(patch: Partial<FormState>) {
    setForm((current) => ({ ...current, ...patch }));
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    setError("");
    try {
      const schedule: ScheduledTaskSchedule = form.kind === "once"
        ? { kind: "once", atMs: new Date(form.onceAt).getTime() }
        : {
            kind: form.kind,
            hour: Number(form.hour),
            minute: Number(form.minute),
            weekday: form.kind === "weekly" ? Number(form.weekday) : null,
          };
      await upsertScheduledTask({
        id: editing?.id ?? null,
        name: form.name,
        schedule,
        prompt: form.prompt,
        mode: form.mode,
        threadId: form.mode === "thread" ? form.threadId || null : null,
        workspacePath: workspace?.current.path ?? null,
        enabled: form.enabled,
      });
      setModalOpen(false);
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setSaving(false);
    }
  }

  async function toggle(task: ScheduledTaskView) {
    try {
      await setScheduledTaskEnabled(task.id, !task.enabled);
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    }
  }

  async function trigger(task: ScheduledTaskView) {
    try {
      await triggerScheduledTask(task.id);
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    }
  }

  async function remove(task: ScheduledTaskView) {
    if (!window.confirm(`删除定时任务“${task.name}”？`)) return;
    try {
      await deleteScheduledTask(task.id);
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    }
  }

  async function bulkToggle(enabled: boolean) {
    try {
      await Promise.all([...selectedIds].map((id) => setScheduledTaskEnabled(id, enabled)));
      setSelectedIds(new Set());
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    }
  }

  async function bulkDelete() {
    if (!selectedIds.size || !window.confirm(`删除选中的 ${selectedIds.size} 个定时任务？`)) return;
    try {
      await Promise.all([...selectedIds].map((id) => deleteScheduledTask(id)));
      setSelectedIds(new Set());
      await load(true);
    } catch (reason) {
      setError(readableError(reason));
    }
  }

  function toggleSelected(taskId: string) {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });
  }

  function taskThreadLabel(task: ScheduledTaskView): string {
    if (task.mode === "background") return "后台新会话";
    return threads.find((thread) => thread.id === task.threadId)?.title ?? "指定会话";
  }

  return (
    <div className="scheduled-page">
      <header className="scheduled-page-header">
        <div className="scheduled-heading">
          <div className="scheduled-eyebrow"><CalendarClock size={14} /> 自动化工作台</div>
          <h1>定时任务</h1>
          <p>让重复的检查、整理和回顾按节奏自动运行。</p>
        </div>
        <div className="scheduled-header-actions">
          <span className="scheduled-local-time"><Clock3 size={14} /> {Intl.DateTimeFormat().resolvedOptions().timeZone}</span>
          <button className="scheduled-primary-button" type="button" onClick={openCreate}><Plus size={16} /> 添加自动化</button>
        </div>
      </header>

      <div className="scheduled-toolbar">
        <label className="scheduled-search"><Search size={15} /><input aria-label="搜索定时任务" placeholder="搜索任务、提示词或项目" value={query} onChange={(event) => setQuery(event.target.value)} />{query && <button type="button" aria-label="清除搜索" title="清除搜索" onClick={() => setQuery("")}><X size={13} /></button>}</label>
        <div className="scheduled-toolbar-actions">
          <button className="scheduled-quiet-button" type="button" onClick={() => void load(true)} disabled={refreshing} title="刷新任务列表"><RefreshCw className={refreshing ? "spin" : undefined} size={15} /><span>刷新</span></button>
          <button className={bulkMode ? "scheduled-quiet-button is-active" : "scheduled-quiet-button"} type="button" onClick={() => { setBulkMode((value) => !value); setSelectedIds(new Set()); }}><Settings2 size={15} /><span>批量管理</span></button>
          <button className="scheduled-quiet-button" type="button" onClick={() => setViewMode(viewMode === "cards" ? "table" : "cards")} title={viewMode === "cards" ? "切换到表格视图" : "切换到卡片视图"}>{viewMode === "cards" ? <List size={15} /> : <Grid2X2 size={15} />}<span>{viewMode === "cards" ? "表格" : "卡片"}</span></button>
        </div>
      </div>

      <section className="scheduled-overview" aria-label="运行概览">
        <div className="scheduled-overview-mark"><Zap size={20} /></div>
        <div className="scheduled-overview-copy"><strong>{runningCount ? "自动化正在运行" : enabledCount ? "自动化已就绪" : "等待你的第一项自动化"}</strong><span>{enabledCount} 个任务已启用 · 共 {tasks.length} 个任务</span></div>
        <div className="scheduled-overview-stats">
          <span><small>下一次运行</small><strong>{nextTask ? relativeTime(nextTask.nextRunAtMs) : "暂无"}</strong></span>
          <span><small>已启用</small><strong>{enabledCount} / {tasks.length}</strong></span>
          <span><small>近 24 小时</small><strong>{recentRuns}</strong></span>
        </div>
      </section>

      {bulkMode && (
        <div className="scheduled-bulk-bar">
          <label><input type="checkbox" checked={Boolean(filteredTasks.length && filteredTasks.every((task) => selectedIds.has(task.id)))} onChange={(event) => setSelectedIds(event.target.checked ? new Set(filteredTasks.map((task) => task.id)) : new Set())} /> 全选当前结果</label>
          <span>{selectedIds.size ? `已选 ${selectedIds.size} 项` : "选择任务后批量处理"}</span>
          <div><button type="button" disabled={!selectedIds.size} onClick={() => void bulkToggle(true)}><Check size={14} /> 启用</button><button type="button" disabled={!selectedIds.size} onClick={() => void bulkToggle(false)}><Square size={14} /> 停用</button><button className="danger" type="button" disabled={!selectedIds.size} onClick={() => void bulkDelete()}><Trash2 size={14} /> 删除</button></div>
        </div>
      )}

      {error && <div className="scheduled-error" role="alert"><AlertCircle size={16} /><span>{error}</span><button type="button" aria-label="关闭错误" title="关闭" onClick={() => setError("")}><X size={14} /></button></div>}

      <div className="scheduled-list-heading"><div><h2>所有定时任务 <span>{filteredTasks.length}</span></h2><p>{query ? `正在显示与“${query}”匹配的结果` : "按下一次运行时间排序"}</p></div></div>

      {loading ? (
        <div className="scheduled-empty"><Loader2 className="spin" size={22} /><span>正在读取任务</span></div>
      ) : filteredTasks.length === 0 ? (
        <div className="scheduled-empty scheduled-empty--quiet"><TimerReset size={28} /><strong>{query ? "没有匹配的任务" : "还没有定时任务"}</strong><span>{query ? "试试其他关键词。" : "创建一项自动化，让 k-Coder 在你不在电脑前时继续工作。"}</span>{!query && <button className="scheduled-primary-button" type="button" onClick={openCreate}><Plus size={15} /> 创建第一项任务</button>}</div>
      ) : viewMode === "cards" ? (
        <div className="scheduled-task-grid">
          {filteredTasks.map((task) => (
            <article className={`scheduled-task-card ${!task.enabled ? "is-disabled" : ""}`} key={task.id}>
              <div className="scheduled-task-card-head">
                {bulkMode && <input className="scheduled-task-check" type="checkbox" aria-label={`选择 ${task.name}`} checked={selectedIds.has(task.id)} onChange={() => toggleSelected(task.id)} />}
                <span className={`scheduled-status-dot ${task.lastRunState === "running" ? "is-running" : task.enabled ? "is-enabled" : "is-disabled"}`} />
                <div className="scheduled-task-title"><strong>{task.name}</strong><span>{task.enabled ? "已启用" : "已停用"}</span></div>
                <button className="scheduled-icon-button" type="button" aria-label={`编辑 ${task.name}`} title="编辑任务" onClick={() => openEdit(task)}><Pencil size={15} /></button>
              </div>
              <div className="scheduled-task-meta"><span><Clock3 size={14} />{formatSchedule(task.schedule)}</span><strong>{relativeTime(task.nextRunAtMs)}</strong></div>
              <p className="scheduled-task-prompt">{task.prompt}</p>
              <div className="scheduled-task-target"><span><Folder size={13} />{task.workspacePath.split(/[\\/]/).pop() || task.workspacePath}</span><span><MessageSquare size={13} />{taskThreadLabel(task)}</span></div>
              {task.lastRunState === "failed" && task.lastError && <div className="scheduled-task-failure"><AlertCircle size={13} />{task.lastError}</div>}
              <footer className="scheduled-task-actions"><span className={task.lastRunState === "failed" ? "is-failed" : ""}>{formatLastRun(task)}</span><div><button type="button" aria-label={`立即运行 ${task.name}`} title="立即运行" onClick={() => void trigger(task)} disabled={task.lastRunState === "running"}><Play size={14} /></button><button type="button" aria-label={task.enabled ? `停用 ${task.name}` : `启用 ${task.name}`} title={task.enabled ? "停用" : "启用"} onClick={() => void toggle(task)}>{task.enabled ? <Square size={14} /> : <Check size={14} />}</button><button type="button" aria-label={`删除 ${task.name}`} title="删除任务" onClick={() => void remove(task)}><Trash2 size={14} /></button></div></footer>
            </article>
          ))}
        </div>
      ) : (
        <div className="scheduled-table-wrap"><table className="scheduled-table"><thead><tr><th>任务</th><th>计划</th><th>目标</th><th>下一次</th><th>状态</th><th aria-label="操作" /></tr></thead><tbody>{filteredTasks.map((task) => <tr key={task.id}><td><div className="scheduled-table-name">{bulkMode && <input className="scheduled-table-check" type="checkbox" aria-label={`选择 ${task.name}`} checked={selectedIds.has(task.id)} onChange={() => toggleSelected(task.id)} />}<span className={`scheduled-status-dot ${task.enabled ? "is-enabled" : "is-disabled"}`} /><strong>{task.name}</strong></div><small>{task.prompt}</small></td><td>{formatSchedule(task.schedule)}</td><td>{taskThreadLabel(task)}</td><td>{relativeTime(task.nextRunAtMs)}</td><td><span className={`scheduled-table-state ${task.enabled ? "is-enabled" : "is-disabled"}`}>{task.enabled ? "已启用" : "已停用"}</span></td><td><div className="scheduled-table-actions"><button type="button" title="立即运行" aria-label={`立即运行 ${task.name}`} onClick={() => void trigger(task)} disabled={task.lastRunState === "running"}><Play size={14} /></button><button type="button" title={task.enabled ? "停用" : "启用"} aria-label={task.enabled ? `停用 ${task.name}` : `启用 ${task.name}`} onClick={() => void toggle(task)}>{task.enabled ? <Square size={14} /> : <Check size={14} />}</button><button type="button" title="编辑" aria-label={`编辑 ${task.name}`} onClick={() => openEdit(task)}><Pencil size={14} /></button><button type="button" title="删除" aria-label={`删除 ${task.name}`} onClick={() => void remove(task)}><Trash2 size={14} /></button></div></td></tr>)}</tbody></table></div>
      )}

      {modalOpen && createPortal(
        <div className="scheduled-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !saving) setModalOpen(false); }}>
          <section className="scheduled-modal" role="dialog" aria-modal="true" aria-labelledby="scheduled-modal-title">
            <header className="scheduled-modal-header"><div><span>新建定时任务</span><h2 id="scheduled-modal-title">{editing ? "编辑任务" : "创建任务"}</h2></div><button type="button" aria-label="关闭" title="关闭" onClick={() => setModalOpen(false)} disabled={saving}><X size={18} /></button></header>
            <form className="scheduled-form" onSubmit={save}>
              <label><span>名称</span><input autoFocus maxLength={100} value={form.name} onChange={(event) => updateForm({ name: event.target.value })} placeholder="例如：每日整理客户反馈" required /></label>
              <div className="scheduled-form-row"><label><span>计划类型</span><select value={form.kind} onChange={(event) => updateForm({ kind: event.target.value as ScheduledTaskKind })}><option value="daily">每天重复</option><option value="weekly">每周重复</option><option value="once">只运行一次</option></select></label>{form.kind === "weekly" && <label><span>星期</span><select value={form.weekday} onChange={(event) => updateForm({ weekday: event.target.value })}>{WEEKDAYS.map((day, index) => <option key={day} value={index}>{day}</option>)}</select></label>}</div>
              {form.kind === "once" ? <label><span>具体时间</span><input type="datetime-local" value={form.onceAt} onChange={(event) => updateForm({ onceAt: event.target.value })} required /></label> : <label><span>具体时间</span><div className="scheduled-time-input"><input aria-label="小时" type="number" min={0} max={23} value={form.hour} onChange={(event) => updateForm({ hour: event.target.value })} required /><b>:</b><input aria-label="分钟" type="number" min={0} max={59} value={form.minute} onChange={(event) => updateForm({ minute: event.target.value })} required /></div></label>}
              <label><span>任务模式</span><select value={form.mode} onChange={(event) => updateForm({ mode: event.target.value as ScheduledTaskMode })}><option value="background">后台智能体任务（新建会话）</option><option value="thread">继续已有会话</option></select><small>{form.mode === "background" ? "每次运行都会建立独立的项目会话，方便查看完整记录。" : "任务会把提示词发送到所选会话，并保留上下文。"}</small></label>
              {form.mode === "thread" && <label><span>目标会话</span><select value={form.threadId} onChange={(event) => updateForm({ threadId: event.target.value })} required><option value="">选择一个会话</option>{threads.map((thread) => <option key={thread.id} value={thread.id}>{thread.title}</option>)}</select></label>}
              <label><span>项目空间</span><div className="scheduled-project-readonly"><Folder size={15} /><span>{workspace?.current.name ?? "当前项目"}</span><small>{workspace?.current.path ?? "当前工作区"}</small></div><small>任务只会在保存时绑定的项目空间中运行；切换项目后会等待你切回对应空间。</small></label>
              <label><span>任务提示词</span><textarea maxLength={16_000} rows={5} value={form.prompt} onChange={(event) => updateForm({ prompt: event.target.value })} placeholder="例如：整理今天收到的客户反馈，归纳主要问题并给出处理建议" required /><small className="scheduled-character-count">{form.prompt.length.toLocaleString()} / 16,000</small></label>
              <details className="scheduled-advanced"><summary><ChevronDown size={14} /> 更多运行设置</summary><label className="scheduled-toggle"><input type="checkbox" checked={form.enabled} onChange={(event) => updateForm({ enabled: event.target.checked })} /><span><strong>创建后立即启用</strong><small>关闭后只保存任务，不会自动运行。</small></span></label></details>
              <footer className="scheduled-modal-actions"><button className="scheduled-secondary-button" type="button" onClick={() => setModalOpen(false)} disabled={saving}>取消</button><button className="scheduled-primary-button" type="submit" disabled={saving}>{saving ? <Loader2 className="spin" size={15} /> : <CheckCircle2 size={15} />}{editing ? "保存更改" : "创建任务"}</button></footer>
            </form>
          </section>
        </div>,
        document.body,
      )}
    </div>
  );
}
