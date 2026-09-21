import { useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, Trash2, X } from "lucide-react";
import { clearLogs, readLogs } from "../api/runtime";
import type { LogLevel, LogRecord } from "../types/runtime";
import { BrandMark } from "./BrandMark";

const LEVELS: Array<{ value: LogLevel | ""; label: string }> = [
  { value: "", label: "全部级别" },
  { value: "info", label: "Info" },
  { value: "error", label: "Error" },
];

const LIMIT_OPTIONS = [100, 200, 500, 1000];

// 后端在写入和读取时都把 fields 收敛到 320 字符以内（src-tauri/src/logging.rs 的
// MAX_FIELDS_CHARS），这里只是更激进的旧记录兜底，正常记录应当整行显示。
const MAX_FIELDS_CHARS = 480;

interface LogViewerDialogProps {
  onClose: () => void;
}

function levelBadgeClass(level: string): string {
  return `log-badge log-badge--${level === "error" ? "error" : "info"}`;
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
    return text.length > MAX_FIELDS_CHARS ? `${text.slice(0, MAX_FIELDS_CHARS)}…` : text;
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
  const [confirmClear, setConfirmClear] = useState(false);
  const [notice, setNotice] = useState("");
  const busy = useRef(false);

  async function load() {
    if (busy.current) return;
    busy.current = true;
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
      busy.current = false;
      setLoading(false);
    }
  }

  async function clear() {
    if (busy.current) return;
    busy.current = true;
    setLoading(true);
    setError("");
    setNotice("");
    try {
      await clearLogs(true);
      setRecords([]);
      setTotal(0);
      setConfirmClear(false);
      setNotice("历史运行日志已清理，保留本次清理记录；新日志会继续记录。");
      const result = await readLogs({ level: level || undefined, event: event.trim() || undefined, limit });
      setRecords(result.records);
      setTotal(result.total);
    } catch (reason) {
      setError(String(reason));
    } finally {
      busy.current = false;
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
            <span className="dialog-brand-mark" aria-hidden="true"><BrandMark size={18} /></span>
            <h2 id="log-viewer-title">本地运行日志</h2>
          </div>
          <button
            className="secondary-button log-viewer-refresh"
            type="button"
            disabled={loading || confirmClear}
            onClick={() => { setConfirmClear(true); setNotice(""); }}
          >
            <Trash2 size={15} />
            清理日志
          </button>
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
            disabled={loading || confirmClear}
            onClick={() => void load()}
          >
            <RefreshCw size={15} className={loading ? "spin" : ""} />
            刷新
          </button>
        </div>

        <div className="log-viewer-meta">
          <div role="status">{loading ? "处理中…" : `共 ${total} 条记录，当前展示 ${visibleRecords.length} 条`}</div>
          {notice && <p role="status">{notice}</p>}
          {confirmClear && (
            <div className="log-clear-confirm" role="group" aria-label="确认清理运行日志">
              <p>将永久清理所有级别的本地运行日志及轮转文件，不受当前筛选条件限制。对话历史不受影响。</p>
              <button type="button" className="secondary-button" disabled={loading} onClick={() => setConfirmClear(false)}>取消</button>
              <button type="button" className="danger-button" disabled={loading} onClick={() => void clear()}>确认清理</button>
            </div>
          )}
          {error && <div className="settings-error" role="alert">{error}</div>}
        </div>

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
              {record.level === "error" && (
                <div className="log-source">
                  来源：{record.threadId
                    ? <>{record.threadTitle || "对话名称不可用"} <code>（{record.threadId}）</code></>
                    : "应用运行时（无关联对话）"}
                </div>
              )}
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
