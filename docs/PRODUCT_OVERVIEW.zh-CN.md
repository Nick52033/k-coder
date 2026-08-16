# Codex 项目功能与对话机制产品文档

> 文档性质：基于当前代码的产品与架构说明，而不是对未来路线图的承诺。
> 主要实现位于 `codex-rs`，本文将“对话”统一称为 thread，将一次可持续执行的用户请求称为 turn。

## 1. 产品定位

Codex 是一个运行在本地计算机上的 coding agent。它不是只返回代码片段的聊天机器人，而是一个可以读取工作区、调用工具、修改文件、执行命令、等待用户审批，并把执行过程持续记录为可恢复线程的代理运行时。

从产品角度看，Codex 同时提供三种使用面：

| 使用面 | 主要入口 | 适合场景 | 输出形态 |
| --- | --- | --- | --- |
| 交互式 CLI/TUI | `codex`、`codex-rs/tui` | 人在终端中持续协作 | 富文本转录、流式状态、审批交互 |
| 非交互式执行 | `codex exec`、`codex-rs/exec` | CI、脚本、批处理、代码审查 | 人类可读输出或 JSONL 事件 |
| App Server | `codex app-server`、`codex-rs/app-server` | VS Code、桌面端、其他客户端 | 双向 JSON-RPC、线程/回合/条目通知 |

三种使用面共享 `codex-core` 的会话、上下文、工具、安全和持久化逻辑；差异主要在输入采集和事件渲染。

## 2. 当前功能版图

### 2.1 编程代理能力

- 读取工作区文件、目录、项目说明和 `AGENTS.md` 指令。
- 通过 `apply_patch` 创建、修改、删除文件，并在转录中呈现文件变更和 diff。
- 通过统一执行层运行 shell 命令，支持前台命令、后台终端和 stdin 交互。
- 使用模型推理完成代码解释、实现、调试、重构和测试建议。
- 运行专门的 code review 流程，可审查未提交变更、相对基线分支的变更、指定 commit 或自定义审查目标。
- 支持 Plan/协作模式、结构化最终输出（JSON Schema）以及中途追加指令。

### 2.2 工具和外部能力

工具由 `ToolRouter` 在每个 sampling step 重新确定，来源包括：

- Core 内置工具：shell、apply patch、查看图片、请求用户输入、请求权限、计划更新、上下文剩余量、等待环境等。
- Responses API 托管能力：web search、image generation 等。
- MCP server 工具、MCP resources 和 elicitation。
- 插件和 skills 提供的工具、说明、依赖和连接器（apps）。
- 动态工具（`dynamicTools`）和 code mode 工具。
- 多代理工具：spawn、send message/input、wait、resume、close 等。

工具并不是“模型直接执行进程”。模型只产生结构化 call；Core 根据工具注册表、权限策略和环境快照进行校验、审批、执行和结果回写。

### 2.3 安全与治理

- 权限配置同时支持 legacy sandbox 和命名 permission profile。
- 文件系统权限可以是只读、工作区可写、精确读写/拒绝 carve-out、全访问或由外部宿主负责隔离。
- 网络权限独立于文件权限，可限制网络或由外部 sandbox 管理。
- `AskForApproval` 决定何时需要用户批准；审批请求通过事件回传，再由客户端提交决定。
- 运行时可以按环境和 turn 追加权限，追加权限在当前 turn 的 `TurnState` 中合并记录。
- Guardian/AutoReview、hooks 和托管配置要求可在工具执行前后介入，形成组织级约束。

### 2.4 连续性和协作

- 线程可以 start、resume、fork、archive、delete；可使用 ephemeral thread 只保存在内存。
- 支持历史分页、线程搜索、turn/items 查询和线程元数据（名称、分组、git 信息等）。
- 用户可以在活动 turn 中 steer；也可以 interrupt，再使用原 turn id recover。
- 线程可以维护长期 goal、memory mode、token budget 和自动唤醒队列。
- 子代理线程具有 parent/root lineage；子代理消息进入父线程 mailbox，可在当前或后续 turn 消费。

### 2.5 多模态和实时能力

