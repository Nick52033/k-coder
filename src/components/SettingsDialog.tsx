import { FormEvent, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getExtensionOverview,
  getWorkflowSkillReadiness,
  getUsageSummary,
  setExtensionEnabled,
  testProviderConnection,
  getAdvancedMetrics,
  getBrowserSettings,
  listBrowserArtifacts,
  listBrowserAudit,
  runRegressionEvaluation,
  saveBrowserSettings,
  getKnowledgeSettings,
  setKnowledgeEnabled,
  listKnowledgeCollections,
  upsertKnowledgeCollection,
  deleteKnowledgeCollection,
  addKnowledgeSource,
  listKnowledgeSources,
  deleteKnowledgeSource,
  refreshKnowledgeSource,
  cancelKnowledgeIndexJob,
  getEmbeddingSettings,
  setEmbeddingSettings,
  setEmbeddingApiKey,
  deleteEmbeddingApiKey,
  testEmbeddingConnection,
  getWorkspaceState,
} from "../api/runtime";
import { useToast } from "./Toast";
import {
  BarChart3,
  Bot,
  Boxes,
  Check,
  KeyRound,
  Library,
  Network,
  Palette,
  Puzzle,
  RefreshCw,
  Save,
  ServerCog,
  Settings,
  ShieldCheck,
  Sparkles,
  Sun,
  Workflow,
  X,
  Plus,
  MoreVertical,
  Edit3,
  Trash2,
  Globe2,
  PlayCircle,
  Target,
  Pause,
  Play,
  CircleCheck,
  CircleDollarSign,
  Clock3,
  Flag,
  CircleStop,
  FolderOpen,
  FileText,
  CheckCircle2,
  AlertCircle,
  Cloud,
  Flame,
  Monitor,
  Moon,
  Terminal,
  Waves,
  ChevronDown,
  ChevronRight,
  Search,
  LockKeyhole,
} from "lucide-react";
import type {
  ProviderConfigView,
  ProviderModelConfig,
  ProviderTransport,
  SaveProviderConfigRequest,
  UsageSummary,
  ExtensionOverview,
  ProviderEndpointConfig,
  BrowserArtifact,
  BrowserAuditEvent,
  BrowserSettings,
  EvaluationReport,
  MetricsSnapshot,
  GoalState,
  GoalView,
  WorkflowDefinitionView,
  WorkflowRunView,
  WorkflowSkillReadinessView,
  SkillCategory,
  KnowledgeSettings,
  EmbeddingSettings,
  KnowledgeCollection,
  KnowledgeSource,
} from "../types/runtime";
import { toUserFacingPath, workspacePathKey } from "../lib/path";
import { McpSettingsPage } from "./McpSettingsPage";
import { PluginSettingsPage } from "./PluginSettingsPage";
import { RuleSettingsPage } from "./RuleSettingsPage";
import { THEME_OPTIONS, themeLabel, type ThemeId } from "../lib/theme";

const DEFAULT_BASE_URL = "https://api.openai.com/v1";

interface ProviderItem {
  id: string;
  name: string;
  baseUrl: string;
  model: string;
  models: ProviderModelConfig[];
  endpoints: ProviderEndpointConfig[];
  transport: ProviderTransport;
  hasApiKey: boolean;
  isDefault: boolean;
  isDraft: boolean;
}

function providerItemFromView(provider: ProviderConfigView, activeProviderId: string | null): ProviderItem {
  return {
    id: provider.id,
    name: provider.name,
    baseUrl: provider.baseUrl,
    model: provider.model,
    models: provider.models,
    endpoints: provider.endpoints,
    transport: provider.transport,
    hasApiKey: provider.hasApiKey,
    isDefault: provider.id === activeProviderId,
    isDraft: false,
  };
}

interface EditableProviderModel extends ProviderModelConfig {
  key: string;
}

let modelRowSequence = 0;

function editableModel(model?: Partial<ProviderModelConfig>): EditableProviderModel {
  modelRowSequence += 1;
  return {
    key: `provider-model-${modelRowSequence}`,
    id: model?.id ?? "",
    displayName: model?.displayName ?? "",
    contextWindow: model?.contextWindow ?? 200_000,
    maxOutputTokens: model?.maxOutputTokens,
    supportsVision: model?.supportsVision ?? true,
    fallback: model?.fallback ?? false,
  };
}

const transportOptions: Array<{ value: ProviderTransport; label: string }> = [
  { value: "open_ai_chat_completions", label: "OpenAI Chat Completions" },
  { value: "deep_seek_chat_completions", label: "DeepSeek Chat Completions" },
  { value: "open_ai_responses", label: "OpenAI Responses API" },
  { value: "anthropic_messages", label: "Anthropic Messages API" },
  { value: "google_gemini", label: "Google Gemini API" },
];

export type SettingsSection =
  | "providers"
  | "appearance"
  | "usage"
  | "mcp"
  | "plugins"
  | "miniapps"
  | "skills"
  | "robots"
  | "workflows"
  | "knowledge"
  | "rules"
  | "general"
  | "browser"
  | "goal";

interface SettingsDefinition {
  id: SettingsSection;
  label: string;
  group: string;
  icon: typeof ServerCog;
  available: boolean;
}

interface SettingsDialogProps {
  initialSection?: SettingsSection;
  provider: ProviderConfigView | null;
  providers: ProviderConfigView[];
  activeProviderId: string | null;
  activeThreadId: string | null;
  goal: GoalView | null;
  workflows: WorkflowDefinitionView[];
  workflowRun: WorkflowRunView | null;
  error: string;
  themeMode: ThemeId;
  onClose: () => void;
  onSelectTheme: (theme: ThemeId) => void;
  onSaveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
  onActivateProvider: (providerId: string) => Promise<boolean>;
  onDeleteProvider: (providerId: string) => Promise<boolean>;
  onCreateGoal: (objective: string, tokenBudget: number | null, timeBudgetMs: number) => Promise<boolean>;
  onTransitionGoal: (state: GoalState, reason?: string) => Promise<boolean>;
}

const settingsDefinitions: SettingsDefinition[] = [
  { id: "providers", label: "模型供应商", group: "模型与用量", icon: ServerCog, available: true },
  { id: "usage", label: "用量追踪", group: "模型与用量", icon: BarChart3, available: true },
  { id: "mcp", label: "MCP", group: "扩展", icon: Network, available: true },
  { id: "plugins", label: "插件管理", group: "扩展", icon: Puzzle, available: true },
  { id: "miniapps", label: "小程序", group: "扩展", icon: Boxes, available: false },
  { id: "skills", label: "Skills", group: "扩展", icon: Sparkles, available: true },
  { id: "robots", label: "机器人", group: "智能体", icon: Bot, available: true },
  { id: "workflows", label: "Workflows", group: "智能体", icon: Workflow, available: false },
  { id: "knowledge", label: "知识库", group: "知识与规则", icon: Library, available: true },
  { id: "browser", label: "浏览器自动化", group: "智能体", icon: Globe2, available: true },
  { id: "goal", label: "目标与预算", group: "智能体", icon: Target, available: true },
  { id: "rules", label: "Rules", group: "知识与规则", icon: ShieldCheck, available: true },
  { id: "appearance", label: "外观", group: "应用", icon: Palette, available: true },
  { id: "general", label: "通用", group: "应用", icon: Settings, available: false },
];

