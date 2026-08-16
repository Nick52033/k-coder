# Local Codex Plugin Host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make k-Coder discover, diagnose, enable, disable, and safely delete locally copied Codex plugin packages while adapting supported Skills and MCP servers into the existing runtime.

**Architecture:** Add a bounded plugin domain in `extensions/plugins.rs` that owns package discovery, manifest/component validation, indexed reads, persistence keys, diagnostics, and deletion target validation. `ExtensionService` remains the single extension fact and registry-rebuild boundary; plugin Skills become two read-only `ToolHandler`s and plugin MCP definitions are converted to the existing MCP client without creating another agent loop. Tauri commands and React consume versioned protocol payloads and never accept a filesystem deletion path.

**Tech Stack:** Rust 2024, Tokio, serde/serde_json/serde_yaml, Tauri 2, React 19, TypeScript, lucide-react, Playwright.

**Repository rule:** Do not run `git commit` or `git push`; `AGENTS.md` explicitly keeps completed work in the working tree unless the user requests a commit.

---

### Task 1: Versioned plugin protocol and bounded package discovery

**Files:**
- Create: `src-tauri/src/extensions/plugins.rs`
- Modify: `src-tauri/src/extensions/mod.rs`
- Modify: `src-tauri/src/protocol/mod.rs`
- Test: `src-tauri/src/extensions/plugins.rs`
- Test: `src-tauri/src/protocol/mod.rs`

- [x] **Step 1: Write failing protocol serialization tests**

Add tests that construct the stable public states and assert camel-case payloads:

```rust
let value = serde_json::to_value(PluginOverview {
    schema_version: 1,
    root_path: r"D:\data\runtime-data\plugins".into(),
    plugins: vec![],
    error: None,
}).unwrap();
assert_eq!(value["schemaVersion"], 1);
assert_eq!(value["plugins"], serde_json::json!([]));
assert_eq!(serde_json::to_value(PluginState::Degraded).unwrap(), "degraded");
```

- [x] **Step 2: Run the focused test and confirm RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_overview_uses_versioned_public_contract -- --exact`

Expected: compilation fails because `PluginOverview` and `PluginState` do not exist.

- [x] **Step 3: Add the public contract**

Define `PluginState`, `PluginComponentSummary`, `PluginDiagnostic`, and `PluginOverview` in `protocol/mod.rs`. Include `schema_version`, stable string states, component counts, warnings, optional error, `enabled`, `deletable`, path, manifest metadata, and credential-name-only MCP diagnostics. Derive `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq`, and `Eq` where field types permit.

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginState { Disabled, Loaded, Degraded, Blocked, Invalid }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginOverview {
    pub schema_version: u32,
    pub root_path: String,
    pub plugins: Vec<PluginDiagnostic>,
    pub error: Option<String>,
}
```

- [x] **Step 4: Write discovery and manifest-validation tests**

Use `tempfile` to cover: direct-child discovery, ignored folders without `.codex-plugin/plugin.json`, stable `<name>@local` IDs, sorting, unknown manifest fields, missing required fields, invalid names, non-UTF-8/over-256-KiB manifests, duplicate IDs, absolute/`..` paths, symlink or Windows reparse-point roots, and the 128-candidate limit. Assert duplicate IDs make every conflicting directory `invalid` and no indexed capability survives.

```rust
write_manifest(root.join("first"), json!({
    "name": "review-tools",
    "version": "1.2.3",
    "description": "Review helpers",
    "futureField": { "kept": true }
}));
let scan = PluginHost::new(data.path().to_path_buf(), projection()).scan().unwrap();
assert_eq!(scan.overview.plugins[0].id, "review-tools@local");
assert_eq!(scan.overview.plugins[0].state, PluginState::Disabled);
```

- [x] **Step 5: Implement `PluginHost` discovery and indexed facts**

Create constants for all specification limits and internal `PluginManifest`, `IndexedPlugin`, `IndexedSkill`, `IndexedResource`, and `PluginScan` types. Implement:

```rust
pub struct PluginHost {
    root: PathBuf,
    projection: ProjectionDb,
    index: Arc<RwLock<PluginIndex>>,
}

impl PluginHost {
    pub fn new(data_root: PathBuf, projection: ProjectionDb) -> Self;
    pub fn scan(&self) -> Result<PluginScan, PluginError>;
    pub fn overview(&self) -> PluginOverview;
    pub fn revision_paths(&self) -> Result<Vec<PathBuf>, PluginError>;
}
```

