import { useEffect, useRef, useState } from "react";
import { RotateCcw, SquareTerminal } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./TerminalPanel.css";
import { closePty, readPtyOutput, resizePty, startPty, waitPty, writePty } from "../api/runtime";
import type { CommandState } from "../types/runtime";

type TerminalStatus = "starting" | "running" | "exited" | "error";

const OUTPUT_POLL_INTERVAL_MS = 32;
const INPUT_BATCH_INTERVAL_MS = 12;

function readCssVariable(name: string, fallback: string) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

function terminalTheme() {
  const dark = document.documentElement.dataset.theme === "dark";
  return {
    background: readCssVariable("--color-surface-panel", "#161A26"),
    foreground: readCssVariable("--color-ink", "#E3E8F0"),
    cursor: readCssVariable("--color-brand-light", "#7FAEFF"),
    cursorAccent: readCssVariable("--color-surface-panel", "#161A26"),
    selectionBackground: readCssVariable("--color-brand-ring", "rgba(79, 138, 255, 0.28)"),
    selectionInactiveBackground: dark ? "rgba(127, 174, 255, 0.14)" : "rgba(47, 111, 228, 0.12)",
    black: dark ? "#11151F" : "#172033",
    red: dark ? "#F87171" : "#B4232C",
    green: dark ? "#34D399" : "#13734F",
    yellow: dark ? "#EAB54B" : "#8A5A00",
    blue: dark ? "#7FAEFF" : "#2F6FE4",
    magenta: dark ? "#C59AF4" : "#8151B4",
    cyan: dark ? "#67D5E8" : "#167A8B",
    white: dark ? "#D7DDEA" : "#D8DEE9",
    brightBlack: dark ? "#707A8C" : "#68758A",
    brightRed: dark ? "#FCA5A5" : "#C24145",
    brightGreen: dark ? "#6EE7B7" : "#158A61",
    brightYellow: dark ? "#F7CD75" : "#AD6800",
    brightBlue: dark ? "#A9C6FF" : "#2459B8",
    brightMagenta: dark ? "#D8B4FE" : "#6F3E9B",
    brightCyan: dark ? "#A5E8F0" : "#0F6978",
    brightWhite: dark ? "#F1F5FA" : "#FFFFFF",
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
  const termRef = useRef<Terminal | null>(null);
  const refreshSizeRef = useRef<(() => void) | null>(null);
  const [status, setStatus] = useState<TerminalStatus>("starting");
  const [detail, setDetail] = useState("");
  const [epoch, setEpoch] = useState(0);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return undefined;
    const host = container;

    let disposed = false;
    let sessionId: string | null = null;
    let running = false;
    let outputCursor = 0;
    let inputBuffer = "";
    let inputTimer: number | null = null;
    let inputChain = Promise.resolve();
    let resizeFrame: number | null = null;
    let resizeInFlight = false;
    let pendingSize: { rows: number; cols: number } | null = null;
    let lastSentSize: { rows: number; cols: number } | null = null;

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: "bar",
      cursorWidth: 1,
      fontSize: 12,
      fontFamily: readCssVariable("--font-family-mono", "Consolas, monospace"),
      fontWeight: "400",
      fontWeightBold: "600",
      letterSpacing: 0,
      lineHeight: 1.25,
      minimumContrastRatio: 4.5,
      scrollback: 5000,
      smoothScrollDuration: 0,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    fitRef.current = fit;
    termRef.current = term;
    term.loadAddon(fit);
    term.open(container);
    fit.fit();

    function showConnectionError(reason: unknown) {
      if (disposed) return;
      running = false;
      setDetail(toReadableError(reason));
      setStatus("error");
    }

    function flushInput() {
      if (inputTimer !== null) {
        window.clearTimeout(inputTimer);
        inputTimer = null;
      }
      if (!sessionId || !running || !inputBuffer) return;
      const targetSessionId = sessionId;
      const input = inputBuffer;
      inputBuffer = "";
      inputChain = inputChain
        .then(() => writePty(targetSessionId, input))
        .catch((reason) => showConnectionError(reason));
    }

    const inputSubscription = term.onData((data) => {
      if (!sessionId || !running) return;
      inputBuffer += data;
      if (/[\r\n\x03\x04\x1b]/.test(data)) {
        flushInput();
        return;
      }
      if (inputTimer === null) {
        inputTimer = window.setTimeout(flushInput, INPUT_BATCH_INTERVAL_MS);
      }
    });

    const themeObserver = new MutationObserver(() => {
      term.options.theme = terminalTheme();
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    async function flushResize() {
      if (resizeInFlight) return;
      resizeInFlight = true;
      while (!disposed && running && sessionId && pendingSize) {
        const size = pendingSize;
        pendingSize = null;
        if (lastSentSize?.rows === size.rows && lastSentSize.cols === size.cols) continue;
        try {
          await resizePty(sessionId, size.rows, size.cols);
          lastSentSize = size;
        } catch (reason) {
          showConnectionError(reason);
        }
      }
      resizeInFlight = false;
    }

    function refreshSize() {
      if (host.offsetWidth === 0 || host.offsetHeight === 0) return;
      fit.fit();
      if (sessionId && running) {
        pendingSize = { rows: Math.max(term.rows, 2), cols: Math.max(term.cols, 2) };
        void flushResize();
      }
    }

    function scheduleSizeRefresh() {
      if (resizeFrame !== null) return;
      resizeFrame = requestAnimationFrame(() => {
        resizeFrame = null;
        refreshSize();
      });
    }

    refreshSizeRef.current = scheduleSizeRefresh;
    const resizeObserver = new ResizeObserver(scheduleSizeRefresh);
    resizeObserver.observe(host);

    async function pollOutput(targetSessionId: string) {
      while (!disposed && running) {
        try {
          const page = await readPtyOutput(targetSessionId, outputCursor, 500);
          if (disposed) return;
          if (page.chunks.length > 0) term.write(page.chunks.map((chunk) => chunk.text).join(""));
          outputCursor = page.nextCursor;
        } catch (reason) {
          showConnectionError(reason);
          return;
        }
        await delay(OUTPUT_POLL_INTERVAL_MS);
      }
    }

    async function drainOutput(targetSessionId: string) {
      for (let attempt = 0; attempt < 2 && !disposed; attempt += 1) {
        const page = await readPtyOutput(targetSessionId, outputCursor, 1000).catch(() => null);
        if (!page) return;
        if (page.chunks.length > 0) term.write(page.chunks.map((chunk) => chunk.text).join(""));
        outputCursor = page.nextCursor;
        if (page.chunks.length === 0) return;
        await delay(16);
      }
    }

    async function monitorSession(targetSessionId: string) {
      const outputTask = pollOutput(targetSessionId);
      try {
        const view = await waitPty(targetSessionId);
        running = false;
        await outputTask;
        await drainOutput(targetSessionId);
        if (disposed) return;
        setDetail(describeState(view.state));
        setStatus("exited");
      } catch (reason) {
        showConnectionError(reason);
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
        lastSentSize = { rows: session.rows, cols: session.cols };
        setStatus("running");
        void monitorSession(session.id);
      } catch (reason) {
        showConnectionError(reason);
      }
    }
    void boot();

    return () => {
      disposed = true;
      running = false;
      if (inputTimer !== null) window.clearTimeout(inputTimer);
      if (resizeFrame !== null) cancelAnimationFrame(resizeFrame);
      themeObserver.disconnect();
      resizeObserver.disconnect();
      inputSubscription.dispose();
      const closingSessionId = sessionId;
      if (closingSessionId) void closePty(closingSessionId).catch(() => undefined);
      fitRef.current = null;
      termRef.current = null;
      refreshSizeRef.current = null;
      term.dispose();
    };
  }, [epoch]);

  useEffect(() => {
    if (!visible) return;
    const frame = requestAnimationFrame(() => {
      refreshSizeRef.current?.();
      termRef.current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [visible, epoch]);

  const stateLabel = status === "starting"
    ? "正在启动…"
    : status === "running"
      ? "运行中"
      : status === "exited"
        ? detail
        : "连接已断开";

  return (
    <div className="terminal-view" data-status={status}>
      <div className="panel-toolbar terminal-toolbar">
        <span className="terminal-title">
          <SquareTerminal size={14} />
          <strong>终端</strong>
          <span className="terminal-state" aria-live="polite">
            <span className="terminal-state-dot" aria-hidden="true" />
            <small>{stateLabel}</small>
          </span>
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
