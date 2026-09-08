# ADR 0055：机器人节点技能绑定与技能入库

- 状态：草案
- 日期：2026-09-08

## 背景

k-Coder 内置机器人（ADR 0039）的节点只有 `instructions` + `completion_criteria` 文本，执行知识依赖 prompt 描述，没有节点级技能支撑。cn-codex 的同名机器人提供了两个经过代码验证的事实：

1. **cn-codex 的节点技能正文注入是真实有效的驱动力**。`robot_orchestrator.rs` 的 `build_overlay_prompt` 在每次请求注入：根目标、节点进度（DONE/CURRENT/PENDING）、当前节点目标、当前节点绑定的技能 ID 列表和 SKILL.md 正文（单技能截断 8000 字符），并以 `<workflow_node_done/>` 哨兵标记推进节点。
2. **cn-codex 的角色 systemPrompt 存而未注入**。`robot.json` 中的大段 `systemPrompt` 不进入运行时（`overlay_prompt_does_not_include_robot_system_prompt` 测试明确排除；goal+robot 模式的主 system prompt 是通用提示词；前端仅在设置页展示）。这正是 ADR 0039 背景中"角色 prompt 可能只存储而没有实际注入"批评的实锤。k-Coder 的 `runtime_instructions` 已真实注入角色、目标和当前节点指令，这一点已领先，不需要对齐。

纯 objective 驱动的缺口：模型在"实现""测试执行"等节点缺少具体方法论、产出格式和验证标准。因此本 ADR 补齐节点技能绑定，同时不复制 cn-codex 的死数据和不引入哨兵推进。

## 决策

### 1. 数据契约

`WorkflowNodeDefinition` 增加可选 `skills: &'static [&'static str]` 字段，值为技能 ID，经既有"内置 → 全局 → 项目"三层来源解析。绑定属于编译期定义，`workflows.jsonl` 运行快照不含技能正文，旧快照无需迁移，`workflow/node` ID 保持稳定。

### 2. 注入机制

- 时机：每次 Provider 请求前，工作流 active 时按 `currentNodeIndex` 读取绑定技能的 SKILL.md 正文，以 `[Workflow Skill: <name>]` 分节加入受控运行时指令，位于节点指令之后、宿主安全约束之下。
- 边界：每节点最多 4 个技能；单技能正文 16 KiB、合计 48 KiB，超限保留首尾并记录省略字节数。
- 降级：项目/全局来源技能缺失时跳过该技能并写扩展审计，不终止机器人。
- 启用检查：`risk: read` 技能直接注入正文；`write`/`external` 技能须已启用，未启用时仅注入一行占位声明。
- 技能正文不写入 JSONL、不进入界面正文、不进入日志；不改变 `PolicyEngine` 决策，不自动启用 MCP/Hook/子智能体，不扩大工作区与外部副作用边界。

### 3. 技能分组目录（方案 A）

- 从 cn-codex 复制的技能统一放 `src/resources/skills/robot-pack/<skill-id>/SKILL.md`，与既有内置技能隔离，避免目录膨胀。
- `discover_skills` 扩展支持一级分组目录（深度 2）：分组目录本身不是技能，其直接子目录按现有规则解析；技能 `name` 仍必须与自身目录名一致且为小写 kebab-case。
- 增加同作用域重名检测：同一来源根内出现重名技能时关闭失败（现状是静默覆盖）。
- `tauri.conf.json` 打包整个 `skills/` 目录、revision 指纹递归收集文件，均无需改动。

### 4. 技能正文准入判据

正文引用的每个工具必须存在于 k-Coder 工具注册表；存在任一不存在的工具引用时必须先重写工具层。frontmatter 须满足 k-Coder 契约（`name`、`description`、`triggers`、`risk`、`enabled`）。已按此判据逐个验证 cn-codex 技能正文（详见附录）。

### 5. 节点绑定表

#### fullstack-delivery 全栈开发

| 节点 | 绑定技能 | 说明 |
| --- | --- | --- |
| 仓库探索 | （无） | 内置 `list_directory`/`read_file`/`rg` 足够，不注入噪声 |
| 方案与计划 | （无） | `requirement-to-dev-design` 为公司钉钉模板专用技能（模板 1.7.2、FMEA、评审门禁），不适合通用规划节点，撤回；待有通用规划技能再绑 |
| 实现 | `tdd-implementation` | 复制 cn-codex `test-driven-development`，纯方法论；就位前该节点按无绑定运行 |
| 验证 | `review-loop` | 既有内置技能，"审查→构建/测试→修复→复验"单循环 |
| 审查与交付 | `code-review-expert`、`review-fix` | 既有内置技能，评审 + 确定性 P0/P1 修复闭环 |

