# 知识库与记忆扩展 Task 1 验证记录（数据库迁移与事件投影）

- 日期：2026-09-15
- 范围：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` Task 1
- 依据：`docs/知识库与记忆扩展详细设计.md` 第 4、9、10 节；ADR 0051；ADR 0053 第 6 条
- 状态：已完成，**未提交、未部署、未做原生桌面验收**

## 1. 交付内容

### 1.1 数据库迁移

`src-tauri/src/persistence.rs`

- `DATABASE_SCHEMA_VERSION` 由 `9` 提升为 `10`。
- `migrate()` 追加 `if version < 10` 迁移块，创建设计文档 §4.1/4.2/4.4 的六张表：
  `memories`、`memory_candidates`、`knowledge_entities`、`knowledge_facts`、
  `knowledge_retrieval_events`、`knowledge_feedback`。
- 按 §9.2 建立索引：`scope/status`（`memories_scope_status`、
  `memory_candidates_scope_status`）、`normalized_key`（`memories_normalized_key`、
  `memory_candidates_normalized_key`）、`source_chunk_id`（`knowledge_facts_source_chunk`）、
  `created_at_ms`（`memories_created_at`、`memory_candidates_created_at`、
  `knowledge_entities_created_at`、`knowledge_facts_created_at`、
  `knowledge_retrieval_events_created_at`、`knowledge_feedback_created_at`），另加
  `knowledge_entities_normalized_name`、`knowledge_entities_collection`、
  `knowledge_facts_subject`、`knowledge_retrieval_events_thread_turn`、
  `knowledge_feedback_citation`。
- §9.3 兼容性：既有 `knowledge_collections`/`knowledge_sources`/`knowledge_revisions`/
  `knowledge_chunks`/`knowledge_chunks_fts`/`knowledge_chunk_embeddings`/
  `knowledge_index_jobs` 未做任何改动。

迁移块使用 `Connection::unchecked_transaction()` 的受检事务，而不是早期版本的内联
`BEGIN; ... COMMIT;` 批次：任何一条语句失败都会回滚整个 v10 块，并且不会写入
`schema_migrations` 的第 10 行。

### 1.2 事件投影与 Repository

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/storage/event_validation.rs` | 两个 Repository 共用的载荷校验（置信度有限性、ID/正文长度、枚举、hex 摘要、非负计数） |
| `src-tauri/src/storage/memory_repository.rs` | `memories`/`memory_candidates` 的版本化事件、投影更新、事件重建、旧 JSONL 回填 |
| `src-tauri/src/storage/knowledge_entity_repository.rs` | `knowledge_entities`/`knowledge_facts`/`knowledge_retrieval_events`/`knowledge_feedback` 的版本化事件、投影更新、事件重建 |

约定与设计文档一致：

- **顺序**：`append()` 先把事实事件写入 JSONL 并 `sync_data`，成功后才更新 SQLite 投影
  （设计 §8“删除先写事实事件，再更新投影”）。投影失败时留下的是可重放的事实，不会出现
  没有事件支撑的投影行。
- **事实日志**：`runtime-data/memory/events.jsonl` 与
  `runtime-data/knowledge/structured-events.jsonl`，均为版本化事件（`schemaVersion` +
  `eventId` + `createdAtMs` + 扁平 `type`/`data`）。
- **重建**：`rebuild_projection()` 先完整解析并校验日志，全部通过后才清空并重放，因此日志
  损坏时重建关闭失败但不会清空已有投影。缺少换行的最后一行按“进程中断”处理并保留前面
  已持久化的事实。
- **禁止 commands 直接执行 SQL**：SQL 只出现在 `persistence.rs` 的迁移与两个 Repository 内，
  通过 `ProjectionDb::with_connection` 的单一连接边界访问。
- **事实不物理删除**：记忆删除走 `memory_status_changed` 软删除；来源 revision 移除走
  `knowledge_facts_expired_for_revision`，把依赖事实置为 `expired`，保留 citation 与审计。
- **隐私边界**：检索事件只接受小写 hex `queryHash`，不保存 query 原文；事件中不含路径、
  凭据、环境变量或会话正文。

### 1.3 旧记忆 JSONL 兼容

`advanced/memories.jsonl`（Phase 9 的 `MemoryStore`）保持只读兼容：

- 事件日志不存在或为空时，`rebuild_projection()` 把旧文件一次性折算为 `memory_upserted`
  事实；同一 `id` 取最大 `revision`，与旧 `MemoryStore::latest_unlocked` 语义一致。
- 映射：`scope_type=user`、`memory_type=fact`、`source_type=user`、`source_ref=旧 source`、
  `confidence=1.0`、`deleted` → `status=deleted`、`createdAtMs`/`expiresAtMs` 原样保留。
