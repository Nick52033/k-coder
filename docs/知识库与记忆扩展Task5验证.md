# 知识库与记忆扩展 Task 5 验证记录：检索增强与反馈

- 完成日期：2026-09-16
- 实施计划：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` 的 Task 5
- 权威设计：`docs/知识库与记忆扩展详细设计.md` §4.4、§6.1–§6.4、§7.1–§7.2、§10.1
- 前置：Task 1（数据库迁移与事件投影）、Task 2（基础记忆领域服务）、Task 3（上下文编排与工作记忆）、Task 4（维护 Turn 与 Dream）已完成
- 相关 ADR：`docs/adr/0051-siliconflow-embedding-hybrid-retrieval.md`

## 0. 接手时的实际状态（必须先说清楚）

Task 5 的后端在本轮接手前**已由并行工作流写入工作区，但没有收口**。接手时观察到的事实：

| 事实 | 证据 |
|---|---|
| `knowledge/retrieval.rs`、`agent/query_rewrite.rs` 是未跟踪的新文件 | `git status --short` 显示 `?? src-tauri/src/knowledge/`、`?? src-tauri/src/agent/query_rewrite.rs` |
| `knowledge.rs`、`commands/mod.rs` 已改但未提交 | 同上 `M`；`commands/mod.rs` 里 `search_knowledge`、`record_knowledge_feedback`、`list_knowledge_retrieval_events` 已存在 |
| 两个命令**未在 `lib.rs` 注册** | `lib.rs` 的 `invoke_handler` 只到 `read_knowledge_citation`，`grep feedback` 零命中；前端无法 invoke |
| 前端无任何调用点 | `src/api/runtime.ts`、`src/types/runtime.ts` 无反馈/检索事件类型与包装函数，`src/` 里零处使用 `KnowledgeSearchResult` |
| 有 1 项真实失败 | `cargo test --lib knowledge::` → 54 通过 / 1 失败，`knowledge::tests::extra_recall_channels_are_additive_and_reported` panic：`a filename-only match must be recalled by the path channel: []` |
| 另有 1 项与之同源的失败 | `agent::query_rewrite::tests::prose_and_oversized_lines_are_dropped` |

本轮的工作就是：补完 IPC 与前端调用点、修掉两处真实缺陷、跑通全部门禁并更新文档。**未改动**并行工作流已经写好的检索主流程设计（见 §2），改动集中在三处缺口（见 §1.3）。

## 1. 交付内容

### 1.1 检索纯函数层（`src-tauri/src/knowledge/retrieval.rs`，689 行）

设计 §6 的全部可复现契约集中在一个纯模块里：无 SQL、无 Provider、不读时钟（`now_ms` 由调用方传入），
因此「同样的候选 + 同样的信号必然得到同样的顺序」，且评审者能逐信号解释*为什么*某个 chunk 排在前面。

- **固定权重**：`WEIGHT_LEXICAL=0.35`、`WEIGHT_SEMANTIC=0.35`、`WEIGHT_TITLE_OR_SYMBOL=0.10`、
  `WEIGHT_PATH_MATCH=0.10`、`WEIGHT_FRESHNESS=0.05`、`WEIGHT_USER_FEEDBACK=0.05`。`RetrievalSignals::score()`
  逐信号 clamp 到 `0..=1`，非有限值（NaN/±INF）按「**没有证据**」记 0 而不是满分——一个调用方的数值
  错误不该被读成一次完美命中。没有任何入口能提交自定义权重向量，所以「因为模型要求而改变排序」
  在构造上不可能。
- **`rank_signal(Option<usize>)`**：1-based 通道名次按 `MAX_CHANNEL_CANDIDATES=24` 线性衰减。选线性而不是
  RRF，是因为加权和需要信号在整个区间上有区分度，而 RRF 刻意压缩它。
- **`freshness_signal`**：90 天半衰期。`updated_at_ms == 0` 返回中性 0.5（未知时间戳不是陈旧的证据），
  未来时间戳被 clamp 到 1.0（时钟偏斜不该产生 `>1` 的信号）。
- **`feedback_signal`**：Laplace-free 比值；无人评分取中性 0.5，所以「没被评过」不等于「被否定」。
  `negative` 折叠 `irrelevant`/`outdated`/`wrong`，因为排序只关心「用户是否否定了它」。
- **`title_or_symbol_signal`**：查询词在标题/符号上的命中比例，全命中饱和到 1.0。
- **`path_match_signal`**：路径命中比例，文件名内的命中权重高于父目录内的命中。
- **`query_terms`**：ASCII 词保留 `_`、`-`、`.`（`is_private`、`bootstrap.ps1` 靠它们可检索）；
  CJK 连续段产出 bigram，因此两字中文词仍然可检索；上限 `MAX_QUERY_TERMS=16`。
- **`deterministic_rewrite` / `bound_rewrites`**：原查询**恒为第一条**，最多再加 `MAX_REWRITTEN_QUERIES=3`
  条，单条上限 `MAX_REWRITTEN_QUERY_CHARS=120`；只折叠宿主给出的 `project_name`/`current_file`/
  `recent_entities`，所以改写不可能引入调用方原本没有的路径、权限或 scope。重复词只保留一份。
- **`QueryRewriter` trait**：定义在 knowledge 侧（不是 `agent`），这样 `knowledge` 不依赖 runtime，
  测试可以注入假实现而不需要 Provider；实现被要求「失败即返回 Err」，由调用方保留确定性改写。
- **`KnowledgeBudget`**：`percent` clamp 到 1–50，默认 `DEFAULT_KNOWLEDGE_BUDGET_PERCENT=8`，
  工作上下文默认 `DEFAULT_WORKING_CONTEXT_TOKENS=96_000`，`CHARS_PER_TOKEN=4`；
  `max_chars() = tokens * percent / 100 * 4`，`max_chunks()` 恒为 `MAX_KNOWLEDGE_CHUNKS=6`。
- **`select_within_budget`**：best-first，**跳过**放不下的候选而不是在那里停止——一个超大 chunk
  不能把它后面所有更小的候选一起挡掉。chunk 数量上限与字节预算各自独立生效（设计 §6.1 第 7 条同时
  规定了「8% 预算」和「最多 6 个 chunk」两件事）。

### 1.2 检索主流程与 citation（`src-tauri/src/knowledge.rs`）

`search_with_options(workspace, thread_id, turn_id, query, limit, options)` 是唯一实现（`search()` 只是
`SearchOptions::default()` 的薄包装）：

1. **改写**先跑 `deterministic_rewrite`；只有在 `options.rewriter` 存在时才多一次有界模型改写，
   合并结果再过 `bound_rewrites`，模型失败只写 `knowledge_query_rewrite_failed` 并保留确定性列表。
2. **四路召回**：lexical（`MATCH_RECALL_SQL`，FTS5/BM25）、title/symbol（同一个 FTS 索引上的
   `{title} : "term"` 列过滤，OR 连接）、path（`PATH_RECALL_PREFIX`）与 semantic（仅当
   `semantic_enabled && embedding_configured`，`active=1 AND embedding_status='semantic_ready'` 且按固定
   provider/model 过滤）。lexical 是 floor，失败即整体失败；其余三路失败只记
   `knowledge_recall_channel_failed` 并降级 lexical-only。每路上限 24 个候选，同一个 chunk 被多路/多次
   改写召回时只保留该通道内的**最好**名次（加改写只能扩大候选集，不能把原本排得好的 chunk 压下去）。
3. **融合排序**：按 `score()` 降序，同分依次用 lexical 名次、path、ordinal、chunk_id 决胜（稳定、可复现）。
   `user_feedback` 来自 `KnowledgeEntityRepository::feedback_totals_for_chunks` 的按 chunk 聚合。
4. **同源去重**：一个来源只占一个槽位（`MAX_KNOWLEDGE_CHUNKS=6`），避免一个长文件把六个槽位全填满。
5. **邻接窗口**：按命中 chunk 的 revision 读回 `REVISION_WINDOW_SQL` 的 `ordinal-1..=ordinal+1`，
   由 `citation_window` 组装成「命中 + 上方连续 heading 块 + 下方一块」。嵌套小节会连续回溯整段
   heading-only chunk；窗口**始终限定在命中自身的 revision 内**，因此 citation 永远不会混两个版本。
6. **预算选择**：`KnowledgeBudget::new(budget_percent ?? 设置, working_context_tokens)`，
   `select_within_budget` 决定返回集合，再受 `limit` 约束。
7. **事实与遥测**：生成 opaque `citation_id`（UUID）写入进程内 `citations` 表（含 `thread_id`/`turn_id`/
   `chunk_id`/`included_start`/`included_end`/行区间），并追加一条
   `KnowledgeRetrievalEventRecord`（`query_hash` 是摘要，**不持久化 query 原文**）；写事件失败只记
   `knowledge_retrieval_event_failed`，绝不把可用的回答变成错误。`knowledge_search_completed` 日志只含
   摘要、模式、通道名、计数、预算和耗时。

`read_citation(thread_id, turn_id, citation_id, before, after)`：跨线程/跨 Turn 返回
`KC_CITATION_FORBIDDEN` 并写 `knowledge_citation_rejected`；随后重查「chunk 属于当前 active revision
且 collection enabled 且 source 非 deleting」，查不到返回 `KC_CITATION_STALE`；`before`/`after` 只在
已有窗口**之外**继续扩一块（不重复已给过的 chunk），总大小受 `MAX_CITATION_WINDOW_BYTES=16 KiB` 约束，
且始终限定在该 citation 自己的 revision 内。

`record_feedback(thread_id, turn_id, citation_id, feedback_type)`：校验 `FEEDBACK_TYPES`
（`useful`/`irrelevant`/`outdated`/`wrong`）、校验 citation 属于该 Turn，然后把评分绑定
`chunk_id = 该 citation 的 chunk` 与 `source_revision_id = 该 citation 的 revision`，追加为事实并投影到
`knowledge_feedback`。这一点很重要：citation 表是**进程内**的，重启即失效，而评分是持久事实，
所以「用户评过什么」跨重启仍然参与排序。

`list_retrieval_events(thread_id, limit)`：单线程的检索遥测，最新在前，只含摘要。

### 1.3 本轮修掉的三处缺口

**(1) IPC 未注册（功能性缺陷）**

`commands::record_knowledge_feedback` 与 `commands::list_knowledge_retrieval_events` 已实现且有
`#[tauri::command(rename_all = "camelCase")]`，但 `lib.rs` 的 `invoke_handler` 里没有它们。前端
`invoke` 会直接失败。已在 `read_knowledge_citation` 之后注册。

