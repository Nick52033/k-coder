import { useEffect, useMemo, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Copy,
  FilePlus2,
  Plus,
  Save,
  Send,
  Trash2,
  Workflow,
  X,
} from "lucide-react";
import {
  deleteWorkflowDraft,
  duplicateWorkflowAsDraft,
  errorMessage,
  getExtensionOverview,
  listManagedWorkflows,
  publishWorkflowDraft,
  saveWorkflowDraft,
} from "../api/runtime";
import type {
  ExtensionOverview,
  WorkflowDefinitionRecord,
  WorkflowDraftRequest,
  WorkflowNodeDraft,
} from "../types/runtime";
import "./WorkflowSettingsPage.css";

interface WorkflowSettingsPageProps {
  threadId: string | null;
  projectAvailable: boolean;
  onPublished: () => Promise<void>;
}

function newNode(existingIds: string[] = []): WorkflowNodeDraft {
  const occupied = new Set(existingIds);
  let next = existingIds.length + 1;
  while (occupied.has(`step-${next}`)) next += 1;
  return {
    id: `step-${next}`,
    title: "",
    description: "",
    instructions: "",
    completionCriteria: "",
    skillIds: [],
  };
}

function editorFromRecord(record: WorkflowDefinitionRecord): WorkflowDraftRequest {
  return {
    workflowId: record.id,
    expectedRevision: record.revision,
    name: record.name,
    description: record.description,
    nodes: record.nodes.map((node) => ({ ...node, skillIds: [...node.skillIds] })),
  };
}

function isCompleteDraft(draft: WorkflowDraftRequest): boolean {
  return Boolean(
    draft.name.trim()
    && draft.description.trim()
    && draft.nodes.length > 0
    && draft.nodes.every((node) =>
      node.title.trim()
      && node.description.trim()
      && node.instructions.trim()
      && node.completionCriteria.trim(),
    ),
  );
}

