# k-Coder 项目长期记忆

## 构建 / 测试（本机特有，务必按此执行）

- 质量门槛见 `AGENTS.md`；日常 Turn 用模块过滤，不跑全量。
- **本机 `pnpm` shim 损坏**（报 `Cannot find module 'D:\c\Users\...corepack\dist\pnpm.js'`）。改用 `./node_modules/.bin/{tsc,vite,playwright}`。Playwright 配置里的 `webServer.command` 走 pnpm 会失败，需先手动起 `./node_modules/.bin/vite --host 127.0.0.1 --port 1420`（配置 `reuseExistingServer: true` 会复用）。
- **Rust 一律用 `bash scripts/cargo-msvc.sh <cargo 子命令>`**（仓库根执行）。Git Bash 的 `/usr/bin/link`（coreutils）会遮蔽 MSVC `link.exe`，而 `cmd.exe` 被安全策略禁止，所以 `vcvars64.bat` 路线不通；该脚本自己设 `PATH`/`LIB`/`INCLUDE`。cargo 写 `src-tauri/target/`，**必须在沙箱外跑**，耗时长用 `run_in_background`。
- **mock runtime 测试需显式开 feature**：`cargo test --lib --features mock-runtime-tests <过滤>`。加 `--lib` 是刻意的（清单注入用的 `cargo:rustc-link-arg` 会同时作用于 bin）。详见 skill `windows-tauri-mock-runtime-tests`。
- **症状识别**：脚本「几秒跑完、无任何 cargo 输出、退出码 0」= `set -e` 在探测 Windows SDK 时静默退出（SDK 现在 `D:\Windows Kits\10`，脚本逐个候选探测），**不是**测试全过。工具链再挪盘时同步更新候选列表。
- 全量 `cargo test --lib` 有 16 项固定失败，落在 `src-tauri/src/{agent,extensions,execution,patch,tools}`：根因是本机无符号链接/junction 创建权限与 `rg` 管道环境差异，这些目录相对 `origin/main` 差异为 0。判断「是不是我改坏了」先看失败文件归属。
- 磁盘满会在链接期报 `LNK1201`。判别：`cargo check` 干净 + 链接失败 ⇒ 环境（磁盘/并发）问题，先 `df -k /c /d`，别急着清缓存。真要腾空间，`target/debug/incremental` 是最安全的清理目标。
- `src-tauri/target` 常有另一条工作流并发跑 cargo，出现 `failed to remove file ...rlib: 拒绝访问 (os error 5)` 属瞬时锁，稍后重试即可。

## 部署脚本 `k-coder-build-deploy.bat`

- 两份必须同步：仓库 `scripts/k-coder-build-deploy.bat` 与用户桌面副本 `C:\Users\nealk\Desktop\k-coder-build-deploy.bat`（用户实际双击的是桌面那份）。
- 默认 `REPO=D:\code\Nick\k-coder`、`DEST=D:\apps\k-coder`，可用环境变量 `KC_REPO`/`KC_DEST` 覆盖；参数 `--skip-build`/`--no-launch`/`--no-pause`/`--full`。指纹文件 `src-tauri/target/.kc-deploy-stamp` 命中就跳过构建。
- **必须保持纯 ASCII（含注释）**：cmd 以 OEM 代码页 cp936 读 .bat，UTF-8 中文注释会被错误解码并在控制台冒出假的「不是内部或外部命令」报错。
- **运行时资源目录只有 `skills/`、`tools/`、`ocr/`**（由 `tauri.conf.json` 的 `bundle.resources` 映射，`--no-bundle` 也会产出），**没有 `resources/`**；Rust 侧一律 `resource_dir().join("skills"/"tools"/"ocr")`。robocopy 对不存在的源目录返回 **16**，脚本已改为「缺目录只告警」，别再把它加回致命检查。
- robocopy 退出码语义：0–7 = 成功（1 = 有文件被拷贝），≥8 = 失败；`/MIR` 会删掉目标端多余文件。
- **沙箱内跑不了 .bat**：Bash 工具与 PowerShell 工具都拦截 cmd.exe（`dangerouslyDisableSandbox` 也一样）。验证只能用 robocopy 直测 + 纯 Python 静态检查（ASCII/CRLF/标签-goto/括号），真跑需用户手动双击。

## Git 状态

- 分支 `codex/robot-workflow-skills`，HEAD `e2f33eb`（2026-09-21 12:08），工作树干净、无 stash，**已推送**（远端同名分支同 SHA）。`origin/main` = `a6e111f`，落后 HEAD 34 个提交。
- **沙箱内 `.git/refs/remotes/origin/*` 写入不生效**：`git fetch`/`git update-ref` 报成功但 `for-each-ref` 不变，导致 `git status` 误报上游 `[gone]`、`origin/main` 停在旧值 `c79527b`。**判断远端真实状态只用 `git ls-remote origin`**（走网络，权威）。
- `git push` 在本沙箱不可用：报 `could not read Username for 'https://github.com'`（终端提示被禁用、credential helper 取不到凭据）。需要真推送时须由用户在本机终端执行。
- 工作树被误还原时先找备份分支（`backup/pre-restore-20260917`），不要急着重写：`git diff --name-only <还原点> <备份分支>` 列差异，逐个 `git diff` 判断归属；纯自己的直接 checkout，**混合文件（`src/App.tsx`、`docs/开发路线图.md`）必须手工只补回自己的 hunk**。

