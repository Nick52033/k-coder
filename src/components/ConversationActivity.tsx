import {
  Activity,
  Bot,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleDot,
  CircleX,
  Clock3,
  Copy,
  FileText,
  LoaderCircle,
  RotateCcw,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { lazy, memo, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { isProviderCallLimitError } from "../types/runtime";
import type {
  AgentActivityStatus,
  ChangeSet,
  PlanView,
  ToolActivity,
  ToolOutputDelta,
  TimelineEventKind,
  TurnErrorValue,
  TurnTimelineItem,
} from "../types/runtime";
import { changeLineStats } from "../lib/diff";
import { isDisplayableReasoningSummary } from "../lib/reasoningSummary";
import { DELEGATION_TOOL_NAMES, subagentIdsOf } from "../lib/subagent";
import { PlanProgress } from "./PlanProgress";
import { RetryWaitingLabel } from "./RetryWaitingLabel";

const ReadOnlyCodeEditor = lazy(() => import("./CodeEditor").then((module) => ({ default: module.CodeEditor })));
const ChangeCodeDiffEditor = lazy(() => import("./CodeEditor").then((module) => ({ default: module.CodeDiffEditor })));

const HIDDEN_PROCESS_EVENT_KINDS = new Set<TimelineEventKind>([
  "provider_context",
  "usage",
  "approval_requested",
  "approval_resolved",
]);

export function TurnWaitingIndicator({
  activityStatus = null,
  activitySinceMs,
  streamRetry = null,
  retryAtMs,
  onStop,
  cancelling = false,
}: {
  activityStatus?: AgentActivityStatus | null;
  activitySinceMs?: number;
  streamRetry?: { attempt: number; maxAttempts: number } | null;
  retryAtMs?: number;
  onStop?: () => void;
  cancelling?: boolean;
}) {
  const [mountedAtMs] = useState(() => Date.now());
  const copy = activityStatus ? {
    rate_limited: {
      title: "正在等待模型恢复",
      detail: "模型服务暂时限流，k-Coder 会在冷却后自动重试。",
    },
    thinking: {
      title: "正在理解你的请求",
      detail: "任务还在处理中，回复会显示在这里。你可以继续补充要求。",
    },
    responding: {
      title: "正在生成回复",
      detail: "模型还在组织内容，结果会在这里逐步显示。",
    },
    running_tool: {
      title: "正在处理操作",
      detail: "当前操作完成后，k-Coder 会继续回复。",
    },
    awaiting_approval: {
      title: "等待你确认",
      detail: "确认卡片会显示在这段对话中。",
    },
    finalizing: {
      title: "正在整理结果",
      detail: "正在收尾，完成后会把结果显示在这里。",
    },
  }[activityStatus] : {
    title: "正在执行",
    detail: "模型正在处理你的请求，回复会显示在这里。",
  };

  return (
    <div className="turn-waiting" role="status" aria-live="polite">
      <span className="turn-waiting__icon" aria-hidden="true">
        <LoaderCircle size={17} />
      </span>
      <div className="turn-waiting__copy">
        <div className="turn-waiting__heading">
          <strong>{cancelling ? "正在停止" : copy.title}</strong>
          <span className="turn-waiting__elapsed" aria-hidden="true">
            {cancelling ? "等待运行时结束当前操作" : activityStatus === "rate_limited"
              ? <>
                <RetryWaitingLabel retryAtMs={retryAtMs} />
                <span aria-hidden="true"> · </span>
                <ActivityStatusLabel label="已等待" hint={null} sinceMs={activitySinceMs ?? mountedAtMs} />
              </>
              : <ActivityStatusLabel
                label="已等待"
                hint={null}
                sinceMs={activitySinceMs ?? mountedAtMs}
                streamRetry={streamRetry}
              />}
          </span>
        </div>
        <p>{cancelling ? "停止请求已发送，当前操作结束后会更新状态。" : copy.detail}</p>
      </div>
      {onStop && !cancelling ? (
        <button className="turn-waiting__stop" type="button" onClick={onStop}>
          <CircleX size={14} aria-hidden="true" />
          <span>停止</span>
        </button>
      ) : null}
    </div>
  );
}

export const ConversationTurnActivity = memo(function ConversationTurnActivity({
  activities,
  timeline = [],
  changes = [],
  plan,
  turnId,
  workflowPlan = false,
  streaming = false,
  initialTextVisible = false,
  activityStatus = null,
  activitySinceMs,
  streamRetry = null,
  retryAtMs,
  finalMessageId,
  renderText,
  onRetry,
  failureError,
  subagentTaskIndex,
  onFocusSubagent,
  onStop,
  cancelling = false,
}: {
  activities: ToolActivity[];
  timeline?: TurnTimelineItem[];
  changes?: ChangeSet[];
  plan: PlanView | null;
  turnId?: string;
  /** 该计划是否由机器人工作流派生；机器人节点进度不参与普通计划的收尾核对。 */
  workflowPlan?: boolean;
  streaming?: boolean;
  initialTextVisible?: boolean;
  activityStatus?: AgentActivityStatus | null;
  /** 当前活动阶段的开始时间，用于"思考中 · 23s"等待计时。 */
  activitySinceMs?: number;
  /** Provider 流中断后的自动重试进度。 */
  streamRetry?: { attempt: number; maxAttempts: number } | null;
  retryAtMs?: number;
  finalMessageId?: string;
  renderText?: (text: string) => ReactNode;
  onRetry?: () => void;
  /** 终态失败的结构化错误；旧历史允许只提供错误文本。 */
  failureError?: TurnErrorValue | null;
  /** Maps a subagent id to its 1-based `taskN` label within the active thread. */
  subagentTaskIndex?: Record<string, number>;
  onFocusSubagent?: (agentId: string) => void;
  onStop?: () => void;
  cancelling?: boolean;
}) {
  const paced = usePacedTimeline(timeline, streaming, initialTextVisible);
  const visuallyStreaming = streaming || paced.settling;
  const visibleTimeline = visuallyStreaming && !streaming
    ? paced.timeline.filter((item) => item.type !== "event" || !isTerminalEvent(item.kind))
    : paced.timeline;

  if (!activities.length && !timeline.length && !plan?.steps.length && !activityStatus && !streaming) return null;

  const finalResponse = !visuallyStreaming && finalMessageId
    ? visibleTimeline.find((item): item is Extract<TurnTimelineItem, { type: "text" }> => item.type === "text" && item.id === finalMessageId)
    : null;
  const processTimeline = finalResponse ? visibleTimeline.filter((item) => item !== finalResponse) : visibleTimeline;
  const terminalEvent = [...processTimeline].reverse().find(
    (item): item is Extract<TurnTimelineItem, { type: "event" }> => item.type === "event" && isTerminalEvent(item.kind),
  );
  const processItems = processTimeline.filter((item) => item !== terminalEvent && isVisibleConversationTimelineItem(item));
  const turnOutcome: TerminalEventKind | null = terminalEvent && isTerminalEvent(terminalEvent.kind)
    ? terminalEvent.kind
    : null;
  const groupedProcessTimeline = groupConsecutiveTimeline(processItems);
  const hasItems = Boolean(activities.length || processItems.length);
  const hasProcess = hasItems || Boolean(activityStatus) || terminalEvent?.kind === "turn_failed" || terminalEvent?.kind === "turn_cancelled";
  const timelineHasTools = processTimeline.some((item) => item.type === "tool");
  const hasDisplayableReasoning = processItems.some((item) => item.type === "reasoning");
  const hasPublicProgress = Boolean(
    activities.length
      || timelineHasTools
      || processItems.some((item) => item.type === "text" || item.type === "event"),
  );
  const hasVisibleProgress = hasPublicProgress || hasDisplayableReasoning || Boolean(plan?.steps.length);
  if (visuallyStreaming && !hasVisibleProgress && !terminalEvent) {
    return (
      <TurnWaitingIndicator
        activityStatus={activityStatus}
        activitySinceMs={activitySinceMs}
        streamRetry={streamRetry}
        retryAtMs={retryAtMs}
        onStop={onStop}
        cancelling={cancelling}
      />
    );
  }
  const showReasoningUnavailableNotice = Boolean(
    visuallyStreaming
      && activityStatus === "thinking"
      && !terminalEvent
      && !hasDisplayableReasoning
      && hasPublicProgress,
  );
  const toolCount = timelineHasTools
    ? groupedProcessTimeline.reduce(
      (count, entry) => count + (entry.type === "tool_group" ? hideSupersededPatchFailures(entry.activities).length : 0),
      0,
    )
    : hideSupersededPatchFailures(activities).length;
  const summaryTitle = terminalEvent?.durationMs !== undefined
    ? terminalEvent.kind === "turn_cancelled"
      ? "已停止"
      : terminalEvent.kind === "turn_failed"
        ? "请求未完成"
        : `执行了 ${formatDuration(terminalEvent.durationMs)}`
    : terminalEvent?.kind === "turn_cancelled"
      ? "已停止"
      : terminalEvent?.kind === "turn_failed"
        ? "请求未完成"
        : "执行过程";
  const terminalCollapse = !visuallyStreaming && Boolean(terminalEvent);
  const SummaryIcon = visuallyStreaming
    ? LoaderCircle
    : terminalEvent?.kind === "turn_cancelled"
      ? Circle
      : terminalEvent?.kind === "turn_failed"
        ? CircleX
        : terminalEvent?.kind === "turn_completed"
          ? CircleCheck
          : Activity;
  const terminalMeta = [
    toolCount ? `${toolCount} 个操作` : null,
    terminalEvent?.durationMs !== undefined ? `耗时 ${formatDuration(terminalEvent.durationMs)}` : null,
  ].filter(Boolean).join(" · ");
  const summaryStatus = terminalEvent?.kind === "turn_completed"
    ? (toolCount ? `${toolCount} 个操作` : "已完成")
    : terminalEvent
      ? terminalMeta || "已结束"
      : toolCount ? `${toolCount} 个操作` : "处理中";
  const providerCallLimitExceeded = isProviderCallLimitError(failureError);
  const statusLabel = paced.pendingTextIds.size
    ? "生成回复中"
    : activityStatus ? {
      rate_limited: "上游返回 429",
      thinking: "思考中",
      responding: "生成回复中",
      running_tool: "处理工具结果中",
      awaiting_approval: "等待确认",
      finalizing: "整理结果中",
    }[activityStatus] : visuallyStreaming ? "生成回复中" : null;
  // codex 式状态行：阶段文案 + 秒级等待计时；思考时提升最新一条可见推理摘要为动态标题；
  // 流中断自动重试时显示"重连中 n/m"，让"卡了"从猜测变成可见事实。
  const reasoningHint = activityStatus === "thinking" ? latestReasoningHint(visibleTimeline) : null;
  const liveStatusLabel = activityStatus && activityStatus !== "rate_limited" && statusLabel
    ? (
      <ActivityStatusLabel
        label={statusLabel}
        hint={reasoningHint}
        sinceMs={activitySinceMs}
        streamRetry={streamRetry}
      />
    )
    : statusLabel;
  const processContent = (
      <div className="turn-disclosure-panel">
        <div className={visuallyStreaming ? "turn-execution-live" : "turn-execution-content"}>
        {showReasoningUnavailableNotice ? (
          <div className="turn-reasoning-unavailable" role="status">
            <Brain size={15} aria-hidden="true" />
            <span>当前模型未提供可展示的思考摘要，公开进度和工具活动仍会继续显示。</span>
          </div>
        ) : null}
        {processItems.length ? (
          <div className="turn-timeline">
            {groupedProcessTimeline.map((entry) => entry.type === "reasoning_group" ? (
              <ReasoningGroup
                items={entry.items}
                renderText={renderText}
                streaming={visuallyStreaming}
                key={`reasoning-group-${entry.items.map((item) => item.itemId).join("-")}`}
              />
            ) : entry.type === "tool_group" ? (
              <ToolActivityGroup
                activities={entry.activities}
                reasoning={entry.reasoning}
                renderText={renderText}
                key={`tool-group-${entry.activities[0].call.id}`}
                subagentTaskIndex={subagentTaskIndex}
                onFocusSubagent={onFocusSubagent}
              />
            ) : (
              <TimelineItem
                item={entry.item}
                changes={changes}
                renderText={renderText}
                streaming={visuallyStreaming}
                typing={entry.item.type === "text" && paced.pendingTextIds.has(entry.item.id)}
                key={timelineItemKey(entry.item)}
              />
            ))}
          </div>
        ) : activities.length ? (
          <div className="turn-timeline">
            <ToolActivityGroup
              activities={activities}
              renderText={renderText}
              subagentTaskIndex={subagentTaskIndex}
              onFocusSubagent={onFocusSubagent}
            />
          </div>
        ) : null}
        {terminalEvent?.kind === "turn_failed" ? (
          <div className="turn-failure-detail">
            <CircleAlert size={16} aria-hidden="true" />
            <div className="turn-failure-copy">
              <strong>{providerCallLimitExceeded ? "本轮已达到安全上限" : "错误原因"}</strong>
              <small>{terminalEvent.detail || "本轮未能完成，且没有返回更多错误信息。"}</small>
            </div>
            {onRetry ? (
              <button className="turn-retry-button" type="button" onClick={onRetry}>
                <RotateCcw size={14} aria-hidden="true" />
                <span>{providerCallLimitExceeded ? "开启新 Turn" : plan?.steps.length ? "继续当前步骤" : "重试"}</span>
              </button>
            ) : null}
          </div>
        ) : terminalEvent?.kind === "turn_cancelled" && onRetry ? (
          <div className="turn-terminal-actions">
            <button className="turn-retry-button" type="button" onClick={onRetry}>
              <RotateCcw size={14} aria-hidden="true" />
              <span>{plan?.steps.length ? "继续当前步骤" : "重试"}</span>
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );

  return (
    <div className="turn-context">
      {hasProcess ? (
        <details
          className={`turn-disclosure turn-execution${visuallyStreaming ? " turn-execution--live" : ""}${terminalEvent?.kind === "turn_cancelled" ? " turn-execution--cancelled" : terminalEvent?.kind === "turn_failed" ? " turn-execution--failed" : ""}`}
          open={!terminalCollapse || undefined}
        >
          <summary>
            <SummaryIcon size={15} aria-hidden="true" className={visuallyStreaming ? "turn-tool-running" : undefined} />
            <span className="turn-disclosure-title">{visuallyStreaming && activityStatus === "rate_limited" ? <RetryWaitingLabel retryAtMs={retryAtMs} /> : visuallyStreaming ? liveStatusLabel : summaryTitle}</span>
            {visuallyStreaming ? (
              <span className="turn-live-status-dots" aria-hidden="true"><i /><i /><i /></span>
            ) : (
              <>
                <span className="turn-disclosure-status">{summaryStatus}</span>
                <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
              </>
            )}
          </summary>
          {processContent}
        </details>
      ) : null}
      {finalResponse ? (
        <div className="turn-final-response">
          <TimelineItem item={finalResponse} changes={changes} renderText={renderText} typing={false} />
        </div>
      ) : null}
      {plan?.steps.length ? (
        <PlanProgress
          activeTurn={visuallyStreaming}
          changes={changes}
          plan={plan}
          turnId={turnId}
          workflowPlan={workflowPlan}
          turnOutcome={turnOutcome}
        />
      ) : null}
    </div>
  );
});

type TerminalEventKind = "turn_completed" | "turn_failed" | "turn_cancelled";

function isTerminalEvent(kind: TimelineEventKind): kind is TerminalEventKind {
  return kind === "turn_completed" || kind === "turn_failed" || kind === "turn_cancelled";
}

export function isVisibleConversationTimelineItem(item: TurnTimelineItem) {
  if (item.type === "reasoning") return isDisplayableReasoningItem(item);
  if (item.type !== "event") return true;
  return item.kind !== "turn_completed" && !HIDDEN_PROCESS_EVENT_KINDS.has(item.kind);
}

function isDisplayableReasoningItem(item: ReasoningTimelineItem) {
  return Boolean(item.visibleSummary) || isDisplayableReasoningSummary(item.summary);
}

function reasoningSummaryText(item: ReasoningTimelineItem) {
  return item.visibleSummary ?? item.summary;
}

/**
 * 从时间线提取最新一条可见推理摘要的最后一行，作为活动状态的动态标题。
 * 复用与"思考摘要"展示一致的中文/内容过滤；最新摘要不可展示时回退到固定阶段文案。
 */
function latestReasoningHint(timeline: TurnTimelineItem[]) {
  const item = [...timeline].reverse().find(
    (entry): entry is ReasoningTimelineItem => entry.type === "reasoning" && isDisplayableReasoningItem(entry),
  );
  if (!item) return null;
  const line = reasoningSummaryText(item).split("\n").map((part) => part.trim()).filter(Boolean).pop() ?? "";
  const cleaned = line.replace(/^[#>*_`\-\s]+/, "").replace(/[#*_`\s]+$/, "").trim();
  if (!cleaned) return null;
  return cleaned.length > 40 ? `${cleaned.slice(0, 40)}…` : cleaned;
}

/**
 * 活动状态行：阶段文案 + 秒级等待计时；流中断重试时改为"重连中 n/m"且计时不清零。
 * 只在活动 Turn 期间挂载，setInterval 随组件卸载清理。
 */
function ActivityStatusLabel({
  label,
  hint,
  sinceMs,
  streamRetry,
}: {
  label: string;
  hint: string | null;
  sinceMs?: number;
  streamRetry?: { attempt: number; maxAttempts: number } | null;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [sinceMs, streamRetry?.attempt]);
  const elapsed = sinceMs !== undefined ? Math.max(0, Math.floor((now - sinceMs) / 1000)) : 0;
  const text = streamRetry
    ? `连接中断，正在重连 ${streamRetry.attempt}/${streamRetry.maxAttempts} · ${elapsed}s`
    : `${hint ?? label} · ${elapsed}s${elapsed >= 60 ? "，等待时间较长" : ""}`;
  return <span>{text}</span>;
}

function TimelineItem({
  item,
  changes,
  renderText,
  streaming = false,
  typing = false,
}: {
  item: TurnTimelineItem;
  changes: ChangeSet[];
  renderText?: (text: string) => ReactNode;
  streaming?: boolean;
  typing?: boolean;
}) {
  if (item.type === "text") {
    return (
      <div className={`turn-progress-text${typing ? " turn-progress-text--typing" : ""}`}>
        {renderText ? renderText(item.text) : item.text}
      </div>
    );
  }
  if (item.type === "reasoning") {
    return <ReasoningGroup items={[item]} renderText={renderText} streaming={streaming} />;
  }
  if (item.type === "event") {
    return <TimelineEventRow item={item} changes={changes} />;
  }
  return <ToolActivityRow activity={item.activity} />;
}

function TimelineEventRow({
  item,
  changes,
}: {
  item: Extract<TurnTimelineItem, { type: "event" }>;
  changes: ChangeSet[];
}) {
  const Icon = item.kind === "turn_completed"
    ? CircleCheck
    : item.kind === "turn_failed"
      ? CircleX
      : item.kind === "turn_cancelled"
        ? Circle
        : CircleDot;
  const change = item.kind === "change_applied" ? findChange(item, changes) : null;
  const hasDetails = Boolean(item.detail || change);
  const [open, setOpen] = useState(false);
  if (!hasDetails) {
    return (
      <div className={`turn-timeline-event turn-timeline-event--${item.kind}`}>
        <Icon size={15} aria-hidden="true" />
        <span><strong>{item.title}</strong></span>
      </div>
    );
  }
  return (
    <details
      className={`turn-disclosure turn-timeline-event turn-event-step turn-event-step--${item.kind} turn-timeline-event--${item.kind}`}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <Icon size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">{item.title}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="turn-disclosure-panel">
        <div className="turn-event-step-content">
          {change ? (
            open ? (
              <div className="turn-change-files">
                {change.files.map((file) => (
                  <ChangeFileView changeId={change.id} file={file} key={`${change.id}-${file.path}`} />
                ))}
              </div>
            ) : null
          ) : item.detail ? (
            <small>{item.detail}</small>
          ) : null}
        </div>
      </div>
    </details>
  );
}

interface TextTarget {
  key: string;
  id: string;
  text: string;
}

interface PacedTimeline {
  timeline: TurnTimelineItem[];
  settling: boolean;
  pendingTextIds: Set<string>;
}

function usePacedTimeline(
  timeline: TurnTimelineItem[],
  streaming: boolean,
  initialTextVisible: boolean,
): PacedTimeline {
  const targets = collectTextTargets(timeline);
  const targetSignature = targets.map((target) => `${target.key}:${target.id}:${target.text.length}:${target.text.slice(-32)}`).join("\u0000");
  const wasStreaming = useRef(streaming);
  if (streaming) wasStreaming.current = true;

  const [displayed, setDisplayed] = useState<Record<string, string>>(() => {
    if (streaming && !initialTextVisible) {
      return Object.fromEntries(targets.map((target) => [target.key, ""]));
    }
    return Object.fromEntries(targets.map((target) => [target.key, target.text]));
  });

  useEffect(() => {
    if (!wasStreaming.current) {
      setDisplayed(Object.fromEntries(targets.map((target) => [target.key, target.text])));
      return undefined;
    }

    const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    if (reducedMotion) {
      setDisplayed(Object.fromEntries(targets.map((target) => [target.key, target.text])));
      return undefined;
    }

    let timer: number | undefined;
    const tick = () => {
      setDisplayed((previous) => {
        let changed = false;
        const next = { ...previous };
        for (const target of targets) {
          const current = normalizeDisplayed(target.text, next[target.key] ?? "");
          if (current.length >= target.text.length) {
            next[target.key] = target.text;
            continue;
          }
          const pending = target.text.length - current.length;
          const step = pending > 600 ? Math.min(16, Math.ceil(pending / 80)) : pending > 192 ? 4 : pending > 64 ? 2 : 1;
          const nextText = advanceText(target.text, current, step);
          if (nextText !== current) {
            next[target.key] = nextText;
            changed = true;
          }
          break;
        }
        if (!changed && timer !== undefined) window.clearInterval(timer);
        return changed ? next : previous;
      });
    };

    tick();
    timer = window.setInterval(tick, 32);
    return () => {
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [targetSignature, streaming]);

  let textIndex = 0;
  let blockedByText = false;
  const pendingTextIds = new Set<string>();
  const displayedTimeline = timeline.flatMap((item): TurnTimelineItem[] => {
    if (blockedByText) return [];
    if (item.type !== "text") return [item];
    const target = targets[textIndex++];
    const text = target ? normalizeDisplayed(target.text, displayed[target.key] ?? "") : item.text;
    if (target && text !== target.text) {
      pendingTextIds.add(item.id);
      blockedByText = true;
    }
    return [text === item.text ? item : { ...item, text }];
  });
  const settling = !streaming && wasStreaming.current && pendingTextIds.size > 0;
  return { timeline: displayedTimeline, settling, pendingTextIds };
}

function collectTextTargets(timeline: TurnTimelineItem[]): TextTarget[] {
  const indexes = new Map<string, number>();
  return timeline.flatMap((item) => {
    if (item.type !== "text") return [];
    const index = indexes.get(item.turnId) ?? 0;
    indexes.set(item.turnId, index + 1);
    return [{ key: `${item.turnId}-${index}`, id: item.id, text: item.text }];
  });
}

function normalizeDisplayed(target: string, current: string) {
  if (target.startsWith(current)) return current;
  let common = 0;
  while (common < target.length && common < current.length && target[common] === current[common]) common += 1;
  return target.slice(0, common);
}

function advanceText(target: string, current: string, step: number) {
  let end = Math.min(target.length, current.length + step);
  if (end < target.length && end > 0) {
    const code = target.charCodeAt(end - 1);
    if (code >= 0xd800 && code <= 0xdbff) end += 1;
  }
  return target.slice(0, end);
}

function timelineItemKey(item: TurnTimelineItem) {
  if (item.type === "text") return item.id;
  if (item.type === "reasoning") return `reasoning-${item.turnId}-${item.itemId}`;
  if (item.type === "event") return `event-${item.itemId}`;
  return item.activity.call.id;
}

type ReasoningTimelineItem = Extract<TurnTimelineItem, { type: "reasoning" }>;
type TimelineRenderEntry =
  | { type: "reasoning_group"; items: ReasoningTimelineItem[] }
  | { type: "tool_group"; activities: ToolActivity[]; reasoning: ReasoningTimelineItem[] }
  | { type: "item"; item: Exclude<TurnTimelineItem, { type: "reasoning" | "tool" }> };

function groupConsecutiveTimeline(items: TurnTimelineItem[]): TimelineRenderEntry[] {
  const grouped: TimelineRenderEntry[] = [];
  for (const item of items) {
    const previous = grouped[grouped.length - 1];
    if (item.type === "reasoning") {
      if (previous?.type === "tool_group") previous.reasoning.push(item);
      else if (previous?.type === "reasoning_group") previous.items.push(item);
      else grouped.push({ type: "reasoning_group", items: [item] });
    } else if (item.type === "tool") {
      if (previous?.type === "tool_group") previous.activities.push(item.activity);
      else grouped.push({ type: "tool_group", activities: [item.activity], reasoning: [] });
    } else {
      grouped.push({ type: "item", item });
    }
  }
  return grouped;
}

function ReasoningGroup({
  items,
  renderText,
  streaming = false,
}: {
  items: ReasoningTimelineItem[];
  renderText?: (text: string) => ReactNode;
  /** 所属 Turn 是否仍在流式输出；决定思考行是"思考中"还是"思考 · 持续了 N 秒"。 */
  streaming?: boolean;
}) {
  const lastItem = items[items.length - 1];
  const active = streaming && lastItem?.complete !== true;
  // Follow activity only until the user chooses; a finished block must not re-open itself.
  const [userExpanded, setUserExpanded] = useState<boolean | null>(null);
  const expanded = userExpanded ?? active;
  // 与 ZCode 一致：右侧流式提示只在收起态出现，展开时不重复已经可见的正文。
  const hint = active && !expanded ? latestReasoningLine(lastItem ? reasoningSummaryText(lastItem) : "") : null;
  return (
    <details className="turn-reasoning" open={expanded}>
      <summary
        className="turn-reasoning-summary"
        onClick={(event) => {
          event.preventDefault();
          setUserExpanded((previous) => !(previous ?? active));
        }}
      >
        <Brain size={15} aria-hidden="true" className="turn-reasoning-icon" />
        <span className="turn-reasoning-label">{active ? "思考中" : "思考"}</span>
        <ReasoningDuration active={active} />
        {hint ? <span className="turn-reasoning-hint">{hint}</span> : null}
        <ChevronDown className="turn-reasoning-chevron" size={14} aria-hidden="true" />
      </summary>
      <div className="turn-reasoning-content">
        {items.map((item) => (
          <div className="turn-reasoning-segment" key={`${item.turnId}-${item.itemId}`}>
            {renderText ? renderText(reasoningSummaryText(item)) : reasoningSummaryText(item)}
          </div>
        ))}
      </div>
    </details>
  );
}

/**
 * 秒级耗时单独成一个组件：计时器的 setState 只重渲染这一小节，
 * 不会每秒带着整段思考正文（可能是长 Markdown）重跑一遍。
 */
function ReasoningDuration({ active }: { active: boolean }) {
  const seconds = useElapsedSeconds(active);
  if (seconds === null) return null;
  return <span className="turn-reasoning-duration">· 持续了 {seconds} 秒</span>;
}

/**
 * 秒级计时：只在 `active` 为真时跑，停止后保留最后一次读数，因此"思考 · 持续了 N 秒"
 * 记录的是这一段思考真实的耗时；历史恢复的思考没有可测起点，保持不显示而不是编一个数。
 */
function useElapsedSeconds(active: boolean): number | null {
  const startedAtRef = useRef<number | null>(null);
  const [seconds, setSeconds] = useState<number | null>(null);
  useEffect(() => {
    if (!active) {
      startedAtRef.current = null;
      return undefined;
    }
    if (startedAtRef.current === null) startedAtRef.current = Date.now();
    const startedAt = startedAtRef.current;
    const tick = () => setSeconds(Math.max(1, Math.round((Date.now() - startedAt) / 1000)));
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return seconds;
}

/** 取最新一条推理摘要的最后一行非空文本，作为思考行右侧的流式提示。 */
function latestReasoningLine(summary: string) {
  const line = summary.split("\n").map((part) => part.trim()).filter(Boolean).pop() ?? "";
  const cleaned = line.replace(/^[#>*_`\-\s]+/, "").replace(/[#*_`\s]+$/, "").trim();
  if (!cleaned) return null;
  return cleaned.length > 48 ? `${cleaned.slice(0, 48)}…` : cleaned;
}

function ToolActivityGroup({
  activities,
  reasoning = [],
  renderText,
  subagentTaskIndex,
  onFocusSubagent,
}: {
  activities: ToolActivity[];
  reasoning?: ReasoningTimelineItem[];
  renderText?: (text: string) => ReactNode;
  subagentTaskIndex?: Record<string, number>;
  onFocusSubagent?: (agentId: string) => void;
}) {
  const visibleActivities = hideSupersededPatchFailures(activities);
  const state = toolGroupState(visibleActivities);
  const allCommands = visibleActivities.every((activity) => activity.call.name === "run_command");
  const count = visibleActivities.length;
  const title = allCommands
    ? count === 1 ? "运行了命令" : "运行了多个命令"
    : count === 1 ? "执行了操作" : "执行了多个操作";
  const status = state === "failed"
    ? "包含失败"
    : state === "cancelled"
      ? "已取消"
      : state === "running"
        ? "执行中"
        : state === "pending"
          ? "等待执行"
          : "已完成";
  const active = state === "running" || state === "pending";
  // Codex 式标记：行首用内容类型图标（命令组=终端，其他操作=工具），不用产品 Logo。
  const GroupMarker = allCommands ? SquareTerminal : Wrench;
  // Follow activity only until the user chooses; subsequent tools must not reset that choice.
  const [userExpanded, setUserExpanded] = useState<boolean | null>(null);
  const expanded = userExpanded ?? active;
  return (
    <details
      className={`turn-disclosure turn-tool-group turn-tool-group--${state}${allCommands ? " turn-tool-group--commands" : ""}${allCommands && count > 1 ? " turn-tool-group--multiple-commands" : ""}`}
      open={expanded}
    >
      <summary
        className="turn-tool-group-summary"
        onClick={(event) => {
          event.preventDefault();
          setUserExpanded((previous) => !(previous ?? active));
        }}
      >
        <span className="turn-tool-group-marker" aria-hidden="true"><GroupMarker size={14} aria-hidden="true" /></span>
        <span className="turn-tool-group-copy">
          <span className="turn-disclosure-title">{title}</span>
          {expanded ? (
            <ChevronDown className="turn-tool-group-chevron" size={15} aria-hidden="true" />
          ) : (
            <ChevronRight className="turn-tool-group-chevron" size={15} aria-hidden="true" />
          )}
          {state !== "completed" ? <span className="turn-disclosure-status">{status}</span> : null}
        </span>
      </summary>
      <div className="turn-disclosure-panel">
        <div className="turn-tool-group-content">
          {reasoning.length ? (
            <div className="turn-tool-group-reasoning" aria-label="思考摘要">
              {reasoning.map((item) => {
                const summary = reasoningSummaryText(item);
                return (
                  <div className="turn-tool-group-reasoning-item" key={`${item.turnId}-${item.itemId}`}>
                    <Brain size={14} aria-hidden="true" />
                    <span>{renderText ? renderText(summary) : summary}</span>
                  </div>
                );
              })}
            </div>
          ) : null}
          {visibleActivities.map((activity) => (
            <ToolActivityRow
              activity={activity}
              key={activity.call.id}
              subagentTaskIndex={subagentTaskIndex}
              onFocusSubagent={onFocusSubagent}
            />
          ))}
        </div>
      </div>
    </details>
  );
}

function hideSupersededPatchFailures(activities: ToolActivity[]) {
  const successfulTargets = new Set<string>();
  const supersededCallIds = new Set<string>();
  let laterSuccessfulPatchCount = 0;

  for (let index = activities.length - 1; index >= 0; index -= 1) {
    const activity = activities[index];
    if (activity.call.name === "apply_patch" && activity.state === "completed") {
      laterSuccessfulPatchCount += 1;
      const target = patchRetryTarget(activity);
      if (target) successfulTargets.add(target);
      continue;
    }
    if (activity.call.name !== "apply_patch" || activity.state !== "failed") continue;
    const target = patchRetryTarget(activity);
    if (target ? successfulTargets.has(target) : laterSuccessfulPatchCount === 1) {
      supersededCallIds.add(activity.call.id);
    }
  }

  return supersededCallIds.size
    ? activities.filter((activity) => !supersededCallIds.has(activity.call.id))
    : activities;
}

function patchRetryTarget(activity: ToolActivity) {
  if (activity.call.name !== "apply_patch" || typeof activity.call.arguments.patch !== "string") return null;
  const paths = patchFilePaths(activity.call.arguments.patch)
    .map((path) => path.replace(/\\/g, "/").replace(/^\.\//, ""))
    .sort();
  return paths.length ? JSON.stringify(paths) : null;
}

function toolGroupState(activities: ToolActivity[]): ToolActivity["state"] {
  if (activities.some((activity) => activity.state === "failed" && !isNoMatchActivity(activity))) return "failed";
  if (activities.some((activity) => activity.state === "cancelled")) return "cancelled";
  if (activities.some((activity) => activity.state === "running")) return "running";
  if (activities.some((activity) => activity.state === "pending")) return "pending";
  return "completed";
}

function isNoMatchActivity(activity: ToolActivity) {
  return activity.call.name === "run_command"
    && activity.state === "failed"
    && activity.result?.metadata?.exitCode === 1
    && activity.result?.metadata?.resultKind === "no_matches";
}


function ToolActivityRow({
  activity,
  subagentTaskIndex,
  onFocusSubagent,
}: {
  activity: ToolActivity;
  subagentTaskIndex?: Record<string, number>;
  onFocusSubagent?: (agentId: string) => void;
}) {
  const isCommand = activity.call.name === "run_command";
  const command = isCommand ? commandText(activity) : "";
  const outputChunks = isCommand ? [] : visibleOutput(activity);
  const elapsedMs = useActivityDuration(activity, !isCommand);
  const fileDetails = isCommand ? null : fileActivityDetails(activity);
  const isPending = activity.state === "pending";
  const isRunning = activity.state === "running";
  const failed = activity.state === "failed";
  const noMatches = isNoMatchActivity(activity);
  const visibleFailure = failed && !noMatches;
  const target = isCommand ? command : visibleFailure ? "" : toolTarget(activity);
  const title = target || (isRunning ? runningToolLabel(activity.call.name) : toolLabel(activity.call.name));
  const meta = isCommand
    ? noMatches
      ? "未匹配"
      : visibleFailure
      ? commandFailureSummary(activity)
      : commandActivityStateLabel(activity.state)
    : visibleFailure && activity.result?.output
      ? truncate(activity.result.output, 120)
      : activityStateLabel(activity);
  const subagentIds = subagentIdsOf(activity);
  const isSubagentActivity = DELEGATION_TOOL_NAMES.has(activity.call.name);
  const subagentLabel = isSubagentActivity ? subagentActivityLabel(activity) : null;
  return (
    <div className={`turn-timeline-tool turn-timeline-tool--${noMatches ? "no-matches" : activity.state}${isCommand ? " turn-timeline-tool--command" : ""}${isSubagentActivity ? " turn-timeline-tool--subagent" : ""}`}>
      {noMatches ? (
        <CircleDot size={15} aria-hidden="true" />
      ) : isSubagentActivity ? (
        <Bot className="subagent-tool-icon" size={15} aria-hidden="true" />
      ) : activity.state === "completed" ? (
        <CircleCheck size={15} aria-hidden="true" />
      ) : activity.state === "failed" ? (
        <CircleX size={15} aria-hidden="true" />
      ) : activity.state === "cancelled" || isPending ? (
        <Circle size={15} aria-hidden="true" />
      ) : (
        <LoaderCircle className="turn-tool-running" size={15} aria-hidden="true" />
      )}
      <span className={isCommand ? "turn-command-summary" : undefined}>
        <span className="turn-tool-heading">
        {isCommand ? <span className="turn-tool-kind">终端</span> : null}
        <strong title={isCommand && command ? command : undefined}>{isSubagentActivity && activity.call.name === "create_agent" ? "子智能体" : title}</strong>
        {subagentLabel ? <span className="subagent-tool-label">{subagentLabel}</span> : null}
        {subagentIds.map((subagentId) => {
          const taskNumber = subagentTaskIndex?.[subagentId];
          return taskNumber !== undefined ? (
          <button
            key={subagentId}
            type="button"
            className="subagent-task-chip"
            title="在右侧面板查看该子智能体"
            aria-label={`查看子智能体 task${taskNumber}`}
            onClick={(event) => {
              event.stopPropagation();
              onFocusSubagent?.(subagentId);
            }}
          >
            task{taskNumber}
          </button>
          ) : null;
        })}
        </span>
        <small className="turn-tool-meta" title={isCommand && failed ? meta : undefined}>
          <span>{meta}</span>
          {elapsedMs !== null ? <span className="turn-tool-duration" title={activity.call.name === "wait_agent" ? "本次等待调用的耗时；子任务耗时可在右侧详情查看" : undefined}><Clock3 size={12} aria-hidden="true" />{activity.call.name === "wait_agent" ? "等待耗时" : "耗时"} {formatDuration(elapsedMs)}</span> : null}
        </small>
      </span>
      {fileDetails ? <FileActivityDetails details={fileDetails} /> : null}
      {outputChunks.length ? (
        <details className="turn-tool-output" open={isRunning || undefined}>
          <summary>
            <SquareTerminal size={14} aria-hidden="true" />
            <span>命令输出</span>
            <ChevronDown size={14} aria-hidden="true" />
          </summary>
          <pre>{outputChunks.map((chunk, index) => (
            <span className={`turn-tool-output-line turn-tool-output-line--${chunk.stream}`} key={`${chunk.cursor}-${index}`}>
              {chunk.text}
            </span>
          ))}</pre>
        </details>
      ) : null}
    </div>
  );
}

function subagentActivityLabel(activity: ToolActivity): string | null {
  const args = activity.call.arguments ?? {};
  if (activity.call.name === "create_agent" && typeof args.task === "string" && args.task.trim()) {
    return truncate(args.task.trim(), 96);
  }
  if (activity.call.name === "wait_agent") return "等待回传";
  if (activity.call.name === "send_agent_message") return "发送消息";
  if (activity.call.name === "resume_agent") return "恢复任务";
  if (activity.call.name === "close_agent") return "停止任务";
  return null;
}

interface FileActivityDetailsValue {
  id: string;
  kind: "patch" | "write";
  label: string;
  path: string;
  content: string;
  meta: string;
  truncated: boolean;
}

function FileActivityDetails({ details }: { details: FileActivityDetailsValue }) {
  const [open, setOpen] = useState(false);

  return (
    <details className="turn-tool-details turn-tool-details--file" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>
        <FileText size={14} aria-hidden="true" />
        <span>{details.label}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </summary>
      {open ? <div className="turn-command-editor-shell">
        <div className="turn-command-editor-header">
          <span><FileText size={13} aria-hidden="true" />{details.path || "Patch"}</span>
          <small>只读</small>
        </div>
        <Suspense fallback={<div className="turn-command-editor-loading">正在载入编辑器...</div>}>
          <div className="turn-file-editor-frame" style={{ height: `${fileDetailEditorHeight(details.content)}px` }}>
            <ReadOnlyCodeEditor
              path={details.path || "Patch"}
              modelPath={`k-coder-file-operation://detail/${encodeURIComponent(details.id)}`}
              language={details.kind === "patch" ? "diff" : fileLanguage(details.path)}
              value={details.content}
              readOnly
            />
          </div>
        </Suspense>
        {details.meta || details.truncated ? (
          <small className="turn-command-editor-meta">
            {details.meta}
            {details.meta && details.truncated ? " · " : ""}
            {details.truncated ? "内容过长，仅显示前 64 KiB" : ""}
          </small>
        ) : null}
      </div> : null}
    </details>
  );
}

function ChangeFileView({ changeId, file }: { changeId: string; file: ChangeSet["files"][number] }) {
  const [copied, setCopied] = useState(false);
  const stats = changeLineStats(file.unifiedDiff);
  const displayPath = file.operation === "move" && file.destinationPath
    ? `${file.path} -> ${file.destinationPath}`
    : file.path;
  const languagePath = file.destinationPath || file.path;
  const hasSnapshots = (file.operation === "add" || file.beforeContent !== null)
    && (file.operation === "delete" || file.afterContent !== null);
  const modelRoot = `k-coder-change://snapshot/${encodeURIComponent(changeId)}/${encodeURIComponent(languagePath)}`;

  async function copyDiff() {
    try {
      await navigator.clipboard.writeText(file.unifiedDiff);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="turn-change-file">
      <div className="turn-change-editor-shell">
        <div className="turn-change-editor-header">
          <span className="turn-change-editor-path" title={displayPath}>
            {changeOperationLabel(file.operation)} {displayPath}
          </span>
          <span className="turn-change-editor-stats" aria-label={`新增 ${stats.added} 行，删除 ${stats.deleted} 行`}>
            <i>+{stats.added}</i><i>-{stats.deleted}</i>
          </span>
          <button
            type="button"
            className="turn-change-copy"
            title={copied ? "已复制 Diff" : "复制 Diff"}
            aria-label={copied ? "已复制 Diff" : "复制 Diff"}
            onClick={() => void copyDiff()}
          >
            {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
          </button>
        </div>
        <Suspense fallback={<div className="turn-command-editor-loading">正在载入编辑器...</div>}>
          <div className="turn-change-editor-frame" style={{ height: `${changeEditorHeight(file)}px` }}>
            {hasSnapshots ? (
              <ChangeCodeDiffEditor
                path={displayPath}
                originalModelPath={`${modelRoot}?side=original`}
                modifiedModelPath={`${modelRoot}?side=modified`}
                language={fileLanguage(languagePath)}
                originalValue={file.beforeContent ?? ""}
                modifiedValue={file.afterContent ?? ""}
              />
            ) : (
              <ReadOnlyCodeEditor
                path={`${displayPath} Diff`}
                modelPath={`${modelRoot}?side=unified`}
                language="diff"
                value={file.unifiedDiff || "没有可显示的 Diff"}
                readOnly
              />
            )}
          </div>
        </Suspense>
      </div>
    </div>
  );
}

function findChange(item: Extract<TurnTimelineItem, { type: "event" }>, changes: ChangeSet[]) {
  if (!item.detail) return null;
  return [...changes].reverse().find((change) =>
    change.turnId === item.turnId
    && change.files.map((file) => file.path).join("、") === item.detail,
  ) ?? null;
}

function commandText(activity: ToolActivity) {
  const argumentsValue = activity.call.arguments;
  const command = typeof argumentsValue.command === "string" ? argumentsValue.command.trim() : "";
  if (command) return command;
  const program = typeof argumentsValue.program === "string" ? argumentsValue.program.trim() : "";
  if (!program) return "";
  const args = Array.isArray(argumentsValue.args)
    ? argumentsValue.args.filter((arg): arg is string => typeof arg === "string")
    : [];
  return [program, ...args].map(shellQuote).join(" ");
}

function fileDetailEditorHeight(content: string) {
  const lines = Math.max(1, content.split(/\r?\n/).length);
  return Math.min(320, Math.max(100, lines * 19 + 14));
}

function changeEditorHeight(file: ChangeSet["files"][number]) {
  const stats = changeLineStats(file.unifiedDiff);
  const originalLines = lineCount(file.beforeContent);
  const modifiedLines = lineCount(file.afterContent);
  const visibleLines = Math.max(originalLines, modifiedLines, stats.added + stats.deleted + 3);
  return Math.min(360, Math.max(120, visibleLines * 19 + 14));
}

function lineCount(value: string | null) {
  return value === null ? 0 : Math.max(1, value.split(/\r?\n/).length);
}

function fileLanguage(path: string) {
  const name = path.toLowerCase().split(/[\\/]/).pop() ?? "";
  const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  if (["ts", "tsx"].includes(extension)) return "typescript";
  if (["js", "jsx", "mjs", "cjs"].includes(extension)) return "javascript";
  if (extension === "json" || name === "package-lock.json") return "json";
  if (["css", "scss", "less"].includes(extension)) return extension;
  if (["html", "htm"].includes(extension)) return "html";
  if (["xml", "svg"].includes(extension)) return "xml";
  if (["md", "mdx"].includes(extension)) return "markdown";
  if (["yaml", "yml"].includes(extension)) return "yaml";
  if (["sh", "bash", "zsh"].includes(extension)) return "shell";
  if (extension === "ps1") return "powershell";
  if (["bat", "cmd"].includes(extension)) return "bat";
  if (extension === "py") return "python";
  if (extension === "rs") return "rust";
  if (extension === "toml" || name === "cargo.lock") return "toml";
  if (extension === "sql") return "sql";
  if (["c", "h"].includes(extension)) return "c";
  if (["cc", "cpp", "cxx", "hpp"].includes(extension)) return "cpp";
  if (extension === "cs") return "csharp";
  if (extension === "java") return "java";
  if (extension === "go") return "go";
  if (extension === "php") return "php";
  if (extension === "rb") return "ruby";
  if (extension === "dockerfile" || name === "dockerfile") return "dockerfile";
  return "plaintext";
}

function fileActivityDetails(activity: ToolActivity): FileActivityDetailsValue | null {
  const args = activity.call.arguments ?? {};
  const path = typeof args.path === "string" ? args.path : "";
  if (activity.call.name === "read_file") return null;
  let label = "";
  let content = "";
  let meta = path;
  let kind: FileActivityDetailsValue["kind"] = "patch";
  let truncated = false;

  if (activity.call.name === "apply_patch" && typeof args.patch === "string") {
    kind = "patch";
    label = "查看补丁";
    content = args.patch;
    meta = patchFilePaths(args.patch).join("、");
  } else if (activity.call.name === "write_file" && typeof args.content === "string") {
    kind = "write";
    label = "查看写入内容";
    content = args.content;
  } else {
    return null;
  }

  const bounded = boundDetail(content);
  content = bounded.content;
  truncated = bounded.truncated;
  return {
    id: `${activity.turnId}-${activity.call.id}`,
    kind,
    label,
    path,
    content,
    meta,
    truncated,
  };
}

function patchFilePaths(patch: string) {
  const paths: string[] = [];
  for (const line of patch.split(/\r?\n/)) {
    const match = line.match(/^\*\*\* (?:Add|Update|Delete) File:\s*(.+)$/)
      ?? line.match(/^\*\*\* Move to:\s*(.+)$/);
    const path = match?.[1]?.trim();
    if (path && !paths.includes(path)) paths.push(path);
  }
  return paths;
}

function boundDetail(value: string) {
  const limit = 64 * 1024;
  if (value.length <= limit) return { content: value, truncated: false };
  return { content: value.slice(0, limit), truncated: true };
}

function shellQuote(value: string) {
  if (/^[\w./:=+-]+$/.test(value)) return value;
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function changeOperationLabel(operation: ChangeSet["files"][number]["operation"]) {
  return ({ add: "新增", modify: "已编辑", delete: "删除", move: "移动" })[operation];
}

function useActivityDuration(activity: ToolActivity, enabled = true): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!enabled || activity.state !== "running" || !activity.startedAtMs) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, [activity.state, activity.startedAtMs, enabled]);
  if (!enabled) return null;
  if (typeof activity.durationMs === "number") return activity.durationMs;
  if (activity.startedAtMs) return Math.max(0, now - activity.startedAtMs);
  return null;
}

function formatDuration(durationMs: number) {
  if (durationMs < 1_000) return `${Math.max(0, Math.round(durationMs))}ms`;
  if (durationMs < 60_000) return `${(durationMs / 1_000).toFixed(1)}s`;
  const minutes = Math.floor(durationMs / 60_000);
  const seconds = Math.floor((durationMs % 60_000) / 1_000);
  return `${minutes}分${seconds.toString().padStart(2, "0")}秒`;
}

function visibleOutput(activity: ToolActivity): ToolOutputDelta[] {
  if (activity.outputChunks?.length) return activity.outputChunks;
  const persisted = activity.result?.metadata?.outputChunks;
  if (Array.isArray(persisted)) {
    const chunks = persisted.flatMap((chunk): ToolOutputDelta[] => {
      if (!chunk || typeof chunk !== "object") return [];
      const value = chunk as Record<string, unknown>;
      if (
        (value.stream !== "stdout" && value.stream !== "stderr")
        || typeof value.cursor !== "number"
        || typeof value.text !== "string"
      ) return [];
      return [{ stream: value.stream, cursor: value.cursor, text: value.text }];
    });
    if (chunks.length) return chunks;
  }
  return [];
}

function toolLabel(name: string) {
  const labels: Record<string, string> = {
    apply_patch: "应用补丁",
    browser_click: "点击页面",
    browser_navigate: "打开网页",
    close_agent: "关闭子智能体",
    create_agent: "创建子智能体",
    list_agents: "查看子智能体",
    list_directory: "查看目录",
    read_file: "读取文件",
    request_user_input: "请求输入",
    resume_agent: "恢复子智能体",
    run_command: "执行命令",
    search_repository: "搜索代码",
    send_agent_message: "发送给子智能体",
    update_plan: "更新计划",
    wait_agent: "等待子智能体",
    write_file: "写入文件",
  };
  return labels[name] ?? name;
}

function runningToolLabel(name: string) {
  const labels: Record<string, string> = {
    apply_patch: "正在应用补丁",
    browser_click: "正在操作页面",
    browser_navigate: "正在打开网页",
    close_agent: "正在关闭子智能体",
    create_agent: "正在创建子智能体",
    list_agents: "正在查看子智能体",
    list_directory: "正在查看目录",
    read_file: "正在读取文件",
    request_user_input: "正在准备问题",
    resume_agent: "正在恢复子智能体",
    run_command: "正在执行命令",
    search_repository: "正在搜索代码",
    send_agent_message: "正在发送给子智能体",
    update_plan: "正在更新计划",
    wait_agent: "正在等待子智能体",
    write_file: "正在写入文件",
  };
  return labels[name] ?? `正在运行 ${name}`;
}

function toolTarget(activity: ToolActivity) {
  const args = activity.call.arguments ?? {};
  if (activity.call.name === "wait_agent" && Array.isArray(args.agentIds)) {
    return "等待任一子智能体结束";
  }
  // Delegation failures already surface through the Chinese tool label; dumping the
  // raw `SubagentView` JSON into the title would only add noise.
  if (activity.state === "failed" && activity.result?.output && !DELEGATION_TOOL_NAMES.has(activity.call.name)) {
    return truncate(activity.result.output, 120);
  }
  if (activity.call.name === "create_agent" && typeof args.task === "string" && args.task.trim()) {
    return truncate(`创建子智能体：${args.task.trim()}`, 120);
  }
  if (activity.call.name === "read_file" && typeof args.path === "string") {
    const metadata = activity.result?.metadata ?? {};
    const startLine = positiveInteger(metadata.startLine) ?? positiveInteger(args.startLine);
    const requestedLineCount = positiveInteger(args.lineCount);
    const requestedEndLine = positiveInteger(args.endLine);
    const endLine = positiveInteger(metadata.endLine)
      ?? requestedEndLine
      ?? (startLine !== null && requestedLineCount !== null
        ? startLine + requestedLineCount - 1
        : null);
    const range = startLine !== null
      ? endLine !== null && endLine !== startLine
        ? ` L${startLine}-${endLine}`
        : ` L${startLine}`
      : "";
    return truncate(`读取 ${args.path}${range}`, 120);
  }
  if (activity.call.name === "search_repository" && typeof args.query === "string") {
    return truncate(`搜索 ${args.query}`, 120);
  }
  if (activity.call.name === "list_directory" && typeof args.path === "string") {
    return truncate(`查看目录 ${args.path}`, 120);
  }
  if (activity.call.name === "write_file" && typeof args.path === "string") {
    return truncate(`写入 ${args.path}`, 120);
  }
  if (activity.call.name === "apply_patch" && typeof args.patch === "string") {
    const paths = patchFilePaths(args.patch);
    if (paths.length) return truncate(`应用补丁 ${paths.join("、")}`, 120);
  }
  if (activity.call.name === "browser_navigate" && typeof args.url === "string") {
    return truncate(`打开 ${args.url}`, 120);
  }
  for (const key of ["path", "filePath", "file_path", "command", "query", "url"]) {
    if (typeof args[key] === "string" && args[key]) return truncate(args[key], 88);
  }
  return "";
}

function positiveInteger(value: unknown) {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : null;
}

function activityStateLabel(activity: ToolActivity) {
  if (activity.state === "pending") return "等待执行";
  if (activity.state === "running") return "执行中";
  if (activity.state === "failed") return "执行失败";
  if (activity.state === "cancelled") return "已取消";
  if (activity.call.name === "wait_agent" && activity.result?.success) {
    try {
      const result = JSON.parse(activity.result.output);
      if (result.timedOut === true || ["queued", "running", "blocked"].includes(result.state)) {
        return "本次等待结束，子任务仍在运行";
      }
      return "已获取结果";
    } catch {
      return "本次等待结束";
    }
  }
  return "已完成";
}

function commandActivityStateLabel(state: ToolActivity["state"]) {
  if (state === "pending") return "等待运行";
  if (state === "running") return "运行中";
  if (state === "failed") return "运行失败";
  if (state === "cancelled") return "已取消";
  return "已运行";
}

function commandFailureSummary(activity: ToolActivity) {
  const metadata = activity.result?.metadata;
  if (metadata?.resultKind === "invalid_command" && metadata.executed === false
    && typeof metadata.recoveryHint === "string") {
    return `未执行：${truncate(metadata.recoveryHint.replace(/\s+/g, " ").trim(), 180)}`;
  }
  const state = metadata?.state as { state?: unknown } | undefined;
  if (state?.state === "timed_out") return "运行超时";
  if (state?.state === "cancelled") return "已取消";
  const outputChunks = activity.result?.metadata?.outputChunks;
  const stderr = Array.isArray(outputChunks)
    ? outputChunks
      .filter((chunk): chunk is { stream: string; text: string } => (
        typeof chunk === "object" && chunk !== null
        && (chunk as { stream?: unknown }).stream === "stderr"
        && typeof (chunk as { text?: unknown }).text === "string"
      ))
      .map((chunk) => chunk.text).join("")
    : activity.result?.output ?? "";
  if (stderr.includes("TerminatorExpectedAtEndOfString")) return "运行失败：PowerShell 字符串引号未闭合";
  if (stderr.includes("ParserError")) return "运行失败：PowerShell 命令语法错误";
  const stderrLine = stderr.split(/\r?\n/).map((line) => line.trim()).find(Boolean);
  if (Array.isArray(outputChunks) && stderrLine) return `运行失败：${truncate(stderrLine, 180)}`;
  const recoveryHint = metadata?.recoveryHint;
  if (typeof recoveryHint === "string" && recoveryHint.trim()) {
    return `运行失败：${truncate(recoveryHint.replace(/\s+/g, " ").trim(), 180)}`;
  }
  // With stream metadata, stdout can belong to a successful earlier command.
  if (Array.isArray(outputChunks) && typeof metadata?.exitCode === "number") {
    const hasOutput = outputChunks.some((chunk) => typeof chunk?.text === "string" && chunk.text.trim());
    return `运行失败：退出码 ${metadata.exitCode}${hasOutput ? "（已有部分输出）" : "（无错误详情）"}`;
  }
  const firstLine = activity.result?.output
    ?.split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
  return firstLine ? `运行失败：${truncate(firstLine, 180)}` : "运行失败";
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max - 1)}...` : value;
}