Scan only direct children, require the root `.codex-plugin/plugin.json`, retain unknown manifest keys, validate names without adding a regex dependency, and resolve each declared component through a shared `resolve_plugin_path` that rejects absolute paths, parents, symlinks, junctions/reparse points, and canonical escape. Read files through byte limits before UTF-8 decoding. Build revision material only from the manifest, supported component files, and declared Skill trees; never walk `node_modules` or unrelated plugin output.

- [x] **Step 6: Run discovery/protocol tests and confirm GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml extensions::plugins -- --nocapture`

Expected: all plugin discovery and public serialization tests pass.

### Task 2: On-demand plugin Skill and resource tools

**Files:**
- Modify: `src-tauri/src/extensions/plugins.rs`
- Modify: `src-tauri/src/extensions/mod.rs`
- Test: `src-tauri/src/extensions/plugins.rs`
- Test: `src-tauri/src/extensions/mod.rs`

- [x] **Step 1: Write failing Skill indexing and tool tests**

Cover optional `triggers`, `risk`, and `enabled`; conservative `write` metadata when risk is absent; plugin-ID namespace isolation; at most 128 direct Skill folders; bounded UTF-8 reads; disabled/unknown plugin denial; resource `..`, absolute, symlink, junction, non-indexed, binary, and oversized rejection.

```rust
let handlers = host.read_handlers();
let result = execute(&handlers, "plugin_skill_read", json!({
    "pluginId": "review-tools@local", "skillName": "review"
})).await.unwrap();
assert!(result.output.contains("# Review"));
host.set_enabled("review-tools@local", false).unwrap();
assert!(execute(&handlers, "plugin_skill_read", json!({
    "pluginId": "review-tools@local", "skillName": "review"
})).await.unwrap_err().to_string().contains("disabled"));
```

- [x] **Step 2: Run the focused tests and confirm RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_skill -- --nocapture`

Expected: tests fail because the plugin index has no read handlers or catalog.

- [x] **Step 3: Implement indexed read handlers and bounded catalog instructions**

Add `PluginSkillReadTool` and `PluginResourceReadTool` as `ToolHandler`s named exactly `plugin_skill_read` and `plugin_resource_read`. Each execution must re-fetch the enabled indexed plugin, re-resolve the canonical target, recheck link/reparse components, enforce 256-KiB UTF-8 output, and return only bounded content plus non-sensitive metadata. Return both handlers with `ToolRisk::Read` only when an enabled plugin exposes Skills.

Add `PluginHost::runtime_catalog(input)` that lists enabled plugin IDs, Skill names, and descriptions without Skill bodies, marks explicit `@<name>` or `plugin://<id>` references first, caps the catalog bytes, and includes the instruction to call `plugin_skill_read` before applying a Skill.

- [x] **Step 4: Integrate the catalog and handlers into `ExtensionService`**

Store `PluginHost` on `ExtensionService`, scan before plugin expansion, append plugin read handlers and risks to `PreparedExtensions`, and append the bounded catalog to `runtime_instructions`. Keep existing built-in/global/project Skill selection semantics unchanged.

- [x] **Step 5: Run focused tests and confirm GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_skill -- --nocapture`

Expected: metadata, namespace, disabled-read, resource-boundary, and catalog tests pass.

### Task 3: Plugin MCP compatibility with per-plugin isolation

**Files:**
- Modify: `src-tauri/src/extensions/plugins.rs`
- Modify: `src-tauri/src/extensions/mcp.rs`
- Modify: `src-tauri/src/extensions/mod.rs`
- Test: `src-tauri/src/extensions/plugins.rs`
- Test: `src-tauri/src/extensions/mcp.rs`
- Test fixture: `src-tauri/test-fixtures/mcp-server.mjs`

- [x] **Step 1: Write failing MCP mapping tests**

Create `.mcp.json` fixtures for stdio and HTTP. Assert namespaced server IDs, structured command/args, relative `cwd`, `${CODEX_PLUGIN_ROOT}` replacement, injected `CODEX_PLUGIN_ROOT`, credential names without values, `Bearer ` HTTP authorization, timeout mapping, OAuth blocking, missing command/credential diagnostics, invalid URL rejection, and one failed plugin leaving another plugin and non-plugin MCP untouched.

```rust
let mapped = map_plugin_mcp("review-tools@local", plugin_root, json!({
  "mcpServers": { "local": {
    "command": "node", "args": ["${CODEX_PLUGIN_ROOT}/server.mjs"],
    "cwd": ".", "env_vars": ["CODEX_PLUGIN_ROOT", "TOKEN"]
  }}
})).unwrap();
assert_eq!(mapped[0].id, "plugin__review_tools_local__local");
assert_eq!(mapped[0].runtime_cwd.as_deref(), Some(plugin_root));
assert!(!format!("{mapped:?}").contains("secret-value"));
```

- [x] **Step 2: Run the focused tests and confirm RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_mcp -- --nocapture`