#### quality-assurance 质量保障

| 节点 | 绑定技能 | 说明 |
| --- | --- | --- |
| 范围确认 | `test-strategy-planning` | 复制件就位后绑定；此前无绑定 |
| 测试设计 | `test-case-design` | 复制件（方法论比既有 `test-cases` 更完整）；`test-cases` 保留为通用技能不进绑定 |
| 测试执行 | `api-test`、`webapp-testing` | `api-test` 为既有内置技能；`webapp-testing` 为重写版（见技能生产计划 P0），就位前仅绑 `api-test` |
| 结果分析 | `review-test-report`、`systematic-debugging` | 前者既有内置；后者为复制件 |
| 测试报告 | `review-test-report` | 同一技能覆盖报告生成；如需独立可后续拆分 |

#### requirements-design 需求设计

| 节点 | 绑定技能 | 说明 |
| --- | --- | --- |
| 上下文收集 | `workspace-review` | 既有内置技能，`risk: read` |
| 需求澄清 | （无） | 无匹配通用技能，objective 驱动 |
| 架构影响 | （无） | 同上 |
| 详细设计 | （无） | 同上；`requirement-to-dev-design` 属公司特定流程，不适合 |
| 验收定义 | `test-case-design` | 验收标准须为"是/否"可判定条件，用例方法论直接适用 |

### 6. 技能生产计划（排除项的弥补）

排除的是"绑定 cn-codex 运行时的正文"，不是能力本身；弥补 = 方法论骨架保留 + 工具层用 k-Coder 真实工具重写。

| 优先级 | 产出 | 弥补对象 | 依赖 |
| --- | --- | --- | --- |
| P0 | `webapp-testing` 重写版 | cn-codex `webapp-testing` | `browser_*` 工具（已具备）+ 浏览器控制台捕获增强（见下） |
| P1 | `parallel-agents` 重写版 | `dispatching-parallel-agents` | 子智能体工具（已具备） |
| P1 | `subagent-development` 重写版 | `subagent-driven-development` | 同上 |
| P2 | `browser-automation` | `control-in-app-browser` | 无 |
| P2 | `docx-generation` | documents 插件 | 无 |

#### P0 前置：浏览器控制台捕获增强

cn-codex `browser_run` 是单工具批量动作模式（28 种动作，含 `eval` 页面内执行 JS），"注入控制台捕获脚本→回读 JS 错误→error 判测试失败"是其 E2E 质量门控核心。k-Coder 浏览器工具面为 6 个独立工具（`browser_navigate/click/type/snapshot/screenshot/close`），无 `eval`，无法复刻控制台检查。增强方案：`advanced/browser.rs` 订阅 CDP `Runtime.consoleAPICalled` / `Log.entryAdded` 事件，新增 `browser_console` 工具（有界缓冲读取），风险归入 `External`。增强就位前，重写版技能降级为"snapshot + 截图 + 接口响应判断"门控。

#### 重写版公共规则

- 子智能体类（P1）使用 k-Coder 的 `create_agent`/`wait_agent`/`send_agent_message`/`close_agent`，并写入边界：最多 4 个并发、派生深度一层、子智能体默认仅 `list_directory` + `read_file`、显式 Token 预算。
- 浏览器类写入前置规则："浏览器会话默认关闭，未启用时先引导用户开启，绝不伪造测试结果"。
- 不引入哨兵标记；推进仍只由 `complete_workflow_node` 决定。

### 7. 启动校验

所有内置工作流定义引用的技能 ID 必须能在注册表解析（含已禁用技能，禁用只影响正文注入）；任一缺失则扩展运行时关闭失败，与既有扩展加载失败语义一致。这保证绑定表与实际入库技能严格一致，复制遗漏会在启动时暴露而非静默退化。

## 影响

- 机器人驱动力从纯 prompt 升级为"角色 + 节点指令 + 节点技能正文"三层，复用既有 Skill 存储与三层来源解析，不新增第二套技能机制或第二套智能体循环。
- 技能正文进入 Provider 上下文会略增 token 消耗，注入边界保证单节点增量不超过约 12k tokens；Compaction 估算需把工作流技能正文计入运行时指令规模。
- `robot-pack` 分组目录 + 发现逻辑扩展是本期唯一 Rust 改动面；浏览器控制台捕获是可选增强（P0 前置），其余交付均为数据与测试。
- 通用方法论技能（superpowers 系列等）入库后同时服务普通对话（触发词机制）与机器人节点绑定，一份内容两处受益。

