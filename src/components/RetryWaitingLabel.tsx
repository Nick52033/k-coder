import { useEffect, useState } from "react";

export function RetryWaitingLabel({ retryAtMs }: { retryAtMs?: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [retryAtMs]);
  const seconds = retryAtMs ? Math.max(0, Math.ceil((retryAtMs - now) / 1000)) : 0;
  return <span>{seconds ? `限流等待 · 预计 ${seconds} 秒后重试` : "限流等待 · 等待请求调度"}</span>;
}