Expected: mapping tests fail because plugin MCP wire format and runtime launch fields are absent.

- [x] **Step 3: Add non-serializable MCP runtime launch fields**

Extend `McpServerConfig` with runtime-only `cwd`, fixed environment, and secret header prefix data initialized empty by normal `mcp.json` deserialization and skipped by serialization. In `StdioClient::start`, apply only the validated canonical cwd and fixed variables after `env_clear()` and the existing minimal environment. In `HttpClient::new`, prefix only the configured secret header value in memory. Do not loosen existing `mcp.json` schema or its ADR 0038 failure semantics.

- [x] **Step 4: Parse and adapt supported plugin MCP entries**

In `plugins.rs`, parse the common `{ "mcpServers": { ... } }` object with a 1-MiB bound. Accept stdio `command`, string-array `args`, relative `cwd`, `timeout_ms` or `tool_timeout_sec`, and string-array `env_vars`; accept HTTP `type: "http"`, `url`, and `bearer_token_env_var`. Replace only the literal `${CODEX_PLUGIN_ROOT}` token, never arbitrary environment placeholders. Mark `oauth_resource` as blocked rather than falling back to anonymous HTTP.

- [x] **Step 5: Connect plugin servers with isolated diagnostics**

Keep the existing config MCP loop fail-closed. Add a separate enabled-plugin loop that calls `mcp::connect`, catches each plugin component error, discards that component's handlers, records a credential-name-only diagnostic, and continues. Compute each plugin state after Skills and MCP attempts: `loaded`, `degraded`, or `blocked` exactly as the design specifies.

- [x] **Step 6: Run MCP and extension tests and confirm GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_mcp -- --nocapture`

Expected: mappings, isolation, OAuth, credential redaction, cwd/root injection, and stale-handler removal tests pass.

### Task 4: Enable, disable, disappearance, deletion, and full registry rebuild

**Files:**
- Modify: `src-tauri/src/extensions/plugins.rs`
- Modify: `src-tauri/src/extensions/mod.rs`
- Modify: `src-tauri/src/app_state.rs`
- Modify: `src-tauri/src/persistence.rs`
- Test: `src-tauri/src/extensions/plugins.rs`
- Test: `src-tauri/src/app_state.rs`

- [x] **Step 1: Write failing lifecycle and destructive-operation tests**

Cover default disabled state, persisted enablement across `PluginHost` instances, immediate removal when disabled/invalid/disappeared, reset-to-false on disappearance, revision change after manifest/Skill/MCP edits, unknown/invalid enable rejection, deletion by indexed ID only, direct-child revalidation, symlink/junction target rejection, successful deletion, and deletion failure leaving the plugin disabled.

```rust
state.set_plugin_enabled("review-tools@local", true).await.unwrap();
assert!(state.tool_registry().definitions().iter().any(|d| d.name == "plugin_skill_read"));
std::fs::remove_dir_all(plugin_root).unwrap();
state.prepare_extensions(true).await.unwrap();
assert!(!state.tool_registry().definitions().iter().any(|d| d.name == "plugin_skill_read"));
assert_eq!(projection.setting("extension/plugin/review-tools@local").unwrap().as_deref(), Some("false"));
```

- [x] **Step 2: Run lifecycle tests and confirm RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_lifecycle -- --nocapture`

Expected: lifecycle methods are missing.

- [x] **Step 3: Implement persistence helpers and plugin mutations**

Add narrowly scoped `ProjectionDb::delete_setting` and, if required for startup cleanup, `settings_with_prefix`. Implement `PluginHost::set_enabled`, `prepare_delete`, `delete_indexed`, and `clear_missing_enabled`. Validate IDs against the latest scan; never accept a path from callers. Record toggle, disappearance, failed deletion, and successful deletion in the existing extension audit without file bodies or secret values.

- [x] **Step 4: Implement AppState orchestration**

Add:

