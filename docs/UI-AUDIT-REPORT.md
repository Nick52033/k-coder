# k-Coder v0.6.0 — 综合 UI/UX 审计报告

> **审计方法**: shadcn/ui · ui-ux-pro-max · redesign-existing-projects 三技能联合审计
> **审计日期**: 2026-07-19
> **项目版本**: 0.6.0 (Phase 2)
> **设计 Dial**: Variance 4 · Motion 3 · Density 7（面向开发者工具的稳态风格）

---

## 一、项目概览

| 维度 | 状态 |
|------|------|
| **产品类型** | 桌面编程智能体 (Developer Tool / IDE) |
| **技术栈** | Tauri 2 + React 19 + TypeScript 5.8 + Vite 7 + Zustand 5 |
| **样式方案** | 纯手写 CSS（2001 行 `App.css`） |
| **图标库** | lucide-react v0.468 |
| **皮肤系统** | Paper（绿）/ Midnight（翠绿，纯暗）/ Amethyst（紫，支持亮暗） |
| **核心场景** | 编程对话、工具调用流、Patch 审阅、Git 状态、文件预览 |

---

## 二、综合评分

```
┌──────────────────────────────┬────────┬───────────────────────────────┐
│ 维度                          │ 评分    │ 一句话评价                       │
├──────────────────────────────┼────────┼───────────────────────────────┤
│ 01 排版 (Typography)          │ ★★★☆☆ │ Inter 单字重，无等宽字            │
│ 02 色彩 & 主题 (Color)        │ ★★★★★ │ 三皮肤令牌系统是标杆资产           │
│ 03 布局 & 响应式 (Layout)     │ ★★★★☆ │ Grid 扎实，桌面优先                │
│ 04 交互 & 状态 (Interaction)  │ ★★★☆☆ │ 缺焦点陷阱、缺活跃态指示            │
│ 05 组件模式 (Components)      │ ★★★★☆ │ BEM 统一，原生元素，缺空/错/加载态   │
│ 06 无障碍 (Accessibility)     │ ★★★☆☆ │ 对比度临界，aria 标签齐备            │
│ 07 动画 (Motion)              │ ★☆☆☆☆ │ 仅一个 spin 旋转动画               │
│ 08 图标 (Iconography)         │ ★★★★☆ │ lucide 一致，专业感强               │
│ 09 内容质量 (Content)         │ ★★★★★ │ 中文文案简洁准确，无 AI 套话        │
│ 10 代码质量 (Code Quality)    │ ★★★★☆ │ 515 行单文件需拆分                  │
├──────────────────────────────┼────────┼───────────────────────────────┤
│             综合评分           │ ★★★★☆ │ 4.0 / 5.0 — 强基础，强设计资产       │
└──────────────────────────────┴────────┴───────────────────────────────┘
```

---

## 三、高光亮点（设计资产）

### 3.1 标杆级的三皮肤令牌系统

k-Coder 的 CSS 变量体系是项目最宝贵的 UI 资产：

```css
Paper    (纸墨精工) → #176b4d 主色 / Light #f3f5f3 · Dark #0f1612
Midnight (午夜终端) → #10b981 主色 / 纯 OLED  #09090b（无 Light）
Amethyst (紫晶指令) → #7c3aed 主色 / Light #faf5ff · Dark #0f0a1a
```

**令牌命名层次清晰**：`--color-brand` → `--color-brand-hover` → `--color-brand-light` → `--color-brand-ring`，通过 `data-skin` × `data-theme` 实现笛卡尔积切换。

### 3.2 完善的 Diff/Patch 审阅流

`PatchReviewDialog` 实现了完整的代码审阅体验：统一/并排 Diff、文件选择、编辑 Patch、撤销变更、焦点陷阱 — 在同类产品中属于标杆。

### 3.3 正确的 `100dvh` 而非 `100vh`

`App.css:258`  使用 `height: 100dvh` — 正确处理 iOS Safari 视口 bug。

### 3.4 干净的中文文案

无 "Elevate" / "Seamless" / "Unleash" 等 AI 套话，错误提示直接（"运行时不可用"），操作动词明确（"归档会话" / "撤销变更"）。

### 3.5 BEM 命名 + CSS 变量一致性

`--active` / `--error` / `--disabled` 状态后缀贯穿全局，硬编码颜色仅出现在极少过渡位置（如 `#c42b1c` 关闭按钮危险色）。

---

## 四、按技能框架的详细审计

