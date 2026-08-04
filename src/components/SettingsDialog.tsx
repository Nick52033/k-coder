import { FormEvent, useEffect, useState } from "react";
import {
  deleteMcpSecret,
  getExtensionOverview,
  getUsageSummary,
  saveMcpSecret,
  setExtensionEnabled,
  testProviderConnection,
  deleteMemory,
  getAdvancedMetrics,
  getBrowserSettings,
  getMemorySettings,
  listBrowserArtifacts,
  listBrowserAudit,
  listMemories,
  runRegressionEvaluation,
  saveBrowserSettings,
  setMemoryEnabled,
  upsertMemory,
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
  MemorySettings,
  MemoryView,
  MetricsSnapshot,
  GoalState,
  GoalView,
} from "../types/runtime";

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
    supportsVision: model?.supportsVision ?? false,
    fallback: model?.fallback ?? false,
  };
}

const transportOptions: Array<{ value: ProviderTransport; label: string }> = [
  { value: "open_ai_chat_completions", label: "OpenAI Chat Completions" },
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
  | "skills"
  | "robots"
  | "workflows"
  | "knowledge"
  | "rules"
  | "general"
  | "browser"
  | "goal";

type ThemeMode = "light" | "dark";

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
  error: string;
  themeMode: ThemeMode;
  onClose: () => void;
  onToggleTheme: () => void;
  onSaveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
  onActivateProvider: (providerId: string) => Promise<boolean>;
  onDeleteProvider: (providerId: string) => Promise<boolean>;
  onCreateGoal: (objective: string, tokenBudget: number | null, timeBudgetMs: number) => Promise<boolean>;
  onTransitionGoal: (state: GoalState, reason?: string) => Promise<boolean>;
}

