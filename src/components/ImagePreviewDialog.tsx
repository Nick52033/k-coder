import { useEffect, useRef } from "react";
import { X } from "lucide-react";

import type { ImageAttachment } from "../types/runtime";

interface ImagePreviewDialogProps {
  image: ImageAttachment | null;
  onClose: () => void;
}

export function ImagePreviewDialog({ image, onClose }: ImagePreviewDialogProps) {
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!image) return;
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    closeButtonRef.current?.focus();

    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") onCloseRef.current();
      if (event.key === "Tab") {
        event.preventDefault();
        closeButtonRef.current?.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.body.style.overflow = previousOverflow;
      document.removeEventListener("keydown", handleKeyDown);
      previousFocus?.focus();
    };
  }, [image]);

  if (!image) return null;

  return (
    <div className="image-preview-backdrop" onMouseDown={onClose}>
      <section
        className="image-preview-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="image-preview-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="image-preview-header">
          <h2 id="image-preview-title" title={image.name}>{image.name}</h2>
          <button
            ref={closeButtonRef}
            type="button"
            aria-label="关闭图片预览"
            title="关闭"
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </header>
        <div className="image-preview-stage" onMouseDown={onClose}>
          <img
            src={image.dataUrl}
            alt={image.name}
            onMouseDown={(event) => event.stopPropagation()}
          />
        </div>
      </section>
    </div>
  );
}
