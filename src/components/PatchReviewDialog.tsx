import { useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  Code2,
  Columns2,
  FileDiff,
  RefreshCw,
  Rows3,
  Undo2,
  X,
} from "lucide-react";
import { errorMessage, previewPatch } from "../api/runtime";
import type {
  ApprovalRequest,
  ApprovalResolution,
  ChangeSet,
  PatchFilePreview,
  PatchPreview,
} from "../types/runtime";

interface PatchReviewDialogProps {
  request?: ApprovalRequest | null;
  change?: ChangeSet | null;
  error?: string;
  onResolve?: (resolution: ApprovalResolution) => Promise<boolean>;
  onUndo?: (changeId: string) => Promise<boolean>;
  onClose?: () => void;
}

export function PatchReviewDialog({
  request = null,
  change = null,
  error = "",
  onResolve,
  onUndo,
  onClose,
}: PatchReviewDialogProps) {
  const initialPreview = useMemo<PatchPreview | null>(() => {
    if (request?.preview) return request.preview;
    if (!change) return null;
    return {
      patch: "",
      files: change.files,
      totalSnapshotBytes: change.files.reduce(
        (total, file) =>
          total + byteLength(file.beforeContent) + byteLength(file.afterContent),
        0,
      ),
    };
  }, [change, request]);
  const [preview, setPreview] = useState(initialPreview);
  const [patchText, setPatchText] = useState(initialPreview?.patch ?? "");
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(
    () => new Set(initialPreview?.files.map((file) => file.path) ?? []),
  );
  const [activePath, setActivePath] = useState(initialPreview?.files[0]?.path ?? "");
  const [viewMode, setViewMode] = useState<"unified" | "side_by_side">("unified");
  const [editingPatch, setEditingPatch] = useState(false);
  const [busy, setBusy] = useState(false);
  const [localError, setLocalError] = useState("");
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const isApproval = Boolean(request);

  useEffect(() => {
    setPreview(initialPreview);
    setPatchText(initialPreview?.patch ?? "");
    setSelectedPaths(new Set(initialPreview?.files.map((file) => file.path) ?? []));
    setActivePath(initialPreview?.files[0]?.path ?? "");
    setEditingPatch(false);
    setLocalError("");
  }, [initialPreview]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const dialogElement = dialog;
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusableSelector =
      'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';
    const focusable = () =>
      Array.from(dialogElement.querySelectorAll<HTMLElement>(focusableSelector));
    (focusable()[0] ?? dialogElement).focus();

    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape" && !isApproval) {
        event.preventDefault();
        closeRef.current?.();
        return;
      }
      if (event.key !== "Tab") return;
      const elements = focusable();
      if (!elements.length) {
        event.preventDefault();
        dialogElement.focus();
        return;
      }
      const first = elements[0];
      const last = elements[elements.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    dialogElement.addEventListener("keydown", handleKeyDown);
    return () => {
      dialogElement.removeEventListener("keydown", handleKeyDown);
      previousFocus?.focus();
    };
  }, [isApproval]);

  if (!preview) return null;

  const activeFile =
    preview.files.find((file) => file.path === activePath) ?? preview.files[0] ?? null;
  const patchIsCurrent = !request || request.toolName !== "apply_patch" || patchText === preview.patch;
  const selectedCount = selectedPaths.size;

  function togglePath(path: string) {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  async function refreshPreview() {
    setBusy(true);
    setLocalError("");
    try {
      const next = await previewPatch(patchText);
      setPreview(next);
      setSelectedPaths(new Set(next.files.map((file) => file.path)));
      setActivePath(next.files[0]?.path ?? "");
      setEditingPatch(false);
    } catch (error) {
      setLocalError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function resolve(action: "approved" | "rejected") {
    if (!request || !onResolve) return;
    setBusy(true);
    setLocalError("");
    const selectedFiles = preview!.files.filter((file) => selectedPaths.has(file.path));
    const resolution: ApprovalResolution = {
      action,
      patch: action === "approved" && request.toolName === "apply_patch" ? patchText : null,
      selectedPaths: action === "approved" ? selectedFiles.map((file) => file.path) : [],
      expectedHashes:
        action === "approved"
          ? selectedFiles.map((file) => ({ path: file.path, beforeHash: file.beforeHash }))
          : [],
    };
    const resolved = await onResolve(resolution);
    if (!resolved) setBusy(false);
  }

  async function undo() {
    if (!change || !onUndo) return;
    setBusy(true);
    setLocalError("");
    const undone = await onUndo(change.id);
    if (undone) onClose?.();
    else setBusy(false);
  }

  return (
    <div
      className="review-overlay"
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-label="代码变更审阅"
      aria-busy={busy}
      tabIndex={-1}
    >
      <header className="review-header">
        <div className="review-heading">
          <FileDiff size={18} />
          <div>
            <h2>{isApproval ? "待审阅变更" : change?.undone ? "已撤销变更" : "已应用变更"}</h2>
            <span>{preview.files.length} 个文件 · {formatBytes(preview.totalSnapshotBytes)}</span>
          </div>
        </div>
        <div className="review-header-actions">
          {request?.toolName === "apply_patch" && (
            <button
              className={`review-tool-button ${editingPatch ? "review-tool-button--active" : ""}`}
              type="button"
              aria-pressed={editingPatch}
              onClick={() => setEditingPatch((value) => !value)}
            >
              <Code2 size={15} />
              编辑 Patch
            </button>
          )}
          <div className="segmented-control" aria-label="Diff 显示模式">
            <button
              type="button"
              className={viewMode === "unified" ? "is-active" : ""}
              aria-pressed={viewMode === "unified"}
              title="统一 Diff"
              aria-label="统一 Diff"
              onClick={() => setViewMode("unified")}
            >
              <Rows3 size={15} />
            </button>
            <button
              type="button"
              className={viewMode === "side_by_side" ? "is-active" : ""}
              aria-pressed={viewMode === "side_by_side"}
              title="并排 Diff"
              aria-label="并排 Diff"
              onClick={() => setViewMode("side_by_side")}
            >
              <Columns2 size={15} />
            </button>
          </div>
          {!isApproval && (
            <button className="icon-button" type="button" title="关闭" aria-label="关闭" onClick={onClose}>
              <X size={17} />
            </button>
          )}
        </div>
      </header>

      <div className="review-body">
        <aside className="review-files" aria-label="变更文件">
          {preview.files.map((file) => (
            <div className={`review-file ${file.path === activeFile?.path ? "review-file--active" : ""}`} key={file.path}>
              {isApproval && (
                <input
                  type="checkbox"
                  aria-label={`选择 ${file.path}`}
                  checked={selectedPaths.has(file.path)}
                  onChange={() => togglePath(file.path)}
                />
              )}
              <button
                type="button"
                aria-current={file.path === activeFile?.path ? "true" : undefined}
                onClick={() => setActivePath(file.path)}
              >
                <span>{file.destinationPath ?? file.path}</span>
                <small className={`operation-label operation-label--${file.operation}`}>
                  {operationLabel(file)}
                </small>
              </button>
            </div>
          ))}
        </aside>

        <section className="review-diff">
          {editingPatch ? (
            <div className="patch-editor">
              <textarea
                aria-label="Patch 内容"
                value={patchText}
                spellCheck={false}
                onChange={(event) => setPatchText(event.target.value)}
              />
              <button className="review-tool-button" type="button" disabled={busy} onClick={() => void refreshPreview()}>
                <RefreshCw className={busy ? "spin" : ""} size={15} />
                刷新预览
              </button>
            </div>
          ) : activeFile ? (
            <DiffContent file={activeFile} mode={viewMode} />
          ) : (
            <div className="review-empty">没有可显示的文件</div>
          )}
        </section>
      </div>

      <footer className="review-footer">
        <div className="review-error" role="alert" aria-live="polite">
          {localError || error || (!patchIsCurrent ? "Patch 已修改，预览待刷新" : "")}
        </div>
        <div className="review-actions">
          {isApproval ? (
            <>
              <button className="review-secondary-button" type="button" disabled={busy} onClick={() => void resolve("rejected")}>
                <X size={15} />
                拒绝
              </button>
              <button
                className="review-primary-button"
                type="button"
                disabled={busy || selectedCount === 0 || !patchIsCurrent}
                onClick={() => void resolve("approved")}
              >
                {busy ? <RefreshCw className="spin" size={15} /> : <Check size={15} />}
                应用 {selectedCount} 个文件
              </button>
            </>
          ) : (
            <button
              className="review-secondary-button"
              type="button"
              disabled={busy || change?.undone}
              onClick={() => void undo()}
            >
              {busy ? <RefreshCw className="spin" size={15} /> : <Undo2 size={15} />}
              {change?.undone ? "已撤销" : "撤销变更"}
            </button>
          )}
        </div>
      </footer>
    </div>
  );
}

function DiffContent({ file, mode }: { file: PatchFilePreview; mode: "unified" | "side_by_side" }) {
  if (mode === "unified") {
    return (
      <pre className="unified-diff">
        {file.unifiedDiff.split("\n").map((line, index) => (
          <span className={`diff-line ${diffLineClass(line)}`} key={`${index}-${line}`}>
            {line || " "}
          </span>
        ))}
      </pre>
    );
  }
  return (
    <div className="side-by-side-diff">
      <div>
        <div className="diff-pane-header">修改前</div>
        <pre>{file.beforeContent ?? ""}</pre>
      </div>
      <div>
        <div className="diff-pane-header">修改后</div>
        <pre>{file.afterContent ?? ""}</pre>
      </div>
    </div>
  );
}

function diffLineClass(line: string) {
  if (line.startsWith("@@")) return "diff-line--hunk";
  if (line.startsWith("+") && !line.startsWith("+++")) return "diff-line--add";
  if (line.startsWith("-") && !line.startsWith("---")) return "diff-line--delete";
  if (line.startsWith("+++") || line.startsWith("---")) return "diff-line--header";
  return "";
}

function operationLabel(file: PatchFilePreview) {
  if (file.operation === "move") return "移动";
  if (file.operation === "add") return "新增";
  if (file.operation === "delete") return "删除";
  return "修改";
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KiB`;
}

function byteLength(content: string | null) {
  return content ? new TextEncoder().encode(content).length : 0;
}
