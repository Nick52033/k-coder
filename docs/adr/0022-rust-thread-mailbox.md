# ADR 0022：Rust Thread mailbox 与输入所有权

## 状态

已接受，2026-08-08。

## 背景

ADR 0021 让 `turn_start` 能在启动确认后立即返回，但同线程 FIFO、终态唤醒和跨线程调度仍由前端集合与锁维护。窗口刷新、事件先于 handle 到达或多个客户端同时调用时，这套状态无法成为运行时权威，也无法为精确 steer/interrupt 提供一致的活动 Turn 前置条件。

Codex 的线程运行时由后端串行接纳输入，客户端只发送请求和投影状态。k-Coder 也需要把调度所有权迁到 Rust，同时继续复用唯一 `AgentRuntime`、工作区互斥、取消令牌和 JSONL 事实源。

## 决策

1. `AppState` 持有进程内 `ThreadMailbox`。每个 thread 只允许一个 worker，worker 按 FIFO 取出 `MailboxTurn`；不同 thread 的 worker 可以并行。
2. `turn_start` 在 Tauri 边界预分配稳定 `turnId` 并入队。首项等待 `turn_started` 已落盘和发布后返回 `streaming` handle；活动线程的新输入立即返回 `queued` handle。
3. mailbox worker 只调用既有 `execute_turn` 和唯一 `AgentRuntime`。它不实现 Provider 协议、工具行为、授权或第二套智能体循环。
4. 新增版本化 mailbox snapshot 以及读取、删除单项和清空 pending 的命令。前端队列完全由 snapshot 投影，不持有执行锁，也不在终态事件后自行启动下一项。
5. pending 输入属于暂态控制状态，在 worker 接纳前不写 JSONL、不发布用户消息。接纳后继续由 AgentRuntime 先写 `UserMessage` 和 `TurnStarted`，再通过 `turn_started.userMessage` 交付稳定身份。应用退出会丢弃尚未接纳的 pending 输入，不得把它们伪装成已持久化对话事实。
6. 首项启动前失败通过等待中的命令返回；已返回 queued handle 的项若随后启动失败，则发布 `turn_rejected`，worker 继续处理下一项。

## 后果

同线程单活动 Turn 和 FIFO 顺序现在由 Rust 运行时保证，多个界面或事件竞态不会创建两套调度状态。前端只负责展示 pending 输入，跨线程仍可并行。代价是未被接纳的 pending 输入不具备崩溃恢复保证；若将来需要持久 mailbox，必须新增独立、明确的待处理输入事实，而不能复用用户消息事件。
