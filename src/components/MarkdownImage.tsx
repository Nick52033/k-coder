import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { readBrowserArtifact, readMessageImage } from "../api/runtime";
import { useWorkbenchStore } from "../stores/workbenchStore";
import { ImagePreviewDialog } from "./ImagePreviewDialog";

export const ARTIFACT_IMAGE_NAME = /^\d{10,20}-[0-9a-f]{16,64}\.png$/i;
const RASTER_DATA_URL = /^data:image\/(?:png|jpeg|gif|webp|bmp);base64,[a-z0-9+/]+={0,2}$/i;

export function isDisplayImageSource(source: string): boolean {
  if (source.length > 12 * 1024 * 1024 || !source || /[\u0000-\u001f]/.test(source)) return false;
  if (RASTER_DATA_URL.test(source)) return true;
  if (/^https?:\/\//i.test(source)) {
    try {
      const url = new URL(source);
      return !url.username && !url.password;
    } catch { return false; }
  }
  // Other URI schemes (including file:, javascript: and SVG data) are never loaded.
  if (/^[a-z][a-z0-9+.-]*:/i.test(source) && !/^[a-z]:[\\/]/i.test(source)) return false;
  if (source.startsWith("//") || source.startsWith("\\\\")) return false;
  return /\.(?:png|jpe?g|gif|webp|bmp)$/i.test(source);
}

export function MarkdownImage({ source, alt }: { source: string; alt: string }) {
  const threadId = useWorkbenchStore(state => state.activeThreadId);
  return <LoadedImage key={`${threadId}:${source}`} source={source} name={alt || source.split(/[\\/]/).pop() || "图片"} threadId={threadId} />;
}

function LoadedImage({ source, name, threadId }: { source: string; name: string; threadId: string | null }) {
  const direct = RASTER_DATA_URL.test(source) || /^https?:\/\//i.test(source);
  const [url, setUrl] = useState(direct && isDisplayImageSource(source) ? source : "");
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const allowed = isDisplayImageSource(source);

  useEffect(() => {
    if (!allowed || direct) return;
    let alive = true;
    const task = Promise.resolve().then(() => ARTIFACT_IMAGE_NAME.test(source)
      ? readBrowserArtifact(source)
      : threadId ? readMessageImage(threadId, decodeURIComponent(source)) : Promise.reject(new Error("no workspace")));
    task.then(value => {
      if (!RASTER_DATA_URL.test(value)) throw new Error("invalid image response");
      if (alive) setUrl(value);
    }).catch(() => { if (alive) setFailed(true); });
    return () => { alive = false; };
  }, [allowed, direct, source, threadId]);

  if (!allowed) return <span className="markdown-image-placeholder">无法显示图片：{name}</span>;
  if (failed) return <span className="markdown-image-placeholder" role="status">图片加载失败：{name}</span>;
  if (!url) return <span className="markdown-image-loading">加载图片…</span>;
  return <>
    <button type="button" className="markdown-image-button" aria-label={`查看图片 ${name}`} title={`查看 ${name}`} onClick={event => { event.preventDefault(); event.stopPropagation(); setExpanded(true); }}>
      <img className="markdown-image" src={url} alt={name} loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(true)} />
    </button>
    {expanded && createPortal(<ImagePreviewDialog image={{ name, dataUrl: url }} onClose={() => setExpanded(false)} />, document.body)}
  </>;
}
