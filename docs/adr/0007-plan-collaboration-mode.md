# ADR 0007: Plan 协作模式与 request_user_input 工具

## 状态

已采纳

## 背景

k-Coder 此前的 `agent_mode` 字段已有 craft/ask/plan 三种模式，但实现是简陋的 prompt 注入——
仅向 `runtime_instructions` 追加一段文字描述，没有硬约束来阻止模型在 Plan 模式下执行变更操作，
也没有让模型向用户主动提问的能力。

OpenAI Codex 的 Plan 模式实现值得借鉴：

1. Plan 是一等协作模式（`ModeKind::Plan`），配合 `plan_mask` 切换预设和工具集。
2. `request_user_input` 工具让模型在规划阶段向用户提问（1-3 个多选问题），阻塞式等待回答。
3. Plan 模式下禁止变更类工具，只允许只读探索 + 提问。
4. 最终用 `<proposed_plan>` 块包裹计划输出，供客户端特殊渲染。

## 决策

### 1. 引入 `AgentMode` 枚举

在 `protocol/mod.rs` 中新增 `AgentMode` 枚举（`Craft`/`Ask`/`Plan`），替代原来的字符串匹配。
每个模式通过 `allowed_tools()` 声明其可用工具子集。

### 2. Plan/Ask 模式工具限制

在 `commands/mod.rs` 的 `run_turn` 中，Plan/Ask 模式下调用 `ToolRegistry::restricted_to()`
把工具限制为只读子集。这是硬约束，不依赖模型遵守 prompt。

- **Plan 模式**：`list_directory`、`read_file`、`search_repository`、`recall_memory`、`request_user_input`
- **Ask 模式**：上述 + `update_plan`
- **Craft 模式**：全部工具

### 3. `request_user_input` 工具

新增 `request_user_input` 工具（`advanced/request_user_input.rs`），让模型向用户提问：
- 1-3 个问题，每个问题 2-4 个互斥选项
- 由 `AgentRuntime::execute_tool_call` 拦截，不经过普通 dispatch
- 通过 `UserInputManager`（类似 `ApprovalManager`）阻塞等待前端回答
- 超时 10 分钟自动取消

### 4. `UserInputManager`

在 `policy/mod.rs` 中新增 `UserInputManager`，复用 `ApprovalManager` 的 oneshot channel 模式。
`AppState` 持有一个 `Arc<UserInputManager>` 实例，`AgentRuntime` 通过 builder 注入。

### 5. Plan 模式指令模板

在 `src-tauri/templates/plan_mode.md` 和 `ask_mode.md` 中定义模式专用的系统指令，
借鉴 Codex 的三阶段流程（环境勘探 → 意图沟通 → 实现沟通），通过 `include_str!` 编译进二进制。

### 6. 前端渲染

- `UserInputCard` 组件渲染提问卡片，用户选择选项后调用 `resolve_user_input` 命令
- `renderMessageText` 函数从消息文本中提取 `<proposed_plan>` 块并特殊渲染
- `update_plan` 的持久计划与工具活动按 `turnId` 内嵌到对应助手消息，刷新或重启后由会话投影恢复关联
- 右侧工作台只保留文件和 Git；没有最终助手消息的失败或取消 Turn 以独立活动块留在对话时间线

## 后果

### 正面

- Plan 模式现在有硬约束，模型无法绕过工具限制执行变更操作
- 模型可以主动向用户提问，避免在模糊需求下盲目规划
- `<proposed_plan>` 块让计划输出有清晰的视觉边界

### 负面

- `request_user_input` 是阻塞式的，如果用户不回答会占用 turn 直到超时
- Plan 模式的工具限制可能导致模型在探索时缺少某些工具（如 `run_command`）

## 参考

- OpenAI Codex `collaboration-mode-templates/templates/plan.md`
- OpenAI Codex `core/src/tools/handlers/request_user_input.rs`
- k-Coder ADR 0005（bounded-advanced-agent-runtime）