- 用户输入可包含文本、图片、音频、skills 和结构化 mention。
- 模型输出可以包含文本、reasoning summary、图片查看、图片生成、web search 和工具结果。
- Core 另有 realtime conversation 通道，支持文本、音频、speech、SDP 和 voice 列表事件；它与普通 Responses turn 的生命周期不同。

## 3. 系统架构

```text
┌──────────────────────────────────────────────────────────────┐
│ 客户端层                                                     │
│ TUI / codex exec / VS Code 等 App Server client              │
└───────────────┬──────────────────────────────────────────────┘
                │ AppCommand、JSON-RPC、事件订阅
┌───────────────▼──────────────────────────────────────────────┐
│ 接口层                                                       │
│ codex-tui │ codex-exec │ codex-app-server │ protocol v1/v2  │
└───────────────┬──────────────────────────────────────────────┘
                │ Op / TurnInputRequest / Event
┌───────────────▼──────────────────────────────────────────────┐
│ Codex Core                                                    │
│ ThreadManager → CodexThread → Session → SessionTask          │
│ ContextManager、TurnContext、ToolRouter、hooks、approvals    │
└───────────────┬──────────────────────────────────────────────┘
                │ Prompt + ToolSpec + Responses metadata
┌───────────────▼──────────────────────────────────────────────┐
│ 模型与执行基础设施                                           │
│ Responses API (WebSocket 优先，HTTP fallback)                 │
│ exec-server / sandbox / MCP / plugins / skills / thread-store │
└──────────────────────────────────────────────────────────────┘
```

关键代码位置：

| 领域 | 代码位置 | 作用 |
| --- | --- | --- |
| 线程生命周期 | `codex-rs/core/src/thread_manager.rs`、`codex_thread.rs` | 创建、恢复、分叉线程；暴露 `CodexThread` |
| 会话调度 | `codex-rs/core/src/session/session.rs`、`state/turn.rs` | 一个线程最多一个活动任务；取消、审批、等待者和 pending input |
| turn 输入 | `codex-rs/core/src/session/turn_input.rs`、`input_queue.rs` | 决定 start、steer、idle start、recover，维护队列/mailbox |
| turn 执行 | `codex-rs/core/src/tasks/regular.rs`、`session/turn.rs` | 采样、工具执行、重试、自动压缩、完成判断 |
| 上下文 | `codex-rs/core/src/context_manager`、`context`、`session/step_context.rs` | 历史、上下文片段、世界状态、工具快照和 token 预算 |
| 模型连接 | `codex-rs/core/src/client.rs`、`client_common.rs` | 构造 Prompt，流式 Responses API，WS/HTTP fallback |
| 协议 | `codex-rs/protocol/src/protocol.rs`、`models.rs`、`items.rs` | `Op`、`EventMsg`、`ResponseItem`、`TurnItem` |
| App Server | `codex-rs/app-server`、`app-server-protocol/src/protocol/v2` | JSON-RPC 请求、通知、类型和客户端订阅 |
| 持久化 | `codex-rs/thread-store`、`rollout`、`state` | rollout、线程元数据、SQLite 投影和历史分页 |
| 工具 | `codex-rs/core/src/tools`、`ext/*`、`codex-mcp` | 工具注册、暴露策略、执行器和扩展 |

## 4. 对话的核心数据模型

### 4.1 Thread：可恢复的对话容器

Thread 是用户与代理之间的长期容器。它保存 thread id、线程设置、模型/provider、工作目录或环境、权限 profile、协作模式、history mode、来源和持久化引用。一个 thread 可以有多个 turn，也可能分叉出子 thread。

`CodexThread` 是对外的双向 conduit：一侧接收 `Op`/turn input，另一侧通过 `SessionIo` 输出 `Event`。它本身不承担模型推理，而是把操作交给线程内的 `Session`。

### 4.2 Turn：一次可持续执行的工作回合

Turn 通常从用户消息开始，以最终 agent message、错误或中断结束。一个 turn 不一定只有一次模型请求：模型调用工具、工具返回结果、用户 steer、hooks 要求继续、上下文压缩都可能在同一个 turn 中产生后续 sampling。

