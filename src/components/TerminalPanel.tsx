import { useEffect, useRef, useState } from "react";
import { RotateCcw, SquareTerminal } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./TerminalPanel.css";
import { closePty, ptyStatus, readPtyOutput, resizePty, startPty, writePty } from "../api/runtime";
import type { CommandState } from "../types/runtime";

type TerminalStatus = "starting" | "running" | "exited" | "error";

const POLL_INTERVAL_MS = 90;

function readCssVariable(name: string, fallback: string) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function terminalTheme() {
  return {
    background: readCssVariable("--color-surface-panel", "#161A26"),
    foreground: readCssVariable("--color-ink", "#E3E8F0"),
    cursor: readCssVariable("--color-ink", "#E3E8F0"),
    selectionBackground: readCssVariable("--color-brand-ring", "rgba(79, 138, 255, 0.28)"),
  };
}

function describeState(state: CommandState) {
  switch (state.state) {
    case "exited":
      return `进程已退出（代码 ${state.code}）`;
    case "cancelled":
      return "进程已取消";
    case "timed_out":
      return "进程已超时";
    case "failed":
      return `进程失败：${state.message}`;
    default:
      return "进程已结束";
  }
}

function toReadableError(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  try {
    return JSON.stringify(reason) ?? "未知错误";
  } catch {
    return "未知错误";
  }
}

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export function TerminalPanel({ visible }: { visible: boolean }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [status, setStatus] = useState<TerminalStatus>("starting");
  const [detail, setDetail] = useState("");
  const [epoch, setEpoch] = useState(0);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !visible) return undefined;

    let disposed = false;
    let sessionId: string | null = null;
    let running = false;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 12,
      fontFamily: readCssVariable("--font-family-mono", "Consolas, monospace"),
      scrollback: 5000,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    term.open(container);
    fit.fit();

    const inputSubscription = term.onData((data) => {
      if (!sessionId || !running) return;
      void writePty(sessionId, data).catch(() => undefined);
    });

    const themeObserver = new MutationObserver(() => {
      term.options.theme = terminalTheme();
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    const resizeObserver = new ResizeObserver(() => {
      if (container.offsetWidth === 0 || container.offsetHeight === 0) return;
      fit.fit();
      if (sessionId && running) {
        void resizePty(sessionId, Math.max(term.rows, 2), Math.max(term.cols, 2)).catch(() => undefined);
      }
    });
    resizeObserver.observe(container);

    async function poll() {
      let cursor = 0;
      while (!disposed && sessionId) {
        try {
          const page = await readPtyOutput(sessionId, cursor, 500);
          if (disposed) return;
          for (const chunk of page.chunks) term.write(chunk.text);
          cursor = page.nextCursor;
          const view = await ptyStatus(sessionId);
          if (disposed) return;
          if (view.state.state !== "running") {
            running = false;
            const tail = await readPtyOutput(sessionId, cursor, 1000).catch(() => null);
            if (tail) for (const chunk of tail.chunks) term.write(chunk.text);
            setDetail(describeState(view.state));
            setStatus("exited");
            return;
          }
        } catch (reason) {
          if (!disposed) {
            setDetail(toReadableError(reason));
            setStatus("error");
          }
          return;
        }
        await delay(POLL_INTERVAL_MS);
      }
    }

    async function boot() {
      setStatus("starting");
      setDetail("");
      try {
        const session = await startPty({
          program: "",
          rows: Math.max(term.rows, 2),
          cols: Math.max(term.cols, 2),
        });
        if (disposed) {
          void closePty(session.id).catch(() => undefined);
          return;
        }
        sessionId = session.id;
        running = true;
        setStatus("running");
        void poll();
      } catch (reason) {
        if (!disposed) {
          setDetail(toReadableError(reason));
          setStatus("error");
        }
      }
    }
    void boot();

    return () => {
      disposed = true;
      running = false;
      themeObserver.disconnect();
      resizeObserver.disconnect();
      inputSubscription.dispose();
      if (sessionId) void closePty(sessionId).catch(() => undefined);
      fitRef.current = null;
      term.dispose();
    };
  }, [epoch, visible]);

  useEffect(() => {
    if (!visible) return;
    const frame = requestAnimationFrame(() => fitRef.current?.fit());
    return () => cancelAnimationFrame(frame);
  }, [visible, epoch]);

  return (
    <div className="terminal-view">
      <div className="panel-toolbar terminal-toolbar">
        <span className="terminal-title">
          <SquareTerminal size={14} />
          <strong>终端</strong>
          <small>
            {status === "starting" && "正在启动…"}
            {status === "running" && "运行中"}
            {status === "exited" && detail}
            {status === "error" && "连接已断开"}
          </small>
        </span>
        <button
          type="button"
          title="重启终端"
          aria-label="重启终端"
          disabled={status === "starting"}
          onClick={() => setEpoch((value) => value + 1)}
        >
          <RotateCcw size={14} />
        </button>
      </div>
      <div className="terminal-host" ref={containerRef} aria-label="终端输出" />
      {status === "error" && <div className="panel-error">{detail}</div>}
    </div>
  );
}
