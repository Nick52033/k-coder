import { FormEvent, useEffect, useState } from "react";
import {
  deleteMcpSecret,
  getExtensionOverview,
  getUsageSummary,
  saveMcpSecret,
  setExtensionEnabled,
  testProviderConnection,
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
} from "lucide-react";
import type {
  ProviderConfigView,
  ProviderTransport,
  SaveProviderConfigRequest,
  UsageSummary,
  ExtensionOverview,
} from "../types/runtime";

const DEFAULT_BASE_URL = "https://api.openai.com/v1";

const transportOptions: Array<{ value: ProviderTransport; label: string }> = [
  { value: "open_ai_chat_completions", label: "OpenAI Chat Completions" },
  { value: "open_ai_responses", label: "OpenAI Responses API" },
  { value: "anthropic_messages", label: "Anthropic Messages API" },
  { value: "google_gemini", label: "Google Gemini API" },
];

type SettingsSection =
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
  | "general";

type Skin = "paper" | "midnight" | "amethyst" | "indigo" | "amber";
type ThemeMode = "light" | "dark";

interface SkinDefinition {
  id: Skin;
  label: string;
  desc: string;
  preview: string;
}

const skinDefinitions: SkinDefinition[] = [
  { id: "paper", label: "纸墨精工", desc: "绿色品牌 · 浅色为主 · 日常精工", preview: "#176b4d" },
  { id: "midnight", label: "午夜终端", desc: "OLED 深黑 · 翠绿高亮 · 纯暗色", preview: "#10b981" },
  { id: "indigo", label: "靛蓝电影", desc: "深蓝渐变 · 紫色高亮 · 玻璃质感", preview: "#5E6AD2" },
  { id: "amethyst", label: "紫晶指令", desc: "紫金渐变 · 明亮活泼 · 双模式", preview: "#7c3aed" },
  { id: "amber", label: "琥珀暖光", desc: "暖白纸感 · 橙金点缀 · 极度护眼", preview: "#D97706" },
];

interface SettingsDefinition {
  id: SettingsSection;
  label: string;
  group: string;
  icon: typeof ServerCog;
  available: boolean;
}

interface SettingsDialogProps {
  provider: ProviderConfigView | null;
  error: string;
  skin: Skin;
  themeMode: ThemeMode;
  onClose: () => void;
  onSetSkin: (skin: Skin) => void;
  onToggleTheme: () => void;
  onSaveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
}

const settingsDefinitions: SettingsDefinition[] = [
  { id: "providers", label: "模型供应商", group: "模型与用量", icon: ServerCog, available: true },
  { id: "usage", label: "用量追踪", group: "模型与用量", icon: BarChart3, available: true },
  { id: "mcp", label: "MCP 与 Hooks", group: "扩展", icon: Network, available: true },
  { id: "plugins", label: "插件管理", group: "扩展", icon: Puzzle, available: false },
  { id: "skills", label: "Skills", group: "扩展", icon: Sparkles, available: true },
  { id: "robots", label: "机器人", group: "智能体", icon: Bot, available: false },
  { id: "workflows", label: "Workflows", group: "智能体", icon: Workflow, available: false },
  { id: "knowledge", label: "本地知识库", group: "知识与规则", icon: Library, available: false },
  { id: "rules", label: "规则与审计", group: "知识与规则", icon: ShieldCheck, available: true },
  { id: "appearance", label: "外观", group: "应用", icon: Palette, available: true },
  { id: "general", label: "通用", group: "应用", icon: Settings, available: false },
];

export function SettingsDialog({
  provider,
  error,
  skin,
  themeMode,
  onClose,
  onSetSkin,
  onToggleTheme,
  onSaveProvider,
}: SettingsDialogProps) {
  const [section, setSection] = useState<SettingsSection>("providers");

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
                error={error}
                onSave={onSaveProvider}
              />
            ) : section === "appearance" ? (
              <AppearancePage
                skin={skin}
                themeMode={themeMode}
                onSetSkin={onSetSkin}
                onToggleTheme={onToggleTheme}
              />
            ) : section === "usage" ? (
              <UsagePage />
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

interface ProviderSettingsPageProps {
  provider: ProviderConfigView | null;
  error: string;
  onSave: (request: SaveProviderConfigRequest) => Promise<boolean>;
}

function ProviderSettingsPage({ provider, error, onSave }: ProviderSettingsPageProps) {
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
            <span className="provider-count">1</span>
          </div>
          <button className="provider-list-item provider-list-item--active" type="button">
            <span className={`provider-status-dot ${provider ? "provider-status-dot--ready" : ""}`} />
            <span className="provider-list-copy">
              <strong>自定义供应商</strong>
              <small>{provider?.baseUrl ?? "尚未配置"}</small>
            </span>
            {provider && <Check size={14} />}
          </button>
        </aside>

        <ProviderEditor initial={provider} error={error} onSave={onSave} />
      </div>
    </section>
  );
}

interface ProviderEditorProps {
  initial: ProviderConfigView | null;
  error: string;
  onSave: (request: SaveProviderConfigRequest) => Promise<boolean>;
}