Core 为每个 turn 创建 `TurnContext`，其中包含：

- 当前模型信息、reasoning effort/summary、personality 和 collaboration mode。
- 当前环境、cwd、workspace roots、shell snapshot 和权限 profile。
- developer instructions、基础模型指令、`AGENTS.md` 和插件/skills 快照。
- MCP binding、动态工具、结构化输出 schema、Responses metadata 和 telemetry。

### 4.3 Item：可持久化的对话事实

模型上下文使用 `ResponseItem` 表示事实和控制项，常见类型包括：

- `Message`：user/developer/assistant 消息。
- `Reasoning`：reasoning summary 或加密 reasoning content。
- `FunctionCall` / `CustomToolCall` 与对应 output。
- `LocalShellCall`、`WebSearchCall`、`ImageGenerationCall`。
- `Compaction`、`ContextCompaction` 等上下文边界。
- `AgentMessage`：代理之间的消息。

面向客户端和 UI 的公开投影是 `TurnItem`，它把上述事实映射成 UserMessage、AgentMessage、Reasoning、Plan、CommandExecution、McpToolCall、FileChange、SubAgentActivity、ContextCompaction 等可展示条目。

### 4.4 Event：运行时进度和交互信号

`Event { id, msg }` 按 submission/turn 关联。`EventMsg` 覆盖 TurnStarted/TurnComplete、AgentMessage delta、Reasoning delta、item lifecycle、命令输出、审批请求、用户输入请求、MCP/web/image 进度、token count、diff、错误和中断等。

因此，历史 Item 描述“已经发生并可恢复的事实”，Event 描述“现在正在发生的进度或等待客户端动作”。客户端应以 item completed/turn completed 等生命周期事件收敛状态，不应只依赖文本 delta。

## 5. 主体对话机制：从输入到回复

下面是一次普通编码请求的完整路径。

```text
用户输入
  │
  ├─ TUI: ChatWidget → AppCommand::UserTurn
  ├─ exec: InitialOperation::UserTurn
  └─ app-server: turn/start(threadId, input)
          │
          ▼
CodexThread / app-server 转成 TurnInputRequest
          │
          ▼
输入仲裁：start-or-steer / start-if-idle / steer-only
          │
          ├─ 无活动 turn：创建 TurnContext，启动 RegularTask
          ├─ 有活动且可 steer：写入当前 turn pending input
          └─ 不满足前置条件：NotSubmitted，不记录输入
          │
          ▼
Session.start_task → RegularTask::run → run_turn 循环
          │
          ▼
上下文更新 + hooks + skills/plugins/MCP 快照 + tool router
          │
          ▼
Prompt(history + context + current input + tools + schema)
          │
          ▼
Responses API 流式采样（WS 优先，HTTP fallback）
          │
          ├─ assistant/reasoning 文本：实时事件 + 写入 history
          ├─ function/custom tool call：审批/执行 → output 写入 history
          ├─ end_turn=false：继续下一次 sampling
          ├─ token 超限：自动 compaction 后继续
          └─ end_turn=true 且无 pending input：完成 turn
          │
          ▼
TurnComplete + token usage + diff + rollout flush
```

### 5.1 输入采集与 start/steer 仲裁

Core 暴露三种语义明确的提交方式：

- `start_or_steer_turn`：空闲时启动，活动 regular turn 可 steer。
- `start_turn_if_idle`：只允许空闲启动，适合自动唤醒、队列工作和 recover。
- `steer_turn`：要求 `expected_turn_id` 与当前活动 turn 完全一致，避免竞态下把消息送入错误 turn。

提交结果不是“模型已经回复”，而是 `Started`、`Steered` 或 `NotSubmitted`。只有输入被接受后，Core 才应用持久线程设置和 start-only 选项；如果被拒绝，设置和输入都不应落入历史。

`TurnInput` 有三类：用户输入、注入的 `ResponseItem`、代理间通信。活动 turn 的 steer 先进入 `TurnState.pending_input`，在合适的 sampling 边界被取出；子代理消息另外进入 session mailbox，并根据 `trigger_turn` 和 mailbox delivery phase 决定并入当前 turn 还是下一 turn。

