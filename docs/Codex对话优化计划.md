# Codex 对话优化计划

本文档负责回答 k-Coder 对话运行时“当前做到哪里、下一步做什么、何时算完成”。总体开发顺序仍以 `docs/开发路线图.md` 为唯一事实来源；本计划是 Codex 对话专项的执行清单，任务状态必须与路线图同步。

## 目标

- 学习并采用 Codex 的 `Thread -> Turn -> Item` 领域模型，而不是复制终端界面。
- 让实时事件、JSONL 恢复、前端投影和最终消息共享同一身份与生命周期。
- 把阻塞式 Turn 逐步拆为可启动、可引导、可精确中断、可恢复的线程运行时。
- 在每一步保持旧会话可恢复，不跨越现有授权、工作区、工具和存储边界。

## 当前状态

| 项目 | 当前值 |
| --- | --- |
| 当前阶段 | 阶段 7：运行时正确性、吞吐与可维护性实施中 |
| 当前任务 | `P10-077` 沿职责边界拆分超大模块（本轮完成 storage 历史分页切片） |
| 后续任务 | `P10-078` Monaco 包体治理 |
| 已完成基础 | 稳定 Item 身份与生命周期、统一历史分页、异步 `turn_start`/`turn_retry`、Rust Thread mailbox、精确 steer/interrupt、Thread fork/resume/rollback、队列消息原子 steer |
| 候选优化 | Thread 操作门、按线程持久化 writer、增量历史索引、事件驱动 mailbox 同步、结构化错误、模块拆分和 Monaco 体积治理 |

## 执行顺序

| 顺序 | 路线图任务 | 工作项 | 状态 | 验收出口 |
| --- | --- | --- | --- | --- |
| 1 | `P10-060` | 助手正文、工具前说明和最终消息共享稳定 Item ID | 已完成 | 实时与恢复投影使用同一 ID，旧事件兼容 |
| 2 | `P10-061` | 助手 `item_started`/`item_completed` 生命周期 | 已完成 | 成功、失败、取消都能闭合已启动的助手 Item |
| 3 | `P10-062` | Reasoning Item 生命周期 | 已完成 | 首个安全摘要增量或直接完成启动 Item，完成事件去重并先落盘再闭合；失败/取消不留下悬空 Item |
| 4 | `P10-063` | Tool Item 生命周期 | 已完成 | Tool call、输出增量、结果共享 Item ID；恢复能区分 pending/running/completed/failed/cancelled |
| 5 | `P10-064` | Approval、Change、UserInput、Compaction Item 生命周期 | 已完成 | 审批和变更按关联 ID 严格排序，所有公共 Item 都有确定性身份 |
| 6 | `P10-065` | 统一 Thread Item 历史投影与分页读取 | 已完成 | 前端不再从多套快照猜测时间线；支持有界 turns/items 分页 |
| 7 | `P10-066` | 异步 `turn_start` 和 turn handle | 已完成 | 启动命令立即返回 `turnId`，结果只通过事件和查询获取，阻塞兼容入口可迁移 |
| 8 | `P10-067` | 后端 Thread mailbox | 已完成 | 每线程单活动 Turn、FIFO 输入和控制消息由 Rust 运行时拥有，前端不再充当调度器 |
| 9 | `P10-068` | 真正的 `turn_steer` 与 `turn_interrupt` | 已完成 | `expectedTurnId` 前置条件防止误导向；steer 不等同于取消后排队，interrupt 精确命中活动 Turn |
| 10 | `P10-069` | Thread fork/resume/rollback 与协议兼容收尾 | 已完成 | 分支和回滚保留审计边界；旧 schema 有明确迁移/淘汰策略 |
| 11 | `P10-070` | 异步重试与精确恢复控制 | 已完成 | 重试复用 mailbox/handle；桌面恢复只精确中断匹配 Turn，不调用线程级取消 |
| 12 | `P10-071` | 队列消息原子 steer | 已完成 | 后端只从 mailbox 读取消息；接纳与删除不可分割，竞态不重复注入或丢失输入 |
| 13 | `P10-072` | Thread 生命周期操作门 | 已完成 | Turn 接纳、worker 取项、fork/rollback 在同一 thread gate 下确定性串行 |
| 14 | `P10-073` | 按线程有界持久化 writer | 已完成 | 同线程保序、跨线程并行，事件发布继续等待 durable ack |
| 15 | `P10-074` | SQLite 增量历史索引 | 已完成 | append 和分页不再全量重放，索引可从 JSONL 确定性重建 |
| 16 | `P10-075` | 事件驱动 mailbox 同步 | 已完成 | revision 通知替代固定 500ms 刷新，活动状态只有一个权威来源 |
| 17 | `P10-076` | 结构化错误与显式状态 | 已完成 | 客户端按稳定 code/retryability 展示操作，旧字符串契约兼容 |
| 18 | `P10-077` | 沿职责边界拆分模块 | 进行中 | thread/storage/commands/frontend reducer 各自单一职责，无行为回归 |
| 19 | `P10-078` | Monaco 包体治理 | 待开始 | 初始与非 TS 预览不加载 TS worker，编辑和 Diff 功能不退化 |