export function SettingsDialog({
  initialSection = "providers",
  provider,
  providers,
  activeProviderId,
  activeThreadId,
  goal,
  workflows,
  workflowRun,
  error,
  themeMode,
  onClose,
  onSelectTheme,
  onSaveProvider,
  onActivateProvider,
  onDeleteProvider,
  onCreateGoal,
  onTransitionGoal,
}: SettingsDialogProps) {
  const [section, setSection] = useState<SettingsSection>(initialSection);

  useEffect(() => {
    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const activeDefinition = settingsDefinitions.find((item) => item.id === section)!;
  const groups = Array.from(new Set(settingsDefinitions.map((item) => item.group)));

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header className="settings-header">
          <div className="settings-title">
            <Settings size={18} />
            <h2 id="settings-title">设置</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            aria-label="关闭设置"
            title="关闭"
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </header>

        <div className="settings-layout">
          <nav className="settings-navigation" aria-label="设置分类">
            {groups.map((group) => (
              <div className="settings-nav-group" key={group}>
                <div className="settings-nav-label">{group}</div>
                {settingsDefinitions
                  .filter((item) => item.group === group)
                  .map((item) => {
                    const Icon = item.icon;
                    return (
                      <button
                        className={`settings-nav-item ${section === item.id ? "settings-nav-item--active" : ""}`}
                        type="button"
                        key={item.id}
                        onClick={() => setSection(item.id)}
                      >
                        <Icon size={16} />
                        <span>{item.label}</span>
                        {!item.available && <span className="settings-nav-dot" aria-label="待接入" />}
                      </button>
                    );
                  })}
              </div>
            ))}
          </nav>

          <div className="settings-content">
            {section === "providers" ? (
              <ProviderSettingsPage
                provider={provider}
                configuredProviders={providers}
                activeProviderId={activeProviderId}
                error={error}
                onSave={onSaveProvider}
                onActivate={onActivateProvider}
                onDelete={onDeleteProvider}
              />
            ) : section === "appearance" ? (
              <AppearancePage themeMode={themeMode} onSelectTheme={onSelectTheme} />
            ) : section === "usage" ? (
              <UsagePage />
            ) : section === "knowledge" ? (
              <KnowledgePage />
            ) : section === "browser" ? (
              <BrowserPage />
            ) : section === "goal" ? (
              <GoalSettingsPage
                threadId={activeThreadId}
                goal={goal}
                onCreate={onCreateGoal}
                onTransition={onTransitionGoal}
              />
            ) : section === "robots" ? (
              <RobotsPage workflows={workflows} workflowRun={workflowRun} />
            ) : section === "mcp" ? (
              <McpSettingsPage />
            ) : section === "plugins" ? (
              <PluginSettingsPage />
            ) : section === "rules" ? (
              <RuleSettingsPage />
            ) : section === "skills" ? (
              <ExtensionsPage />
            ) : (
              <PendingSection definition={activeDefinition} />
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

interface GoalSettingsPageProps {
  threadId: string | null;
  goal: GoalView | null;
  onCreate: (objective: string, tokenBudget: number | null, timeBudgetMs: number) => Promise<boolean>;
  onTransition: (state: GoalState, reason?: string) => Promise<boolean>;
}

function formatTokenUsage(tokensUsed: number, tokenBudget: number | null) {
  return tokenBudget === null
    ? `${tokensUsed.toLocaleString()} / 无上限 tokens`
    : `${tokensUsed.toLocaleString()} / ${tokenBudget.toLocaleString()} tokens`;
}

function GoalSettingsPage({
  threadId,
  goal,
  onCreate,
  onTransition,
}: GoalSettingsPageProps) {
  const toast = useToast();
  const [objective, setObjective] = useState("");
  const [tokenBudget, setTokenBudget] = useState("");
  const [timeBudgetMs, setTimeBudgetMs] = useState(60 * 60 * 1000);
  const [busy, setBusy] = useState(false);
  const [reason, setReason] = useState("");

  const active = goal && goal.state !== "completed" && goal.state !== "budget_exhausted";

  async function handleCreate(event: FormEvent) {
    event.preventDefault();
    if (!threadId) {
      toast.error("请先选择对话");
      return;
    }
    if (!objective.trim()) {
      toast.error("请输入目标说明");
      return;
    }
    const parsedTokenBudget = tokenBudget.trim() ? Number(tokenBudget) : null;
    if (parsedTokenBudget !== null && (!Number.isSafeInteger(parsedTokenBudget) || parsedTokenBudget <= 0)) {
      toast.error("Token 预算必须是正整数");
      return;
    }
    setBusy(true);
    const ok = await onCreate(objective.trim(), parsedTokenBudget, timeBudgetMs);
    setBusy(false);
    if (ok) toast.success("Goal 已创建");
    else toast.error("创建 Goal 失败，请检查预算设置");
  }

  async function handleTransition(state: GoalState) {
    setBusy(true);
    const ok = await onTransition(state, reason.trim() || undefined);
    setBusy(false);
    if (ok) {
      toast.success("Goal 状态已更新");
      setReason("");
    } else {
      toast.error("更新 Goal 状态失败");
    }
  }

  const percent = goal?.tokenBudget
    ? Math.min(100, Math.round((goal.tokensUsed / goal.tokenBudget) * 100))
    : null;

  return (
    <section className="settings-page" aria-labelledby="goal-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">智能体</p>
          <h3 id="goal-page-title">目标与预算</h3>
        </div>
        {active && goal && (
          <span className={`goal-state-badge goal-state-badge--${goal.state}`}>
            {goal.state === "active" ? "运行中" : goal.state === "paused" ? "已暂停" : "已阻塞"}
          </span>
        )}
      </div>
      <p className="settings-page-description">
        为当前对话设定目标与时间边界。Token 预算默认不限制，也可以显式设置累计上限。
      </p>

      {goal ? (
        <div className="goal-settings-current">
          <div className="goal-settings-card">
            <div className="goal-settings-card-head">
              <Flag size={16} />
              <strong title={goal.objective}>{goal.objective}</strong>
            </div>
            <div className="goal-settings-meta">
              <span><CircleDollarSign size={13} />{formatTokenUsage(goal.tokensUsed, goal.tokenBudget)}</span>
              <span><Clock3 size={13} />{(goal.elapsedMs / 60000).toFixed(1)} / {(goal.timeBudgetMs / 60000).toFixed(1)} 分钟</span>
            </div>
            {percent !== null && (
              <div className="goal-progress" aria-label={`Goal 预算已使用 ${percent}%`}>
                <span style={{ width: `${percent}%` }} />
              </div>
            )}
          </div>

          {active ? (
            <div className="goal-settings-actions">
              {goal.state === "active" ? (
                <button className="secondary-button" type="button" disabled={busy} onClick={() => void handleTransition("paused")}>
                  <Pause size={15} /> 暂停
                </button>
              ) : (
                <button className="secondary-button" type="button" disabled={busy} onClick={() => void handleTransition("active")}>
                  <Play size={15} /> 继续
                </button>
              )}
              <button className="primary-button" type="button" disabled={busy} onClick={() => void handleTransition("completed")}>
                <CircleCheck size={15} /> 完成
              </button>
            </div>
          ) : (
            <p className="goal-settings-terminal">
              {goal.state === "completed" ? "目标已完成" : "预算已耗尽，无法继续"}
            </p>
          )}
        </div>
      ) : (
        <form className="goal-settings-form" onSubmit={(event) => void handleCreate(event)}>
          <label>
            目标说明
            <textarea
              value={objective}
              maxLength={2000}
              placeholder="例如：重构 mod.rs 的权限校验，并跑通相关测试"
              onChange={(event) => setObjective(event.target.value)}
            />
          </label>
          <div className="goal-settings-row">
            <label>
              Token 预算（可选）
              <input
                type="number"
                min={1}
                step={10_000}
                value={tokenBudget}
                placeholder="默认不限制"
                onChange={(event) => setTokenBudget(event.target.value)}
              />
            </label>
            <label>
              时间预算（分钟）
              <input
                type="number"
                min={1}
                max={24 * 60}
                step={5}
                value={Math.round(timeBudgetMs / 60000)}
                onChange={(event) => setTimeBudgetMs(Number(event.target.value) * 60000)}
              />
            </label>
          </div>
          <button className="primary-button" type="submit" disabled={busy}>
            <Target size={15} /> 创建 Goal
          </button>
        </form>
      )}

      {goal && active && (
        <div className="goal-settings-reason">
          <label>
            操作说明（可选）
            <input
              type="text"
              value={reason}
              maxLength={2000}
              placeholder="例如：暂停，等待用户确认后再继续"
              onChange={(event) => setReason(event.target.value)}
            />
          </label>
        </div>
      )}
    </section>
  );
}

interface ProviderSettingsPageProps {
  provider: ProviderConfigView | null;
  configuredProviders: ProviderConfigView[];
  activeProviderId: string | null;
  error: string;
  onSave: (request: SaveProviderConfigRequest) => Promise<boolean>;
  onActivate: (providerId: string) => Promise<boolean>;
  onDelete: (providerId: string) => Promise<boolean>;
}

function ProviderSettingsPage({
  provider,
  configuredProviders,
  activeProviderId,
  error,
  onSave,
  onActivate,
  onDelete,
}: ProviderSettingsPageProps) {
  const toast = useToast();

  const [providers, setProviders] = useState<ProviderItem[]>(() => {
    const runtimeProviders = configuredProviders.length > 0
      ? configuredProviders
      : provider ? [provider] : [];
    return runtimeProviders.map((item) => providerItemFromView(item, activeProviderId));
  });

  const [selectedId, setSelectedId] = useState<string | null>(
    activeProviderId ?? providers[0]?.id ?? null
  );
  const [showMenu, setShowMenu] = useState<string | null>(null);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renamingValue, setRenamingValue] = useState("");

  const selectedProvider = providers.find(p => p.id === selectedId) ?? null;

  useEffect(() => {
    const runtimeProviders = configuredProviders.length > 0
      ? configuredProviders
      : provider ? [provider] : [];
    const runtimeIds = new Set(runtimeProviders.map((item) => item.id));
    setProviders((current) => [
      ...runtimeProviders.map((item) => providerItemFromView(item, activeProviderId)),
      ...current.filter((item) => item.isDraft && !runtimeIds.has(item.id)),
    ]);
    setSelectedId((current) => {
      if (current && (runtimeIds.has(current) || providers.some((item) => item.id === current))) {
        return current;
      }
      return activeProviderId ?? runtimeProviders[0]?.id ?? null;
    });
  }, [activeProviderId, configuredProviders, provider]);

  function handleAddProvider() {
    const newProvider: ProviderItem = {
      id: crypto.randomUUID(),
      name: "新供应商",
      baseUrl: DEFAULT_BASE_URL,
      model: "",
      models: [],
      endpoints: [],
      transport: "open_ai_chat_completions",
      hasApiKey: false,
      isDefault: providers.length === 0,
      isDraft: true,
    };
    setProviders([...providers, newProvider]);
    setSelectedId(newProvider.id);
    toast.success("已添加新供应商");
  }

  async function handleSetDefault(id: string) {
    const didActivate = await onActivate(id);
    if (!didActivate) {
      toast.error("切换失败，请先保存该供应商的 API Key");
      return;
    }
    setProviders(providers.map(p => ({ ...p, isDefault: p.id === id })));
    setShowMenu(null);
    toast.success("已切换当前供应商");
  }

  function handleStartRename(id: string, currentName: string) {
    setRenamingId(id);
    setRenamingValue(currentName);
    setShowMenu(null);
  }

  function handleRename(id: string, newName: string) {
    if (!newName.trim()) {
      setRenamingId(null);
      return;
    }
    setProviders(providers.map(p => p.id === id ? { ...p, name: newName.trim() } : p));
    setRenamingId(null);
    setRenamingValue("");
    toast.success("已重命名");
  }

  async function handleDelete(id: string) {
    const isPersisted = configuredProviders.some((item) => item.id === id);
    if (isPersisted && !(await onDelete(id))) {
      toast.error("删除供应商失败");
      return;
    }
    const filtered = providers.filter(p => p.id !== id);
    if (providers.find(p => p.id === id)?.isDefault && filtered.length > 0) {
      filtered[0].isDefault = true;
    }
    setProviders(filtered);
    if (selectedId === id) {
      setSelectedId(filtered[0]?.id ?? null);
    }
    setShowDeleteConfirm(null);
    setShowMenu(null);
    toast.success("已删除供应商");
  }

  async function handleSaveProvider(updatedProvider: ProviderItem, apiKey?: string) {
    const request: SaveProviderConfigRequest = {
      id: updatedProvider.id,
      kind: "open_ai_compatible",
      transport: updatedProvider.transport,
      name: updatedProvider.name,
      baseUrl: updatedProvider.baseUrl,
      model: updatedProvider.model,
      models: updatedProvider.models,
      endpoints: updatedProvider.endpoints,
      activate: updatedProvider.isDefault,
      ...(apiKey ? { apiKey } : {}),
    };
    const saved = await onSave(request);
    if (saved) {
      setProviders(providers.map(p => p.id === updatedProvider.id
        ? { ...updatedProvider, hasApiKey: Boolean(apiKey) || updatedProvider.hasApiKey }
        : p));
    }
    return saved;
  }

  return (
    <section className="settings-page settings-page--provider" aria-labelledby="provider-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">模型与用量</p>
          <h3 id="provider-page-title">模型供应商</h3>
        </div>
      </div>

      <div className="provider-workspace">
        <aside className="provider-list-panel" aria-label="供应商列表">
          <div className="provider-list-heading">
            <span>供应商</span>
            <span className="provider-count">{providers.length}</span>
            <button
              className="provider-add-button"
              type="button"
              title="新增供应商"
              onClick={handleAddProvider}
            >
              <Plus size={14} />
            </button>
          </div>

          {providers.map((p) => (
            <div
              key={p.id}
              className={`provider-list-item ${selectedId === p.id ? "provider-list-item--active" : ""}`}
            >
              {renamingId === p.id ? (
                <input
                  className="provider-rename-input"
                  type="text"
                  value={renamingValue}
                  onChange={(e) => setRenamingValue(e.target.value)}
                  onBlur={() => handleRename(p.id, renamingValue)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleRename(p.id, renamingValue);
                    if (e.key === "Escape") { setRenamingId(null); setRenamingValue(""); }
                  }}
                  autoFocus
                />
              ) : (
                <>
                  <button
                    className="provider-list-item-button"
                    type="button"
                    onClick={() => setSelectedId(p.id)}
                  >
                    <span className={`provider-status-dot ${p.hasApiKey ? "provider-status-dot--ready" : ""}`} />
                    <span className="provider-list-copy">
                      <strong>{p.name}</strong>
                      <small>{p.models.length > 0 ? `${p.models.length} 个模型` : "尚未配置"}</small>
                    </span>
                    {p.isDefault && <Check size={14} className="provider-default-icon" />}
                  </button>
                  <div className="provider-menu-wrapper">
                    <button
                      className="provider-menu-trigger"
                      type="button"
                      onClick={() => setShowMenu(showMenu === p.id ? null : p.id)}
                    >
                      <MoreVertical size={14} />
                    </button>
                    {showMenu === p.id && (
                      <>
                        <div className="provider-menu-backdrop" onClick={() => setShowMenu(null)} />
                        <div className="provider-menu">
                          {!p.isDefault && (
                            <button type="button" onClick={() => handleSetDefault(p.id)}>
                              <Check size={14} />
                              <span>设为当前供应商</span>
                            </button>
                          )}
                          <button type="button" onClick={() => handleStartRename(p.id, p.name)}>
                            <Edit3 size={14} />
                            <span>重命名</span>
                          </button>
                          {providers.length > 1 && (
                            <button
                              type="button"
                              className="provider-menu-delete"
                              onClick={() => {
                                setShowDeleteConfirm(p.id);
                                setShowMenu(null);
                              }}
                            >
                              <Trash2 size={14} />
                              <span>删除</span>
                            </button>
                          )}
                        </div>
                      </>
                    )}
                  </div>
                </>
              )}
            </div>
          ))}

          {providers.length === 0 && (
            <div className="provider-list-empty">
              <p>尚未添加供应商</p>
              <button className="secondary-button" type="button" onClick={handleAddProvider}>
                <Plus size={14} />
                添加供应商
              </button>
            </div>
          )}
        </aside>

        {selectedProvider ? (
          <ProviderEditor
            key={selectedProvider.id}
            providerItem={selectedProvider}
            error={error}
            onSave={(updated, apiKey) => handleSaveProvider(updated, apiKey)}
          />
        ) : (
          <div className="provider-editor provider-editor--empty">
            <p>请选择或添加一个供应商</p>
          </div>
        )}
      </div>

      {showDeleteConfirm && (
        <div className="modal-backdrop" onClick={() => setShowDeleteConfirm(null)}>
          <div className="delete-confirm-dialog" onClick={(e) => e.stopPropagation()}>
            <h4>确认删除</h4>
            <p>确定要删除供应商 "{providers.find(p => p.id === showDeleteConfirm)?.name}" 吗？</p>
            <div className="delete-confirm-actions">
              <button className="secondary-button" onClick={() => setShowDeleteConfirm(null)}>
                取消
              </button>
              <button className="danger-button" onClick={() => handleDelete(showDeleteConfirm)}>
                <Trash2 size={14} />
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

interface ProviderEditorProps {
  providerItem: ProviderItem;
  error: string;
  onSave: (updated: ProviderItem, apiKey?: string) => Promise<boolean>;
}

function ProviderEditor({ providerItem, error, onSave }: ProviderEditorProps) {
  const toast = useToast();
  const [providerName, setProviderName] = useState(providerItem.name);
  const [baseUrl, setBaseUrl] = useState(providerItem.baseUrl);
  const [models, setModels] = useState<EditableProviderModel[]>(() =>
    providerItem.models.length > 0
      ? providerItem.models.map(m => editableModel(m))
      : [editableModel()]
  );
  const [defaultModelKey, setDefaultModelKey] = useState(() =>
    models.find((model) => model.id === providerItem.model)?.key ?? models[0]?.key ?? "",
  );
  const [transport, setTransport] = useState<ProviderTransport>(providerItem.transport);
  const [endpoints, setEndpoints] = useState<ProviderEndpointConfig[]>(providerItem.endpoints ?? []);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState("");
  const deepSeekTextOnly = transport === "deep_seek_chat_completions";

  const normalizedModels = models.map(({ id, displayName, contextWindow, maxOutputTokens, supportsVision, fallback }) => ({
    id: id.trim(),
    displayName: displayName.trim(),
    contextWindow,
    maxOutputTokens,
    supportsVision: deepSeekTextOnly ? false : supportsVision,
    fallback,
  }));
  const modelIds = normalizedModels.map((model) => model.id).filter(Boolean);
  const modelError = models.length === 0
    ? "至少添加一个模型。"
    : normalizedModels.some((model) => !model.id || !model.displayName)
      ? "模型 ID 和显示名称不能为空。"
      : normalizedModels.some((model) => !Number.isInteger(model.contextWindow) || model.contextWindow < 1_024 || model.contextWindow > 10_000_000)
        ? "上下文长度必须是 1,024 到 10,000,000 之间的整数。"
        : new Set(modelIds).size !== modelIds.length
          ? "模型 ID 不能重复。"
          : "";

  function markChanged() {
    setSaved(false);
    setTestResult("");
  }

  function updateModel(key: string, patch: Partial<ProviderModelConfig>) {
    setModels((current) => current.map((model) => model.key === key ? { ...model, ...patch } : model));
    markChanged();
  }

  function addModel() {
    const next = editableModel();
    setModels((current) => [...current, next]);
    if (!defaultModelKey) setDefaultModelKey(next.key);
    markChanged();
  }

  function removeModel(key: string) {
    const next = models.filter((model) => model.key !== key);
    setModels(next);
    if (defaultModelKey === key) setDefaultModelKey(next[0]?.key ?? "");
    markChanged();
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const activeModel = models.find((model) => model.key === defaultModelKey);
    if (modelError || !activeModel) return;
    setSaving(true);
    setSaved(false);

    const updated: ProviderItem = {
      ...providerItem,
      name: providerName,
      baseUrl,
      model: activeModel.id.trim(),
      models: normalizedModels,
      transport,
      hasApiKey: apiKey.trim() ? true : providerItem.hasApiKey,
    };

    const didSave = await onSave(updated, apiKey.trim() || undefined);
    setSaving(false);
    if (didSave) {
      setApiKey("");
      setSaved(true);
      toast.success("配置已保存");
    } else {
      toast.error("保存失败，请重试");
    }
  }

  return (
    <form className="provider-editor" onSubmit={submit}>
      <header className="provider-editor-header">
        <span className="provider-logo" aria-hidden="true"><Boxes size={21} /></span>
        <div>
          <div className="provider-name-row">
            <h4>{providerName || "未命名供应商"}</h4>
            <span className={`provider-health ${providerItem.hasApiKey ? "provider-health--ready" : ""}`}>
              {providerItem.isDefault ? "当前" : "可用"}
            </span>
          </div>
          <div className="provider-tags" aria-label="供应商能力">
            <span>LLM</span>
            <span>CHAT</span>
            <span>STREAM</span>
          </div>
        </div>
      </header>

      <div className="provider-form-grid">
        <label>
          <span>供应商名称</span>
          <input
            required
            maxLength={80}
            value={providerName}
            onChange={(event) => { setProviderName(event.target.value); markChanged(); }}
            placeholder="例如 OpenAI、DeepSeek 或公司网关"
          />
        </label>
        <label>
          <span>传输协议</span>
          <select
            value={transport}
            onChange={(event) => { setTransport(event.target.value as ProviderTransport); markChanged(); }}
          >
            {transportOptions.map((option) => (
              <option value={option.value} key={option.value}>{option.label}</option>
            ))}
          </select>
        </label>
        <label className="provider-form-field--wide">
          <span>API 地址</span>
          <input
            type="url"
            required
            value={baseUrl}
            onChange={(event) => { setBaseUrl(event.target.value); markChanged(); }}
            placeholder="https://api.example.com/v1"
          />
        </label>

        <section className="provider-models provider-form-field--wide" aria-labelledby="provider-models-title">
          <div className="provider-models-heading">
            <div>
              <span id="provider-models-title">模型列表</span>
              <small>{models.length} 个模型，单选按钮表示默认模型</small>
            </div>
            <button className="secondary-button provider-model-add" type="button" onClick={addModel}>
              <Plus size={14} />新增模型
            </button>
          </div>
          <div className="provider-model-list" role="list">
            {models.map((configuredModel, index) => (
              <div className="provider-model-card" key={configuredModel.key}>
                <div className="provider-model-card-header">
                  <label className="provider-model-default">
                    <input
                      type="radio"
                      name="default-provider-model"
                      checked={configuredModel.key === defaultModelKey}
                      onChange={() => { setDefaultModelKey(configuredModel.key); markChanged(); }}
                      aria-label={`设为默认模型：${configuredModel.displayName || configuredModel.id || index + 1}`}
                    />
                    <span className="default-badge">默认模型</span>
                  </label>
                  <div className="provider-model-actions">
                    <label className="provider-model-option">
                      <input
                        type="checkbox"
                        checked={!deepSeekTextOnly && (configuredModel.supportsVision || false)}
                        disabled={deepSeekTextOnly}
                        onChange={(event) => updateModel(configuredModel.key, { supportsVision: event.target.checked })}
                      />
                      <span>支持图片</span>
                    </label>
                    <label className="provider-model-option">
                      <input
                        type="checkbox"
                        checked={configuredModel.fallback}
                        disabled={configuredModel.key === defaultModelKey}
                        onChange={(event) => updateModel(configuredModel.key, { fallback: event.target.checked })}
                      />
                      <span>故障切换</span>
                    </label>
                    <button
                      className="icon-button provider-model-delete"
                      type="button"
                      onClick={() => removeModel(configuredModel.key)}
                      aria-label={`删除模型：${configuredModel.displayName || configuredModel.id || index + 1}`}
                      title="删除模型"
                    >
                      <Trash2 size={16} />
                    </button>
                  </div>
                </div>
                <div className="provider-model-card-body">
                  <div className="provider-model-field">
                    <label>模型 ID</label>
                    <input
                      required
                      maxLength={200}
                      value={configuredModel.id}
                      onChange={(event) => updateModel(configuredModel.key, { id: event.target.value })}
                      placeholder="claude-opus-4-8"
                      aria-label={`模型 ID ${index + 1}`}
                    />
                  </div>
                  <div className="provider-model-field">
                    <label>显示名称</label>
                    <input
                      required
                      maxLength={120}
                      value={configuredModel.displayName}
                      onChange={(event) => updateModel(configuredModel.key, { displayName: event.target.value })}
                      placeholder="Claude Opus 4.8"
                      aria-label={`显示名称 ${index + 1}`}
                    />
                  </div>
                  <div className="provider-model-field">
                    <label>上下文长度</label>
                    <div className="provider-model-input-with-unit">
                      <input
                        type="number"
                        required
                        min={1_024}
                        max={10_000_000}
                        step={1}
                        value={configuredModel.contextWindow || ""}
                        onChange={(event) => updateModel(configuredModel.key, { contextWindow: Number(event.target.value) })}
                        aria-label={`上下文长度 ${index + 1}`}
                      />
                      <span className="unit">tokens</span>
                    </div>
                  </div>
                  <div className="provider-model-field">
                    <label>最大输出</label>
                    <div className="provider-model-input-with-unit">
                      <input
                        type="number"
                        min={1_024}
                        max={10_000_000}
                        step={1}
                        value={configuredModel.maxOutputTokens || ""}
                        onChange={(event) => updateModel(configuredModel.key, { maxOutputTokens: event.target.value ? Number(event.target.value) : undefined })}
                        placeholder="16384"
                        aria-label={`最大输出 ${index + 1}`}
                      />
                      <span className="unit">tokens</span>
                    </div>
                  </div>
                </div>
              </div>
            ))}
            {models.length === 0 && <div className="provider-model-empty">还没有模型</div>}
          </div>
          {modelError && <small className="provider-model-error" role="alert">{modelError}</small>}
        </section>

        <section className="provider-models provider-form-field--wide" aria-labelledby="provider-endpoints-title">
          <div className="provider-models-heading">
            <div><span id="provider-endpoints-title">备用端点</span><small>仅在瞬时请求失败且尚未输出内容时切换</small></div>
            <button className="secondary-button provider-model-add" type="button" onClick={() => { setEndpoints((items) => [...items, { id: crypto.randomUUID(), name: "备用端点", baseUrl: "", enabled: true }]); markChanged(); }}><Plus size={14} />新增端点</button>
          </div>
          <div className="provider-endpoint-list">
            {endpoints.map((endpoint) => <div className="provider-endpoint-row" key={endpoint.id}>
              <input required maxLength={80} aria-label="端点名称" value={endpoint.name} onChange={(event) => { setEndpoints((items) => items.map((item) => item.id === endpoint.id ? { ...item, name: event.target.value } : item)); markChanged(); }} />
              <input required type="url" aria-label="端点地址" placeholder="https://backup.example.com/v1" value={endpoint.baseUrl} onChange={(event) => { setEndpoints((items) => items.map((item) => item.id === endpoint.id ? { ...item, baseUrl: event.target.value } : item)); markChanged(); }} />
              <label><input type="checkbox" checked={endpoint.enabled} onChange={(event) => { setEndpoints((items) => items.map((item) => item.id === endpoint.id ? { ...item, enabled: event.target.checked } : item)); markChanged(); }} />启用</label>
              <button className="icon-button" type="button" title="删除端点" aria-label={`删除端点 ${endpoint.name}`} onClick={() => { setEndpoints((items) => items.filter((item) => item.id !== endpoint.id)); markChanged(); }}><Trash2 size={15} /></button>
            </div>)}
            {endpoints.length === 0 && <div className="provider-model-empty">未配置备用端点</div>}
          </div>
        </section>

        <label className="provider-form-field--wide">
          <span className="provider-key-label">
            API Key
            {providerItem.hasApiKey && <em><KeyRound size={12} /> 已安全保存</em>}
          </span>
          <input
            type="password"
            value={apiKey}
            onChange={(event) => { setApiKey(event.target.value); markChanged(); }}
            placeholder={providerItem.hasApiKey ? "留空则继续使用已保存密钥" : "输入 API Key"}
            required={!providerItem.hasApiKey}
            autoComplete="off"
          />
          <small>密钥仅写入操作系统凭据存储。</small>
        </label>
      </div>

      {error && <div className="settings-error" role="alert">{error}</div>}

      <footer className="provider-form-actions">
        {saved && <span className="provider-saved-state"><Check size={14} /> 配置已保存</span>}
        {testResult && <span className="provider-saved-state">{testResult}</span>}
        <button
          className="secondary-button settings-command"
          type="button"
          disabled={!providerItem.hasApiKey || testing}
          onClick={() => {
            setTesting(true);
            setTestResult("");
            void testProviderConnection(providerItem.id).then((result) => {
              setTestResult(`连接正常 · ${result.latencyMs} ms`);
              toast.success(`连接正常 · ${result.latencyMs} ms`);
            }).catch((reason) => {
              const msg = String(reason);
              setTestResult(msg);
              toast.error(msg);
            }).finally(() => setTesting(false));
          }}
        >
          <Network size={15} />{testing ? "测试中" : "测试连接"}
        </button>
        <button
          className="primary-button settings-command"
          type="submit"
          disabled={saving || Boolean(modelError) || !defaultModelKey}
        >
          <Save size={15} />
          {saving ? "保存中" : "保存配置"}
        </button>
      </footer>
    </form>
  );
}

function UsagePage() {
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const [metrics, setMetrics] = useState<MetricsSnapshot | null>(null);
  const [evaluation, setEvaluation] = useState<EvaluationReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void Promise.allSettled([getUsageSummary(), getAdvancedMetrics()]).then(([usageResult, metricsResult]) => {
      if (!active) return;
      if (usageResult.status === "fulfilled") setUsage(usageResult.value);
      else setError("无法读取用量统计");
      if (metricsResult.status === "fulfilled") setMetrics(metricsResult.value);
      setLoading(false);
    });
    return () => { active = false; };
  }, []);

  const daily = usageDailySeries(usage);
  const maxDailyTokens = Math.max(0, ...daily.map((entry) => entry.totalTokens));
  const estimatedCostUsd = usage?.estimatedCostUsd ?? metrics?.estimatedCostUsd ?? null;

  return <section className="settings-page usage-page" aria-labelledby="usage-page-title">
    <div className="settings-page-header">
      <div><p className="settings-eyebrow">模型与用量</p><h3 id="usage-page-title">用量追踪</h3></div>
      <button className="secondary-button settings-command" type="button" onClick={() => void runRegressionEvaluation().then(setEvaluation)}><PlayCircle size={15} />运行回归评估</button>
    </div>
    {error && <div className="settings-error" role="alert">{error}</div>}
    {loading && <div className="usage-loading">正在读取用量...</div>}

    <section className="usage-section usage-token-section" aria-labelledby="usage-token-title">
      <header className="usage-section-header">
        <h4 id="usage-token-title">Token 明细</h4>
        <span>{formatInteger(usage?.providerCalls ?? 0)} 次调用</span>
      </header>
      <div className="usage-token-list">
        <UsageDetailRow label="总 Token" value={usage?.totalTokens} emphasis />
        <div className="usage-detail-divider" />
        <UsageDetailRow label="输入 Token" value={usage?.inputTokens} />
        <UsageDetailRow label="缓存命中" value={usage?.cachedInputTokens} nested />
        <UsageDetailRow label="缓存未命中" value={usage?.uncachedInputTokens} nested />
        <UsageDetailRow label="缓存写入" value={usage?.cacheWriteInputTokens} nested />
        <div className="usage-detail-divider" />
        <UsageDetailRow label="输出 Token" value={usage?.outputTokens} />
        <UsageDetailRow label="推理 Token" value={usage?.reasoningOutputTokens} nested />
        <UsageDetailRow label="回复 Token" value={usage?.replyOutputTokens} nested />
        <div className="usage-detail-divider" />
        <UsageDetailRow label="缓存命中率" value={usage?.cacheHitRate} percentage emphasis />
      </div>
    </section>

    <section className="usage-section" aria-labelledby="usage-trend-title">
      <header className="usage-section-header">
        <h4 id="usage-trend-title">每日趋势</h4>
        <span>最近 {usage?.trendDays ?? 30} 天</span>
      </header>
      <div className="usage-trend-chart" role="img" aria-label="最近 30 天 Token 趋势">
        <div className="usage-trend-plot">
          {daily.map((entry) => {
            const height = entry.totalTokens > 0 && maxDailyTokens > 0
              ? Math.max(3, (entry.totalTokens / maxDailyTokens) * 100)
              : 0;
            return <div className="usage-trend-column" key={entry.date} title={`${entry.date} · ${formatTokenCount(entry.totalTokens)} Token · ${formatInteger(entry.providerCalls)} 次调用`}>
              <span className="usage-trend-bar" data-has-usage={entry.totalTokens > 0 ? "true" : "false"} style={{ height: `${height}%` }} />
            </div>;
          })}
        </div>
        <div className="usage-trend-labels" aria-hidden="true">
          {daily.map((entry, index) => <span key={entry.date}>{(index % 7 === 0 && index < daily.length - 2) || index === daily.length - 1 ? entry.date.slice(5) : ""}</span>)}
        </div>
      </div>
    </section>

    <section className="usage-section" aria-labelledby="usage-model-title">
      <header className="usage-section-header"><h4 id="usage-model-title">按模型统计</h4><span>{usage?.models?.length ?? 0} 个模型</span></header>
      <div className="usage-model-table-scroll">
        <table className="usage-model-table" aria-label="按模型统计">
          <thead><tr><th>模型</th><th>请求</th><th>输入 Token</th><th>输出 Token</th><th>缓存命中</th><th>总 Token</th><th>费用</th></tr></thead>
          <tbody>
            {(usage?.models ?? []).map((model, index) => <tr key={`${model.provider ?? "unknown"}:${model.model ?? "unknown"}:${index}`}>
              <td><div className="usage-model-name"><span className="usage-model-dot" /><div><strong title={model.model ?? "未记录模型"}>{model.model ?? "未记录模型"}</strong><span>{model.provider ?? "未记录 Provider"}</span></div></div></td>
              <td>{formatInteger(model.providerCalls)}</td>
              <td title={formatInteger(model.inputTokens)}>{formatTokenCount(model.inputTokens)}</td>
              <td title={formatInteger(model.outputTokens)}>{formatTokenCount(model.outputTokens)}</td>
              <td title={model.cachedInputTokens == null ? undefined : formatInteger(model.cachedInputTokens)}>{formatTokenCount(model.cachedInputTokens)}</td>
              <td title={formatInteger(model.totalTokens)}>{formatTokenCount(model.totalTokens)}</td>
              <td>{formatCost(model.estimatedCostUsd)}</td>
            </tr>)}
            {!loading && (usage?.models?.length ?? 0) === 0 && <tr><td className="usage-model-empty" colSpan={7}>暂无用量记录</td></tr>}
          </tbody>
        </table>
      </div>
    </section>

    <section className="usage-section" aria-labelledby="usage-runtime-title">
      <header className="usage-section-header"><h4 id="usage-runtime-title">运行指标</h4></header>
      <div className="usage-summary-grid">
        <div><span>平均延迟</span><strong>{metrics?.averageProviderLatencyMs ?? 0} ms</strong></div>
        <div><span>Provider 失败</span><strong>{metrics?.providerFailures ?? 0}</strong></div>
        <div><span>自动重试</span><strong>{metrics?.retryCount ?? 0}</strong></div>
        <div><span>工具成功率</span><strong>{Math.round((metrics?.toolSuccessRate ?? 0) * 100)}%</strong></div>
        <div><span>故障切换</span><strong>{metrics?.fallbackCount ?? 0}</strong></div>
      </div>
      <div className="settings-note">成本：{estimatedCostUsd == null ? "未知（供应商未提供价格元数据）" : formatCost(estimatedCostUsd)}</div>
    </section>
    {evaluation && <div className={evaluation.failures.length ? "settings-error" : "settings-success"}>回归评估 {evaluation.passed}/{evaluation.total} · {Math.round(evaluation.passRate * 100)}%{evaluation.failures.map((failure) => <small key={failure}>{failure}</small>)}</div>}
  </section>;
}

function UsageDetailRow({ label, value, nested = false, emphasis = false, percentage = false }: { label: string; value: number | null | undefined; nested?: boolean; emphasis?: boolean; percentage?: boolean }) {
  const displayValue = percentage ? formatPercentage(value) : formatTokenCount(value);
  return <div className={`usage-detail-row${nested ? " usage-detail-row--nested" : ""}${emphasis ? " usage-detail-row--emphasis" : ""}`}>
    <span>{label}</span>
    <strong title={value == null ? "无数据" : percentage ? formatPercentage(value) : formatInteger(value)}>{displayValue}</strong>
  </div>;
}

function formatTokenCount(value: number | null | undefined) {
  if (value == null) return "无数据";
  if (value >= 1_000_000) return `${trimDecimal(value / 1_000_000)}M`;
  if (value >= 1_000) return `${trimDecimal(value / 1_000)}K`;
  return formatInteger(value);
}

function trimDecimal(value: number) {
  return value.toFixed(1).replace(/\.0$/, "");
}

function formatInteger(value: number) {
  return new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 0 }).format(value);
}

function formatPercentage(value: number | null | undefined) {
  return value == null ? "无数据" : `${(value * 100).toFixed(1)}%`;
}

function formatCost(value: number | null | undefined) {
  return value == null ? "未知" : `$${value.toFixed(4)}`;
}

function localDateKey(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function usageDailySeries(usage: UsageSummary | null) {
  const days = Math.max(1, usage?.trendDays ?? 30);
  const byDate = new Map((usage?.daily ?? []).map((entry) => [entry.date, entry]));
  const today = new Date();
  today.setHours(12, 0, 0, 0);
  return Array.from({ length: days }, (_, index) => {
    const date = new Date(today);
    date.setDate(today.getDate() - (days - index - 1));
    const key = localDateKey(date);
    return byDate.get(key) ?? { date: key, providerCalls: 0, inputTokens: 0, outputTokens: 0, totalTokens: 0 };
  });
}

function RobotsPage({
  workflows,
  workflowRun,
}: {
  workflows: WorkflowDefinitionView[];
  workflowRun: WorkflowRunView | null;
}) {
  const [expandedId, setExpandedId] = useState<string | null>(
    workflowRun?.state === "active" ? workflowRun.workflowId : workflows[0]?.id ?? null,
  );
  const [readiness, setReadiness] = useState<Record<string, WorkflowSkillReadinessView>>({});
  const [loading, setLoading] = useState(false);
  const [readinessError, setReadinessError] = useState("");

  useEffect(() => {
    let disposed = false;
    if (!workflows.length) return undefined;
    setLoading(true);
    void Promise.all(workflows.map(async (workflow) => [
      workflow.id,
      await getWorkflowSkillReadiness(workflow.id),
    ] as const))
      .then((entries) => {
        if (disposed) return;
        setReadiness(Object.fromEntries(entries));
        setReadinessError("");
      })
      .catch((reason) => {
        if (!disposed) setReadinessError(String(reason));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => { disposed = true; };
  }, [workflows]);

  return (
    <section className="settings-page" aria-labelledby="robots-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">智能体</p>
          <h3 id="robots-page-title">内置机器人</h3>
        </div>
        <span className="settings-count">{workflows.length}</span>
      </div>
      <p className="settings-page-description">
        每个机器人按固定步骤运行。当前步骤需要的内置 Skills 始终启用；插件可选，未安装或停用时自动使用内置兼容实现。
      </p>
      {readinessError && <div className="settings-error" role="alert">{readinessError}</div>}
      <div className="robot-list">
        {workflows.map((workflow) => {
          const active = workflowRun?.state === "active" && workflowRun.workflowId === workflow.id;
          const expanded = expandedId === workflow.id;
          const workflowReadiness = readiness[workflow.id];
          const readinessByDeclaration = new Map(
            workflowReadiness?.bindings.map((item) => [item.binding.declaration, item]) ?? [],
          );
          return (
            <article className={`robot-row ${active ? "robot-row--active" : ""}`} key={workflow.id}>
              <button
                className="robot-row-header"
                type="button"
                aria-expanded={expanded}
                onClick={() => setExpandedId((current) => current === workflow.id ? null : workflow.id)}
              >
                <Bot size={17} />
                <span className="robot-row-copy">
                  <strong>{workflow.name}</strong>
                  <span>{workflow.description}</span>
                  <small>{workflow.uniqueSkillCount} 个技能 · {workflow.nodes.length} 个步骤</small>
                </span>
                <span className="robot-row-state">
                  {active ? (
                    <span className="robot-active-state">运行中</span>
                  ) : workflowReadiness?.ready ? (
                    <span className="robot-ready-state">可运行</span>
                  ) : workflowReadiness ? (
                    <span className="robot-blocked-state">{workflowReadiness.blockerCount} 项异常</span>
                  ) : loading ? (
                    <span className="robot-loading-state">检查中</span>
                  ) : null}
                  {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                </span>
              </button>

              {expanded && (
                <div className="robot-details">
                  <div className="robot-system-prompt-group">
                    <div className="robot-section-heading">
                      <span>System Prompt</span>
                      <small>内置只读</small>
                    </div>
                    <pre
                      className="robot-system-prompt"
                      aria-label={`${workflow.name} 的 System Prompt`}
                    >{workflow.rolePrompt}</pre>
                  </div>

                  <div className="robot-skill-summary" aria-label={`${workflow.name} 技能组成`}>
                    <span><strong>{workflow.localSkillCount}</strong> 本地技能</span>
                    <span><strong>{workflow.pluginSkillCount}</strong> 插件技能</span>
                    <span><strong>{workflow.nodes.length}</strong> 工作流步骤</span>
                  </div>

                  <RobotSkillGroup
                    title="本地技能"
                    bindings={workflow.skillCatalog.filter((binding) => binding.kind === "skill")}
                    readinessByDeclaration={readinessByDeclaration}
                  />
                  <RobotSkillGroup
                    title="插件技能"
                    bindings={workflow.skillCatalog.filter((binding) => binding.kind === "plugin_skill")}
                    readinessByDeclaration={readinessByDeclaration}
                  />

                  <div className="robot-section-heading">
                    <span>工作流</span>
                    <small>{workflow.nodes.length} 步，按次执行</small>
                  </div>
                  <ol className="robot-node-list">
                    {workflow.nodes.map((node, index) => {
                      const completed = active && index < workflowRun.currentNodeIndex;
                      const current = active && index === workflowRun.currentNodeIndex;
                      const nodeReadiness = workflowReadiness?.nodes.find((item) => item.nodeId === node.id);
                      return (
                        <li className={completed ? "robot-node--completed" : current ? "robot-node--current" : ""} key={node.id}>
                          <span>{completed ? <Check size={12} /> : index + 1}</span>
                          <div>
                            <strong>{index + 1}. {node.title}</strong>
                            <small>{node.description}</small>
                            <div className="robot-node-skills">
                              {[...node.localSkillBindings, ...node.pluginSkillBindings].map((binding) => {
                                const bindingReadiness = readinessByDeclaration.get(binding.declaration);
                                return (
                                  <span
                                    className={`robot-skill-chip robot-skill-chip--${bindingReadiness?.status ?? "unknown"}`}
                                    title={robotSkillStatusTitle(bindingReadiness)}
                                    key={`${node.id}-${binding.declaration}`}
                                  >
                                    {binding.declaration}
                                  </span>
                                );
                              })}
                            </div>
                            {nodeReadiness && !nodeReadiness.ready && (
                              <small className="robot-node-warning"><AlertCircle size={12} />{nodeReadiness.blockers.length} 项技能异常</small>
                            )}
                          </div>
                        </li>
                      );
                    })}
                  </ol>
                </div>
              )}
            </article>
          );
        })}
      </div>
    </section>
  );
}

function RobotSkillGroup({
  title,
  bindings,
  readinessByDeclaration,
}: {
  title: string;
  bindings: WorkflowDefinitionView["skillCatalog"];
  readinessByDeclaration: Map<string, WorkflowSkillReadinessView["bindings"][number]>;
}) {
  return <div className="robot-skill-group">
    <div className="robot-section-heading"><span>{title}</span><small>{bindings.length}</small></div>
    <div className="robot-skill-cloud">
      {bindings.map((binding) => {
        const bindingReadiness = readinessByDeclaration.get(binding.declaration);
        return <span
          className={`robot-skill-chip robot-skill-chip--${bindingReadiness?.status ?? "unknown"}`}
          title={robotSkillStatusTitle(bindingReadiness)}
          key={binding.declaration}
        >
          {binding.declaration}
          {bindingReadiness?.status === "builtin_fallback" && <small>内置兼容</small>}
        </span>;
      })}
    </div>
  </div>;
}

function robotSkillStatusTitle(
  binding: WorkflowSkillReadinessView["bindings"][number] | undefined,
) {
  if (!binding) return "正在解析技能来源";
  if (binding.blocker) return binding.blocker;
  switch (binding.status) {
    case "plugin": return "使用已启用插件";
    case "builtin_fallback": return "插件未启用或不可用，使用内置兼容 Skill";
    case "builtin": return "机器人内置 Skill，始终启用";
    case "global": return "使用全局 Skill";
    case "project": return "使用项目 Skill";
    default: return binding.status;
  }
}

function knowledgeSourceStateLabel(state: string) {
  switch (state) {
    case "queued": return "等待索引";
    case "indexing": return "正在索引";
    case "indexed": return "索引可用";
    case "failed": return "索引失败";
    case "cancelled": return "已取消";
    default: return state;
  }
}

function formatKnowledgeError(reason: unknown) {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  if (reason && typeof reason === "object") {
    const value = reason as { code?: unknown; message?: unknown };
    const code = typeof value.code === "string" ? value.code : "";
    const message = typeof value.message === "string" ? value.message : "";
    if (code && message) return `${code}：${message}`;
    if (message) return message;
    if (code) return code;
  }
  try {
    const serialized = JSON.stringify(reason);
    return serialized && serialized !== "{}" ? serialized : "知识库操作失败";
  } catch {
    return "知识库操作失败";
  }
}

function normalizeKnowledgePath(value: string) {
  return toUserFacingPath(value).replace(/\\/g, "/").replace(/\/+$/, "");
}

function selectedKnowledgeRelativePath(selectedPath: string, workspaceRoot: string) {
  const selected = normalizeKnowledgePath(selectedPath);
  const root = normalizeKnowledgePath(workspaceRoot);
  const selectedKey = workspacePathKey(selected);
  const rootKey = workspacePathKey(root);
  if (selectedKey === rootKey || !selectedKey.startsWith(`${rootKey}/`)) {
    throw new Error("只能选择当前工作区内的文件");
  }
  const relative = selected.slice(root.length + 1).replace(/^\/+/, "");
  if (!relative) throw new Error("请选择文件，而不是文件夹");
  return relative;
}

function formatKnowledgeBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function KnowledgePage() {
  const [settings, setSettings] = useState<KnowledgeSettings | null>(null);
  const [embedding, setEmbedding] = useState<EmbeddingSettings | null>(null);
  const [collections, setCollections] = useState<KnowledgeCollection[]>([]);
  const [sources, setSources] = useState<Record<string, KnowledgeSource[]>>({});
  const [name, setName] = useState("");
  const [nameError, setNameError] = useState("");
  const [sourcePaths, setSourcePaths] = useState<Record<string, string>>({});
  const [sourceErrors, setSourceErrors] = useState<Record<string, string>>({});
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [pickerCollectionId, setPickerCollectionId] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<null | {
    kind: "collection" | "source";
    id: string;
    name: string;
  }>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const loadVersionRef = useRef(0);

  async function load() {
    const version = ++loadVersionRef.current;
    const [nextSettings, nextEmbedding, nextCollections] = await Promise.all([
      getKnowledgeSettings(), getEmbeddingSettings(), listKnowledgeCollections(),
    ]);
    const entries = await Promise.all(nextCollections.map(async (collection) => [collection.id, await listKnowledgeSources(collection.id)] as const));
    if (version !== loadVersionRef.current) return;
    setSettings(nextSettings); setEmbedding(nextEmbedding); setCollections(nextCollections);
    setSources(Object.fromEntries(entries));
  }
  useEffect(() => { void load().catch((reason) => setError(formatKnowledgeError(reason))); }, []);
  useEffect(() => {
    const hasActiveJob = Object.values(sources)
      .flat()
      .some((source) => source.state === "queued" || source.state === "indexing");
    if (!hasActiveJob) return undefined;
    const timer = window.setInterval(() => {
      void load().catch((reason) => setError(formatKnowledgeError(reason)));
    }, 750);
    return () => window.clearInterval(timer);
  }, [sources]);

  async function run(action: () => Promise<void>) {
    setBusy(true); setError("");
    try { await action(); await load(); } catch (reason) { setError(formatKnowledgeError(reason)); } finally { setBusy(false); }
  }

  async function addManualSource(collectionId: string) {
    const value = sourcePaths[collectionId]?.trim() ?? "";
    if (!value) return;
    await run(async () => {
      await addKnowledgeSource({ collectionId, workspaceRelativePath: value });
      setSourcePaths((current) => ({ ...current, [collectionId]: "" }));
      setSourceErrors((current) => ({ ...current, [collectionId]: "" }));
    });
  }

  async function pickSources(collectionId: string) {
    setPickerCollectionId(collectionId);
    setError("");
    setSourceErrors((current) => ({ ...current, [collectionId]: "" }));
    try {
      const selected = await open({
        title: "选择知识库来源",
        multiple: true,
        directory: false,
      });
      if (!selected) return;
      const selectedPaths = Array.isArray(selected) ? selected : [selected];
      const workspace = await getWorkspaceState();
      const validPaths: string[] = [];
      const rejectedPaths: string[] = [];
      for (const selectedPath of selectedPaths) {
        try {
          validPaths.push(selectedKnowledgeRelativePath(selectedPath, workspace.current.path));
        } catch {
          rejectedPaths.push(toUserFacingPath(selectedPath));
        }
      }
      if (rejectedPaths.length) {
        setSourceErrors((current) => ({
          ...current,
          [collectionId]: `${rejectedPaths.length} 个文件不在当前工作区内，已跳过。`,
        }));
      }
      if (!validPaths.length) return;
      setBusy(true);
      const failed: string[] = [];
      for (const relativePath of validPaths) {
        try {
          await addKnowledgeSource({ collectionId, workspaceRelativePath: relativePath });
        } catch (reason) {
          failed.push(`${relativePath}: ${formatKnowledgeError(reason)}`);
        }
      }
      if (failed.length) {
        setSourceErrors((current) => ({
          ...current,
          [collectionId]: `${failed.length} 个文件添加失败：${failed[0]}`,
        }));
      } else {
        setSourcePaths((current) => ({ ...current, [collectionId]: "" }));
      }
      await load();
    } catch (reason) {
      setError(formatKnowledgeError(reason));
    } finally {
      setPickerCollectionId(null);
      setBusy(false);
    }
  }

  async function createCollection(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const nextName = name.trim();
    if (!nextName) {
      setNameError("请输入 Collection 名称");
      nameInputRef.current?.focus();
      return;
    }
    setNameError("");
    await run(async () => {
      await upsertKnowledgeCollection({ name: nextName, enabled: true });
      setName("");
    });
  }

  async function confirmDelete() {
    const target = pendingDelete;
    if (!target) return;
    setPendingDelete(null);
    await run(async () => {
      if (target.kind === "collection") {
        await deleteKnowledgeCollection(target.id, target.id);
      } else {
        await deleteKnowledgeSource(target.id, target.id);
      }
    });
  }

  if (!settings || !embedding) return <section className="settings-page knowledge-page"><div className="settings-page-header"><div><p className="settings-eyebrow">知识与规则</p><h3>知识库</h3></div></div><div className="settings-pending">正在加载知识库</div></section>;
  return <section className="settings-page knowledge-page" aria-labelledby="knowledge-page-title">
    <header className="knowledge-hero">
      <div className="knowledge-hero-copy">
        <div className="knowledge-title-line"><span className="knowledge-title-icon" aria-hidden="true"><Library size={18} /></span><div><p className="settings-eyebrow">知识与规则</p><h3 id="knowledge-page-title">知识库</h3></div></div>
        <p>把工作区中的文档整理成可检索的本地知识。只会索引你明确添加的文件。</p>
      </div>
      <label className="knowledge-enable-toggle"><input type="checkbox" checked={settings.enabled} disabled={busy} onChange={(event) => void run(async () => { setSettings(await setKnowledgeEnabled(event.target.checked)); })} /><span><strong>{settings.enabled ? "知识库已启用" : "知识库已停用"}</strong><small>{settings.enabled ? "搜索时可使用已建立的索引" : "开启后允许智能体检索本地来源"}</small></span></label>
    </header>

    <section className="knowledge-create-card" aria-labelledby="knowledge-create-title">
      <div className="knowledge-card-heading"><div><span className="knowledge-section-kicker">集合</span><h4 id="knowledge-create-title">新建 Collection</h4></div><span className="knowledge-card-hint">按项目、领域或团队分组</span></div>
      <form className="knowledge-collection-form" noValidate onSubmit={(event) => void createCollection(event)}>
        <label className="knowledge-field"><span>名称</span><input ref={nameInputRef} value={name} maxLength={80} placeholder="例如：项目文档" aria-label="Collection 名称" aria-invalid={Boolean(nameError)} aria-describedby={nameError ? "knowledge-collection-name-error" : undefined} onChange={(event) => { setName(event.target.value); if (nameError) setNameError(""); }} /></label>
        <button className="primary-button" type="submit" disabled={busy}><Plus size={14} />创建</button>
        {nameError && <small className="knowledge-field-error" id="knowledge-collection-name-error" role="alert">{nameError}</small>}
      </form>
    </section>

    <div className="knowledge-section-header"><div><span className="knowledge-section-kicker">已配置</span><h4>Collections</h4></div><span className="knowledge-count-badge">{collections.length}</span></div>
    {collections.length ? collections.map((collection) => {
      const collectionPath = sourcePaths[collection.id] ?? "";
      const collectionSources = sources[collection.id] ?? [];
      const picking = pickerCollectionId === collection.id;
      return <article className="memory-row knowledge-collection-card" key={collection.id}>
        <header className="knowledge-collection-header">
          <div className="knowledge-collection-title"><span className="knowledge-collection-icon" aria-hidden="true"><Library size={16} /></span><div><div className="knowledge-name-line"><h4>{collection.name}</h4><span className="knowledge-count-badge">{collectionSources.length} 个来源</span></div><p>{collection.indexedChunkCount} 个可检索切片</p></div></div>
          <div className="knowledge-collection-actions"><label className="extension-toggle"><input type="checkbox" checked={collection.enabled} disabled={busy} onChange={(event) => void run(async () => { await upsertKnowledgeCollection({ id: collection.id, name: collection.name, enabled: event.target.checked }); })} /><span>启用</span></label><button className="knowledge-icon-button" type="button" title="删除 Collection" aria-label={`删除 Collection ${collection.name}`} disabled={busy} onClick={() => setPendingDelete({ kind: "collection", id: collection.id, name: collection.name })}><Trash2 size={15} /></button></div>
        </header>
        <div className="knowledge-source-panel">
          <div className="knowledge-source-heading"><div><span className="knowledge-section-kicker">来源</span><strong>添加要纳入检索的文件</strong></div><span>{collectionSources.length ? `${collectionSources.length} 个已添加` : "尚未添加来源"}</span></div>
          <div className="knowledge-source-add">
            <label className="knowledge-source-input"><span className="sr-only">工作区相对路径</span><FileText size={15} aria-hidden="true" /><input value={collectionPath} placeholder="工作区相对路径，如 docs/guide.md" aria-label="来源路径" onChange={(event) => { setSourcePaths((current) => ({ ...current, [collection.id]: event.target.value })); if (sourceErrors[collection.id]) setSourceErrors((current) => ({ ...current, [collection.id]: "" })); }} /></label>
            <button className="secondary-button" type="button" disabled={busy || picking || !collectionPath.trim()} onClick={() => void addManualSource(collection.id)}><Plus size={14} />添加路径</button>
            <button className="secondary-button knowledge-picker-button" type="button" disabled={busy || picking} aria-label="选择文件" title="从当前工作区选择文件" onClick={() => void pickSources(collection.id)}><FolderOpen size={15} />{picking ? "选择中..." : "选择文件"}</button>
          </div>
          {sourceErrors[collection.id] && <small className="knowledge-field-error" role="alert">{sourceErrors[collection.id]}</small>}
          <div className="knowledge-source-list">
            {collectionSources.length ? collectionSources.map((source) => {
              const active = source.state === "queued" || source.state === "indexing";
              const StatusIcon = source.state === "indexed" ? CheckCircle2 : source.state === "failed" ? AlertCircle : Clock3;
              return <div className="knowledge-source-row" key={source.sourceId}>
                <span className="knowledge-source-file-icon" aria-hidden="true"><FileText size={15} /></span><div className="knowledge-source-copy"><strong title={source.relativePath}>{source.relativePath}</strong><small>{formatKnowledgeBytes(source.sizeBytes)}{source.contentHashPrefix ? ` · ${source.contentHashPrefix}` : ""} · {source.chunkCount} chunks</small></div>
                <span className={`knowledge-source-status knowledge-source-status--${source.state}`} aria-live="polite"><StatusIcon size={13} aria-hidden="true" />{knowledgeSourceStateLabel(source.state)}{source.lastErrorCode ? ` · ${source.lastErrorCode}` : ""}</span>
                <div className="knowledge-source-actions">
                  {active && source.initialJobId ? <button className="knowledge-icon-button" type="button" title="取消索引" aria-label={`取消 ${source.relativePath} 的索引`} disabled={busy} onClick={() => void run(async () => { await cancelKnowledgeIndexJob(source.initialJobId!); })}><CircleStop size={15} /></button> : null}
                  <button className="knowledge-icon-button" type="button" title="刷新来源" aria-label={`刷新 ${source.relativePath}`} disabled={busy || active} onClick={() => void run(async () => { await refreshKnowledgeSource(source.sourceId); })}><RefreshCw className={active ? "spin" : ""} size={15} /></button>
                  <button className="knowledge-icon-button knowledge-icon-button--danger" type="button" title="删除知识来源" aria-label={`删除知识来源 ${source.relativePath}`} disabled={busy} onClick={() => setPendingDelete({ kind: "source", id: source.sourceId, name: source.relativePath })}><Trash2 size={15} /></button>
                </div>
              </div>;
            }) : <div className="knowledge-source-empty"><FileText size={19} aria-hidden="true" /><span>还没有来源，选择一个工作区文件开始建立索引。</span></div>}
          </div>
        </div>
      </article>;
    }) : <div className="knowledge-empty-collections"><Library size={20} aria-hidden="true" /><strong>先创建一个 Collection</strong><span>Collection 用来管理一组相关的工作区来源。</span></div>}

    <section className="knowledge-settings-card" aria-labelledby="knowledge-semantic-title">
      <div className="knowledge-card-heading"><div><span className="knowledge-section-kicker">检索引擎</span><h4 id="knowledge-semantic-title">语义检索</h4></div><span className="knowledge-card-hint">固定 SiliconFlow BGE-M3</span></div>
      <p className="settings-help">关闭语义检索时使用本地 FTS-only；开启后只会向 SiliconFlow 发送已建立索引的文本片段。</p>
      <div className="knowledge-semantic-layout"><label className="knowledge-enable-toggle knowledge-enable-toggle--compact"><input type="checkbox" checked={embedding.semanticEnabled} disabled={busy} onChange={(event) => void run(async () => { setEmbedding(await setEmbeddingSettings({ semanticEnabled: event.target.checked, batchSize: embedding.batchSize, timeoutMs: embedding.timeoutMs, maxVectorScanChunks: embedding.maxVectorScanChunks })); })} /><span><strong>启用语义检索</strong><small>{embedding.semanticEnabled ? "混合 BM25 与向量排序" : "仅使用本地全文检索"}</small></span></label><div className="knowledge-engine-meta"><span>{embedding.provider}</span><span>{embedding.model}</span><span>{embedding.embeddingStatus}</span></div></div>
      <div className="knowledge-api-row"><label className="knowledge-api-input"><span className="sr-only">SiliconFlow API Key</span><KeyRound size={15} aria-hidden="true" /><input type="password" value={apiKey} maxLength={512} autoComplete="new-password" placeholder={embedding.embeddingConfigured ? "已配置 API Key" : "SiliconFlow API Key"} aria-label="SiliconFlow API Key" onChange={(event) => setApiKey(event.target.value)} /></label><button className="secondary-button" type="button" disabled={busy || !apiKey.trim()} onClick={() => void run(async () => { await setEmbeddingApiKey(apiKey); setApiKey(""); })}><Save size={14} />保存</button>{embedding.embeddingConfigured && <><button className="secondary-button" type="button" disabled={busy} onClick={() => void run(async () => { const result = await testEmbeddingConnection(); if (!result.connected) throw new Error(result.errorCode ?? "连接失败"); })}><PlayCircle size={14} />测试连接</button><button className="secondary-button knowledge-danger-button" type="button" disabled={busy} onClick={() => void run(async () => { await deleteEmbeddingApiKey(); })}><Trash2 size={14} />删除 Key</button></>}</div>
    </section>
    {error && <div className="settings-error" role="alert">{error}</div>}
    {pendingDelete && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && setPendingDelete(null)} onKeyDown={(event) => { if (event.key !== "Escape") return; event.preventDefault(); event.stopPropagation(); setPendingDelete(null); }}>
      <section className="delete-confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="knowledge-delete-title">
        <h4 id="knowledge-delete-title">{pendingDelete.kind === "collection" ? "删除 Collection" : "删除知识来源"}</h4>
        <p>{pendingDelete.kind === "collection" ? `将删除“${pendingDelete.name}”中的全部本地索引，工作区原文件不会被删除。` : `将删除“${pendingDelete.name}”的本地索引，工作区原文件不会被删除。`}</p>
        <div className="delete-confirm-actions">
          <button className="secondary-button" type="button" autoFocus disabled={busy} onClick={() => setPendingDelete(null)}>取消</button>
          <button className="danger-button" type="button" disabled={busy} onClick={() => void confirmDelete()}><Trash2 size={14} />删除索引</button>
        </div>
      </section>
    </div>}
  </section>;
}

function BrowserPage() {
  const [settings, setSettings] = useState<BrowserSettings>({ enabled: false, allowLocalhost: false });
  const [audit, setAudit] = useState<BrowserAuditEvent[]>([]);
  const [artifacts, setArtifacts] = useState<BrowserArtifact[]>([]);
  const [error, setError] = useState("");
  const load = () => Promise.all([getBrowserSettings(), listBrowserAudit(), listBrowserArtifacts()]).then(([nextSettings, nextAudit, nextArtifacts]) => { setSettings(nextSettings); setAudit(nextAudit); setArtifacts(nextArtifacts); });
  useEffect(() => { void load().catch((reason) => setError(String(reason))); }, []);
  async function update(next: BrowserSettings) { try { setSettings(await saveBrowserSettings(next)); setError(""); await load(); } catch (reason) { setError(String(reason)); } }
  return <section className="settings-page" aria-labelledby="browser-page-title">
    <div className="settings-page-header"><div><p className="settings-eyebrow">智能体</p><h3 id="browser-page-title">浏览器自动化</h3></div><label className="extension-toggle"><input type="checkbox" checked={settings.enabled} onChange={(event) => void update({ ...settings, enabled: event.target.checked })} /><span>启用</span></label></div>
    <div className="browser-permissions"><label><input type="checkbox" checked={settings.allowLocalhost} disabled={!settings.enabled} onChange={(event) => void update({ ...settings, allowLocalhost: event.target.checked })} />允许 localhost 与私网地址</label><p>浏览器操作需要单独审批；导航、点击、输入和截图都会记录审计事件。</p></div>
    {error && <div className="settings-error">{error}</div>}
    <div className="extension-section-label">制品 · {artifacts.length}</div>
    <div className="artifact-list">{artifacts.slice(0, 20).map((artifact) => <div key={artifact.id}><strong>{artifact.name}</strong><span>{Math.ceil(artifact.sizeBytes / 1024)} KiB · {new Date(artifact.createdAtMs).toLocaleString()}</span></div>)}</div>
    <div className="extension-section-label">浏览器审计</div>
    <div className="audit-list">{audit.slice().reverse().slice(0, 50).map((event, index) => <div key={`${event.timestampMs}-${index}`}><span className={event.success ? "audit-ok" : "audit-failed"}>{event.success ? "成功" : "失败"}</span><div><strong>{event.action}</strong><small>{event.target} · {event.detail}</small></div><time>{new Date(event.timestampMs).toLocaleString()}</time></div>)}</div>
  </section>;
}

function ExtensionsPage() {
  const [overview, setOverview] = useState<ExtensionOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<SkillCategory | "all">("all");
  const [scope, setScope] = useState<"all" | "builtin" | "global" | "project">("all");
  const [collapsedGroups, setCollapsedGroups] = useState<ReadonlySet<string>>(
    () => new Set(SKILL_CATEGORIES.slice(1).map((item) => item.id)),
  );

  async function load(refresh = false) {
    setLoading(true);
    setError("");
    try {
      setOverview(await getExtensionOverview(refresh));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { void load(); }, []);

  async function toggle(kind: "skill" | "hook", id: string, enabled: boolean) {
    setLoading(true);
    try {
      setOverview(await setExtensionEnabled(kind, id, enabled));
      setError("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filtered = (overview?.skills ?? []).filter((skill) => {
    const matchesQuery = !normalizedQuery
      || skill.name.toLocaleLowerCase().includes(normalizedQuery)
      || skill.description.toLocaleLowerCase().includes(normalizedQuery)
      || skill.triggers.some((trigger) => trigger.toLocaleLowerCase().includes(normalizedQuery));
    return matchesQuery
      && (category === "all" || skill.category === category)
      && (scope === "all" || skill.scope === scope);
  });
  const groups = SKILL_CATEGORIES
    .map((item) => ({ ...item, skills: filtered.filter((skill) => skill.category === item.id) }))
    .filter((item) => item.skills.length > 0);
  const hasActiveFilter = normalizedQuery !== "" || category !== "all" || scope !== "all";
  const isGroupExpanded = (groupId: string) => hasActiveFilter || !collapsedGroups.has(groupId);
  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((current) => {
      const next = new Set(current);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  };

  return <section className="settings-page extensions-page" aria-labelledby="skills-page-title">
    <div className="settings-page-header">
      <div><p className="settings-eyebrow">扩展</p><h3 id="skills-page-title">Skills</h3></div>
      <span className="settings-count">{overview?.skills.length ?? 0}</span>
      <button className="icon-button" type="button" aria-label="刷新扩展" title="刷新扩展" disabled={loading} onClick={() => void load(true)}><RefreshCw className={loading ? "spin" : ""} size={16} /></button>
    </div>
    <p className="settings-page-description">按职责浏览本地、全局和项目 Skills。机器人包属于内置工作流契约，默认启用且不能关闭。</p>
    <div className="skill-filters" aria-label="筛选 Skills">
      <label className="skill-search"><Search size={15} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索名称、说明或触发词" aria-label="搜索 Skills" /></label>
      <select value={category} onChange={(event) => setCategory(event.target.value as SkillCategory | "all")} aria-label="按分类筛选">
        <option value="all">全部分类</option>
        {SKILL_CATEGORIES.map((item) => <option value={item.id} key={item.id}>{item.label}</option>)}
      </select>
      <select value={scope} onChange={(event) => setScope(event.target.value as typeof scope)} aria-label="按来源筛选">
        <option value="all">全部来源</option>
        <option value="builtin">内置</option>
        <option value="global">全局</option>
        <option value="project">项目</option>
      </select>
    </div>
    {(error || overview?.error) && <div className="settings-error" role="alert">{error || overview?.error}</div>}
    {groups.length ? <div className="skill-category-list">
      {groups.map((group) => {
        const expanded = isGroupExpanded(group.id);
        const enabledCount = group.skills.filter((skill) => skill.enabled || skill.managedByRobot).length;
        return <section className="skill-category-group" aria-labelledby={`skill-category-${group.id}`} key={group.id}>
          <button
            type="button"
            className="skill-category-heading"
            aria-expanded={expanded}
            aria-controls={`skill-category-${group.id}-body`}
            onClick={() => toggleGroup(group.id)}
          >
            {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
            <div>
              <strong id={`skill-category-${group.id}`}>{group.label}</strong>
              <span>{group.description}</span>
            </div>
            <span className="skill-category-meta">
              <small>{enabledCount}/{group.skills.length} 已启用</small>
              <small className="skill-category-count">{group.skills.length}</small>
            </span>
          </button>
          {expanded && (
            <div className="extension-list" id={`skill-category-${group.id}-body`}>
              {group.skills.map((skill) => <div className="extension-row" key={`${skill.scope}-${skill.path}-${skill.name}`}>
                <div className="extension-row-main">
                  <div className="skill-name-line">
                    <strong>{skill.name}</strong>
                    <span className={`skill-scope skill-scope--${skill.scope}`}>{skillScopeText(skill.scope)}</span>
                    <span className={`skill-risk skill-risk--${skill.risk}`}>{riskText(skill.risk)}</span>
                  </div>
                  <span className="skill-desc">{skill.description}</span>
                  {skill.triggers.length > 0 && (
                    <div className="skill-triggers">
                      {skill.triggers.map((trigger) => <code key={trigger}>{trigger}</code>)}
                    </div>
                  )}
                </div>
                {skill.managedByRobot ? (
                  <span className="skill-managed-state" title="机器人工作流运行所需，不能停用"><LockKeyhole size={13} />机器人必需</span>
                ) : (
                  <label className="extension-toggle"><input type="checkbox" checked={skill.enabled} disabled={loading} onChange={(event) => void toggle("skill", skill.name, event.target.checked)} /><span>启用</span></label>
                )}
              </div>)}
            </div>
          )}
        </section>;
      })}
    </div> : <ExtensionEmpty text={overview?.skills.length ? "没有符合筛选条件的 Skill" : "未发现有效的 SKILL.md"} />}
  </section>;
}

const SKILL_CATEGORIES: Array<{ id: SkillCategory; label: string; description: string }> = [
  { id: "requirements_planning", label: "需求与规划", description: "需求澄清、方案设计与实施计划" },
  { id: "development_delivery", label: "开发与交付", description: "编码、协作、分支与交付流程" },
  { id: "quality_review", label: "质量与评审", description: "调试、验证与代码评审" },
  { id: "testing", label: "测试工程", description: "用例、接口、性能、安全与报告" },
  { id: "design_experience", label: "设计与体验", description: "界面设计、交互与视觉质量" },
  { id: "data_documents", label: "数据与文档", description: "结构化数据与文档产出" },
  { id: "observability", label: "可观测性", description: "日志、诊断与运行状态" },
  { id: "integration_automation", label: "集成与自动化", description: "浏览器和外部服务集成" },
  { id: "extension_platform", label: "扩展平台", description: "Skill 与插件的创建、安装和同步" },
  { id: "other", label: "其他", description: "尚未归入固定职责的 Skill" },
];

function ExtensionEmpty({ text }: { text: string }) {
  return <div className="extension-empty">{text}</div>;
}

function riskText(risk: "read" | "write" | "delete" | "external") {
  return risk === "read" ? "只读" : risk === "write" ? "写入" : risk === "delete" ? "删除" : "外部";
}

function skillScopeText(scope: string) {
  return scope === "builtin" ? "内置" : scope === "global" ? "全局" : scope === "project" ? "项目" : scope;
}

function AppearancePage({
  themeMode,
  onSelectTheme,
}: {
  themeMode: ThemeId;
  onSelectTheme: (theme: ThemeId) => void;
}) {
  const iconForTheme = {
    sun: Sun,
    moon: Moon,
    cloud: Cloud,
    monitor: Monitor,
    flame: Flame,
    sparkles: Sparkles,
    terminal: Terminal,
    waves: Waves,
  } as const;

  return (
    <section className="settings-page appearance-page" aria-labelledby="appearance-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">应用</p>
          <h3 id="appearance-page-title">外观</h3>
          <p className="settings-page-description">
            跟随系统，或强制使用浅色、深色与主题皮肤。顶部的切换按钮是浅色和深色模式的快捷方式。
          </p>
        </div>
      </div>

      <div className="appearance-theme-section">
        <div className="appearance-section-heading">
          <div>
            <p className="settings-eyebrow">主题</p>
            <h4>选择工作区氛围</h4>
          </div>
          <span className="appearance-current-theme" role="status">
            当前：{themeLabel(themeMode)}
          </span>
        </div>

        <div className="theme-picker" role="radiogroup" aria-label="选择主题">
          {THEME_OPTIONS.map((option) => {
            const Icon = iconForTheme[option.icon];
            const selected = option.id === themeMode;
            return (
              <button
                className={`theme-option ${selected ? "theme-option--selected" : ""}`}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-label={`${option.label}：${option.description}`}
                onClick={() => onSelectTheme(option.id)}
                key={option.id}
              >
                <span
                  className="theme-option-swatch"
                  aria-hidden="true"
                  style={{ background: `linear-gradient(135deg, ${option.swatch[0]} 0 58%, ${option.swatch[1]} 58% 100%)` }}
                />
                <span className="theme-option-icon" aria-hidden="true"><Icon size={15} /></span>
                <span className="theme-option-copy">
                  <strong>{option.label}</strong>
                  <small>{option.description}</small>
                </span>
                {selected && <Check className="theme-option-check" size={14} aria-hidden="true" />}
              </button>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function PendingSection({ definition }: { definition: SettingsDefinition }) {
  const Icon = definition.icon;

  return (
    <section className="settings-page" aria-labelledby={`${definition.id}-settings-title`}>
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">{definition.group}</p>
          <h3 id={`${definition.id}-settings-title`}>{definition.label}</h3>
        </div>
      </div>
      <div className="settings-pending">
        <Icon size={24} />
        <strong>尚未接入</strong>
        <span>等待对应运行时能力完成</span>
      </div>
    </section>
  );
}
