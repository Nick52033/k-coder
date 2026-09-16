# 知识库与记忆扩展 Task 6 验证记录：实体与事实候选

- 完成日期：2026-09-16
- 实施计划：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` 的 Task 6
- 权威设计：`docs/知识库与记忆扩展详细设计.md` §3.1、§4.1、§7.2、§10.1
- 前置：Task 1（数据库迁移与事件投影）、Task 2（基础记忆领域服务）、Task 3–5 已完成

## 1. 交付内容

### 1.1 新增 `src-tauri/src/entities/`（设计 §3.1 的 `entities` 模块）

设计给这个模块的职责与禁令是：「实体、事实、关系候选及来源绑定 | **不接受无来源关系**」。两半都是结构性的，不是约定：

- **无来源即无关系。** 提案必须带 `citationId`，而宿主通过 `KnowledgeService::citation_source` 解析它——和 `read_knowledge_citation` 用的是同一套 Turn 绑定与 active revision 复核。模型不能指定 chunk、revision 或 collection，也不能引用本轮从未拿到过的 citation；解析不出来就没有事实。而且顺序是刻意的：先校验形状（谓词、实体名），**再**解析来源，所以「无 citation」的拒绝发生在任何一行写入之前，一次坏提案不会留下半个图。
- **模型永远写不出 `active` 事实。** `FactCandidate::from_model` 只接受四个字符串（subject / predicate / object / citationId），表达不了状态、实体 ID、chunk、revision、时间戳或置信度；服务把**所有**模型提案都记成 `candidate`，与置信度无关（设计与记忆不同，对事实是明确写死的「禁止模型直接写 active fact」，而且一个错的事实比一个错的偏好更伤）。`active` 只有两条路：`review_fact`（人的决定）或 `FactCandidate::from_user`（用户本人就是那个权威）。

| 文件 | 内容 |
|---|---|
| `entities/mod.rs` | 模块职责说明、`EntityError`（`ENT_*` 码）、`KnowledgeError → EntityError` 保留 `KC_*` 码的转换 |
| `entities/normalize.rs` | 身份归一化与宿主词表 |
| `entities/candidate.rs` | 候选草稿、审核决定、冲突种类、`FactOutcome` |
| `entities/service.rs` | `EntityService`：提案、审核、列表、只读关系查询 |
| `entities/tools.rs` | `propose_knowledge_fact`（Write）与 `query_knowledge_relations`（Read） |

**身份归一化**（`normalize_entity_name`）：去首尾空白与引号/书名号装饰、空白折叠为一个空格、Unicode 小写。确定性且幂等（`normalize(normalize(x)) == normalize(x)`，有测试钉住），所以 `find_entity_by_normalized_name` 是一次完备查找而不是启发式。词间分隔符**属于身份**：`k-coder` 与 `kcoder` 是两个实体。

**谓词归一化**（`normalize_predicate`）：把空白、标点、路径分隔符统一成一个下划线（`depends on` / `depends-on` / `uses::v3` 都归一），拒绝非 ASCII 谓词（中文谓词被明确拒绝，而不是被悄悄转写）。

**宿主词表**：`ENTITY_TYPES`（concept/module/symbol/file/api/config/service/technology）与 `ENTITY_STATUSES`（candidate/active/rejected）。模型提案的实体一律是 `concept` + `candidate`；只有审核者能从词表里指定类型，而且只对**仍是 candidate** 的实体生效。

### 1.2 事实状态机与来源绑定

- 事实状态：`candidate` / `active` / `disputed` / `expired` / `rejected`（Task 1 已建表，本轮才有人写）。
- **冲突**：同一 subject+predicate 已有 `active` 且 object 不同 → 新提案进审核队列；审核通过时把原来那条转成 `disputed`。**永不删除**：那条事实（以及它的 citation）曾经是真的，属于审计轨迹。object 相同则去重（`Deduplicated`，不写任何一行），比较走身份归一化，所以 `Store` 与 `store` 是同一个 object。
- **object 何时成为链接**：只有当该归一化名字在同一个 collection 里**已经**存在实体时才写 `object_entity_id`，否则写 `object_text`。这样模型不能借着提案顺手发明第二个实体；一个名字在别处成为 subject 之后自然开始成链。
- **来源删除**：`KnowledgeService::delete_source` / `delete_collection` 在物理删除 revision 之前先收集 revision id，删除后按 `knowledge_facts_expired_for_revision` 逐条转 `expired`（Task 1 定义的事件，本轮才有生产者）。`MAX_PURGED_REVISIONS_FOR_EXPIRY=512` 保证一次清理的主键列表有界，超出只记警告而不是无界展开。
- **只读关系查询**：`list_relations` 一次 join 出 subject 名、object 名/文本、来源路径与行区间，且只返回 `active` 事实、只返回 `active` 实体，并以 `c.deleted=0 AND c.enabled=1 AND c.scope_key=?` 收口工作区边界。没有来源可读时 `source_path` 为 `None`，而不是编一个路径。
- `RelationQueryResult` 把 `subject` 与 `relations` 分开：**「这个实体未知」与「这个实体已知但没有关系」是两个不同的答案**，合并它们会让调用方把「还没成链」误读成「不存在」。

### 1.3 `storage` 侧新增只读查询（`knowledge_entity_repository.rs`）

`find_entity_by_normalized_name`、`list_entities_by_normalized_name`（按 scope_key）、`list_active_facts_for_subject_predicate`、`list_facts_by_status_for_collection`、`list_relations`，以及 `KnowledgeRelationRecord` / `KnowledgeFactCandidateRecord`。全部只读、全部带 `LIMIT`，`entities/` 侧零 SQL（与 `memory/` 同一约束）。

join 查询单独定义 `ENTITY_COLUMNS_JOINED`，因为 `knowledge_collections` 也有 `id`/`name`/`created_at_ms`/`updated_at_ms`，不限定表名会歧义。

### 1.4 词表放宽（`storage/event_validation.rs`）

`validate_token` 现在允许 ASCII 数字。理由是宿主自有的 token 词表本来就包含带版本的名字（`supports_http2`、`v3`），而原来的规则会让这类谓词要么被拒绝、要么被转写成语义不同的词。仍然拒绝大小写、空白、标点和路径分隔符，所以「值里夹带结构」这条路没被打开。这是**放宽**（既有值全部仍然合法），并新增了 2 项测试钉住两侧。

### 1.5 接线

- `lib.rs`：`pub mod entities;`，并在 `invoke_handler` 注册 4 个命令。
- `commands/mod.rs`：`entities_command_error` + `list_knowledge_entities`、`list_knowledge_facts`、`review_knowledge_fact`、`query_knowledge_relations`。
- `app_state.rs`：新增 `entities` 服务（与 `knowledge` 共用同一个 projection 与 citation 表）与 `entities()` 访问器；**三处**工具注册（构造、切换工作区、`base_tool_registry`）统一收口到 `structured_knowledge_tools()`，handler 与 risk 一起返回——分开写会让一个工具在没有 risk 的情况下被注册，而 `ToolRegistry::authorization` 对未知名字回退到工作区策略，把只读查询变成需要审批的工具。
- 工具风险：`propose_knowledge_fact` = `Write`（它确实往知识库写一行持久化记录，所以和其它写工具一样需要审批；审核队列是**之后**的第二道门，决定它是否*为真*），`query_knowledge_relations` = `Read`。
- 前端：`src/types/runtime.ts`（实体/事实/关系类型）、`src/api/runtime.ts`（4 个包装函数）、`src/components/SettingsDialog.tsx` 新增「关系审核」面板（候选通过/驳回 + 实体类型选择 + 只读关系查询）与配套样式。

### 1.6 记忆页（Task 2 推迟到本轮的界面）

Task 2 的验证记录写明「记忆列表/审核/删除确认 UI 与 Task 6 实体审核一并排期」，Task 6 的计划也把「审核 UI」列在 Files 里，因此本轮一并交付：

- 新增 `MemoryPage`（设置侧栏「记忆」，与知识库同组）与其浏览器专项回归。
- 开关 `enabled`、写入策略（`autoAcceptHighConfidence` + 默认 TTL，一个保存按钮、一次载荷）、待审核候选的接受/拒绝、按作用域查看生效记忆、逐条删除与清空作用域。
- 作用域选择器只提供宿主能算出规范串的四种：`user` / `workspace:<id>` / `project:<id>` / `thread:<活动会话>`；**没有活动会话时会话级直接禁用**，而不是拿一个猜出来的 ID 去查。这个应用里一个工作区就是一个 `ProjectRecord`，所以工作区级与项目级共用同一个宿主 ID（代码注释里写明）。
- 删除/清空的确认令牌由宿主计算（记忆 ID 与规范 scope 串），前端只回传；因此不存在「构造一个参数就删掉一条没看见的记忆」的路径。

## 2. 测试

新增 **25** 项回归：

| 文件 | 数量 | 覆盖 |
|---|---|---|
| `entities/normalize.rs` | 5 | 归一化确定性与幂等、装饰/大小写折叠到同一身份、空名与纯装饰名无身份、谓词折叠与拒绝非 ASCII、词表限制 |
| `entities/candidate.rs` | 4 | 模型候选表达不了状态/身份/来源、用户候选仍需 citation、空/超长/非有限值拒绝、决定解析 |
| `entities/service.rs` | 12 | 来源无法解析时拒绝且零写入、跨 Turn citation 拒绝（同一 citation 在本轮可用）、revision 被替换后 `KC_CITATION_STALE`、非法关系（非 ASCII 谓词 / 纯装饰名）在解析来源之前被拒、模型提案停在 candidate 且实体对关系查询不可见、通过后实体与事实一起生效并带来源路径与行区间、驳回不出现在图里、重复提案去重、冲突读法在通过新读法后转 `disputed`、同一事实不能审核两次、未知实体与无关系是不同答案 |
| `knowledge.rs` | 1 | 端到端：索引 → 写入 active 事实 → `delete_source` → 事实变 `expired` 且来源 revision 保留、实体不受影响、重复过期幂等 |
| `storage/event_validation.rs` | 2 | token 接受数字但拒绝结构、bounded/confidence/hash 校验 |
| `e2e/knowledge-retrieval-feedback.spec.ts` | 2（含既有 1 项） | 关系审核面板：候选渲染、`review_knowledge_fact` 的 `factId`/`decision`/`entityType`、只读 `query_knowledge_relations` 的 `name`/`limit`、驳回不带 `entityType`、700px 无横向溢出（desktop+narrow） |
| `e2e/memory-settings.spec.ts`（新增） | 2 | 记忆页：候选审核、生效记忆渲染、删除确认令牌 = 记忆 ID、`set_memory_settings` 载荷、`list_memories` 用规范 scope 串、700px 无横向溢出（desktop+narrow） |

专项命令：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib entities::
cargo test --manifest-path src-tauri/Cargo.toml --lib deleting_a_source_expires
pnpm exec playwright test e2e/knowledge-*.spec.ts e2e/memory-settings.spec.ts --reporter=line
```

