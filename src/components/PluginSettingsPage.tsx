import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Boxes,
  ChevronDown,
  Download,
  FolderOpen,
  LoaderCircle,
  Plus,
  Puzzle,
  RefreshCw,
  Server,
  Sparkles,
  Store,
  Trash2,
} from "lucide-react";
import {
  addPluginMarketplace,
  deletePlugin,
  getPluginMarketplaceOverview,
  getPluginOverview,
  installMarketplacePlugin,
  installPlugin,
  removePluginMarketplace,
  setPluginEnabled,
} from "../api/runtime";
import type {
  MarketplaceSourceKind,
  PluginDiagnostic,
  PluginMarketplaceEntryView,
  PluginMarketplaceOverview,
  PluginMarketplaceView,
  PluginOverview,
  PluginScope,
  PluginSourceKind,
  PluginState,
} from "../types/runtime";
import "./PluginSettingsPage.css";

const stateLabels: Record<PluginState, string> = {
  disabled: "未启用",
  loaded: "已加载",
  degraded: "部分可用",
  blocked: "已阻止",
  invalid: "无效",
};

const scopeLabels: Record<PluginScope, string> = {
  builtin: "内置",
  local: "本地",
  project: "项目",
};

const marketplaceSourceKindLabels: Record<MarketplaceSourceKind, string> = {
  http: "HTTP 清单",
  git: "Git 仓库",
  local_directory: "本地目录",
};

