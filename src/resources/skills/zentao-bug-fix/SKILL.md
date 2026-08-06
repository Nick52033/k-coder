---
name: zentao-bug-fix
description: 处理禅道 Bug：查询并让用户选择，拉取详情，在代码仓库定位问题；先将修复文档写入 `docs/lessons`，人工确认后改代码。验证通过后更新文档、commit/push、创建 MR，并调用 zentao-bug-resolve 标记已解决；纯查询请用 zentao-bugs。
triggers:
  - 修bug
  - 修复禅道
  - 处理bug
  - 自动修bug
  - zentao bug fix
  - "修复 #123"
  - 验证通过
risk: external
enabled: true
---

# 禅道 Bug 修复（文档先行 + 人工确认）

## 核心原则

1. **先文档、后代码**：未完成人工确认前，**禁止**修改业务代码、禁止 commit/push。
2. **用户选择**：必须展示 Bug 列表，由用户选定要处理的 Bug（或用户直接给出 Bug ID）。
3. **知识库落盘**：分析结果写入目标仓库 `docs/lessons/zentao-bugs/`，便于复用与评审。
4. **最小改动**：确认后改代码时，遵循目标仓库 `AGENTS.md` / `CLAUDE.md` 规范。

## 阶段一：查询列表并让用户选择

### 1.1 获取 Bug 列表

凭据与列表 API 见 [zentao-api.md](references/zentao-api.md)。列表查询规则与 `zentao-bugs` 一致：

- 默认：**我的 bug**（`browseType=assigntome`，过滤 `assignedTo.account` = 当前用户）
- 用户指定「团队」时：`browseType=unresolved`，按 Bug `id` 去重
- 只保留 `status` = `active`

PowerShell 快捷方式（可选）：

```powershell
& "$env:USERPROFILE\.cursor\scripts\Get-ZentaoMyBugs.ps1" -Mode mine
```

### 1.2 展示待选列表

按严重程度、优先级排序，输出编号表：

```markdown
| 序号 | Bug ID | 标题 | 严重程度 | 优先级 | 产品 | 模块 |
|------|--------|------|----------|--------|------|------|
| 1    | 12345  | ...  | 严重     | 高     | ...  | ...  |
```

### 1.3 等待用户选择

- 用户已给出 Bug ID（如「修复 #12345」）→ 跳过选择，直接进入阶段二
- 否则用 **AskQuestion**（选项 ≤ 4）或请用户回复 **序号 / Bug ID**
- **未收到明确选择前，不得进入分析与写文档**

## 阶段二：拉取详情、定位、生成处理文档

### 2.1 获取 Bug 详情

```
GET /api.php/v1/bugs/{bugID}
```

提取：`title`、`steps`（重现步骤）、`product`/`module`、`severity`、`pri`、`assignedTo`、`openedBy`、`keywords`、`type`、`os`、`browser`、`mailto`、备注类字段。

禅道链接：`https://zentao.isscloud.com/bug-view-{bugID}.html`

### 2.2 映射代码仓库

按 [repo-mapping.md](references/repo-mapping.md) 将产品/模块/关键词映射到 `D:\ProjectCode\{项目}`。

映射不确定时：**列出 2～3 个候选仓库让用户确认**，再继续。

### 2.3 检索知识库与代码

在目标仓库依次执行：

1. **已有经验**：`docs/lessons/` 及 `D:\1.CursorProject\文档\{项目名}\` 搜索 Bug ID、模块名、报错关键词
2. **代码定位**：SemanticSearch + Grep，结合 Bug 标题、步骤、模块名、接口名、页面路径
3. **相关规范**：读取目标仓库 `AGENTS.md` / `CLAUDE.md`、`.cursorrules`

记录：疑似文件路径、类/方法、数据表（如能从代码或 MCP 推断）、调用链简述。

### 2.4 生成处理文档（必须执行）

模板见 [fix-document-template.md](references/fix-document-template.md)。

**输出路径**（按优先级）：

1. `{目标仓库}/docs/lessons/zentao-bugs/zentao-bug-{id}-{slug}.md`
2. 仓库未确定时：`D:\1.CursorProject\文档\{项目名}\03-缺陷修复\zentao-bug-{id}-{slug}.md`

`slug`：从标题取 2～4 个英文关键词，小写、`-` 连接。

文档状态字段：**`待确认`**。

### 2.5 向用户汇报（本阶段结束）

```
已生成 Bug 修复处理文档：
- 路径：{完整路径}
- Bug：#{id} {标题}
- 目标仓库：{仓库名}
- 初步结论：{一句话}
- 建议改动：{文件列表摘要}

