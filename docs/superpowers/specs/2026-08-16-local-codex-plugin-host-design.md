# k-Coder 本地 Codex 插件宿主设计

## 背景

k-Coder 已有统一的 Skills、MCP、Hooks、工具策略、审批、取消和审计运行时，但尚未支持以 Codex 插件包为单位发现和管理扩展。用户希望自行取得插件包，然后把完整目录复制到 k-Coder 的本地插件目录；只要插件根目录包含 `.codex-plugin/plugin.json`，k-Coder 就能发现它，并在设置页提供启用、禁用和删除。

用户明确要求：

- k-Coder 不提供插件市场、下载或远程安装。
- 插件由用户自行复制，不在本次工作中向仓库内置第三方插件。
- 新发现的插件默认禁用。
- 插件页只管理启用、禁用和删除，同时展示路径、版本、组件及诊断。

## 目标

1. 在应用数据目录建立稳定的本地插件根目录。
2. 发现带有 `.codex-plugin/plugin.json` 的直接子目录，并保留插件包身份。
3. 把已启用插件的标准 Skills 和受支持 MCP 配置适配到现有 `ExtensionService`，不创建第二套智能体或工具运行时。
4. 对不支持的 Apps、OAuth MCP、Codex 专用 Hooks、agents 和 commands 提供诚实诊断，不执行、不静默放行。
5. 对插件路径、组件引用、删除和本地进程启动维持 k-Coder 现有安全边界。

## 非目标

- 插件市场、目录搜索、下载、Git/NPM 安装、在线升级和自动更新。
- 把任何第三方插件复制进 k-Coder 仓库或安装包。
- 完整复刻 Codex Browser、Primary Runtime、Apps 或 OAuth 运行时。
- 执行 Codex/Claude/Cursor 私有 Hook 协议。
- 支持项目级插件目录、嵌套 marketplace/version 目录或同一插件的多版本选择。
- 自动安装 Node、Python、LibreOffice、Poppler、FFmpeg 等插件外部依赖。

## 目录契约

应用启动时确保以下目录存在：

```text
<app_data_dir>/runtime-data/plugins/
  <folder>/
    .codex-plugin/
      plugin.json
    skills/
    .mcp.json
    .app.json
    hooks.json
    assets/
```

规则如下：

- 只扫描 `plugins/` 的直接子目录，目录名不作为插件 ID。
- 只有根下存在 `.codex-plugin/plugin.json` 的目录才是候选插件；其他目录忽略。
- 清单中的 `name` 必须匹配 `^[a-z0-9][a-z0-9._-]{0,63}$`，并生成稳定 ID `<name>@local`；`version` 只用于展示和修订诊断。
- 同一轮扫描中出现重复插件 ID 时，所有冲突目录都标记为无效，任何一个都不加载。Windows 上目录名大小写差异不能规避重复检测。
- 插件升级由用户整体替换目录完成。清单或受支持组件变化会改变扩展修订并触发完整注册表重建。
- 所有插件都是用户来源、可删除；本期没有内置只读插件源。

## 清单兼容范围

首期只识别 `.codex-plugin/plugin.json`，不把根级 Agent Plugins v1 `plugin.json` 或 `.claude-plugin/plugin.json` 当作安装清单。

清单至少需要：

- `name`
- `version`
- `description`

同时读取以下可选字段：

- `author`
- `license`
- `homepage`
- `repository`
- `skills`
- `mcpServers`
- `apps`
- `interface`

清单允许保留未知字段，以兼容 Codex 插件向前演进，但未知字段不会获得执行语义。所有组件路径必须是插件根目录内的相对路径。

扫描使用固定上限：最多发现 128 个候选插件；单个清单最大 256 KiB；每个插件最多索引 128 个 Skills；单个 `SKILL.md` 或文本资源最大 256 KiB；单个 `.mcp.json` 最大 1 MiB。修订扫描只遍历清单引用的 Skills 树和受支持组件文件，不递归遍历插件根下的 `node_modules`、构建产物或其他未声明目录。任一插件超过自己的上限时只关闭该插件。

## 架构

### `extensions/plugins.rs`

新增本地插件领域模块，负责：

