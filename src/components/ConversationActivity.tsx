import {
  Activity,
  Brain,
  Check,
  ChevronDown,
  Circle,
  CircleCheck,
  CircleDot,
  CircleX,
  Clock3,
  Copy,
  FileText,
  ListChecks,
  LoaderCircle,
  SquareTerminal,
} from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import type {
  AgentActivityStatus,
  ChangeSet,
  PlanStepState,
  PlanView,
  ToolActivity,
  ToolOutputDelta,
  TimelineEventKind,
  TurnTimelineItem,
} from "../types/runtime";

const ReadOnlyCodeEditor = lazy(() => import("./CodeEditor").then((module) => ({ default: module.CodeEditor })));
const ChangeCodeDiffEditor = lazy(() => import("./CodeEditor").then((module) => ({ default: module.CodeDiffEditor })));

export function ConversationTurnActivity({
  activities,
  timeline = [],
  changes = [],
  plan,
  streaming = false,
  activityStatus = null,
  finalMessageId,
  renderText,
}: {
  activities: ToolActivity[];
  timeline?: TurnTimelineItem[];
  changes?: ChangeSet[];
  plan: PlanView | null;
  streaming?: boolean;
  activityStatus?: AgentActivityStatus | null;
  finalMessageId?: string;
  renderText?: (text: string) => ReactNode;
}) {
  const paced = usePacedTimeline(timeline, streaming);
  const visuallyStreaming = streaming || paced.settling;
  const visibleTimeline = visuallyStreaming && !streaming
    ? paced.timeline.filter((item) => item.type !== "event" || !isTerminalEvent(item.kind))
    : paced.timeline;

  if (!activities.length && !timeline.length && !plan?.steps.length && !activityStatus) return null;

  const finalResponse = !visuallyStreaming && finalMessageId
    ? visibleTimeline.find((item): item is Extract<TurnTimelineItem, { type: "text" }> => item.type === "text" && item.id === finalMessageId)
    : null;
  const processTimeline = finalResponse ? visibleTimeline.filter((item) => item !== finalResponse) : visibleTimeline;
  const terminalEvent = [...processTimeline].reverse().find(
    (item): item is Extract<TurnTimelineItem, { type: "event" }> => item.type === "event" && isTerminalEvent(item.kind),
  );
  const processItems = processTimeline.filter((item) => item !== terminalEvent);
  const hasItems = Boolean(plan?.steps.length || activities.length || processItems.length);
  const hasProcess = hasItems || Boolean(activityStatus) || terminalEvent?.kind === "turn_failed" || terminalEvent?.kind === "turn_cancelled";
  const toolCount = processTimeline.filter((item) => item.type === "tool").length || activities.length;
  const summaryTitle = terminalEvent?.durationMs !== undefined
    ? terminalEvent.kind === "turn_cancelled"
      ? "已停止"
      : terminalEvent.kind === "turn_failed"
        ? "执行失败"
        : `执行了 ${formatDuration(terminalEvent.durationMs)}`
    : terminalEvent?.kind === "turn_cancelled"
      ? "已停止"
      : terminalEvent?.kind === "turn_failed"
        ? "执行失败"
        : "执行过程";
  const autoCollapse = !visuallyStreaming && terminalEvent?.kind === "turn_completed";
  const SummaryIcon = visuallyStreaming
    ? LoaderCircle
    : terminalEvent?.kind === "turn_cancelled"
      ? Circle
      : terminalEvent?.kind === "turn_failed"
        ? CircleX
        : Activity;
  const summaryStatus = terminalEvent?.kind === "turn_completed"
    ? (toolCount ? `${toolCount} 个操作` : "已完成")
    : terminalEvent?.durationMs !== undefined
      ? `耗时 ${formatDuration(terminalEvent.durationMs)}`
      : toolCount ? `${toolCount} 个操作` : "处理中";
  const statusLabel = paced.pendingTextIds.size
    ? "生成回复中"
    : activityStatus ? {
      thinking: "思考中",
      responding: "生成回复中",
      running_tool: "处理工具结果中",
      awaiting_approval: "等待确认",
      finalizing: "整理结果中",
    }[activityStatus] : visuallyStreaming ? "生成回复中" : null;
  const groupedProcessTimeline = groupConsecutiveTimeline(processTimeline);
  const processContent = (
    <div className="turn-disclosure-panel">
      <div className={visuallyStreaming ? "turn-execution-live" : "turn-execution-content"}>
        {plan?.steps.length ? <ConversationPlan plan={plan} allowAutoCollapse={autoCollapse} /> : null}
        {processTimeline.length ? (
          <div className="turn-timeline">
            {groupedProcessTimeline.map((entry) => entry.type === "reasoning_group" ? (
              <ReasoningGroup
                items={entry.items}
                renderText={renderText}
                allowAutoCollapse={autoCollapse}
                key={`reasoning-group-${entry.items.map((item) => item.itemId).join("-")}`}
              />
            ) : entry.type === "tool_group" ? (
              <ToolActivityGroup
                activities={entry.activities}
                allowAutoCollapse={autoCollapse}
                key={`tool-group-${entry.activities.map((activity) => activity.call.id).join("-")}`}
              />
            ) : (
              <TimelineItem
                item={entry.item}
                changes={changes}
                renderText={renderText}
                allowAutoCollapse={autoCollapse}
                typing={entry.item.type === "text" && paced.pendingTextIds.has(entry.item.id)}
                key={timelineItemKey(entry.item)}
              />
            ))}
          </div>
        ) : activities.length ? (
          <div className="turn-timeline">
            <ToolActivityGroup activities={activities} allowAutoCollapse={autoCollapse} />
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
          open={!autoCollapse || undefined}
        >
          <summary>
            <SummaryIcon size={15} aria-hidden="true" className={visuallyStreaming ? "turn-tool-running" : undefined} />
            <span className="turn-disclosure-title">{visuallyStreaming ? statusLabel : summaryTitle}</span>
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
          <TimelineItem item={finalResponse} changes={changes} renderText={renderText} allowAutoCollapse={autoCollapse} typing={false} />
        </div>
      ) : null}
    </div>
  );
}