### 4.1 [shadcn/ui] 框架视角

> **关键前提**: k-Coder **未使用 shadcn/ui**，且不需要。理由：纯 CSS 桌面应用、Tauri 窗口高度受限、已有 2001 行成熟令牌体系，引入 shadcn 替换成本 > 收益。

#### 4.1.1 可采纳的 shadcn 原则（已部分遵守）

| 原则 | 状态 | 备注 |
|------|------|------|
| 语义颜色（`bg-primary` 而非 `bg-blue-500`） | ✅ | 已有完整 `--color-brand` 等令牌 |
| `gap-*` 替代 `space-y-*` | ✅ | 完全使用 flex + gap |
| `size-*` 替代 `w-* h-*` | ⚠️ | `App.css:354` 用 `width: 32px; height: 32px;` 而非 `size-32` |
| `truncate` 统一 | ⚠️ | 手动组合三件套 `overflow-hidden text-overflow-ellipsis whitespace-nowrap` 出现 17+ 次 |
| `cn()` 条件类名 | ❌ | 模板字面量手写（`App.tsx:266`） |
| Dialog/Sheet/Overlay 需 Title | ⚠️ | `PatchReviewDialog` 有，但 `SettingsDialog` 缺 `aria-labelledby` |
| FieldGroup/Field 组合 | ✅ | Settings 已用 `<label>` + 控件 |
| 必填项用 Badge/Alert | ⚠️ | `.settings-error` 自定义 div，可考虑 `Alert` 模式 |

#### 4.1.2 不适用 shadcn 的原因

| 原因 | 说明 |
|------|------|
| 无 Tailwind | shadcn 深度依赖 Tailwind，引入会重塑构建 |
| 桌面窗口约束 | shadcn 组件面向 Web 全屏，Tauri 1280×820 需自定义 |
| 已有令牌体系 | 2001 行 CSS 资产成熟 |
| 性能成本 | 桌面应用追求极简启动，运行时组件库有开销 |

#### 4.1.3 shadcn 视角的最终建议

**不引入 shadcn/ui**。保持纯 CSS 架构，**仅采纳其作为代码规范参考**，将以下原则固化到 `AGENTS.md`：

1. 状态后缀规范（`--active` / `--error` / `--disabled`）— 已是
2. 禁止 `space-x-*` / `space-y-*`，统一 `flex gap-*` — 已是
3. Icon 组件传对象，禁字符串 key
4. Dialog/Overlay 必须有 Title + `aria-labelledby`

---

### 4.2 [ui-ux-pro-max] 优先级 1-10 逐项审计

#### 优先级 1: 无障碍 ⚠️ 临界

| 检查项 | 状态 | 详情 |
|--------|------|------|
| 对比度 4.5:1 (Paper Light) | ⚠️ | `--color-ink-muted: #57615b` 在 `#ffffff` 上 ≈ 4.85:1，临界通过；`--color-ink-subtle: #7a847e` ≈ 3.7:1，**不达 AA** |
| 对比度 4.5:1 (Amethyst Light) | ❌ | `--color-ink: #2e1065` 在 `#faf5ff` 上对比度 ≈ 13:1，OK；但 `--color-ink-subtle: #8b5cf6` 在 `#faf5ff` 上 ≈ 2.6:1，**不达 AA** |
| Focus Ring | ✅ | `:focus-visible { outline: 2px solid var(--color-brand) }` 全局 |
| Aria 标签 | ✅ | 所有按钮均有 `aria-label` / `title` |
| 键盘导航 | ⚠️ | SettingsDialog 缺焦点陷阱（PatchReviewDialog 已实现，可复用） |
| Skip-to-content | ❌ | 无 `Skip to main content` 链接 |

**修复建议**：
- 提高 `--color-ink-subtle` 暗度到 ≥ 5:1 对比度（`Paper Light` 用 `#5a6360`，`Amethyst Light` 用 `#6d28d9`）
- 给 `SettingsDialog` 增加焦点陷阱（复用 `PatchReviewDialog` 的实现）

#### 优先级 2: 触摸与交互 ⚠️ 桌面应用

| 检查项 | 状态 | 详情 |
|--------|------|------|
| 最小尺寸 44×44px | ⚠️ | `icon-button` 是 32×32px，低于 Apple HIG 44pt |
| 8px+ 间距 | ✅ | 边距均 ≥ 6px，按钮内 padding ≥ 8px |
| 加载反馈 | ⚠️ | Composer 发送后无视觉反馈，仅 `disabled` 状态 |
| 状态变化瞬时 | ⚠️ | 全部 `150ms ease-out` 平滑，达标 |

