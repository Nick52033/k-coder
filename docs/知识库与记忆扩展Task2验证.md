# 知识库与记忆扩展 Task 2 验证记录：基础记忆领域服务

- 日期：2026-09-15
- 范围：《知识库与记忆扩展详细设计》实施计划 Task 2
- 前置：Task 1（数据库迁移与事件投影）已完成，见 `docs/知识库与记忆扩展Task1验证.md`
- 状态：领域层、IPC 层与前端契约已完成并通过质量门槛；**未做原生桌面验收、未提交、未部署**

## 1. 交付内容

### 1.1 新增领域模块 `src-tauri/src/memory/`

| 文件 | 职责 |
|---|---|
| `mod.rs` | 模块出口与 `MemoryError`（带稳定错误码，`From<ProjectionError>` 把 `InvalidData` 映射为 `MEM_INVALID_DATA`，其余映射为 `MEM_STORAGE`） |
| `entity.rs` | `MemoryScope`/`MemoryScopeKind`、`MemoryType`、`Sensitivity`、`MemorySourceType`、`MemoryStatus`、`MemoryOperation`、`MemoryCursor` |
| `policy.rs` | 默认 TTL、敏感项检测、确认令牌校验 |
| `candidate.rs` | `CandidateDraft`、`CandidateDecision`、`ConflictKind`、`CandidateOutcome` 与审核判定 |
| `service.rs` | `MemoryService`：settings、list、upsert、record_candidate、review_candidate、delete、clear |

模块边界遵循设计 §3.1：`memory/` 不含任何 SQL，全部读写经 `storage::memory_repository`；`memory/` 也不依赖 `protocol/`，IPC 载荷由 `protocol/memory.rs` 反向引用领域类型。

### 1.2 新增 IPC 载荷 `src-tauri/src/protocol/memory.rs`

`UpsertMemoryRequest`、`SetMemorySettingsRequest`；响应类型直接复用领域类型（`MemoryPage`、`MemoryUpsertOutcome`、`MemorySettings`、`MemoryClearOutcome`）。请求字段保持字符串，由领域模块解析，因此未知枚举与非法 scope 返回带码的领域错误，而不是命令边界无法描述的反序列化失败。

### 1.3 命令与接线

`src-tauri/src/commands/mod.rs` 的记忆命令整体迁移到新服务，`src-tauri/src/lib.rs` 完成注册：

```text
get_memory_settings()                                  -> MemorySettings
set_memory_settings(enabled, autoAcceptHighConfidence, defaultTtlDays)
set_memory_enabled(enabled)                            // 兼容保留，只改 enabled
list_memories(scope, status?, cursor?, limit?)         -> MemoryPage
upsert_memory(request)                                 -> MemoryUpsertOutcome
list_memory_candidates(status?, limit?)                -> MemoryCandidateRecord[]
review_memory_candidate(candidateId, decision)         -> MemoryCandidateRecord
delete_memory(memoryId, confirmationToken)             -> MemoryRecord
clear_memories(scope, confirmationToken)               -> MemoryClearOutcome
```

`AppState` 新增 `memory: MemoryService` 与 `pub fn memory()`，并在构造时用旧 `advanced/memory-settings.json` 的 `enabled` 播种一次新设置行，使升级不会静默关闭记忆。

`run_memory_maintenance` 未在 Task 2 实现：实施计划把该命令与维护 Turn 一起归到 Task 4。

### 1.4 存储层补充（`storage/memory_repository.rs`）

Task 1 的既有 API 未改动，仅新增三个只读投影查询：

- `list_page(scope_type, scope_id, status, after, limit)`：`(updated_at_ms DESC, id ASC)` keyset 分页。设置页打开期间仍在写入记忆，用 `OFFSET` 会跳行或重复，keyset 游标不会。
- `list_by_key(normalized_key, limit)`：走 `memories_normalized_key` 索引，供服务判定去重/冲突/跨 scope。
- `count_in_scope(scope_type, scope_id, status)`：分页同时给出 scope 总数。

### 1.5 前端契约