export function WorkflowSettingsPage({ threadId, projectAvailable, onPublished }: WorkflowSettingsPageProps) {
  const [records, setRecords] = useState<WorkflowDefinitionRecord[]>([]);
  const [overview, setOverview] = useState<ExtensionOverview | null>(null);
  const [editor, setEditor] = useState<WorkflowDraftRequest | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const selectedRecord = records.find((record) => record.id === selectedId) ?? null;
  const readOnly = selectedRecord?.status === "published";
  const availableSkills = useMemo(
    () => (overview?.skills ?? []).filter((skill) => skill.enabled && !skill.managedByRobot),
    [overview],
  );

  useEffect(() => {
    let disposed = false;
    setRecords([]);
    setOverview(null);
    setEditor(null);
    setSelectedId(null);
    setNotice("");
    setError("");
    if (!threadId || !projectAvailable) {
      setLoading(false);
      return undefined;
    }
    setLoading(true);
    void Promise.all([
      listManagedWorkflows(threadId),
      getExtensionOverview(false, threadId),
    ])
      .then(([workflowRecords, extensionOverview]) => {
        if (disposed) return;
        setRecords(workflowRecords);
        setOverview(extensionOverview);
      })
      .catch((reason) => {
        if (!disposed) setError(errorMessage(reason));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => { disposed = true; };
  }, [threadId, projectAvailable]);

  function openNewDraft() {
    setSelectedId(null);
    setEditor({ name: "", description: "", nodes: [newNode()] });
    setError("");
    setNotice("");
  }

  function openRecord(record: WorkflowDefinitionRecord) {
    setSelectedId(record.id);
    setEditor(editorFromRecord(record));
    setError("");
    setNotice("");
  }

  function updateNode(index: number, update: (node: WorkflowNodeDraft) => WorkflowNodeDraft) {
    setEditor((current) => current ? {
      ...current,
      nodes: current.nodes.map((node, nodeIndex) => nodeIndex === index ? update(node) : node),
    } : current);
  }

  function moveNode(index: number, offset: number) {
    setEditor((current) => {
      if (!current) return current;
      const target = index + offset;
      if (target < 0 || target >= current.nodes.length) return current;
      const nodes = [...current.nodes];
      [nodes[index], nodes[target]] = [nodes[target], nodes[index]];
      return { ...current, nodes };
    });
  }

  async function handleSave() {
    if (!threadId || !editor) return;
    if (!isCompleteDraft(editor)) {
      setError("请填写流程名称、说明，以及每一步的标题、任务说明和完成标准后再保存。");
      return;
    }
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const saved = await saveWorkflowDraft(threadId, editor);
      setRecords((current) => [...current.filter((record) => record.id !== saved.id), saved]);
      setSelectedId(saved.id);
      setEditor(editorFromRecord(saved));
      setNotice("草稿已保存。");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function handlePublish() {
    if (!threadId || !editor?.workflowId || !editor.expectedRevision) return;
    if (!isCompleteDraft(editor)) {
      setError("请先补齐流程名称、说明和每一步内容。");
      return;
    }
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const saved = await saveWorkflowDraft(threadId, editor);
      setRecords((current) => [...current.filter((record) => record.id !== saved.id), saved]);
      setEditor(editorFromRecord(saved));
      const published = await publishWorkflowDraft(
        threadId,
        saved.id,
        saved.revision,
      );
      setRecords((current) => [...current.filter((record) => record.id !== published.id), published]);
      setSelectedId(published.id);
      setEditor(editorFromRecord(published));
      await onPublished();
      setNotice("流程已发布，可在聊天输入框旁的流程选择器中启动。");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function handleDuplicate(record: WorkflowDefinitionRecord) {
    if (!threadId) return;
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const copy = await duplicateWorkflowAsDraft(threadId, record.id);
      setRecords((current) => [...current, copy]);
      openRecord(copy);
      setNotice("已复制为新草稿；已发布流程保持不变。");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  async function handleDeleteDraft(record: WorkflowDefinitionRecord) {
    if (!threadId) return;
    setSaving(true);
    setError("");
    setNotice("");
    try {
      await deleteWorkflowDraft(threadId, record.id, record.revision);
      setRecords((current) => current.filter((item) => item.id !== record.id));
      setSelectedId(null);
      setEditor(null);
      setNotice("草稿已删除。");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  }

  if (!threadId || !projectAvailable) {
    return (
      <section className="settings-page workflow-settings-page" aria-labelledby="workflows-page-title">
        <div className="settings-page-header">
          <div><p className="settings-eyebrow">智能体</p><h3 id="workflows-page-title">Workflows</h3></div>
          <Workflow size={20} aria-hidden="true" />
        </div>
        <p className="settings-page-description">请先选择一个项目会话，再创建和管理该项目的流程。独立会话不支持项目流程。</p>
      </section>
    );
  }

  return (
    <section className="settings-page workflow-settings-page" aria-labelledby="workflows-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">智能体</p>
          <h3 id="workflows-page-title">项目 Workflows</h3>
        </div>
        <button className="primary-button workflow-settings-new" type="button" onClick={openNewDraft} disabled={saving}>
          <FilePlus2 size={15} /> 新建流程
        </button>
      </div>
      <p className="settings-page-description">
        为当前项目创建顺序执行的流程。每一步包含任务说明、完成标准和可选 Skills；发布后即可从聊天输入框旁选择。
      </p>
      {error && <div className="settings-error" role="alert">{error}</div>}
      {notice && <div className="workflow-settings-notice" role="status">{notice}</div>}

      <div className="workflow-settings-layout">
        <aside className="workflow-settings-list" aria-label="项目流程">
          <h4>流程列表</h4>
          {loading ? (
            <p className="workflow-settings-muted">正在加载流程…</p>
          ) : records.length === 0 ? (
            <p className="workflow-settings-muted">还没有项目流程，点击“新建流程”开始。</p>
          ) : records.map((record) => (
            <button
              className={`workflow-settings-item${selectedId === record.id ? " is-selected" : ""}`}
              type="button"
              key={record.id}
              onClick={() => openRecord(record)}
            >
              <span className="workflow-settings-item-title">{record.name}</span>
              <span className={`workflow-settings-status workflow-settings-status--${record.status}`}>
                {record.status === "published" ? "已发布" : "草稿"}
              </span>
              <small>{record.nodes.length} 个步骤</small>
            </button>
          ))}
          {overview?.error && <p className="workflow-settings-muted">Skills 状态暂不可用：{overview.error}</p>}
        </aside>

        <div className="workflow-settings-editor">
          {!editor ? (
            <div className="workflow-settings-empty">
              <Workflow size={24} />
              <strong>选择一个流程，或新建流程</strong>
              <span>流程保存在项目设置中，不会写入项目目录。</span>
            </div>
          ) : (
            <>
              {selectedRecord && (
                <div className="workflow-settings-editor-meta">
                  <span className={`workflow-settings-status workflow-settings-status--${selectedRecord.status}`}>
                    {readOnly ? "已发布，只读" : "草稿"}
                  </span>
                  {readOnly && <button className="quiet-button" type="button" onClick={() => void handleDuplicate(selectedRecord)} disabled={saving}>
                    <Copy size={14} /> 复制为草稿
                  </button>}
                </div>
              )}
              <div className="workflow-settings-form">
                <label>
                  流程名称
                  <input
                    value={editor.name}
                    maxLength={80}
                    placeholder="例如：版本发布检查"
                    disabled={readOnly || saving}
                    onChange={(event) => setEditor({ ...editor, name: event.target.value })}
                  />
                </label>
                <label>
                  流程说明
                  <textarea
                    value={editor.description}
                    maxLength={2000}
                    rows={2}
                    placeholder="说明流程适用的任务和目标。"
                    disabled={readOnly || saving}
                    onChange={(event) => setEditor({ ...editor, description: event.target.value })}
                  />
                </label>
                <div className="workflow-settings-steps-heading">
                  <div><strong>执行步骤</strong><small>按顺序完成；运行时由主智能体逐步推进。</small></div>
                  {!readOnly && <button className="quiet-button" type="button" onClick={() => setEditor({ ...editor, nodes: [...editor.nodes, newNode(editor.nodes.map((node) => node.id))] })} disabled={saving || editor.nodes.length >= 20}>
                    <Plus size={14} /> 添加步骤
                  </button>}
                </div>
                <div className="workflow-settings-steps">
                  {editor.nodes.map((node, index) => (
                    <article className="workflow-settings-step" key={`${node.id}-${index}`}>
                      <header>
                        <span className="workflow-settings-step-number">{index + 1}</span>
                        <strong>步骤 {index + 1}</strong>
                        {!readOnly && <div className="workflow-settings-step-order">
                          <button type="button" aria-label={`步骤 ${index + 1} 上移`} title="上移" disabled={saving || index === 0} onClick={() => moveNode(index, -1)}><ArrowUp size={14} /></button>
                          <button type="button" aria-label={`步骤 ${index + 1} 下移`} title="下移" disabled={saving || index === editor.nodes.length - 1} onClick={() => moveNode(index, 1)}><ArrowDown size={14} /></button>
                          <button type="button" aria-label={`删除步骤 ${index + 1}`} title="删除步骤" disabled={saving || editor.nodes.length === 1} onClick={() => setEditor({ ...editor, nodes: editor.nodes.filter((_, nodeIndex) => nodeIndex !== index) })}><X size={14} /></button>
                        </div>}
                      </header>
                      <div className="workflow-settings-form-grid">
                        <label>
                          标题
                          <input value={node.title} maxLength={120} placeholder="例如：检查变更" disabled={readOnly || saving} onChange={(event) => updateNode(index, (current) => ({ ...current, title: event.target.value }))} />
                        </label>
                        <label>
                          步骤摘要
                          <input value={node.description} maxLength={2000} placeholder="这一阶段要完成什么？" disabled={readOnly || saving} onChange={(event) => updateNode(index, (current) => ({ ...current, description: event.target.value }))} />
                        </label>
                      </div>
                      <label className="workflow-settings-label">
                        任务说明
                        <textarea value={node.instructions} maxLength={12000} rows={3} placeholder="告诉智能体本步骤要做什么、遵守哪些边界。" disabled={readOnly || saving} onChange={(event) => updateNode(index, (current) => ({ ...current, instructions: event.target.value }))} />
                      </label>
                      <label className="workflow-settings-label">
                        完成标准
                        <textarea value={node.completionCriteria} maxLength={2000} rows={2} placeholder="明确本步骤达到什么结果后才能进入下一步。" disabled={readOnly || saving} onChange={(event) => updateNode(index, (current) => ({ ...current, completionCriteria: event.target.value }))} />
                      </label>
                      <fieldset className="workflow-settings-skills" disabled={readOnly || saving}>
                        <legend>此步骤可用的 Skills</legend>
                        {availableSkills.length === 0 ? (
                          <span className="workflow-settings-muted">当前项目没有已启用的普通 Skill；此项可以留空。</span>
                        ) : (
                          <div className="workflow-settings-skill-list">
                            {availableSkills.map((skill) => (
                              <label key={skill.name}>
                                <input
                                  type="checkbox"
                                  checked={node.skillIds.includes(skill.name)}
                                  onChange={(event) => updateNode(index, (current) => ({
                                    ...current,
                                    skillIds: event.target.checked
                                      ? [...current.skillIds, skill.name]
                                      : current.skillIds.filter((id) => id !== skill.name),
                                  }))}
                                />
                                <span><strong>{skill.name}</strong><small>{skill.description}</small></span>
                              </label>
                            ))}
                          </div>
                        )}
                      </fieldset>
                    </article>
                  ))}
                </div>
              </div>
              {!readOnly && (
                <div className="workflow-settings-actions">
                  <button className="quiet-button workflow-settings-delete" type="button" disabled={saving || !selectedRecord} onClick={() => selectedRecord && void handleDeleteDraft(selectedRecord)}>
                    <Trash2 size={14} /> 删除草稿
                  </button>
                  <div>
                    <button className="quiet-button" type="button" onClick={() => setEditor(null)} disabled={saving}>取消</button>
                    <button className="quiet-button" type="button" onClick={() => void handleSave()} disabled={saving}>
                      <Save size={14} /> 保存草稿
                    </button>
                    {selectedRecord && <button className="primary-button" type="button" onClick={() => void handlePublish()} disabled={saving || !isCompleteDraft(editor)}>
                      <Send size={14} /> 发布流程
                    </button>}
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  );
}