## 数据与状态归属

- 运行期 DB：`%APPDATA%/com.kcoder.app/runtime-data/k-coder.db`（SQLite，用 `file:<path>?mode=ro` 只读 URI；路径可能不存在，先判存在）。关键表 `sessions`（`in_project`/`workspace_path` 是项目归属事实）、`projects`、`settings`（含 `active_workspace`）。
- 项目归属事实在服务端（`P10-188` 治本）：`projects` 表是项目清单**唯一来源**，经 `project/list` RPC 下发。归属键统一用 `workbench::workspace_path_key()`（与前端 `src/lib/path.ts` 同源），**新增任何比较路径的地方都必须用它**。会话归属由 `mobile::projects::resolve()` 解析后经 `thread/list` 的 `projectKey` 下发，手机端不再自行推断。遗留会话（`in_project=true` 且 `workspace_path=NULL`）兜底到当前活动工作区——刻意为之。
- `MobileProject` DTO **刻意不含 `path`**（绝对路径不跨设备边界）。线上形状由 `e2e/fixtures/mobile-dto.json` + `src-tauri/tests/mobile_dto_contract.rs` 双向钉住，**两个文件必须一起改**。

## 前端 / e2e

- 改 `mobile.html` 响应式样式时**避免 `border-width` 简写**（会把未指定边也置 0）。`--composer-h` 由 `composer.offsetHeight` 取整写入，±1px 会顶破 `e2e/mobile-stream.spec.ts` 里 `|mainPaddingBottom - composerHeight| < 40` 的断言。优先用 `border-bottom-width` 这类单边属性。
- `mobile-stream.spec.ts` 与 `mobile-settings.spec.ts` 直接 `readFileSync` 真实 `mobile.html`（非快照），改动立刻影响用例。
- 定位「e2e 失败是否我的改动」：改动文件 `cp` 到临时目录 → `git checkout -- <files>` → 复跑 → `cp` 回来（比 `git stash` 可靠，本机 stash 曾静默失败）。CSS 问题用整段移除 + 逐条二分定位。
- **safe-delete shim 拦批量删除**（≥50 条/次报 `SAFE_DELETE_BULK_CONFIRM_REQUIRED`）：`vite build` 用 `--outDir .workbuddy-ai/dist-verify --emptyOutDir`；Playwright 加 `--output=.workbuddy-ai/pw-out`。手动清临时目录只能按 <50 个一批（每批约 4 分钟）。
- 全量 e2e = 240 次执行（120 用例 × 2 视口），其中 8 项固定失败（4 个既有用例 × 双视口，多为知识库设置页标题、K 品牌标记、推理强度、恢复项去重、`mobile.html` 输入区高度），会随并行工作流中间态波动；**单项复跑再判定**。

## 环境注意事项

- **本机 WebView2 DevTools 端点不可用**（端口 `LISTENING`、TCP 可连、**零字节响应**，`curl .../json/version` 超时）：所有 `scripts/validate-*-native.cjs` 的「隔离宿主 + `connectOverCDP`」原生验收都跑不动。已排除独立 user-data-folder、`--remote-allow-origins=*`、`Host: localhost`、IPv6、代理、残留进程。**别再花时间排查**——原生路径如实标注「未验收」，用 mock runtime 命令测试 + Playwright e2e 顶上。症状记录见 `scripts/validate-mobile-delete-native.cjs` 头部。
- `tauri dev` 可能被并行工作流的 cargo 挡在 `Blocking waiting for file lock on build directory`（1~2 分钟自行放行）。判断宿主是否真起来要看**新的 `k-coder.exe` PID**，不能只看 DevTools 端口。
- 工作区长期有另一条「知识库与记忆扩展」工作流在改写 `src-tauri/src/storage/*`、`memory/*`、`knowledge*`、`commands/mod.rs`、`agent/query_rewrite.rs`，以及 `src/App.tsx`、`src/App.css`、`src/components/SettingsDialog.tsx`、`index.html`、`src/lib/theme.ts`、`docs/开发路线图.md`。它处于中间态时整个 lib 编译不过、`cargo fmt --check` 也会报它的文件——**先看报错文件归属，不要动它的文件**；上面几个前端文件 Edit 前先读当前内容，别按旧文本匹配。
- 主工作区编译不过但需验证自己的改动：把工作区复制到临时目录（排除 `target`/`node_modules`/`.git`），在副本里临时补掉报错，并把 `CARGO_TARGET_DIR` 指向主仓库的 `src-tauri/target` 复用依赖（增量约 30 秒，不干扰主工作区）。