## 最近完成：P10-062

### 范围

- Provider 第一次发送 `reasoning_summary_delta` 时，为该 `itemId` 写入并发布 `item_started(reasoning)`。
- `reasoning_summary_completed` 先持久化有界摘要，再写入并发布 `item_completed(reasoning, completed)`。
- Provider 重复完成事件不能产生重复 Item；没有增量而直接完成时也必须补齐启动事件。
- Turn 失败或取消时，已启动但未完成的 Reasoning Item按对应状态闭合。
- 前端继续用现有安全摘要投影；生命周期事件不能暴露原始思维链，也不能进入下一轮 Provider 历史。

### 非范围

- 不接收或展示 Provider 私有 reasoning/thinking 正文。
- 不迁移 Tool、Approval 或 Change Item。
- 不改变 `run_turn` 阻塞返回、前端队列或取消语义。

## 最近完成：P10-063

### 范围

- 每个已接受的 Tool call 使用现有 `call.id` 启动一个 `tool` Item；`tool_started`、stdout/stderr 增量和 `tool_completed` 继续共享这个身份。
- ToolResult 持久化并发布后，再写入 `item_completed(tool)`；成功结果闭合为 `completed`，拒绝、执行错误和重复调用保护闭合为 `failed`。
- 取消中的当前调用和同批次尚未执行的调用均闭合为 `cancelled`，Turn 失败/取消收尾会再次扫描，避免留下悬空 Tool Item。
- JSONL 恢复以 Tool Item 完成状态修正工具活动投影，pending、running、completed、failed、cancelled 均可区分；前端继续只投影已有 ToolActivity。

### 验收结果

- 成功、失败、取消三条 Rust 回归覆盖事实事件、实时事件和恢复投影。
- 前端 E2E 注入 Tool Item 生命周期事件，确认旧展示投影不重复渲染。
- `pnpm build`、`cargo fmt --check`、`cargo check`、单线程全量 Rust 220 项测试和完整 E2E（66 通过、2 条窄屏条件跳过）通过；并行 Rust 全量命令中的既有多智能体时序断言单独重跑通过。

## 最近完成：P10-064

### 范围

- Approval、UserInput 和 Change 分别复用请求或变更对象 ID；ContextCompaction 复用外层事实事件 ID，不生成平行身份。
- 请求事件前启动 Approval/UserInput Item，解决事件后按通过/回答、拒绝/跳过/超时、取消映射到 `completed`、`failed`、`cancelled`。
- Change 在审计写入前启动，成功审计后闭合为 `completed`；审计失败并完成工作区回滚后闭合为 `failed`。
- 自动与手动 ContextCompaction 都写入完整事实生命周期；自动压缩同时按相同 ID 发布实时事件。
- Turn 收尾和应用崩溃恢复闭合仍活动的 Item；旧会话缺少 `ItemStarted` 时不伪造 `ItemCompleted`。

### 验收结果

- Rust 回归覆盖 Approval、Change、UserInput 和 ContextCompaction 的成功、失败、取消、审计回滚及旧事件兼容分支。
- 前端 E2E 夹具注入交互类 Item 生命周期，确认通用生命周期事件不会重复创建既有时间线步骤。
- `pnpm build`、Rust 格式/检查、全量 Rust 222 项测试和完整 E2E（66 通过、2 条窄屏条件跳过）通过；`pnpm tauri dev` 的最新调试二进制已运行，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常。

