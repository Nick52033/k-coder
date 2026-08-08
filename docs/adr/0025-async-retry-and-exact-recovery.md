# ADR 0025：异步重试与精确恢复控制

## 状态

已接受。

## 背景

`turn_start` 已通过 Rust Thread mailbox 返回稳定 `TurnHandle`，但桌面重试仍调用阻塞式 `retry_turn`，会让 IPC 一直等待 Turn 终态。错误恢复入口还会调用只按线程取消的 `cancel_turn`，无法防止活动 Turn 在读取与取消之间发生切换。与此同时，mailbox snapshot 不能区分普通消息和重试，界面也无法准确解释队列项。

## 决策

1. 新增异步 `turn_retry(threadId)`。Tauri 边界预分配 `turnId`，把重试作为带类型的 mailbox 工作项入队；首项在 `turn_started` 已持久化并发布后返回 `streaming` handle，其余项立即返回 `queued` handle。
2. 重试继续复用主 `AgentRuntime`、原用户消息、原协作模式、取消令牌和 `TurnControl`，不得创建第二套循环。返回 handle、实时事件和 JSONL 使用同一 `turnId`。
3. `QueuedTurn` 增加默认值为 `message` 的 `kind` 字段，取值为 `message` 或 `retry`。默认值用于读取旧载荷；重试项不伪造用户输入和附件。
4. 桌面正常路径不再调用阻塞式 `retry_turn` 或线程级 `cancel_turn`。二者仅作为迁移兼容入口保留。
5. 停止和错误恢复都必须读取当前活动 `turnId` 并调用 `turn_interrupt(threadId, turnId)`。恢复请求后重新读取线程和 mailbox 状态，不得在终态事实到达前乐观清除活动 Turn。
6. 普通队列消息通过 `turn_steer_queued` 在 Rust mailbox 内原子接纳并删除；客户端只提供活动 Turn 和队列项身份，不能覆盖 mailbox 中的正文或附件。重试队列项不能 steer，但仍可从 pending 队列删除。

## 影响

- 重试不再占用一次长生命周期 IPC，请求方可以像普通发送一样通过公共事件和统一历史观察结果。
- mailbox snapshot 可以准确展示和管理重试项，旧 snapshot 仍按普通消息读取。
- 精确中断前置条件避免恢复操作误停已经切换的新 Turn。
- 兼容命令暂不删除；后续移除前必须确认不存在旧客户端调用，并单独制定协议淘汰窗口。

## 验证

- Rust 回归覆盖重试 mailbox 类型、固定 `turnId` 的重试事件与历史一致性。
- 双视口 E2E 覆盖桌面只调用 `turn_retry`、恢复携带精确 `turnId`，且不调用 `cancel_turn`。
- 执行 Phase 10 规定的前端构建、Rust 格式/检查/测试、完整 E2E 和桌面启动验证。
