# k-Coder 项目长期记忆

## 质量门槛与验证

- 质量门槛：`pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test`（见 `AGENTS.md`）。
- **本机 `pnpm` shim 损坏**：直接调用会报 `Cannot find module 'D:\c\Users\...\corepack\dist\pnpm.js'`（路径被拼坏）。改用 `./node_modules/.bin/tsc`、`./node_modules/.bin/vite`、`./node_modules/.bin/playwright`。Playwright 的 `webServer.command` 也走 pnpm，需先手动起 `./node_modules/.bin/vite --host 127.0.0.1 --port 1420`（配置里 `reuseExistingServer: true`）。
- **Rust 侧 mock runtime 测试（Windows）默认不编译**，必须显式开 feature：
  `cargo test --lib --features mock-runtime-tests`。
  根因是 `comctl32.dll!TaskDialogIndirect` 需要 Common Controls v6 清单，完整诊断与修复见 skill `windows-tauri-mock-runtime-tests`。加 `--lib` 是刻意的——清单注入用的 `cargo:rustc-link-arg` 会同时作用于 bin。
- 基线数字：feature 关闭时 `cargo test --lib mobile::` 为 78 通过；开启后同一过滤串会额外带上 `commands::mobile::tests` 的 9 项。2026-09-17 起 `mobile::` 为 97 通过（新增 `mobile::projects::` 8 项 + `view::project_key_is_not_the_workspace_path`）。
- **Rust 构建必须用 `bash scripts/cargo-msvc.sh <cargo 子命令>`**（在仓库根执行）。Git Bash 的 `/usr/bin/link`（GNU coreutils）会遮蔽 MSVC 的 `link.exe`，而 `cmd.exe` 被工具安全策略禁止调用，所以 `vcvars64.bat` 路线走不通；该脚本直接设好 `PATH`/`LIB`/`INCLUDE` 再 `exec cargo`。例：`bash scripts/cargo-msvc.sh check --all-targets`、`bash scripts/cargo-msvc.sh test --lib --features mock-runtime-tests mobile::`。**cargo 会写 `src-tauri/target/`，必须在沙箱外跑**（否则报 `could not write output to ... permission denied`）；耗时较长时用 `run_in_background: true`。
- **全量 `cargo test --lib` 在本机有 16 项固定失败**（2026-09-17 实测 819 通过 / 16 失败），落在 `src-tauri/src/{agent,extensions,execution,patch,tools}`：原因是**本机无符号链接/junction 创建权限**（建链静默失败，测试里的 `unwrap_err()` 实际拿到 `Ok`）与 `rg` 管道环境差异。这些目录相对 `origin/main` 的差异行数为 0，属环境而非代码问题。判断「是不是我改坏了」时先看失败文件归属。

## Git 仓库状态（重要陷阱）

- 当前分支 `codex/robot-workflow-skills` **尚无任何提交**（`git log` 报 "No commits yet"）。`origin/main` 也**不含 `src-tauri/src/mobile/` 模块**——整个手机网关都是未提交工作。
- 因此 **`HEAD` 不可用于基线对比**（`git rev-parse HEAD` 直接失败）。要取证「某个失败是否由我引入」，用这两种方式：

  1. `git diff origin/main --numstat -- <路径>` 看差异行数（0 = 与远端一致）；
  2. **临时移除自己的改动再复跑**，两头对照（最可靠）。
- `git stash push -- <路径>` 在本机曾出现过「无输出但也没暂存」的情况，别只信它的静默成功——`git stash list` 复核一下。
- **工作树被误还原时先找备份分支，不要急着重写代码。** 2026-09-17 工作区被 `git reset` 回 `c818bdc`，改动靠 `backup/pre-restore-20260917` 恢复。恢复流程：
  1. `git diff --name-only <还原点> <备份分支>` 列出全部差异文件；
  2. **逐个 `git diff <还原点> <备份分支> -- <file>` 判断归属**——纯自己的直接 `git checkout <备份分支> -- <files>`；**混合的**（同时含别人改动，本机是 `src/App.tsx` 与 `docs/开发路线图.md`）必须手工只补回自己的 hunk，否则会把别人的改动连它对其他文件的依赖一起拖进来；
  3. 校验 `git diff --numstat <备份分支> -- <恢复的文件>` 应零输出。
- 本机 `src-tauri/target` 长期有**另一条工作流并发跑 cargo**，会出现 `failed to remove file ...rlib: 拒绝访问 (os error 5)` 这类瞬时锁——过一会儿 `rm -f` 就能删掉，不要当成代码问题。

## ⚠️ 本机磁盘容量（曾阻断链接，注意复发）

