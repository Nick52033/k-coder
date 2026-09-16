# 知识库与记忆扩展 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在现有 SQLite + FTS5/BGE-M3 知识库上实现可审计的结构化知识、分层记忆、维护 Turn 和增强检索。

**Architecture:** 新能力以 `knowledge`、`memory`、`retrieval`、`context` 和 `storage` 分层接入。所有写入先追加事实事件，再更新 SQLite 投影；模型只能提出候选，统一由宿主校验和授权。

**Tech Stack:** Rust 2024、Tauri 2、SQLite/rusqlite、FTS5、React、TypeScript、现有 AgentRuntime/ToolRegistry/PolicyEngine、SiliconFlow BGE-M3。

**Spec:** [知识库与记忆扩展详细设计](../../知识库与记忆扩展详细设计.md)

## Global Constraints

- 不引入 PostgreSQL、pgvector、Redis、Elasticsearch 或常驻向量服务。
- 不创建第二套智能体循环；Dream 必须复用 AgentRuntime。
- 模型权限字段不能作为授权依据；所有路径必须规范化并拒绝链接逃逸。
- API Key、Authorization、完整环境变量和完整会话正文不得进入日志或学习样本。
- 每个任务完成后运行相关 Rust 测试；最终执行 `pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test` 和真实 `pnpm tauri dev` 验证。

### Task 1: 数据库迁移与事件投影

**Files:**
- Modify: `src-tauri/src/storage/` 中现有 SQLite migration、Repository 和事件投影文件。
- Create: 对应 knowledge/memory migration、Repository 单元测试。

**Interfaces:**
- Produces: `memories`、`memory_candidates`、`knowledge_entities`、`knowledge_facts`、`knowledge_retrieval_events`、`knowledge_feedback` 表和版本化 Repository 方法。

- [x] 编写迁移成功、重复执行、失败回滚和事件重建测试。
- [x] 添加索引并验证 scope、status、source_chunk_id 查询计划。
- [x] 实现事件追加后投影更新，禁止 commands 直接执行 SQL。
- [x] 验证旧 JSONL 和记忆表兼容读取。

> Task 1 已完成（2026-09-15）。`storage/event_validation.rs` 提供共享载荷校验，
> `storage/memory_repository.rs` 与 `storage/knowledge_entity_repository.rs` 分别负责记忆和
> 结构化知识的事实事件、投影与重建。验证记录见 `docs/知识库与记忆扩展Task1验证.md`。
> 本轮未接线 IPC/UI，未做原生桌面验收；Task 2 起再接入 `commands/` 与 `memory/`。

### Task 2: 基础记忆领域服务

**Files:**
- Create: `src-tauri/src/memory/` 的实体、候选、策略和服务模块。
- Modify: `src-tauri/src/commands/`、`src-tauri/src/protocol/` 注册类型化 IPC。
- Test: memory domain and command tests。

**Interfaces:**
- Consumes: Task 1 Repository。
- Produces: `list_memories`、`upsert_memory`、`review_memory_candidate`、`delete_memory`、`clear_memories` 服务和 IPC。

- [x] 先写 scope、TTL、版本递增、敏感项和非法 ID 失败测试。
- [x] 实现用户/工作区/项目/线程 scope 校验和默认 TTL。
- [x] 实现候选审核、去重、冲突和独立清除审计事件。
- [x] 运行 memory 专项测试并确认模型不能提交真实 memory ID。

> Task 2 已完成：新增 `src-tauri/src/memory/`（`mod`/`entity`/`policy`/`candidate`/`service`）与 `src-tauri/src/protocol/memory.rs`，记忆命令整体迁移到 `MemoryService` 并在 `lib.rs` 注册，`AppState` 增加 `memory` 服务与旧 `enabled` 播种；`storage/memory_repository.rs` 新增 keyset 分页、按 key 查询与 scope 计数。`run_memory_maintenance` 按计划归 Task 4。全量 `cargo test --lib` 686 通过 / 16 失败（失败集合与 Task 1 基线一致）。本轮只到 IPC 与前端契约，没有新增界面调用点，因此未启动 `pnpm tauri dev`；未提交、未部署，详见 `docs/知识库与记忆扩展Task2验证.md`。

