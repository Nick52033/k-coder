import { useEffect, useRef } from "react";

interface Props {
  side: "sidebar" | "panel";
  width: number;
  min: number;
  max: number;
  onChange: (width: number | null) => void;
  onResizing: (active: boolean) => void;
}

export function PanelResizeHandle({ side, width, min, max, onChange, onResizing }: Props) {
  const drag = useRef<{ x: number; width: number; pointerId: number } | null>(null);
  const handle = useRef<HTMLDivElement>(null);
  const direction = side === "sidebar" ? 1 : -1;
  const clamp = (value: number) => Math.min(max, Math.max(min, value));

  function finish() {
    const pointerId = drag.current?.pointerId;
    drag.current = null;
    if (pointerId !== undefined && handle.current?.hasPointerCapture(pointerId)) {
      handle.current.releasePointerCapture(pointerId);
    }
    onResizing(false);
  }

  useEffect(() => {
    window.addEventListener("blur", finish);
    window.addEventListener("resize", finish);
    return () => {
      window.removeEventListener("blur", finish);
      window.removeEventListener("resize", finish);
      finish();
    };
  }, [onResizing]);

  return (
    <div
      ref={handle}
      className={`panel-resize-handle panel-resize-handle--${side}`}
      role="separator"
      aria-label={side === "sidebar" ? "调整侧边栏宽度" : "调整右侧面板宽度"}
      aria-orientation="vertical"
      aria-valuemin={Math.round(min)}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(width)}
      aria-valuetext={`${Math.round(width)} 像素`}
      tabIndex={0}
      title="拖动调整宽度，双击恢复默认；方向键微调"
      onPointerDown={(event) => {
        if (event.button !== 0 || !event.isPrimary) return;
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { x: event.clientX, width, pointerId: event.pointerId };
        onResizing(true);
      }}
      onPointerMove={(event) => {
        if (drag.current?.pointerId !== event.pointerId) return;
        onChange(clamp(drag.current.width + direction * (event.clientX - drag.current.x)));
      }}
      onPointerUp={finish}
      onPointerCancel={finish}
      onLostPointerCapture={finish}
      onDoubleClick={() => onChange(null)}
      onKeyDown={(event) => {
        if (event.key === "Escape" && drag.current) {
          event.preventDefault();
          event.stopPropagation();
          onChange(drag.current.width);
          finish();
        } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
          event.preventDefault();
          const delta = (event.shiftKey ? 40 : 10) * (event.key === "ArrowRight" ? 1 : -1);
          onChange(clamp(width + direction * delta));
        } else if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          onChange(event.key === "Home" ? min : max);
        } else if (event.key === "Enter") {
          event.preventDefault();
          onChange(null);
        }
      }}
    />
  );
}