**修复建议**：
- `icon-button` 升级为 36×36px（桌面应用介于 32-44 之间合理）
- 发送后 `Composer` 加微妙视觉提示（淡入或边框脉动 200ms）

#### 优先级 3: 性能 ⚠️ 需关注

| 检查项 | 状态 | 详情 |
|--------|------|------|
| 长列表虚拟化 | ❌ | `thread-list` 渲染所有会话（`App.tsx:264` `.map`） |
| Context 拆分 | ❌ | `useWorkbenchStore` 单一 store，14 个字段一次订阅（`App.tsx:65-93`） |
| React.memo | ❌ | 无任何 `memo` / `useMemo` |
| 滚动性能 | ✅ | 消息区使用 `scrollTop = scrollHeight`（简单）但无虚拟化 |

**修复建议**：
- 高频重渲染字段（`messages` / `loading` / `error`）从主 store 拆出到细粒度 selector
- `thread-list` 超过 100 项时引入 `react-window` 或自实现虚拟滚动

#### 优先级 4: 样式选择 ✅ 与产品契合

**ui-ux-pro-max 推荐**: Developer Tool/IDE 应使用 **Dark Mode (OLED) + Minimalism** + Terminal 仪表盘风格。

**k-coder 现状**:
- ✅ 已有 `Midnight` 暗色皮肤
- ✅ 已有 `Terminal CLI` 元素的纯单色风格
- ✅ Minimalism 风（无装饰性元素）
- ⚠️ 但 `Amethyst` 紫色偏离 "Code dark + run green" 主流审美

**建议**:
- 保留三皮肤（品牌资产）
- 给 Midnight 加 `Cascadia Code` 字体作为 mono 优化（`Amethyst` 已用 `Syne` 做 heading）

#### 优先级 5: 布局与响应式 ✅ 扎实

| 检查项 | 状态 |
|--------|------|
| CSS Grid 优先 | ✅ 全局使用 Grid |
| Mobile-first 断点 | ⚠️ 仅两个断点（920px / 720px），桌面优先 |
| `100dvh` 而非 `100vh` | ✅ |
| 视口 meta | ✅ `index.html` |
| 无水平滚动 | ✅ |
| 容器 max-width | ✅ `message-list { width: min(100%, 820px) }` |

**建议**: 三个断点足够（`1280+ / 920-1280 / 720-920 / <720`），可保留。

#### 优先级 6: 排版与色彩 ⚠️

| 维度 | 现状 | ui-ux-pro-max 推荐 | 差距 |
|------|------|--------------------|------|
| 主字体 | Inter | Inter | ✅ |
| 等宽字体 | Consolas | JetBrains Mono / Cascadia Code | ⚠️ 缺开发者字体 |
| 字重 | 400/600/650 | 300/400/500/600/700 | ⚠️ 缺 500 中间权重 |
| H1 | 14px | 19-22px | ⚠️ 偏小 |
| 行高 | 1.65（消息） | 1.5-1.6 | ✅ |
| 色彩饱和度 | Midnight #10b981 100% | 80% 以下 | ⚠️ 微超 |
| 单色品牌 | ✅ | ✅ | ✅ |

**关键改进建议**:
- 引入 `JetBrains Mono` 或 `Cascadia Code` 作为 mono 字体（开发者工具标配）
- H1 14px → 16-18px 提升层级感
- 字重补 500（Medium）用于次级信息

#### 优先级 7: 动画 ❌ 几乎缺失

| 检查项 | 状态 | 详情 |
|--------|------|------|
| 150-300ms 过渡 | ✅ | 全局 `--transition-fast: 150ms ease-out` |
| Skeleton 加载 | ❌ | 仅有 `empty-thread` 的 `spin` 旋转 |
| 进入动画 | ❌ | 模态直接出现，无 fade-in |
| 状态变化动效 | ❌ | 无 |
| `prefers-reduced-motion` | ✅ | `App.css:1330` 处理 spin 动画 |

**关键改进**:
- 给 `PatchReviewDialog` / `SettingsDialog` 增加 fade-in + scale 0.96 → 1 弹出动画（150ms）
- 给消息列表增加 staggered entry（每条消息 30ms 间隔，淡入 + Y+8 → 0）

#### 优先级 8: 表单与反馈 ⚠️ 基础到位

