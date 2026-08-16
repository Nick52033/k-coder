import { useEffect, useState } from "react";
import {
  AlertCircle,
  Braces,
  CheckCircle2,
  Circle,
  FileJson2,
  KeyRound,
  LoaderCircle,
  RefreshCw,
  Save,
  Server,
  Trash2,
  XCircle,
} from "lucide-react";
import {
  deleteMcpSecret,
  getMcpConfig,
  saveMcpConfig,
  saveMcpSecret,
  setExtensionEnabled,
} from "../api/runtime";
import type {
  ExtensionOverview,
  McpConfigDocumentView,
  McpConfigView,
  McpDiagnostic,
} from "../types/runtime";
import { useToast } from "./Toast";
import "./McpSettingsPage.css";

type McpScope = "global" | "project";

interface StdioMcpServer {
  id: string;
  enabled?: boolean;
  timeoutMs?: number;
  transport: "stdio";
  command: string[];
  secret_env?: Record<string, string>;
}

interface HttpMcpServer {
  id: string;
  enabled?: boolean;
  timeoutMs?: number;
  transport: "streamable_http";
  url: string;
  secret_headers?: Record<string, string>;
}

interface McpConfigDocument {
  mcpServers?: Array<StdioMcpServer | HttpMcpServer>;
}

interface EditableDocument {
  path: string;
  exists: boolean;
  raw: string;
  savedRaw: string;
  parseError: string;
}

type EditableDocuments = Record<McpScope, EditableDocument>;

const MAX_CONFIG_BYTES = 1024 * 1024;
const SERVER_ID_PATTERN = /^[a-z0-9_-]{1,64}$/;
const ENVIRONMENT_PATTERN = /^[A-Z0-9_]{1,128}$/;
const CREDENTIAL_PATTERN = /^[A-Za-z0-9_.-]{1,128}$/;
const HEADER_PATTERN = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function byteLength(value: string) {
  return new TextEncoder().encode(value).byteLength;
}

function assertKnownFields(value: Record<string, unknown>, allowed: string[], label: string) {
  const unknown = Object.keys(value).find((key) => !allowed.includes(key));
  if (unknown) throw new Error(`${label} 包含未知字段 ${unknown}`);
}

function validateCredentialMap(
  value: unknown,
  field: string,
  validateTarget: (target: string) => boolean,
  invalidTargetMessage: string,
) {
  if (value === undefined) return;
  if (!isRecord(value)) throw new Error(`${field} 必须是对象`);
  for (const [target, credential] of Object.entries(value)) {
    if (!validateTarget(target)) throw new Error(`${field}.${target || "<空>"} ${invalidTargetMessage}`);
    if (typeof credential !== "string" || !CREDENTIAL_PATTERN.test(credential)) {
      throw new Error(`${field}.${target} 的凭据名称无效`);
    }
  }
}