### Task 3: 上下文编排与工作记忆

**Files:**
- Create: `agent/` 或现有 context 目录下的 `ContextAssembler`。
- Modify: AgentRuntime 请求构建和 Compaction 边界。
- Test: context priority, budget and refresh tests。

**Interfaces:**
- Consumes: 用户规则、项目规则、Task 2 memories、knowledge citations。
- Produces: 有界、带来源和 revision 的 Provider 上下文片段。

- [x] 编写优先级覆盖和预算裁剪测试。
- [x] 实现安全策略→规则→记忆→知识→推断的确定性排序。
- [x] 实现工作记忆 14 天 TTL 和任务结束压缩。
- [x] 验证 Compaction 不复制图片、密钥或完整工具输出。

> Task 3 已完成：新增 `src-tauri/src/context/assembler.rs`（`ContextTier` 8 级优先级、来源与
> `revision` 记录、敏感过滤、预算裁剪、确定性排序）与 `src-tauri/src/context/task_summary.rs`
> （有界任务摘要：折叠图片载荷、脱敏、字段/列表/总量三级上限）。`context.rs` 的压缩路径补了
> 三处脱敏（摘要正文、高信号工具观察、工具结果，含渲染边界兜底），因此「Compaction 不复制图片、
> 密钥或完整工具输出」有 3 项对抗性测试实证。记忆注入已接到
> `commands::live_runtime_instruction_provider`：`user` 与 `thread` scope 的 active 记忆经组装器
> 进入 `<memory>`，由 `MemorySettings::enabled` 门控，每次注入写一条有界审计日志；旧
> `advanced/memory` 存储保持独立块不并入组装预算。组装期重算 work_state 14 天 / experience
> 180 天 TTL，不依赖 `expires_at_ms` 是否存在。新增 26 项回归；`context::` 40 项、`memory_context`
> 2 项全绿；`tsc && vite build`、`cargo fmt --check`、`cargo check --all-targets` 通过，全量
> `cargo test --lib` 712 通过 / 16 失败（失败集合与 Task 1/2 基线逐项一致）。已知边界：未做原生
> `pnpm tauri dev` 验收；`project`/`workspace` scope 记忆可管理但不自动注入（缺宿主 project 身份）；
> 安全策略与规则层仍由 `build_system_prompt` 拼接，未统一进组装器；`TaskSummary` 的生产调用方属
> Task 4。未提交、未部署，详见 `docs/知识库与记忆扩展Task3验证.md`。

### Task 4: 维护 Turn 与 Dream

**Files:**
- Modify: AgentRuntime 调度、取消、预算和后台任务注册。
- Create: memory maintenance service、候选结构化解析和调度测试。

**Interfaces:**
- Consumes: Task 2/3 memory Repository and bounded task summaries。
- Produces: `run_memory_maintenance`、单实例维护 Turn、离线 TTL/去重维护。

- [x] 编写 24 小时、空闲、单实例、取消和恢复失败测试。
- [x] 实现无工具维护 Turn，模型只返回候选操作。
- [x] 实现远程 Dream 首次开启披露状态和 Token 预算。
- [x] 验证 Dream 失败不影响普通 Turn。

