# ADR 0055：机器人节点技能绑定、插件兼容与公共技能分类

- 状态：已接受
- 日期：2026-09-08

## 背景

k-Coder 已有三个内置机器人和由宿主维护的工作流状态，但当前节点只有角色、指令和完成条件，没有节点级 Skill。cn-codex 的机器人设置界面展示了本地 Skill、插件 Skill、工作流步骤和 System Prompt；代码核查确认真正进入运行时的是目标、节点状态和当前节点 Skill 正文，配置中的 System Prompt 并未注入。其实现还存在缺失 Skill 静默跳过、插件启用状态未复核、直接拼路径读取和可伪造完成哨兵等问题。

用户提供的界面版本比仓库中的旧 `robot.json` 更新，目标规格以界面为准：

- 全栈开发机器人：23 个声明，8 个步骤；
- 软件测试机器人：16 个声明，7 个步骤；
- 需求设计机器人：9 个声明，7 个步骤。

本决策迁移完整声明和步骤，但不重复 cn-codex 的失效 System Prompt、重复正文注入或安全缺陷。

## 决策

### 1. 单一运行时与两类声明

机器人继续复用主 AgentRuntime 和 ExtensionService，不建立第二套智能体循环或普通 Skill 注册表。节点绑定使用两类静态声明：

```rust
enum WorkflowSkillBindingDefinition {
    Skill {
        skill_id: &'static str,
    },
    PluginSkill {
        plugin_id: &'static str,
        skill_id: &'static str,
        fallback_skill_id: Option<&'static str>,
    },
}
```

- `Skill` 始终解析机器人包中的受管内置 Skill。机器人包是保留命名空间，不能被全局/项目同名 Skill 覆盖；旧测试夹具或旧资源包没有机器人包时才允许回退到内置根目录中的兼容 Skill。
- `PluginSkill` 优先解析已安装、已启用且可读取的插件 Skill。
- 每个 cn-codex 插件声明都有明确的内置兼容实现时，可以在插件缺失、禁用或正文超限时解析到 `fallback_skill_id`。插件是增强项，不是机器人启动前提。
- UI 仍显示原始声明，例如 `superpowers/writing-plans`，并标记实际来源为“插件”或“内置兼容”。
- 没有可用插件也没有可用 fallback 的声明严格阻断。

插件兼容映射如下：

| 插件声明 | 内置兼容 Skill |
| --- | --- |
| `superpowers/brainstorming` | `brainstorming` |
| `superpowers/writing-plans` | `writing-plans` |
| `superpowers/executing-plans` | `executing-plans` |
| `superpowers/test-driven-development` | `test-driven-development` |
| `superpowers/systematic-debugging` | `systematic-debugging` |
| `superpowers/verification-before-completion` | `verification-before-completion` |
| `superpowers/requesting-code-review` | `requesting-code-review` |
| `superpowers/receiving-code-review` | `receiving-code-review` |
| `superpowers/using-git-worktrees` | `using-git-worktrees` |
| `superpowers/finishing-a-development-branch` | `finishing-a-development-branch` |
| `superpowers/dispatching-parallel-agents` | `dispatching-parallel-agents` |
| `superpowers/subagent-driven-development` | `subagent-driven-development` |
| `browser/control-in-app-browser` | `control-in-app-browser` |
| `documents/documents` | `documents` |

同一节点中的本地副本和插件/fallback 可能解析到相同内容。运行时按规范化正文 SHA-256 去重，只注入一次，头部保留所有原始声明和解析来源。

### 2. 公共 Skill 分类

普通 Skill 固定使用以下分类：

- `requirements_planning`
- `development_delivery`
- `quality_review`
- `testing`
- `design_experience`
- `data_documents`
- `observability`
- `integration_automation`
- `extension_platform`
- `other`

`SkillMetadata` 和 `SkillDiagnostic` 增加 `category`。缺少字段的旧全局/项目 Skill 归入 `other`；显式非法值关闭失败。现有 31 个内置 Skill 全部补明确分类。

本期新增 28 个公共内置 Skill，而不是先前讨论中的 23 个。新增项包括原 23 个以及界面核对后确认缺少的 `dispatching-parallel-agents`、`subagent-driven-development`、`webapp-testing`、`control-in-app-browser`、`documents`。完成后普通内置 Skill 共 59 个。

### 3. 分组目录与发现

新增 Skill 放在 `src/resources/skills/robot-pack/<skill-id>/`。发现器仅支持平铺 Skill 和一层分组目录，不递归进入 Skill 自身的 `references/` 或 `scripts/`。

所有根、分组、Skill 目录和正文路径在读取前规范化；拒绝符号链接和 Windows 目录联接逃逸。同一作用域的平铺/分组重复 ID 关闭失败，不再静默覆盖；不同作用域继续按现有优先级覆盖。

### 4. 严格预检与注入边界

