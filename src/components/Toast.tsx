import { createContext, useCallback, useContext, useState, type ReactNode } from "react";
import { Check, CircleAlert, X } from "lucide-react";

interface ToastItem {
  id: number;
  message: string;
  type: "success" | "error" | "info";
}

interface ToastCtx {
  success: (message: string) => void;
  error: (message: string) => void;
  info: (message: string) => void;
}

const ToastContext = createContext<ToastCtx | null>(null);
let nextId = 0;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const add = useCallback(
    (message: string, type: ToastItem["type"]) => {
      const id = nextId++;
      setToasts((prev) => [...prev, { id, message, type }]);
      setTimeout(() => dismiss(id), 3200);
    },
    [dismiss],
  );

  const ctx: ToastCtx = {
    success: (m) => add(m, "success"),
    error: (m) => add(m, "error"),
    info: (m) => add(m, "info"),
  };

  return (
    <ToastContext.Provider value={ctx}>
      {children}
      <div className="toast-container" aria-live="polite">
        {toasts.map((t) => (
          <div className={`toast toast--${t.type}`} key={t.id} role="status">
            <span className="toast-icon">
              {t.type === "success" ? (
                <Check size={15} />
              ) : t.type === "error" ? (
                <CircleAlert size={15} />
              ) : (
                <CircleAlert size={15} />
              )}
            </span>
            <span className="toast-msg">{t.message}</span>
            <button className="toast-close" type="button" aria-label="关闭通知" onClick={() => dismiss(t.id)}>
              <X size={13} />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast() {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within ToastProvider");
  return ctx;
}
