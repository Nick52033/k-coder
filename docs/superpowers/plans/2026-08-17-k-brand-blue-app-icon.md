# K Brand And Blue App Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the `K` product identity in the titlebar and native application icon while keeping a distinct command-pulse illustration in the new-conversation welcome state.

**Architecture:** Keep `BrandGlyph` and `WelcomeGlyph` as separate presentation-only inline SVGs with separate DOM contracts, and keep `assets/app-icon.svg` as the single native icon source. Generate every platform asset through the existing Tauri CLI, then validate the DOM roles, source color, raster output, build, Rust gates, and desktop workflow.

**Tech Stack:** React 19, TypeScript, SVG, Playwright, Tauri 2 CLI, Rust.

---

## File Map

- `e2e/workbench.spec.ts`: separate brand/welcome DOM contracts and native SVG source regression.
- `src/App.tsx`: theme-aware `K` brand glyph plus a distinct command-pulse welcome glyph.
- `assets/app-icon.svg`: blue native icon source of truth.
- `src-tauri/build.rs`: rebuild trigger for the embedded Windows ICO resource.
- `src-tauri/icons/`: Tauri-generated platform assets.
- `docs/开发路线图.md`: final `P10-118` scope and verified change record.

### Task 1: Establish The Failing Contract

- [x] Require `data-brand-mark="k-letter"` in both UI locations and confirm that contract fails against the old command-pulse brand implementation.
- [x] Add a source-level assertion for the `#2F6FE4` background and canonical white `K` path.
- [x] Run `pnpm exec playwright test e2e/workbench.spec.ts --grep "K brand welcome|CodeBuddy blue K" --project=desktop --project=narrow` and confirm four expected failures against the old implementation.

### Task 2: Implement And Generate Icons

- [x] Replace the command-pulse paths in `BrandGlyph` with the canonical `K` geometry.
- [x] Change `assets/app-icon.svg` from `#176b4d` to `#2F6FE4`.
- [x] Run `pnpm tauri icon assets/app-icon.svg` to regenerate every platform asset.
- [x] Re-run the targeted Playwright command and confirm four passes.

### Task 3: Separate The Welcome Illustration

- [x] Revise the regression to require a single `data-welcome-mark="command-pulse"` in the welcome area, no welcome-area `k-letter`, and a titlebar `k-letter`.
- [x] Run the revised desktop/narrow regression and confirm the two expected failures while the central area still renders `K`.
- [x] Add a dedicated `WelcomeGlyph` and replace only the central welcome usage.
- [x] Re-run the revised regression and confirm four passes, including the native SVG checks.
- [x] Inspect desktop and narrow screenshots for centering and overflow.

### Task 4: Make The Embedded Windows Icon Rebuildable

- [x] Extract the icon from the rebuilt debug executable at a new path and confirm it still contains the old green `K` despite the blue source ICO.
- [x] Add a failing source regression requiring `cargo:rerun-if-changed=icons/icon.ico`.
- [x] Add the rebuild trigger to `src-tauri/build.rs` and confirm the regression passes.
- [x] Let `pnpm tauri dev` regenerate `resource.lib`, then confirm an uncached executable extraction and the live Windows taskbar both display the blue `K`.

### Task 5: Synchronize Documentation And Verify

- [x] Update `P10-118`, the roadmap current position, and the change record with final command outcomes.
- [x] Run `pnpm build`.
- [ ] Run `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: the shared worktree still reports formatting diffs only in concurrently added `src-tauri/src/extensions/mcp.rs` and `plugins.rs`; those unrelated files were preserved.
- [x] Run `cargo check --manifest-path src-tauri/Cargo.toml`.
- [x] Run `cargo test --manifest-path src-tauri/Cargo.toml` and confirm 325/325 pass after concurrent plugin work stabilizes.
- [x] Run `pnpm exec playwright test e2e/workbench.spec.ts --grep "distinct command-pulse|CodeBuddy blue K" --project=desktop --project=narrow` once more after all edits and confirm 4/4 pass.
- [x] Start the `pnpm tauri dev` workflow and inspect the native desktop window, embedded executable icon, and live taskbar button.

## Plan Self-Review

- The plan covers the separate titlebar brand and welcome-illustration roles plus the native taskbar asset chain.
- The SVG source is hand-maintained once; generated platform files are not edited individually.
- No runtime, storage, provider, tool, policy, or protocol contract changes are included.
- No placeholders or unresolved design choices remain.