## 最近完成：P10-065

### 范围

- `protocol/` 新增版本化 `ThreadTurn -> ThreadItem`、turns/items 页和统一历史快照；Item 载荷同时承载消息、Reasoning、Tool、审批、用户输入、变更、压缩及精确时间线片段。
- `storage/` 从 JSONL 事实只读构建统一历史，旧记录缺少 `TurnStarted` 或 Item 生命周期时使用确定性领域事实降级，不追加伪造事件。
- 新增 `read_thread_history`、`list_thread_turns` 和 `list_thread_items` Tauri 命令；页大小默认 50、最大 100，支持正反排序、Item 视图、Turn 过滤以及绑定查询条件的不透明双向游标。
- 工作台正常路径只从统一 Item 页投影消息、时间线、待处理交互与变更，并提供“加载更早记录”；旧 `read_thread` 仅保留为旧后端兼容降级。
- 本项没有改变阻塞式 `run_turn`、前端队列、授权、Thread mailbox 或 steer/interrupt。

### 验收结果

- Rust 回归覆盖统一载荷序列化、旧事件只读恢复、审批/工具展示顺序、items view、页大小、正反向游标以及跨线程、跨资源和跨过滤条件拒绝。
- 双视口 E2E 确认统一快照不再调用旧 `read_thread`，并使用游标加载更早 Turn；既有旧详情兼容、实时事件和 hydration 竞态流程继续通过。
- `pnpm build`、Rust 格式/检查、全量 Rust 224 项测试和完整 E2E（68 通过、2 条窄屏条件跳过）通过；最新 `pnpm tauri dev` 调试二进制已运行，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常。全量 Rust 首次运行中的既有多智能体 350ms 时序断言偶发超时，单独复核和完整重跑均通过。

## 最近完成：P10-066

### 范围

- `protocol/` 新增版本化 `TurnHandle`；Tauri 边界预分配 `turnId`，AgentRuntime 使用该 ID 持久化和发布完整 Turn 生命周期。
- 新增异步 `turn_start`：后台任务在 `turn_started` 已落盘并发布后返回 `streaming` handle，随后不再通过命令返回终态。
- 启动前的工作区、输入、Provider、扩展和同线程互斥错误继续由命令返回；启动后的成功、失败和取消只通过公共事件及统一历史查询获得。
- 桌面正常发送迁移到 `turn_start`，终态事件负责释放线程并唤醒下一条；极短 Turn 的终态先于 invoke 响应时不会被迟到 handle 恢复为活动状态。
- 阻塞 `run_turn` 继续复用同一执行函数作为兼容入口；本项没有实现 Thread mailbox、steer/interrupt 或 fork/rollback。

### 验收结果

- Rust 回归覆盖 handle 序列化、启动事件握手、启动前错误和调用方预分配 Turn ID 的实时事件/JSONL 一致性。
- 双视口 E2E 确认正常发送只调用 `turn_start`，不调用阻塞 `run_turn`；既有同线程串行、跨线程并行、取消终态解锁和图片输入流程继续通过。
- `pnpm build`、Rust 格式/检查、全量 Rust 228 项测试和完整 E2E（68 通过、2 条窄屏条件跳过）通过；最新 `pnpm tauri dev` 已热重载本次调试二进制，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常。

## 最近完成：P10-067

### 范围

- Rust `ThreadMailbox` 以线程为键维护一个 worker 和 FIFO 待处理 Turn；同线程串行，不同线程继续并行，所有执行仍复用唯一 `AgentRuntime`。
- `turn_start` 对活动线程返回 `queued` handle；首项在 `turn_started` 已落盘并发布后返回 `streaming`，启动失败对等待方返回错误、对已排队调用发布 `turn_rejected`。
- 新增 mailbox snapshot、删除单项和清空命令。前端队列只投影后端 pending 状态，不再持有调度锁或在终态后自行启动下一项。
- pending 输入是进程内控制状态，尚未被运行时接纳时不写成 JSONL 用户消息；进入活动 Turn 后才由既有先落盘后发布边界接管。

## 最近完成：P10-068

### 范围