> Task 4 已完成：新增 `src-tauri/src/memory/maintenance.rs`（约 1,200 行含测试）承载设计 §5.2 的
> 调度与状态机：`MaintenanceOutcome`/`MaintenanceTrigger`、版本化 `MaintenanceSettings`、
> `MaintenanceGate` + `MaintenanceLease`（`Drop` 释放，任何错误路径都不会卡死门）、
> `automatic_trigger` 的「间隔 + 空闲」双条件、构造期崩溃恢复（残留 `running_since_ms` 转
> `Interrupted` 并推进 `last_run_at_ms`，不伪造成功）、`run_offline_maintenance`（先 `expire_due`
> 再 `merge_duplicate_keys`，不需要 Provider）、`build_maintenance_prompt`（全字段有界且脱敏）、
> `parse_proposals`（`deny_unknown_fields`，长度先于解析，`MEM_DREAM_INVALID_PROPOSAL` /
> `MEM_DREAM_TOO_MANY_PROPOSALS`，宿主解析 scope 与 target）。判活规则抽到
> `memory::policy::memory_is_expired` 并让 `context/assembler.rs` 复用同一份，避免「可注入」与
> 「active」分叉。`memory/service.rs` 新增 `expire_due`（按 id 分页扫描，避开自身改写
> `updated_at_ms` 导致的跳行）、`merge_duplicate_keys`、`maintenance_input`、`get`；
> `storage/memory_repository.rs` 新增 4 个只读维护查询与 `DuplicateKeyGroup`。接线：
> `AppState` 增加 `memory_maintenance` 服务与由 Turn 接纳驱动的 `maintenance_idle_since_ms` 空闲
> 时钟；`commands/mod.rs` 新增 5 个命令与 `run_memory_maintenance_with_publisher`（取租约 → 记录
> 开始 → 离线维护 → Dream → 记录结果 → 日志）、`spawn_memory_maintenance_scheduler`（5 秒轮询，
> 先 sleep 再判断）；Dream 复用唯一 `AgentRuntime` 且工具注册表为空（`AllowRegisteredTools` 下
> 没有注册就没有可授权的工具），取消用租约令牌桥接到 Turn 令牌；`lib.rs` 注册命令并启动调度
> 循环；前端 `src/types/runtime.ts`、`src/api/runtime.ts` 同步。新增 32 项回归；`memory::` 93 项、
> `commands::tests::` 28 项、`app_state::` 30 项全绿；`tsc && vite build`、`cargo fmt --check`、
> `cargo check --all-targets` 通过，全量 `cargo test --lib` 744 通过 / 16 失败（失败集合与
> Task 1/2/3 基线逐项一致）。已知边界：未做原生 `pnpm tauri dev` 验收（含 Dream 的真实 Provider
> 往返）；`TaskSummary` 仍未接入维护提示词（参数已就位，生产调用方传空切片）；`project`/
> `workspace` scope 记忆仍不自动注入；离线扫描达上限只是停下，无「还有剩余」反馈。未提交、
> 未部署，详见 `docs/知识库与记忆扩展Task4验证.md`。

### Task 5: 检索增强与反馈

**Files:**
- Modify: `KnowledgeService`、Retriever、citation snapshot 和设置页。
- Create: retrieval scoring、rewrite、parent-child expansion tests。

**Interfaces:**
- Consumes: 现有 FTS/embedding 检索、Task 1 feedback 表、Task 3 context budget。
- Produces: 多路召回、固定初始权重、父子 chunk 扩展和 `knowledge_feedback` IPC/UI。

- [x] 编写 lexical/semantic/title/path/freshness/feedback 排序测试。
- [x] 实现 deterministic query rewrite 和可选有界模型 rewrite。
- [x] 实现邻接 chunk 扩展并保持 citation 绑定 revision。
- [x] 验证 8% knowledge budget 和最多 6 个 chunk 限制。