- 插件根目录创建和直接子目录扫描。
- 清单解析、字段校验、组件路径解析和重复 ID 检测。
- 插件状态、组件摘要、警告和错误诊断。
- 启用状态读取、删除目标解析及扩展修订材料收集。
- 把受支持组件转换为现有 Skill/MCP 输入。

该模块不执行 Agent Turn，不直接调用 Provider，也不绕过 `ToolRegistry` 或 `PolicyEngine`。

### `ExtensionService`

`ExtensionService` 保持扩展事实入口：

1. 发现现有全局/项目指令、Skills、MCP 和 Hooks。
2. 调用本地插件扫描器。
3. 只展开已启用且校验成功的插件。
4. 把插件 Skills 加入有界 Skill 索引。
5. 把插件 MCP 转换为现有 MCP 客户端和普通 `ToolHandler`。
6. 使用已有工具注册表、风险、审批、Hook、取消和审计链路完成注册。

插件失败不得保留上一轮已注册能力。注册表始终从内置能力、现有扩展配置和当前有效插件重新构建。

### Tauri 边界

新增轻量命令：

- `plugin_overview(refresh)`
- `set_plugin_enabled(plugin_id, enabled)`
- `delete_plugin(plugin_id)`

命令只做参数接收、错误映射和状态调用，不包含扫描、路径或运行时业务逻辑。

### 设置界面

新增独立 `PluginSettingsPage`，接入现有设置中心的“插件管理”导航项。页面使用类型化 API，不直接访问文件系统或数据库。

## Skills 适配

插件清单的 `skills` 指向一个目录，默认约定为 `./skills/`。该目录的每个直接子目录可包含一个 `SKILL.md`。

插件 Skill 采用 Codex 常见 frontmatter：

- `name` 和 `description` 必填。
- `triggers`、`risk`、`enabled` 是可选的 k-Coder 扩展字段。
- 未声明 `risk` 时使用保守的 `write` 诊断值；实际工具授权仍完全由工具自身风险和 `PolicyEngine` 决定。
- 未声明 `triggers` 时不做自由文本子串触发。
- Skill 名称以插件 ID 命名空间隔离，不能覆盖内置、全局或项目 Skill。

启用插件后，运行时指令只加入有界的 Skill 目录，包括插件 ID、Skill 名称和描述，不把所有 Skill 正文一次性塞入上下文。新增只读内部工具：

- `plugin_skill_read(pluginId, skillName)`：读取已启用插件的已索引 `SKILL.md`。
- `plugin_resource_read(pluginId, path)`：读取 Skill 引用的插件内 UTF-8 文本资源。

两个工具只访问扫描时建立的插件索引，重新规范化路径，限制单文件和总输出大小，并拒绝绝对路径、`..`、符号链接及目录联接。系统指令要求模型在应用某个插件 Skill 前先调用 `plugin_skill_read`。插件资源工具不执行脚本，也不扩大 Shell 权限。

用户在输入中写出 `@<plugin-name>` 或 `plugin://<plugin-id>` 时，目录会优先突出该插件，但仍由模型通过只读工具按需读取具体 Skill。

## MCP 适配

插件清单的 `mcpServers` 指向 `.mcp.json`。首期支持常见的 `mcpServers` 对象格式：

- stdio：字符串 `command`、字符串数组 `args`、可选相对 `cwd`、`timeout_ms` 或 `tool_timeout_sec`，以及可选字符串数组 `env_vars`。
- HTTP：`type: "http"`、`url`、可选 `bearer_token_env_var`。

适配规则：

- 服务 ID 加入插件命名空间，避免和用户 MCP 配置或其他插件冲突。
- 相对 `cwd` 固定解析到插件根目录内。
- `${CODEX_PLUGIN_ROOT}` 只替换为当前插件规范化根目录；不提供任意环境变量插值。
- stdio 继续使用最小进程环境、结构化命令、超时、取消和进程清理。
- `env_vars` 中的 `CODEX_PLUGIN_ROOT` 由宿主注入；其他名称被解释为同名操作系统凭据，不从 k-Coder 完整进程环境透传。
- `bearer_token_env_var` 被解释为操作系统凭据名称，HTTP 客户端在内存中添加 `Bearer ` 前缀。
- 凭据值只从现有操作系统凭据存储读取，清单和诊断只保留凭据名称。
- HTTP URL 继续使用现有协议和 URL 校验。
- `oauth_resource` 等需要 OAuth 的配置标记为 `blocked`，不会退化为匿名连接。