### 5.2 TurnContext 和 step context

`TurnContext` 是一个 turn 的稳定配置；`StepContext` 是一次 sampling request 的精确视图。每个 step 会重新捕获环境、MCP 连接、已加载 `AGENTS.md`、能力 roots 和 `ToolRouter`，保证“模型看到的工具”和“实际执行的工具”来自同一个快照。

这一区分解决了两个产品问题：

1. turn 内可以安全处理 steer、MCP refresh、环境连接变化而不污染既有 turn 配置。
2. 工具、上下文和权限在一次请求中保持一致，降低工具列表与执行结果不匹配的风险。

### 5.3 上下文构建

在第一次采样前，Core 会依次处理：

1. 读取和合并基础指令、developer instructions、`AGENTS.md`、personality 和 collaboration mode 指令。
2. 根据当前环境和工作区生成 world state；变化以 full snapshot 或 patch 形式写入上下文。
3. 解析用户的 skill/plugin/app/tool mention，加载所需 skills、插件能力和 MCP server。
4. 运行 `UserPromptSubmit` 等 hooks；hook 可以追加上下文、阻断输入或要求后续继续。
5. 把本轮输入、上下文片段和工具注入 `ContextManager`。

`ContextualUserFragment` 用于把系统生成的环境、权限、时间、插件、hook 或多代理信息注入模型；这些片段可以合并成 developer/user message，但会被标记和过滤，避免把内部上下文误当作普通用户历史。`ContextManager` 在发送前做归一化：补齐 call/output 配对、删除孤立 output、按模型 input modalities 去掉不支持的图片/音频，并截断过大的工具输出。

### 5.4 Prompt 和模型请求

每次 sampling 的 `Prompt` 由四部分构成：

- `input`：经过 `ContextManager::for_prompt` 归一化的历史和本轮输入。
- `tools`：当前 step 的模型可见 `ToolSpec`，而不是所有注册工具。
- `base_instructions`：模型内置 instructions、产品默认提示词和有效 personality。
- `output_schema`：可选的结构化最终输出约束。

`ModelClientSession` 以 turn 为生命周期，允许同一 turn 的多次 sampling 复用 WebSocket 和 sticky routing 状态。Responses WebSocket 可用且健康时优先使用；失败后切换到 HTTP Responses API，并在 provider 允许时执行重试和模型 fallback。

### 5.5 流式事件归约

模型流被映射成 `ResponseEvent`，`session/turn.rs` 负责把它归约为内部状态和协议事件：

- `OutputItemAdded/Done` 建立和完成一个 `TurnItem`。
- `OutputTextDelta` 转成 `AgentMessageContentDelta` 或计划 delta。
- reasoning summary/content delta 转成对应 reasoning 事件。
- tool argument delta 可驱动工具参数 diff 展示。
- `Completed` 更新 token usage、响应 id、是否需要 follow-up，并触发后续工具处理。

已完成的 ResponseItem 会先记录到 `ContextManager` 和 rollout，再通过 app-server/TUI 的映射层转成公开 item。这样 UI 可以实时渲染，后续请求也能使用同一份历史。

### 5.6 工具调用闭环

当模型输出 `FunctionCall`、`CustomToolCall` 或可执行的 `ToolSearchCall` 时：

1. `ToolRouter` 根据 namespace/name 找到注册的 runtime。
2. Core 检查工具暴露模式（direct/deferred/code-mode）、当前环境、审批策略和权限 profile。
3. 需要用户动作时发送 `ExecApprovalRequest`、`ApplyPatchApprovalRequest`、`RequestPermissions`、`RequestUserInput` 或 MCP elicitation 事件，并在 `TurnState` 中保存等待者。
4. 工具执行器运行 shell、patch、MCP、web/image、插件或子代理操作，持续发送 begin/output/end 事件。
5. 结果编码为 `FunctionCallOutput`/`CustomToolCallOutput` 等 ResponseItem，写入历史。
6. 如果模型的 response 标记 `end_turn=false`，或者工具结果要求继续，Core 使用更新后的 history 发起下一次 sampling。

