import { CSSProperties, useEffect, useState } from "react";

const STORAGE_KEY = "kcoder_panel_widths_v1";
type Widths = { sidebar: number | null; panel: number | null };
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

function readWidths(): Widths {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    const valid = (width: unknown, min: number, max: number) =>
      typeof width === "number" && Number.isFinite(width) && width >= min && width <= max ? width : null;
    return { sidebar: valid(value?.sidebar, 200, 400), panel: valid(value?.panel, 320, 900) };
  } catch {
    return { sidebar: null, panel: null };
  }
}

export function usePanelWidths(panelOpen: boolean) {
  const [preferred, setPreferred] = useState(readWidths);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  const [resizing, setResizing] = useState(false);
  useEffect(() => {
    const resize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  useEffect(() => {
    if (resizing) return;
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(preferred)); } catch { /* Optional presentation preference. */ }
  }, [preferred, resizing]);

  const sidebarVisible = viewport > 720 && (!panelOpen || viewport > 1180);
  const panelVisible = panelOpen && viewport > 720;
  const conversationMin = viewport > 1180 ? 420 : 360;
  const sidebar = sidebarVisible
    ? clamp(preferred.sidebar ?? (viewport > 1180 ? 232 : 210), 200, Math.min(400, viewport - conversationMin - (panelVisible ? 320 : 0)))
    : 0;
  const panelMax = Math.max(320, Math.min(900, viewport - sidebar - conversationMin));
  const defaultPanel = viewport > 1180 ? clamp(viewport * 0.28, 360, 440) : clamp(viewport * 0.34, 320, 400);
  const panel = panelVisible ? clamp(preferred.panel ?? defaultPanel, 320, panelMax) : 0;
  const sidebarMax = Math.max(200, Math.min(400, viewport - panel - conversationMin));

  return {
    style: { "--sidebar-width": `${sidebar}px`, "--panel-width": `${panel}px` } as CSSProperties,
    resizing,
    setResizing,
    sidebarVisible,
    panelVisible,
    sidebar,
    panel,
    sidebarMax,
    panelMax,
    setSidebar: (sidebar: number | null) => setPreferred((value) => ({ ...value, sidebar })),
    setPanel: (panel: number | null) => setPreferred((value) => ({ ...value, panel })),
  };
}
