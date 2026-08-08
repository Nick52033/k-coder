# ADR 0021：异步 Turn 启动与 Handle 契约

## 状态

已接受，2026-08-08。

## 背景

k-Coder 的 `run_turn` Tauri 命令过去会等待完整 AgentRuntime 循环结束后返回 `TurnOutcome`。虽然实时过程已经通过公共事件发布，命令 Promise 仍同时承担启动确认、终态结果和前端队列锁，导致客户端必须长期持有一次 IPC 调用，也无法围绕稳定的活动 Turn ID 构建后续 mailbox、steer 和精确 interrupt。

Codex 的 `turn/start` 在运行时接受输入后返回处于进行中的 Turn，后续结果由通知和线程读取获得。k-Coder 已具备先落盘后发布的 `turn_started`、终态事件及统一 Thread/Turn/Item 查询，因此可以先迁移启动边界，而不同时引入 mailbox 或控制消息。

## 决策

1. `protocol/` 新增版本化 `TurnHandle`，公开 `schemaVersion`、`threadId`、`turnId` 和当前 `state`。`turn_start` 返回的状态为 `streaming`，不携带终态、错误或耗时。
2. Tauri `turn_start` 在边界生成稳定 `turnId`，把它交给唯一的 AgentRuntime 循环。运行时完成既有工作区、输入、Provider 和线程互斥校验，持久化用户消息、Turn 模式及 `turn_started`，再发布 `turn_started`。
3. `turn_start` 只在 `turn_started` 已发布后确认 handle。启动前错误继续作为类型化命令错误返回；确认后的 Provider、工具、审批、失败、取消和完成结果不再通过该命令返回。
4. 已确认的 Turn 在 Tauri 后台任务中继续执行。后台任务沿用现有目标预算、取消令牌、事件发布、结构化日志和 `AppState::finish_turn` 清理路径；调用方关闭 invoke Promise 不会取消 Turn。
5. 客户端通过公共实时事件获取增量和终态，通过 `read_thread_history`、`list_thread_turns` 或 `list_thread_items` 恢复和查询结果。JSONL 继续是唯一持久化事实来源。
6. 桌面正常发送路径迁移到 `turn_start`。handle 只补充活动 Turn 身份，`turn_completed`、`turn_failed` 和 `turn_cancelled` 才能释放该线程并唤醒下一条消息；客户端必须防止终态先于 invoke 响应投影时被迟到 handle 重新标记为活动。
7. 阻塞式 `run_turn` 和 `TurnOutcome` 暂时保留为兼容入口，继续复用相同执行函数，不创建第二套循环。
8. 本决策本身不把前端 FIFO 队列迁入 Rust，也不新增 Thread mailbox、steer、精确 interrupt、fork 或 rollback；这些后续所有权与语义已分别由 ADR 0022、0023、0024 落地。

## 结果

正常客户端不再为完整模型循环持有阻塞 IPC，并在启动确认时获得与事件、JSONL 和历史查询一致的稳定 Turn ID。启动错误与运行终态拥有清晰边界，后续 mailbox 和带 `expectedTurnId` 的控制命令可以直接引用活动 Turn。

P10-067 完成后，Rust Thread mailbox 已接管 FIFO 和跨线程调度，前端只投影后端 pending 状态。阻塞兼容入口仍可返回终态，但正常桌面路径不再调用；它在 1.0 发布前保留为旧调用方迁移入口，不得承载 mailbox 或控制语义。
