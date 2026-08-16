# Command Pulse Empty State Implementation Plan

> Superseded as an implementation contract by `2026-08-17-k-brand-blue-app-icon.md`. The command-pulse geometry remains only as the new-conversation welcome illustration; the titlebar and native app identity use `K`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace k-Coder's plain new-conversation placeholder and letter-box logo with the approved command-pulse identity while preserving all existing conversation states.

**Architecture:** Keep this entirely in the React presentation layer. `BrandGlyph` remains a pure inline SVG in `src/App.tsx`; the empty branch adds semantic markup only when no content exists and the CSS file owns visual, responsive, skin, and motion styling. The e2e suite validates the DOM state and both supported viewport widths through the existing mocked Tauri runtime.

**Tech Stack:** React 19, TypeScript, CSS custom properties, Lucide React, Playwright, Vite, Tauri 2.

---

## File Map

- `src/App.tsx`: inline brand SVG and the no-content/non-loading welcome markup.
- `src/App.css`: command-pulse geometry, empty-state hierarchy, responsive sizing, reduced-motion behavior.
- `e2e/workbench.spec.ts`: frontend acceptance regression for desktop and narrow layouts.
- `docs/开发路线图.md`: user-priority task, current status and audit trail.
- `docs/superpowers/specs/2026-08-16-command-pulse-empty-state-design.md`: approved design record.

### Task 1: Establish the E2E Contract

**Files:**
- Modify: `e2e/workbench.spec.ts`

- [ ] **Step 1: Write the failing test**

```ts
test("renders the command-pulse welcome state in a new empty conversation", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("kcoder_e2e_thread_detail", JSON.stringify({
      schemaVersion: 1,
      summary: { schemaVersion: 1, id: "thread-1", title: "新会话", createdAtMs: 1, updatedAtMs: 1, archived: false },
      messages: [], messageTurnIds: {}, turnUserMessageIds: {}, lastTurn: null,
      toolActivities: [], turnTimeline: [], approvals: [], userInputs: [], changes: [], todos: [],
    }));
  });
  await page.goto("/");
  const welcome = page.locator(".empty-thread--welcome");
  await expect(welcome).toBeVisible();
  await expect(welcome.getByRole("heading", { name: "从一个明确的任务开始" })).toBeVisible();
  await expect(page.locator(".brand-mark svg[data-brand-mark='command-pulse']")).toHaveCount(1);
  await expect(page.locator(".message-list")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("command-pulse-empty-state.png"), fullPage: true });
});
```

- [ ] **Step 2: Run the targeted test and verify the expected failure**

Run: `pnpm exec playwright test e2e/workbench.spec.ts --grep "command-pulse welcome" --project=desktop --project=narrow`

Expected: FAIL because `.empty-thread--welcome` and the `data-brand-mark` SVG do not exist.

### Task 2: Implement the Minimum Presentation Change

**Files:**
- Modify: `src/App.tsx:71-81`
- Modify: `src/App.tsx:1870-1873`
- Modify: `src/App.css:3381-3389`

- [ ] **Step 1: Replace the letter mark with the command-pulse SVG**

```tsx
<svg data-brand-mark="command-pulse" viewBox="0 0 32 32" width={size} height={size} fill="none" aria-hidden="true">
  <rect width="32" height="32" rx="8" fill="currentColor" />
  <path d="M9.5 12.25h9.75M9.5 19.75h9.75" stroke="var(--color-brand-light)" strokeWidth="1.8" strokeLinecap="round" />
  <path d="M13 9.5v13" stroke="var(--color-surface)" strokeWidth="2.75" strokeLinecap="round" />
  <path d="m18.25 10 4.25 6-4.25 6" stroke="var(--color-surface)" strokeWidth="2.75" strokeLinecap="round" strokeLinejoin="round" />
</svg>
```

- [ ] **Step 2: Replace only the empty welcome branch**

```tsx
<div className="empty-thread empty-thread--welcome">
  <div className="empty-thread__signal" aria-hidden="true"><BrandGlyph size={48} /></div>
  <h2>从一个明确的任务开始</h2>
  <p>描述你想完成的工作，k-Coder 会在当前项目中协作推进。</p>
  <span className="empty-thread__status">等待你的第一条请求</span>
</div>
```

- [ ] **Step 3: Add scoped styling based on existing semantic tokens**

```css
.empty-thread--welcome { display: flex; min-height: 100%; padding: 48px 24px; align-items: center; justify-content: center; flex-direction: column; }
.empty-thread__signal { position: relative; display: grid; width: 72px; height: 72px; margin-bottom: 14px; place-items: center; }
.empty-thread__signal::before { position: absolute; right: -28px; left: -28px; height: 1px; background: color-mix(in srgb, var(--color-brand) 32%, transparent); content: ""; }
.empty-thread--welcome h2 { margin: 0; color: var(--color-ink); font-size: clamp(1.05rem, 1.8vw, 1.35rem); }
.empty-thread--welcome p { max-width: 26rem; margin: 8px 0 0; line-height: 1.6; }
.empty-thread__status { margin-top: 14px; color: var(--color-ink-faint); font-size: var(--font-size-sm); }
```

- [ ] **Step 4: Re-run the targeted test**

Run: `pnpm exec playwright test e2e/workbench.spec.ts --grep "command-pulse welcome" --project=desktop --project=narrow`

Expected: PASS for two projects, with no horizontal overflow.

### Task 3: Synchronize Project Documentation and Validate

**Files:**
- Modify: `docs/开发路线图.md`

- [ ] **Step 1: Record the priority item before final validation**

```markdown
- [~] `P10-118` 用户明确要求：把新会话默认空状态与标题栏品牌标识升级为“指令脉冲”视觉系统；保持既有会话状态、输入流程与主题变量，新增 desktop/narrow 回归和原生窗口核验。
```

- [ ] **Step 2: Run required verification**

Run:

```powershell
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
pnpm exec playwright test e2e/workbench.spec.ts --grep "command-pulse welcome" --project=desktop --project=narrow
pnpm tauri dev
```

Expected: production build and Rust checks pass; the desktop dev window shows the new welcome state without clipping at desktop and narrow widths.

- [ ] **Step 3: Mark the roadmap task completed in the same change**

```markdown
| 当前状态 | 用户优先插入的 `P10-118` 指令脉冲默认状态与品牌标识已完成 |
| 最新完成 | `P10-118` 指令脉冲默认状态与品牌标识 |
| 下一项任务 | 开始 `P10-078` Monaco ESM 与 worker 加载精简 |
```

Add a date-stamped change-record row with the exact command outcomes, targeted e2e result, native Tauri verification result, and any residual limitations.

## Plan Self-Review

- Spec coverage: Task 1 covers the observable default state and both viewports; Task 2 covers the inline SVG, state boundary and style behavior; Task 3 covers the mandatory route-map and quality gates.
- Placeholder scan: no placeholder tasks remain.
- Type consistency: no runtime type or IPC contract changes are introduced; test selectors match the planned JSX class and `data-brand-mark` attribute.

## Execution Note

The user has explicitly selected direct implementation. Execute inline with the test-first sequence above. Do not commit: the repository's `AGENTS.md` requires changes to remain in the working tree unless the user explicitly requests a commit.
