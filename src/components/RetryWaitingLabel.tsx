import { useEffect, useState } from "react";

export function RetryWaitingLabel({ retryAtMs }: { retryAtMs?: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [retryAtMs]);
  const seconds = retryAtMs ? Math.max(0, Math.ceil((retryAtMs - now) / 1000)) : 0;
  return <span>{seconds ? `上游返回 429 · 预计 ${seconds} 秒后重试` : "上游返回 429 · 等待上游恢复"}</span>;
}
