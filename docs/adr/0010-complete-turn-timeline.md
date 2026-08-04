# ADR 0010：补全对话时间线缺失事件

- 状态：已接受；第 6 项中的固定 Token 上限已由 ADR 0012 取代，固定 Provider/工具调用次数上限已由 ADR 0013 取代
- 日期：2026-08-02

## 背景

JSONL 是会话事件的事实来源，但旧的 `TurnTimelineItem` 只投影文本、推理摘要和工具活动。Provider 上下文、逐次用量、压缩、审批、文件变更、用户提问、任务清单和 Turn 终态虽然已经部分落盘，却在刷新后被分散到其他字段或直接丢失。失败/取消的 Turn 也没有稳定的时间线项，重试时无法把多个尝试放在同一条用户消息下。

实时事件还有两个一致性风险：加载线程快照期间到达的事件可能被旧快照覆盖；命令的 stdout/stderr 游标输出只存在于瞬时 UI 状态，刷新后语义丢失。

## 决策

1. `TurnTimelineItem` 保留 `text`、`reasoning` 和 `tool`，新增统一的有界 `event` 变体。`TimelineEventKind` 独立表示 provider context、usage、compacted、approval requested/resolved、user input requested/resolved、todo updated、change applied/undone 和 turn completed/failed/cancelled。
2. `project_thread` 按 JSONL 事件顺序生成时间线，同时保留审批、用户输入、变更、任务清单和最后用量的领域快照。生命周期事件使用稳定的领域 ID，实时重放不会重复同一事件。
3. `request_user_input`、`todo_write`、Provider 用量和终态事件先追加 JSONL，再发布给前端。用户输入恢复为 FIFO 队列；线程恢复会把未解决的审批和用户输入转为取消，避免展示已失效的操作卡片。
4. `run_command` 的最终结果元数据保留有界的脱敏 stdout/stderr chunks（含 stream、cursor、text），前端优先读取该结构，旧结果继续回退到 stdout 文本。
5. 线程加载建立临时 hydration buffer。当前线程的实时事件在快照完成前暂存，应用快照后按接收顺序同步重放；非当前线程事件不能污染当前视图。消息的 `turnUserMessageIds` 将失败/取消/重试尝试锚定到原用户消息，无法锚定的孤儿 Turn 单独显示。
6. 设置边界：Provider 上下文最多 512 KiB，单次模型响应最多 512 KiB，单 Turn 最多 24 次迭代、24 次工具调用和 1,000,000 tokens，时间线文本单项最多 2,000 字符，持久化命令输出最多 64 KiB。瞬时进展队列可以丢弃展示增量，但最终结果与审计事件不得丢失。
7. `read_file` 的有界 `ToolResult.output` 是 Provider 历史与审计事件共享的事实。无范围参数时默认最多返回 64 KiB；`startLine/lineCount` 优先于兼容的 `offset/limit`，存在任一行范围字段时忽略冗余字节范围，显式范围最多返回 256 KiB。对话时间线只展示读取路径、范围和状态等摘要，不再提供“查看读取内容”入口；结果仍原样保留在 Provider 历史中，不重新读取当前文件。
8. 每次 Provider 请求前都从持久化事件重建并修复工具历史。结构化 `assistant_tool_calls` 只有在调用 ID 唯一、后续结果数量一致且 `tool_call_id` 集合完全匹配时才保留；中断或崩溃遗留的缺失、重复、错配和孤立结果不得发送给 Provider。工具调用事件中的非空可见说明可以降级为普通助手文本，不能为满足协议而伪造工具结果。
9. 时间线展示把完成状态相同且相邻的 reasoning 摘要合并为一个可折叠组，只显示一次标题和状态；组内保留各摘要事件的原始顺序，任何非 reasoning 条目或完成状态变化都会切断分组。审批请求与解决事件按请求关联的 `toolCallId` 排在对应工具活动之前，实时事件和历史恢复都遵循该展示顺序。活动 Turn 没有工具或时间线条目时，只保留状态标题，不渲染额外的空工具调用占位行。上述规则只作用于前端投影，不合并、改写或删除持久化事件。
10. 时间线展示把相邻工具活动聚合为独立的可展开组；纯 `run_command` 组使用“运行了 N 个命令”，其他组使用“执行了 N 个操作”。全部完成的组默认折叠，Pending、Running、Failed 或 Cancelled 组保持展开，确保等待与异常状态不会被隐藏。说明、reasoning、审批、变更和其他事件都会切断工具组；展开后继续展示每个工具的原始状态、参数摘要、输出和耗时。该分组同样只改变前端展示，不改变 JSONL、Provider 历史、领域快照或 Compaction。

## 影响

- 刷新、崩溃恢复和实时展示都使用同一份事件顺序，审批、用户输入、变更、终态和用量不再静默消失。
- 时间线只保存受控摘要和 ID，不复制完整 Provider payload、密钥或无限命令输出。
- 前端需要兼容旧的 `toolActivities` 和缺少新增字段的历史 `ThreadDetail`；事件 schema 从 2 迁移到 3。
- 文件读取的上下文成本在工具边界确定，持久化工具结果与模型所见保持一致；对话时间线只显示受控摘要，不重新读取工作区或复制完整读取内容。
- 中断在 `assistant_tool_calls` 与全部 `tool_result` 落盘之间发生时，旧会话仍可重试；代价是未完整落盘的工具组不会继续进入模型上下文。

## 验收门槛

- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` 通过。
- `cargo check --manifest-path src-tauri/Cargo.toml` 通过。
- `cargo test --manifest-path src-tauri/Cargo.toml` 通过，并覆盖用户输入落盘顺序、终态投影、输出 chunks 和旧事件迁移。
- `pnpm build` 和双视口 Playwright 时间线、恢复、取消重试及 hydration 竞态测试通过。
- 启动 `pnpm tauri dev`，在真实桌面窗口打开含审批/用户输入/变更的会话，确认事件可操作且刷新后仍可恢复。
