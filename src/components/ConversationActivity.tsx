import {
  Brain,
  ChevronDown,
  Circle,
  CircleCheck,
  CircleDot,
  CircleX,
  Clock3,
  FileDiff,
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
  TurnTimelineItem,
} from "../types/runtime";

export function ConversationTurnActivity({
  activities,
  timeline = [],
  changes = [],
  plan,
  streaming = false,
  activityStatus = null,
  renderText,
}: {
  activities: ToolActivity[];
  timeline?: TurnTimelineItem[];
  changes?: ChangeSet[];
  plan: PlanView | null;
  streaming?: boolean;
  activityStatus?: AgentActivityStatus | null;
  renderText?: (text: string) => ReactNode;
}) {
  if (!activities.length && !timeline.length && !plan?.steps.length && !activityStatus) return null;

  return (
    <div className="turn-context">
      {plan?.steps.length ? <ConversationPlan plan={plan} /> : null}
      {timeline.length ? (
        <div className="turn-timeline">
          {timeline.map((item) => <TimelineItem item={item} changes={changes} renderText={renderText} key={timelineItemKey(item)} />)}
        </div>
      ) : activities.length ? (
        <div className="turn-timeline">
          {activities.map((activity) => <ToolActivityRow activity={activity} key={activity.call.id} />)}
        </div>
      ) : null}
      {streaming && activityStatus ? <ActivityStatusRow status={activityStatus} /> : null}
    </div>
  );
}

function TimelineItem({ item, changes, renderText }: { item: TurnTimelineItem; changes: ChangeSet[]; renderText?: (text: string) => ReactNode }) {
  if (item.type === "text") {
    return (
      <div className="turn-progress-text">
        {renderText ? renderText(item.text) : item.text}
      </div>
    );
  }
  if (item.type === "reasoning") {
    return (
      <details className="turn-disclosure turn-reasoning" open={!item.complete || undefined}>
        <summary>
          <Brain size={15} aria-hidden="true" />
          <span className="turn-disclosure-title">推理摘要</span>
          <span className="turn-disclosure-status">{item.complete ? "已完成" : "生成中"}</span>
          <ChevronDown className="turn-disclosure-chevron" size={15} aria-hidden="true" />
        </summary>
        <div className="turn-reasoning-content">
          {renderText ? renderText(item.summary) : item.summary}
        </div>
      </details>
    );
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

function ActivityStatusRow({ status }: { status: AgentActivityStatus }) {
  const labels: Record<AgentActivityStatus, string> = {
    thinking: "Thinking",
    responding: "正在组织回复",
    running_tool: "正在执行操作",
    awaiting_approval: "等待你的确认",
    finalizing: "正在整理结果",
  };
  return (
    <div className={`turn-activity-status turn-activity-status--${status}`} role="status">
      <LoaderCircle size={15} aria-hidden="true" />
      <span>{labels[status]}</span>
    </div>
  );
}

function ToolActivityRow({ activity }: { activity: ToolActivity }) {
  const outputChunks = visibleOutput(activity);
  const elapsedMs = useActivityDuration(activity);
  const command = activity.call.name === "run_command" ? commandDetails(activity.call.arguments) : null;
  return (
    <div className={`turn-timeline-tool turn-timeline-tool--${activity.state}`}>
      {activity.state === "completed" ? (
        <CircleCheck size={15} aria-hidden="true" />
      ) : activity.state === "failed" ? (
        <CircleX size={15} aria-hidden="true" />
      ) : (
        <CircleDot className="turn-tool-running" size={15} aria-hidden="true" />
      )}
      <span>
        <strong>{toolLabel(activity.call.name)}</strong>
        <small className="turn-tool-meta">
          <span>{toolTarget(activity) || activityStateLabel(activity)}</span>
          {elapsedMs !== null ? <span className="turn-tool-duration"><Clock3 size={12} aria-hidden="true" />耗时 {formatDuration(elapsedMs)}</span> : null}
        </small>
      </span>
      {command ? <CommandDetails command={command} /> : null}
      {outputChunks.length ? (
        <details className="turn-tool-output" open={activity.state === "running" || undefined}>
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
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
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

function toolTarget(activity: ToolActivity) {
  const args = activity.call.arguments ?? {};
  if (activity.call.name === "run_command") {
    const command = commandDetails(args);
    if (command) return truncate(command.text, 120);
  }
  for (const key of ["path", "filePath", "file_path", "command", "query", "url"]) {
    if (typeof args[key] === "string" && args[key]) return truncate(args[key], 88);
  }
  if (activity.state === "failed" && activity.result?.output) return truncate(activity.result.output, 88);
  return "";
}

function activityStateLabel(activity: ToolActivity) {
  if (activity.state === "running") return "执行中";
  if (activity.state === "failed") return "执行失败";
  return "已完成";
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max - 1)}...` : value;
}
