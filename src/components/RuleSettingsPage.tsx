import { FormEvent, useEffect, useMemo, useState } from "react";
import {
  FileText,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import {
  deleteUserRule,
  getExtensionOverview,
  getUserRules,
  saveUserRule,
} from "../api/runtime";
import type {
  ExtensionOverview,
  SaveUserRuleRequest,
  UserRule,
  UserRulesView,
} from "../types/runtime";
import { useToast } from "./Toast";
import "./RuleSettingsPage.css";

const MAX_RULE_TITLE_CHARACTERS = 80;
const MAX_RULE_CONTENT_BYTES = 16 * 1024;

interface RuleEditorState extends SaveUserRuleRequest {
  originalTitle: string;
  originalContent: string;
}

function errorMessage(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return "规则操作失败";
}

function contentBytes(value: string) {
  return new TextEncoder().encode(value).length;
}

function visibleCharacterCount(value: string) {
  return Array.from(value.trim()).length;
}

function contentPreview(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

function editorFor(rule?: UserRule): RuleEditorState {
  return {
    id: rule?.id ?? null,
    title: rule?.title ?? "",
    content: rule?.content ?? "",
    originalTitle: rule?.title ?? "",
    originalContent: rule?.content ?? "",
  };
}

function formatUpdatedTime(timestampMs: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestampMs));
}

