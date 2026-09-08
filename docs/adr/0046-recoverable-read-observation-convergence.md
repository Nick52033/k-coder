# ADR 0046：可恢复的重复读取观察收敛

## 状态

已接受，2026-08-22；2026-09-08 修订批次收敛与压缩后上下文重载规则。

## 背景

已安装客户端 `D:\apps\k-coder` 的一次真实 Turn 在首次成功读取 `TData.cs` 后发生自动 Compaction。压缩摘要保留了文件正文中的 `ReturnModel`，但没有保留正文所属路径、文件修订和行范围。后续 Provider 因而再次请求同一文件、同一修订和同一区间：第一次重复请求得到 `read_observation_already_covered`，下一次 Provider 响应仍重复读取，运行时按 ADR 0045 立即返回 `repeated_observation_loop` 并将 Turn 标记为失败。

原策略能够阻止无限空转，但把 Provider 第一次忽略抑制提示直接转化为用户可见失败。对于 DeepSeek 等可能在 Compaction 后失去来源关联的 Provider，这个失败点过于激进；同时，简单放宽次数又会重新引入无界费用和重复副作用风险。

后续真实会话 `b681c99e-4a72-4b9a-8523-762686670a96` 的 Turn `641952dd-ffe3-4983-b9c5-386eb971ccdd` 暴露出两个更精确的问题。CompactionSummary v5 已保留 `src-tauri/src/scheduled_tasks.rs` 的路径、修订和 `1-684` 行来源，但有界摘要只留下稀疏事实，模型不再持有实现所需正文；内存覆盖跟踪器却仍把全部行视为可用。随后同一次 Provider 响应请求 `55-154` 和 `160-239` 两个重叠范围，工具按顺序执行时，第一项把计数推进到 `RecoveryRequired` 并排队纠偏，第二项又立即推进到 `RepeatedLoop`。纠偏只能在下一次 Provider 请求前注入，因此模型从未获得既定的恢复机会。

## 决策

1. `read_file` 继续由 `AgentRuntime` 按“规范化工作区相对路径 + 完整文件修订 + 合并行区间”判断语义重复，不把策略下放到 Provider 适配器。
2. 初次正文读取后的第一次高度重叠 Provider 响应批次仍返回成功的 `read_observation_already_covered`，抑制重复正文。第二次高度重叠批次返回成功的 `read_observation_recovery_required`，同样抑制正文，但让当前 Turn 继续。同一 Provider 响应内串行执行的多个重叠 `read_file` 共享稳定批次标识，对同一路径/修订最多推进一次计数，并全部返回该批次对应的非致命阶段。
3. `read_observation_recovery_required` 为下一次 Provider 请求排队一条带路径/修订身份的宿主 System 纠偏。纠偏携带 JSON 编码的路径、修订和行范围，要求复用已有观察，并禁止通过 `read_file`、Shell 或仓库搜索绕过同一修订的重读边界。该纠偏只作用于下一次请求，最多同时排队 4 条，不写入 JSONL，也不伪造成 User 消息；运行时只有在纠偏文本实际加入 Provider 请求后，才把对应路径/修订标记为已送达。
4. 如果 Provider 收到纠偏后仍对同一修订发起高度重叠重读，运行时返回失败的 `repeated_observation_loop` 并终止 Turn。硬停止以“纠偏已送达”为必要条件，而不是仅依赖工具调用累计次数；同一响应中的后一项调用不能消耗尚未发生的恢复机会。失败原因使用面向用户的中文说明；结构化 `TurnError` 使用同名 code、`Tool` category 和 `retryable = true`。
5. 已成功产出版本化观察的 `read_file` 不再受通用“完全相同参数连续调用”保护提前截断，因为文件版本和行覆盖跟踪器提供了更精确且有界的保护。连续失败或缺少版本元数据的读取仍在第三次同参调用时由通用保护终止；其他工具也保留原有同参调用上限。
6. 从 JSONL 派生 Provider 历史时，宿主根据持久化 `ToolResult.metadata` 为有效 `read_file` 输出添加结构化来源头：

   ```text
   [read_file observation] {"path":"...","fileRevision":"...","startLine":1,"endLine":76}
   ```

   来源头只存在于 Provider-facing 派生历史，原始工具输出和 JSONL 事实不改写。这样来源会进入 `important_tool_observations`、自由摘要和近期工具结果，Compaction 后仍能把正文关联回具体文件修订。
