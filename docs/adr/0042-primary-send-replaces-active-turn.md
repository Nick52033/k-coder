# ADR 0042：主发送替换活动 Turn

## 状态

已接受，2026-08-18。

## 背景

Rust Thread mailbox 已允许活动 Turn 期间继续提交输入，但主发送按钮此前只调用普通 `turn_start`：新消息进入 pending 队列，当前输出继续运行。只有队列项上的“加入当前对话”会调用 `turn_steer_queued`，独立停止按钮才会调用 `turn_interrupt`。这与用户期望的主交互不一致：点击主发送后，应立即停止当前生成，并让新消息成为同一对话的下一方向。

早期 `P10-021` 曾把“立即发送”描述为提升队列项并取消当前 Turn；该实现随后被 `P10-068` 的精确 Turn 前置条件和 `P10-071` 的原子 queued steer 取代。真正的 steer 必须保持同一 Turn 且不取消，而主发送替换当前工作需要保留清晰的取消终态和新 Turn 审计边界，不能再次把两种语义混为一个操作。

还存在活动 Turn 切换竞态：前端观察到 Turn A 后，请求到达 Rust 时 A 可能已经结束并由 Turn B 接替。如果只按 thread 取消，会误停 B；如果先取消再入队，又可能在窗口关闭或启动失败时丢失用户的新方向。

## 决策

1. 活动 Turn 中的主发送继续调用 `turn_start`，并额外携带可选的 `interruptActiveTurnId`。该 ID 只能来自前端对当前 thread 的活动 Turn 投影，模型输入或权限字段不能影响它。
2. `AppState` 在同一 thread operation gate 内按 operation gate -> active Turn -> mailbox 的固定锁序执行接纳。新 `MailboxTurn` 必须先进入 mailbox，随后才允许关闭 `TurnControl`、取消精确匹配的活动 Turn 及其子智能体。
3. 当前活动 Turn ID 与期望值不一致或已经不存在时，新消息仍按 FIFO 保留，但不得取消任何 Turn。这样过期客户端不会误伤接替者，也不会因竞态丢失已提交输入。
4. 正常异步活动 Turn 已由该 thread 的唯一 mailbox worker 执行，因此 replacement 入队不会创建第二个 worker。旧 Turn 持久化 `turn_cancelled`、`turn_failed` 或 `turn_completed` 并释放活动项后，原 worker 才取出 replacement，后者以新的稳定 `turnId`、`UserMessage` 和 `TurnStarted` 继续同一 thread。
5. 阻塞兼容入口可能持有活动 Turn 却没有 mailbox worker。若 replacement 入队需要创建 worker，该 worker必须等待观察到的兼容 Turn 从活动表释放后再取项，避免新消息提前出队并因 `TurnAlreadyActive` 被拒绝。
6. 前端在发起 `turn_start` 前立即把观察到的 Turn 标记为 cancelling；启动失败时撤销对应的乐观标记，活动 Turn 终态或 replacement 的 `turn_started` 继续通过既有 reducer 收敛状态。主发送的可访问名称保持“发送消息”，活动时的 tooltip 明确为“发送并停止当前生成”。
7. 队列项“加入当前对话”继续只调用 `turn_steer_queued`，保持同一 Turn 且不取消。活动 Turn 正在停止时该动作禁用，因为 `TurnControl` 已关闭。独立停止和错误恢复继续使用 `turn_interrupt(expectedTurnId)`；主发送不得额外组合调用 `turn_interrupt`、`turn_steer` 或 `turn_steer_queued`。

## 影响

- 点击主发送会立即给出停止反馈，当前输出以持久化终态结束，新消息随后成为同一对话中的新 Turn。
- 已进入 mailbox 的 replacement 在活动 Turn 竞态中不会丢失，过期 ID 也不会误停后续 Turn。
- 主发送 replacement 与队列 steer 拥有不同且明确的历史语义：前者产生“旧 Turn 取消 + 新 Turn 开始”，后者产生原 Turn 内的 `turn_steered`。
- 多次快速主发送仍由 mailbox 按 FIFO 保存；它们不能绕过单 thread 单活动 Turn 约束。

## 验证

- Rust 单测覆盖 mailbox worker 正在运行时的精确取消、过期 ID 不取消且消息保留，以及阻塞兼容 Turn 的 worker 延迟启动。
- desktop/narrow Playwright 覆盖主发送携带精确 ID、即时停止态、replacement 用户消息投影、无独立 interrupt/steer 调用，并单独保留 queued steer 回归。
- 执行 Phase 10 规定的前端构建、Rust 格式/检查/测试、`git diff --check` 和真实 `pnpm tauri dev` 工作流验证。
