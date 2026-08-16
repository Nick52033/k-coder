# k-Coder 指令脉冲空状态设计

> 状态：品牌身份部分已被 `2026-08-17-k-brand-blue-app-icon-design.md` 取代。用户后续明确要求标题栏品牌与原生应用图标使用 `K`；本文中的“指令脉冲”仅保留为新会话欢迎插图，不再作为产品 Logo。

## 背景

用户指出新会话默认区域过于简单，顶部的 `k-Coder` 标记也缺少辨识度。用户在三个视觉方向中选择并确认了 A「指令脉冲」。

## 目标

把“新会话且尚无内容”的区域变成一个安静、明确的起点，同时让标题栏和空状态使用同一套品牌图形语言。界面仍然是高密度、工作导向的桌面开发工具，不变成营销页或新手教程页。

## 范围

- 替换 `BrandGlyph`：使用基于“请求进入执行流”的几何 SVG 标记，而不是方块中的字母 K。
- 只改造 `!loading && !hasConversationContent` 的新会话空状态。
- 空状态展示放大的同源标记、标题“从一个明确的任务开始”、一句工作导向的说明，以及低干扰状态提示。
- 提供专用 CSS，覆盖常规、窄屏、深色皮肤和 `prefers-reduced-motion`。
- 为品牌标记和默认空状态增加 desktop/narrow Playwright 回归。
- 同步路线图中的用户优先插入任务、当前位置和变更记录。

## 非范围

- 不改动 Provider、Tauri 命令、AgentRuntime、会话持久化或安全策略。
- 不加入示例任务卡片、额外按钮、快捷键教学、图片资产或网络请求。
- 不改变加载态“正在读取会话”、已有消息列表、输入区、侧栏和工作台面板的行为。
- 不创建新的品牌图片文件；图形保持为可继承主题颜色的内联 SVG。

## 视觉系统

受 `frontend-design` UI 设计指导影响，本次把大胆性集中在一个能表达产品本质的图形上：深色品牌底上的细脉冲线与垂直指令杆。它代表用户请求进入本地执行流，而不是泛化的 `</>` 图标。

| 层级 | 设计 |
| --- | --- |
| 标题栏 | 24 px 容器中的紧凑品牌标记，与既有 `--color-brand`、`--color-surface` 变量协调。 |
| 空状态锚点 | 62-68 px 的同源标记，外围只有细信号轨迹，不使用卡片或装饰性大阴影。 |
| 标题 | “从一个明确的任务开始”，为新用户提供方向，但不替代输入区。 |
| 说明 | “描述你想完成的工作，k-Coder 会在当前项目中协作推进。” 保持一句、可读且不夸张。 |
| 状态提示 | 低对比的“等待你的第一条请求”，用于稳住留白，不提供假功能。 |

## 结构和状态

`App.tsx` 继续以 `hasConversationContent` 作为唯一的内容分支事实：

- `loading && !hasConversationContent`：保留既有读取状态。
- `hasConversationContent`：保留既有消息时间线。
- 其他空状态：渲染新的 `.empty-thread--welcome` 语义结构。

`BrandGlyph` 保持纯展示组件，不新增状态、事件、外部依赖或跨边界载荷。所有颜色使用现有 CSS 变量，避免让某个皮肤固定为蓝色或白色。

## 可访问性和响应式

- SVG 继续是 `aria-hidden` 的装饰，正文使用真实标题和段落。
- 桌面与窄屏都在对话列中居中，宽度由 `min()` / `max-width` 约束，不能撑开工作台列。
- 入场过渡只增强默认可见内容；`prefers-reduced-motion` 下去除过渡。
- 不新增可点击的空状态控件，因此没有额外焦点陷阱或键盘路径。

## 验收

1. 新会话为空且非读取时，可见 `.empty-thread--welcome`、新的标题和说明。
2. 顶部品牌标使用新的 SVG，并保留对主题颜色变量的继承。
3. 读取态和已有会话不出现欢迎空状态。
4. desktop 与 narrow Playwright 均验证布局没有横向溢出，且拍摄回归截图。
5. `pnpm build`、`cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`、`cargo check --manifest-path src-tauri/Cargo.toml` 和 `cargo test --manifest-path src-tauri/Cargo.toml` 通过；在 `pnpm tauri dev` 原生窗口实际查看新会话状态。