工具执行因此是对话的一部分，而不是对话外的副作用。一个用户 turn 可以包含多次“模型采样 → 工具执行 → 结果回写 → 模型采样”。

### 5.7 Turn 结束、继续与中断

普通结束条件是：本次响应没有需要执行的工具、模型没有要求 follow-up、没有待处理 steer/mailbox input、stop hooks 未要求继续。随后 Core 发送 TurnComplete，记录最终 assistant message、token usage、turn diff，并 flush thread store。

以下情况会继续当前 turn：

- 模型显式返回 `end_turn=false`。
- 工具输出要求下一次采样。
- 用户在活动 turn 中 steer。
- stop hook、计划或内部队列追加了输入。
- 子代理的 trigger-turn 消息在当前 turn 仍可接收。

`turn/interrupt` 或 `Op::Interrupt` 取消 turn cancellation token，清理等待者并发出 TurnAborted；它不会自动杀死后台 terminal。需要时再调用 `CleanBackgroundTerminals`。`recover_turn_if_idle` 使用已经记录的 turn id 恢复中断 turn，不创建新的用户消息边界。

## 6. 上下文窗口、压缩与回滚

### 6.1 Token 预算

Core 同时追踪：完整活动上下文 token、自动压缩范围 token、模型完整 context window 和 fallback buffer。一次 sampling 完成后计算 token 状态；达到阈值且需要继续时，先执行自动 compaction，避免把模型请求推过硬上限。

### 6.2 Compaction

压缩有三种实现：本地 summarization、provider 的 remote compaction v1、remote compaction v2。手动 `Compact` 和自动 compaction 共用生命周期 hooks 与 `ContextCompaction` item。压缩流程会：

1. 发送 pre-compact hooks，并记录 compaction boundary。
2. 用当前 history 生成摘要/替代历史。
3. 在合适的位置重新注入初始上下文、world state 和必要的 developer instructions。
4. 替换 `ContextManager` history，更新 token usage 和 compaction window。
5. 发出 `ContextCompacted`，执行 post-compact hooks。

压缩并非无损缓存：代码中明确提示长线程和多次压缩可能降低模型准确性；产品上应优先使用小而聚焦的 thread，必要时 fork 或新建 thread。

### 6.3 Rollback

`ThreadRollback` 只回滚模型可见历史中的最后 N 个 user turns，不回滚磁盘上的代码修改。客户端如果需要撤销文件，必须单独使用 git 或其他工作区恢复机制。回滚会重放持久历史、更新 token 统计，并使后续 turn 重新建立必要上下文基线。

## 7. 持久化、恢复和分叉

`thread-store` 抽象屏蔽本地和远程存储差异。当前本地实现以 rollout 记录原始事件/条目，并通过 SQLite 保存线程元数据和历史投影；paginated history 可以按 turns/items cursor 查询，而不必一次加载完整转录。

关键行为：

- turn 完成前会 flush rollout；写入失败会发送 warning 并继续重试，避免静默丢失上下文。
- `thread/resume` 恢复 thread settings、历史、token usage 和必要的运行时状态。
- `thread/fork` 复制指定历史边界，生成新 thread id；源 thread 活动时会按中断边界处理，不把未标记的半截 turn 当成完整历史。
- paginated thread 同时有 durable history 和 live notification；单个 app-server 进程对已加载线程拥有写入所有权。
- ephemeral thread 不写入持久存储，适合 review 子线程、临时实验和隔离任务。

## 8. App Server 产品接口

App Server 是面向富客户端的控制面：使用 JSON-RPC 2.0 语义，默认通过 stdio JSONL，也支持 WebSocket 和 Unix socket。连接必须先 `initialize`/`initialized`，之后才能操作线程。

最小生命周期：

```text
initialize
  → thread/start | thread/resume | thread/fork
  → turn/start
  → turn/started + item/* + tool progress + deltas
  → turn/completed
```

核心方法：

