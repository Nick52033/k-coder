import {
  CheckCircle2,
  ChevronDown,
  Circle,
  CircleDot,
  CircleX,
  MinusCircle,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { changeLineStats } from "../lib/diff";
import type { ChangeSet, PlanStepState, PlanView } from "../types/runtime";
import "./PlanProgress.css";

interface PlanProgressProps {
  activeTurn: boolean;
  changes: ChangeSet[];
  plan: PlanView;
  turnId?: string;
}

export function PlanProgress({ activeTurn, changes, plan, turnId }: PlanProgressProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const closeTimerRef = useRef<number | null>(null);
  const pointerInsideRef = useRef(false);
  const pinnedRef = useRef(false);
  const [pinned, setPinned] = useState(false);
  const [visible, setVisible] = useState(false);
  const popoverId = `plan-progress-${useId().replace(/:/g, "")}`;
  const summary = useMemo(() => summarizePlan(plan), [plan]);
  const changeSummary = useMemo(
    () => summarizeChanges(changes, turnId),
    [changes, turnId],
  );

  const positionPopover = useCallback(() => {
    const trigger = triggerRef.current;
    const popover = popoverRef.current;
    if (!trigger || !popover || !popover.matches(":popover-open")) return;

    const edge = 12;
    const gap = 8;
    const triggerRect = trigger.getBoundingClientRect();
    const popoverRect = popover.getBoundingClientRect();
    const left = Math.min(
      Math.max(triggerRect.left, edge),
      Math.max(edge, window.innerWidth - popoverRect.width - edge),
    );
    const topSpace = triggerRect.top - edge;
    const bottomSpace = window.innerHeight - triggerRect.bottom - edge;
    const placeAbove = topSpace >= popoverRect.height + gap || topSpace >= bottomSpace;
    const preferredTop = placeAbove
      ? triggerRect.top - popoverRect.height - gap
      : triggerRect.bottom + gap;
    const top = Math.min(
      Math.max(preferredTop, edge),
      Math.max(edge, window.innerHeight - popoverRect.height - edge),
    );

    popover.style.left = `${Math.round(left)}px`;
    popover.style.top = `${Math.round(top)}px`;
    popover.dataset.placement = placeAbove ? "top" : "bottom";
    popover.style.visibility = "visible";
  }, []);

  const showPopover = useCallback(() => {
    const popover = popoverRef.current;
    if (!popover) return;
    if (!popover.matches(":popover-open")) {
      popover.style.visibility = "hidden";
      try {
        popover.showPopover();
      } catch {
        return;
      }
    }
    setVisible(true);
    window.requestAnimationFrame(positionPopover);
  }, [positionPopover]);

  const hidePopover = useCallback(() => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    pinnedRef.current = false;
    setPinned(false);
    setVisible(false);
    const popover = popoverRef.current;
    if (popover?.matches(":popover-open")) {
      try {
        popover.hidePopover();
      } catch {
        // The top layer may already have dismissed the popover.
      }
    }
  }, []);

  const scheduleHide = useCallback(() => {
    if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;
      if (
        pinnedRef.current
        || pointerInsideRef.current
        || document.activeElement === triggerRef.current
      ) return;
      hidePopover();
    }, 120);
  }, [hidePopover]);

  useEffect(() => {
    if (!visible) return;
    const reposition = () => positionPopover();
    window.addEventListener("resize", reposition);
    document.addEventListener("scroll", reposition, true);
    return () => {
      window.removeEventListener("resize", reposition);
      document.removeEventListener("scroll", reposition, true);
    };
  }, [positionPopover, visible]);

  useEffect(() => {
    if (!visible) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") hidePopover();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [hidePopover, visible]);

  useEffect(() => {
    if (!pinned) return;
    const closeOutside = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      hidePopover();
    };
    document.addEventListener("pointerdown", closeOutside, true);
    return () => document.removeEventListener("pointerdown", closeOutside, true);
  }, [hidePopover, pinned]);

  useEffect(() => () => {
    if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
  }, []);

  function enterProgress(event: ReactPointerEvent<HTMLElement>) {
    if (event.pointerType === "touch") return;
    pointerInsideRef.current = true;
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    showPopover();
  }

  function leaveProgress(event: ReactPointerEvent<HTMLElement>) {
    if (event.pointerType === "touch") return;
    pointerInsideRef.current = false;
    scheduleHide();
  }

  function togglePinned() {
    if (pinnedRef.current) {
      hidePopover();
      return;
    }
    pinnedRef.current = true;
    setPinned(true);
    showPopover();
  }

  return (
    <div
      className={`plan-progress plan-progress--${summary.state}${activeTurn ? " plan-progress--live" : ""}`}
      data-state={summary.state}
    >
      <button
        ref={triggerRef}
        className="plan-progress-trigger"
        type="button"
        aria-controls={popoverId}
        aria-expanded={visible}
        aria-haspopup="dialog"
        aria-label={progressAriaLabel(summary.current, summary.total, summary.completed, changeSummary)}
        onBlur={scheduleHide}
        onClick={togglePinned}
        onFocus={showPopover}
        onPointerEnter={enterProgress}
        onPointerLeave={leaveProgress}
      >
        <span className="plan-progress-position">第 {summary.current}/{summary.total} 步</span>
        {changeSummary.files > 0 ? (
          <>
            <span className="plan-progress-separator" aria-hidden="true">·</span>
            <span className="plan-progress-files">{changeSummary.files} 个文件已更新</span>
            <span className="plan-progress-added">+{changeSummary.added}</span>
            <span className="plan-progress-deleted">-{changeSummary.deleted}</span>
          </>
        ) : null}
        <ChevronDown className="plan-progress-chevron" size={14} aria-hidden="true" />
      </button>

      <div
        ref={popoverRef}
        id={popoverId}
        className="plan-progress-popover"
        popover="manual"
        role="dialog"
        aria-label="执行计划详情"
        onPointerEnter={enterProgress}
        onPointerLeave={leaveProgress}
      >
        <header className="plan-progress-popover-header">
          <strong>执行步骤</strong>
          <span>{summary.completed}/{summary.total} 已完成</span>
        </header>
        <ol className="plan-progress-steps">
          {plan.steps.map((step, index) => (
            <li
              className={`plan-progress-step plan-progress-step--${step.status}`}
              key={step.id}
            >
              <PlanStateIcon status={step.status} />
              <span className="plan-progress-step-copy">
                <strong>{step.step}</strong>
                {step.detail ? <small>{step.detail}</small> : null}
              </span>
              <span className="plan-progress-step-status">
                <span className="plan-progress-step-number">{index + 1}</span>
                {planStatusLabel(step.status)}
              </span>
            </li>
          ))}
        </ol>
      </div>
    </div>
  );
}