- `turn_steer` 和 `turn_interrupt` 都要求 `expectedTurnId` 精确匹配当前活动 Turn；无活动 Turn、空 ID 或过期 ID 均拒绝。
- steer 将新输入加入活动 Turn 的控制队列，在当前 Provider 边界后持久化为用户消息、发布 `turn_steered` 并继续同一个 Turn，不通过取消和新建 Turn 模拟。
- interrupt 只取消匹配的活动 Turn；前端在持久化终态到达前保持忙碌，不因命令返回提前释放线程。
- `TurnControl` 的关闭和接收在同一锁内完成，覆盖 steer 与 Turn 收尾竞争，已接收输入不会丢失，关闭后的迟到输入不会被误报成功。

## 最近完成：P10-069

### 范围

- `thread_fork` 可复制全部或截至指定已完成 Turn 的有效对话历史，生成新线程与审计标记；活动 Turn 不可作为分支锚点。
- fork 不复制 `ChangeApplied`/`ChangeUndone` 及 Change Item 生命周期，避免新线程获得对同一工作区变更的第二份撤销权。
- `thread_resume` 通过统一历史读取恢复线程并触发既有孤儿 Turn/交互恢复；`thread_rollback` 在无线程活动和无 pending 输入时追加审计标记，并按完整 Turn 数截断有效历史。
- rollback 只改写后续对话读取视图，不回滚本地文件；文件撤销继续使用独立、可审阅的 change transaction。
- 公共事件 schema 升至 v4，JSONL schema 升至 8。旧事件缺少 `userMessage` 和历史重写标记时继续只读兼容；阻塞 `run_turn` 保留为迁移入口，正常桌面路径只使用异步协议。

### 验收结果

- Rust 回归覆盖 mailbox FIFO 与跨线程独立性、队列启动失败、精确控制前置条件、steer 续接下一次 Provider 请求、收尾竞态、fork 选定历史和 rollback 重启恢复/审计边界。
- 双视口 E2E 覆盖后端 mailbox 投影、steer 不触发 interrupt、跨线程隔离、终态前取消保持、纯图片消息由 `turn_started.userMessage` 以持久化身份展示。
- `pnpm build`、Rust 格式/检查、全量 Rust 236 项测试和完整 E2E（68 通过、2 条窄屏条件跳过）通过；既有多智能体 350ms 时序断言首次重跑偶发超时，单项复核和完整重跑通过。现有 `pnpm tauri dev` watcher 已加载最新调试二进制，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常。

## 最近完成：P10-070

### 范围

- 新增异步 `turn_retry`，在 Tauri 边界预分配 `turnId` 并把重试作为带类型工作项送入现有 Rust Thread mailbox；首项通过 `turn_started` 握手返回 `streaming`，待处理项立即返回 `queued`。
- 重试复用原用户消息、原协作模式、共享交互管理器、取消令牌和主 AgentRuntime；handle、实时事件和持久化历史使用同一身份。
- mailbox snapshot 的 `QueuedTurn.kind` 区分普通消息和重试，旧载荷缺少字段时按普通消息读取；重试项可删除但不能 steer。
- 桌面重试不再调用阻塞式 `retry_turn`；停止和错误恢复只调用带精确 `turnId` 的 `turn_interrupt`，恢复后重新读取线程状态，不乐观清除活动 Turn。
- 阻塞式 `retry_turn` 和线程级 `cancel_turn` 仅保留迁移兼容，不再出现在正常桌面调用路径。

### 验收结果

- `pnpm build`、Rust 格式检查和 `cargo check` 通过；Rust 237 项测试在单测试线程全量通过。
- 默认并行 Rust 全量中的既有多智能体 350ms 时序断言两次受负载影响超时，单项复核用时 0.29s 通过；未为本次无关改动放宽阈值。
- 完整双视口 E2E 为 70 通过、2 条窄屏条件跳过；覆盖异步重试、旧阻塞命令不再调用、精确恢复和队列文案。
- 现有 `pnpm tauri dev` watcher 已加载最新调试二进制，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常。

## 最近完成：P10-071

### 范围