**(2) path 召回通道的 `LIKE` 缺少通配符（真实缺陷，有失败测试）**

原实现（`recall_paths`）：

```sql
replace(lower(s.relative_path),'\','/') LIKE ?2   -- 参数是裸词，如 'persistence'
```

`relative_path` 存的是 `persistence.md`，所以「内容里没有该词、只有文件名命中」的查询**永远**召回
不到——而这恰恰是设计 §6.1 第 4 条「精确文件名作为额外召回通道」存在的理由。
`knowledge::tests::extra_recall_channels_are_additive_and_reported` 因此失败
（`a filename-only match must be recalled by the path channel: []`）。修复：

```sql
replace(lower(s.relative_path),'\','/') LIKE '%'||?2||'%'
```

同时把 `MIN_PATH_TERM_CHARS` 的注释从「只匹配整段路径」改成实话：这是**子串**匹配，
2 字符下限只是防止「一个字符召回几乎所有来源」。注意 `_` 在 SQLite `LIKE` 中本身是单字符通配，
所以 `memory_repository` 在路径通道里理论上也会匹配 `memoryXrepository`——召回通道内可接受
（`LIMIT 24` 有界，排序负责排序），已记入已知边界而不是为它加转义。

**(3) 模型改写的「回复过滤」只在注释里（真实缺陷，有失败测试）**