function ProviderEditor({ initial, error, onSave }: ProviderEditorProps) {
  const toast = useToast();
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? DEFAULT_BASE_URL);
  const [model, setModel] = useState(initial?.model ?? "");
  const [transport, setTransport] = useState<ProviderTransport>(
    initial?.transport ?? "open_ai_chat_completions",
  );
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState("");

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    setSaved(false);
    const didSave = await onSave({
      kind: "open_ai_compatible",
      transport,
      baseUrl,
      model,
      ...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
    });
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
            <h4>自定义供应商</h4>
            <span className={`provider-health ${initial ? "provider-health--ready" : ""}`}>
              {initial ? "当前" : "未配置"}
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
        <label className="provider-form-field--wide">
          <span>传输协议</span>
          <select
            value={transport}
            onChange={(event) => {
              setTransport(event.target.value as ProviderTransport);
              setSaved(false);
            }}
          >
            {transportOptions.map((option) => (
              <option value={option.value} key={option.value}>{option.label}</option>
            ))}
          </select>
        </label>
        <label>
          <span>API 地址</span>
          <input
            type="url"
            required
            value={baseUrl}
            onChange={(event) => {
              setBaseUrl(event.target.value);
              setSaved(false);
            }}
            placeholder="https://api.example.com/v1"
          />
        </label>
        <label>
          <span>模型 ID</span>
          <input
            required
            value={model}
            onChange={(event) => {
              setModel(event.target.value);
              setSaved(false);
            }}
            placeholder="例如 gpt-4.1"
          />
        </label>
        <label className="provider-form-field--wide">
          <span className="provider-key-label">
            API Key
            {initial?.hasApiKey && <em><KeyRound size={12} /> 已安全保存</em>}
          </span>
          <input
            type="password"
            value={apiKey}
            onChange={(event) => {
              setApiKey(event.target.value);
              setSaved(false);
            }}
            placeholder={initial?.hasApiKey ? "留空则继续使用已保存密钥" : "输入 API Key"}
            required={!initial?.hasApiKey}
            autoComplete="off"
          />
          <small>密钥仅写入操作系统凭据存储。</small>
        </label>
      </div>

      {error && <div className="settings-error" role="alert">{error}</div>}

      <footer className="provider-form-actions">
        {saved && <span className="provider-saved-state"><Check size={14} /> 配置已保存</span>}
        {testResult && <span className="provider-saved-state">{testResult}</span>}
        <button className="secondary-button settings-command" type="button" disabled={!initial || testing} onClick={() => { setTesting(true); setTestResult(""); void testProviderConnection().then((result) => { setTestResult(`连接正常 · ${result.latencyMs} ms`); toast.success(`连接正常 · ${result.latencyMs} ms`); }).catch((reason) => { const msg = String(reason); setTestResult(msg); toast.error(msg); }).finally(() => setTesting(false)); }}>
          <Network size={15} />{testing ? "测试中" : "测试连接"}
        </button>
        <button className="primary-button settings-command" type="submit" disabled={saving}>
          <Save size={15} />
          {saving ? "保存中" : "保存配置"}
        </button>
      </footer>
    </form>
  );
}

function UsagePage() {
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  useEffect(() => { void getUsageSummary().then(setUsage); }, []);
  return <section className="settings-page" aria-labelledby="usage-page-title">
    <div className="settings-page-header"><div><p className="settings-eyebrow">模型与用量</p><h3 id="usage-page-title">累计用量</h3></div></div>
    <div className="usage-summary-grid">
      <div><span>Provider 调用</span><strong>{usage?.providerCalls ?? 0}</strong></div>
      <div><span>输入 Token</span><strong>{usage?.inputTokens ?? 0}</strong></div>
      <div><span>输出 Token</span><strong>{usage?.outputTokens ?? 0}</strong></div>
      <div><span>总 Token</span><strong>{usage?.totalTokens ?? 0}</strong></div>
    </div>
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
  skin,
  themeMode,
  onSetSkin,
  onToggleTheme,
}: {
  skin: Skin;
  themeMode: ThemeMode;
  onSetSkin: (skin: Skin) => void;
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

      <p className="settings-eyebrow" style={{ marginBottom: 12 }}>皮肤主题</p>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(180px, 1fr))", gap: 12 }}>
        {skinDefinitions.map((item) => (
          <button
            key={item.id}
            className={`settings-nav-item ${skin === item.id ? "settings-nav-item--active" : ""}`}
            type="button"
            onClick={() => onSetSkin(item.id)}
            style={{ flexDirection: "column", alignItems: "flex-start", gap: 10, minHeight: 120, padding: 14 }}
          >
            <span
              style={{
                display: "block",
                width: 28,
                height: 28,
                borderRadius: "var(--radius-sm)",
                background: item.preview,
                flexShrink: 0,
              }}
            />
            <div style={{ textAlign: "left" }}>
              <div style={{ fontWeight: 650, fontSize: "var(--font-size-md)", marginBottom: 3 }}>
                {item.label}
              </div>
              <div style={{ fontSize: "var(--font-size-xs)", color: "var(--color-ink-subtle)", lineHeight: 1.4 }}>
                {item.desc}
              </div>
            </div>
          </button>
        ))}
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