> Task 5 已完成（2026-09-16）。`knowledge/retrieval.rs` 是设计 §6 的全部纯函数契约（六路信号与固定
> 权重、`rank_signal`/`freshness_signal`/`feedback_signal`/`title_or_symbol_signal`/`path_match_signal`、
> `query_terms`、`deterministic_rewrite`/`bound_rewrites`、`QueryRewriter`、`KnowledgeBudget`、
> `select_within_budget`），`KnowledgeService::search_with_options` 把四路召回（FTS5/BM25、title 列
> 过滤、path、semantic）接上 I/O 并完成融合、同源去重、邻接窗口与预算选择；`agent/query_rewrite.rs`
> 是可选的有界模型改写（失败即回退确定性改写）。
> 本轮收口了三处缺口：`lib.rs` 注册 `record_knowledge_feedback`/`list_knowledge_retrieval_events`；
> path 通道 `LIKE` 补 `%` 包装（`extra_recall_channels_are_additive_and_reported` 原为失败）；
> `normalize_rewrite_line` 真正丢弃围栏/blob/寒暄句（`prose_and_oversized_lines_are_dropped` 原为失败）。
> 前端新增 `KnowledgeFeedbackType`/`KnowledgeFeedbackRecord`/`KnowledgeRetrievalEventRecord` 与两个包装
> 函数，`searchKnowledge` 透传 `modelRewrite`；知识库设置页新增「检索控制台」（检索、元数据与信号展示、
> 引用展开、四类反馈、最近检索事件）。新增 1 项 Rust 回归与 1 个浏览器专项
> `e2e/knowledge-retrieval-feedback.spec.ts`（双视口）钉住命令名与驼峰参数。已知边界：未做原生
> `pnpm tauri dev` 验收；`knowledge.auto_search` 仍无消费者；citation 内存表满 500 条整体清空。
> 未提交、未部署，详见 `docs/知识库与记忆扩展Task5验证.md`。

### Task 6: 实体与事实候选

**Files:**
- Create: entities service、candidate parser、关系查询工具。
- Modify: knowledge settings、审核 UI、事件投影。
- Test: source binding, conflict, expiry and query tests。

**Interfaces:**
- Consumes: knowledge citation and Task 2 candidate workflow。
- Produces: 带来源的 entity/fact candidate、审核后的关系查询。

- [x] 编写无 citation、过期 revision、冲突事实和非法关系测试。
- [x] 实现实体规范化、事实状态和来源绑定。
- [x] 增加只读关系查询工具，禁止模型直接写 active fact。
- [x] 验证来源删除后事实进入 expired。

> Task 6 已完成（2026-09-16）。新增 `src-tauri/src/entities/`（`mod`/`normalize`/`candidate`/`service`/`tools`）：
> 宿主归一化（幂等，装饰/空白/大小写折叠，词间分隔符属于身份）、宿主词表（`ENTITY_TYPES` +
> `ENTITY_STATUSES`）、`FactCandidate::from_model` 只能表达四个字符串（状态/实体 ID/chunk/revision/
> collection/时间戳/置信度结构性不可表达；所有模型提案一律 `candidate`，与置信度无关）、
> `propose_fact` 先校验形状再解析来源（`KnowledgeService::citation_source`，与 `read_knowledge_citation`
> 同一套 Turn 绑定与 active revision 复核，因此「无来源即无关系」在写入前成立）、冲突检测（同
> subject+predicate 异 object → 审核；通过新读法时旧读法转 `disputed` 而不删除；同 object 走身份归一化
> 去重）、object 只在同 collection 已有实体时才成链接、`review_fact`（唯一把模型提案变成 `active`
> 的路径，并只对仍是 `candidate` 的实体应用审核者选定的类型）、只读 `query_knowledge_relations`
> （`list_relations` 一次 join 出端点与来源路径/行区间，只返回 active 事实与 active 实体，`scope_key`
> 收口工作区边界；`subject` 与 `relations` 分开以区分「未知实体」与「已知但无关系」）。
> `knowledge.rs` 的 `delete_source`/`delete_collection` 在物理删除 revision 前收集 revision id，删除后
> 逐个发 `knowledge_facts_expired_for_revision`（Task 1 定义、本轮才有生产者），来源保留、实体不受影响、
> 重复过期幂等。`storage` 侧新增 5 个只读查询与 `KnowledgeRelationRecord`/`KnowledgeFactCandidateRecord`；
> `validate_token` 放宽到允许 ASCII 数字（带版本的宿主 token 如 `supports_http2`），仍拒绝结构字符。
> 接线：`app_state` 新增 `entities` 服务并把三处工具注册收口到 `structured_knowledge_tools()`
> （handler 与 risk 一起返回，避免工具在没有 risk 时被注册后回退到工作区策略）；
> `propose_knowledge_fact` = `Write`（审批 + 审核两道门），`query_knowledge_relations` = `Read`；
> 前端新增关系审核面板（候选通过/驳回 + 实体类型 + 只读关系查询）。同时补上 Task 2 推迟到本轮的
> **记忆页**（开关、写入策略、候选审核、按作用域查看/删除/清空，确认令牌一律宿主计算）。
> 新增 25 项回归（entities 21、端到端过期 1、词表 2、记忆页与关系审核各 1 个浏览器专项）；
> `entities::` 21 项、4 个浏览器专项双视口 8 项全绿；全量 `cargo test --lib` 813 通过 / 3 失败
> （失败集合与基线逐项一致）。已知边界：未做原生验收；关系查询只有 subject 方向；实体类型只能改一次；
> 非 ASCII 谓词被拒绝；`knowledge_facts` 缺 `source_revision_id` 索引。未提交、未部署，详见
> `docs/知识库与记忆扩展Task6验证.md`。

