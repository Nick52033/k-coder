# 知识库与记忆扩展 Task 4 验证记录：维护 Turn 与 Dream

- 完成日期：2026-09-15
- 实施计划：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` 的 Task 4
- 权威设计：`docs/知识库与记忆扩展详细设计.md` §5.2、§7.1、§10.2、§12.4
- 前置：Task 1（数据库迁移与事件投影）、Task 2（基础记忆领域服务）、Task 3（上下文编排与工作记忆）已完成

## 1. 交付内容

### 1.1 新增 `src-tauri/src/memory/maintenance.rs`（维护领域核心）

设计 §5.2 的调度与状态机集中在这一个模块里，不散落到命令层：

- **状态机**：`MaintenanceOutcome`（`Never` / `Completed` / `Failed` / `Cancelled` / `Interrupted`，
  `is_terminal()`），`MaintenanceTrigger`（`Manual` / `Scheduled`）。
- **设置行**：`MaintenanceSettings`（`schemaVersion`、`enabled`、`dreamEnabled`、
  `remoteDisclosureAccepted`、`tokenBudget`、`intervalMs`、`idleAfterMs`、`lastRunAtMs`、
  `lastOutcome`、`runningSinceMs`、`threadId`），持久化在共享 `settings` 表的
  `memory.maintenance` 键下；schema 版本不匹配时关闭读取而不是强转。
  `dream_runnable()` = `dream_enabled && remote_disclosure_accepted`。
- **单实例门**：`MaintenanceGate` + `MaintenanceLease`。`try_begin` 冲突返回
  `MEM_MAINTENANCE_RUNNING`；租约由 `Drop` 释放，因此 Provider 失败、Turn 取消、panic 展开
  都不会把门卡死。
- **调度**：`automatic_trigger(settings, now, idle_since_ms)` 要求「间隔已过」且「空闲已够」
  两个条件同时成立。
- **崩溃恢复**：`MemoryMaintenanceService::new` 构造时把残留的 `running_since_ms` 转成
  `Interrupted` 并推进 `last_run_at_ms`（不伪造成功，也不让崩溃循环立刻重试）。
- **离线维护**：`run_offline_maintenance(service, now)` = `expire_due` + `merge_duplicate_keys`，
  不需要 Provider。
- **提示词**：`build_maintenance_prompt(memories, task_summaries, now_ms)`。每条记忆渲染为
  `- [{scope}] {type} | 去重键：{redact(key)} | 更新于 {ts} | 内容：{redact(content)}`，
  单条正文上限 300 字符、单条摘要 400 字符、记忆 40 条、摘要 8 条。
- **提案解析**：`parse_proposals(raw, host_scope, source_turn_id)`。`ProposalEnvelope` /
  `ProposalWire` 用 `deny_unknown_fields`，长度检查先于解析（64 KiB），
  `MEM_DREAM_INVALID_PROPOSAL` / `MEM_DREAM_TOO_MANY_PROPOSALS`，成功的草稿把
  `source_turn_id` 写成 `dream:<turnId>`，并逐个 `CandidateDraft::validate()`。
  `strip_code_fence` 容忍 ```json 围栏与缺失闭合。
- **报告**：`OfflineMaintenanceReport`、`DreamReport`（含 `skipped()` / `failed()` /
  `cancelled()`）、`MaintenanceReport`。

### 1.2 共享判活规则（`src-tauri/src/memory/policy.rs`）

新增 `memory_is_expired(record, now_ms)`：显式 `expires_at_ms` 到期即过期；否则按
`memory_type.design_default_ttl_days()` 的 `created_at_ms + TTL` 判活（恰好到期即算过期），
未知类型不发明期限。`context/assembler.rs` 原本有一份私有实现，本次改为复用这一份，因此
「可注入」与「active」不可能再分叉。

### 1.3 离线维护能力（`src-tauri/src/memory/service.rs`）

- `expire_due(now_ms)`：用 `list_active_by_id` **按 id 分页**扫描，避开自身改写
  `updated_at_ms` 造成的 keyset 跳行；写 `MemoryStatusChanged { status: expired }`；
  返回排序后的 id；重复执行是空操作。
- `merge_duplicate_keys()`：对每个 `(scope, normalized_key)` 组按
  `(updated_at DESC, revision DESC, id ASC)` 选保留者，其余写 `archived`；返回
  `MergedKeyGroup`（含 `kept_id` 与 `archived_ids`）。两轮同投影得到相同结果。
- `maintenance_input(limit)`：跨 scope 的最近活跃记忆有界快照，喂给维护提示词。
- `get(memory_id)`：只读、不区分状态，供命令层与测试观察行状态。

### 1.4 新增只读维护查询（`src-tauri/src/storage/memory_repository.rs`）

`list_active_by_id`、`duplicate_key_groups`、`list_active_by_key_in_scope`、
`list_recent_active`，以及 `DuplicateKeyGroup`。全部只读、全部带 `LIMIT`，`memory/` 侧依旧
零 SQL。