| 维度 | 状态 |
|------|------|
| 可见标签 | ✅ Settings 用 `<label>` |
| 字段错误 | ✅ `.settings-error` |
| Helper text | ✅ `.provider-form-grid small` |
| 渐进披露 | ⚠️ 模型字段全部展开，未折叠高级设置 |

**建议**: `Provider Settings` 把 `Base URL` / `Max Tokens` 折叠到 "高级" 区域，默认只显示模型 + API Key。

#### 优先级 9: 导航模式 ✅ 良好

| 维度 | 状态 |
|------|------|
| 当前页指示 | ✅ `.thread-item--active` / `.settings-nav-item--active` |
| 快捷键 | ✅ `Ctrl+N` / `Ctrl+,` / `Escape` |
| 深链接 | ❌ 桌面应用无需 |
| 返回导航 | ✅ Modal 默认有 `Escape` 关闭 |

**建议**: 增加 `Ctrl+Shift+P` 命令面板（Command Palette）作为进阶导航。

#### 优先级 10: 图表与数据 — 暂不适用

k-Coder 无独立图表，仅有 `usage-block`（输入/输出/总计），无需审计。

---

### 4.3 [redesign-existing-projects] 系统化诊断

按 redesign 技能的 8 大维度对 k-coder 扫描：

#### 4.3.1 排版 (Typography) — 3 个问题

| # | 问题 | 修复 |
|---|------|------|
| T1 | **浏览器默认 Inter**（`App.css:3`） | 保留 Inter，但补 JetBrains Mono 改善代码区 |
| T2 | **H1 14px 偏小**，标题缺重量 | 14 → 17px，字重 650 → 700 |
| T3 | **缺 500 Medium 字重** | 补 `--font-weight-medium: 500` |

#### 4.3.2 色彩与表面 (Color & Surfaces) — 4 个问题

| # | 问题 | 修复 |
|---|------|------|
| C1 | `--color-ink-subtle` 对比度不达 AA（Paper/Amethyst Light） | 加深到 ≥ 5:1 |
| C2 | **图标 hover 边框硬编码** `#d9dfda`（`App.css:366`） | 改用 `--color-border` |
| C3 | 三皮肤有渐变无纹理，**零纹理** | 给 `Midnight` 底色加极弱噪点（`background-image: url(noise.svg)` 3% opacity） |
| C4 | **`window-control--close` 用硬编码红色** `#c42b1c` | 改用 `--color-error` 派生 |

#### 4.3.3 布局 (Layout) — 3 个问题

| # | 问题 | 修复 |
|---|------|------|
| L1 | **三栏等宽无层次** | Activity 264px → 240px，Sidebar 232px → 220px，让主对话区更主导 |
| L2 | **卡片等高（`.usage-summary-grid`）** flexbox 强制 | 已用 grid auto-rows 自然高度 ✅ |
| L3 | **缺 max-width 容器** `Settings` 已 1120px ✅ | 其他区域 OK |

#### 4.3.4 交互与状态 (Interactivity) — 6 个问题

| # | 问题 | 修复 |
|---|------|------|
| I1 | 按钮 **缺 active 反馈**（`send-button` 仅有 opacity 0.88） | 加 `transform: scale(0.96)` on `:active` |
| I2 | **缺焦点环补强** 仅 2px 描边 | 焦点态加 `background: var(--color-surface-hover)` 双重视觉 |
| I3 | **`thread-actions` 缺键盘可达** 用 `<span role="button">` 而非 `<button>` | 改为真正的 `<button>` 元素 |
| I4 | **`SettingsDialog` 缺焦点陷阱** | 复用 `PatchReviewDialog` 的焦点陷阱 |
| I5 | **滚动无平滑** 无 `scroll-behavior: smooth` | 全局加 `scroll-behavior: smooth` |
| I6 | **`window.prompt` 改名会话**（`App.tsx:274`） | 替换为 inline `<input>` 或自有 `PromptDialog` |

#### 4.3.5 内容 (Content) — 几乎无可挑剔

| # | 问题 | 修复 |
|---|------|------|
| Cn1 | 无 AI 套话 | ✅ |
| Cn2 | 无 "John Doe" 占位 | ✅ |
| Cn3 | **空状态文案略生硬** "开始对话 — 输入消息与 AI 协作" | 改为更情境化："选一个工作区，然后告诉我你想构建什么" |

#### 4.3.6 组件模式 (Components) — 4 个问题

