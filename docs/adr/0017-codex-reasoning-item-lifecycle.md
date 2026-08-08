# ADR 0017：Reasoning Item 生命周期迁移

## 状态

已接受，2026-08-07。

## 背景

助手回复已经具备 `item_started`/`item_completed` 生命周期，但安全推理摘要仍只有增量和完成事件。这样恢复日志中无法区分“摘要正在生成”和“摘要已经完成”，超限、失败或取消也可能留下未闭合的 Reasoning Item。

## 决策

1. Provider 第一个 `reasoning_summary_delta` 或直接的 `reasoning_summary_completed` 为该 `itemId` 持久化并发布 `item_started`，类型为 `reasoning`。
2. 一个 Reasoning Item 只允许一个 `item_completed`；重复完成事件不重复写摘要或生命周期。
3. 摘要正文仍受原有有界上限约束，只持久化安全摘要，不加入下一轮 Provider 历史，也不接收私有 thinking 内容。
4. 正常完成先写有界 `ReasoningSummary`，再闭合成功 Item；摘要超限、Turn 失败和取消会以对应状态闭合已启动 Item。
5. 前端不需要为生命周期事件新增可见内容，继续按安全摘要事件投影已有折叠步骤。

## 结果

Reasoning 现在与助手正文共享同一套可恢复生命周期契约，后续 Tool、Approval 和 Change 可以复用同一个运行时辅助层。该迁移不改变 Provider 历史、授权或 Turn 调度语义。
