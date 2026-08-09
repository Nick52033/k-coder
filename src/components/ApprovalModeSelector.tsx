import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Hand, ShieldCheck } from "lucide-react";
import type { ApprovalMode } from "../types/runtime";

interface ApprovalModeSelectorProps {
  mode: ApprovalMode;
  disabled: boolean;
  onChange: (mode: ApprovalMode) => Promise<boolean>;
}

export function ApprovalModeSelector({ mode, disabled, onChange }: ApprovalModeSelectorProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const closeFromPointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", closeFromPointer);
    document.addEventListener("keydown", closeFromKeyboard);
    return () => {
      document.removeEventListener("pointerdown", closeFromPointer);
      document.removeEventListener("keydown", closeFromKeyboard);
    };
  }, [open]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  async function select(nextMode: ApprovalMode) {
    setOpen(false);
    if (nextMode !== mode) await onChange(nextMode);
  }

  const fullAccess = mode === "full_access";
  return (
    <div className="approval-mode-selector" ref={rootRef}>
      <button
        type="button"
        className={`approval-mode-trigger ${fullAccess ? "approval-mode-trigger--full" : ""}`}
        aria-label={`操作批准方式：${fullAccess ? "完整访问" : "请求批准"}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={fullAccess ? "完整访问：自动批准工具操作" : "请求批准：敏感操作前先询问"}
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
      >
        {fullAccess ? <ShieldCheck size={16} /> : <Hand size={16} />}
        {fullAccess ? <span>完整访问</span> : null}
        <ChevronDown size={14} className="approval-mode-chevron" />
      </button>
      {open && (
        <div className="approval-mode-menu" role="menu" aria-label="操作批准方式">
          <div className="approval-mode-menu-title">如何批准智能体操作？</div>
          <button
            type="button"
            className="approval-mode-option"
            role="menuitemradio"
            aria-checked={mode === "ask"}
            onClick={() => void select("ask")}
          >
            <Hand size={18} />
            <span>
              <strong>请求批准</strong>
              <small>修改文件和外部访问前先询问</small>
            </span>
            {mode === "ask" && <Check size={17} />}
          </button>
          <button
            type="button"
            className="approval-mode-option approval-mode-option--full"
            role="menuitemradio"
            aria-checked={mode === "full_access"}
            onClick={() => void select("full_access")}
          >
            <ShieldCheck size={18} />
            <span>
              <strong>完整访问</strong>
              <small>自动批准工具，工作区和运行时边界仍生效</small>
            </span>
            {mode === "full_access" && <Check size={17} />}
          </button>
        </div>
      )}
    </div>
  );
}
