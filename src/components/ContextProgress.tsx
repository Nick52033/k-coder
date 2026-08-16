import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Gauge } from "lucide-react";
import type { TokenUsage } from "../types/runtime";

interface ContextProgressProps {
  usage: TokenUsage | null;
  contextWindow: number | null;
}

type ContextLevel = "normal" | "warning" | "critical" | "unknown";

export function ContextProgress({ usage, contextWindow }: ContextProgressProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [popoverPosition, setPopoverPosition] = useState({ left: 12, bottom: 48, width: 300 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();
  const validContextWindow = contextWindow && Number.isFinite(contextWindow) && contextWindow > 0
    ? contextWindow
    : null;
  const usedTokens = usage && Number.isFinite(usage.totalTokens)
    ? Math.max(0, usage.totalTokens)
    : null;
  const rawPercent = usedTokens !== null && validContextWindow
    ? (usedTokens / validContextWindow) * 100
    : null;
  const displayPercent = rawPercent === null ? null : Math.min(999, Math.max(0, Math.round(rawPercent)));
  const progressPercent = rawPercent === null ? 0 : Math.min(100, Math.max(0, rawPercent));
  const level: ContextLevel = rawPercent === null
    ? "unknown"
    : rawPercent >= 90
      ? "critical"
      : rawPercent >= 70
        ? "warning"
        : "normal";
  const usedLabel = usedTokens === null ? "--" : formatTokenCount(usedTokens);
  const windowLabel = validContextWindow === null ? "--" : formatTokenCount(validContextWindow);
  const percentageLabel = displayPercent === null ? "--" : `${displayPercent}%`;
  const accessibleLabel = displayPercent === null
    ? "上下文用量等待更新"
    : `上下文 ${displayPercent}%，${usedLabel} / ${windowLabel}`;

  function updatePopoverPosition() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const viewportPadding = 12;
    const width = Math.min(300, Math.max(220, window.innerWidth - viewportPadding * 2));
    setPopoverPosition({
      left: Math.max(
        viewportPadding,
        Math.min(rect.right - width, window.innerWidth - width - viewportPadding),
      ),
      bottom: Math.max(viewportPadding, window.innerHeight - rect.top + 7),
      width,
    });
  }

  useEffect(() => {
    if (!isOpen) return;

    function handlePointerDown(event: MouseEvent) {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !popoverRef.current?.contains(target)) {
        setIsOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      setIsOpen(false);
      triggerRef.current?.focus();
    }

    updatePopoverPosition();
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("resize", updatePopoverPosition);
    window.addEventListener("scroll", updatePopoverPosition, true);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("resize", updatePopoverPosition);
      window.removeEventListener("scroll", updatePopoverPosition, true);
    };
  }, [isOpen]);

  return (
    <div className="context-progress" data-level={level}>
      <button
        ref={triggerRef}
        className="context-progress-trigger"
        type="button"
        aria-label={accessibleLabel}
        aria-expanded={isOpen}
        aria-haspopup="dialog"
        aria-controls={isOpen ? popoverId : undefined}
        title="上下文用量"
        onClick={() => {
          if (!isOpen) updatePopoverPosition();
          setIsOpen((open) => !open);
        }}
      >
        <Gauge size={15} strokeWidth={2} aria-hidden="true" />
        <span>{percentageLabel}</span>
      </button>

      {isOpen && createPortal(
        <div
          ref={popoverRef}
          id={popoverId}
          className="context-progress-popover"
          data-level={level}
          role="dialog"
          aria-label="上下文用量"
          style={popoverPosition}
        >
          <div className="context-progress-heading">
            <span className="context-progress-heading-icon" aria-hidden="true">
              <Gauge size={16} strokeWidth={2} />
            </span>
            <div>
              <strong>上下文</strong>
              <span>最近一次模型调用</span>
            </div>
          </div>
          <div className="context-progress-metrics">
            <strong>{usedLabel} / {windowLabel}</strong>
            <span>{percentageLabel}</span>
          </div>
          <div
            className="context-progress-track"
            role="progressbar"
            aria-label="上下文占用"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={displayPercent === null ? undefined : Math.min(100, displayPercent)}
            aria-valuetext={displayPercent === null ? "等待更新" : `${displayPercent}%`}
          >
            <span style={{ width: `${progressPercent}%` }} />
          </div>
          <p>{contextStatusText(level, usedTokens, validContextWindow)}</p>
        </div>,
        document.body,
      )}
    </div>
  );
}

function formatTokenCount(tokens: number) {
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return Math.round(tokens).toLocaleString("zh-CN");
}

function contextStatusText(
  level: ContextLevel,
  usedTokens: number | null,
  contextWindow: number | null,
) {
  if (!contextWindow) return "当前模型未配置上下文窗口";
  if (usedTokens === null) return "模型返回下一次用量后更新";
  if (level === "critical") return "上下文接近上限，运行时将尽快压缩";
  if (level === "warning") return "上下文较长，运行时将按需压缩";
  return `约剩余 ${formatTokenCount(Math.max(0, contextWindow - usedTokens))} 可用`;
}
