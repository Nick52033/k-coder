import { useEffect, useState } from "react";
import {
  AlertCircle,
  Boxes,
  LoaderCircle,
  Puzzle,
  RefreshCw,
  Server,
  Sparkles,
  Trash2,
} from "lucide-react";
import { deletePlugin, getPluginOverview, setPluginEnabled } from "../api/runtime";
import type { PluginDiagnostic, PluginOverview, PluginState } from "../types/runtime";
import "./PluginSettingsPage.css";

const stateLabels: Record<PluginState, string> = {
  disabled: "未启用",
  loaded: "已加载",
  degraded: "部分可用",
  blocked: "已阻止",
  invalid: "无效",
};

function messageFromError(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return "插件操作失败";
}

function componentSummary(plugin: PluginDiagnostic) {
  const { components } = plugin;
  return (
    <div className="plugin-components" aria-label={`${plugin.name} 组件`}>
      <span title="Skills"><Sparkles size={12} />{components.skillCount} Skills</span>
      <span title="MCP 服务器"><Server size={12} />{components.mcpServerCount} MCP</span>
      <span title="MCP 工具"><Boxes size={12} />{components.mcpToolCount} 工具</span>
      {components.unsupportedCount > 0 && (
        <span className="plugin-components-unsupported">
          {components.unsupportedCount} 项不支持
        </span>
      )}
    </div>
  );
}

export function PluginSettingsPage() {
  const [overview, setOverview] = useState<PluginOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [pendingDelete, setPendingDelete] = useState<PluginDiagnostic | null>(null);

  async function load(refresh: boolean) {
    setLoading(true);
    setError("");
    try {
      setOverview(await getPluginOverview(refresh));
    } catch (loadError) {
      setError(messageFromError(loadError));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load(true);
  }, []);

  async function handleToggle(plugin: PluginDiagnostic, enabled: boolean) {
    setBusyId(plugin.id);
    setError("");
    try {
      setOverview(await setPluginEnabled(plugin.id, enabled));
    } catch (toggleError) {
      setError(messageFromError(toggleError));
      try {
        setOverview(await getPluginOverview(true));
      } catch (refreshError) {
        setError(`${messageFromError(toggleError)}；刷新失败：${messageFromError(refreshError)}`);
      }
    } finally {
      setBusyId(null);
    }
  }

  async function handleDelete() {
    if (!pendingDelete) return;
    const plugin = pendingDelete;
    setBusyId(plugin.id);
    setError("");
    try {
      setOverview(await deletePlugin(plugin.id));
      setPendingDelete(null);
    } catch (deleteError) {
      setError(messageFromError(deleteError));
      try {
        setOverview(await getPluginOverview(true));
      } catch {
        // Preserve the last backend facts while reporting the destructive-operation error.
      }
    } finally {
      setBusyId(null);
    }
  }

  const plugins = overview?.plugins ?? [];

  return (
    <section className="settings-page plugin-settings-page" aria-labelledby="plugin-page-title">
      <div className="settings-page-header plugin-page-header">
        <div>
          <p className="settings-eyebrow">扩展</p>
          <h3 id="plugin-page-title">本地插件</h3>
        </div>
        <button
          className="plugin-icon-button"
          type="button"
          aria-label="刷新插件"
          title="刷新插件"
          disabled={loading || busyId !== null}
          onClick={() => void load(true)}
        >
          {loading ? <LoaderCircle className="plugin-spin" size={16} /> : <RefreshCw size={16} />}
        </button>
      </div>

      <div className="plugin-root" title={overview?.rootPath ?? ""}>
        <Puzzle size={15} />
        <span>{overview?.rootPath ?? "正在读取插件目录..."}</span>
      </div>

      {(error || overview?.error) && (
        <div className="plugin-alert" role="alert">
          <AlertCircle size={15} />
          <span>{error || overview?.error}</span>
        </div>
      )}

      {!loading && plugins.length === 0 ? (
        <div className="plugin-empty">
          <Puzzle size={22} />
          <span>未发现插件</span>
        </div>
      ) : (
        <div className="plugin-list" aria-label="本地插件列表">
          {plugins.map((plugin) => {
            const busy = busyId === plugin.id;
            const toggleDisabled = busyId !== null || plugin.state === "invalid";
            return (
              <article className={`plugin-row plugin-row--${plugin.state}`} key={`${plugin.id}:${plugin.path}`}>
                <div className="plugin-row-icon" aria-hidden="true">
                  <Puzzle size={17} />
                </div>
                <div className="plugin-row-body">
                  <div className="plugin-row-heading">
                    <strong title={plugin.name}>{plugin.name}</strong>
                    {plugin.version && <span className="plugin-version">v{plugin.version}</span>}
                    <span className={`plugin-state plugin-state--${plugin.state}`}>
                      {stateLabels[plugin.state]}
                    </span>
                  </div>
                  {plugin.description && <p className="plugin-description">{plugin.description}</p>}
                  <div className="plugin-path" title={plugin.path}>{plugin.path}</div>
                  {componentSummary(plugin)}
                  {(plugin.warnings.length > 0 || plugin.error) && (
                    <div className="plugin-diagnostics">
                      {plugin.warnings.map((warning) => <span key={warning}>{warning}</span>)}
                      {plugin.error && <span className="plugin-diagnostic-error">{plugin.error}</span>}
                    </div>
                  )}
                </div>
                <div className="plugin-row-actions">
                  <label className="plugin-switch" title={toggleDisabled ? "此插件不能启用" : undefined}>
                    <input
                      type="checkbox"
                      aria-label={`启用 ${plugin.name}`}
                      checked={plugin.enabled}
                      disabled={toggleDisabled}
                      onChange={(event) => void handleToggle(plugin, event.currentTarget.checked)}
                    />
                    <span aria-hidden="true" />
                  </label>
                  <button
                    className="plugin-delete-button"
                    type="button"
                    aria-label={`删除 ${plugin.name}`}
                    title={plugin.deletable ? "删除插件" : "当前插件无法安全删除"}
                    disabled={busyId !== null || !plugin.deletable}
                    onClick={() => setPendingDelete(plugin)}
                  >
                    {busy ? <LoaderCircle className="plugin-spin" size={15} /> : <Trash2 size={15} />}
                  </button>
                </div>
              </article>
            );
          })}
        </div>
      )}

      {pendingDelete && (
        <div className="plugin-confirm-backdrop" role="presentation">
          <section
            className="plugin-confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="plugin-delete-title"
          >
            <div>
              <h4 id="plugin-delete-title">删除插件</h4>
              <strong>{pendingDelete.name}</strong>
            </div>
            <p title={pendingDelete.path}>{pendingDelete.path}</p>
            <div className="plugin-confirm-actions">
              <button
                className="secondary-button"
                type="button"
                disabled={busyId !== null}
                onClick={() => setPendingDelete(null)}
              >
                取消
              </button>
              <button
                className="danger-button"
                type="button"
                disabled={busyId !== null}
                onClick={() => void handleDelete()}
              >
                {busyId === pendingDelete.id && <LoaderCircle className="plugin-spin" size={15} />}
                删除
              </button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}
