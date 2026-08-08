# ADR 0023：精确 Turn steer 与 interrupt

## 状态

已接受，2026-08-08。

## 背景

前端过去把“取消当前 Turn，再把队列项放到下一位”称为立即发送。该行为会创建新 Turn、改变取消审计，也可能在活动 Turn 已切换后误取消新的工作。真正的 steer 应把新用户输入加入当前 Turn，并在同一 AgentRuntime 循环的下一个 Provider 边界继续。

控制请求还存在典型竞态：客户端读取活动 Turn A 后，A 可能在请求到达前结束并由 Turn B 接替。如果命令只带 thread ID，就会错误控制 B。

## 决策

1. `turn_steer` 和 `turn_interrupt` 都必须携带非空 `expectedTurnId`。`AppState` 只在它与当前活动 Turn ID 精确相等时接受请求；无活动 Turn 或 ID 过期均返回类型化错误。
2. 每个活动 Turn 持有 `TurnControl`。steer 将经过既有文本、图片能力和 OCR 边界验证的 `ChatMessage` 加入控制队列，不取消 Turn，也不创建新 Turn。
3. AgentRuntime 在 Provider 调用前和响应处理后提取已接受的控制输入，先写 `UserMessage`，再发布带同一消息身份的 `turn_steered`，随后以原 `turnId` 发起下一次 Provider 请求。
4. Provider 已在进行时，steer 等待该 Provider 请求返回或被其他原因终止，不通过取消令牌抢占流。这样保留已产生的助手输出和工具事实，并在确定边界继续。
5. `TurnControl` 的接收与关闭使用同一互斥状态。Turn 收尾只有在确认无 pending 输入后才关闭；已接受输入必须被处理，关闭后的迟到 steer 必须拒绝。
6. `turn_interrupt` 只取消精确匹配的活动 Turn。命令返回不代表终态已经持久化，客户端必须等待 `turn_cancelled`、`turn_failed` 或 `turn_completed` 才释放忙碌状态。
7. 已进入 Rust mailbox 的普通消息通过 `turn_steer_queued(threadId, expectedTurnId, queuedTurnId)` 加入当前 Turn。命令只接收身份字段，正文和附件从 mailbox 读取；活动 Turn 校验、`TurnControl` 接纳和 pending 删除在同一原子边界内完成。只有控制输入已被接受后才删除队列项；worker 已取走、Turn 已关闭、重复请求或 retry 队列项必须拒绝且不得重复注入。前端不得再组合 `turn_steer` 与 `remove_queued_turn` 模拟这一操作。

## 后果

steer 现在保持同一 Turn 的历史、用量和审计边界，interrupt 不会误伤接替运行的 Turn。所有控制请求都能明确识别过期状态。代价是 steer 不会立即中断一个仍在流式响应的 Provider；其可见生效点是当前 Provider 边界完成之后。