启动或恢复机器人前，对整个工作流的所有声明执行预检：机器人包缺失、插件/fallback 均不可用、正文过大或节点总量超限时，不创建工作流、不开始 Turn，并通过 `workflow_skill_preflight_failed` 的结构化 details 一次列出全部问题；机器人包本身不会因普通启用设置变成 disabled。

完整 cn-codex 节点最多有 18 个声明，因此边界调整为：

- 每节点最多 24 个声明；
- 解析并按正文哈希去重后最多 24 个正文；
- 单正文最多 16 KiB；
- 当前节点正文合计最多 128 KiB。

超限关闭失败，不静默截断。机器人包 Skill 在发现时强制启用，设置页显示“机器人必需”且拒绝禁用；不得自动启用普通 Skill、插件或 MCP。

活动工作流在每次 Provider 请求前重新预检。运行中配置变化使当前解析失效时，下一次请求失败，工作流保持 active，修复后可继续。

### 5. 逐请求动态指令

AgentRuntime 接收 `RuntimeInstructionProvider`。每个外层 Provider 请求在上下文估算和自动压缩之前生成一次指令快照；同一次请求的瞬时网络重试复用该快照。`complete_workflow_node` 仍是唯一推进方式，更新状态后下一次请求重新编译新的节点和 Skill。

Skill 正文只存在于瞬时 Provider 请求，不写 JSONL、不进入 UI、不进入日志。审计只记录机器人、节点、声明、解析来源、正文哈希和字节数。动态区块末尾固定重申宿主边界：Skill 不授予工具、不改变审批、不启用插件/MCP/Hook、不扩大工作区且不能绕过 PolicyEngine。

### 6. Skill 参考资源

新增只读 `skill_resource_read` 工具，读取当前有效且已启用普通 Skill 的 UTF-8 参考资源。参数只接受 Skill ID、相对路径和可选范围；拒绝绝对路径、父目录、目录、二进制、链接/目录联接逃逸和越界读取。工具不执行 Skill 内脚本。

自动注入正文不持久化；显式资源读取作为普通有界工具结果持久化和审计。

### 7. 28 个迁移/适配 Skill

| 分类 | Skill |
| --- | --- |
| 需求与规划 | `brainstorming`、`writing-plans`、`requirements-intake`、`prd-story-modeler`、`prd-delivery-review`、`create-plan` |
| 开发与交付 | `executing-plans`、`test-driven-development`、`using-git-worktrees`、`finishing-a-development-branch`、`dispatching-parallel-agents`、`subagent-driven-development` |
| 质量与评审 | `systematic-debugging`、`verification-before-completion`、`requesting-code-review`、`receiving-code-review` |
| 测试工程 | `test-strategy-planning`、`test-case-design`、`test-report-generation`、`api-testing`、`performance-testing`、`security-testing`、`webapp-testing` |
| 设计与体验 | `taste-skill`、`awesome-design-md` |
| 数据与文档 | `documents` |
| 集成与自动化 | `dingtalk-document`、`control-in-app-browser` |

所有正文适配 k-Coder 的真实工具名和仓库规则。移除自动 commit/push、固定外部容器、Claude `Task`/`TodoWrite` 和根目录强制文档路径。大体积内容拆为 16 KiB 内入口和按需参考资源。

`documents` 内置兼容 Skill 负责 Markdown 和结构化文档交付；只有真实 `documents` 插件可用时才承诺 DOCX。`control-in-app-browser` 适配 k-Coder 的 browser 工具面。`webapp-testing` 不假设不存在的页面 JS eval/console 工具，无法读取控制台时必须明确记录验证限制。

`dingtalk-document` 不迁移明文保存 App Secret/Access Token 的 shell helper，只允许使用用户已经配置的钉钉 MCP；无兼容工具时停止，不回退 curl 或命令行密钥。

Apache-2.0 和 superpowers MIT 来源许可证及 NOTICE 随 robot-pack 打包。

## 完整工作流

以下 `L:` 表示本地普通 Skill 声明，`P:` 表示插件 Skill 声明。插件声明按前述兼容表解析。

### 全栈开发机器人：23 个声明，8 步