- Thread：`thread/start`、`thread/resume`、`thread/fork`、`thread/list`、`thread/read`、`thread/archive`、`thread/delete`、`thread/turns/list`、`thread/items/list`。
- Turn：`turn/start`、`turn/steer`、`turn/interrupt`；另有 compact、review、shell command、goal、memory 等扩展方法。
- 工具和治理：审批响应、`tool/requestUserInput`、MCP server/tool/resource、skills/plugins、config。
- 事件：`turn/started`、`item/started`、`item/completed`、各种 delta、命令/MCP/tool progress、`turn/completed`。

服务器使用有界队列做背压；请求入口饱和时返回可重试的 `-32001 Server overloaded; retry later.`。客户端应按方法名精确处理订阅和 opt-out，不要假设未知通知一定不存在。

## 9. 客户端体验映射

### TUI

TUI 的 `ChatWidget` 负责编辑输入、显示转录、审批和快捷命令；`AppCommand::UserTurn` 交给 app event routing。提交时优先尝试 steer 当前活动 turn，遇到竞态再根据服务端 turn id 重试或退回排队/新 turn。app-server 通知再被映射回 TUI 的历史 cell、状态指示器、diff 和审批面板。

### Exec

`codex exec` 通过 in-process app-server client 驱动同一套 thread/turn API。它支持新 turn、resume、fork 和 review，并将公开 item 映射为人类可读文本或 JSONL。它通常不能等待交互式用户，因此对审批/MCP elicitation 使用命令行策略或自动拒绝路径。

### 第三方集成

VS Code、桌面端或其他宿主应优先使用 app-server v2 协议，而不是直接依赖 `codex-core` 内部模块。宿主需要维护 thread/turn/item 状态机，处理 delta 合并、审批回调、断线恢复和分页历史。

## 10. 扩展点与演进建议

当前代码已经形成比较明确的扩展边界：

1. **新增模型可见能力**：优先实现 tool runtime/extension，再由 `build_tool_router` 决定暴露范围。
2. **新增上下文信息**：使用 `codex-rs/core/src/context` 的 fragment 机制，控制大小、来源和是否进入持久历史。
3. **新增客户端能力**：在 app-server v2 增加 Params/Response/Notification，并保持 wire naming 与 TS schema 一致。
4. **新增持久化数据**：通过 `thread-store` 接口和 rollout/history projection 管理，不直接让 UI 读内部文件。
5. **新增用户可见 UI**：同时更新 item/event 映射和 TUI snapshot/integration coverage，确保流式和恢复状态一致。

产品设计上应优先保证以下不变量：

- 对话上下文只能增量构建，不能在普通 turn 中悄悄重写历史。
- 每个模型可见工具调用都有对应 output，历史不会留下孤立 call/output。
- 一个 thread 同时最多一个活动 regular task；并发输入必须通过 start/steer/mailbox 仲裁。
- 权限、环境和工具快照在同一次 sampling 中一致。
- 客户端可仅依赖 completed lifecycle 事件恢复最终状态，delta 丢失不应破坏持久历史。

## 11. 当前边界和风险

- 模型推理和部分 compaction 能力依赖 provider/API；网络故障会触发重试或 transport fallback，但不会保证请求一定成功。
- 长线程的自动压缩有信息损失，连续压缩可能降低准确性。
- `steer` 不是对所有 task 都可用；review、compact 和其他 non-steerable task 会返回明确拒绝原因。
- 工具调用的真实权限由 sandbox/permission profile 决定，UI 上显示“可执行”不等于绕过审批。
- app-server 中标记 experimental 的字段/方法可能变化；集成应使用生成的 schema 并显式 opt-in。
- realtime conversation 与普通 turn 使用不同事件族和生命周期，不能把两者简单拼成一个历史序列。

## 12. 一句话总结

Codex 的主体不是“请求一次模型得到一次答案”，而是一个以 thread 为长期容器、以 turn 为调度单元、以 item 为上下文事实、以 event 为实时投影的闭环代理系统：模型负责提出下一步，Core 负责在上下文、工具、权限、环境和持久化约束下执行下一步，直到得到最终回答或明确中断。