```rust
pub async fn plugin_overview(&self, refresh: bool) -> PluginOverview;
pub async fn set_plugin_enabled(&self, id: &str, enabled: bool) -> Result<PluginOverview, AppStateError>;
pub async fn delete_plugin(&self, id: &str) -> Result<PluginOverview, AppStateError>;
```

For deletion, persist disabled, force a complete registry rebuild, revalidate and remove the indexed direct child, clear its setting, then rebuild and return fresh diagnostics. If filesystem deletion fails, return a recoverable error while keeping the registry disabled. Ensure any prepare failure installs a registry built only from built-ins/advanced handlers, so stale plugin tools cannot survive.

- [x] **Step 5: Run lifecycle tests and confirm GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_lifecycle -- --nocapture`

Expected: all persistence, rebuild, disappearance, and deletion tests pass.

### Task 5: Typed Tauri commands and frontend API

**Files:**
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types/runtime.ts`
- Modify: `src/api/runtime.ts`
- Test: `src-tauri/src/commands/mod.rs`

- [x] **Step 1: Write failing command boundary tests**

Assert camel-case arguments, versioned responses, unknown IDs mapped to a stable `plugins` command error, and that delete accepts only `plugin_id` with no path field.

- [x] **Step 2: Add the three lightweight commands**

```rust
#[tauri::command]
pub async fn plugin_overview(state: State<'_, AppState>, refresh: bool) -> CommandResult<PluginOverview>;

#[tauri::command(rename_all = "camelCase")]
pub async fn set_plugin_enabled(state: State<'_, AppState>, plugin_id: String, enabled: bool) -> CommandResult<PluginOverview>;

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_plugin(state: State<'_, AppState>, plugin_id: String) -> CommandResult<PluginOverview>;
```

Register them in `lib.rs`. Commands only delegate and map errors.

- [x] **Step 3: Add matching TypeScript contracts and wrappers**

Define literal `PluginState`, component/diagnostic/overview interfaces matching Rust camel-case fields. Add `getPluginOverview(refresh)`, `setPluginEnabled(pluginId, enabled)`, and `deletePlugin(pluginId)` wrappers.

- [x] **Step 4: Run Rust contract tests and TypeScript build**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_command -- --nocapture`

Expected: command contract tests pass.

Run: `pnpm build`

Expected: TypeScript and Vite build successfully.

### Task 6: Plugin settings page and responsive interactions

**Files:**
- Create: `src/components/PluginSettingsPage.tsx`
- Create: `src/components/PluginSettingsPage.css`
- Modify: `src/components/SettingsDialog.tsx`
- Modify: `e2e/workbench.spec.ts`

- [x] **Step 1: Extend the E2E Tauri mock and write failing UI tests**

Add `plugin_overview`, `set_plugin_enabled`, and `delete_plugin` fixtures. Test automatic page load, root path, empty state, all five states, invalid disabled toggle, successful toggle using returned backend facts, rejected toggle retaining returned/refetched facts, delete confirmation/cancel/success, long path wrapping, desktop/narrow width bounds, and dark theme.

```ts
await dialog.getByRole("button", { name: "插件管理" }).click();
await expect(dialog.getByRole("heading", { name: "本地插件" })).toBeVisible();
await expect(dialog.getByText("review-tools", { exact: true })).toBeVisible();
await dialog.getByRole("checkbox", { name: "启用 review-tools" }).check();
await expect.poll(() => invocationArgs.set_plugin_enabled).toEqual({
  pluginId: "review-tools@local", enabled: true,
});
```

- [x] **Step 2: Run the focused Playwright test and confirm RED**

Run: `pnpm exec playwright test e2e/workbench.spec.ts -g "local plugin" --project=desktop`

Expected: plugin page heading and controls are absent.

- [x] **Step 3: Implement the feature-complete settings page**

Build a compact un-nested list. Header shows `rootPath` and refresh icon. Each row shows Puzzle icon, name/version, description, path, component counts, state badge, warnings/error, a labelled checkbox, and a Trash2 icon button with tooltip. Invalid toggles are disabled. Use a modal confirmation that names the plugin and path; cancellation performs no API call. On every success replace state with the returned overview; on error show an alert and refresh backend facts rather than maintaining an optimistic second source of truth.

- [x] **Step 4: Wire navigation and responsive CSS**

Set `plugins` to `available: true` and render `PluginSettingsPage`. Use stable grid tracks, `min-width: 0`, `overflow-wrap: anywhere`, 8-px-or-smaller radii, existing theme variables, and a narrow breakpoint that stacks row controls without horizontal overflow.

- [x] **Step 5: Run focused desktop and narrow UI tests and confirm GREEN**

Run: `pnpm exec playwright test e2e/workbench.spec.ts -g "local plugin"`

Expected: desktop and narrow projects pass, including delete confirmation and bounds.

### Task 7: ADR, architecture, extension guide, and roadmap synchronization

**Files:**
- Create: `docs/adr/0040-local-codex-plugin-host.md`
- Modify: `docs/开发路线图.md`
- Modify: `docs/架构.md`
- Modify: `docs/扩展.md`

- [x] **Step 1: Write ADR 0040**

Record the accepted decisions: local directory drop only, default disabled, stable `<name>@local`, direct-child and link safety, reuse of `ExtensionService`, per-plugin MCP isolation, indexed read-only Skill access, and explicit non-execution of Apps/OAuth/Hooks/agents/commands.

- [x] **Step 2: Update architecture and operator-facing extension documentation**

Document `runtime-data/plugins`, supported manifest/Skill/MCP subsets, data flow, credential names, state meanings, limits, deletion behavior, unsupported components, and the fact that k-Coder neither downloads plugins nor installs external runtimes.

- [x] **Step 3: Update Phase 10 facts only after implementation verification**

Add a completed user-priority `P10-119` entry, set current/latest to the local plugin host, retain `P10-078` as next, and prepend a dated change-log entry containing the exact verification results. Do not mark the item complete until Tasks 8 and 9 have passed.

### Task 8: Automated verification and regression review

**Files:**
- Modify only if a failure proves the new implementation is at fault.

- [x] **Step 1: Format and inspect the diff**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml`