function isTerminalEvent(kind: TimelineEventKind) {
  return kind === "turn_completed" || kind === "turn_failed" || kind === "turn_cancelled";
}

function TimelineItem({
  item,
  changes,
  renderText,
  allowAutoCollapse,
  typing = false,
}: {
  item: TurnTimelineItem;
  changes: ChangeSet[];
  renderText?: (text: string) => ReactNode;
  allowAutoCollapse: boolean;
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
    return <ReasoningGroup items={[item]} renderText={renderText} allowAutoCollapse={allowAutoCollapse} />;
  }
  if (item.type === "event") {
    return <TimelineEventRow item={item} changes={changes} allowAutoCollapse={allowAutoCollapse} />;
  }
  return <ToolActivityRow activity={item.activity} />;
}

function TimelineEventRow({
  item,
  changes,
  allowAutoCollapse,
}: {
  item: Extract<TurnTimelineItem, { type: "event" }>;
  changes: ChangeSet[];
  allowAutoCollapse: boolean;
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
  const expanded = !allowAutoCollapse || item.kind === "turn_failed";
  const [open, setOpen] = useState(expanded);
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
      open={expanded || undefined}
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

function usePacedTimeline(timeline: TurnTimelineItem[], streaming: boolean): PacedTimeline {
  const targets = collectTextTargets(timeline);
  const targetSignature = targets.map((target) => `${target.key}:${target.id}:${target.text.length}:${target.text.slice(-32)}`).join("\u0000");
  const wasStreaming = useRef(streaming);
  if (streaming) wasStreaming.current = true;

  const [displayed, setDisplayed] = useState<Record<string, string>>(() => {
    if (streaming) return Object.fromEntries(targets.map((target) => [target.key, ""]));
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
  | { type: "tool_group"; activities: ToolActivity[] }
  | { type: "item"; item: Exclude<TurnTimelineItem, { type: "reasoning" | "tool" }> };

function groupConsecutiveTimeline(items: TurnTimelineItem[]): TimelineRenderEntry[] {
  const grouped: TimelineRenderEntry[] = [];
  for (const item of items) {
    const previous = grouped[grouped.length - 1];
    if (item.type === "reasoning") {
      if (previous?.type === "reasoning_group"
        && Boolean(previous.items[0]?.complete) === Boolean(item.complete)) previous.items.push(item);
      else grouped.push({ type: "reasoning_group", items: [item] });
    } else if (item.type === "tool") {
      if (previous?.type === "tool_group") previous.activities.push(item.activity);
      else grouped.push({ type: "tool_group", activities: [item.activity] });
    } else {
      grouped.push({ type: "item", item });
    }
  }
  return grouped;
}

function ReasoningGroup({
  items,
  renderText,
  allowAutoCollapse,
}: {
  items: ReasoningTimelineItem[];
  renderText?: (text: string) => ReactNode;
  allowAutoCollapse: boolean;
}) {
  const complete = items.every((item) => item.complete);
  const status = complete && items.length > 1 ? `${items.length} 段` : complete ? "已完成" : "生成中";
  return (
    <details className="turn-disclosure turn-reasoning" open={allowAutoCollapse ? (!complete || undefined) : true}>
      <summary>
        <Brain size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">思考内容</span>
        <span className="turn-disclosure-status">{status}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="turn-disclosure-panel">
        <div className="turn-reasoning-content">
          {items.map((item) => (
            <div className="turn-reasoning-segment" key={`${item.turnId}-${item.itemId}`}>
              {renderText ? renderText(item.summary) : item.summary}
            </div>
          ))}
        </div>
      </div>
    </details>
  );
}

function ToolActivityGroup({ activities, allowAutoCollapse }: { activities: ToolActivity[]; allowAutoCollapse: boolean }) {
  const state = toolGroupState(activities);
  const allCommands = activities.every((activity) => activity.call.name === "run_command");
  const count = activities.length;
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
  const Icon = state === "failed"
    ? CircleX
    : state === "cancelled" || state === "pending"
      ? Circle
      : state === "running"
        ? LoaderCircle
        : allCommands ? SquareTerminal : Activity;
  const expanded = state !== "completed";

  return (
    <details className={`turn-disclosure turn-tool-group turn-tool-group--${state}`} open={allowAutoCollapse ? (expanded || undefined) : true}>
      <summary>
        <Icon className={state === "running" ? "turn-tool-running" : undefined} size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">{title}</span>
        <span className="turn-disclosure-status">{status}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="turn-disclosure-panel">
        <div className="turn-tool-group-content">
          {activities.map((activity) => <ToolActivityRow activity={activity} keepDetailsExpanded={!allowAutoCollapse} key={activity.call.id} />)}
        </div>
      </div>
    </details>
  );
}

function toolGroupState(activities: ToolActivity[]): ToolActivity["state"] {
  if (activities.some((activity) => activity.state === "failed")) return "failed";
  if (activities.some((activity) => activity.state === "cancelled")) return "cancelled";
  if (activities.some((activity) => activity.state === "running")) return "running";
  if (activities.some((activity) => activity.state === "pending")) return "pending";
  return "completed";
}


function ToolActivityRow({ activity, keepDetailsExpanded = false }: { activity: ToolActivity; keepDetailsExpanded?: boolean }) {
  const outputChunks = visibleOutput(activity);
  const elapsedMs = useActivityDuration(activity);
  const command = activity.call.name === "run_command" ? commandDetails(activity) : null;
  const fileDetails = command ? null : fileActivityDetails(activity);
  const isPending = activity.state === "pending";
  const isRunning = activity.state === "running";
  const failed = activity.state === "failed";
  const target = failed ? "" : toolTarget(activity);
  const title = target || (isRunning ? runningToolLabel(activity.call.name) : toolLabel(activity.call.name));
  const meta = failed && activity.result?.output
    ? truncate(activity.result.output, 120)
    : activityStateLabel(activity);
  return (
    <div className={`turn-timeline-tool turn-timeline-tool--${activity.state}`}>
      {activity.state === "completed" ? (
        <CircleCheck size={15} aria-hidden="true" />
      ) : activity.state === "failed" ? (
        <CircleX size={15} aria-hidden="true" />
      ) : activity.state === "cancelled" || isPending ? (
        <Circle size={15} aria-hidden="true" />
      ) : (
        <LoaderCircle className="turn-tool-running" size={15} aria-hidden="true" />
      )}
      <span>
        <strong>{title}</strong>
        <small className="turn-tool-meta">
          <span>{meta}</span>
          {elapsedMs !== null ? <span className="turn-tool-duration"><Clock3 size={12} aria-hidden="true" />耗时 {formatDuration(elapsedMs)}</span> : null}
        </small>
      </span>
      {command ? <CommandDetails command={command} /> : null}
      {fileDetails ? <FileActivityDetails details={fileDetails} /> : null}
      {outputChunks.length ? (
        <details className="turn-tool-output" open={keepDetailsExpanded || isRunning || activity.state === "failed" || activity.state === "cancelled" || undefined}>
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

function CommandDetails({ command }: { command: CommandDetailsValue }) {
  return (
    <div className="turn-command-inline">
      <div className="turn-command-editor-header turn-command-inline-header">
        <span><SquareTerminal size={13} aria-hidden="true" />命令</span>
        <small>{command.shellLabel}</small>
      </div>
      <pre>{command.text}</pre>
      {command.cwd || command.timeoutMs ? (
        <small className="turn-command-editor-meta">
          {command.cwd ? `工作目录：${command.cwd}` : ""}
          {command.cwd && command.timeoutMs ? " · " : ""}
          {command.timeoutMs ? `超时：${formatDuration(command.timeoutMs)}` : ""}
        </small>
      ) : null}
    </div>
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

interface CommandDetailsValue {
  text: string;
  cwd: string;
  timeoutMs: number | null;
  shellLabel: string;
}

function commandDetails(activity: ToolActivity): CommandDetailsValue | null {
  const argumentsValue = activity.call.arguments;
  const shell = typeof activity.result?.metadata.shell === "string" ? activity.result.metadata.shell : "";
  const command = typeof argumentsValue.command === "string" ? argumentsValue.command.trim() : "";
  if (command) {
    return {
      text: command,
      cwd: typeof argumentsValue.cwd === "string" ? argumentsValue.cwd : "",
      timeoutMs: typeof argumentsValue.timeoutMs === "number" ? argumentsValue.timeoutMs : null,
      shellLabel: shellLabel(shell),
    };
  }
  const program = typeof argumentsValue.program === "string" ? argumentsValue.program.trim() : "";
  if (!program) return null;
  const args = Array.isArray(argumentsValue.args)
    ? argumentsValue.args.filter((arg): arg is string => typeof arg === "string")
    : [];
  const text = [program, ...args].map(shellQuote).join(" ");
  return {
    text,
    cwd: typeof argumentsValue.cwd === "string" ? argumentsValue.cwd : "",
    timeoutMs: typeof argumentsValue.timeoutMs === "number" ? argumentsValue.timeoutMs : null,
    shellLabel: shellLabel(shell),
  };
}

function shellLabel(shell: string) {
  const value = shell.toLowerCase();
  if (value.includes("cmd")) return "命令提示符 · CMD";
  if (value.includes("powershell") || value.includes("pwsh")) return "PowerShell";
  if (value.includes("zsh")) return "Zsh";
  if (value.includes("bash")) return "Bash";
  if (value.includes("sh")) return "Shell";
  return "Shell";
}

function fileDetailEditorHeight(content: string) {
  const lines = Math.max(1, content.split(/\r?\n/).length);
  return Math.min(320, Math.max(100, lines * 19 + 14));
}

function changeLineStats(diff: string) {
  let added = 0;
  let deleted = 0;
  for (const line of diff.split(/\r?\n/)) {
    if (line.startsWith("+") && !line.startsWith("+++")) added += 1;
    if (line.startsWith("-") && !line.startsWith("---")) deleted += 1;
  }
  return { added, deleted };
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

function useActivityDuration(activity: ToolActivity): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (activity.state !== "running" || !activity.startedAtMs) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, [activity.state, activity.startedAtMs]);
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
  if (activity.call.name === "run_command" && activity.result?.output) {
    return [{ stream: "stdout", cursor: 0, text: activity.result.output.slice(-64 * 1024) }];
  }
  return [];
}

function ConversationPlan({ plan, allowAutoCollapse }: { plan: PlanView; allowAutoCollapse: boolean }) {
  const completed = plan.steps.filter((step) => step.status === "completed" || step.status === "skipped").length;
  const active = plan.steps.some((step) => step.status === "in_progress");

  return (
    <details className="turn-disclosure turn-plan" open={allowAutoCollapse ? (active || undefined) : true}>
      <summary>
        <ListChecks size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">计划</span>
        <span className="turn-disclosure-status">{completed}/{plan.steps.length}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="turn-disclosure-panel">
        <ol className="turn-plan-steps">
          {plan.steps.map((step) => (
            <li className={`turn-plan-step turn-plan-step--${step.status}`} key={step.id}>
              <PlanStateIcon status={step.status} />
              <span>
                <strong>{step.step}</strong>
                {step.detail ? <small>{step.detail}</small> : null}
              </span>
            </li>
          ))}
        </ol>
      </div>
    </details>
  );
}

function PlanStateIcon({ status }: { status: PlanStepState }) {
  if (status === "completed") return <CircleCheck size={15} aria-hidden="true" />;
  if (status === "in_progress") return <CircleDot className="turn-tool-running" size={15} aria-hidden="true" />;
  if (status === "failed") return <CircleX size={15} aria-hidden="true" />;
  return <Circle size={15} aria-hidden="true" />;
}

function toolLabel(name: string) {
  const labels: Record<string, string> = {
    apply_patch: "应用补丁",
    browser_click: "点击页面",
    browser_navigate: "打开网页",
    list_directory: "查看目录",
    read_file: "读取文件",
    request_user_input: "请求输入",
    run_command: "执行命令",
    search_repository: "搜索代码",
    update_plan: "更新计划",
    write_file: "写入文件",
  };
  return labels[name] ?? name;
}

function runningToolLabel(name: string) {
  const labels: Record<string, string> = {
    apply_patch: "正在应用补丁",
    browser_click: "正在操作页面",
    browser_navigate: "正在打开网页",
    list_directory: "正在查看目录",
    read_file: "正在读取文件",
    request_user_input: "正在准备问题",
    run_command: "正在执行命令",
    search_repository: "正在搜索代码",
    update_plan: "正在更新计划",
    write_file: "正在写入文件",
  };
  return labels[name] ?? `正在运行 ${name}`;
}

function toolTarget(activity: ToolActivity) {
  const args = activity.call.arguments ?? {};
  if (activity.state === "failed" && activity.result?.output) {
    return truncate(activity.result.output, 120);
  }
  if (activity.call.name === "read_file" && typeof args.path === "string") {
    const metadata = activity.result?.metadata ?? {};
    const startLine = positiveInteger(metadata.startLine) ?? positiveInteger(args.startLine);
    const requestedLineCount = positiveInteger(args.lineCount);
    const endLine = positiveInteger(metadata.endLine)
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
  if (activity.call.name === "run_command") {
    const command = commandDetails(activity);
    if (command) return truncate(`执行 ${command.text}`, 120);
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
  return "已完成";
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max - 1)}...` : value;
}
