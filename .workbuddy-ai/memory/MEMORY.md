# k-Coder 项目长期记忆

## 质量门槛与验证

- 质量门槛：`pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test`（见 `AGENTS.md`）。
- **本机 `pnpm` shim 损坏**：直接调用会报 `Cannot find module 'D:\c\Users\...\corepack\dist\pnpm.js'`（路径被拼坏）。改用 `./node_modules/.bin/tsc`、`./node_modules/.bin/vite`、`./node_modules/.bin/playwright`。Playwright 的 `webServer.command` 也走 pnpm，需先手动起 `./node_modules/.bin/vite --host 127.0.0.1 --port 1420`（配置里 `reuseExistingServer: true`）。
- **Rust 侧 mock runtime 测试（Windows）默认不编译**，必须显式开 feature：
  `cargo test --lib --features mock-runtime-tests`。
  根因是 `comctl32.dll!TaskDialogIndirect` 需要 Common Controls v6 清单，完整诊断与修复见 skill `windows-tauri-mock-runtime-tests`。加 `--lib` 是刻意的——清单注入用的 `cargo:rustc-link-arg` 会同时作用于 bin。
- 基线数字：feature 关闭时 `cargo test --lib mobile::` 为 78 通过；开启后同一过滤串会额外带上 `commands::mobile::tests` 的 9 项。

## 环境注意事项

- 工作区长期有另一条「知识库与记忆扩展」工作流在改写 `src-tauri/src/storage/*`、`src-tauri/src/memory/*`、`src-tauri/src/knowledge*`、`src-tauri/src/commands/mod.rs`、`src-tauri/src/agent/query_rewrite.rs`。它处于中间态时整个 lib 编译不过，`cargo fmt --check` 也会报出它的文件。判断「是不是我改坏了」之前先看报错文件的归属，**不要动它的文件**。
- 主工作区编译不过但需要验证自己的改动时：把工作区整体复制到临时目录（排除 `target`/`node_modules`/`.git`），在**副本里**临时补掉报错，并把 `CARGO_TARGET_DIR` 指向主仓库的 `src-tauri/target` 复用依赖——增量编译约 30 秒，不会干扰主工作区。
