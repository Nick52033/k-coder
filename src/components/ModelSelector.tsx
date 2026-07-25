import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, ServerCog } from "lucide-react";
import type { ProviderConfigView, SaveProviderConfigRequest } from "../types/runtime";

interface ModelSelectorProps {
  provider: ProviderConfigView | null;
  providers: ProviderConfigView[];
  activeProviderId: string | null;
  onSaveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
  onActivateProvider: (providerId: string) => Promise<boolean>;
}

export function ModelSelector({
  provider,
  providers,
  activeProviderId,
  onSaveProvider,
  onActivateProvider,
}: ModelSelectorProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ left: 20, bottom: 40, width: 320 });
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const pendingFocusIndex = useRef<number | null>(null);

  const currentModelId = provider?.model || "";
  const providerName = provider?.name || "未配置供应商";
  const options = useMemo(
    () => provider
      ? provider.models.filter((model, index, models) =>
          Boolean(model.id.trim()) && models.findIndex((candidate) => candidate.id === model.id) === index,
        )
      : [],
    [provider],
  );
  const currentModel = options.find((model) => model.id === currentModelId) ?? options[0];
  const showCurrentModelId = currentModel
    ? currentModel.displayName.trim().toLocaleLowerCase() !== currentModel.id.trim().toLocaleLowerCase()
    : false;
  const selectedIndex = Math.max(0, options.findIndex((model) => model.id === currentModelId));

  function closeMenu({ restoreFocus = false } = {}) {
    setIsOpen(false);
    pendingFocusIndex.current = null;
    if (restoreFocus) {
      requestAnimationFrame(() => triggerRef.current?.focus());
    }
  }

  function updateMenuPosition() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(320, window.innerWidth - 40);
    setMenuPosition({
      left: Math.max(20, Math.min(rect.left, window.innerWidth - width - 20)),
      bottom: Math.max(12, window.innerHeight - rect.top + 4),
      width,
    });
  }

  function openMenu(focusIndex = selectedIndex) {
    updateMenuPosition();
    pendingFocusIndex.current = Math.max(0, focusIndex);
    setIsOpen(true);
  }

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        closeMenu();
      }
    }

    function handleViewportChange() {
      updateMenuPosition();
    }

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      window.addEventListener("resize", handleViewportChange);
      window.addEventListener("scroll", handleViewportChange, true);
      return () => {
        document.removeEventListener("mousedown", handleClickOutside);
        window.removeEventListener("resize", handleViewportChange);
        window.removeEventListener("scroll", handleViewportChange, true);
      };
    }
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || pendingFocusIndex.current === null) return;
    const index = pendingFocusIndex.current;
    pendingFocusIndex.current = null;
    requestAnimationFrame(() => optionRefs.current[index]?.focus());
  }, [isOpen]);

  function handleTriggerKeyDown(event: React.KeyboardEvent<HTMLButtonElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openMenu(selectedIndex);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      openMenu(options.length - 1);
    } else if (event.key === "Escape" && isOpen) {
      event.preventDefault();
      closeMenu({ restoreFocus: true });
    }
  }

  function handleMenuKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const currentIndex = optionRefs.current.findIndex((option) => option === document.activeElement);
    let nextIndex: number | null = null;

    if (event.key === "ArrowDown") {
      nextIndex = (Math.max(currentIndex, -1) + 1) % options.length;
    } else if (event.key === "ArrowUp") {
      nextIndex = (currentIndex <= 0 ? options.length : currentIndex) - 1;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = options.length - 1;
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeMenu({ restoreFocus: true });
      return;
    }

    if (nextIndex !== null) {
      event.preventDefault();
      optionRefs.current[nextIndex]?.focus();
    }
  }

  async function handleSelectModel(modelId: string) {
    if (!provider || switching || modelId === provider.model) {
      closeMenu({ restoreFocus: true });
      return;
    }

    setSwitching(true);
    const success = await onSaveProvider({
      id: provider.id,
      kind: provider.kind,
      transport: provider.transport,
      name: provider.name,
      baseUrl: provider.baseUrl,
      model: modelId,
      models: provider.models,
      endpoints: provider.endpoints,
      activate: true,
    });

    setSwitching(false);
    if (success) {
      closeMenu({ restoreFocus: true });
    }
  }

  async function handleSelectProvider(providerId: string) {
    if (switching || providerId === activeProviderId) return;
    setSwitching(true);
    const success = await onActivateProvider(providerId);
    setSwitching(false);
    if (success) closeMenu({ restoreFocus: true });
  }

  return (
    <div className="model-selector" ref={rootRef}>
      <button
        ref={triggerRef}
        className="model-selector-trigger"
        type="button"
        onClick={() => isOpen ? closeMenu() : openMenu()}
        onKeyDown={handleTriggerKeyDown}
        disabled={!provider || switching}
        aria-label="选择模型"
        aria-haspopup="listbox"
        aria-expanded={isOpen}
        aria-controls={isOpen ? "model-selector-options" : undefined}
        title={provider ? "切换模型" : "请先在设置中配置模型"}
      >
        <span className="model-selector-current">
          <small>{providerName}</small>
          <span aria-hidden="true">/</span>
          <strong>{currentModel?.displayName || "未配置模型"}</strong>
          {showCurrentModelId && <em>{currentModel?.id}</em>}
        </span>
        <ChevronDown className="model-selector-chevron" size={14} aria-hidden="true" />
      </button>

      {provider && isOpen && createPortal(
        <div
          className="model-selector-menu"
          ref={menuRef}
          style={menuPosition}
          onKeyDown={handleMenuKeyDown}
        >
          <div className="model-selector-menu-header">模型供应商</div>
          <div className="model-selector-provider-options" aria-label="可用模型供应商">
            {providers.map((candidate) => {
              const isActive = candidate.id === activeProviderId;
              return (
                <button
                  className={`model-selector-provider ${isActive ? "model-selector-provider--active" : ""}`}
                  type="button"
                  key={candidate.id}
                  aria-pressed={isActive}
                  disabled={switching || !candidate.hasApiKey}
                  title={candidate.hasApiKey ? `切换到 ${candidate.name}` : `${candidate.name} 需要配置 API Key`}
                  onClick={() => void handleSelectProvider(candidate.id)}
                >
                  <ServerCog size={16} aria-hidden="true" />
                  <span>
                    <strong>{candidate.name}</strong>
                    <small>{candidate.hasApiKey ? `${candidate.models.length} 个模型` : "需要 API Key"}</small>
                  </span>
                  {isActive && <Check size={16} aria-hidden="true" />}
                </button>
              );
            })}
          </div>
          <div className="model-selector-menu-header">模型</div>
          <div className="model-selector-options" id="model-selector-options" role="listbox" aria-label="可用模型">
            {options.map((model, index) => (
              <button
                ref={(element) => { optionRefs.current[index] = element; }}
                key={model.id}
                className={`model-selector-option ${model.id === currentModelId ? "model-selector-option--active" : ""}`}
                type="button"
                role="option"
                aria-selected={model.id === currentModelId}
                onClick={() => handleSelectModel(model.id)}
                disabled={switching}
              >
                <div className="model-selector-option-content">
                  <div className="model-selector-option-main">
                    <strong>{model.displayName}</strong>
                  </div>
                  <small>{model.id} · {formatContextWindow(model.contextWindow)} 上下文</small>
                </div>
                {model.id === currentModelId && (
                  <Check size={16} style={{ color: "var(--color-brand)" }} />
                )}
              </button>
            ))}
          </div>
        </div>,
        document.querySelector(".app-shell") ?? document.body,
      )}
    </div>
  );
}

function formatContextWindow(tokens: number) {
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return String(tokens);
}
