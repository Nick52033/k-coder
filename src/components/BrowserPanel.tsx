import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, ExternalLink, Globe, RotateCw } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./BrowserPanel.css";

type Navigation = { entries: string[]; cursor: number };

const MAX_ENTRIES = 50;

function normalizeAddress(raw: string): string | null {
  const text = raw.trim();
  if (!text) return null;
  if (/^https?:\/\//i.test(text)) return text;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(text)) return null;
  if (/^(localhost|127(?:\.\d+){3}|\[[0-9a-f:]+\])(:\d+)?([/?#].*)?$/i.test(text)) return `http://${text}`;
  return `https://${text}`;
}

function toReadableError(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  try {
    const parsed = JSON.stringify(reason);
    return parsed === undefined || parsed === '"undefined"' ? "未知错误" : parsed;
  } catch {
    return "未知错误";
  }
}

export function BrowserPanel({ visible }: { visible: boolean }) {
  const [draft, setDraft] = useState("");
  const [nav, setNav] = useState<Navigation>({ entries: [], cursor: -1 });
  const [nonce, setNonce] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const current = nav.cursor >= 0 ? nav.entries[nav.cursor] : null;

  useEffect(() => {
    if (visible) inputRef.current?.focus();
  }, [visible]);

  const navigate = useCallback((raw: string) => {
    const target = normalizeAddress(raw);
    if (!target) {
      setError("请输入 http 或 https 网址");
      return;
    }
    setError("");
    setLoading(true);
    setDraft(target);
    setNav((prev) => {
      let entries = [...prev.entries.slice(0, prev.cursor + 1), target];
      if (entries.length > MAX_ENTRIES) entries = entries.slice(entries.length - MAX_ENTRIES);
      return { entries, cursor: entries.length - 1 };
    });
    setNonce((value) => value + 1);
  }, []);

  const step = useCallback((delta: number) => {
    setNav((prev) => {
      const cursor = prev.cursor + delta;
      if (cursor < 0 || cursor >= prev.entries.length) return prev;
      setLoading(true);
      setDraft(prev.entries[cursor]);
      setNonce((value) => value + 1);
      return { ...prev, cursor };
    });
  }, []);

  const reload = useCallback(() => {
    if (!current) return;
    setLoading(true);
    setNonce((value) => value + 1);
  }, [current]);

  const openExternal = useCallback(async () => {
    if (!current) return;
    try {
      await openUrl(current);
      setError("");
    } catch (reason) {
      setError(toReadableError(reason));
    }
  }, [current]);

  return (
    <div className="browser-view">
      <form
        className="panel-toolbar browser-toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          navigate(draft);
        }}
      >
        <button type="button" aria-label="后退" title="后退" disabled={nav.cursor <= 0} onClick={() => step(-1)}>
          <ArrowLeft size={14} />
        </button>
        <button
          type="button"
          aria-label="前进"
          title="前进"
          disabled={nav.cursor < 0 || nav.cursor >= nav.entries.length - 1}
          onClick={() => step(1)}
        >
          <ArrowRight size={14} />
        </button>
        <button type="button" aria-label="重新加载" title="重新加载" disabled={!current} onClick={reload}>
          <RotateCw size={14} className={loading ? "browser-spinner" : undefined} />
        </button>
        <span className="browser-address">
          <Globe size={13} aria-hidden="true" />
          <input
            ref={inputRef}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="输入网址，例如 localhost:1420"
            aria-label="网址"
            spellCheck={false}
            autoComplete="off"
          />
        </span>
        <button type="button" aria-label="在系统浏览器打开" title="在系统浏览器打开" disabled={!current} onClick={() => void openExternal()}>
          <ExternalLink size={14} />
        </button>
      </form>
      {error && <div className="panel-error" role="alert">{error}</div>}
      {current ? (
        <div className="browser-host">
          <iframe key={`${nav.cursor}:${nonce}`} src={current} title="内置浏览器" onLoad={() => setLoading(false)} />
        </div>
      ) : (
        <div className="browser-empty">
          <Globe size={44} aria-hidden="true" />
          <div>
            <strong>内嵌浏览预览</strong>
            <p>输入地址后在面板内打开网页，适合预览本地开发服务器。</p>
            <p>例如：<code>localhost:1420</code>、<code>127.0.0.1:3000</code> 或 <code>example.com</code></p>
          </div>
          <small>部分网站禁止内嵌显示（X-Frame-Options），可用右上角按钮改用系统浏览器打开。</small>
        </div>
      )}
    </div>
  );
}