插件 MCP 连接失败只关闭该插件的 MCP 组件并产生明确诊断，不影响现有配置来源或其他插件；失败组件不注册任何工具。其他来源的 MCP 仍保持 ADR 0003 和 ADR 0038 的既有关闭失败语义。

## 不支持组件

以下目录或清单字段会进入组件摘要，但首期不执行：

- `.app.json` / `apps`
- Codex/Claude/Cursor Hooks
- `agents/`
- `commands/`
- OAuth MCP

包含受支持 Skills 的插件仍可处于 `degraded` 并提供 Skills；只有不含任何可用组件的插件才处于 `blocked`。界面不得把“已发现”或“已启用”显示成“全部运行时依赖就绪”。插件自行要求的 Node/Python/外部二进制等依赖在实际启动或使用时产生类型化诊断，k-Coder 不自动安装。

## 状态和持久化

新增版本化载荷：

```text
PluginOverview
  schemaVersion
  rootPath
  plugins[]
  error

PluginDiagnostic
  id
  name
  version
  description
  path
  enabled
  state
  deletable
  components
  warnings[]
  error
```

`state` 使用稳定值：

- `disabled`：已发现且有效，但用户未启用。
- `loaded`：所有声明的受支持组件已加载。
- `degraded`：至少一个受支持组件可用，同时存在不支持或失败组件。
- `blocked`：用户已启用，但没有可注册能力或运行时前置条件缺失。
- `invalid`：清单、重复 ID、路径或组件格式不合法。

启用状态写入现有设置投影的 `extension/plugin/<plugin-id>`，默认值必须是 `false`。同一次应用运行中，如果已启用插件消失，必须立即从重建后的注册表移除并把持久化启用状态重置为禁用，避免再次复制时静默恢复执行。

设置开关表示用户的启用意图；组件失败时可以保持 `enabled: true`，同时状态为 `degraded` 或 `blocked`，便于用户修复依赖后刷新恢复。

## 数据流

### 发现

1. 应用启动或设置页请求刷新。
2. 后端创建并规范化 `runtime-data/plugins`。
3. 扫描直接子目录并解析清单。
4. 建立插件、Skill 和资源索引。
5. 返回排序稳定的 `PluginOverview`。

### 启用

1. 设置页发送插件 ID 和 `enabled: true`。
2. 后端确认 ID 来自当前扫描结果且插件有效。
3. 写入投影并强制重建完整扩展注册表。
4. 加载 Skills，连接受支持 MCP，注册普通工具。
5. 返回新的插件诊断；失败组件保持关闭且不遗留旧工具。

### 禁用

1. 写入 `enabled: false`。
2. 强制重建注册表。
3. 不再暴露插件 Skill 目录、资源读取权限或 MCP 工具。
4. 写入不含插件正文和密钥的审计记录。

### 删除

1. 前端显示插件名称和路径并要求二次确认。
2. 后端根据当前扫描索引解析 ID，不接受前端传入路径。
3. 先持久化禁用并重建注册表。
4. 再次确认目标是插件根目录的直接子目录，且目标和祖先不是符号链接或目录联接。
5. 删除插件目录并清理对应启用设置，最后刷新诊断和审计。
6. 如果文件删除失败，插件目录保留但保持禁用，返回可恢复错误。

## 错误处理

- 一个无效插件不会阻止其他插件或既有扩展，但必须显示为 `invalid`，不能静默跳过或加载部分未知内容。
- 已启用插件变为无效时，先移除它上一轮的全部能力，再报告错误。
- 根目录无法创建、规范化或读取属于宿主级错误，`PluginOverview.error` 返回错误，所有用户插件保持未加载。
- 清单超过大小限制、非 UTF-8、字段缺失、ID 非法、组件越界、重复 ID 和链接逃逸都关闭该插件。
- Skill 或资源读取超过上限时，工具返回有界错误，不回退到任意文件读取。
- MCP 缺少凭据、命令、运行时或 OAuth 支持时不重试启动，不暴露秘密值。
- 删除、启停和连接行为写入现有扩展审计，日志不包含插件文件正文、完整环境变量或授权 Header。

## 界面设计

插件设置页采用紧凑列表而不是安装市场：

