import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ToastProvider } from "./components/Toast";
import { finishBoot, installBootFallback } from "./lib/boot";

installBootFallback();

const container = document.getElementById("root") as HTMLElement;

ReactDOM.createRoot(container).render(
  <React.StrictMode>
    <ToastProvider>
      <App />
    </ToastProvider>
  </React.StrictMode>,
);

// 等首帧真正绘制出来再收起启动屏、显示主窗口（双 rAF：第一次提交后，第二次确认已上屏）。
requestAnimationFrame(() => requestAnimationFrame(finishBoot));
