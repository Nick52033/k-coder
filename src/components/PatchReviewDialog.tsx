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

  if (request && !preview) {
    return (
      <GenericToolApproval
        request={request}
        error={error}
        onResolve={onResolve}
      />
    );
  }
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

type GenericApprovalChoice = "once" | "session" | "reject";

const GENERIC_CHOICES: { value: GenericApprovalChoice; label: string; description?: string }[] = [
  { value: "once", label: "允许一次", description: "仅本次调用放行" },
  { value: "session", label: "本会话都允许", description: "本会话内同类操作不再询问" },
  { value: "reject", label: "拒绝" },
];

function GenericToolApproval({
  request,
  error,
  onResolve,
}: {
  request: ApprovalRequest;
  error: string;
  onResolve?: (resolution: ApprovalResolution) => Promise<boolean>;
}) {
  const [busy, setBusy] = useState(false);
  const [localError, setLocalError] = useState("");
  const [highlight, setHighlight] = useState<number>(0);
  const [feedback, setFeedback] = useState("");
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    (dialog.querySelector<HTMLElement>("[data-choice]") ?? dialog).focus();
  }, [request.id]);

  function pick(choice: GenericApprovalChoice) {
    if (busy) return;
    if (choice === "reject" && feedback.trim().length > 0) {
      // 暂存反馈，由调用方决定是否回传给模型
    }
    void resolve(choice);
  }

  async function resolve(choice: GenericApprovalChoice) {
    if (!onResolve) return;
    setBusy(true);
    setLocalError("");
    const action: ApprovalResolution["action"] = choice === "reject" ? "rejected" : "approved";
    const success = await onResolve({
      action,
      patch: null,
      selectedPaths: [],
      expectedHashes: [],
      scope: choice === "session" ? "session" : "once",
      feedback: choice === "reject" ? feedback.trim() : "",
    });
    if (!success) {
      setLocalError("审批未能提交，请重试");
      setBusy(false);
    }
  }

  function handleKey(event: React.KeyboardEvent<HTMLDivElement>) {
    if (busy) return;
    if (event.key === "Escape") {
      event.preventDefault();
      void resolve("reject");
      return;
    }
    if (event.key === "1") { event.preventDefault(); pick("once"); return; }
    if (event.key === "2") { event.preventDefault(); pick("session"); return; }
    if (event.key === "3") { event.preventDefault(); pick("reject"); return; }
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      setHighlight((value) => (value + 1) % GENERIC_CHOICES.length);
      return;
    }
    if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      setHighlight((value) => (value - 1 + GENERIC_CHOICES.length) % GENERIC_CHOICES.length);
      return;
    }
    if (event.key === "Enter") {
      const target = event.target as HTMLElement | null;
      if (target && target.tagName === "TEXTAREA") return;
      event.preventDefault();
      pick(GENERIC_CHOICES[highlight]?.value ?? "once");
    }
  }

  const targetLine = summarizeTarget(request);

  return (
    <div
      className="claude-approval"
      role="dialog"
      aria-modal="false"
      aria-label="外部工具审批"
      aria-busy={busy}
      tabIndex={-1}
      ref={dialogRef}
      onKeyDown={handleKey}
    >
      <section className="claude-approval-panel" aria-busy={busy}>
        <h2 className="claude-approval-title">
          确认执行 {request.toolName}{targetLine ? `: ${targetLine}` : ""}?
        </h2>
        <p className="claude-approval-reason">{request.reason || riskLabel(request.risk)}</p>
        <details className="claude-approval-details">
          <summary>查看参数</summary>
          <pre><code>{JSON.stringify(request.arguments, null, 2)}</code></pre>
        </details>
        <div className="claude-approval-actions" role="group" aria-label="审批选项">
          {GENERIC_CHOICES.map((choice, index) => (
            <button
              key={choice.value}
              type="button"
              data-choice={choice.value}
              className={`claude-approval-action ${choice.value === "once" ? "claude-approval-action--primary" : ""} ${
                choice.value === "reject" ? "claude-approval-action--danger" : ""
              } ${highlight === index ? "is-highlight" : ""}`}
              onMouseEnter={() => setHighlight(index)}
              onClick={() => pick(choice.value)}
              disabled={busy}
            >
              {choice.label}
            </button>
          ))}
        </div>
        <input
          type="text"
          className="claude-approval-input"
          placeholder="告诉 k-Coder 应该怎么做（可选）"
          value={feedback}
          onChange={(event) => setFeedback(event.target.value)}
          disabled={busy}
        />
        {(error || localError) && (
          <div className="claude-approval-error" role="alert">{localError || error}</div>
        )}
        <div className="claude-approval-hint">按 Esc 取消 · 按数字键快速选择</div>
      </section>
    </div>
  );
}

function summarizeTarget(request: ApprovalRequest): string {
  const args = request.arguments ?? {};
  if (typeof args.path === "string") return args.path;
  if (typeof args.file_path === "string") return args.file_path;
  if (typeof args.filePath === "string") return args.filePath;
  if (typeof args.destination === "string") return args.destination;
  if (typeof args.command === "string") return truncate(args.command, 80);
  if (typeof args.cmd === "string") return truncate(args.cmd, 80);
  if (typeof args.url === "string") return args.url;
  return "";
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function riskLabel(risk: ApprovalRequest["risk"]) {
  switch (risk) {
    case "read": return "只读";
    case "write": return "写入";
    case "delete": return "破坏性操作";
    case "external": return "外部访问";
  }
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
