# ADR 0041：DeepSeek V4 Chat Completions 适配

## 状态

已接受，2026-08-18；2026-08-19 补充失败工具 Turn 的显式重试边界；2026-08-20 更正兼容端点缺失非空 `reasoning_content` 时的降级要求；同日扩展到 DeepSeek 全家族模型并增加自适应 passback 降级（详见 ADR 0044）。

## 背景

`deepseek-v4-pro-0813` 可以通过 Chat Completions 端点访问，但并不等同于普通 OpenAI-compatible 模型。对 DeepSeek 官方 `deepseek-harness` 仓库 `99f6f02` 的 `packages/llm/llm-deepseek` 实现进行对照后，确认 V4 请求使用顶层 `thinking: { type }` 和 `reasoning_effort: low | high | max`；思考模式产生工具调用时，后续请求还必须在对应 assistant 工具消息中原样回传本轮 `reasoning_content`。通用适配器此前只发送 OpenAI 推理档位并忽略 `reasoning_content`，因此工具结果续轮可能被端点拒绝。

`reasoning_content` 是私有思维链，不是安全摘要。把它加入 `ProviderContext` 会将明文写入 JSONL、SQLite 历史投影或审计界面，违反运行时只持久化安全推理摘要的边界。完全丢弃它又无法完成 DeepSeek 当前工具轮。

## 决策

1. 增加 `deep_seek_chat_completions` 传输，继续复用 Chat Completions HTTP、SSE、取消、错误脱敏和工具增量基础设施，但由 Provider 适配器独立生成 DeepSeek 载荷。
2. 为兼容已经保存为 `open_ai_chat_completions` 的配置，只要模型 ID 的独立供应商段以 `deepseek` 开头并带 `-`、`_` 或 `.` 分隔符，就自动使用 DeepSeek 方言，覆盖 `deepseek-chat`、`deepseek-reasoner`、V3/V4/R1 以及常见网关前缀。显式 DeepSeek 传输不限制模型 ID；相近但不匹配的名称继续使用普通 OpenAI 方言。
3. `Off` 映射为 `thinking.type=disabled` 并省略 `reasoning_effort`；`Minimal/Low` 映射为 `low`，`Medium/High` 映射为 `high`，`XHigh` 映射为 `max`。配置的 `maxOutputTokens` 映射为 `max_tokens`。
4. DeepSeek `reasoning_content` 不产生任何公共或持久化事件。适配器在单个 Provider 实例内使用有界内存按工具调用 ID 暂存，最多接受 2 MiB 的单次推理、4 MiB 的 Turn 缓存和 512 个调用；完成同一 Turn 的下一次请求时原样回传。官方流的第一帧可以携带空字符串占位，但完整思考工具响应必须取得非空私有推理才能执行原生 passback；响应结束后缓存仍为空表示兼容端点没有提供必要协议事实，不能发送 `reasoning_content: ""` 冒充原始 CoT。关闭思考不需要私有推理，仍可发送原生工具消息并省略该字段。
5. 应用重启、进入后续 Turn、切换备用 Provider，或兼容端点完成工具响应却没有返回非空私有推理时，passback 不可用。此时保留既有 `AssistantToolCalls` 的非空进度正文，并把配对 `ToolResult` 确定性渲染为自然语言的 assistant 历史观察；不再发送 `tool_calls`、`role=tool` 或伪造的 `reasoning_content`，也不得把工具结果提升为 user 角色。降级文本不包含调用 ID、原始参数或内部协议标记；`request_user_input` 只保留已经解析的用户澄清结果。请求边界还会从同一请求的结构化 `AssistantToolCalls` / `ToolResult` 重建旧版 `[Historical tool calls]` / `[Historical tool result ...]`，并只移除与真实工具事实逐字匹配的 assistant 尾块；不得按标记名称模糊删除，也不得改写无关 assistant 说明、user/system 正文或持久化事实。只要降级后的请求仍以 assistant 观察结尾，适配器就追加一条固定且不含工具正文的 user 续跑意图；该消息只表示继续中断任务，不能拼接模型文本、工具输出、调用参数或私有推理。这保留必要工具事实，并避免无配对、缺少 passback、提示角色提升、内部格式复述以及 DeepSeek 把末尾 assistant 误判为待回传思考响应。
6. DeepSeek Chat Completions 按官方适配器能力视为文本输入；即使旧目录错误声明支持图片，运行时也不得选择该模型执行原图请求。普通 OpenAI 模型的视觉行为保持不变。
7. DeepSeek 请求总超时使用 300 秒，连接超时仍为 30 秒；取消令牌和进程内资源边界保持现有语义。

## 影响

- 已有 DeepSeek 家族配置无需重建供应商即可进入兼容路径；设置页也提供显式 DeepSeek 传输供新配置使用。
- 当前 Turn 的思考模式工具调用可以遵循官方 passback 规则，私有思维链不会进入会话事实、日志或界面。
- 重启后的历史不会保持原生 DeepSeek 工具消息格式，但会以不含调用元数据的有界 assistant 观察继续提供上下文。失败工具 Turn 的显式重试还会以不含任何工具内容的固定 user 消息重新建立请求边界。相比持久化私有思维链或把工具内容提升为 user 指令，这是明确接受的模型上下文精度折衷。
- 通用 OpenAI Chat Completions 的请求字段、图片消息和历史工具格式不变。

## 验证

- 请求测试覆盖 `thinking` 开关、`low/high/max` 映射、`max_tokens` 和普通 OpenAI `xhigh` 不回退。
- SSE 测试证明 `reasoning_content` 只进入私有缓冲，不产生 `ProviderEvent`。
- 工具历史测试覆盖当前 Turn 非空原样 passback、思考模式空缓存的同 Provider 安全降级、关闭思考时的原生工具配对、重启历史文本降级、失败工具 Turn 重试边界、`request_user_input` 原始参数隔离、失败结果的 assistant 角色、旧内部标记与结构化事实逐字匹配后的定向清理、无关 assistant 说明及 user 同名正文保留、空工具结果和缓存上限；环回 HTTP/SSE 测试分别验证同一 Provider 的原始 `reasoning_content`/`tool_calls`/`role=tool` passback、同 Provider 缺失 reasoning 时的无原生工具降级，以及重建 Provider 后请求以不含工具正文的固定 user 续跑意图收尾。
- 配置与应用状态测试覆盖传输枚举、DeepSeek 家族识别、相近名称拒绝和文本输入能力收缩。
