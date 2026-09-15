# Chat Completions 最大输出配置修复

日期：2026-09-15。关联任务：P10-185。

## 真实故障与证据

来源会话“插件和skill区别在哪”（`46b2cf48-b468-44d8-9e54-f741d0e1f7d4`），失败 Turn 为 `943588c3-f2e6-4ce2-8cc2-809adfcb3078`。实际失败请求是该会话第三条用户消息“那我试试presentations插件，展示k-code产品介绍”，不是此前两轮概念说明。

只读检查正式应用 `C:/Users/nealk/AppData/Roaming/com.kcoder.app/runtime-data` 的会话事实与公开供应商配置：

- Turn 于 10:17:19.569 开始，10:18:29.121 失败，总耗时约 69.55 秒。
- 第 11 次模型调用（`call_index=10`）使用 TokenHub / deepseek-flash，输入 12,354、输出 8,192、合计 20,546 tokens；该响应报告 reasoningOutputTokens=0。
- 当时流程已经加载 presentations Skill、探测到 artifact-tool 2.8.59、完成上下文压缩并成功写入计划文件。最后一轮没有落盘完整工具调用，因此无法从事实日志确定被截断参数的具体内容；未把该猜测当成已确认原因。
- 当前公开配置为 `open_ai_chat_completions`、`supportsVision=true`、`contextWindow=200000`、`maxOutputTokens=65355`。历史未保存每次请求的完整配置快照；结合现有代码和实际 HTTP 回归确认该配置路径存在漏参。
- `app_state::deepseek_dialect` 让显式支持图片的模型走普通多模态协议；`OpenAiChatCompletionsProvider::payload` 原先仅在 DeepSeek 分支写 `max_tokens`。普通分支完全忽略最大输出，因此第三方网关可使用自身默认值。实际输出恰好为 8192 且返回 `length`，与命中单次生成上限一致。没有向真实网关重新请求，不能据此证明网关一定允许 65355。

`length` 表示本次生成被长度上限截断。把它标成成功会让不完整回答或工具参数冒充完成；当前拒绝截断调用是正确行为。上下文压缩不能修复漏发的单次输出上限。

## 修改

- 仅在 Provider 适配层补发已保存的最大输出，普通兼容模型使用 `max_tokens`。
- 已知 GPT-5/GPT-6 和 o1/o3/o4 家族使用 `max_completion_tokens`，支持斜杠命名空间以及连字符/点号后缀；相近但不匹配的别名按普通兼容模型处理。该映射不推断或更改模型的上下文大小、图片能力、推理档位或用户预算。
- 未设置最大输出时省略；两种参数不会同时发送。显式 DeepSeek 分支保持现有映射。
- 保留 `length` 失败、同帧 Usage 入账、未完成工具不执行、不得将失败伪装成完成等边界。

OpenAI 参数依据：[Create chat completion](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)。官方文档列出 `max_completion_tokens`，并标记旧 `max_tokens` 不兼容 o 系列；第三方网关仍由自身协议决定。官方 MCP 的 Markdown 入口返回 404 后，实际读取相同页面 HTML 核对字段。

## 验证

- 修改前新增两项测试均失败：已配置 65355 的 JSON 与实际 HTTP 请求缺少最大输出字段。
- 修改后 Chat Completions 专项 28/28 通过，覆盖配置为空/有值、模型参数映射与边界名称、HTTP 实际载荷、截断工具不发布、8192 输出 Usage 先于终止错误。
- `pnpm build` 通过，保留现有大 chunk 提示。
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`、`cargo check --manifest-path src-tauri/Cargo.toml` 通过。
- `cargo test --manifest-path src-tauri/Cargo.toml`：553 通过、3 失败。失败项为既有 `read_recovery_delivery_only_hard_stops_the_corrected_provider_batch`、`semantic_read_tracker_recovers_once_before_stopping_overlap_loops`、`recovery_still_stops_varied_overlapping_reads_after_one_correction`，与 P10-184 文档记录一致；本次未更改这些逻辑。
- `git diff --check` 通过。

实际启动 `pnpm tauri dev --no-watch --config src-tauri/target/output-limit-validation.json`，使用独立应用标识 `com.kcoder.validation.outputlimit`、Vite 1494、WebView2 调试端口 9434。运行 `node scripts/validate-chat-output-limit-native.cjs` 通过四次真实本机 HTTP 请求：

| 请求 | 已发送 max_tokens | 模拟服务响应与实际运行结果 |
| --- | --- | --- |
| 1 | 省略 | 返回截断工具 JSON、Usage 8192 与 length，Turn failed，文件不存在，无自动重放 |
| 2 | 65355 | 修改配置后显式重试；分帧返回完整工具 JSON，实际写入 19,200 字节测试文件 |
| 3 | 65355 | 完整工具结果进入下一请求，Turn completed；刷新后成功回答、文件变更与旧失败均恢复 |
| 4 | 65355 | 模拟网关仍强制 length，Turn failed；既有文件内容逐字不变，无额外请求 |

以上 Usage 数值由本机夹具模拟，实际验证的是 Tauri IPC、供应商配置、Provider 载荷、SSE、AgentRuntime、文件工具和历史恢复，不代表真实 TokenHub 可输出 9000 或 65355 tokens。验证脚本早期因工作区恢复导致文件断言两次失败，之后改用 dev 宿主源码工作区内唯一的 `src-tauri/target/output-limit-native/<timestamp>/deck.js` 路径；验证文件位于 Cargo 忽略的 target 目录，且无散落 `src-tauri/deck.js`。最终结果见 [原生结果](Chat输出上限原生结果.json)、[原生截图](Chat输出上限原生验证.png)。

隔离测试供应商及虚拟凭据已通过产品命令移除；测试输出保留在源码的忽略目录供复核。

## 交付范围

源码修改在 `D:/code/Nick/k-coder`；不提交、不推送，未部署 `D:/apps/k-coder`。未修改正式供应商配置、正式会话事件或触发该用户会话重试。此处修复客户端输出参数，未继续执行原会话的 PPT 制作任务。