- 新增 `turn_steer_queued(threadId, expectedTurnId, queuedTurnId)`；客户端只提交身份字段，正文与附件始终从 Rust mailbox 读取。
- `AppState` 在活动 Turn 锁内校验 `expectedTurnId`，再由 mailbox 在同一临界区完成 `TurnControl` 接纳与 pending 删除。
- 只有控制输入已被接受后才删除队列项。worker 已先取走、Turn 已关闭、重复请求和 retry 项均拒绝，不会出现同一输入既 steer 又启动新 Turn。
- 工作台不再组合 `turn_steer` 与 `remove_queued_turn`；失败分支重新读取 mailbox，避免显示已经被 worker 接纳的旧快照。

### 验收结果

- Rust 回归覆盖消息接纳、关闭后保留、过期活动 Turn、重复请求和 retry 项拒绝。
- 双视口 E2E 确认“加入当前对话”只调用一次 `turn_steer_queued`，不调用旧 steer、独立删除或 interrupt。
- `pnpm build`、Rust 格式/检查、全量 Rust 240 项测试和完整 E2E（70 通过、2 条窄屏条件跳过）通过。
- 现有 `pnpm tauri dev` watcher 已加载最新调试二进制，开发服务返回 HTTP 200，`k-Coder` 主窗口响应正常；未触发真实模型请求。

## 最近完成：P10-073

### 范围

- `storage/writer.rs` 新增 `ThreadWriters`：按 thread 建立容量 64 的有界 channel 与独立 writer task，同线程严格保序，跨线程并行。
- `JsonlThreadRepository::append` 移除进程级 `append_lock`，改为 `lock_thread` 后通过 writer 提交；durable ack（write+flush+`sync_data`）成功前不发布公共事件。
- writer 启动失败、写入失败或 channel 断开均返回类型化 `StorageError::Io`；`Drop` 时按 drain 关闭所有 writer，不遗留后台线程。

### 验收结果

- Rust 回归覆盖同线程顺序、跨线程独立 writer、启动失败有界返回；`cargo test` 相关用例通过。

## 最近完成：P10-074

### 范围

- SQLite 新增 `indexed_events`、`history_turns`、`history_items`、`history_state` 可重建投影索引；普通 `append_event` 按单事件增量 insert，不再整线程 delete/reinsert。
- `ThreadRolledBack` 显式触发 `replace_thread` 重建，保证 rollback 后索引与有效历史一致；fork 继续从有效投影复制。
- 新增 `history_index_is_current` 一致性校验；启动或显式 rebuild 时从 JSONL 全量重建，SQLite 损坏不反向改写 JSONL。

### 验收结果

- Rust 回归覆盖增量投影与 JSONL 重建一致性、并发 append 顺序、rollback 后索引可见性。

## 最近完成：P10-075

### 范围

- `ThreadMailbox` 为每个 thread 维护单调 `revision`；入队、取项、删除、清空和原子 steer 成功后递增。
- 后端发布版本化 `thread_mailbox_changed` 事件；前端 `handleMailboxChanged` 按 `event.revision <= knownRevision` 拒绝旧值，以 revision 驱动 `processQueue`。
- 移除终态后的固定 500ms `setTimeout` 刷新；`activeTurns[threadId]` 作为活动 Turn 权威投影，跨线程事件继续按 thread 隔离。

### 验收结果

- Rust 回归覆盖 revision 仅在成功变更后递增；双视口 E2E 覆盖事件驱动刷新与乱序收敛。

## 最近完成：P10-076

### 范围

- 公共协议新增结构化 `TurnError`：稳定 `code`、用户可见 `message`、`retryable`、`category` 和可选有界 `details`。
- `TurnFailed`、历史 Turn 和兼容 `TurnOutcome` 使用结构化错误；旧 JSONL 字符串失败读取时升级为 `legacy_failure`。
- Provider、认证、限流、策略、存储、协议和运行时错误映射到稳定分类；敏感信息不进入载荷，前端不再解析错误文案决定操作。

### 验收结果

- 协议序列化测试覆盖各错误分类；旧事件兼容升级路径通过回归。

## P10-077 本轮进展

### 已完成切片

- 将 `storage/mod.rs` 中的历史 turns/items 游标分页、游标编解码、页大小校验和 Item 视图裁剪提取到 `storage/history_pagination.rs`。
- `JsonlThreadRepository` 继续负责仓储生命周期、事实事件追加和索引协调，只通过模块边界调用分页实现。
- 保持公共协议、JSONL 事实来源、游标绑定条件、陈旧游标拒绝和 `NotLoaded`/`Summary`/`Full` 三种 Item 视图语义不变。