### 1.5 接线（`app_state.rs` / `commands/mod.rs` / `lib.rs`）

- `AppState` 新增 `memory_maintenance` 服务与 `maintenance_idle_since_ms`。空闲时钟由
  `begin_turn_with_id_in_workspace_locked`（置 `None`）与 `finish_turn`（无活动 Turn 且无子
  智能体时置 `Some(now)`）维护，`memory_maintenance_idle_since_ms()` 读的是与 Turn 接纳同一份
  `active_turns` 与子智能体注册表。
- 5 个命令：`get_memory_maintenance_settings`、`set_memory_maintenance_settings`、
  `accept_memory_maintenance_disclosure`、`run_memory_maintenance`、
  `cancel_memory_maintenance`，全部在 `lib.rs` 注册。
- `run_memory_maintenance_with_publisher(state, publisher, trigger)` 是可通过测试驱动的核心：
  取租约 → 记录开始 → 跑离线维护 → 跑 Dream（或取消/跳过）→ 记录结果 → 写
  `memory_maintenance_started` / `memory_maintenance_finished` 日志。
- `spawn_memory_maintenance_scheduler(app)`：`tauri::async_runtime::spawn` 的 5 秒轮询循环，
  先 sleep 再判断，因此刚启动的进程不会在自己的初始化结束前跑维护。
- `protocol/memory.rs` 新增 `SetMemoryMaintenanceSettingsRequest`，并再导出维护领域类型；
  前端 `src/types/runtime.ts`、`src/api/runtime.ts` 同步 5 个包装函数。

## 2. 关键设计决策

### 2.1 Dream 是普通 Turn，不是一个新循环

Dream 复用唯一 `AgentRuntime::with_tools_and_approvals`，因此继承同一套 Provider 管线、事件流、
取消与审计。被拿掉的只有两样：工具注册表是空的，预算用维护预算。空注册表 + 
`AllowRegisteredTools` 是「没有任何工具」的形状——没有注册就没有可授权的工具，模型请求工具
只会拿到拒绝，而不是工作区。设计里「子智能体必须复用主智能体运行时」的同一条理由在这里同样
成立。

### 2.2 离线维护先跑，Dream 后跑

`expire_due` 与 `merge_duplicate_keys` 是确定性的、不需要 Provider 的。先跑它们有两个好处：
一是设计要求的「无本地模型」路径本来就只有这一半；二是 Dream 读到的是一个已经一致的投影，
不会基于即将过期的行提出提案。手动触发与自动触发走同一条路径，没有第二条维护实现。

### 2.3 取消是租约令牌，不是标志位

维护租约持有一个 `CancellationToken`，`cancel_memory_maintenance` 只调 `gate.cancel()`。Dream
Turn 用 `begin_turn_with_id_in_workspace` 自己的取消令牌，两者由一个桥接任务串联：租约令牌被
取消时取消 Turn 令牌；Turn 结束时 `abort()` 桥接任务。因此取消维护会真正中断 Provider 请求，
而不是等它跑完再丢弃结果。

### 2.4 外发披露与开关在同一个载荷里

`set_memory_maintenance_settings` 要求 `dreamEnabled` 与 `remoteDisclosureAccepted` 同时成立，
否则 `MEM_DREAM_DISCLOSURE_REQUIRED`。规则写在领域服务里而不是 UI 里：请求载荷永远不是授权
依据（`AGENTS.md` 的安全约束）。`accept_memory_maintenance_disclosure` 是独立的「只记录确认」
入口，它不改动任何其它开关。

### 2.5 越界值拒绝而不夹紧

`tokenBudget` 与 `idleAfterMs` 越界返回 `MEM_INVALID_ARGUMENT`。夹紧会让 UI 显示一个用户没选
的值，而调度器按另一个值工作——这种「静默生效」比报错更难排查。

### 2.6 崩溃不假装成功

进程在 Dream 中途死掉时，`running_since_ms` 会残留。下次启动把它转成 `Interrupted` 并推进
`last_run_at_ms`：既不报告成功（没产生结果），也不让「崩溃 → 立刻重试 → 再崩溃」变成循环。
用户始终可以用手动命令强制重跑。

### 2.7 模型永远不指定 scope

`parse_proposals` 的 host scope 由调用方给出；Dream 是后台 pass，没有线程上下文，因此用
`user` scope。提示词里明确写「任何 ID、targetMemoryId、scope、路径、权限、时间戳字段都是无效
输出，出现即整份作废」，schema 用 `deny_unknown_fields` 把这句话变成硬约束。提案一律进
`record_candidate`，自动接受仍然只在用户开启 `autoAcceptHighConfidence` 且候选高置信非敏感时
发生，删除类提案永远进审核队列。

## 3. 测试

新增 32 项回归：