const pluginSourceKindLabels: Record<PluginSourceKind, string> = {
  github: "GitHub",
  git: "Git",
  url: "ZIP 下载",
  directory: "本地目录",
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
  const [marketplace, setMarketplace] = useState<PluginMarketplaceOverview | null>(null);
  const [marketplaceLoading, setMarketplaceLoading] = useState(true);
  const [marketplaceSource, setMarketplaceSource] = useState("");

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

  async function loadMarketplace() {
    setMarketplaceLoading(true);
    try {
      setMarketplace(await getPluginMarketplaceOverview(false));
    } catch (marketplaceError) {
      setError(messageFromError(marketplaceError));
    } finally {
      setMarketplaceLoading(false);
    }
  }

  useEffect(() => {
    void load(true);
    void loadMarketplace();
  }, []);

  async function handleToggle(plugin: PluginDiagnostic, enabled: boolean) {
    setBusyId(`${plugin.scope}:${plugin.id}`);
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
    setBusyId(`${plugin.scope}:${plugin.id}`);
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

  async function handleAddMarketplace() {
    const source = marketplaceSource.trim();
    if (!source) return;
    setBusyId("add-marketplace");
    setError("");
    try {
      setMarketplace(await addPluginMarketplace(source));
      setMarketplaceSource("");
    } catch (addError) {
      setError(messageFromError(addError));
      try {
        setMarketplace(await getPluginMarketplaceOverview(false));
      } catch {
        // Keep the last known marketplaces while surfacing the add failure.
      }
    } finally {
      setBusyId(null);
    }
  }

  async function handleRemoveMarketplace(marketplaceRow: PluginMarketplaceView) {
    setBusyId(`remove-marketplace:${marketplaceRow.id}`);
    setError("");
    try {
      setMarketplace(await removePluginMarketplace(marketplaceRow.id));
    } catch (removeError) {
      setError(messageFromError(removeError));
      try {
        setMarketplace(await getPluginMarketplaceOverview(false));
      } catch {
        // Keep the last known marketplaces while surfacing the remove failure.
      }
    } finally {
      setBusyId(null);
    }
  }

  async function handleInstallMarketplacePlugin(entry: PluginMarketplaceEntryView, scope: PluginScope) {
    setBusyId(`install-marketplace:${entry.marketplaceId}:${entry.name}:${scope}`);
    setError("");
    try {
      setOverview(await installMarketplacePlugin(entry.marketplaceId, entry.name, scope));
      await loadMarketplace();
    } catch (installError) {
      setError(messageFromError(installError));
      try {
        setOverview(await getPluginOverview(true));
        setMarketplace(await getPluginMarketplaceOverview(false));
      } catch (refreshError) {
        setError(`${messageFromError(installError)}；刷新失败：${messageFromError(refreshError)}`);
      }
    } finally {
      setBusyId(null);
    }
  }

  async function handleInstall(scope: PluginScope) {
    let selected: string | string[] | null;
    try {
      selected = await open({ directory: true, multiple: false, title: `选择要安装到${scopeLabels[scope]}的插件文件夹` });
    } catch (dialogError) {
      setError(messageFromError(dialogError));
      return;
    }
    if (typeof selected !== "string" || !selected.trim()) return;
    const busyKey = `install:${scope}`;
    setBusyId(busyKey);
    setError("");
    try {
      setOverview(await installPlugin(selected, scope));
    } catch (installError) {
      setError(messageFromError(installError));
      try {
        setOverview(await getPluginOverview(true));
      } catch (refreshError) {
        setError(`${messageFromError(installError)}；刷新失败：${messageFromError(refreshError)}`);
      }
    } finally {
      setBusyId(null);
    }
  }

  const plugins = overview?.plugins ?? [];
  const marketplaces = marketplace?.marketplaces ?? [];

  return (
    <section className="settings-page plugin-settings-page" aria-labelledby="plugin-page-title">
      <div className="settings-page-header plugin-page-header">
        <div>
          <p className="settings-eyebrow">扩展</p>
          <h3 id="plugin-page-title">本地插件</h3>
          <p className="plugin-page-description">内置插件随应用安装并始终启用；安装到本地的插件可用于所有项目，安装到项目的插件只在当前项目生效。添加插件市场后可以直接安装远程市场的插件，安装后默认停用。</p>
        </div>
        <div className="plugin-header-actions">
          <button
            className="plugin-icon-button"
            type="button"
            aria-label="安装到本地"
            title="安装到本地"
            disabled={loading || busyId !== null}
            onClick={() => void handleInstall("local")}
          >
            <FolderOpen size={16} />
            <span>安装到本地</span>
          </button>
          <button
            className="plugin-icon-button"
            type="button"
            aria-label="安装到项目"
            title="安装到项目"
            disabled={loading || busyId !== null}
            onClick={() => void handleInstall("project")}
          >
            <FolderOpen size={16} />
            <span>安装到项目</span>
          </button>
          <button
            className="plugin-icon-button"
            type="button"
            aria-label="刷新插件"
            title="刷新插件"
            disabled={loading || busyId !== null}
            onClick={() => {
              void load(true);
              void loadMarketplace();
            }}
          >
            {loading ? <LoaderCircle className="plugin-spin" size={16} /> : <RefreshCw size={16} />}
            <span>刷新</span>
          </button>
        </div>
      </div>

      <div className="plugin-roots">
        {([
          ["builtin", "内置插件目录", overview?.builtinRootPath],
          ["local", "本地插件目录", overview?.localRootPath],
          ["project", "项目插件目录", overview?.projectRootPath ?? overview?.rootPath],
        ] as const).map(([scope, label, path]) => (
          <div className="plugin-root" key={scope} title={path ?? ""}>
            <FolderOpen size={18} aria-hidden="true" />
            <div>
              <span className="plugin-root-label">{label}</span>
              <span className="plugin-root-path">{path ?? (loading ? "正在读取插件目录..." : "插件目录读取失败")}</span>
            </div>
          </div>
        ))}
      </div>

      <div className="plugin-marketplace">
        <div className="plugin-list-heading">
          <h4>插件市场 <span>{marketplaces.length}</span></h4>
          <span>{marketplace?.entries.length ?? 0} 个可安装插件</span>
        </div>
        <div className="plugin-marketplace-add">
          <input
            className="plugin-marketplace-input"
            type="text"
            value={marketplaceSource}
            aria-label="插件市场来源"
            placeholder="市场来源：HTTP(S) 清单地址、git 仓库、owner/repo 或本地目录"
            disabled={loading || busyId !== null || marketplaceLoading}
            onChange={(event) => setMarketplaceSource(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              void handleAddMarketplace();
            }}
          />
          <button
            className="plugin-icon-button"
            type="button"
            aria-label="添加市场"
            title="添加市场"
            disabled={loading || busyId !== null || marketplaceLoading || !marketplaceSource.trim()}
            onClick={() => void handleAddMarketplace()}
          >
            {busyId === "add-marketplace" ? <LoaderCircle className="plugin-spin" size={16} /> : <Plus size={16} />}
            <span>添加市场</span>
          </button>
        </div>
        <p className="plugin-marketplace-hint">
          市场只是一份 <code>marketplace.json</code> 清单，插件来源支持 GitHub、Git 仓库、ZIP 下载与本地目录；安装后默认停用，需要在下方「已发现」列表中启用。
        </p>
        {marketplaceLoading && !marketplace ? (
          <div className="plugin-marketplace-empty" role="status">
            <LoaderCircle className="plugin-spin" size={20} />
            <span>正在读取已添加的市场…</span>
          </div>
        ) : marketplaces.length === 0 ? (
          <div className="plugin-marketplace-empty">
            <Store size={22} aria-hidden="true" />
            <span>尚未添加市场。填入 <code>marketplace.json</code> 的 HTTP(S) 地址或 Git 仓库后点击「添加市场」。</span>
          </div>
        ) : (
          <div className="plugin-marketplace-list" aria-label="已添加的市场">
            {marketplaces.map((marketplaceRow) => {
              const entries = (marketplace?.entries ?? []).filter((entry) => entry.marketplaceId === marketplaceRow.id);
              const removing = busyId === `remove-marketplace:${marketplaceRow.id}`;
              return (
                <article className="plugin-marketplace-row" key={marketplaceRow.id}>
                  <div className="plugin-marketplace-row-heading">
                    <Store size={16} aria-hidden="true" />
                    <strong title={marketplaceRow.label}>{marketplaceRow.label}</strong>
                    <span className="plugin-marketplace-kind">{marketplaceSourceKindLabels[marketplaceRow.sourceKind]}</span>
                    <span className="plugin-marketplace-count">{marketplaceRow.entryCount} 个插件</span>
                    <span className="plugin-marketplace-source" title={marketplaceRow.sourceDisplay}>
                      {marketplaceRow.sourceDisplay}
                    </span>
                    <button
                      className="plugin-delete-button"
                      type="button"
                      aria-label={`移除市场 ${marketplaceRow.label}`}
                      title="移除市场"
                      disabled={busyId !== null}
                      onClick={() => void handleRemoveMarketplace(marketplaceRow)}
                    >
                      {removing ? <LoaderCircle className="plugin-spin" size={15} /> : <Trash2 size={15} />}
                    </button>
                  </div>
                  {marketplaceRow.error && (
                    <div className="plugin-marketplace-error" role="alert">
                      <AlertCircle size={14} aria-hidden="true" />
                      <span>{marketplaceRow.error}</span>
                    </div>
                  )}
                  {entries.length === 0 ? (
                    <p className="plugin-marketplace-row-empty">
                      {marketplaceRow.error ? "市场不可用，暂时无法列出插件。" : "此市场暂无可安装插件。"}
                    </p>
                  ) : (
                    <ul className="plugin-marketplace-entries">
                      {entries.map((entry) => {
                        const localInstalled = entry.installedScopes.includes("local") || entry.installedScopes.includes("builtin");
                        const projectInstalled = entry.installedScopes.includes("project");
                        return (
                          <li className="plugin-marketplace-entry" key={`${entry.marketplaceId}:${entry.name}`}>
                            <div className="plugin-marketplace-entry-heading">
                              <Puzzle size={14} aria-hidden="true" />
                              <strong title={entry.name}>{entry.name}</strong>
                              {entry.version && <span className="plugin-version">v{entry.version}</span>}
                              <span className="plugin-marketplace-kind">{pluginSourceKindLabels[entry.sourceKind]}</span>
                              {entry.installedScopes.map((scope) => (
                                <span className="plugin-marketplace-installed" key={scope}>
                                  已安装到{scopeLabels[scope]}
                                </span>
                              ))}
                            </div>
                            {entry.description && <p className="plugin-marketplace-entry-description">{entry.description}</p>}
                            <div className="plugin-marketplace-entry-meta">
                              <span title={entry.sourceDisplay}>{entry.sourceDisplay}</span>
                              {entry.dependencies.length > 0 && <span>依赖：{entry.dependencies.join("、")}</span>}
                            </div>
                            <div className="plugin-marketplace-entry-actions">
                              <button
                                className={`plugin-icon-button${localInstalled ? " plugin-marketplace-installed-action" : ""}`}
                                type="button"
                                aria-label={`安装 ${entry.name} 到本地`}
                                title={localInstalled ? "已安装到本地" : "安装到本地"}
                                disabled={busyId !== null || localInstalled}
                                onClick={() => void handleInstallMarketplacePlugin(entry, "local")}
                              >
                                {busyId === `install-marketplace:${entry.marketplaceId}:${entry.name}:local` ? (
                                  <LoaderCircle className="plugin-spin" size={14} />
                                ) : (
                                  <Download size={14} />
                                )}
                                <span>{localInstalled ? "已在本地" : "安装到本地"}</span>
                              </button>
                              <button
                                className={`plugin-icon-button${projectInstalled ? " plugin-marketplace-installed-action" : ""}`}
                                type="button"
                                aria-label={`安装 ${entry.name} 到项目`}
                                title={projectInstalled ? "已安装到项目" : "安装到项目"}
                                disabled={busyId !== null || projectInstalled}
                                onClick={() => void handleInstallMarketplacePlugin(entry, "project")}
                              >
                                {busyId === `install-marketplace:${entry.marketplaceId}:${entry.name}:project` ? (
                                  <LoaderCircle className="plugin-spin" size={14} />
                                ) : (
                                  <Download size={14} />
                                )}
                                <span>{projectInstalled ? "已在项目" : "安装到项目"}</span>
                              </button>
                            </div>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </article>
              );
            })}
          </div>
        )}
      </div>

      {(error || overview?.error) && (
        <div className="plugin-alert" role="alert">
          <AlertCircle size={15} />
          <span>{error || overview?.error}</span>
        </div>
      )}

      {overview && (
        <div className="plugin-list-heading">
          <h4>已发现 <span>{plugins.length}</span></h4>
          <span>{plugins.filter((plugin) => plugin.enabled).length} 个已启用</span>
        </div>
      )}

      {loading && !overview ? (
        <div className="plugin-empty" role="status">
          <LoaderCircle className="plugin-spin" size={24} />
          <span>正在读取插件…</span>
        </div>
      ) : !loading && plugins.length === 0 ? (
        <div className="plugin-empty">
          <div className="plugin-empty-icon"><Puzzle size={28} /></div>
          <strong>未发现插件</strong>
          <p>使用上方按钮选择插件文件夹，或在插件市场中安装远程插件；安装后可在列表中启用、禁用或卸载。</p>
        </div>
      ) : (
        <div className="plugin-list" aria-label="插件列表">
          {plugins.map((plugin) => {
            const pluginBusyId = `${plugin.scope}:${plugin.id}`;
            const busy = busyId === pluginBusyId;
            const toggleDisabled = busyId !== null || plugin.state === "invalid" || plugin.scope === "builtin";
            return (
              <article className={`plugin-row plugin-row--${plugin.state}`} key={`${plugin.id}:${plugin.path}`}>
                <div className="plugin-row-icon" aria-hidden="true">
                  <Puzzle size={20} />
                </div>
                <div className="plugin-row-body">
                  <div className="plugin-row-heading">
                    <strong title={plugin.name}>{plugin.name}</strong>
                    {plugin.version && <span className="plugin-version">v{plugin.version}</span>}
                    <span className="plugin-scope">{scopeLabels[plugin.scope]}</span>
                    <span className={`plugin-state plugin-state--${plugin.state}`}>
                      {stateLabels[plugin.state]}
                    </span>
                  </div>
                  {plugin.description && (plugin.description.length > 160 ? (
                    <details className="plugin-description-details">
                      <summary>
                        <span className="plugin-description plugin-description-preview">{plugin.description}</span>
                        <span className="plugin-description-toggle">
                          <ChevronDown size={13} aria-hidden="true" />
                          <span className="plugin-description-expand">展开说明</span>
                          <span className="plugin-description-collapse">收起说明</span>
                        </span>
                      </summary>
                      <p className="plugin-description">{plugin.description}</p>
                    </details>
                  ) : <p className="plugin-description">{plugin.description}</p>)}
                  {componentSummary(plugin)}
                  <div className="plugin-path" title={plugin.path}>
                    <FolderOpen size={13} aria-hidden="true" />
                    <span>{plugin.path}</span>
                  </div>
                  {(plugin.warnings.length > 0 || plugin.error) && (
                    <div className="plugin-diagnostics">
                      {plugin.warnings.map((warning) => <span key={warning}>{warning}</span>)}
                      {plugin.error && <span className="plugin-diagnostic-error">{plugin.error}</span>}
                    </div>
                  )}
                </div>
                <div className="plugin-row-actions">
                  <label
                    className="plugin-switch"
                    title={plugin.scope === "builtin" ? "内置插件始终启用" : toggleDisabled ? "此插件不能启用" : undefined}
                  >
                    <input
                      type="checkbox"
                      aria-label={`启用 ${plugin.name}`}
                      checked={plugin.enabled}
                      disabled={toggleDisabled}
                      onChange={(event) => void handleToggle(plugin, event.currentTarget.checked)}
                    />
                    <span className="plugin-switch-track" aria-hidden="true" />
                  </label>
                  <button
                    className="plugin-delete-button"
                    type="button"
                    aria-label={`卸载 ${plugin.name}`}
                    title={plugin.deletable ? "卸载插件" : "当前插件无法安全卸载"}
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
              <h4 id="plugin-delete-title">卸载插件</h4>
              <strong>{pendingDelete.name}</strong>
            </div>
            <div className="plugin-confirm-notice">将卸载此插件文件夹及其中的文件。此操作无法撤销。</div>
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
                {busyId === `${pendingDelete.scope}:${pendingDelete.id}` && <LoaderCircle className="plugin-spin" size={15} />}
                卸载
              </button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}
