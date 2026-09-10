import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 启动时序控制。
 *
 * 主窗口在 `tauri.conf.json` 中配置为初始隐藏（`visible: false`），HTML 里内联的启动屏
 * 负责 WebView 首帧；等 React 挂载完成、界面已经有内容后，再收起启动屏并显示窗口，
 * 从而消除"先弹一个空白窗口、过一会儿才有内容"的过程。
 *
 * 任何一步失败都不能把用户留在隐藏窗口里，因此这里同时提供超时与错误兜底。
 */

const BOOT_FALLBACK_MS = 3_000;
const SPLASH_FADE_MS = 260;

let finished = false;
let fallbackInstalled = false;

function inTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function showMainWindow(): void {
  if (!inTauriRuntime()) return;
  try {
    void getCurrentWindow().show();
  } catch {
    // 浏览器预览或非 Tauri 环境：没有窗口可显示，忽略。
  }
}

function hideSplash(): void {
  const splash = document.getElementById("app-splash");
  if (!splash || splash.classList.contains("is-hiding")) return;
  splash.classList.add("is-hiding");
  window.setTimeout(() => splash.remove(), SPLASH_FADE_MS);
}

/** 收起启动屏并显示主窗口；可重复调用，只有第一次生效。 */
export function finishBoot(): void {
  if (finished) return;
  finished = true;
  hideSplash();
  showMainWindow();
}

/** 兜底：模块加载失败或渲染抛错时，也要把窗口显示出来。 */
export function installBootFallback(): void {
  if (fallbackInstalled || typeof window === "undefined") return;
  fallbackInstalled = true;
  window.setTimeout(finishBoot, BOOT_FALLBACK_MS);
  window.addEventListener("error", finishBoot);
  window.addEventListener("unhandledrejection", finishBoot);
}