1. 需求理解与分析：L `writing-plans`、`executing-plans`、`verification-before-completion`；P `superpowers/writing-plans`、`superpowers/executing-plans`、`superpowers/verification-before-completion`、`browser/control-in-app-browser`、`documents/documents`。
2. 界面与架构设计：L `brainstorming`、`writing-plans`、`taste-skill`、`awesome-design-md`、`verification-before-completion`；P `superpowers/brainstorming`、`superpowers/writing-plans`、`superpowers/verification-before-completion`、`browser/control-in-app-browser`、`documents/documents`。
3. 原型 HTML：L `taste-skill`、`awesome-design-md`、`verification-before-completion`；P `browser/control-in-app-browser`、`superpowers/verification-before-completion`。
4. 后端开发：L `test-driven-development`、`executing-plans`、`verification-before-completion`、`requesting-code-review`、`dispatching-parallel-agents`；P `superpowers/test-driven-development`、`superpowers/executing-plans`、`superpowers/verification-before-completion`、`superpowers/requesting-code-review`、`superpowers/receiving-code-review`、`superpowers/systematic-debugging`、`superpowers/subagent-driven-development`、`superpowers/using-git-worktrees`、`superpowers/dispatching-parallel-agents`、`superpowers/finishing-a-development-branch`、`browser/control-in-app-browser`。
5. 前端开发：L `test-driven-development`、`executing-plans`、`verification-before-completion`、`requesting-code-review`、`taste-skill`、`awesome-design-md`、`dispatching-parallel-agents`；插件声明与后端开发相同。
6. 全面测试：L `test-driven-development`、`executing-plans`、`verification-before-completion`；P `superpowers/test-driven-development`、`superpowers/executing-plans`、`superpowers/verification-before-completion`、`superpowers/systematic-debugging`、`browser/control-in-app-browser`。
7. 构建与发布：L `executing-plans`、`verification-before-completion`；P `superpowers/executing-plans`、`superpowers/verification-before-completion`。
8. 代码审查与交付：L `requesting-code-review`、`verification-before-completion`；P `superpowers/requesting-code-review`、`superpowers/receiving-code-review`、`superpowers/verification-before-completion`、`superpowers/finishing-a-development-branch`。

### 软件测试机器人：16 个声明，7 步

1. 测试策略制定：L `test-strategy-planning`；P `superpowers/writing-plans`、`superpowers/verification-before-completion`。
2. 测试用例设计：L `test-case-design`、`test-strategy-planning`；P `superpowers/writing-plans`、`superpowers/verification-before-completion`、`documents/documents`。
3. 单元测试：L `test-case-design`；P `superpowers/test-driven-development`、`superpowers/systematic-debugging`、`superpowers/executing-plans`、`superpowers/verification-before-completion`、`superpowers/dispatching-parallel-agents`。
4. 集成/API 测试：L `api-testing`、`test-case-design`；P 与单元测试相同。
5. E2E/UI 测试：L `webapp-testing`、`test-case-design`；P `browser/control-in-app-browser`、`superpowers/systematic-debugging`、`superpowers/executing-plans`、`superpowers/verification-before-completion`。
6. 非功能测试：L `performance-testing`、`security-testing`；P `superpowers/systematic-debugging`、`superpowers/executing-plans`、`superpowers/verification-before-completion`。
7. 测试报告与交付：L `test-report-generation`；P `superpowers/verification-before-completion`、`documents/documents`。

### 需求设计机器人：9 个声明，7 步

1. 需求采集与理解：L `requirements-intake`；P `superpowers/brainstorming`、`superpowers/verification-before-completion`。
2. 业务边界划定：L `requirements-intake`、`create-plan`；P `superpowers/brainstorming`、`superpowers/writing-plans`、`superpowers/verification-before-completion`。
3. 用户故事与功能建模：L `prd-story-modeler`；P `superpowers/verification-before-completion`。
4. 交互流程设计：L `prd-story-modeler`；P `superpowers/brainstorming`、`superpowers/verification-before-completion`。
5. PRD 整合与审查：L `prd-delivery-review`；P `superpowers/verification-before-completion`。
6. 文档格式化输出：L `prd-delivery-review`；P `superpowers/verification-before-completion`、`documents/documents`。
7. 发布到钉钉知识库：L `dingtalk-document`；P `superpowers/verification-before-completion`。

## 设置界面

Settings -> Skills 对 59 个普通内置 Skill 及全局/项目 Skill 使用固定分类分组，提供搜索、分类和来源筛选。机器人包 Skill 仍在同一页展示，但显示“机器人必需”并隐藏启用开关；插件 Skill 仍位于 Plugins 页面。

Settings -> Robots 显示原始本地/插件声明数量、节点声明和解析状态，包括 plugin、builtin fallback、disabled、missing、oversized。机器人选择器显示阻塞数量；已知阻塞时前端引导到筛选后的 Skills/Plugins 页面，但后端始终重新预检。

## 兼容与非目标

- 三个工作流的节点数会变化；活动旧运行按工作流 ID 和旧节点 ID 恢复时需要版本兼容或明确取消，不能错配索引。
- 工作流快照不保存正文和解析来源。
- 本期不提供自定义机器人 CRUD、可视化编排或钉钉专用凭据客户端。
- 本期不声称内置 `documents` fallback 能生成 DOCX；该能力取决于真实插件。

## 验收

- Rust 覆盖分类、二级发现、重复 ID、三层覆盖、插件/fallback 解析、哈希去重、链接逃逸、资源读取、聚合预检、旧运行兼容、逐请求节点切换和正文不落盘。
- 前端覆盖分类/来源筛选、`23/16/9` 声明计数、节点解析状态、阻塞导航及窄屏布局。
- `pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test` 全部通过。
- 使用隔离应用标识启动 `pnpm tauri dev`，实测分类、插件 fallback、严格阻塞、启用后启动、节点推进换 Skill 和运行中禁用后的恢复。