`src/types/runtime.ts` 与 `src/api/runtime.ts` 同步为新契约（新增 `MemoryRecord`、`MemoryPage`、`MemoryUpsertOutcome`、`MemoryCandidate`、`MemoryClearOutcome`、`SetMemorySettingsRequest` 及枚举联合类型）。改动前已确认 `MemoryView`/`MemoryUpsertRequest`/`MemorySettings` 与五个旧包装函数在 `src/` 和 `e2e/` 中**没有任何调用方**。

## 2. 关键设计决策

### 2.1 IPC 破坏性变更（已确认低风险）

设计 §7.1 的 `list_memories`/`upsert_memory`/`delete_memory`/`get_memory_settings` 与 Phase 9 高级记忆同名命令冲突。按设计语义演进这些命令名，依据是：

- 五个前端包装函数零调用方；
- 设置对话框没有「记忆」分区（`SettingsDialog.tsx` 中唯一的 `memory` 命中是无关 CSS 类名 `memory-row`）；
- `e2e/workbench.spec.ts:5548` 的「记忆」用例点击的是不存在的设置按钮，在改动前就已失效（见 §5）。

旧 `advanced/memory.rs` 与其 `recall_memory`/`remember` 工具、`advanced/memories.jsonl` 完全保留；`set_memory_settings`/`set_memory_enabled` 会把 `enabled` 镜像回旧存储，因此旧工具仍按用户选择工作。

### 2.2 模型不能提交真实 memory ID（结构性保证，非约定）

- `CandidateDraft::from_model` 无法表达 `target_memory_id`、scope、敏感等级、时间戳或确认令牌；scope 由宿主从当前 Turn 解析后作为参数传入。
- `record_candidate` 的目标 ID 一律由宿主按 `(scope, normalized_key)` 解析；`create` 携带 target 直接 `MEM_INVALID_ARGUMENT`。
- `delete_memory`/`clear_memories` 的确认令牌必须等于宿主算出的 `memoryId`/规范 scope 字符串，与知识库删除契约一致。

### 2.3 去重与冲突的语义

- **去重**：`normalized_key`（小写、折叠空白、有界）即记忆身份。同 key 同正文 = 幂等空操作，不写事件、不增版本；同 key 不同正文 = 同一条记忆被重新表述，版本 +1 且保留 `created_at_ms`；不同 key = 两条记忆。
- **冲突**：`create` 候选命中同 scope 不同正文时，宿主把它改判为 `update` 并**强制人工审核**（"以为是新增，其实在覆盖"正是设计 §4.2 要求人工裁决的冲突）。
- **跨 scope 更新**：`update` 目标与草稿 scope 不一致时标记 `CrossScopeUpdate`，同样强制审核。
- **自动接受**：仅当 `auto_accept_high_confidence` 打开、置信度 ≥ 0.8、且宿主检测的敏感等级为 `normal` 时才自动落库；默认关闭，即默认全部进审核队列。

### 2.4 敏感项与 TTL

- 敏感等级由宿主检测，调用方无法降低：复用 `execution::redact`（记忆与日志对"什么像凭据"的判断一致），外加 `name: value` 形态扫描；绝对路径、邮箱、手机号判为 `private`。
- **凭据形态的内容直接拒绝**（`MEM_SECRET_REJECTED`），不写入记忆——设计 §8 要求 API Key 只存操作系统凭据槽。
- TTL：显式 `expiresAtMs` 优先（必须在未来且 ≤ 3650 天）；否则 `work_state` 14 天、`experience` 180 天（设计 §4.3 固定值优先）；其余类型回落到 `defaultTtlDays`，`0` 表示不过期。

### 2.5 清除与审计

- 删除是软删除：写 `memory_status_changed` 事件，投影行保留，重建可复现；重复删除幂等且不产生第二条审计事件。
- `clear_memories` 逐条写 `memory_status_changed`，因此清除可按行审计，而不是一个不可追溯的批量操作。
- 记忆与学习数据分开清除：`clear_memories` 不触碰 `knowledge_retrieval_events`/`knowledge_feedback`（有回归断言）。

## 3. 测试