export function RuleSettingsPage() {
  const toast = useToast();
  const [view, setView] = useState<UserRulesView | null>(null);
  const [overview, setOverview] = useState<ExtensionOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editor, setEditor] = useState<RuleEditorState | null>(null);
  const [pendingDelete, setPendingDelete] = useState<UserRule | null>(null);

  async function load(refresh: boolean) {
    setLoading(true);
    setError("");
    try {
      const nextView = await getUserRules(refresh);
      setView(nextView);
      setOverview(await getExtensionOverview(false));
    } catch (loadError) {
      setError(errorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load(false);
  }, []);

  const bytes = useMemo(() => contentBytes(editor?.content ?? ""), [editor?.content]);
  const titleCharacters = useMemo(
    () => visibleCharacterCount(editor?.title ?? ""),
    [editor?.title],
  );
  const editorDirty = Boolean(
    editor
      && (editor.title !== editor.originalTitle || editor.content !== editor.originalContent),
  );
  const editorValid = Boolean(
    editor
      && titleCharacters > 0
      && titleCharacters <= MAX_RULE_TITLE_CHARACTERS
      && editor.content.trim()
      && bytes <= MAX_RULE_CONTENT_BYTES,
  );

  async function refreshOverview() {
    setOverview(await getExtensionOverview(false));
  }

  async function handleSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!editor || !editorValid) return;
    setBusy(true);
    setError("");
    try {
      const nextView = await saveUserRule({
        id: editor.id,
        title: editor.title.trim(),
        content: editor.content,
      });
      setView(nextView);
      setEditor(null);
      await refreshOverview();
      toast.success(editor.id ? "规则已更新" : "规则已创建");
    } catch (saveError) {
      setError(errorMessage(saveError));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!pendingDelete) return;
    const rule = pendingDelete;
    setBusy(true);
    setError("");
    try {
      const nextView = await deleteUserRule(rule.id);
      setView(nextView);
      setPendingDelete(null);
      if (editor?.id === rule.id) setEditor(null);
      await refreshOverview();
      toast.success("规则已删除");
    } catch (deleteError) {
      setError(errorMessage(deleteError));
    } finally {
      setBusy(false);
    }
  }

  const rules = view?.rules ?? [];
  const sourceRules = overview?.instructions.filter((source) => source.scope === "user_rule") ?? [];

  return (
    <section className="settings-page rule-settings-page" aria-labelledby="rules-page-title">
      <div className="settings-page-header rule-page-header">
        <div>
          <p className="settings-eyebrow">知识与规则</p>
          <h3 id="rules-page-title">自定义规则</h3>
        </div>
        <div className="rule-page-actions">
          <button
            className="rule-icon-button"
            type="button"
            aria-label="刷新规则"
            title="刷新规则"
            disabled={loading || busy}
            onClick={() => void load(true)}
          >
            {loading ? <LoaderCircle className="rule-spin" size={16} /> : <RefreshCw size={16} />}
          </button>
          <button
            className="rule-new-button"
            type="button"
            disabled={loading || busy || editor !== null}
            onClick={() => setEditor(editorFor())}
          >
            <Plus size={15} />
            <span>新建</span>
          </button>
        </div>
      </div>

      <div className="rule-file-path" title={view?.path ?? ""}>
        <FileText size={14} />
        <span>{view?.path ?? "正在读取规则配置..."}</span>
        <strong>{rules.length}</strong>
      </div>

      {(error || view?.error || overview?.error) && (
        <div className="settings-error" role="alert">
          {error || view?.error || overview?.error}
        </div>
      )}

      {editor ? (
        <form className="rule-editor" onSubmit={(event) => void handleSave(event)}>
          <div className="rule-editor-heading">
            <div>
              <strong>{editor.id ? "编辑规则" : "新建规则"}</strong>
              <span>用户规则</span>
            </div>
            <button
              className="rule-icon-button"
              type="button"
              aria-label="取消编辑"
              title="取消编辑"
              disabled={busy}
              onClick={() => setEditor(null)}
            >
              <X size={16} />
            </button>
          </div>
          <label className="rule-field">
            <span>名称</span>
            <input
              autoFocus
              value={editor.title}
              aria-invalid={titleCharacters > MAX_RULE_TITLE_CHARACTERS}
              onChange={(event) => setEditor({ ...editor, title: event.currentTarget.value })}
            />
            <small className={titleCharacters > MAX_RULE_TITLE_CHARACTERS ? "rule-limit--invalid" : ""}>
              {titleCharacters}/{MAX_RULE_TITLE_CHARACTERS}
            </small>
          </label>
          <label className="rule-field rule-content-field">
            <span>规则内容</span>
            <textarea
              value={editor.content}
              aria-invalid={bytes > MAX_RULE_CONTENT_BYTES}
              onChange={(event) => setEditor({ ...editor, content: event.currentTarget.value })}
            />
            <small className={bytes > MAX_RULE_CONTENT_BYTES ? "rule-limit--invalid" : ""}>
              {bytes.toLocaleString()}/{MAX_RULE_CONTENT_BYTES.toLocaleString()} B
            </small>
          </label>
          <div className="rule-editor-actions">
            <button
              className="secondary-button"
              type="button"
              disabled={busy}
              onClick={() => setEditor(null)}
            >
              取消
            </button>
            <button
              className="primary-button settings-command"
              type="submit"
              disabled={busy || !editorValid || !editorDirty}
            >
              {busy ? <LoaderCircle className="rule-spin" size={15} /> : <Save size={15} />}
              保存
            </button>
          </div>
        </form>
      ) : !loading && rules.length === 0 ? (
        <div className="rule-empty">
          <FileText size={21} />
          <span>暂无自定义规则</span>
          <button className="secondary-button settings-command" type="button" onClick={() => setEditor(editorFor())}>
            <Plus size={15} />
            新建规则
          </button>
        </div>
      ) : (
        <div className="rule-list" aria-label="自定义规则列表">
          {rules.map((rule) => (
            <article className="rule-row" key={rule.id}>
              <div className="rule-row-icon" aria-hidden="true"><FileText size={15} /></div>
              <button className="rule-row-main" type="button" onClick={() => setEditor(editorFor(rule))}>
                <strong>{rule.title}</strong>
                <span>{contentPreview(rule.content)}</span>
                <small>用户规则 · {formatUpdatedTime(rule.updatedAtMs)}</small>
              </button>
              <div className="rule-row-actions">
                <button
                  type="button"
                  aria-label={`编辑 ${rule.title}`}
                  title="编辑规则"
                  disabled={busy}
                  onClick={() => setEditor(editorFor(rule))}
                >
                  <Pencil size={15} />
                </button>
                <button
                  className="rule-delete-button"
                  type="button"
                  aria-label={`删除 ${rule.title}`}
                  title="删除规则"
                  disabled={busy}
                  onClick={() => setPendingDelete(rule)}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            </article>
          ))}
        </div>
      )}

      <details className="rule-runtime-details">
        <summary>运行时来源与审计</summary>
        <div className="extension-section-label">已编译用户规则</div>
        <div className="instruction-list">
          {sourceRules.length ? sourceRules.map((source) => (
            <div key={source.path}>
              <span>{source.priority}</span>
              <div><strong>用户规则</strong><small title={source.path}>{source.path}</small></div>
              <em>{source.bytes} B</em>
            </div>
          )) : <div className="extension-empty">暂无已编译用户规则</div>}
        </div>
        <div className="extension-section-label">扩展审计</div>
        <div className="audit-list">
          {overview?.audit.length ? overview.audit.slice().reverse().slice(0, 20).map((record, index) => (
            <div key={`${record.timestampMs}-${index}`}>
              <span className={record.success ? "audit-ok" : "audit-failed"}>{record.success ? "成功" : "失败"}</span>
              <div><strong>{record.event}</strong><small>{record.kind}/{record.id} · {record.detail}</small></div>
              <time>{new Date(record.timestampMs).toLocaleString()}</time>
            </div>
          )) : <div className="extension-empty">暂无扩展审计记录</div>}
        </div>
      </details>

      {pendingDelete && (
        <div
          className="rule-confirm-backdrop"
          role="presentation"
          onKeyDown={(event) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            setPendingDelete(null);
          }}
        >
          <section
            className="rule-confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="rule-delete-title"
          >
            <div>
              <h4 id="rule-delete-title">删除规则</h4>
              <strong>{pendingDelete.title}</strong>
            </div>
            <div className="rule-confirm-actions">
              <button
                className="secondary-button"
                type="button"
                autoFocus
                disabled={busy}
                onClick={() => setPendingDelete(null)}
              >
                取消
              </button>
              <button className="danger-button" type="button" disabled={busy} onClick={() => void handleDelete()}>
                {busy ? <LoaderCircle className="rule-spin" size={15} /> : <Trash2 size={15} />}
                删除
              </button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}
