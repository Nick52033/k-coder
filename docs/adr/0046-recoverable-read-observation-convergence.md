# ADR 0046：可恢复的重复读取观察收敛

## 状态

已接受，2026-08-22。

## 背景

已安装客户端 `D:\apps\k-coder` 的一次真实 Turn 在首次成功读取 `TData.cs` 后发生自动 Compaction。压缩摘要保留了文件正文中的 `ReturnModel`，但没有保留正文所属路径、文件修订和行范围。后续 Provider 因而再次请求同一文件、同一修订和同一区间：第一次重复请求得到 `read_observation_already_covered`，下一次 Provider 响应仍重复读取，运行时按 ADR 0045 立即返回 `repeated_observation_loop` 并将 Turn 标记为失败。

原策略能够阻止无限空转，但把 Provider 第一次忽略抑制提示直接转化为用户可见失败。对于 DeepSeek 等可能在 Compaction 后失去来源关联的 Provider，这个失败点过于激进；同时，简单放宽次数又会重新引入无界费用和重复副作用风险。

## 决策

1. `read_file` 继续由 `AgentRuntime` 按“规范化工作区相对路径 + 完整文件修订 + 合并行区间”判断语义重复，不把策略下放到 Provider 适配器。
2. 初次正文读取后的第一次高度重叠重读仍返回成功的 `read_observation_already_covered`，抑制重复正文。第二次高度重叠重读返回成功的 `read_observation_recovery_required`，同样抑制正文，但让当前 Turn 继续。
3. `read_observation_recovery_required` 为下一次 Provider 请求排队一条宿主生成的 System 纠偏。纠偏携带 JSON 编码的路径、修订和行范围，要求复用已有观察，并禁止通过 `read_file`、Shell 或仓库搜索绕过同一修订的重读边界。该纠偏只作用于下一次请求，最多同时排队 4 条，不写入 JSONL，也不伪造成 User 消息。
4. 如果 Provider 收到纠偏后仍对同一修订发起第三次高度重叠重读，运行时返回失败的 `repeated_observation_loop` 并终止 Turn。失败原因使用面向用户的中文说明；结构化 `TurnError` 使用同名 code、`Tool` category 和 `retryable = true`。
5. 已成功产出版本化观察的 `read_file` 不再受通用“完全相同参数连续调用”保护提前截断，因为文件版本和行覆盖跟踪器提供了更精确且有界的保护。连续失败或缺少版本元数据的读取仍在第三次同参调用时由通用保护终止；其他工具也保留原有同参调用上限。
6. 从 JSONL 派生 Provider 历史时，宿主根据持久化 `ToolResult.metadata` 为有效 `read_file` 输出添加结构化来源头：

   ```text
   [read_file observation] {"path":"...","fileRevision":"...","startLine":1,"endLine":76}
   ```

   来源头只存在于 Provider-facing 派生历史，原始工具输出和 JSONL 事实不改写。这样来源会进入 `important_tool_observations`、自由摘要和近期工具结果，Compaction 后仍能把正文关联回具体文件修订。
7. 新生成的 `CompactionSummary` 合约版本升为 v5，表示读取来源是压缩语义的一部分。既有 v4 及更早版本继续按兼容路径读取。

## 安全与边界

- 来源头只使用工具处理器持久化的结果元数据。模型参数中的路径、修订或权限字段不能替代该事实，也不能扩大工作区访问能力。
- 注入 System 纠偏前，路径和修订使用 JSON 字符串编码，避免特殊文件名改变指令结构。
- 恢复只允许当前 Turn 再获得一次 Provider 决策机会，不重新执行写入，不自动批准工具，也不改变 Shell、审批、取消和进程树策略。
- `read_observation_recovery_required` 和最终失败仍以有界工具结果进入 JSONL，保留审计链；临时 System 纠偏不作为用户事实持久化。
- Compaction 继续只改变 Provider 派生历史，不删除或重写原始事件。文件修订变化后建立新的覆盖状态，允许对真实新内容进行定向验证。

## 备选方案

- 保持第二次重读立即失败：边界最简单，但已证明会把一次可纠正的 Provider 偏航暴露成用户失败。
- 只提高重复次数：不能告诉 Provider 为什么必须改变动作，也会把失败延后为更多无效调用。
- 在 DeepSeek 适配器内特殊重试：会让模型协议层拥有 Turn 协调策略，并使其他 Provider 的同类行为无法复用。
- 把纠偏保存为 User 消息：会污染真实用户意图、后续 Compaction 和会话审计，因此拒绝。

## 影响

截图对应的第二次重复读取不再直接结束任务。Provider 会收到一次明确、受信任且不可通过其他读取工具绕过的纠偏机会；能利用已有观察的 Provider 可以继续修改、验证或给出最终答复。持续忽略纠偏的 Provider 仍会在下一次重复读取时确定性失败，不会恢复成无限循环。

Provider-facing 读取结果增加一个很小的结构化来源头，并计入上下文估算。代价是 v5 摘要比 v4 多保留少量路径和修订文本，但能够避免正文与来源分离造成的重复读取，整体上下文和调用成本更低。

## 验证

- 跟踪器单元测试覆盖 `NewCoverage -> AlreadyCovered -> RecoveryRequired -> RepeatedLoop`、新修订重置和变化范围。
- 运行时回归覆盖完全相同参数绕过通用同参门、第二次重读成功恢复、下一请求包含 System 纠偏、无伪造 User 事件以及 Provider 最终完成。
- 硬停止回归覆盖插入其他工具和变化区间仍累计，纠偏后再次重读返回中文 `repeated_observation_loop`。
- Provider 历史与 Compaction 回归覆盖路径、修订、起止行在 v5 渲染摘要中保留，非读取或旧版无元数据结果保持原输出。
- 质量门槛执行 `pnpm build`、Rust 格式检查、`cargo check`、`cargo test` 和隔离的真实 `pnpm tauri dev` 桌面验证。