- 2026-09-17 白天实测：**`C:` 剩余 0 字节（100% 满）**，`D:` 仅剩 4.5G（99%）。`src-tauri/target` 占 61G（`debug/incremental` 26G、`debug/deps` 26G、`target/validation-p10122` 8.2G、`target/release` 3.1G）。
- 症状：`cargo check --all-targets` 能过（不链接），但 `cargo test` 在链接阶段报 `LNK1201: 写入程序数据库 ...pdb 时出错；请检查是否是磁盘空间不足...`。
- **判别**：`cargo check` 干净 + 链接期 `LNK1201`/`拒绝访问` ⇒ 环境（磁盘/并发）问题，不是代码问题。先 `df -k /c /d` 看空间再判断，**不要急着清缓存**。
- 同日晚空间自行恢复到 `D:` 约 27.5G，全部测试随即通过（`mobile::` 97、`persistence::` 15、`mobile_gateway` 20、`mobile_dto_contract` 9），**因此最终没有删除 `target/debug/incremental`**。真要腾空间时，它是最安全的清理目标（纯增量缓存，只损失增量编译速度）。

## 数据与状态归属

- 运行期数据库：`%APPDATA%/com.kcoder.app/runtime-data/k-coder.db`（SQLite，application id `com.kcoder.app`）。诊断时用只读 URI 打开：`sqlite3.connect('file:<path>?mode=ro', uri=True)`。注意该路径**可能不存在**（应用数据被清理或从未启动过），先 `os.path.exists` 再查。
- 关键表：`sessions`（`in_project` / `workspace_path` 是项目归属事实）、`projects`（`id,name,path,trusted,last_opened_at_ms`）、`settings`（含 `active_workspace`）。
- **项目归属事实已于 2026-09-17 收回到服务端**（`P10-188` 治本）。现状：
  - `projects` 表是项目清单的**唯一来源**，经新增的 `project/list` RPC 下发给手机端。
  - 归属键统一用 `workbench::workspace_path_key()`（去 `\\?\` / `\\?\UNC\` 前缀、`\`→`/`、去尾 `/`、Windows 小写折叠），与前端 `src/lib/path.ts` 语义同源。**新增任何比较路径的地方都必须用它**，不要再手写字符串比较。
  - 会话归属由 `mobile::projects::resolve()` 解析后经 `thread/list` 的 `projectKey` 下发；手机端不再自行推断归属。
  - 遗留会话（`in_project = true` 且 `workspace_path = NULL`）兜底到**当前活动工作区**——这是刻意的：桌面端靠 localStorage 的展示映射把它们归到当前工作区，服务端没有那份映射。
  - 登记/移除项目现在也写服务端：`register_project_paths`（只登记，**不切换活动工作区**）、`remove_project_path`（按归一化键删，**不删会话与文件**）。
  - `MobileProject` DTO **刻意不含 `path`**——工作区绝对路径不跨设备边界。DTO 线上形状由 `e2e/fixtures/mobile-dto.json` + `src-tauri/tests/mobile_dto_contract.rs` 双向钉住，**这两个文件必须一起改**。

## 前端 / 样式注意事项

- **改 `mobile.html` 的响应式样式时避免 `border-width` 简写**：简写会把未指定的边也置 0，若元素本来有边框或后续被别的规则覆盖，会多出/少掉 1px 改变高度。`--composer-h` 由 `composer.offsetHeight` **取整**写入、正文底距 `calc(var(--composer-h) + 14px + safe-bottom)` 按它计算，所以 ±1px 会顶破 `e2e/mobile-stream.spec.ts` 里 `|mainPaddingBottom - composerHeight| < 40` 的断言。优先用 `border-bottom-width` / `border-top-width` 这类单边写属性。
- 定位「某个 e2e 失败是不是我的 CSS 造成的」：把新增 CSS 整段临时移除 → 复跑 → 若全绿说明确有影响 → **逐条二分**定位到具体规则（比读 CSS 猜快得多）。
- `e2e/mobile-stream.spec.ts` 与 `mobile-settings.spec.ts` 都直接 `readFileSync` 真实 `mobile.html`（不是快照），所以改动那个文件会立刻影响这些用例。
- 本地跑 e2e：先起 `./node_modules/.bin/vite --host 127.0.0.1 --port 1420`（后台），再 `./node_modules/.bin/playwright test <spec> [--project=desktop]`。Playwright 配置里的 `webServer.command` 走 pnpm，会失败，靠 `reuseExistingServer: true` 复用你手动起的那个。**首次冷启动可能因首次编译竞态而假失败**，复跑或加 `--repeat-each=3` 复核。
- `./node_modules/.bin/vite build` 有时会在清理 `dist/assets` 时报 safe-delete shim 错误——手动 `rm -rf dist/assets` 后重跑即可，不是代码问题。

## 环境注意事项

- 工作区长期有另一条「知识库与记忆扩展」工作流在改写 `src-tauri/src/storage/*`、`src-tauri/src/memory/*`、`src-tauri/src/knowledge*`、`src-tauri/src/commands/mod.rs`、`src-tauri/src/agent/query_rewrite.rs`。它处于中间态时整个 lib 编译不过，`cargo fmt --check` 也会报出它的文件。判断「是不是我改坏了」之前先看报错文件的归属，**不要动它的文件**。
- 主工作区编译不过但需要验证自己的改动时：把工作区整体复制到临时目录（排除 `target`/`node_modules`/`.git`），在**副本里**临时补掉报错，并把 `CARGO_TARGET_DIR` 指向主仓库的 `src-tauri/target` 复用依赖——增量编译约 30 秒，不会干扰主工作区。
