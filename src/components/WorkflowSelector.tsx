import { useEffect, useRef, useState } from "react";
import { Bot, Check, ChevronDown } from "lucide-react";
import { getWorkflowSkillReadiness } from "../api/runtime";
import type { WorkflowDefinitionView, WorkflowRunView, WorkflowSkillReadinessView } from "../types/runtime";

interface WorkflowSelectorProps {
  definitions: WorkflowDefinitionView[];
  run: WorkflowRunView | null;
  selectedWorkflowId: string | null;
  standalone: boolean;
  disabled: boolean;
  compact?: boolean;
  onSelect: (workflowId: string | null) => void;
}

export function WorkflowSelector({
  definitions,
  run,
  selectedWorkflowId,
  standalone,
  disabled,
  compact = false,
  onSelect,
}: WorkflowSelectorProps) {
  const [open, setOpen] = useState(false);
  const [readiness, setReadiness] = useState<Record<string, WorkflowSkillReadinessView>>({});
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

  useEffect(() => {
    let disposed = false;
    if (!definitions.length || standalone) return undefined;
    void Promise.all(definitions.map(async (definition) => [
      definition.id,
      await getWorkflowSkillReadiness(definition.id),
    ] as const))
      .then((entries) => {
        if (!disposed) setReadiness(Object.fromEntries(entries));
      })
      .catch(() => {
        if (!disposed) setReadiness({});
      });
    return () => { disposed = true; };
  }, [definitions, standalone]);

  return (
    <div className={`workflow-selector${compact ? " workflow-selector--compact" : ""}`} ref={rootRef}>
      <button
        type="button"
        className="workflow-toggle"
        aria-label="选择机器人"
        aria-expanded={open}
        title={standalone ? "独立会话不能使用项目机器人" : "选择机器人"}
        disabled={unavailable}
        onClick={() => setOpen((value) => !value)}
      >
        <Bot size={15} aria-hidden="true" data-icon="robot" />
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
            const state = readiness[definition.id];
            const blocked = state ? !state.ready : false;
            return (
              <button
                type="button"
                className={`workflow-option ${selected ? "workflow-option--active" : ""}`}
                role="menuitemradio"
                aria-checked={selected}
                disabled={blocked}
                title={blocked ? state.blockers[0]?.message ?? "机器人技能未就绪" : undefined}
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
                  <small className={blocked ? "workflow-option-status workflow-option-status--blocked" : "workflow-option-status"}>
                    {definition.uniqueSkillCount} 个技能 · {definition.nodes.length} 个步骤{blocked ? ` · ${state.blockerCount} 项异常` : ""}
                  </small>
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
