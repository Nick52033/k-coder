import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Lightbulb } from "lucide-react";
import type { ReasoningEffort } from "../types/runtime";

interface ReasoningSelectorProps {
  effort: ReasoningEffort;
  disabled?: boolean;
  onChange: (effort: ReasoningEffort) => Promise<boolean>;
}

const options: Array<{ value: ReasoningEffort; label: string }> = [
  { value: "off", label: "关闭" },
  { value: "minimal", label: "最低" },
  { value: "low", label: "低" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
  { value: "x_high", label: "最高" },
];

export function ReasoningSelector({ effort, disabled = false, onChange }: ReasoningSelectorProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = options.find((option) => option.value === effort) ?? options[3];

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  async function select(next: ReasoningEffort) {
    setOpen(false);
    if (next !== effort) await onChange(next);
  }

  return (
    <div className="reasoning-selector" ref={rootRef}>
      <button
        type="button"
        className="reasoning-trigger"
        aria-label="选择推理强度"
        aria-haspopup="menu"
        aria-expanded={open}
        title="设置模型推理强度"
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
      >
        <Lightbulb size={16} aria-hidden="true" />
        <span>推理 {current.label}</span>
        <ChevronDown size={14} className="reasoning-chevron" aria-hidden="true" />
      </button>
      {open && (
        <div className="reasoning-menu" role="menu" aria-label="模型推理强度">
          <div className="reasoning-menu-title">设置模型推理强度（写入全局配置，按供应商兼容字段发送）</div>
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              role="menuitemradio"
              className={`reasoning-option ${option.value === effort ? "reasoning-option--active" : ""}`}
              aria-checked={option.value === effort}
              onClick={() => void select(option.value)}
            >
              <span>{option.label}</span>
              {option.value === effort && <Check size={16} aria-hidden="true" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