function PlanStateIcon({ status }: { status: PlanStepState }) {
  if (status === "completed") return <CheckCircle2 size={16} aria-hidden="true" />;
  if (status === "in_progress") return <CircleDot size={16} aria-hidden="true" />;
  if (status === "failed") return <CircleX size={16} aria-hidden="true" />;
  if (status === "skipped") return <MinusCircle size={16} aria-hidden="true" />;
  return <Circle size={16} aria-hidden="true" />;
}

function summarizePlan(plan: PlanView) {
  const total = plan.steps.length;
  const activeIndex = plan.steps.findIndex((step) => step.status === "in_progress");
  const failedIndex = plan.steps.findIndex((step) => step.status === "failed");
  const pendingIndex = plan.steps.findIndex((step) => step.status === "pending");
  const currentIndex = activeIndex >= 0
    ? activeIndex
    : failedIndex >= 0
      ? failedIndex
      : pendingIndex >= 0
        ? pendingIndex
        : Math.max(0, total - 1);
  const completed = plan.steps.filter((step) => step.status === "completed").length;
  const settled = plan.steps.filter(
    (step) => step.status === "completed" || step.status === "skipped",
  ).length;
  const state = failedIndex >= 0
    ? "failed"
    : total > 0 && settled === total
      ? "completed"
      : activeIndex >= 0
        ? "active"
        : "pending";
  return { completed, current: currentIndex + 1, state, total };
}

function summarizeChanges(changes: ChangeSet[], turnId?: string) {
  const paths = new Set<string>();
  let added = 0;
  let deleted = 0;
  for (const change of changes) {
    if (change.undone || (turnId && change.turnId !== turnId)) continue;
    for (const file of change.files) {
      paths.add(file.destinationPath || file.path);
      const stats = changeLineStats(file.unifiedDiff);
      added += stats.added;
      deleted += stats.deleted;
    }
  }
  return { added, deleted, files: paths.size };
}

function planStatusLabel(status: PlanStepState) {
  if (status === "completed") return "已完成";
  if (status === "in_progress") return "进行中";
  if (status === "failed") return "失败";
  if (status === "skipped") return "已跳过";
  return "待处理";
}

function progressAriaLabel(
  current: number,
  total: number,
  completed: number,
  changes: { added: number; deleted: number; files: number },
) {
  const planLabel = `执行计划：第 ${current}/${total} 步，${completed} 步已完成`;
  if (!changes.files) return planLabel;
  return `${planLabel}，${changes.files} 个文件已更新，新增 ${changes.added} 行，删除 ${changes.deleted} 行`;
}
