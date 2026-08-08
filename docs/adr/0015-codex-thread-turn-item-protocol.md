# ADR 0015：Codex 式 Thread/Turn/Item 对话协议演进

## 状态

已接受，2026-08-07。

## 背景

k-Coder 已经能流式展示正文、推理摘要、工具、审批和 Turn 终态，但公共事件仍以界面需要为中心：`text_delta` 只有 Turn ID，前端需要生成随机 `stream-*` ID，再在 Turn 完成时猜测哪段正文应替换为最终消息。这样会让实时事件、JSONL 恢复和最终消息拥有不同身份，也阻碍后续实现 Codex 的 item lifecycle、真正的 steer 和分页历史。

Codex 把对话建模为 `Thread -> Turn -> ThreadItem`。每个可见或可审计项目先获得稳定 ID，增量和完成通知引用同一项目；客户端只负责投影，不负责制造领域身份。

## 决策

1. k-Coder 逐步把对话公共契约迁移为 `Thread -> Turn -> Item`，不复制 Codex 的 TUI 展示实现。
2. Item ID 由 Rust 运行时在 Provider 请求开始前生成。前端不得为协议 v2 正文生成领域 ID。
3. `text_delta` 升级为事件 schema v2，增加必填 `itemId`。同一 Provider 回复的所有正文增量、工具前说明持久化记录和最终 `ChatMessage.id` 复用该 ID。
4. `assistant_tool_calls` 存储事件增加可选 `itemId`，事件 schema 升级为 6。旧事件缺少该字段时，历史投影继续以持久化事件 ID 作为确定性降级值。
5. 前端按 `threadId + turnId + itemId` 合并正文增量。schema v1 事件仅保留显示兼容路径，不能影响新的领域契约。
6. `TurnTimelineItem` 保留为 Item 内的有界展示片段。ADR 0016 至 0019 为全部公共 Item 引入统一生命周期，ADR 0020 已让历史时间线完全从 Thread Item 投影生成。
7. ADR 0021 已把阻塞式 `run_turn` 拆出立即返回的 turn handle 和独立事件流；ADR 0022 至 0024 已完成后端 Thread mailbox、带 `expectedTurnId` 前置条件的 `steer`/`interrupt` 以及 Thread fork/resume/rollback。前端取消加排队不能冒充 steer。
8. 授权、工作区校验、工具执行和持久化职责保持现有架构边界。模型输入或 Item 载荷不能成为授权依据。

## 分阶段迁移

- 第一阶段：稳定的 assistant item identity。本 ADR 随该阶段落地。
- 第二阶段：助手 Item 生命周期事件。本阶段由 ADR 0016 落地，其他 Item 类型继续迁移。
- 第三阶段：统一 AgentMessage、Reasoning、Tool、Change、Approval 等 Item 生命周期。
- 第四阶段：统一 Thread Item 历史与 turns/items 分页读取，由 ADR 0020 落地。
- 第五阶段：立即返回的 `turn_start`、后端 Thread mailbox、`turn_steer` 和 `turn_interrupt` 分别由 ADR 0021、0022、0023 落地。
- 第六阶段：Thread fork/resume/rollback 和协议兼容由 ADR 0024 收尾。

每个阶段必须保持旧 JSONL 可恢复，并为公共契约、迁移和终态分支添加测试。不得用一次跨层重写替换可验证的增量迁移。

## 结果

实时正文和最终持久化消息首次拥有同一稳定身份，前端不再依赖随机 ID 或仅按 Turn 猜测正文归属。这为通用 Item 生命周期和真正的 steer 建立了兼容基础；本 ADR 初始阶段没有改变 `run_turn` 的阻塞返回、消息队列语义或工具串行策略，异步启动的后续决策见 ADR 0021。
