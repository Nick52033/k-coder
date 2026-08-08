# ADR 0028：Mailbox revision 与变更通知

## 状态

已接受，2026-08-08。

## 背景

工作台通过固定延时在终态后重新读取 mailbox，并维护多份活动 Turn 状态。延时不能证明后端已经变化，快速 Turn、跨线程和窗口 hydration 时会短暂显示旧队列。

## 决策

1. `ThreadMailboxSnapshot` 增加单调 `revision`。每次入队、取项、删除、清空和原子 steer 成功后递增。
2. 后端发布版本化 `thread_mailbox_changed`，携带 thread ID 和 revision；事件不复制 pending 正文或附件。
3. 前端收到更高 revision 时读取 snapshot；重复和旧 revision 忽略，重连和 hydration 后以 snapshot 收敛。
4. `activeTurns[threadId]` 是活动 Turn 权威投影；当前线程标量从它派生，不再作为独立事实源。
5. 移除终态后的固定 500ms mailbox 刷新；兼容旧后端时仍允许显式 snapshot 读取。

## 后果

队列展示由后端变化驱动，乱序和遗漏可通过 revision 与 snapshot 收敛。每次变更会增加一次轻量通知，客户端仍只读取有界 snapshot。

