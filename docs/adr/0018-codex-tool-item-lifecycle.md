# ADR 0018：Tool Item 生命周期迁移

## 状态

已接受，2026-08-07。

## 背景

Tool call 已经有独立的 `tool_started`、输出增量和 `tool_completed` 事件，工具活动恢复也能从 `assistant_tool_calls`、`tool_started` 和 `tool_result` 推导状态。但这些事件没有和 Codex 式 Item 生命周期建立事实关联，取消或异常路径无法保证每个 Tool call 都有明确终态。

## 决策

1. 每个已接受的 Tool call 使用现有 `ToolCall.id` 作为 `tool` Item ID，并在工具批次进入串行执行前持久化/发布 `item_started`。
2. `tool_started`、`tool_output_delta` 和 `tool_completed` 继续使用现有 Tool 事件契约；它们的 `callId` 与 Item ID 相同，不新增第二套身份。
3. `ToolResult` 事实事件和 `tool_completed` 实时事件完成后，再持久化/发布 `item_completed(tool)`。成功结果使用 `completed`，拒绝、执行错误、重复调用保护和跳过使用 `failed`。
4. 当前调用被取消或取消批次中尚未执行的调用使用 `cancelled`。Turn 失败/取消收尾会扫描仍处于活动状态的 Tool Item，作为异常保护，不改变已经闭合的 Item。
5. 恢复投影以 `item_completed(tool).status` 修正 `ToolActivitySnapshot`，所以 Tool activity 能区分 pending、running、completed、failed 和 cancelled；前端不从生命周期事件创建新的展示记录。

## 结果

工具的实时事件和 JSONL 事实事件现在共享稳定 Item 身份和确定性终态。串行执行、授权策略、工具输出边界和 Provider 历史语义保持不变；后续 Approval、Change、UserInput 和 ContextCompaction 可以复用同一生命周期辅助层。