`parse_rewrites` 的文档注释承诺「不按格式走的回复（prose、代码块、JSON blob）会被过滤成看起来像查询的
那些行」，但实现只做了「去编号/圆点/引号 + 长度上限 + 去重 + 取前 3 条」。于是
`Sure! Here are some queries:`、```` ```text ````、```` ``` ```` 都会被当成查询，
`agent::query_rewrite::tests::prose_and_oversized_lines_are_dropped` 因此失败（期望
`["retrieval weights"]`，实际前三条被寒暄句和围栏占满）。

修复为新增 `normalize_rewrite_line`：

- 以 ` ``` ` 开头的行是围栏 → 丢弃（围栏是包裹答案的标记，不是答案）；
- 内联反引号是装饰 → **剥离**而不是丢弃内容（`` `budget percent` `` → `budget percent`）；
- 以 `{` 或 `[` 开头的行是结构化 blob → 丢弃（提示词明确禁止的回复形状）；
- 以 `:` 或 `：` 结尾的行是「以下是查询：」这类告知句 → 丢弃；
- 其余仍按原规则做长度上限与去重。

并补了 1 项回归 `fences_blobs_and_announcements_are_not_queries`。

### 1.4 前端契约与「检索控制台」

- `src/types/runtime.ts`：新增 `KnowledgeFeedbackType`（四值联合）、`KnowledgeFeedbackRecord`
  （`chunkId`/`sourceRevisionId` 可空，对应 schema v10 之前的历史行）、
  `KnowledgeRetrievalEventRecord`。
- `src/api/runtime.ts`：新增 `recordKnowledgeFeedback(citationId, feedbackType, threadId, turnId)`、
  `listKnowledgeRetrievalEvents(threadId, limit)`；`searchKnowledge` 增加第 5 个参数 `modelRewrite`。
- `src/components/SettingsDialog.tsx` 的知识库页新增「检索控制台」（`KnowledgeRetrievalPanel`）：
  - 关键词输入 + 「模型改写」开关 + 检索按钮（知识库停用时按钮禁用并给出说明）；
  - 融合元数据：模式、参与通道、改写条数、预算字符数、降级码；
  - 每条结果：标题、`路径 · 行区间 · rev`、`score`（三位小数）、lexical/semantic 名次、预览；
  - 「展开引用」按钮走真实 `read_knowledge_citation`，展开后显示带 revision 的完整窗口，并明确标注
    「仍是当前版本」/「已不是当前版本」；
  - 四类反馈按钮（有用/不相关/已过时/有错误），用 `aria-pressed` 反映已选状态；
  - 「最近检索事件」列表：`queryHash`（不透明摘要）、模式、候选数、引用数、耗时、时间。
- 检索与反馈**共用宿主生成的 `settings` 伪 Turn**（`threadId = turnId = "settings"`）：citation 只对
  「返回它的那一轮」有效，而设置页没有真实 Turn；共用同一个伪 Turn 才能让反馈有 citation 可绑定，
  否则必然是 `KC_CITATION_FORBIDDEN`。这条约束写在组件顶部的注释里。
- 样式复用既有 `knowledge-*` 设计令牌（`--color-brand`、`--color-ink*`、`--color-border*`、
  `--radius-*`、`--font-family-mono`、`--transition-fast`），并补 760px 断点（表单改单列、
  反馈按钮铺满、事件行改单列）。

## 2. 关键设计决策（本轮确认而非新造）

### 2.1 排序契约放在 `knowledge/retrieval.rs`，不放进 `KnowledgeService`

纯函数层可复现、可逐信号测试，且 `knowledge` 不需要依赖 runtime（`QueryRewriter` trait 定义在
knowledge 侧）。`search_with_options` 只负责 I/O 与装配。

### 2.2 改写永远只可能「加宽」而不能「收窄」

改写列表由 `bound_rewrites` 收口，原查询恒在首位；每条召回都走与原始 query 完全相同的 scope
（`c.scope_key=?1`）与可见性过滤（`r.active=1 AND c.enabled=1 AND c.deleted=0`）。改写里没有任何字段
能到达工作区、endpoint 或 model 名，所以「模型改写扩大读取范围」在参数层面不可能。

### 2.3 非 lexical 通道失败一律降级而不是失败

设计 §6.1 第 3 条与 §12 第 3 条要求「没有 embedding 凭据或远程失败时普通对话仍可用」。
因此 lexical 是唯一的 floor，title/path/semantic 任一路出错只记 `knowledge_recall_channel_failed`
（含通道名与错误码），检索照常返回 lexical 结果并把 `fallbackCode` 报给前端。

### 2.4 同源去重 + 邻接扩展，各自解决一个具体问题

去重解决「一个长文件填满六个槽位」（`one_source_never_fills_every_result_slot`）；
邻接扩展解决「上下文自己不可读」（`citations_carry_the_heading_and_one_neighbour_from_the_same_revision`）。
两者都以 revision 为边界，所以 citation 永远能对应到一个稳定版本。

### 2.5 反馈绑定 chunk + revision，而不是 citation

citation 是进程内的、可被 `MAX_CITATIONS` 淘汰的、重启即失效的；评分是持久事实。绑定
`(chunk_id, source_revision_id)` 才能让「用户评过什么」跨重启继续影响排序，
同时 `KnowledgeFeedbackRecord::validate` 强制两者**同给同缺**，避免出现「有 chunk 没 revision」的
半归属行。

### 2.6 未做原生验收，就不说桌面工作流完成

本轮改动包含设置页 UI，但**没有启动 `pnpm tauri dev`**，也没有在桌面窗口里点击过检索控制台。
可用的替代验证是浏览器级专项（见 §3.3）：它用真实 Chromium 渲染真实组件，并同时钉住命令名、参数名
与返回字段名，能挡住「命令漏注册」「参数拼错」「字段改名」这类接缝错误；但它**不能**替代真实 IPC
往返与真实 Provider 的语义召回往返。

## 3. 测试

### 3.1 本轮新增

| 文件 | 数量 | 覆盖 |
|---|---|---|
| `agent/query_rewrite.rs`（新增 1 项） | 1 | `fences_blobs_and_announcements_are_not_queries`：围栏/JSON blob/告知句被丢弃，内联反引号被剥离而内容保留 |
| `e2e/knowledge-retrieval-feedback.spec.ts`（新增） | 2（desktop+narrow） | 检索控制台渲染、挂载时读检索事件、检索参数（含 `modelRewrite`）、结果与元数据逐项渲染、引用展开、反馈下发与选中态、700px 无横向溢出 |

### 3.2 专项结果（本轮测得时刻 2026-09-16 14:0x–14:2x）

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib knowledge
cargo test --manifest-path src-tauri/Cargo.toml --lib query_rewrite
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml
```

- `knowledge::` **43 通过 / 0 失败**（含 `knowledge/retrieval.rs` 的 11 项纯函数测试）。
  修复前后唯一差别是 `extra_recall_channels_are_additive_and_reported` 由失败转通过。
- `agent::query_rewrite::tests::` **5 通过 / 0 失败**（修复前 4 通过 / 1 失败）。
- 全量 `--lib`：**788 通过 / 3 失败**（见 §3.4）。
- `cargo test`（含集成目标）**31 项全绿**：`mobile_dto_contract` 6、`mobile_gateway` 19、
  `mobile_turn_lifecycle` 6。

### 3.3 浏览器专项

```powershell
pnpm exec playwright test e2e/knowledge-index-progress.spec.ts e2e/knowledge-retrieval-feedback.spec.ts --reporter=line
```

`4 passed`（两个 spec × desktop/narrow）。新 spec 的关键断言：

- `search_knowledge` 收到的参数**逐字段**匹配 `{ query: "schema 迁移", limit: 6, threadId: "settings",
  turnId: "settings", modelRewrite: true }`；
- `record_knowledge_feedback` 收到 `{ citationId: "citation-1", feedbackType: "useful",
  threadId: "settings", turnId: "settings" }`，且按钮 `aria-pressed` 变为 `true`；
- `list_knowledge_retrieval_events` 的返回被按驼峰字段正确渲染（`queryHash`、候选 4、引用 1、12 ms）；
- 700px 视口下 `scrollWidth <= clientWidth + 1`。

本轮同时修正了既有 `e2e/knowledge-index-progress.spec.ts`：它的桩对未知命令返回 `null`，而知识库页
现在挂载时会读一次检索事件，导致整个设置页渲染中断。已为该命令补上 `return [];`。
（这也说明了一个真实约束：设置页新增任何挂载期 IPC 调用，既有桩都必须同步。）

### 3.4 失败基线的诚实说明

本轮全量 `--lib` 的 3 项失败是**文档化的既有失败**（Task 1–4 记录的「3 项既有读取收敛」）：

- `agent::tests::read_recovery_delivery_only_hard_stops_the_corrected_provider_batch`
- `agent::tests::recovery_still_stops_varied_overlapping_reads_after_one_correction`
- `agent::tests::semantic_read_tracker_recovers_once_before_stopping_overlap_loops`

Task 1–4 基线里另有 13 项「本机符号链接创建异常」失败，**本轮未复现**。相关源码
（`tools/mod.rs`、`patch/mod.rs`、`extensions/plugins.rs`、`extensions/mod.rs`）都在本轮改动范围之外、
且 `git status` 显示未被修改，所以这只能解释为环境差异（创建符号链接的权限），而不是被谁修好了。
本记录不复述「13 项失败仍然存在」。

## 4. 质量门槛

| 命令 | 结果 |
|---|---|
| `npx tsc --noEmit` | 通过（零错误） |
| `npx vite build` | 通过（`✓ built in 11.00s`） |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过（先 `cargo fmt` 修掉 6 处并行工作流遗留差异） |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml --no-fail-fast` | `--lib` 788 通过 / 3 失败（既有 3 项）；集成目标 31/31 通过 |
| `pnpm exec playwright test e2e/knowledge-*.spec.ts` | 4 通过（desktop + narrow） |

说明：`npx vite build` 被本机命令包装判成 watch 命令，因此把输出重定向到文件后再读，构建本身成功
（`✓ built in 11.00s`）且 `dist/` 有产物。

## 5. 未完成与已知问题

- **未做原生 `pnpm tauri dev` 验收**。设置页的检索控制台只在 Chromium（Playwright）里验证过渲染与
  下发参数，**没有**在桌面窗口里实际点击过；真实 IPC 往返、真实 Provider 的语义召回往返、
  `read_knowledge_citation` 在真实数据上的加宽行为都未验证。不得声称桌面工作流已完成。
- **`knowledge.auto_search` 仍无消费者**。该设置项被 `settings()` 读出、也已出现在设置结构体里，
  但全仓没有第二处引用：检索目前只在模型工具（`search_knowledge`）与设置页显式触发，
  普通 Turn 不会自动检索。这是 Task 5 之前的既有缺口，本轮未越界实现（设计 §6 没有规定自动触发时机，
  擅自接线等于发明契约）。
- **citation 表是进程内的，且淘汰策略粗糙**。满 `MAX_CITATIONS=500` 时整体 `clear()`，而不是按 Turn
  或者 TTL 回收。后果是极端情况下一个长会话会把更早 Turn 的 citation 一起清掉，此后对该 citation
  的反馈会得到 `KC_CITATION_FORBIDDEN`。评分本身仍然持久（绑定 chunk+revision），只有「展开引用」
  与「给旧 citation 评分」受影响。
- **path 通道是子串匹配**。`_` 在 SQLite `LIKE` 中本身是单字符通配，所以 `a_b` 也会匹配 `aXb`
  （极小概率的真实路径）。`LIMIT 24` 有界且排序负责排序，判断为可接受；若要精确，需要加
  `ESCAPE` 并转义 `_`。
- **`project` / `workspace` scope 记忆仍不自动注入**（Task 3 遗留）；**`TaskSummary` 仍未接入维护
  提示词**（Task 4 遗留）；这两项都按计划留给后续任务。
- **`knowledge/retrieval.rs` 的 `title_rank` / `path_rank` 字段目前在排序里没有被消费**：title 与 path
  两个信号走的是「查询词与标题/路径的字符串重合度」（`title_or_symbol_signal` / `path_match_signal`），
  而不是「通道内名次」。字段本身仍被 `merge_channel` 用于保留该通道的最好名次，因此不是死代码，
  但确实没有参与打分。保留原样（改它等于改检索主流程的语义，超出本轮收口范围），在此记录以免被
  误读为「设计如此」。
- 未提交、未部署。

## 6. 工作区状态

- 新增：`e2e/knowledge-retrieval-feedback.spec.ts`、`docs/知识库与记忆扩展Task5验证.md`。
- 修改：`src-tauri/src/lib.rs`（注册 2 个命令）、`src-tauri/src/knowledge.rs`（path 通道 `LIKE` 加
  `%` 包装 + 常量注释）、`src-tauri/src/agent/query_rewrite.rs`（`normalize_rewrite_line` + 1 项回归）、
  `src/types/runtime.ts`、`src/api/runtime.ts`、`src/components/SettingsDialog.tsx`（检索控制台）、
  `src/App.css`（`knowledge-retrieval-*`/`knowledge-result-*`/`knowledge-feedback-*`/`knowledge-event-*`
  样式与 760px 断点）、`e2e/knowledge-index-progress.spec.ts`（桩补一条命令）、
  `docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md`、`docs/开发路线图.md`。
- 接手时已在工作区、本轮未改其设计：`src-tauri/src/knowledge/retrieval.rs`、
  `src-tauri/src/knowledge.rs` 的 `search_with_options`/`recall_paths`/`citation_window`/`read_citation`/
  `record_feedback`/`list_retrieval_events`、`src-tauri/src/commands/mod.rs` 的三个知识库命令、
  `src-tauri/src/storage/knowledge_entity_repository.rs` 的反馈聚合。
- 工作区是移动靶：验证期间存在并行工作流（`src/App.tsx`、`e2e/workbench.spec.ts`、
  `src/components/MobileSettingsPage.tsx` 等），与本次改动无关。上述质量门槛结论只对应本记录标注的
  时刻（2026-09-16 14:0x–14:2x）。
- 未执行 `git commit` / `git push`。