- 页头展示只读插件根路径和整体错误。
- 每行展示图标、名称、版本、描述、路径、组件数量和状态。
- 二进制控制使用开关；无效插件的开关不可用。
- 删除使用垃圾桶图标和工具提示，点击后进入确认对话框。
- 不提供安装、下载、URL、搜索市场、升级或拖放入口。
- 长名称、路径和错误可换行或截断并提供完整 title，不得撑开设置页。
- 空状态只显示“未发现插件”和插件根路径。
- 页面进入时自动刷新；成功启停或删除后使用后端返回状态更新，不维护第二份插件事实。

## 安全边界

- 插件目录位于受信任应用数据根下，但插件内容仍按不可信输入解析。
- 路径检查使用规范化后的真实路径，并显式拒绝 Windows reparse point、junction 和符号链接逃逸。
- 前端永远不提交删除路径，模型也不能调用插件删除或启停命令。
- 插件清单中的权限、风险或 capability 字段不能授权工具。
- 插件 MCP 工具仍经过 Schema、`PolicyEngine`、审批、取消、Hooks、结果持久化和审计。
- Apps、Hooks、agents 和 commands 在没有独立契约与威胁分析前不执行。

## 测试设计

### Rust 单元和集成测试

- 有效清单发现、稳定排序和 `<name>@local` ID。
- 没有清单的普通目录被忽略。
- 新插件默认禁用，启用状态跨实例恢复。
- 重复 ID、无效 JSON、缺失字段、超限和未知字段兼容。
- 清单组件绝对路径、`..`、符号链接和 Windows 目录联接逃逸。
- 标准 Skill 元数据、命名空间隔离、按需读取和资源读取边界。
- 禁用插件无法通过 Skill/resource 工具读取内容。
- stdio/HTTP MCP 映射、`${CODEX_PLUGIN_ROOT}`、命名空间和凭据脱敏。
- OAuth MCP、缺少凭据、缺少命令及启动失败的 `blocked/degraded` 状态。
- 一个插件失败不影响其他插件，且旧注册工具在刷新后消失。
- 删除未知 ID、越界目标和链接目标失败；正常删除先注销并记录审计。
- 插件目录内容变化会改变扩展修订。
- 公共载荷 schema 版本和 serde 兼容。

### 前端和 E2E 测试

- 插件导航项变为可用，进入页面自动加载。
- 空状态、禁用、loaded、degraded、blocked 和 invalid 状态渲染。
- 开关调用类型化 API，并在失败时回滚显示为后端事实。
- 删除必须确认，取消不调用后端，成功后移除列表项。
- 无效插件不能启用，长路径和错误不产生横向溢出。
- desktop 和窄屏设置页回归。

### 完成验证

至少执行：

```powershell
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

随后启动 `pnpm tauri dev`，在真实应用数据目录放入一个最小测试插件，验证“发现为禁用 -> 启用 -> Skill 可读 -> 禁用后能力移除 -> 确认删除”完整路径。未完成该桌面验证前不声称插件工作流完成。

## 路线图和 ADR

- 用户已明确把本地插件宿主插入当前优先级；实现完成时新增并完成对应 Phase 10 任务，同步当前位置和变更记录。
- 新增 ADR，记录“本地目录投放、默认禁用、复用 ExtensionService、按插件隔离失败、不执行不支持组件”的长期决策。
- 不修改 ADR 0003/0038 对现有扩展配置和 MCP 的关闭失败语义；插件来源的隔离诊断由新 ADR 明确限定。

## 验收标准

1. 用户把完整插件目录复制到页面显示的 `runtime-data/plugins` 后，进入插件页即可看到它，且首次为禁用。
2. 有效的标准 Skills 插件启用后可由同一 AgentRuntime 发现并按需读取，禁用后立即不可用。
3. 受支持的插件 MCP 使用现有安全和审计链路；缺少运行时或凭据时准确显示 blocked/degraded。
4. 无效或越界插件不注册任何能力，也不影响其他插件。
5. 设置页只提供启用、禁用和经确认的删除，不包含下载或安装入口。
6. 删除只作用于当前已发现的直接子插件目录，不能被路径或链接逃逸利用。
7. 所有公共契约、安全分支、桌面和窄屏交互均有回归覆盖，并通过仓库规定的构建、Rust 和真实 Tauri 验证。
