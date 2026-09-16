# ADR 0059：手机局域网直连控制面

- 状态：已接受
- 日期：2026-09-15
- 关联：ADR 0009、0015、0018、0021、0048、0056，`docs/superpowers/specs/2026-09-15-mobile-control-design.md`

用户要求手机可以查看会话、发送消息、接收流式事件、停止运行中的 Turn、处理审批和回答 `request_user_input`，而工作区、Provider 凭据、`AgentRuntime`、`ToolRegistry`、`PolicyEngine` 与 `ThreadRepository` 继续留在桌面端。设计文档给出局域网直连（方案 1）与出站中继（方案 2）两条路径，并要求先实现方案 1。

## 决策

1. 只新增一个控制面网关，不新建第二套智能体循环。`src-tauri/src/mobile/` 通过 `GatewayHost` 取 `AppState`，Turn 一律走 `commands::enqueue_message_turn`，与桌面共用同一条运行时、策略与持久化路径。网关不解析模型协议、不直接读写 SQL、不做授权判断。
2. 传输为 axum 之上的 HTTP + WebSocket JSON-RPC 2.0，`MOBILE_PROTOCOL_VERSION = 1`、`EVENT_PROTOCOL_VERSION = 1`。`initialize` 之前只接受 `initialize` 与 `ping`，`initialized` 之后才放行其余方法。错误统一为结构化 `MobileError`（`unauthorized`、`forbidden`、`not_found`、`stale_request`、`rate_limited`、`server_overloaded`、`resume_required`、`unsupported_capability`、`internal_error`），带 `retryable` 与可选 `retryAfterMs`。
3. 绑定非回环地址时必须启用 TLS。证书由 `rcgen` 本机自签并缓存于数据根，指纹为 SHA-256 冒号分隔大写；`tokio-rustls` 显式使用 ring provider，避免引入需要 cmake 的 aws-lc-rs。只监听回环时允许明文，仅供本机调试。
4. 来源校验在握手前完成：只接受回环、RFC1918、ULA 与链路本地地址，IPv4-mapped IPv6 先规范化；携带 `X-Forwarded-*`、`Forwarded` 或 `Via` 的请求直接拒绝；`Host` 头必须命中本机地址白名单，用于防 DNS 重绑定。握手与配对分别按 IP 限流为每分钟 60 次、每 10 分钟 10 次。
5. 配对是一次性挑战：10 分钟有效、单次使用、最多 5 次错误尝试后锁定；6 位人工校验码供人工比对；挑战密钥放在配对 URL 的 fragment 中，浏览器不会把它发给服务端。设备密钥与刷新令牌只以 SHA-256 摘要落盘（`<data_root>/mobile/state.json`，临时文件加原子 rename），访问令牌只驻内存且 15 分钟过期，撤销立即失效。
6. 出站事件走有界队列（容量 256）加连接内单调 `deliverySeq`，并保留 512 条补发缓冲。`TextDelta`、`ToolOutputDelta`、`UsageUpdated`、`ActivityStatusChanged`、`ProviderRetryWaiting` 可丢弃；其余关键事件在队列满时改发 `resync_required`。游标超出缓冲范围返回 `resume_required`（`cursor_expired` 或 `cursor_not_buffered`），客户端改为重新 `thread/read` 取快照。补发缓冲不进入模型历史。
7. 事件在单一出口做移动安全投影：剥掉工具原始参数、补丁正文、文件内容、unified diff、图片 base64 与私有推理（`ThreadItemPayload::Reasoning` 不投影），路径只给工作区相对形式；`ThreadWorkspaceMismatch` 这类含绝对路径的错误映射为不含路径的 `forbidden`。
8. 审批复用桌面语义：手机不持有补丁正文，批准时由服务端从 `PendingRequestIndex` 保存的 preview 生成 `selectedPaths` 与 `expectedHashes`。`IdempotencyCache` 以「设备 + 方法 + requestId」去重（上限 256），移动网络重试不会产生重复 Turn。
9. 能力默认只授予 `chat`、`approval`、`interrupt`。`fileRead`、`shell`、`settings`、`plugins`、`secrets` 即使显式开启也返回 `unsupported_capability`，因为本期没有实现。供应商配置、插件与 MCP 管理、密钥管理和系统级设置不向手机开放。
10. 手机端页面是随二进制内联的单个 HTML（`include_str!("assets/mobile.html")`），不引入前端构建产物，响应头固定 `default-src 'none'`。页面在 `hashchange` 时重新解析配对片段：手机浏览器对「同源同路径、只有 hash 不同」的跳转不重新加载页面，否则用户在已打开的标签页里点配对链接会停在「没有检测到配对链接」。
11. `thread/subscribe` 返回的 `deliverySeq` 与 `events/resume` 的 `afterDeliverySeq` 使用同一语义，即**本连接最后已分配的 `deliverySeq`**（`next_delivery_seq - 1`），而不是「下一个待分配序号」。两者差一会让客户端把订阅返回值直接存成游标后每次重连都被判成 `cursor_not_buffered`，使增量补发失效并触发一次误导性的「事件游标已过期」。协议内同名字段必须同义。
12. 方案 2（出站中继）不得通过直接暴露方案 1 的端口来实现，仍需独立设计。

## 验证

Rust 单元测试覆盖协议错误映射、能力门控、认证与配对生命周期、事件路由与补发游标、移动投影和 RPC 分派；`src-tauri/tests/mobile_gateway.rs` 用 `StubHost` 驱动真实 axum 监听端口，覆盖健康检查、移动页面、`Host` 头与转发头拒绝、非回环无 TLS 拒绝、公网绑定拒绝、完整配对流程、`initialize` 门控、错误密钥、版本协商、访问令牌重认证、未知方法、能力门控、缺参与畸形 JSON、撤销即时生效、无 `AppState` 时 fail-closed 以及无订阅不投递。`e2e/mobile-settings.spec.ts` 在浏览器中打桩 Tauri 调用，覆盖设置页的启停、指纹展示、配对挑战、设备批准与撤销、能力开关和窄屏布局。前后端字段契约由 `src-tauri/tests/mobile_dto_contract.rs` 与 `e2e/fixtures/mobile-dto.json` 共同钉住：Rust 用该文件构造 DTO 再序列化做全等断言，前端用同一文件当桩数据，字段改名会同时让两侧失败（已用改名实验验证过约束不是空转）。

原生验收由 `scripts/verify-mobile-native.mjs` 承担，它不依赖任何第三方包，用 `node:tls` 与手写 WebSocket 帧编解码扮演手机端，直连真实运行中的客户端：`phone` 子命令 29 项断言全部通过（TLS 1.3 握手与指纹逐位核对、`/health`、内联页面、`/` 跳转、`Host` 与转发头拒绝、`initialize` 生命周期、42 条真实会话、会话「推送代码」的 227 项投影与泄漏扫描、失败路径、撤销）；`pwa` 子命令用真实 Chromium 打开手机端页面，9 项断言全部通过（渲染、配对片段解析、`POST /pair` 错误展示、同源 `wss` 未被 CSP 拦截、无 CSP 违规）。该工具需要 `seed` / `restore` 两个子命令配合，验收设备记录不残留。

前端 `pnpm build` 与 Rust 格式/检查通过；详见 `docs/手机局域网直连验证.md`。
