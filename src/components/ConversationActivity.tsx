import {
  Activity,
  Brain,
  ChevronDown,
  Circle,
  CircleCheck,
  CircleDot,
  CircleX,
  Clock3,
  FileDiff,
  FileText,
  ListChecks,
  LoaderCircle,
  SquareTerminal,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
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
  if (!activities.length && !timeline.length && !plan?.steps.length && !activityStatus) return null;

  const finalResponse = finalMessageId
    ? timeline.find((item): item is Extract<TurnTimelineItem, { type: "text" }> => item.type === "text" && item.id === finalMessageId)
    : null;
  const processTimeline = finalResponse ? timeline.filter((item) => item !== finalResponse) : timeline;
  const terminalEvent = [...processTimeline].reverse().find(
    (item): item is Extract<TurnTimelineItem, { type: "event" }> => item.type === "event" && isTerminalEvent(item.kind),
  );
  const processItems = processTimeline.filter((item) => item !== terminalEvent);
  const hasItems = Boolean(plan?.steps.length || activities.length || processItems.length);
  const hasProcess = hasItems || Boolean(activityStatus);
  const toolCount = processTimeline.filter((item) => item.type === "tool").length || activities.length;
  const summaryTitle = terminalEvent?.durationMs !== undefined
    ? `执行了 ${formatDuration(terminalEvent.durationMs)}`
    : "执行过程";
  const statusLabel = activityStatus ? {
    thinking: "思考中",
    responding: "生成回复中",
    running_tool: "处理工具结果中",
    awaiting_approval: "等待确认",
    finalizing: "整理结果中",
  }[activityStatus] : null;
  const groupedProcessTimeline = groupConsecutiveReasoning(processTimeline);
  const processContent = (
    <div className={streaming ? "turn-execution-live" : "turn-execution-content"}>
      {plan?.steps.length ? <ConversationPlan plan={plan} /> : null}
      {processTimeline.length ? (
        <div className="turn-timeline">
          {groupedProcessTimeline.map((entry) => entry.type === "reasoning_group" ? (
            <ReasoningGroup
              items={entry.items}
              active={streaming}
              renderText={renderText}
              key={`reasoning-group-${entry.items.map((item) => item.itemId).join("-")}`}
            />
          ) : (
            <TimelineItem
              item={entry.item}
              changes={changes}
              active={streaming}
              renderText={renderText}
              key={timelineItemKey(entry.item)}
            />
          ))}
        </div>
      ) : activities.length ? (
        <div className="turn-timeline">
          {activities.map((activity) => <ToolActivityRow activity={activity} key={activity.call.id} />)}
        </div>
      ) : null}
    </div>
  );

  return (
    <div className="turn-context">
      {hasProcess && streaming ? (
        <details className="turn-disclosure turn-execution turn-execution--live" open>
          <summary>
            <LoaderCircle size={15} aria-hidden="true" />
            <span className="turn-disclosure-title">{statusLabel}</span>
            <span className="turn-live-status-dots" aria-hidden="true"><i /><i /><i /></span>
          </summary>
          {processContent}
        </details>
      ) : null}
      {hasProcess && !streaming ? (
        <details className="turn-disclosure turn-execution" open={streaming || undefined}>
          <summary>
            <Activity size={15} aria-hidden="true" />
            <span className="turn-disclosure-title">{summaryTitle}</span>
            <span className="turn-disclosure-status">{toolCount ? `${toolCount} 个操作` : "已完成"}</span>
            <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
          </summary>
          {processContent}
        </details>
      ) : null}
      {finalResponse ? (
        <div className="turn-final-response">
          <TimelineItem item={finalResponse} changes={changes} renderText={renderText} />
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
  active = false,
  renderText,
}: {
  item: TurnTimelineItem;
  changes: ChangeSet[];
  active?: boolean;
  renderText?: (text: string) => ReactNode;
}) {
  if (item.type === "text") {
    return (
      <div className="turn-progress-text">
        {renderText ? renderText(item.text) : item.text}
      </div>
    );
  }
  if (item.type === "reasoning") {
    return <ReasoningGroup items={[item]} active={active} renderText={renderText} />;
  }
  if (item.type === "event") {
    const Icon = item.kind === "turn_completed"
      ? CircleCheck
      : item.kind === "turn_failed"
        ? CircleX
        : item.kind === "turn_cancelled"
          ? Circle
          : CircleDot;
    return (
      <div className={`turn-timeline-event turn-timeline-event--${item.kind}`}>
        <Icon size={15} aria-hidden="true" />
        <span>
          <strong>{item.title}</strong>
          {item.detail ? <small>{item.detail}</small> : null}
          {item.kind === "change_applied" ? <ChangeDetails change={findChange(item, changes)} /> : null}
        </span>
      </div>
    );
  }
  return <ToolActivityRow activity={item.activity} />;
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
  | { type: "item"; item: Exclude<TurnTimelineItem, { type: "reasoning" }> };

function groupConsecutiveReasoning(items: TurnTimelineItem[]): TimelineRenderEntry[] {
  const grouped: TimelineRenderEntry[] = [];
  for (const item of items) {
    const previous = grouped[grouped.length - 1];
    if (item.type === "reasoning") {
      if (previous?.type === "reasoning_group"
        && Boolean(previous.items[0]?.complete) === Boolean(item.complete)) previous.items.push(item);
      else grouped.push({ type: "reasoning_group", items: [item] });
    } else {
      grouped.push({ type: "item", item });
    }
  }
  return grouped;
}

function ReasoningGroup({
  items,
  active,
  renderText,
}: {
  items: ReasoningTimelineItem[];
  active: boolean;
  renderText?: (text: string) => ReactNode;
}) {
  const complete = items.every((item) => item.complete);
  const status = complete && items.length > 1 ? `${items.length} 段` : complete ? "已完成" : "生成中";
  return (
    <details className="turn-disclosure turn-reasoning" open={active || !complete || undefined}>
      <summary>
        <Brain size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">思考内容</span>
        <span className="turn-disclosure-status">{status}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="turn-reasoning-content">
        {items.map((item) => (
          <div className="turn-reasoning-segment" key={`${item.turnId}-${item.itemId}`}>
            {renderText ? renderText(item.summary) : item.summary}
          </div>
        ))}
      </div>
    </details>
  );
}


function ToolActivityRow({ activity }: { activity: ToolActivity }) {
  const outputChunks = visibleOutput(activity);
  const elapsedMs = useActivityDuration(activity);
  const command = activity.call.name === "run_command" ? commandDetails(activity.call.arguments) : null;
  const fileDetails = command ? null : fileActivityDetails(activity);
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
      ) : activity.state === "cancelled" ? (
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

interface FileActivityDetailsValue {
  kind: "patch" | "write";
  label: string;
  content: string;
  meta: string;
  truncated: boolean;
}

function FileActivityDetails({ details }: { details: FileActivityDetailsValue }) {
  return (
    <details className="turn-tool-details">
      <summary>
        <FileText size={14} aria-hidden="true" />
        <span>{details.label}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </summary>
      <div className="turn-command-details">
        <pre>{details.content}</pre>
        {details.meta || details.truncated ? (
          <small>
            {details.meta}
            {details.meta && details.truncated ? " · " : ""}
            {details.truncated ? "内容过长，仅显示前 64 KiB" : ""}
          </small>
        ) : null}
      </div>
    </details>
  );
}

function CommandDetails({ command }: { command: CommandDetailsValue }) {
  return (
    <details className="turn-tool-details">
      <summary>
        <SquareTerminal size={14} aria-hidden="true" />
        <span>查看命令</span>
        <ChevronDown size={14} aria-hidden="true" />
      </summary>
      <div className="turn-command-details">
        <pre>{command.text}</pre>
        {command.cwd || command.timeoutMs ? (
          <small>
            {command.cwd ? `工作目录：${command.cwd}` : ""}
            {command.cwd && command.timeoutMs ? " · " : ""}
            {command.timeoutMs ? `超时：${formatDuration(command.timeoutMs)}` : ""}
          </small>
        ) : null}
      </div>
    </details>
  );
}

function ChangeDetails({ change }: { change: ChangeSet | null }) {
  if (!change) return null;
  return (
    <details className="turn-change-details">
      <summary>
        <FileDiff size={14} aria-hidden="true" />
        <span>查看变更</span>
        <small>{change.files.length} 个文件</small>
        <ChevronDown size={14} aria-hidden="true" />
      </summary>
      <div className="turn-change-files">
        {change.files.map((file) => (
          <details className="turn-change-file" key={`${change.id}-${file.path}`}>
            <summary>
              <span>{changeOperationLabel(file.operation)} {file.path}</span>
              <ChevronDown size={13} aria-hidden="true" />
            </summary>
            <pre>{file.unifiedDiff || "没有可显示的 Diff"}</pre>
          </details>
        ))}
      </div>
    </details>
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
}

function commandDetails(argumentsValue: Record<string, unknown>): CommandDetailsValue | null {
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
  };
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
    kind,
    label,
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
  return ({ add: "新增", modify: "修改", delete: "删除", move: "移动" })[operation];
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

function ConversationPlan({ plan }: { plan: PlanView }) {
  const completed = plan.steps.filter((step) => step.status === "completed" || step.status === "skipped").length;
  const active = plan.steps.some((step) => step.status === "in_progress");

  return (
    <details className="turn-disclosure turn-plan" open={active || undefined}>
      <summary>
        <ListChecks size={15} aria-hidden="true" />
        <span className="turn-disclosure-title">计划</span>
        <span className="turn-disclosure-status">{completed}/{plan.steps.length}</span>
        <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
      </summary>
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
    const command = commandDetails(args);
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
  if (activity.state === "running") return "执行中";
  if (activity.state === "failed") return "执行失败";
  if (activity.state === "cancelled") return "已取消";
  return "已完成";
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max - 1)}...` : value;
}