新增回归覆盖设计 §10.1 的 scope、TTL、版本递增、去重、冲突、敏感项分类、非法 ID、模型伪造删除与事件重建：

| 文件 | 数量 |
|---|---|
| `memory/entity.rs` | 6 |
| `memory/policy.rs` | 5 |
| `memory/candidate.rs` | 7 |
| `memory/service.rs` | 24 |
| `protocol/memory.rs` | 3 |
| `storage/memory_repository.rs`（新增） | 2 |
| `app_state.rs`（新增） | 2 |

专项命令：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib memory::
cargo test --manifest-path src-tauri/Cargo.toml --lib app_state::tests::
```

两轮均全绿（`memory::` 46 项、`app_state::tests::` 28 项）。

## 4. 质量门槛

| 命令 | 结果 |
|---|---|
| `pnpm build`（本机 corepack 链接损坏，改用等价 `tsc && vite build`） | 通过 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 686 通过 / 16 失败 |

失败清单与 Task 1 基线**逐项一致**：3 项既有读取收敛失败（`agent::tests::read_recovery_*`、`semantic_read_tracker_*`）+ 13 项本机符号链接创建异常导致的环境失败（`extensions::*link*`、`patch::tests::rejects_traversal_absolute_and_symlink_paths`、`tools::tests::rejects_absolute_parent_and_link_escape_paths`、`run_command_explains_and_recovers_from_powershell_rg_path_globs` 等）。基线为 636 通过 / 16 失败，本轮 +50 项通过，失败集合未变。

> 说明：`pnpm build` 在本机因 corepack/pnpm 软链接损坏（`Cannot find module 'D:\c\Users\...\corepack\dist\pnpm.js'`）无法直接执行，沿用 Task 1 的等价替代 `node node_modules/typescript/bin/tsc && node node_modules/vite/bin/vite.js build`，两者均成功。

## 5. 未完成与已知问题

1. **未做原生桌面验收**：Task 2 只到 IPC 与前端契约，没有新增任何界面调用点（记忆列表/审核/删除确认 UI 属设计 §7.1 的界面部分，与 Task 6 的实体审核一起排期），因此未启动 `pnpm tauri dev`。按仓库约定，这不构成"桌面工作流已完成"。
2. **`e2e/workbench.spec.ts:5548` 已失效且本轮未修**：该用例点击名为 `/^记忆/` 的设置按钮并 mock 旧的 `list_memories` 返回数组，而设置对话框既无该按钮、命令契约也已变更。它在 Task 2 之前就已失效，属既有问题；正确修法依赖尚不存在的记忆设置界面，留待界面落地时一并处理。
3. **`run_memory_maintenance` 未实现**：按实施计划归 Task 4。
4. **`enabled` 的语义**：`enabled` 只控制后续的自动捕获与上下文注入（Task 3/4），不拦截用户介导的查看/编辑/审核/删除——设计 §12.4 要求记忆在任何时候都可查看、可审核、可独立清除。
5. **迁移窗口的旧记忆工具**：`recall_memory`/`remember` 仍读写 `advanced/memories.jsonl`，与新的 `memory/events.jsonl` 并行；旧文件被只读回填进投影（Task 1），但旧工具新写入的记忆要等下一次回填才会出现在新列表里。统一这两个写入路径属 Task 4 的候选流水线工作。
6. **多行同 key 的极端情况**：若历史数据里已有两条同 `(scope, key)` 的不同 id 行，`upsert` 会更新较新的那条；合并重复行属 Task 4 的去重维护。

## 6. 工作区状态

- 修改文件：`src-tauri/src/lib.rs`、`src-tauri/src/app_state.rs`、`src-tauri/src/commands/mod.rs`、`src-tauri/src/protocol/mod.rs`、`src-tauri/src/storage/memory_repository.rs`、`src/types/runtime.ts`、`src/api/runtime.ts`
- 新增文件：`src-tauri/src/memory/{mod,entity,policy,candidate,service}.rs`、`src-tauri/src/protocol/memory.rs`
- 未执行 `git commit`、未部署到安装目录 `D:\apps\k-coder\`