7. 新生成的 `CompactionSummary` 合约版本升为 v5，表示读取来源是压缩语义的一部分。既有 v4 及更早版本继续按兼容路径读取。
8. Compaction 成功持久化时，运行时把当前 Turn 已跟踪的路径/修订标记为可重载。该修订的首个后续高度重叠 Provider 响应批次允许重新返回真实正文，不执行重复正文抑制，并在持久化结果元数据中写入 `observationStatus = read_observation_rehydrated_after_compaction`、`rehydratedAfterCompaction = true` 和 `contentSuppressed = false`。同一批次内的其他重叠读取共享该重载阶段；同一路径/修订在整个 Turn 生命周期内最多获得一个重载批次，重复自动或手动续跑 Compaction 不得刷新额度。新文件修订继续建立独立状态。

## 安全与边界

- 来源头只使用工具处理器持久化的结果元数据。模型参数中的路径、修订或权限字段不能替代该事实，也不能扩大工作区访问能力。
- 注入 System 纠偏前，路径和修订使用 JSON 字符串编码，避免特殊文件名改变指令结构。
- 恢复只允许当前 Turn 再获得一次 Provider 决策机会，不重新执行写入，不自动批准工具，也不改变 Shell、审批、取消和进程树策略。
- 压缩后重载仍逐次执行原始 `read_file` 的路径规范化、工作区逃逸、文件大小、编码、范围和输出上限检查；它只改变成功结果是否抑制正文，不缓存文件、不扩大范围，也不接受模型提供的修订作为授权事实。
- 重载额度绑定当前 Turn 内宿主观察到的路径/修订状态，并以“每修订至多一个 Provider 批次”为生命周期上限；重复 Compaction 不能把它变成无限重读旁路。
- `read_observation_recovery_required` 和最终失败仍以有界工具结果进入 JSONL，保留审计链；临时 System 纠偏不作为用户事实持久化。
- Compaction 继续只改变 Provider 派生历史，不删除或重写原始事件。文件修订变化后建立新的覆盖状态，允许对真实新内容进行定向验证。

## 备选方案

- 保持第二次重读立即失败：边界最简单，但已证明会把一次可纠正的 Provider 偏航暴露成用户失败。
- 只提高重复次数：不能告诉 Provider 为什么必须改变动作，也会把失败延后为更多无效调用。
- 在 DeepSeek 适配器内特殊重试：会让模型协议层拥有 Turn 协调策略，并使其他 Provider 的同类行为无法复用。
- 把纠偏保存为 User 消息：会污染真实用户意图、后续 Compaction 和会话审计，因此拒绝。

## 影响

截图对应的第二次重复读取不再直接结束任务。同一响应即使包含多个重叠读取，也只推进一个恢复阶段；Provider 会先收到一次明确、受信任且不可通过其他读取工具绕过的纠偏机会，持续忽略纠偏时才在后续响应中确定性失败，不会恢复成无限循环。

Compaction 后摘要只有来源而缺少实现正文时，Provider 可以在一个有界批次内重新取得该修订正文，而不会被过时的内存覆盖信息永久阻断。代价是每个已观察文件修订在一次 Turn 中最多增加一个重载批次的上下文和读取成本；重复压缩不增加额度。

Provider-facing 读取结果增加一个很小的结构化来源头，并计入上下文估算。代价是 v5 摘要比 v4 多保留少量路径和修订文本，但能够避免正文与来源分离造成的重复读取，整体上下文和调用成本更低。

## 验证

- 跟踪器单元测试覆盖 `NewCoverage -> AlreadyCovered -> RecoveryRequired -> RepeatedLoop`、新修订重置和变化范围。
- 批次回归覆盖同一 Provider 响应中的多个重叠范围只推进一次状态，达到 `RecoveryRequired` 后同批后一项不能触发硬停止；只有纠偏实际进入下一请求后，后续重读才进入 `RepeatedLoop`。
- Compaction 回归覆盖“压缩后继续”的首个重叠批次返回真实正文和重载元数据，并覆盖第二次 Compaction 不重新开放同一路径/修订额度。
- 运行时回归覆盖完全相同参数绕过通用同参门、第二次重读成功恢复、下一请求包含 System 纠偏、无伪造 User 事件以及 Provider 最终完成。
- 硬停止回归覆盖插入其他工具和变化区间仍累计，纠偏后再次重读返回中文 `repeated_observation_loop`。
- Provider 历史与 Compaction 回归覆盖路径、修订、起止行在 v5 渲染摘要中保留，非读取或旧版无元数据结果保持原输出。
- 质量门槛执行 `pnpm build`、Rust 格式检查、`cargo check`、`cargo test` 和隔离的真实 `pnpm tauri dev` 桌面验证。
