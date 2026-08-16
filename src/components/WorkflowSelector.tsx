import { useEffect, useRef, useState } from "react";
import { Bot, Check, ChevronDown } from "lucide-react";
import type { WorkflowDefinitionView, WorkflowRunView } from "../types/runtime";

interface WorkflowSelectorProps {
  definitions: WorkflowDefinitionView[];
  run: WorkflowRunView | null;
  selectedWorkflowId: string | null;
  standalone: boolean;
  disabled: boolean;
  onSelect: (workflowId: string | null) => void;
}

export function WorkflowSelector({
  definitions,
  run,
  selectedWorkflowId,
  standalone,
  disabled,
  onSelect,
}: WorkflowSelectorProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const activeWorkflowId = run?.state === "active" ? run.workflowId : selectedWorkflowId;
  const activeDefinition = definitions.find((definition) => definition.id === activeWorkflowId);
  const unavailable = standalone || disabled || run?.state === "active";

  useEffect(() => {
    if (!open) return;
    function closeOnOutsideClick(event: MouseEvent) {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) setOpen(false);
    }
    document.addEventListener("mousedown", closeOnOutsideClick);
    return () => document.removeEventListener("mousedown", closeOnOutsideClick);
  }, [open]);

  return (
    <div className="workflow-selector" ref={rootRef}>
      <button
        type="button"
        className="workflow-toggle"
        aria-label="选择机器人"
        aria-expanded={open}
        title={standalone ? "独立会话不能使用项目机器人" : "选择机器人"}
        disabled={unavailable}
        onClick={() => setOpen((value) => !value)}
      >
        <Bot size={15} />
        <span>{activeDefinition?.name ?? "普通智能体"}</span>
        <ChevronDown size={13} />
      </button>
      {open && (
        <div className="workflow-menu" role="menu" aria-label="机器人列表">
          <button
            type="button"
            className={`workflow-option ${activeWorkflowId === null ? "workflow-option--active" : ""}`}
            role="menuitemradio"
            aria-checked={activeWorkflowId === null}
            onClick={() => {
              onSelect(null);
              setOpen(false);
            }}
          >
            <Bot size={16} />
            <span>
              <strong>普通智能体</strong>
              <small>不使用固定工作流</small>
            </span>
            {activeWorkflowId === null && <Check size={14} />}
          </button>
          {definitions.map((definition) => {
            const selected = definition.id === activeWorkflowId;
            return (
              <button
                type="button"
                className={`workflow-option ${selected ? "workflow-option--active" : ""}`}
                role="menuitemradio"
                aria-checked={selected}
                key={definition.id}
                onClick={() => {
                  onSelect(definition.id);
                  setOpen(false);
                }}
              >
                <Bot size={16} />
                <span>
                  <strong>{definition.name}</strong>
                  <small>{definition.description}</small>
                </span>
                {selected && <Check size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