| # | 问题 | 修复 |
|---|------|------|
| P1 | **三等高卡片** (`.usage-summary-grid`) | 改用 grid 4 列或保持 2 列自由高度 |
| P2 | **全用 lucide 图标** （redesign 提醒"默认 AI 选择"） | 当前是合理选择，开发者工具 lucide 是主流 ✅ |
| P3 | **`alert/empty` 状态自写 div** | 提取 `.alert` / `.empty` 共享类，方便复用 |
| P4 | **三个 Modal 各自硬编码样式** | 抽取 `.modal-overlay` / `.modal-dialog` 基础类 |

#### 4.3.7 图标 (Iconography) — 2 个问题

| # | 问题 | 修复 |
|---|------|------|
| Ic1 | **缺 favicon** 检查 `public/tauri.svg` | 需设计专门的品牌 favicon |
| Ic2 | **图标尺寸类 `size-17`**（`App.tsx:213/222/300`） | 不规范的 size 值，改为 `16` 或 `18` |

#### 4.3.8 代码质量 (Code Quality) — 4 个问题

| # | 问题 | 修复 |
|---|------|------|
| Q1 | **App.tsx 515 行**单文件包含所有 UI | 拆分到 `src/components/` |
| Q2 | **App.css 2001 行** 单文件 | 拆为 `tokens.css` / `layout.css` / `components.css` |
| Q3 | **`App.css:393/401` 一行超长** 单行类 | 拆为多行，提高可读性 |
| Q4 | **缺 skip-to-content 链接** | 添加 `<a class="skip-link" href="#root">跳到主内容</a>` |

---

## 五、UI/UX 设计系统推荐（k-coder 落地建议）

基于 ui-ux-pro-max 的设计令牌输出：

### 5.1 字体升级方案

```css
/* 现有 */
--font-family: Inter, "Segoe UI", Arial, sans-serif;
--font-family-heading: var(--font-family); /* Amethyst 例外：Syne */

/* 建议新增 */
--font-family-mono: "JetBrains Mono", "Cascadia Code", "SFMono-Regular", Consolas, monospace;
--font-weight-medium: 500;  /* 新增 500 权重 */
```

### 5.2 排版尺度调整

| Token | 当前 | 建议 |
|-------|------|------|
| `--font-size-base` | 12px (0.75rem) | 保持 12px（开发者工具密度） |
| `--font-size-md` | 13px (0.8125rem) | 保持 |
| `--font-size-lg` | 14px (0.875rem) | 14 → 15px（消息区） |
| 消息 H1 | 14px | 16px |
| 标题字重 | 650 | 700 |

### 5.3 动画系统新增

```css
:root {
  --transition-fast: 150ms ease-out;
  --transition-base: 200ms cubic-bezier(0.16, 1, 0.3, 1);  /* 新增 */
  --transition-modal: 240ms cubic-bezier(0.16, 1, 0.3, 1); /* 新增 */
}

@keyframes modal-enter {
  from { opacity: 0; transform: scale(0.96) translateY(4px); }
  to   { opacity: 1; transform: scale(1) translateY(0); }
}

@keyframes message-enter {
  from { opacity: 0; transform: translateY(8px); }
  to   { opacity: 1; transform: translateY(0); }
}

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    transition-duration: 0.01ms !important;
  }
}
```

### 5.4 焦点环强化

```css
/* 现有 */
:focus-visible {
  outline: 2px solid var(--color-brand);
  outline-offset: 2px;
  border-radius: 3px;
}

/* 建议升级 */
:focus-visible {
  outline: 2px solid var(--color-brand);
  outline-offset: 1px;
  box-shadow: 0 0 0 4px var(--color-brand-ring);
  border-radius: var(--radius-sm);
}
```

---

## 六、优先级修复路线图

### 🟥 P0 - 立即修复（影响可访问性）

| # | 任务 | 影响文件 | 工作量 |
|---|------|----------|--------|
| 1 | 提高 `--color-ink-subtle` 对比度至 AA | `App.css:39/155` | 5 min |
| 2 | 引入 `JetBrains Mono` 等宽字体 | `App.css:3` | 15 min |
| 3 | `SettingsDialog` 增加焦点陷阱 | `SettingsDialog.tsx` | 30 min |
| 4 | 替换 `window.prompt` / `window.confirm` | `App.tsx:274-275` | 1h |

### 🟧 P1 - 短期（1-2 周内）