const settingsDefinitions: SettingsDefinition[] = [
  { id: "providers", label: "模型供应商", group: "模型与用量", icon: ServerCog, available: true },
  { id: "usage", label: "用量追踪", group: "模型与用量", icon: BarChart3, available: true },
  { id: "mcp", label: "MCP 与 Hooks", group: "扩展", icon: Network, available: true },
  { id: "plugins", label: "插件管理", group: "扩展", icon: Puzzle, available: false },
  { id: "skills", label: "Skills", group: "扩展", icon: Sparkles, available: true },
  { id: "robots", label: "机器人", group: "智能体", icon: Bot, available: false },
  { id: "workflows", label: "Workflows", group: "智能体", icon: Workflow, available: false },
  { id: "knowledge", label: "记忆", group: "知识与规则", icon: Library, available: true },
  { id: "browser", label: "浏览器自动化", group: "智能体", icon: Globe2, available: true },
  { id: "goal", label: "目标与预算", group: "智能体", icon: Target, available: true },
  { id: "rules", label: "规则与审计", group: "知识与规则", icon: ShieldCheck, available: true },
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
  error,
  themeMode,
  onClose,
  onToggleTheme,
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
              <AppearancePage themeMode={themeMode} onToggleTheme={onToggleTheme} />
            ) : section === "usage" ? (
              <UsagePage />
            ) : section === "knowledge" ? (
              <MemoryPage />
            ) : section === "browser" ? (
              <BrowserPage />
            ) : section === "goal" ? (
              <GoalSettingsPage
                threadId={activeThreadId}
                goal={goal}
                onCreate={onCreateGoal}
                onTransition={onTransitionGoal}
              />
            ) : section === "mcp" || section === "skills" || section === "rules" ? (
              <ExtensionsPage mode={section} />
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

  const normalizedModels = models.map(({ id, displayName, contextWindow, maxOutputTokens, supportsVision, fallback }) => ({
    id: id.trim(),
    displayName: displayName.trim(),
    contextWindow,
    maxOutputTokens,
    supportsVision,
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
                        checked={configuredModel.supportsVision || false}
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
  useEffect(() => { void Promise.all([getUsageSummary(), getAdvancedMetrics()]).then(([nextUsage, nextMetrics]) => { setUsage(nextUsage); setMetrics(nextMetrics); }); }, []);
  return <section className="settings-page" aria-labelledby="usage-page-title">
    <div className="settings-page-header"><div><p className="settings-eyebrow">模型与用量</p><h3 id="usage-page-title">运行指标</h3></div><button className="secondary-button settings-command" type="button" onClick={() => void runRegressionEvaluation().then(setEvaluation)}><PlayCircle size={15} />运行回归评估</button></div>
    <div className="usage-summary-grid">
      <div><span>Provider 调用</span><strong>{usage?.providerCalls ?? 0}</strong></div>
      <div><span>输入 Token</span><strong>{usage?.inputTokens ?? 0}</strong></div>
      <div><span>输出 Token</span><strong>{usage?.outputTokens ?? 0}</strong></div>
      <div><span>总 Token</span><strong>{usage?.totalTokens ?? 0}</strong></div>
      <div><span>平均延迟</span><strong>{metrics?.averageProviderLatencyMs ?? 0} ms</strong></div>
      <div><span>Provider 失败</span><strong>{metrics?.providerFailures ?? 0}</strong></div>
      <div><span>工具成功率</span><strong>{Math.round((metrics?.toolSuccessRate ?? 0) * 100)}%</strong></div>
      <div><span>故障切换</span><strong>{metrics?.fallbackCount ?? 0}</strong></div>
    </div>
    <div className="settings-note">成本：{metrics?.estimatedCostUsd == null ? "未知（供应商未提供价格元数据）" : `$${metrics.estimatedCostUsd.toFixed(4)}`}</div>
    {evaluation && <div className={evaluation.failures.length ? "settings-error" : "settings-success"}>回归评估 {evaluation.passed}/{evaluation.total} · {Math.round(evaluation.passRate * 100)}%{evaluation.failures.map((failure) => <small key={failure}>{failure}</small>)}</div>}
  </section>;
}

function MemoryPage() {
  const [settings, setSettings] = useState<MemorySettings>({ enabled: false });
  const [memories, setMemories] = useState<MemoryView[]>([]);
  const [content, setContent] = useState("");
  const [source, setSource] = useState("用户设置");
  const [retentionDays, setRetentionDays] = useState(30);
  const [editingId, setEditingId] = useState<string | undefined>();
  const [error, setError] = useState("");
  const load = () => Promise.all([getMemorySettings(), listMemories()]).then(([nextSettings, nextMemories]) => { setSettings(nextSettings); setMemories(nextMemories); });
  useEffect(() => { void load().catch((reason) => setError(String(reason))); }, []);
  async function save() {
    try {
      await upsertMemory({ id: editingId, content, source, retentionDays });
      setContent(""); setEditingId(undefined); setError(""); await load();
    } catch (reason) { setError(String(reason)); }
  }
  return <section className="settings-page" aria-labelledby="memory-page-title">
    <div className="settings-page-header"><div><p className="settings-eyebrow">知识与规则</p><h3 id="memory-page-title">记忆</h3></div><label className="extension-toggle"><input type="checkbox" checked={settings.enabled} onChange={(event) => void setMemoryEnabled(event.target.checked).then((next) => { setSettings(next); setError(""); }).catch((reason) => setError(String(reason)))} /><span>启用</span></label></div>
    <div className="memory-editor"><textarea maxLength={4000} value={content} onChange={(event) => setContent(event.target.value)} placeholder="需要智能体长期记住的偏好或约束" disabled={!settings.enabled} /><div><input maxLength={240} value={source} onChange={(event) => setSource(event.target.value)} aria-label="记忆来源" /><label>保留天数<input type="number" min={1} max={365} value={retentionDays} onChange={(event) => setRetentionDays(Number(event.target.value))} /></label><button className="primary-button" type="button" disabled={!settings.enabled || !content.trim() || !source.trim()} onClick={() => void save()}><Save size={14} />{editingId ? "更新" : "添加"}</button></div></div>
    {error && <div className="settings-error">{error}</div>}
    <div className="memory-list">{memories.map((memory) => <div className="memory-row" key={memory.id}><div><strong>{memory.content}</strong><span>{memory.source} · 到期 {new Date(memory.expiresAtMs).toLocaleDateString()}</span></div><button type="button" title="编辑记忆" aria-label="编辑记忆" onClick={() => { setEditingId(memory.id); setContent(memory.content); setSource(memory.source); setRetentionDays(Math.max(1, Math.ceil((memory.expiresAtMs - Date.now()) / 86_400_000))); }}><Edit3 size={14} /></button><button type="button" title="删除记忆" aria-label="删除记忆" onClick={() => void deleteMemory(memory.id).then(load).catch((reason) => setError(String(reason)))}><Trash2 size={14} /></button></div>)}</div>
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

function ExtensionsPage({ mode }: { mode: "mcp" | "skills" | "rules" }) {
  const [overview, setOverview] = useState<ExtensionOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

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

  useEffect(() => { void load(); }, [mode]);

  async function toggle(kind: "skill" | "mcp" | "hook", id: string, enabled: boolean) {
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

  const title = mode === "mcp" ? "MCP 与 Hooks" : mode === "skills" ? "Skills" : "规则与审计";
  return <section className="settings-page extensions-page" aria-labelledby={`${mode}-page-title`}>
    <div className="settings-page-header"><div><p className="settings-eyebrow">可控扩展</p><h3 id={`${mode}-page-title`}>{title}</h3></div><button className="icon-button" type="button" aria-label="刷新扩展" title="刷新扩展" disabled={loading} onClick={() => void load(true)}><RefreshCw className={loading ? "spin" : ""} size={16} /></button></div>
    {(error || overview?.error) && <div className="settings-error" role="alert">{error || overview?.error}</div>}
    {mode === "mcp" && <>
      <div className="extension-section-label">服务器</div>
      <div className="extension-list">{overview?.mcpServers.length ? overview.mcpServers.map((server) => <div className="extension-row" key={server.id}>
        <div className={`extension-state extension-state--${server.state}`} aria-hidden="true" />
        <div className="extension-row-main"><strong>{server.id}</strong><span>{server.transport} · {server.toolCount} 个工具 · {server.state}</span>{server.error && <small>{server.error}</small>}</div>
        <label className="extension-toggle"><input type="checkbox" checked={server.enabled} disabled={loading} onChange={(event) => void toggle("mcp", server.id, event.target.checked)} /><span>启用</span></label>
        {server.credentials.length > 0 && <div className="extension-credentials">{server.credentials.map((credential) => <McpCredential key={credential.name} server={server.id} name={credential.name} configured={credential.configured} onUpdated={setOverview} />)}</div>}
      </div>) : <ExtensionEmpty text="尚未配置 MCP 服务器" />}</div>
      <div className="extension-section-label">Hooks</div>
      <div className="extension-list">{overview?.hooks.length ? overview.hooks.map((hook) => <div className="extension-row extension-row--compact" key={hook.id}><div className="extension-row-main"><strong>{hook.id}</strong><span>{hook.phase} · {hook.tool}</span></div><label className="extension-toggle"><input type="checkbox" checked={hook.enabled} disabled={loading} onChange={(event) => void toggle("hook", hook.id, event.target.checked)} /><span>启用</span></label></div>) : <ExtensionEmpty text="尚未配置工具 Hook" />}</div>
    </>}
    {mode === "skills" && <div className="extension-list">{overview?.skills.length ? overview.skills.map((skill) => <div className="extension-row" key={skill.name}><div className={`skill-risk skill-risk--${skill.risk}`}>{riskText(skill.risk)}</div><div className="extension-row-main"><strong>{skill.name}</strong><span>{skill.description}</span><small>{skill.scope} · {skill.triggers.join("、")}</small></div><label className="extension-toggle"><input type="checkbox" checked={skill.enabled} disabled={loading} onChange={(event) => void toggle("skill", skill.name, event.target.checked)} /><span>启用</span></label></div>) : <ExtensionEmpty text="未发现有效的 SKILL.md" />}</div>}
    {mode === "rules" && <>
      <div className="extension-section-label">指令优先级</div>
      <div className="instruction-list">{overview?.instructions.length ? overview.instructions.map((source) => <div key={source.path}><span>{source.priority}</span><div><strong>{source.scope}</strong><small title={source.path}>{source.path}</small></div><em>{source.bytes} B</em></div>) : <ExtensionEmpty text="未发现全局或项目指令" />}</div>
      <div className="extension-section-label">配置位置</div>
      <div className="config-paths">{overview?.configPaths.map((path) => <code key={path}>{path}</code>)}</div>
      <div className="extension-section-label">审计历史</div>
      <div className="audit-list">{overview?.audit.length ? overview.audit.slice().reverse().slice(0, 40).map((record, index) => <div key={`${record.timestampMs}-${index}`}><span className={record.success ? "audit-ok" : "audit-failed"}>{record.success ? "成功" : "失败"}</span><div><strong>{record.event}</strong><small>{record.kind}/{record.id} · {record.detail}</small></div><time>{new Date(record.timestampMs).toLocaleString()}</time></div>) : <ExtensionEmpty text="暂无扩展审计记录" />}</div>
    </>}
  </section>;
}

function McpCredential({ server, name, configured, onUpdated }: { server: string; name: string; configured: boolean; onUpdated: (overview: ExtensionOverview) => void }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function save() {
    if (!value.trim()) return;
    setBusy(true);
    try { onUpdated(await saveMcpSecret(server, name, value)); setValue(""); setError(""); } catch (reason) { setError(String(reason)); } finally { setBusy(false); }
  }
  async function remove() {
    setBusy(true);
    try { onUpdated(await deleteMcpSecret(server, name)); setError(""); } catch (reason) { setError(String(reason)); } finally { setBusy(false); }
  }
  return <div className="credential-row"><div><KeyRound size={13} /><span>{name}</span><small>{configured ? "已配置" : "缺失"}</small></div><input type="password" value={value} onChange={(event) => setValue(event.target.value)} placeholder={configured ? "替换凭据" : "输入凭据"} autoComplete="off" aria-label={`${server} ${name} 凭据`} /><button type="button" disabled={busy || !value.trim()} onClick={() => void save()}>保存</button>{configured && <button type="button" disabled={busy} onClick={() => void remove()}>删除</button>}{error && <small className="credential-error" role="alert">{error}</small>}</div>;
}

function ExtensionEmpty({ text }: { text: string }) {
  return <div className="extension-empty">{text}</div>;
}

function riskText(risk: "read" | "write" | "delete" | "external") {
  return risk === "read" ? "只读" : risk === "write" ? "写入" : risk === "delete" ? "删除" : "外部";
}

function AppearancePage({
  themeMode,
  onToggleTheme,
}: {
  themeMode: ThemeMode;
  onToggleTheme: () => void;
}) {
  return (
    <section className="settings-page" aria-labelledby="appearance-page-title">
      <div className="settings-page-header">
        <div>
          <p className="settings-eyebrow">应用</p>
          <h3 id="appearance-page-title">外观</h3>
        </div>
      </div>

      <div style={{ marginBottom: 28 }}>
        <p className="settings-eyebrow" style={{ marginBottom: 12 }}>模式</p>
        <div style={{ display: "flex", gap: 10 }}>
          <button
            className={`secondary-button ${themeMode === "light" ? "primary-button" : ""}`}
            type="button"
            onClick={() => themeMode === "dark" && onToggleTheme()}
            style={{ minWidth: 100 }}
          >
            <Sun size={15} style={{ marginRight: 6 }} />
            浅色
          </button>
          <button
            className={`secondary-button ${themeMode === "dark" ? "primary-button" : ""}`}
            type="button"
            onClick={() => themeMode === "light" && onToggleTheme()}
            style={{ minWidth: 100 }}
          >
            <Sun size={15} style={{ marginRight: 6, opacity: 0.4 }} />
            深色
          </button>
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
