import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],
  resolve: {
    alias: {
      // monaco-editor 的 package exports 会把 CSS 等子路径错误拼接 `.js`，
      // 深路径别名绕过 exports，直接映射到 ESM 文件。
      "monaco-editor/esm/vs": fileURLToPath(
        new URL("./node_modules/monaco-editor/esm/vs", import.meta.url),
      ),
    },
  },
  optimizeDeps: {
    // monaco-editor 以纯 ESM 交付；Vite 7 依赖优化处理其 `?worker` 导入会抛出
    // "optimized info should be defined" 并让 worker 请求 504，排除后走原生 ESM 与 worker 管线。
    exclude: ["monaco-editor"],
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          markdown: ["react-markdown", "remark-gfm"],
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
