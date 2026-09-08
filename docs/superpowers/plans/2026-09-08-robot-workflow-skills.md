# Robot Workflow Skills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reproduce the current cn-codex robot capability declarations as safe k-Coder workflows, with categorized public Skills, plugin fallbacks, strict readiness checks, and per-request dynamic instruction compilation.

**Architecture:** ExtensionService remains the only ordinary Skill registry and PluginHost remains the plugin registry. Workflow nodes declare local or plugin Skills; plugin declarations may resolve to an explicit built-in compatibility Skill. AgentRuntime receives a dynamic instruction provider and recompiles the current node before context estimation on every outer Provider request.

**Tech Stack:** Rust 2024, Tauri 2, React 19, TypeScript 5.8, Playwright, serde/serde_yaml.

**Spec:** `docs/adr/0055-robot-node-skill-binding.md`

## Global Constraints

- Target the user-provided UI versions: fullstack 23 declarations/8 steps, QA 16/7, requirements 9/7.
- Keep ordinary Skill precedence `builtin < global < project` outside robots; robot-pack IDs are reserved built-ins and cannot be overridden by global/project Skills.
- Preserve original local/plugin declarations in UI but deduplicate equal normalized bodies by SHA-256 before injection.
- Reject missing or oversized declarations before Turn start; robot-pack Skills are always enabled and plugin declarations automatically use their explicit built-in fallback when unavailable.
- Allow at most 24 declarations, 24 unique bodies, 16 KiB per body, and 128 KiB total per node.
- Never persist automatically injected bodies in JSONL, logs, workflow snapshots, or UI payloads.
- Normalize and contain every path, including Windows reparse points and directory junctions.
- Do not stage unrelated concurrent changes, including the subagent panel and usage-metrics work.
- Do not commit or push until the full repository gates and desktop workflow pass.

---

### Task 1: Skill category contract and grouped discovery

**Files:**
- Modify: `src-tauri/src/extensions/mod.rs`
- Modify: `src/types/runtime.ts`

**Interfaces:**
- Produces: public `SkillCategory` enum and `SkillDiagnostic.category`.
- Produces: flat and one-group-deep Skill discovery with same-scope duplicate rejection.

- [x] Write extension tests that fail when a missing category does not default to `other`, an unknown category is accepted, a grouped Skill is skipped, a same-scope duplicate overwrites silently, or a reparse-point escape is accepted.
- [x] Run the focused tests and confirm each fails for the absent contract.
- [x] Implement the serde enum, metadata default, diagnostic field, bounded candidate discovery, canonical containment, and duplicate set.
- [x] Update TypeScript diagnostics and fixtures, then run focused Rust tests and `pnpm build` to green.

### Task 2: Effective ordinary/plugin resolution and resource reads

**Files:**
- Modify: `src-tauri/src/extensions/mod.rs`
- Modify: `src-tauri/src/extensions/plugins.rs`
- Modify: `src-tauri/src/app_state.rs`

**Interfaces:**
- Produces: body-private resolved ordinary and plugin Skill records with ID, scope/source, risk, enabled state, bytes, and SHA-256.
- Produces: plugin resolution with explicit ordinary fallback.
- Produces: read-only `skill_resource_read` handler for effective ordinary Skills.

- [x] Write failing tests for builtin/global/project precedence, enabled plugin preference, missing/disabled plugin fallback, no-fallback failure, and equal-body hash identity.
- [x] Write failing resource tests for valid bounded UTF-8 reads, unknown/disabled Skills, absolute paths, parent traversal, directories, binary data, links/reparse points, and size bounds.
- [x] Implement resolution APIs without exposing bodies through serde diagnostics.
- [x] Implement and register `skill_resource_read`; run extension/plugin/tool tests to green.

### Task 3: Complete workflow definitions and readiness protocol

**Files:**
- Modify: `src-tauri/src/advanced/workflow.rs`
- Modify: `src-tauri/src/advanced/mod.rs`
- Modify: `src-tauri/src/app_state.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/api/runtime.ts`
- Modify: `src/types/runtime.ts`

**Interfaces:**
- Produces: `WorkflowSkillBindingDefinition`, serialized binding views, and readiness views.
- Produces: `get_workflow_skill_readiness(workflowId)`.
- Extends: `CommandError` with optional structured `details`.

- [x] First replace workflow definition assertions with literal 8/7/7 node tables and exact local/plugin declaration lists; confirm old 5-node definitions fail.
- [x] Add tests proving legacy active runs never map an old index to a new node and instead return an actionable incompatible-run error.
- [x] Add command tests proving every blocker is returned before workflow state or `TurnStarted` is written.
- [x] Implement the three approved definitions, stable new node IDs, binding views, readiness aggregation, bounds, and structured error details.
- [x] Invoke preflight on direct send, queued send, and retry; run workflow/AppState/command tests to green.

### Task 4: Per-request RuntimeInstructionProvider

**Files:**
- Modify: `src-tauri/src/agent/mod.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/advanced/mod.rs`
- Modify: `src-tauri/src/advanced/workflow.rs`

**Interfaces:**
- Produces: `RuntimeInstructionProvider::compile() -> Result<RuntimeInstructionSnapshot, String>`.
- Preserves: `with_runtime_instructions(String)` through a static provider.