function validateHttpUrl(value: string, label: string) {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${label} 的 url 无效`);
  }
  const loopback = url.protocol === "http:"
    && ["localhost", "127.0.0.1", "[::1]", "::1"].includes(url.hostname);
  if (url.protocol !== "https:" && !loopback) {
    throw new Error(`${label} 必须使用 HTTPS，只有 loopback 地址可以使用 HTTP`);
  }
  if (url.username || url.password || url.hash) {
    throw new Error(`${label} 的 url 不能包含凭据或片段`);
  }
}

function parseMcpDocument(content: string): { document: McpConfigDocument | null; error: string } {
  try {
    if (byteLength(content) > MAX_CONFIG_BYTES) {
      throw new Error("mcp.json 不能超过 1 MiB");
    }
    const value = JSON.parse(content) as unknown;
    if (!isRecord(value)) throw new Error("mcp.json 顶层必须是对象");
    assertKnownFields(value, ["mcpServers"], "mcp.json");
    if (value.mcpServers !== undefined && !Array.isArray(value.mcpServers)) {
      throw new Error("mcpServers 必须是数组");
    }

    const ids = new Set<string>();
    (value.mcpServers ?? []).forEach((entry, index) => {
      const label = `服务器 ${index + 1}`;
      if (!isRecord(entry)) throw new Error(`${label} 必须是对象`);
      if (typeof entry.id !== "string" || !SERVER_ID_PATTERN.test(entry.id)) {
        throw new Error(`${label} 的 id 必须使用 1-64 位小写字母、数字、下划线或连字符`);
      }
      if (ids.has(entry.id)) throw new Error(`服务器 id ${entry.id} 不能重复`);
      ids.add(entry.id);
      if (entry.enabled !== undefined && typeof entry.enabled !== "boolean") {
        throw new Error(`${label} 的 enabled 必须是布尔值`);
      }
      if (
        entry.timeoutMs !== undefined
        && (
          typeof entry.timeoutMs !== "number"
          || !Number.isInteger(entry.timeoutMs)
          || entry.timeoutMs < 1
          || entry.timeoutMs > 300_000
        )
      ) {
        throw new Error(`${label} 的 timeoutMs 必须是 1-300000 之间的整数`);
      }

      if (entry.transport === "stdio") {
        assertKnownFields(
          entry,
          ["id", "enabled", "timeoutMs", "transport", "command", "secret_env"],
          label,
        );
        if (
          !Array.isArray(entry.command)
          || entry.command.length === 0
          || entry.command.length > 128
          || entry.command.some((part) => typeof part !== "string" || byteLength(part) > 8192)
        ) {
          throw new Error(`${label} 的 command 必须是 1-128 段字符串，单段不能超过 8192 字节`);
        }
        validateCredentialMap(
          entry.secret_env,
          `${label}.secret_env`,
          (target) => ENVIRONMENT_PATTERN.test(target),
          "不是有效的环境变量名",
        );
        return;
      }

      if (entry.transport === "streamable_http") {
        assertKnownFields(
          entry,
          ["id", "enabled", "timeoutMs", "transport", "url", "secret_headers"],
          label,
        );
        if (typeof entry.url !== "string") throw new Error(`${label} 的 url 必须是字符串`);
        validateHttpUrl(entry.url, label);
        validateCredentialMap(
          entry.secret_headers,
          `${label}.secret_headers`,
          (target) => HEADER_PATTERN.test(target),
          "不是有效的 Header 名称",
        );
        return;
      }

      throw new Error(`${label} 的 transport 必须是 stdio 或 streamable_http`);
    });

    return { document: value as McpConfigDocument, error: "" };
  } catch (reason) {
    return { document: null, error: reason instanceof Error ? reason.message : String(reason) };
  }
}

function editableDocument(view: McpConfigDocumentView): EditableDocument {
  const parsed = parseMcpDocument(view.content);
  return {
    path: view.path,
    exists: view.exists,
    raw: view.content,
    savedRaw: view.content,
    parseError: view.error ?? parsed.error,
  };
}

function statusLabel(diagnostic: McpDiagnostic) {
  if (diagnostic.state === "ready") return `${diagnostic.toolCount} 个工具`;
  if (diagnostic.state === "failed") return "连接失败";
  if (diagnostic.state === "disabled") return "已停用";
  return diagnostic.state || "未加载";
}

function statusTone(diagnostic: McpDiagnostic) {
  if (["ready", "failed", "disabled"].includes(diagnostic.state)) return diagnostic.state;
  return "unknown";
}

function StatusIcon({ diagnostic }: { diagnostic: McpDiagnostic }) {
  if (diagnostic.state === "ready") return <CheckCircle2 size={15} aria-hidden="true" />;
  if (diagnostic.state === "failed") return <XCircle size={15} aria-hidden="true" />;
  return <Circle size={14} aria-hidden="true" />;
}

function transportLabel(transport: string) {
  return transport === "streamable_http" ? "Streamable HTTP" : transport.toUpperCase();
}

export function McpSettingsPage() {
  const toast = useToast();
  const [view, setView] = useState<McpConfigView | null>(null);
  const [documents, setDocuments] = useState<EditableDocuments | null>(null);
  const [scope, setScope] = useState<McpScope>("global");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [busyHook, setBusyHook] = useState("");
  const [error, setError] = useState("");

  function applyView(next: McpConfigView) {
    setView(next);
    setDocuments({
      global: editableDocument(next.global),
      project: editableDocument(next.project),
    });
  }

  async function load(refresh = false) {
    const hasDraft = documents
      ? Object.values(documents).some((document) => document.raw !== document.savedRaw)
      : false;
    if (refresh && hasDraft && !window.confirm("放弃尚未保存的 MCP 配置更改？")) return;

    setLoading(true);
    setError("");
    try {
      applyView(await getMcpConfig(refresh));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
  }, []);

  const current = documents?.[scope] ?? null;
  const dirty = current ? current.raw !== current.savedRaw : false;
  const currentBytes = current ? byteLength(current.raw) : 0;

  function updateRaw(raw: string) {
    const parsed = parseMcpDocument(raw);
    setDocuments((previous) => previous ? {
      ...previous,
      [scope]: {
        ...previous[scope],
        raw,
        parseError: parsed.error,
      },
    } : previous);
  }

  function formatRaw() {
    if (!current) return;
    const parsed = parseMcpDocument(current.raw);
    if (!parsed.document) {
      updateRaw(current.raw);
      return;
    }
    updateRaw(`${JSON.stringify(parsed.document, null, 2)}\n`);
  }

  async function save() {
    if (!current || !dirty) return;
    const parsed = parseMcpDocument(current.raw);
    if (!parsed.document) {
      updateRaw(current.raw);
      return;
    }

    const savingScope = scope;
    setSaving(true);
    setError("");
    try {
      applyView(await saveMcpConfig(savingScope, current.raw));
      toast.success(`${savingScope === "global" ? "全局" : "项目"} MCP 配置已保存`);
    } catch (reason) {
      setError(String(reason));
      toast.error("MCP 配置保存失败");
    } finally {
      setSaving(false);
    }
  }

  async function toggleHook(id: string, enabled: boolean) {
    setBusyHook(id);
    try {
      const overview = await setExtensionEnabled("hook", id, enabled);
      setView((value) => value ? { ...value, overview } : value);
      setError("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusyHook("");
    }
  }

  function updateOverview(overview: ExtensionOverview) {
    setView((value) => value ? { ...value, overview } : value);
  }

  return (
    <section className="settings-page mcp-page" aria-labelledby="mcp-page-title">
      <div className="settings-page-header mcp-page-header">
        <div>
          <p className="settings-eyebrow">可控扩展</p>
          <h3 id="mcp-page-title">MCP 配置</h3>
        </div>
        <button
          className="icon-button"
          type="button"
          aria-label="刷新 MCP 配置"
          title="刷新"
          disabled={loading || saving}
          onClick={() => void load(true)}
        >
          <RefreshCw className={loading ? "spin" : ""} size={16} />
        </button>
      </div>

      <div className="mcp-toolbar">
        <div className="mcp-segmented" role="group" aria-label="MCP 配置作用域">
          <button type="button" aria-pressed={scope === "global"} onClick={() => setScope("global")}>全局</button>
          <button type="button" aria-pressed={scope === "project"} onClick={() => setScope("project")}>当前项目</button>
        </div>
        <div className="mcp-config-path" title={current?.path}>
          <FileJson2 size={14} aria-hidden="true" />
          <code>{current?.path ?? "mcp.json"}</code>
          <span>{current?.exists ? "已创建" : "尚未创建"}</span>
        </div>
      </div>

      {(error || view?.overview.error) && (
        <div className="settings-error mcp-global-error" role="alert">
          <AlertCircle size={15} aria-hidden="true" />
          <span>{error || view?.overview.error}</span>
        </div>
      )}

      {loading && !documents ? (
        <div className="mcp-loading" role="status">
          <LoaderCircle className="spin" size={18} />
          正在读取配置
        </div>
      ) : current ? (
        <div className="mcp-json-workspace">
          <div className="mcp-json-heading">
            <div>
              <Braces size={15} aria-hidden="true" />
              <strong>mcp.json</strong>
            </div>
            <button
              className="secondary-button mcp-format-button"
              type="button"
              disabled={saving}
              onClick={formatRaw}
            >
              <Braces size={14} aria-hidden="true" />
              格式化 JSON
            </button>
          </div>
          <textarea
            aria-label={`${current.path} JSON`}
            aria-invalid={Boolean(current.parseError)}
            value={current.raw}
            wrap="off"
            spellCheck={false}
            onChange={(event) => updateRaw(event.target.value)}
            onKeyDown={(event) => {
              if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
                event.preventDefault();
                void save();
              }
            }}
          />
          {current.parseError && (
            <div className="mcp-json-error" role="alert">
              <AlertCircle size={14} aria-hidden="true" />
              <span>{current.parseError}</span>
            </div>
          )}
          <div className="mcp-json-footer">
            <div className="mcp-json-state" aria-live="polite">
              <span>{currentBytes.toLocaleString()} B / 1 MiB</span>
              <span className={dirty ? "mcp-json-state--dirty" : ""}>{dirty ? "未保存" : "已同步"}</span>
            </div>
            <button
              className="primary-button mcp-save-button"
              type="button"
              disabled={saving || !dirty || Boolean(current.parseError) || currentBytes > MAX_CONFIG_BYTES}
              onClick={() => void save()}
            >
              {saving ? <LoaderCircle className="spin" size={15} /> : <Save size={15} />}
              {saving ? "保存中" : "保存 JSON"}
            </button>
          </div>
        </div>
      ) : null}

      <section className="mcp-runtime-section" aria-labelledby="mcp-runtime-title">
        <div className="mcp-runtime-heading">
          <div>
            <Server size={15} aria-hidden="true" />
            <h4 id="mcp-runtime-title">运行状态</h4>
          </div>
          <span>{view?.overview.mcpServers.length ?? 0}</span>
        </div>
        <div className="mcp-runtime-list">
          {view?.overview.mcpServers.length ? view.overview.mcpServers.map((diagnostic) => (
            <div className="mcp-runtime-server" key={diagnostic.id}>
              <div className="mcp-runtime-summary">
                <div className="mcp-runtime-identity">
                  <strong>{diagnostic.id}</strong>
                  <span>{transportLabel(diagnostic.transport)}</span>
                </div>
                <span className={`mcp-runtime-status mcp-runtime-status--${statusTone(diagnostic)}`}>
                  <StatusIcon diagnostic={diagnostic} />
                  {statusLabel(diagnostic)}
                </span>
              </div>
              {diagnostic.error && (
                <div className="mcp-runtime-error" role="alert">
                  <AlertCircle size={13} aria-hidden="true" />
                  <span>{diagnostic.error}</span>
                </div>
              )}
              {diagnostic.credentials.length > 0 && (
                <div className="mcp-credentials" aria-label={`${diagnostic.id} 系统凭据`}>
                  {diagnostic.credentials.map((credential) => (
                    <McpCredentialEditor
                      key={credential.name}
                      server={diagnostic.id}
                      name={credential.name}
                      configured={credential.configured}
                      onUpdated={updateOverview}
                    />
                  ))}
                </div>
              )}
            </div>
          )) : (
            <div className="mcp-runtime-empty">尚无运行中的 MCP 服务器</div>
          )}
        </div>
      </section>

      {view?.overview.hooks.length ? (
        <details className="mcp-hooks">
          <summary>Hooks <span>{view.overview.hooks.length}</span></summary>
          <div className="mcp-hook-list">
            {view.overview.hooks.map((hook) => (
              <div className="mcp-hook-row" key={hook.id}>
                <div><strong>{hook.id}</strong><small>{hook.phase} · {hook.tool}</small></div>
                <label className="extension-toggle">
                  <input
                    type="checkbox"
                    checked={hook.enabled}
                    disabled={busyHook === hook.id}
                    onChange={(event) => void toggleHook(hook.id, event.target.checked)}
                  />
                  <span>启用</span>
                </label>
              </div>
            ))}
          </div>
        </details>
      ) : null}
    </section>
  );
}

function McpCredentialEditor({
  server,
  name,
  configured,
  onUpdated,
}: {
  server: string;
  name: string;
  configured: boolean;
  onUpdated: (overview: ExtensionOverview) => void;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function save() {
    if (!value) return;
    setBusy(true);
    try {
      onUpdated(await saveMcpSecret(server, name, value));
      setValue("");
      setError("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!window.confirm(`删除 ${server} 的 ${name} 凭据？`)) return;
    setBusy(true);
    try {
      onUpdated(await deleteMcpSecret(server, name));
      setError("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mcp-credential-row">
      <div className="mcp-credential-name">
        <KeyRound size={14} aria-hidden="true" />
        <span>{name}</span>
        <small className={configured ? "mcp-credential-state--ready" : ""}>{configured ? "已配置" : "缺失"}</small>
      </div>
      <input
        type="password"
        value={value}
        autoComplete="off"
        aria-label={`${server} ${name} 凭据`}
        placeholder={configured ? "替换凭据" : "输入凭据"}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") void save();
        }}
      />
      <button
        type="button"
        disabled={busy || !value}
        aria-label={`保存 ${name} 凭据`}
        title="保存凭据"
        onClick={() => void save()}
      >
        <Save size={14} />
      </button>
      {configured && (
        <button
          type="button"
          disabled={busy}
          aria-label={`删除 ${name} 凭据`}
          title="删除凭据"
          onClick={() => void remove()}
        >
          <Trash2 size={14} />
        </button>
      )}
      {error && <small className="mcp-credential-error" role="alert">{error}</small>}
    </div>
  );
}