## 验收门槛

- Rust 测试覆盖：分组目录发现（深度 2、分组目录自身不解析为技能）、同作用域重名关闭失败、定义完整性校验（绑定 ID 全部可解析，缺失时扩展运行时关闭失败）、注入有界性（数量与字节上限）、缺失降级与审计、未启用 write/external 技能的占位注入、三层覆盖解析。
- 注入内容只存在于 Provider 请求，不写入 JSONL、不进入界面正文、不进入日志。
- 前端类型新增字段保持可选，旧快照与旧 mailbox 兼容；设置页机器人展示节点绑定技能，运行进度展示当前节点已注入技能。
- `pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test` 通过，并以 `pnpm tauri dev` 实际验证：项目线程选择机器人后当前节点技能可见、正文在对应节点生效、缺失技能走降级路径。

## 暂不交付

- 节点技能的可视化编排、技能版本指纹进扩展修订、从外部仓库自动抓取技能正文。
- 非功能测试（性能/安全）节点：`performance-testing`、`security-testing` 技能入库备用，待 QA 机器人扩充节点后再绑。

## 附录：cn-codex 技能验证与入库清单

目标位置：`src/resources/skills/robot-pack/<skill-id>/`。每个 SKILL.md 补 k-Coder frontmatter（`name` 与目录名一致、`description`、`triggers` 2-3 条、`risk`、`enabled`）。

### 直接复制（已验证正文无外部工具引用，共 22 个）

| 来源 | 目标 ID | risk 建议 |
| --- | --- | --- |
| `plugins/superpowers/skills/brainstorming` | `brainstorming` | read |
| `plugins/superpowers/skills/writing-plans` | `writing-plans` | read |
| `plugins/superpowers/skills/executing-plans` | `executing-plans` | read |
| `plugins/superpowers/skills/test-driven-development` | `tdd-implementation` | write |
| `plugins/superpowers/skills/systematic-debugging` | `systematic-debugging` | read |
| `plugins/superpowers/skills/verification-before-completion` | `verification-before-completion` | read |
| `plugins/superpowers/skills/requesting-code-review` | `requesting-code-review` | read |
| `plugins/superpowers/skills/receiving-code-review` | `receiving-code-review` | read |
| `plugins/superpowers/skills/finishing-a-development-branch` | `finishing-a-development-branch` | write |
| `plugins/superpowers/skills/using-git-worktrees` | `using-git-worktrees` | write |
| `skills/test-strategy-planning` | `test-strategy-planning` | read |
| `skills/test-case-design` | `test-case-design` | write |
| `skills/test-report-generation` | `test-report-generation` | write |
| `skills/api-testing` | `api-testing` | write |
| `skills/performance-testing` | `performance-testing` | write |
| `skills/security-testing` | `security-testing` | write |
| `skills/taste-skill` | `taste-skill` | read |
| `skills/awesome-design-md` | `awesome-design-md` | read |
| `skills/requirements-intake` | `requirements-intake` | read |
| `skills/prd-story-modeler` | `prd-story-modeler` | read |
| `skills/prd-delivery-review` | `prd-delivery-review` | read |
| `skills/create-plan` | `create-plan` | write |

### 带同目录脚本复制（1 个）

| 来源 | 说明 |
| --- | --- |
| `skills/dingtalk-document`（含 `scripts/dt_helper.sh`） | 正文走 `bash scripts/dt_helper.sh` + curl，k-Coder `run_command` 可执行；`risk: external`；复制前人工检查脚本内无硬编码凭据 |

### 排除（正文引用 k-Coder 不存在的运行时，按技能生产计划弥补）

| 技能 | 实锤依据 |
| --- | --- |
| `webapp-testing` | 正文引用 `browser_run`/`exec_command`/`write_stdin`，且要求先读 `codey/skills/browser/SKILL.md` |
| `control-in-app-browser`（browser 插件） | 依赖 `mcp__node_repl__js`、`scripts/browser-client.mjs`、`agent.browsers.*` API |
| `dispatching-parallel-agents`（superpowers） | 示例为 Claude Code `Task("...")` 派发模式 |
| `subagent-driven-development`（superpowers） | 依赖同目录 `implementer-prompt.md` 等文件 + Claude Code 工具名 |
| `documents`（documents 插件） | 依赖容器内脚本链路，未完整验证 |