| # | 任务 | 工作量 |
|---|------|--------|
| 5 | 拆分 `App.tsx` 515 行 → 多个组件 | 4h |
| 6 | 拆分 `App.css` 2001 行 → 3 个 CSS 文件 | 2h |
| 7 | 增加 Modal 进入动画（fade + scale） | 1h |
| 8 | 改 `<span role="button">` → 真实 `<button>` | 30 min |
| 9 | `icon-button` 32px → 36px | 15 min |
| 10 | 主题切换增加系统跟随 `prefers-color-scheme` | 1h |

### 🟨 P2 - 中期（Phase 3 之前）

| # | 任务 | 工作量 |
|---|------|--------|
| 11 | 增加 `Ctrl+Shift+P` Command Palette | 1d |
| 12 | 消息列表 staggered entry 动画 | 2h |
| 13 | 给 `Midnight` 皮肤加极弱噪点纹理 | 1h |
| 14 | 空状态文案情境化 | 30 min |
| 15 | 提取 `.alert` / `.empty` / `.modal-overlay` 共享类 | 2h |
| 16 | Provider Settings 渐进披露（高级字段折叠） | 2h |

### 🟦 P3 - 长期（待评估）

| # | 任务 | 工作量 |
|---|------|--------|
| 17 | `thread-list` 虚拟滚动（> 100 项时） | 1d |
| 18 | 拆分 Zustand store 细粒度 selector | 1d |
| 19 | 设计专属品牌 favicon | 4h |
| 20 | 增加 `Skip to main content` 链接 | 30 min |

---

## 七、与同类产品对比定位

| 维度 | k-Coder | Cursor | GitHub Copilot Chat | Continue.dev |
|------|---------|--------|--------------------|--------------|
| 主题切换 | 3 皮肤 × 2 主题 | 2 | 1 | 2 |
| Diff 审阅 | ✅ 统一/并排 | ✅ | ⚠️ 基础 | ⚠️ 基础 |
| 焦点陷阱 | ✅（部分） | ✅ | ✅ | ✅ |
| 等宽字体优化 | ❌ | ✅ | ✅ | ✅ |
| 命令面板 | ❌ | ✅ | ✅ | ⚠️ |
| 流式渲染 | ✅ | ✅ | ✅ | ✅ |
| 自定义窗口装饰 | ✅ | N/A | N/A | N/A |

**差异化优势**:
- 唯一支持 **三皮肤 × 主题矩阵** 的本地工具
- 自定义窗口装饰（`window-control`）原生体验
- 完整的 Diff/Patch 审阅闭环

**追赶项**:
- 缺少 Command Palette（开发者高频入口）
- 等宽字体在 Patch Editor / 工具调用结果区视觉重量不足

---

## 八、最终建议：方向与禁忌

### ✅ 保持优势

- **三皮肤令牌系统** — 品牌资产
- **CSS Grid 三栏布局** — 经典高效
- **中文文案** — 简洁准确
- **Patch 审阅流** — 标杆实现
- **自绘窗口控制** — 原生感

### ⚠️ 谨慎调整

- **不引入 shadcn/ui** — 替换成本不划算
- **不破坏 BEM 命名** — 团队已习惯
- **不增加新依赖** — 当前 7 个包已是极致精简
- **不重写令牌系统** — 仅补强

### ❌ 避免

- ❌ 引入紫色 AI 渐变（与已有 `Amethyst` 冲突）
- ❌ 用 emoji 替代 lucide 图标
- ❌ 加 Tailwind（会大幅膨胀构建产物）
- ❌ 用 `window.alert` 反馈
- ❌ 动画 > 300ms（桌面应用要快）

---

## 九、结语

**k-Coder v0.6.0 的 UI 是一个「设计资产强、执行细节弱」** 的项目。

- **强**: 三皮肤令牌系统是同类项目中少有的设计深度，可作为品牌长期资产
- **弱**: 动画系统、焦点管理、文件拆分等"开发者自研项目"的常见短板

**修复路线图的总投入**: 约 4-5 人日即可将综合评分从 4.0 提升到 4.5+。

**建议策略**: 优先做 P0/P1 的 10 项（约 2 人日），即可在保留设计资产的同时显著提升专业感；P2/P3 留给后续 Phase。

---

> **附录**: 本报告基于 `App.css` (2001 行)、`App.tsx` (515 行)、`SettingsDialog.tsx`、`WorkbenchPanel.tsx`、`PatchReviewDialog.tsx` 完整审计生成。