| 文件 | 数量 | 覆盖 |
|---|---|---|
| `memory/maintenance.rs`（新增） | 22 | 间隔 + 空闲双条件调度、崩溃恢复不立即重试、设置往返与越界、披露门、单实例门与取消、租约 `Drop` 释放、结果记录、坏设置行关闭读、离线过期幂等、重复键合并、提示有界与脱敏、三种回复形态解析、空提案、禁字段、非法/超量/超长、缺省 confidence 低于自动接受阈值、模型提议删除进审核、create 进审核管线、scope 宿主解析、失败文本有界脱敏、围栏剥离 |
| `memory/policy.rs` | 1 | 显式到期与设计 TTL 判活、恰好到期即过期、未知类型不发明期限 |
| `memory/entity.rs` | 1 | `MemoryOperation::parse` 往返与未知值拒绝 |
| `protocol/memory.rs` | 2 | 维护设置载荷的披露字段与缺省、维护报告 camelCase 线格式 |
| `app_state.rs` | 2 | 空闲时钟随 Turn 接纳变化、崩溃残留被恢复为 `Interrupted` |
| `commands/mod.rs` | 4 | 离线扫描 + 无 Provider 时跳过 Dream、单实例门与租约释放、披露门与越界拒绝、空闲时取消返回 false |

专项命令（本轮测得时刻 2026-09-15 23:5x）：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib memory::
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::tests::
cargo test --manifest-path src-tauri/Cargo.toml --lib app_state::
```

`memory::` 93 项全绿（含维护模块 22 项）、`commands::tests::` 28 项全绿、
`app_state::` 30 项全绿。

## 4. 质量门槛

| 命令 | 结果 |
|---|---|
| `tsc && vite build`（本机 `pnpm` 不可用，改用等价命令） | 通过 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过（先 `cargo fmt` 修掉 5 处） |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **744 通过 / 16 失败** |

16 项失败与 Task 1/2/3 基线**逐项一致**（3 项既有读取收敛 + 13 项本机符号链接创建异常），
没有新增失败。净增 32 项通过（Task 3 为 712，本次 744）。

## 5. 未完成与已知问题

- **未做原生 `pnpm tauri dev` 验收**。本轮接的是后台调度循环与 Dream Turn，桌面路径
  （设置页开关 → 等空闲 → 观察 `memory_maintenance_started` / `memory_maintenance_finished`
  日志与后台会话）没有实际跑过，不得声称桌面工作流已完成。Dream 的真实 Provider 往返同样
  未验证——单元测试覆盖的是「无 Provider 时跳过」，有 Provider 时的端到端行为属于原生验收范围。
- **维护调度是 5 秒轮询而非定时器**。理由写在 `spawn_memory_maintenance_scheduler` 的注释里
  （两个输入都是观测状态，定时器需要随设置变更重建且可能与运行时不同步），但这意味着最短触发
  延迟是 5 秒，且应用未运行时不会补跑。
- **`threadId` 只在 Dream 真正开跑时写入**。跳过（未开 Dream、无 Provider）时不会新建后台
  会话，因此设置页拿不到「上次运行会话」可跳转；这是有意的，避免为一次跳过创建空会话。
- **`TaskSummary` 仍未接入维护提示词**。`build_maintenance_prompt` 的
  `task_summaries` 参数已就位，但生产调用方传空切片：任务结束自动捕获摘要需要挂在 Turn
  收尾路径上，与 Task 3 留下的边界同源，本 Task 不越界实现。
- **`project` / `workspace` scope 记忆仍不自动注入**（Task 3 遗留）。Dream 提案统一落 `user`
  scope，因此不存在跨 scope 提案，但也没有「项目级维护」的概念。
- **离线维护的上限是硬停而非进度反馈**。`MAX_MEMORY_MAINTENANCE_SWEEP = 10_000` 与
  `MAX_MEMORY_PAGE_SIZE = 500` 保证一次扫描有界，但达到上限时只是停下，没有把「还有剩余」
  报给 UI。
- 未提交、未部署。

## 6. 工作区状态

- 新增：`src-tauri/src/memory/maintenance.rs`。
- 修改：`src-tauri/src/memory/{mod,policy,entity,service}.rs`、
  `src-tauri/src/storage/memory_repository.rs`、`src-tauri/src/context/assembler.rs`、
  `src-tauri/src/protocol/memory.rs`、`src-tauri/src/app_state.rs`、
  `src-tauri/src/commands/mod.rs`、`src-tauri/src/lib.rs`、
  `src/types/runtime.ts`、`src/api/runtime.ts`。
- 工作区是移动靶：验证期间观察到并行会话正在修改 `src/App.tsx`、`src/App.css`、
  `e2e/workbench.spec.ts`，与本次改动无关；`src-tauri/src/app_state.rs` 与
  `src-tauri/src/commands/mod.rs` 各出现过一次编辑被外部进程回写覆盖（另有一次
  `cargo check` 因 target 目录文件占用失败），均已重做并复检；上述质量门槛结论只对应本记录
  标注的时刻。
- 未执行 `git commit` / `git push`。