`entities::` 21 项全绿；端到端过期 1 项通过；4 个浏览器专项双视口 8 项全绿。

## 3. 已知边界

- **未做原生 `pnpm tauri dev` 验收**。关系审核面板与记忆页只在 Chromium（Playwright，桩 IPC）里验证过，**没有**在桌面窗口里对真实后端点过；`propose_knowledge_fact` 作为模型工具的审批往返同样未跑过。不得声称桌面工作流已完成。
- **`list_relations` 只用 `NORMALIZED_NAME` 索引命中 subject 方向**。object 方向（「谁依赖 X」）没有查询入口，反向关系需要按 `object_entity_id` 建索引与查询，本轮未做。
- **实体类型只能改一次**。审核时类型只对仍是 `candidate` 的实体生效；一旦生效就没有「改类型」的界面或命令。
- **事实没有 `source_turn_id` 列**。事实的来源是 `chunk_id + revision_id`，所以「这条事实是哪一轮提出的」只能从事实日志的事件顺序推断，不是可查询字段。
- **`knowledge_facts` 没有 `source_revision_id` 索引**。设计 §9.2 列的是 `source_chunk_id`，因此「按 revision 过期/列举」是一次全表扫描。事实量小的时候无感，量大了需要一次 schema 迁移。
- **非 ASCII 谓词被拒绝**。中文谓词拿不到 `ENT_INVALID_RELATION` 之外的帮助；要支持就得改存储词表或引入编码方案，本轮明确不做。
- 未提交、未部署。

## 4. 工作区状态

- 新增：`src-tauri/src/entities/{mod,normalize,candidate,service,tools}.rs`、`e2e/memory-settings.spec.ts`、`docs/知识库与记忆扩展Task6验证.md`。
- 修改：`src-tauri/src/storage/knowledge_entity_repository.rs`、`src-tauri/src/storage/event_validation.rs`、`src-tauri/src/knowledge.rs`、`src-tauri/src/app_state.rs`、`src-tauri/src/commands/mod.rs`、`src-tauri/src/lib.rs`、`src/types/runtime.ts`、`src/api/runtime.ts`、`src/components/SettingsDialog.tsx`、`src/App.css`、`e2e/knowledge-index-progress.spec.ts`（桩补三个命令）、`e2e/knowledge-retrieval-feedback.spec.ts`。
- 未执行 `git commit` / `git push`。
