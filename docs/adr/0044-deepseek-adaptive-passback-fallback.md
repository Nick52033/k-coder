# ADR 0044：DeepSeek 不可回传推理的自适应工具历史降级

## 状态

已接受，2026-08-20。

## 背景

`D:\code\Nick\deepseek-harness` 的 `llm-deepseek` 适配器要求：思考模式中，带 `tool_calls` 的 assistant 消息必须原样带回同一轮产生的非空 `reasoning_content`。首个 `reasoning_content: ""` 只是流占位，不能作为完整思考内容。k-Coder 已经把可取得的私有推理限制在当前 Provider 的有界内存中，但部分 OpenAI-compatible 网关会消耗思考 token，却不返回可回传的完整字段，或者返回看似非空但实际上不能被端点接受的内容。此时第一次工具请求成功，下一次携带原生工具历史的请求会返回：

`The 'reasoning_content' in the thinking mode must be passed back to the API.`

通用运行时不会重放普通 HTTP 400，因为参数错误和鉴权错误不应被自动重试；但这个特定 400 表示历史协议形态与兼容端点能力不一致，需要在 Provider 边界修复请求形态。

## 对照实现

官方 harness 只在工具调用 assistant 轮次发送原生 `tool_calls`、配对 `role: tool` 和非空 `reasoning_content`；思考关闭时保留原生工具配对并省略私有字段。它不会把空占位伪装成完整 CoT，也不会把私有推理写入公共会话历史。

## 决策

1. DeepSeek Provider 识别 OpenAI-compatible 配置中以 `deepseek` 开头的独立模型供应商段，覆盖 `deepseek-chat`、`deepseek-reasoner`、V3/V4/R1 和网关前缀；普通 OpenAI 模型不改变载荷。
2. 首次请求只在以下条件同时满足时执行一次 Provider 内部兼容重发：思考模式已开启、请求载荷确实包含 assistant `tool_calls`、HTTP 状态为 `400`，且脱敏错误文本同时包含 `reasoning_content` 与 passback/thinking 语义。读取错误响应后清空当前 Provider 的私有 reasoning 缓存并标记 `force_degraded`，然后使用同一 `ProviderRequest` 重建安全历史。
3. 重发历史不含私有 `reasoning_content`、assistant `tool_calls` 或 `role: tool`；工具进度和结果继续使用既有自然语言 assistant observation，并在观察收尾时追加固定、不含工具正文的 user 续跑意图。第二次请求仍失败时返回第二次的状态码和脱敏消息。
4. `force_degraded` 只存在当前 Provider 实例，且会持续作用于后续思考工具轮；重建 Provider 后状态清除。关闭思考时不使用该标记，仍保留官方原生工具配对。
5. 其他 HTTP 400、非思考请求、没有原生工具历史的请求、普通 OpenAI 方言和流已经开始后的错误均不走该兼容重发；不得把该路径提升为通用重试或故障切换。

## 安全与边界

- `reasoning_content` 只存于有界进程内缓存，不进入 ProviderEvent、JSONL、SQLite、日志、指标、摘要或界面。
- 降级文本不包含调用 ID、原始参数、私有推理或内部协议标记；工具输出只作为低权限 assistant observation，不提升为 user 指令。
- 错误匹配使用脱敏文本，API Key 和授权请求头不写入诊断；重发最多一次，避免错误循环和重复副作用。
- 兼容重发不改变工具执行、持久化事件或公共 Turn 计数语义；它只在 Provider 尚未返回流之前替换网络请求载荷。

## 影响

支持 DeepSeek 的网关即使无法提供可回传 CoT，也能在一次受限重发后继续复杂工具任务；原生 passback 可用的端点继续使用官方格式。关闭思考的调用不受降级状态影响。端点持续拒绝或返回其他 400 时，用户仍会看到明确失败原因，不会被伪装成已恢复。

## 验证

- Rust 单元测试覆盖 DeepSeek 全家族识别、思考切换后的缓存隔离、空/空白 reasoning 降级、关闭思考原生工具历史、错误文本筛选和普通 OpenAI 不重试。
- HTTP/SSE 环回测试覆盖首轮工具调用、第二请求特定 HTTP 400、第三请求安全历史成功；断言重发载荷不含 `reasoning_content`、`tool_calls`、`role: tool` 或调用元数据。
- 质量门槛继续执行 `pnpm build`、Rust 格式检查、`cargo check`、`cargo test` 和 `git diff --check`；桌面验证使用隔离配置与本地假端点，不发送真实 API 请求。