### 验收结果

- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` 通过。
- `cargo check --manifest-path src-tauri/Cargo.toml` 通过。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1` 通过，249 项测试全部通过。
- `pnpm build` 通过；构建仍提示 Monaco 编辑器 chunk 偏大，该问题属于后续 `P10-078`。
- `git diff --check` 通过。

本项仍在进行中，后续继续按同样的小步方式拆分 `agent/mod.rs`、`commands/mod.rs` 和 `workbenchStore.ts`，每个切片独立回归，避免一次性重写运行时。

## 剩余优化计划

以下项目是对照 `D:\code\codex` 后确认并由用户明确提升为 `P10-072` 至 `P10-078` 的实施批次。执行顺序遵循正确性优先于吞吐、吞吐优先于整理、每批可独立恢复和回滚。

### 优先级总览

| 候选批次 | 优先级 | 目标 | 主要证据 | 前置依赖 |
| --- | --- | --- | --- | --- |
| A | P0 | Thread 生命周期操作门 | `thread_fork`/`thread_rollback` 的活动检查与仓库操作分离，仍有启动竞态窗口 | `P10-071` |
| B | P1 | 按线程有界持久化 writer | JSONL append 使用进程级全局锁，每次写入都同步落盘并重放线程 | A |
| C | P1 | SQLite 增量历史索引 | append 和每次 turns/items 查询都全量加载、投影 JSONL；长会话累计成本持续增长 | B |
| D | P1 | 事件驱动 mailbox 与活动状态同步 | 工作台在多个终态分支使用固定 500ms 定时刷新，并同时维护多份活动 Turn 状态 | A |
| E | P2 | 结构化 TurnError 与显式 Item 状态 | `turn_failed` 主要暴露字符串，客户端难以区分可重试、权限、限流和协议错误 | A |
| F | P2 | 沿职责边界拆分超大模块 | `agent/mod.rs`、`storage/mod.rs`、`commands/mod.rs` 和 `workbenchStore.ts` 已混合多种职责 | B 至 E |
| G | P2 | Monaco 与前端包体治理 | 当前 `CodeEditor` 动态 chunk 约 3.98 MiB，TypeScript worker 约 7.03 MiB | 无 |

### 候选 A：Thread 生命周期操作门

问题：`src-tauri/src/commands/mod.rs` 中的 `thread_fork` 和 `thread_rollback` 先读取活动 Turn/mailbox 状态，再调用 Repository；`turn_start` 可能在两步之间入队或启动。当前检查能拒绝常见冲突，但不是和输入接纳共享的原子状态转换。

实施范围：

1. 在 `agent/` 或 `AppState` 增加按 thread 的 operation gate，统一串行化 `turn_start` 接纳、mailbox worker 取项、fork、rollback 和需要稳定历史边界的恢复操作。
2. 明确唯一锁顺序，禁止 `commands/` 自行组合“检查后执行”；Tauri 命令只做载荷校验和错误映射。
3. fork/rollback 获得 gate 后重新校验活动 Turn、pending 输入和历史锚点；操作结束前不允许新 Turn 穿过边界。
4. 保持 rollback 只改写对话读取视图，不回滚工作区文件；所有拒绝和成功继续保留审计语义。

验收出口：

- 并发 `turn_start` 与 fork/rollback 时只有一个操作获胜，另一方得到稳定错误。
- rollback 检查后到落盘前不能插入新 pending 输入；fork 不会复制半启动 Turn。
- 覆盖 worker 抢先、空 mailbox、非空 mailbox、过期锚点、连续 rollback 和重启恢复。

### 候选 B：按线程有界持久化 writer

问题：`src-tauri/src/storage/mod.rs` 中的 `JsonlThreadRepository` 使用一个进程级 `append_lock`；每个事件执行文件打开、写入、`sync_data`、重新加载整个线程并更新投影。不同线程相互阻塞，长线程的追加延迟随历史增长。

实施范围：

