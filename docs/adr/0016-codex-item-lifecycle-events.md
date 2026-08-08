# ADR 0016：Codex 式 Item 生命周期事件

## 状态

已接受，2026-08-07。

## 背景

ADR 0015 已让助手正文的增量、工具前说明和最终消息共享稳定 Item ID，但事件流仍只有具体内容事件。客户端无法直接知道一个 Item 何时开始、何时以成功、失败或取消结束，恢复时也只能从具体事件推断生命周期。

Codex 的对话协议把 Item 生命周期作为独立公共事件，内容增量只是 Item 的载荷。k-Coder 需要先引入这层边界，再逐步迁移推理、工具、审批和变更等 Item 类型。

## 决策

1. 新增 `item_started` 和 `item_completed` 事件，事件 schema 升至 v3。
2. 本阶段只为助手回复 Item 生成生命周期事件；Item ID 由 Rust 运行时生成，并复用 `text_delta`、工具前说明和最终 `ChatMessage.id`。
3. `item_completed.status` 只允许 `completed`、`failed` 和 `cancelled`。失败或取消终态到达前，运行时必须先闭合当前助手 Item；没有可恢复的启动记录时不伪造 Item。
4. JSONL 增加对应的事实事件，存储 schema 升至 7。旧版本事件读取时继续升级到当前版本，旧记录没有生命周期事件不影响历史投影。
5. `TurnTimelineItem` 仍是前端展示投影。前端使用 `item_started` 接管流式助手消息的真实身份，`item_completed` 由具体正文和 Turn 终态共同完成展示收尾。
6. 本阶段不改变工具串行执行、审批授权、队列语义、阻塞式 `run_turn` 或取消/重试行为；“取消后发送下一条”仍不是 steer。

## 后续迁移

- Reasoning、Tool 和当前交互类 Item 已分别由 ADR 0017、0018、0019 完成迁移。
- 统一 Thread Item 历史投影与有界分页已由 ADR 0020 落地，异步 turn handle 和 Thread mailbox 已由 ADR 0021、0022 落地。
- 带 `expectedTurnId` 前置条件的 steer 与精确 interrupt 已由 ADR 0023 落地，Thread 历史控制由 ADR 0024 收尾。

## 结果

实时事件和 JSONL 现在都能表达助手、Reasoning、Tool 及交互类 Item 的完整生命周期，成功、失败和取消路径拥有相同的身份闭合规则。增量展示仍保持旧前端投影和 schema v1 兼容，统一历史投影可以直接建立在这些事实事件之上。