- **原文件不修改、不删除**；回填后事件日志非空，后续重建走重放而不是重复导入。

## 2. 测试

新增 21 项回归，全部通过：

- 迁移：`migration_creates_the_knowledge_and_memory_tables_without_touching_the_knowledge_index`、
  `migration_adds_the_scope_status_normalized_key_and_source_chunk_indexes`、
  `scope_status_normalized_key_and_source_chunk_lookups_avoid_full_scans`（`EXPLAIN QUERY PLAN`
  断言走索引且不是 `SCAN`）、`repeated_migration_runs_are_idempotent`、
  `a_failing_migration_block_rolls_back_without_recording_the_version`、
  `migrating_an_existing_v9_database_adds_the_tables_without_touching_existing_rows`。
- 记忆投影：追加即投影、清空后事件重建、候选审核、软删除保留审计行、旧 JSONL 只读回填、
  损坏日志不破坏投影、非法载荷拒绝、未知 ID 状态变更失败、`normalize_memory_key` 确定性。
- 知识实体：实体/事实投影与重建、revision 移除后事实转 `expired` 且不删除、检索事件与反馈
  追加式事实、无来源/无对象/非法状态拒绝、原始 query 与不可能计数拒绝、非法反馈类型拒绝。

## 3. 质量门槛

| 命令 | 结果 |
| --- | --- |
| `pnpm build`（等价 `tsc && vite build`） | 通过；`tsc` 无错误，`vite build` 成功 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 改动前 615 通过 / 16 失败 → 改动后 **636 通过 / 16 失败** |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib mobile::` | 75 通过 / 0 失败（移动网关未受影响） |

失败基线与改动前完全一致（同一批测试名），未引入新失败：

- 3 项为路线图长期记录的既有读取收敛失败
  （`agent::tests::read_recovery_*`、`semantic_read_tracker_recovers_once_before_stopping_overlap_loops`）。
- 13 项为本机环境限制：沙箱内无法创建符号链接/目录联接
  （实测 `mklink /J` 与 `ln -s` 静默失败），因此
  `extensions::*link_escape*`、`patch::tests::rejects_traversal_absolute_and_symlink_paths`、
  `tools::tests::rejects_absolute_parent_and_link_escape_paths` 等“必须拒绝链接逃逸”的断言
  拿不到链接；另有 `execution::shell` 与 `tools` 的 PowerShell/ripgrep 路径专项 2 项。

改动前后测试总数差为 21，与新增回归数量一致。

> 计数口径说明：路线图 `P10-188` 行记录的是“635 通过 / 16 失败”，而本次改动前在同一工作区
> 实测为 615 通过 / 16 失败（总数 631）。两者相差 20 项，且本轮只新增测试、未删除或改名任何
> 既有测试。已如实记录该差异，未做归因。

## 4. 未做与限制

- 未启动 `pnpm tauri dev`：Task 1 只有持久化层，没有任何 IPC 或 UI 调用点，没有可验证的
  桌面路径。原生桌面验收留到 Task 2 起接线 `commands/` 与设置页时执行。
- 未接线 `commands/`、`protocol/`、`memory/` 领域模块；未实现 scope/TTL/去重/冲突/敏感分级
  等业务规则（Task 2 范围）。
- 未运行 `cargo test` 的集成测试目标（`src-tauri/tests/`），该目录属于工作区中既有的移动
  网关未提交改动，不在本次范围。
- 本轮未提交、未部署到 `D:\apps\k-coder\`。

## 5. 工作区说明

改动前工作区已存在另一批未提交改动（移动网关：`src-tauri/src/mobile/`、
`commands/mobile.rs`、`MobileSettingsPage.tsx`、ADR 0059、`axum`/`rcgen`/`tokio-rustls`
依赖等），与本次无关，未触碰。

本次开发期间，`docs/架构.md`（21:41）与 `docs/开发路线图.md`（21:51）被另一个进程改写
（路线图 `P10-188` 条目的补充），不是本次改动所为。路线图更新以该时刻的最新内容为基础追加，
未覆盖 `P10-188` 的既有记录。

本次实际改动：

- 修改：`src-tauri/src/persistence.rs`、`src-tauri/src/storage/mod.rs`
- 新增：`src-tauri/src/storage/event_validation.rs`、
  `src-tauri/src/storage/memory_repository.rs`、
  `src-tauri/src/storage/knowledge_entity_repository.rs`
- 文档：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md`（Task 1 复选框）、
  `docs/开发路线图.md`（当前位置、`P10-143` 子项、变更记录）、本文件