1. 为活动 thread 建立有界 writer actor/channel，单个 thread 内严格保序，不同 thread 可并行；禁止无界队列。
2. writer 独占对应 JSONL 句柄，按明确的字节数或短时间窗口批量 flush；关闭、崩溃恢复和应用退出必须有有界 drain。
3. 所有会改变运行时状态的事件继续等待 durable ack 后再发布公共事件。“异步 writer”不能退化为 fire-and-forget。
4. 写入失败使对应 thread fail closed，返回原始类型化存储错误；不能跳过事件后继续运行。
5. 保留截断尾记录恢复、完整坏记录拒绝、schema 校验和 JSONL 可独立审计能力。

验收出口：

- 记录 100、1,000、5,000 事件线程的 append p50/p95 基线和优化结果。
- 两个线程并发写入时，一个长线程的投影工作不阻塞另一个线程的 durable append。
- 压力、取消、退出和尾记录截断测试证明无重排、无静默丢失、无未关闭 writer。

### 候选 C：SQLite 增量历史索引

问题：`src-tauri/src/storage/mod.rs` 的 `read_thread_history`、`list_thread_turns` 和 `list_thread_items` 每页都加载并投影完整 JSONL；append 后 `src-tauri/src/persistence.rs` 的 `replace_thread` 还会删除并重建线程用量投影。分页限制了返回大小，但没有限制读取和计算量。

实施范围：

1. JSONL 继续作为唯一事实来源；SQLite 新增可重建的 Turn、Item、稳定顺序和游标索引，不写入第二套领域事实。
2. writer 按事件增量更新会话摘要、用量、Turn 和 Item 投影，停止每次 append 后的整线程 delete/reinsert。
3. turns/items 查询直接按绑定 thread、turn、方向和锚点的索引分页；页大小继续默认 50、最大 100。
4. rollback 标记使索引按有效历史隐藏后缀；fork 从有效投影复制，但 JSONL 重建必须得到完全相同结果。
5. 提供显式 rebuild 和一致性校验，投影损坏时从 JSONL 重建，不反向修改事实日志。

验收出口：

- 1、50、100 条页面的查询成本主要随页大小变化，不随整线程事件数线性增长。
- 增量投影与全量 JSONL 重建在旧 schema、失败/取消、retry、连续 rollback 和 fork 后逐项一致。
- 游标跨线程、跨资源、跨过滤条件、陈旧或篡改时继续 fail closed。

### 候选 D：事件驱动 mailbox 与活动状态同步

问题：`src/stores/workbenchStore.ts` 在 Turn 完成、失败和取消后使用固定 500ms `setTimeout` 刷新 mailbox，并同时维护 `activeTurnId`、`activeTurnThreadId` 与 `activeTurns`。延迟刷新会产生短暂旧状态，固定等待也不能证明后端已经变化。

实施范围：

1. mailbox snapshot 增加单调 revision，入队、取项、删除、清空和原子 steer 后发布版本化 `thread_mailbox_changed`。
2. 前端以事件 revision 决定是否重新读取 snapshot，去掉终态后的 500ms 定时器；乱序或重复事件不得回退状态。
3. 逐步以 `activeTurns[threadId]` 作为活动 Turn 权威投影，其他标量只作为当前视图派生值。
4. hydration 和跨线程事件继续按 thread 隔离；窗口重连时以 snapshot 校正遗漏通知。

验收出口：

- 队列变化无需固定延迟即可显示，重复、乱序和漏事件经 snapshot 后收敛。
- 多线程并行、切换线程、极短 Turn、终态先于 handle 和原子 steer 竞态均有双视口 E2E。

### 候选 E：结构化错误和显式状态

问题：`src-tauri/src/protocol/mod.rs` 的 `TurnFailed` 主要携带 `message: String`，Item 只暴露终态枚举。客户端无法可靠决定是否展示重试、配置、权限或存储恢复入口，也难以区分未开始、进行中和终态。

实施范围：

1. 新增版本化 `TurnError`，至少包含稳定 code、用户消息、retryability、可选 provider/storage 分类和有界诊断；禁止 API Key、授权头和完整环境变量进入载荷。
2. 为 Turn/Item 查询契约提供明确的 queued、in_progress 和终态，不从 `Option<status>` 反推进行中。
3. Provider、工具、策略、存储和用户取消分别映射到稳定分类；原始内部错误只进入脱敏日志。
4. 旧事件和旧客户端继续读取字符串错误；升级过程需要 schema 兼容测试。

验收出口：

