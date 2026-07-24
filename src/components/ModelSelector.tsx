import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ServerCog } from "lucide-react";
import type { ProviderConfigView, SaveProviderConfigRequest } from "../types/runtime";

interface ModelSelectorProps {
  provider: ProviderConfigView | null;
  onSaveProvider: (request: SaveProviderConfigRequest) => Promise<boolean>;
}

export function ModelSelector({ provider, onSaveProvider }: ModelSelectorProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
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
  const selectedIndex = Math.max(0, options.findIndex((model) => model.id === currentModelId));

  function closeMenu({ restoreFocus = false } = {}) {
    setIsOpen(false);
    pendingFocusIndex.current = null;
    if (restoreFocus) {
      requestAnimationFrame(() => triggerRef.current?.focus());
    }
  }

  function openMenu(focusIndex = selectedIndex) {
    pendingFocusIndex.current = Math.max(0, focusIndex);
    setIsOpen(true);
  }

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        closeMenu();
      }
    }

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      return () => document.removeEventListener("mousedown", handleClickOutside);
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
      kind: provider.kind,
      transport: provider.transport,
      name: provider.name,
      baseUrl: provider.baseUrl,
      model: modelId,
      models: provider.models,
      endpoints: provider.endpoints,
    });

    setSwitching(false);
    if (success) {
      closeMenu({ restoreFocus: true });
    }
  }

  return (
    <div className="model-selector" ref={menuRef}>
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
          {currentModel && <em>{currentModel.id}</em>}
        </span>
        <ChevronDown className="model-selector-chevron" size={14} aria-hidden="true" />
      </button>

      {provider && isOpen && (
        <div className="model-selector-menu" onKeyDown={handleMenuKeyDown}>
          <div className="model-selector-provider">
            <ServerCog size={16} aria-hidden="true" />
            <span>
              <small>模型供应商</small>
              <strong>{provider.name}</strong>
            </span>
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
        </div>
      )}
    </div>
  );
}

function formatContextWindow(tokens: number) {
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return String(tokens);
}
