# ADR 0020：统一 Thread Item 历史投影与分页读取

## 状态

已接受，2026-08-08。

## 背景

k-Coder 过去通过 `ThreadDetail` 同时返回消息、工具活动、时间线、审批、用户输入和变更等多套快照。界面需要再次关联 Turn、修正审批与工具顺序并推断终态，实时事件与刷新恢复因此存在两套投影逻辑。

Codex 将读取契约拆为 `Thread -> Turn -> ThreadItem`，并提供独立的 turns/items 分页：turns 支持 `summary`、`full` 和 `not_loaded` Item 视图，items 支持按 Turn 过滤；`nextCursor` 用于继续当前方向，`backwardsCursor` 用于反向读取锚点之后的新记录。

## 决策

1. 在 `protocol/` 定义版本化 `ThreadTurn`、`ThreadItem`、`ThreadTurnsPage`、`ThreadItemsPage` 和 `ThreadHistorySnapshot`。Item 使用稳定领域 ID，携带类型化载荷、生命周期状态、时间戳及对应的有界 `timelineItems` 展示片段。
2. JSONL 继续是唯一事实来源。`storage/` 在读取时统一投影 UserMessage、AgentMessage、Reasoning、Tool、Approval、UserInput、Change、ContextCompaction 和审计事件，不创建第二套持久化记录，也不为缺少生命周期事实的旧会话反写事件。
3. `read_thread_history` 返回最近 50 个 Turn 的完整工作台快照；`list_thread_turns` 和 `list_thread_items` 的默认页大小为 50、最大为 100。turns 默认按倒序返回摘要 Item，items 默认按正序返回，并可绑定 `turnId`。
4. 游标是版本化的不透明 Base64URL 载荷，绑定 thread、资源类型、稳定事件顺序、锚点 ID、包含语义和 items 的可选 `turnId` 过滤条件。跨线程、跨资源、跨过滤条件、无效和陈旧游标一律拒绝。
5. Approval 请求及解决继续组成一个 Item，但其展示片段由后端排在关联 Tool 之前。前端只从 Item 联合类型投影消息、时间线、待处理交互和变更，不再读取多套快照并修正顺序。
6. `AppState` 在历史查询前继续执行原有崩溃恢复，确保孤立 Turn 和活动 Item 先得到确定性终态，再生成分页结果。
7. 旧 `read_thread` 保留为协议兼容入口。新桌面客户端正常使用统一历史命令，只在连接旧后端或测试兼容夹具时回退。
8. 本阶段不改变阻塞式 `run_turn`、前端消息队列、审批授权、工具执行、Thread mailbox 或 steer/interrupt 语义。

## 结果

刷新恢复和实时展示现在共享稳定的 Item 身份及同一组展示片段。大型线程可以按 Turn 或 Item 有界读取，客户端可以使用反向游标获取锚点之后的新尾部记录。旧 JSONL 即使缺少 `TurnStarted` 或 Item 生命周期事件仍可只读恢复，但不会因此获得伪造的持久化事实。