- 限流、认证、权限拒绝、存储失败、协议错误、取消和可重试网络错误都有稳定序列化测试。
- 工作台只根据结构化 code/retryability 决定操作入口，不解析错误文案。

### 候选 F：沿职责边界拆分模块

问题：运行时、历史投影、持久化、Tauri 映射和前端 hydration 已集中在 `agent/mod.rs`、`storage/mod.rs`、`commands/mod.rs` 和 `workbenchStore.ts` 等少数超大文件中。继续增加并发和索引逻辑会扩大修改冲突及审查成本。

实施范围：

1. 不单独进行大爆炸式重写；在候选 B 至 E 落地时按既有边界提取 writer、history projector、thread operations、command handlers 和 frontend reducers。
2. `commands/` 只保留请求校验和映射；`agent/` 拥有 Turn 协调；`storage/` 拥有 writer/rebuild/query；`src/` 只投影后端事实。
3. 每次提取保持公共 API 和 JSONL schema 不变，并先添加特征测试再移动实现。
4. 删除兼容入口必须单独列出调用审计和淘汰窗口，不能借模块拆分顺带移除。

验收出口：

- 每个新模块只有一个主要变化原因，跨模块依赖不形成第二套 AgentRuntime 或持久化事实源。
- 移动前后的全量 Rust/E2E 行为一致，`git diff` 不混入无关格式化。

### 候选 G：Monaco 与前端包体治理

问题：`pnpm build` 当前显示主工作台 chunk 约 407 KiB，但 `src/components/CodeEditor.tsx` 对应的按需 chunk 约 3.98 MiB，TypeScript worker 约 7.03 MiB；只读 Shell、Diff 或普通文件预览不应无条件承担全部语言服务成本。

实施范围：

1. 保持 CodeEditor 按需加载，并按实际文件类型加载语言贡献和 worker；只读预览默认不启动不需要的 TypeScript 语言服务。
2. 评估 Monaco ESM 最小导入、worker 工厂和常用语言白名单；不为降低体积破坏现有行号、高亮、Diff 和复制功能。
3. 记录初次工作台加载、首次展开编辑器和再次展开的资源大小与耗时，不只调整 Vite warning 阈值。

验收出口：

- 初始工作台不请求 Monaco worker；展开非 TypeScript 只读预览不加载 TypeScript worker。
- `CodeEditor` chunk 和 worker 总传输量相对当前基线显著下降，并保持桌面/窄屏编辑器 E2E。

## 推荐执行顺序

```text
A Thread 操作门
  -> B 按线程 writer
  -> C 增量历史索引
  -> D 事件驱动状态同步
  -> E 结构化错误
  -> F 随批次拆分模块

G 包体治理可在 A 完成后独立并行，但不得替代运行时正确性和存储验收。
```

每个候选批次开始前必须先记录基线、确定协议/JSONL 是否升级、补充对应 ADR，并在总路线图中分配唯一任务 ID。B 与 C 不得在同一批同时改写 writer、投影 schema 和查询 API；先建立可验证的 durable writer，再迁移增量索引。

## 专项下一步

Codex 对话协议专项已经完成 `P10-060` 至 `P10-071`。当前仍回到 `docs/开发路线图.md` 的 `P10-001` Phase 10 验收门槛；若用户明确要求继续对话优化，先把候选 A“Thread 生命周期操作门”提升为下一项路线图任务，完成并发基线和 ADR 后再修改生产代码。

## 每项验收门槛

1. 公共协议、JSONL 迁移和成功/失败/取消分支有测试。
2. 旧事件和旧会话仍能恢复，前端只负责投影，不生成新领域身份。
3. 至少通过：

```powershell
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
pnpm test:e2e
```

4. `pnpm tauri dev` 实际启动，开发服务可访问且主窗口响应正常。
5. 同步更新本计划、开发路线图、架构文档和相关 ADR；默认不提交代码。

## 架构护栏

- `src/` 只投影 Item，不决定 Item ID、授权或 Turn 调度。
- `agent/` 协调生命周期；Provider 协议解析继续留在 `providers/`。
- `storage/` 的 JSONL 仍是事实来源；新增 Item 事件必须先持久化再发布。
- Item 载荷和模型传入字段不能成为权限依据。
- 每次只迁移一个可独立恢复、可独立回归的 Item 类型或运行时能力。
