# ADR 0019：交互类 Item 生命周期迁移

## 状态

已接受，2026-08-07。

## 背景

ADR 0016 至 0018 已为助手回复、安全 Reasoning 摘要和 Tool call 建立统一的 `item_started` / `item_completed` 契约。审批、用户提问、文件变更和上下文压缩仍只有各自领域事件，异常退出后可能留下无法判定终态的活动记录，客户端也无法把这些交互稳定地关联到同一个 Item。

## 决策

1. Approval 使用 `ApprovalRequest.id`，UserInput 使用 `UserInputRequest.id`，Change 使用 `ChangeSet.id`，ContextCompaction 使用外层 `ContextCompacted` 事实事件的 `event_id` 作为稳定 Item ID，不创建第二套身份。
2. Approval 和 UserInput 在请求事实事件之前启动 Item，在解决事实事件持久化并发布后闭合。审批通过和已回答闭合为 `completed`；拒绝、超时和跳过闭合为 `failed`；取消闭合为 `cancelled`。
3. Change 在应用结果准备写入审计事实前启动 Item。`ChangeApplied` 成功持久化并发布后闭合为 `completed`；审计写入失败时先回滚工作区变更，再闭合为 `failed`。模型参数和变更载荷仍不能扩大授权。
4. 自动 ContextCompaction 在 `ContextCompacted` 之前启动 Item，事实事件持久化并发布后闭合为 `completed`。手动压缩复用同一 JSONL 生命周期，但当前命令没有实时 Publisher，因此只写事实事件且 `turn_id` 为空。
5. Turn 成功或失败收尾会把仍活动的非消息 Item 闭合为 `failed`，取消收尾闭合为 `cancelled`。应用崩溃恢复绕过 `AgentRuntime`，因此 `AppState::read_thread` 在补写交互取消结果后，也会把该 Turn 中实际启动但未完成的 Item 闭合为 `cancelled`。
6. 旧 schema 记录没有 `ItemStarted` 时不补造 `ItemCompleted`。原有请求、解决、变更和压缩事件继续按旧投影恢复，所以本次迁移不提升协议或存储 schema 版本。
7. 请求和解决事件在时间线上仍保留各自的展示 ID，避免同一时间线出现重复 React key；生命周期事实使用领域稳定 ID，不改变既有 UI 投影。

## 结果

所有当前公共 Item 类型都拥有确定性身份和终态，实时流、JSONL 审计及崩溃恢复遵循相同闭合规则。前端继续从具体领域事件投影交互，不从通用生命周期事件重复创建审批、提问、变更或压缩步骤。统一 Thread Item 历史和有界分页读取已由 ADR 0020 建立。
