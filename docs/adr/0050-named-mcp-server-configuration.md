# ADR 0050：MCP 命名对象配置契约

- 状态：已接受
- 日期：2026-08-31
- 取代：ADR 0038 中专用 `mcp.json` 的数组契约

## 背景

ADR 0038 为全局和项目专用 `mcp.json` 定义了 `{ "mcpServers": [...] }` 数组。实际 MCP 配置生态通常使用服务名作为对象键，例如 `{ "mcpServers": { "dingtalk-docs": { ... } } }`。设置页因此把可直接用于其他客户端的命名对象误报为“mcpServers 必须是数组”，后端保存接口也无法加载该配置。

插件 `.mcp.json` 已使用命名对象，但专用配置仍维护另一套外层结构。继续保留差异会增加导入、排错和文档成本，并要求每个数组元素重复声明 `id`。

## 决策

1. 全局 `runtime-data/mcp.json` 和项目 `.k-coder/mcp.json` 的规范格式升级为 `{ "mcpServers": { "<server-id>": { ... } } }`。服务器 ID 只取对象键，不在条目中重复保存。
2. stdio 条目使用 `type: "stdio"`、字符串 `command` 和可选字符串数组 `args`。Streamable HTTP 条目使用 `type: "streamable-http"`、`url`、可选固定 `headers` 和 `secret_headers` 凭据名称映射。省略 `type` 时，只能通过互斥的 `command` 或 `url` 推断传输。
3. 为兼容常见配置，HTTP 类型读取时接受 `http`、`streamable-http` 和 `streamable_http`；stdio 命名条目读取时也接受旧的结构化命令数组。保存统一输出规范类型、字符串 `command` 和 `args`。
4. 已存在的专用数组配置继续严格读取。用户再次保存时，后端把它规范化为命名对象。旧 `extensions.json` 的 `mcpServers` 数组继续只作为历史兼容来源，不由本决策自动改写。
5. 命名对象必须拒绝重复服务器键、无效 ID、未知字段、混合传输字段和重复 Header。前端对可由标准 JSON 值观察的结构提供即时校验；原始 JSON 重复键由保留键序列的后端解析器关闭失败，后端仍是最终权威。
6. `headers` 只允许非敏感固定值。Authorization、Cookie、API Key、Token、Secret 和 MCP 会话/协议控制 Header 等名称关闭失败；凭据 Header 必须继续通过 `secret_headers` 映射到操作系统凭据存储。固定 Header 和凭据 Header 不能按大小写重复。
7. `McpConfigView.schemaVersion` 升级为 `2`，未创建文件的默认正文改为 `{ "mcpServers": {} }`。保存仍执行大小、UTF-8、路径规范化、链接逃逸拒绝、同目录临时文件替换和不含正文的审计。

## 影响

收益：

- 常见 MCP 命名对象可以直接粘贴、格式化、保存和运行，服务名与运行时命名空间保持一致。
- 专用配置与插件配置不再在 `mcpServers` 外层结构上冲突，空配置也使用直观的空对象。
- 旧数组不会在升级后失效，并有明确、可审计的下次保存迁移点。
- 固定协议 Header 可以互操作，同时不把凭据值降级写入普通 JSON。

成本与限制：

- 专用配置与旧 `extensions.json` 的每条服务器字段仍不完全相同；后者只保留读取兼容。
- 规范化保存会调整字段布局并移除数组条目中的重复 `id`。
- 需要固定敏感 Header 值的第三方配置不能原样保存，必须先建立系统凭据名称映射。

## 未采用方案

### 只放宽前端校验

未采用，因为后端仍会拒绝保存或扩展准备，界面成功提示会与真实运行时事实冲突。

### 立即拒绝全部旧数组

未采用，因为现有用户文件来自 ADR 0038 的公开契约，直接关闭会制造不必要的升级回归。

### 允许任意固定 Header

未采用，因为常见配置会把 Authorization 或 API Key 直接放在 Header 值中，这会绕过既有系统凭据和脱敏边界。