请审阅文档后回复：
- 「确认修复」→ 进入阶段三改代码
- 「修改方案」+ 意见 → 更新文档后再次确认
- 「取消」→ 仅保留文档，不改代码
```

**本阶段禁止**：修改 `.cs`、`.ts`、`.ascx` 等业务源码（读取、搜索除外）。

## 阶段三：人工确认后改代码

仅当用户明确回复 **「确认修复」**、**「开始改代码」**、**「按方案执行」** 等肯定意图时执行。细则见 [code-fix-phase.md](references/code-fix-phase.md)。

概要：

1. 将文档状态更新为 **`修复中`**
2. 按文档「建议改动」做**最小必要**代码修改
3. 跑目标仓库相关测试（`dotnet test` 等）
4. 更新文档：补充实际改动、测试结果，状态改为 **`已修复待验证`**
5. 向用户展示 diff 与测试结论，**不自动 commit/push**（阶段三内）

请本地验证后回复 **「验证通过」** → 自动进入阶段四（见下）。

## 阶段四：验证通过收尾（自动执行）

用户回复 **「验证通过」**（或同义）时，**在本技能内自动执行**，细则见 [verify-passed-phase.md](references/verify-passed-phase.md)。

| 步骤 | 动作 |
|------|------|
| 4.1 | 知识库文档状态 → **已解决**，补充验证记录 |
| 4.2 | 各相关仓库 **commit + push**（仅 Bug 相关文件） |
| 4.3 | **创建合并请求**（CodeHub MR / GitHub PR，多仓库各一条） |
| 4.4 | 读取并执行 **`zentao-bug-resolve`**：`Set-ZentaoBugResolved.ps1` 标记禅道 **resolved** |
| 4.5 | 汇报文档路径、commit、MR 链接、禅道状态 |

**无需**用户再单独说「提交」「推送」「建 MR」「标记已解决」。

关联技能：

| 技能 | 阶段四中的用途 |
|------|----------------|
| **zentao-bug-resolve** | 禅道 confirm + resolve（POST，操作后验证 status） |
| split-to-prs | 仅当改动需拆成多个 MR 时参考，默认单 Bug 单 MR/仓 |

commit message：`fix: 禅道 #{id} {简短标题}`

默认**不关闭**禅道 Bug；用户明确要求关闭时，resolve 脚本加 `-CloseAfterResolve`。

## 异常处理

| 情况 | 处理 |
|------|------|
| 登录 401 | 提示检查 `zentao.config.json` 或环境变量 |
| Bug 列表为空 | 告知无待处理 Bug，结束 |
| 无法定位仓库 | 暂停，请用户指定项目名或路径 |
| 无法定位代码 | 文档中标注「待人工定位」，列出已搜索关键词与候选文件 |
| 文档目录不存在 | 自动创建 `docs/lessons/zentao-bugs/` |

## 参考文件

| 文件 | 用途 |
|------|------|
| [workflow.md](references/workflow.md) | **完整处理工作流**（四阶段、状态机、门禁） |
| [zentao-api.md](references/zentao-api.md) | 登录、列表、详情、备注 API |
| [repo-mapping.md](references/repo-mapping.md) | 禅道产品 → 代码仓库 |
| [fix-document-template.md](references/fix-document-template.md) | 处理文档模板 |
| [code-fix-phase.md](references/code-fix-phase.md) | 阶段三改代码细则 |
| [verify-passed-phase.md](references/verify-passed-phase.md) | **阶段四**：验证通过 → 文档/commit/MR/禅道 resolve |
| [zentao-bug-resolve](../zentao-bug-resolve/SKILL.md) | 阶段四 4.4 禅道已解决 |