- [x] Write an AgentRuntime test where request one observes node A, a real test tool advances host state, and request two must observe node B; verify static instructions fail it.
- [x] Add failing tests for compile error before provider I/O, runtime-size compaction input, transient retry snapshot reuse, equal-body deduplication, host enforcement ordering, and no body in stored events.
- [x] Replace the runtime string field with a provider, compile before each outer request's context calculation, and preserve one snapshot inside transient retry loops.
- [x] Build the main-turn provider from fixed workspace/mode/tools/memory/input plus live extensions and workflow state.
- [x] Render all original declarations in headers, inject unique bodies once, append host enforcement last, and emit hash-only bounded audit metadata.
- [x] Run agent, context, command, and workflow tests to green.

### Task 5: Package and adapt 28 public Skills

**Files:**
- Create: `src/resources/skills/robot-pack/NOTICE`
- Create: `src/resources/skills/robot-pack/LICENSE-APACHE-2.0`
- Create: `src/resources/skills/robot-pack/LICENSE-SUPERPOWERS-MIT`
- Create: `src/resources/skills/robot-pack/<28-skill-id>/SKILL.md`
- Create: required `references/*.md` resources
- Modify: all 31 existing `src/resources/skills/*/SKILL.md` frontmatters
- Modify: `src-tauri/src/extensions/mod.rs` bundled-catalog tests

**Interfaces:**
- Produces: exactly 59 explicitly categorized built-in ordinary Skills.
- Preserves upstream ID `test-driven-development` for cn-codex compatibility.

- [x] Write a real bundled-catalog test that fails until all 59 Skills parse, all 28 target IDs exist, all workflow fallbacks resolve, and every bound body meets limits.
- [x] Add explicit categories to the 31 current Skills without changing existing risk, triggers, or enablement.
- [x] Adapt the original 23 Skills, replacing unavailable tool names, external-container assumptions, automatic commit/push, and root-level generated docs.
- [x] Add adapted `dispatching-parallel-agents`, `subagent-driven-development`, `webapp-testing`, `control-in-app-browser`, and `documents` compatibility Skills.
- [x] Split large/resource-oriented content for `skill_resource_read`; exclude executable upstream helpers and plaintext credential storage.
- [x] Preserve Apache/MIT notices and run the real catalog/workflow readiness tests to green.

### Task 6: Categorized Skills and complete robot UI

**Files:**
- Modify: `src/components/SettingsDialog.tsx`
- Modify: `src/components/WorkflowSelector.tsx`
- Modify: `src/components/WorkflowControl.tsx`
- Modify: `src/App.tsx`
- Modify: `src/stores/workbenchStore.ts`
- Modify: `src/api/runtime.ts`
- Modify: `src/types/runtime.ts`
- Modify: `src/App.css`
- Modify: `e2e/workbench.spec.ts`

**Interfaces:**
- Consumes: category diagnostics, local/plugin binding views, and readiness statuses.
- Produces: category/search/source filters and exact 23/16/9 robot declaration summaries.

- [x] Add Playwright scenarios that fail on flat Skill rendering, wrong category order, missing `other`, broken filtering, missing plugin/fallback labels, wrong robot counts, blocker navigation, and narrow-screen overflow.
- [x] Implement fixed category labels/icons/order and unframed grouped Skill rows with search/category/source controls.
- [x] Render all node declarations with local/plugin and resolved-source state in Robots; show current-node declaration summary in WorkflowControl.
- [x] Surface readiness in WorkflowSelector and intercept known-blocked sends while retaining backend enforcement for stale races.
- [x] Run `pnpm build` and targeted Playwright scenarios; inspect desktop/narrow screenshots and correct overlap or wrapping defects.

### Task 7: Documentation, roadmap, review, and delivery

**Files:**
- Modify: `docs/机器人工作流设计.md`
- Modify: `docs/开发路线图.md`
- Modify: `docs/adr/0055-robot-node-skill-binding.md` only for implementation facts

**Interfaces:**
- Produces: completed user-priority roadmap item `P10-151`; preserves the factual state of existing `P10-142` and concurrent `P10-150` work.

- [x] Reconcile the implementation line by line with ADR 0055 and update architecture, roadmap checkbox/current position/next task/change log.
- [x] Run the production TypeScript and Vite build from the isolated staged snapshot.
- [x] Run `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`.
- [x] Run `cargo check --manifest-path src-tauri/Cargo.toml`.
- [x] Run `cargo test --manifest-path src-tauri/Cargo.toml`.
- [x] Run targeted Playwright coverage and `git diff --check`.
- [x] Start Tauri dev with an isolated app identifier and ports; verify native IPC catalogs/readiness, categorized Skills, managed locks, robot details, and layout. Strict blockers, fallback, node transition, runtime invalidation, and resume remain covered by Rust and Playwright tests.
- [x] Review the staged diff for secrets, generated artifacts, body leakage, missing notices, accidental Cargo staging, and unrelated changes. The current task rules do not authorize an additional reviewer subagent.
- [x] Stage only explicit feature files, commit with a Chinese conventional message, and push `codex/robot-workflow-skills` to `origin`.