Run: `git diff --check`

Expected: no whitespace errors.

- [x] **Step 2: Run focused plugin tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml plugin_ -- --nocapture --test-threads=1`

Expected: every plugin protocol, discovery, path, Skill, MCP, lifecycle, and command test passes.

- [x] **Step 3: Run required repository gates**

Run in order:

```powershell
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

Expected: all commands exit 0. Record exact Rust test totals for the roadmap.

- [x] **Step 4: Run focused and full frontend regressions**

Run: `pnpm exec playwright test e2e/workbench.spec.ts -g "local plugin"`

Expected: plugin tests pass in desktop and narrow projects.

Run: `pnpm test:e2e`

Expected: all non-skipped tests pass; if unrelated pre-existing failures remain, capture exact names and prove focused plugin tests pass.

### Task 9: Real Tauri desktop workflow

**Files:**
- Create temporarily outside the repository: `<app_data_dir>/runtime-data/plugins/k-coder-test-plugin/...`
- Remove through the product's confirmed delete flow.

- [x] **Step 1: Start an isolated real desktop instance**

Use a non-conflicting Vite/devtools port and isolated Tauri identifier/data directory, then run `pnpm tauri dev`. Wait for both Vite and the native WebView.

- [x] **Step 2: Place a minimal local test plugin**

Create a plugin containing `.codex-plugin/plugin.json`, `skills/review/SKILL.md`, and one UTF-8 reference file. Do not add it to the repository or package resources.

- [x] **Step 3: Verify the full native path**

In the real app confirm: discovered as disabled, enabled, catalog visible to the same AgentRuntime, `plugin_skill_read` succeeds, disabling removes both read access and catalog, enabling then confirmed deletion removes only that direct plugin directory, and diagnostics contain no secret values. Also inspect desktop and narrow settings layouts for overlap or horizontal overflow.

- [x] **Step 4: Stop dev processes and finalize the roadmap evidence**

Stop the exact isolated Tauri/Vite processes, verify no test plugin remains, then fill the `P10-119` roadmap change-log row with the actual commands, totals, port/identifier, and native workflow observations.

---

## Plan Self-Review

- Spec coverage: directory contract, manifest compatibility, limits, Skills, MCP, unsupported components, persistence, discovery/enable/disable/delete flows, error isolation, UI, security, tests, ADR, roadmap, and real desktop validation each map to Tasks 1-9.
- Placeholder scan: the plan contains no TBD, TODO, “implement later,” or unspecified error/test steps.
- Type consistency: Rust uses `PluginOverview`, `PluginDiagnostic`, `PluginComponentSummary`, and `PluginState`; TypeScript mirrors those names; Tauri arguments consistently use `pluginId` after camel-case mapping; tool names are exactly `plugin_skill_read` and `plugin_resource_read`.