### Task 7: 评测、迁移和桌面验收

**Files:**
- Create: 固定知识检索评测集、回放脚本和验证文档（均在 `docs/`）。
- Modify: 路线图、相关 ADR、设置页和测试夹具。

- [x] 建立 Recall@k、MRR、引用正确率、降级率和延迟基线。
- [x] 执行全局 Rust/前端检查和全量测试。
- [ ] 启动隔离 `pnpm tauri dev`，验证记忆查看/审核/删除、检索引用、重启恢复。
- [x] 更新路线图当前位置、任务复选框和变更记录；未部署、未提交状态如实记录。

> Task 7 **部分完成**（2026-09-16）。已完成：`evals/knowledge-retrieval-baseline.json`（8 个语料文件 +
> 8 条查询 + 阈值，全是数据）+ `src-tauri/src/knowledge/evaluation.rs` 回放执行器（建临时工作区、
> 写语料、**走真实 `KnowledgeService`** 完整跑召回/融合/去重/邻接扩展/预算，每条查询用自己的 Turn 以
> 便真正执行 citation 解析与跨 Turn 规则），并暴露为 `run_knowledge_retrieval_evaluation` 命令。
> 记录基线：`recall@3=1.000 recall@5=1.000 mrr=1.000 citations=1.000 degraded=1.000 availability=1.000
> p95=6ms`。语料里放了共享词表的干扰项（`docs/retrieval-history.md`），因此 MRR 具备区分力而非空转；
> 基线刻意不带 embedding key 运行，钉住「没有凭据也必须可用」的词法底线，降级率因此恒为 1.000 而
> **可用性**才是被断言的值——hybrid 数字属于原生验收范围。全局检查全部通过（`tsc`、`vite build`、
> `cargo fmt --check`、`cargo check --all-targets` 零警告），全量 `cargo test --no-fail-fast`
> `--lib` 813 通过 / 3 失败（既有 3 项读取收敛，逐项一致）+ 集成目标 31 项全绿，4 个浏览器专项
> 双视口 8 项全绿。
>
> **未完成：隔离 `pnpm tauri dev` 原生验收。** 原因具体且可复核：(1) 仓库里没有可复用的隔离配置——
> 既有 `scripts/validate-*-native.cjs` 都假设一个已起好的隔离宿主（Vite 1459 / WebView2 CDP 9399），
> 而那个宿主是当时临时搭起、验收后清理掉的；(2) 不隔离就会写到 `D:\apps\k-coder\` 下用户正在使用的
> 应用数据，违反 `AGENTS.md` 的安装目录约束；(3) 补齐隔离后仍需新建一个 CDP 驱动脚本才能点
> 审核/删除/引用展开与重启恢复，本轮没有为它腾出预算。因此**不得**声称桌面工作流已验证。
> 缺失项、补齐步骤与其它跨轮遗留边界见 `docs/知识库与记忆扩展Task7验证.md`。未提交、未部署。
