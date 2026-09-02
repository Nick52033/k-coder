# ADR 0052：可中断 Provider 请求的队列引导

## 状态

已接受，2026-08-31。取代 ADR 0047 第 5 项；ADR 0047 的主发送排队、原子 mailbox 接纳和独立停止语义继续有效。

## 背景

ADR 0047 把队列项“发送到当前对话”定义为同一 Turn 的 steer，但要求等待当前 Provider 响应自然结束后再接纳输入。实际使用中，这会让长时间生成无法及时响应引导；如果活动 Turn 已进入收尾，前端已经显示的 steer 用户消息还可能与 mailbox pending 项同时存在。原 Turn 释放后，worker 会把残留项启动成第二个 Turn，用户随后点击一次停止只能停止旧 Turn，第二个 Turn 又开始运行。

队列引导需要同时满足两组边界：它必须立即停止当前模型生成并把输入加入现有上下文，但不能调用整个 Turn 的 interrupt、产生取消终态、取消已经由工具运行时管理的工作，或创建替代 Turn。

## 决策

1. `TurnControl` 为当前 Provider 请求登记一个从根 Turn cancellation 派生的子 `CancellationToken`。每次 Provider 请求结束后清除登记；工具执行继续只受根 Turn cancellation 管理。
2. `TurnControl::steer` 在同一互斥区内先接纳消息，再取消已登记的 Provider 子令牌。`ThreadMailbox::steer_message` 仍只在接纳成功后删除对应 pending 项，因此消息不能同时保留在 mailbox 和当前 Turn。
3. AgentRuntime 在流建立、流式读取和 Provider 自动重试等待期间都监听该子令牌。根 Turn cancellation 已取消时继续走现有 `turn_cancelled` 收尾；只有根令牌仍有效且 Provider 子令牌被取消时，才按 queued steer 继续原 Turn。
4. steer 中断发生时，已经流出的助手正文或图片以当前 `itemId` 持久化为部分 AssistantMessage；当前 AgentMessage 和尚未闭合的 Reasoning 等非工具执行 Item 以 `cancelled` 状态闭合。尚未收到 Provider `Completed` 的工具调用不得启动。随后持久化 steer UserMessage、发布原 `turnId` 的 `turn_steered`，并在同一个 AgentRuntime 循环发起下一次 Provider 请求。
5. steer 自身不得发布 `turn_started`、`turn_completed`、`turn_failed` 或 `turn_cancelled`，不得取消根 Turn 或子智能体，也不得调用 `turn_interrupt`。下一次请求的 Provider 历史必须包含初始用户消息、可用的部分助手输出和 steer 用户消息。
6. 显式停止继续只通过精确 `turnId` 的 `turn_interrupt` 取消根 Turn。若停止紧随已接纳的 steer 到达，运行时可以先落盘该用户消息，但整个交互只能产生原 Turn 的一个取消终态，mailbox 不得再次启动同一消息。
7. steer 在 Provider 请求登记前到达时，新建的子令牌必须立即处于取消状态；在 Provider 已自然完成后到达时，由既有 `close_if_idle` 原子边界决定继续同一 Turn 或拒绝接纳。拒绝时 mailbox 项保持不变，不能静默丢失。

## 后果

- 队列项的显式发送会立即停止当前模型流，并在原 Turn 内用新约束继续生成。
- 用户只看到一条 steer 用户消息；被接纳的 mailbox 项立即消失，不会在旧 Turn 停止后重新启动。
- 已经显示的部分回答可刷新恢复，但其 Item 明确记录为被引导中断；后续最终回答仍属于同一个 Turn。
- Provider 请求级取消与整个 Turn 停止成为两个独立控制层，Provider 适配器继续只接收标准 `CancellationToken`，不感知 mailbox 或界面语义。

## 验证

- `TurnControl` 单测验证 steer 只取消 Provider 子令牌，不取消根 Turn，并保留已接纳消息。
- AgentRuntime 回归在首轮流式正文后发起 steer，验证旧流尾部不再处理、部分正文被持久化、第二次请求包含 steer、全程只有一个 `turn_started` 和一个 `turn_completed`，且没有 `turn_cancelled`。
- AgentRuntime 回归在第二次 Provider 请求开始后只取消一次根 Turn，验证只有一个取消终态且不产生第三次请求。
- desktop/narrow Playwright 验证 mailbox 项归零、steer 用户消息只出现一次、停止只调用一次精确 `turn_interrupt`，终态后不再调用新的 `turn_start`。
