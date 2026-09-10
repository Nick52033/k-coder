# ADR 0018：Tool Item 生命周期迁移

## 状态

已接受，2026-08-07；批次排队态实时投影于 2026-09-10 修订。

## 背景

Tool call 已经有独立的 `tool_started`、输出增量和 `tool_completed` 事件，工具活动恢复也能从 `assistant_tool_calls`、`tool_started` 和 `tool_result` 推导状态。但这些事件没有和 Codex 式 Item 生命周期建立事实关联，取消或异常路径无法保证每个 Tool call 都有明确终态。

## 决策

1. 每个已接受的 Tool call 使用现有 `ToolCall.id` 作为 `tool` Item ID，并在工具批次进入串行执行前持久化/发布 `item_started`。
2. 公共事件 schema v6 新增瞬时 `tool_queued`。运行时先以 `assistant_tool_calls` 持久化完整调用批次，再为每个已接受调用发布携带完整 `ToolCall` 的 `tool_queued`；前端据此按调用 ID 创建 `pending` 活动。整批排队态必须在任一调用开始执行前可见，不新增重复的 JSONL 事实事件。
3. `tool_started`、`tool_output_delta` 和 `tool_completed` 继续使用现有 Tool 事件契约；`tool_started` 按调用 ID 把已有活动原位升级为 `running`，它们的 `callId` 与 Item ID 相同，不新增第二套身份。紧随工具结果发布的 `item_completed(tool)` 是权威终态，按同一 ID 把活动修正为 `completed`、`failed` 或 `cancelled`，避免取消在实时界面被中间失败结果覆盖。
4. `ToolResult` 事实事件和 `tool_completed` 实时事件完成后，再持久化/发布 `item_completed(tool)`。成功结果使用 `completed`，拒绝、执行错误、重复调用保护和跳过使用 `failed`。
5. 当前调用被取消或取消批次中尚未执行的调用使用 `cancelled`。Turn 失败/取消收尾会扫描仍处于活动状态的 Tool Item，作为异常保护，不改变已经闭合的 Item。
6. 恢复投影以持久化的 `assistant_tool_calls` 创建 pending 活动，并由 `tool_started`、`tool_result` 与 `item_completed(tool).status` 修正状态；实时投影只由 `tool_queued` 创建同一活动，再应用相同的启动、结果与 Item 终态。通用 `item_started` 不创建工具展示记录。

## 结果

工具的实时事件和 JSONL 事实事件现在共享稳定 Item 身份和确定性终态。同批工具会先完整显示为等待执行，再严格按 Provider 顺序逐项进入运行态；串行执行、授权策略、工具输出边界和 Provider 历史语义保持不变。后续 Approval、Change、UserInput 和 ContextCompaction 可以复用同一生命周期辅助层。
