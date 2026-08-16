# k-Coder K 品牌标记与蓝色应用图标设计

## 背景

标题栏和新会话欢迎区曾把字母 `K` 替换为“指令脉冲”几何标记，但 Tauri 原生图标仍保留绿色底白色 `K`，造成同一应用同时出现两套品牌身份。用户明确要求品牌标记恢复为 `K`、任务栏图标改为蓝色 `K`，并补充新会话中央可以使用另一枚图形。最终方案因此区分稳定的产品品牌与场景化欢迎插图。

## 目标

- 标题栏使用主题感知的 `K` 品牌标记。
- 新会话中央使用独立的“指令脉冲”欢迎图形，不把它声明为品牌 Logo。
- 原生应用图标使用固定的 CodeBuddy 品牌蓝 `#2F6FE4` 和白色 `K`。
- SVG 母版、Windows ICO、macOS ICNS、通用 PNG、Appx、iOS 和 Android 图标保持同源。
- 保留现有欢迎区文案、布局、响应式边界、加载态和会话状态行为。

## 设计

### 界面角色

`BrandGlyph` 继续是 `src/App.tsx` 内的纯展示 SVG。它使用与 `assets/app-icon.svg` 相同的 `512 x 512` 圆角底板和 `K` 路径；界面底板继承当前主题的 `--color-brand`，字形继承 `--color-surface`，因此继续适配深浅主题。它只用于标题栏等产品身份位置。

`WelcomeGlyph` 是同文件内的独立纯展示 SVG，使用“指令脉冲”几何图形表达请求进入执行流。它只用于新会话中央的 `.empty-thread--welcome`，不复用品牌选择器，也不替代标题栏或系统图标的 `K`。

### 原生图标

原生图标不能随主题切换，固定使用 CodeBuddy 浅色主题的主品牌蓝 `#2F6FE4` 作为底色，字形保持白色。`assets/app-icon.svg` 是唯一手工维护的原生图标母版，`src-tauri/icons/` 由 `pnpm tauri icon assets/app-icon.svg` 机械生成，不单独手改二进制资源。

Windows 调试可执行文件通过 Cargo 构建脚本把 `src-tauri/icons/icon.ico` 编入资源。`src-tauri/build.rs` 必须显式输出 `cargo:rerun-if-changed=icons/icon.ico`；否则只更新 ICO 时，Cargo 可能复用旧的 `resource.lib`，导致源码和平台图标已经变蓝但任务栏仍读取绿色的旧嵌入图标。

## 状态和边界

- `BrandGlyph` 使用稳定选择器 `data-brand-mark="k-letter"`。
- `WelcomeGlyph` 使用独立选择器 `data-welcome-mark="command-pulse"`，不得使用 `data-brand-mark`。
- 标题栏与原生应用图标保持 `K` 身份；“指令脉冲”只是一枚欢迎区插图。
- Windows ICO 变化必须触发 Tauri/Cargo 资源重建，不能依赖手动清理 `target/`。
- 新会话欢迎区仍只在非读取且没有对话内容时出现。
- 不改动 Provider、Tauri 命令、AgentRuntime、持久化、安全策略或输入工作流。
- 本次不重做 `K` 轮廓、不增加品牌动画，也不为欢迎插图建立原生图标母版。

## 验收

1. desktop 和 narrow 新会话回归都能在标题栏找到一个 `k-letter` 品牌标记。
2. 欢迎区只包含一个 `data-welcome-mark="command-pulse"`，不包含 `k-letter`，并且页面不存在把 `command-pulse` 声明为 `data-brand-mark` 的节点。
3. `assets/app-icon.svg` 明确包含 `#2F6FE4` 底色和白色 `K` 路径。
4. Tauri 图标生成命令成功，生成的 512 px 与 32 px PNG 均显示蓝底白色 `K`。
5. `src-tauri/build.rs` 监听 `icons/icon.ico`，从全新路径提取重建后 exe 的嵌入图标仍为蓝底白色 `K`。
6. `pnpm build`、Rust 格式检查、`cargo check`、`cargo test` 和定向 Playwright 回归完成；任何与本改动无关的共享工作区失败必须如实记录。
7. `pnpm tauri dev` 实际启动，原生窗口显示标题栏 `K`、中央欢迎图形，并加载新生成的 Windows 图标。
