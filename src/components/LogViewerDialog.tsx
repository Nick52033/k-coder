import { useEffect, useMemo, useState } from "react";
import { RefreshCw, ScrollText, X } from "lucide-react";
import { readLogs } from "../api/runtime";
import type { LogLevel, LogRecord } from "../types/runtime";

const LEVELS: Array<{ value: LogLevel | ""; label: string }> = [
  { value: "", label: "全部级别" },
  { value: "trace", label: "Trace" },
  { value: "debug", label: "Debug" },
  { value: "info", label: "Info" },
  { value: "warn", label: "Warn" },
  { value: "error", label: "Error" },
];

const LIMIT_OPTIONS = [100, 200, 500, 1000];

interface LogViewerDialogProps {
  onClose: () => void;
}

function levelBadgeClass(level: string): string {
  switch (level.toLowerCase()) {
    case "error":
      return "log-badge log-badge--error";
    case "warn":
      return "log-badge log-badge--warn";
    case "info":
      return "log-badge log-badge--info";
    case "debug":
      return "log-badge log-badge--debug";
    default:
      return "log-badge log-badge--trace";
  }
}

function formatTimestamp(ms: number): string {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return String(ms);
  return date.toLocaleString();
}

function summarizeFields(fields: unknown): string {
  if (fields === null || fields === undefined) return "";
  if (typeof fields === "string") return fields;
  try {
    const text = JSON.stringify(fields);
    return text.length > 240 ? `${text.slice(0, 240)}…` : text;
  } catch {
    return String(fields);
  }
}

export function LogViewerDialog({ onClose }: LogViewerDialogProps) {
  const [level, setLevel] = useState<LogLevel | "">("");
  const [event, setEvent] = useState("");
  const [limit, setLimit] = useState(200);
  const [records, setRecords] = useState<LogRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  async function load() {
    setLoading(true);
    setError("");
    try {
      const result = await readLogs({
        level: level || undefined,
        event: event.trim() || undefined,
        limit,
      });
      setRecords(result.records);
      setTotal(result.total);
    } catch (reason) {
      setError(String(reason));
      setRecords([]);
      setTotal(0);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
    // 仅首次挂载自动加载一次，后续由用户点击“刷新”触发，避免每次按键都请求。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const visibleRecords = useMemo(() => records.slice().reverse(), [records]);

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="log-viewer-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="log-viewer-title"
      >
        <header className="log-viewer-header">
          <div className="log-viewer-title">
            <ScrollText size={18} />
            <h2 id="log-viewer-title">本地运行日志</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            aria-label="关闭日志查看器"
            title="关闭"
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </header>

        <div className="log-viewer-toolbar">
          <label className="log-viewer-field">
            <span>级别</span>
            <select
              value={level}
              onChange={(event) => setLevel(event.target.value as LogLevel | "")}
            >
              {LEVELS.map((option) => (
                <option value={option.value} key={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <label className="log-viewer-field log-viewer-field--grow">
            <span>事件</span>
            <input
              type="text"
              value={event}
              maxLength={120}
              placeholder="按事件名过滤，如 turn_failed"
              onChange={(event) => setEvent(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void load();
              }}
            />
          </label>
          <label className="log-viewer-field">
            <span>条数</span>
            <select
              value={limit}
              onChange={(event) => setLimit(Number(event.target.value))}
            >
              {LIMIT_OPTIONS.map((value) => (
                <option value={value} key={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          <button
            className="primary-button log-viewer-refresh"
            type="button"
            disabled={loading}
            onClick={() => void load()}
          >
            <RefreshCw size={15} className={loading ? "spin" : ""} />
            刷新
          </button>
        </div>

        <div className="log-viewer-meta">
          {loading ? "加载中…" : `共 ${total} 条记录，当前展示 ${visibleRecords.length} 条`}
        </div>

        {error && <div className="settings-error" role="alert">{error}</div>}

        <div className="log-viewer-body">
          {!loading && visibleRecords.length === 0 && !error && (
            <div className="log-viewer-empty">没有匹配的日志记录。</div>
          )}
          {visibleRecords.map((record, index) => (
            <article className="log-row" key={`${record.timestampMs}-${index}`}>
              <div className="log-row-head">
                <span className={levelBadgeClass(record.level)}>{record.level}</span>
                <span className="log-event">{record.event}</span>
                <time className="log-time">{formatTimestamp(record.timestampMs)}</time>
              </div>
              {record.fields !== null && record.fields !== undefined && (
                <pre className="log-fields">{summarizeFields(record.fields)}</pre>
              )}
            </article>
          ))}
        </div>
      </section>
    </div>
  );
}
