# ADR 0047：主发送排队与显式队列 steer

## 状态

已接受，2026-08-24。取代 ADR 0042。

## 背景

ADR 0042 曾把活动 Turn 中的主发送定义为“先把新消息放入 mailbox，再取消当前 Turn，随后以新 Turn 继续”。该行为会让第二次发送立即出现“正在停止”，并留下“旧 Turn 取消 + 新 Turn 启动”的审计事实。

用户现已明确交互预期：活动 Turn 中再次使用主发送时，消息应只进入队列；只有用户在队列项上显式点击发送时，才把该消息作为引导加入当前 Turn。独立停止按钮才代表取消。三个动作必须具有不同、稳定且可审计的语义。

现有 Rust Thread mailbox 和 `turn_steer_queued` 已经提供所需边界。回归来自 `turn_start` 额外接受 `interruptActiveTurnId`，导致主发送绕过普通 FIFO 语义并关闭当前 `TurnControl`。

## 决策

1. `turn_start` 只接纳新的 mailbox 工作项，不再接受 `interruptActiveTurnId`，也不得关闭 `TurnControl`、触发取消令牌或取消当前 Turn 的子智能体。
2. 同一 thread 已有活动 Turn 时，`turn_start` 返回 `queued` handle。pending 输入继续只存在于 mailbox snapshot 中，不提前写入 `UserMessage`，也不提前显示为对话气泡。
3. 主发送在活动 Turn 中保持当前生成状态，界面提示为“加入消息队列”，不得乐观切换为“正在停止”。
4. 普通队列项的“发送到当前对话”只调用 `turn_steer_queued(threadId, expectedTurnId, queuedTurnId)`。后端从 mailbox 读取原始正文和附件，在精确活动 Turn 接受输入后原子删除 pending 项，并以原 `turnId` 发布 `turn_steered`。
5. queued steer 不取消正在进行的 Provider 请求。输入在当前 Provider 响应后的安全边界进入同一个 AgentRuntime Turn，保留已产生的正文、工具事实、用量和审计边界。
6. 队列中的 retry 和内置工作流启动不能 steer；它们只能按 FIFO 启动或由用户删除。活动 Turn 已进入停止状态时，队列 steer 继续禁用。
7. 取消只允许由独立停止、错误恢复等明确控制入口调用带 `expectedTurnId` 的 `turn_interrupt`。主发送和 queued steer 都不得组合或间接调用 interrupt。
8. 兼容阻塞 Turn 没有 mailbox worker 时，普通入队可以创建等待中的 worker，但 worker 必须等活动 Turn 释放后再取项，不得取消或越过活动 Turn。

## 后果

- 第二次主发送稳定显示为队列项，当前 Turn 继续运行。
- 用户可以等待 FIFO 自动执行，也可以显式把普通队列消息发送到当前对话作为 steer。
- 对话历史不再因为普通第二次发送产生非预期的取消终态和替代 Turn。
- `turn_start`、`turn_steer_queued` 和 `turn_interrupt` 分别对应排队、引导和停止，客户端不能通过一个主按钮隐式混合三种语义。

## 验证

- Rust 回归覆盖活动 mailbox Turn 收到普通入队后取消令牌保持未触发、`TurnControl` 仍可接受 steer、pending 消息保持在 FIFO，以及兼容阻塞 Turn 下 worker 等待释放。
- desktop/narrow Playwright 覆盖主发送不携带中断字段、不进入停止态、产生一个 pending 项；队列发送只调用一次 `turn_steer_queued`，不调用旧 steer、独立删除或 interrupt，并保持原活动 `turnId`。
- 执行 Phase 10 规定的前端构建、Rust 格式/检查/测试、`git diff --check` 和真实 `pnpm tauri dev` 工作流验证。
