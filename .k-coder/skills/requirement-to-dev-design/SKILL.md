---
name: requirement-to-dev-design
description: "从钉钉或本地需求文档自动生成开发详细设计（公司模板1.7.2），本地Markdown与钉钉文档双写。检索代码仓库、映射3.3.1与7.2接口、2.10 FMEA故障分析、开发经理评审门禁。支持 user-config.md 自定义钉钉输出目录nodeId、代码根目录与本地文档路径。触发词：生成开发设计、详细设计、开发详细设计、按模板写设计、钉钉需求链接。"
triggers:
  - 生成开发设计
  - 详细设计
  - 开发详细设计
risk: external
enabled: true
---

# 需求 → 开发详细设计

根据可变需求文档，按固定详细设计模板生成开发设计，**本地 + 钉钉双写**。

> **用户指南**（技能广场下载者）：见 [references/user-guide.md](references/user-guide.md)  
> **自定义路径**：复制 [user-config.example.md](user-config.example.md) 为 `user-config.md` 并修改钉钉 `outputFolderId`、本地 `codeRoot` / `localDocRoot`。

## 触发条件

- 用户说「生成开发设计」「详细设计」「开发详细设计」「按模板写设计」
- 用户提供钉钉需求文档链接或本地需求文件
- 用户提供目标项目名（如 paypro-platform-be、ipsa-rra）

## 固定配置

**配置优先级**（高 → 低）：

1. 用户对话中显式指定的路径 / nodeId / folderId  
2. skill 根目录 **`user-config.md`**（由 [user-config.example.md](user-config.example.md) 复制而来）  
3. 下文默认值 + [references/dingtalk-config.md](references/dingtalk-config.md)

执行开始时：若存在 `user-config.md`，读取并覆盖下表对应项；若用户对话又指定了同项，以对话为准。

| 配置项 | 默认键名 | 默认值（可被 user-config 覆盖） |
|--------|----------|--------------------------------|
| MCP 服务器 | `mcpServerName` | **`钉钉文档`**（`~/.cursor/mcp.json`） |
| 项目代码根目录 | `codeRoot` | **`D:\code\ai\`** |
| 本地设计文档根 | `localDocRoot` | **`D:\code\ai\文档`**（其下 `{项目名}/02-技术文档/`） |
| 详细设计模板 nodeId | `designTemplateNodeId` | `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9` |
| **钉钉输出目录 folderId** | **`outputFolderId`** | **`3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6`** |
| 仅本地不写钉钉 | `skipDingtalkWrite` | `false` |
| 变更记录修改人 | `defaultAuthor` | 空（用当前用户） |
| 离线模板 | — | 本 skill `templates/详细设计模板.md` |

模板 URL（默认）：https://alidocs.dingtalk.com/i/nodes/qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9  
输出目录 URL（默认）：https://alidocs.dingtalk.com/i/nodes/3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6  

**路径约定：**

- 代码仓库：`{codeRoot}\{项目名}\`
- 设计文档：`{localDocRoot}\{项目名}\02-技术文档\{需求分组}\{设计主题}开发设计.md`（命名规则见 [naming-rules.md](references/naming-rules.md)）

> 调用 MCP 前必须先读取工具 schema。MCP 不可用时走回退链，**禁止编造需求**。`skipDingtalkWrite=true` 时跳过 §6 钉钉写入。

---

## 执行流程

### 0. 从上下文自动提取链接（最高优先级）

在收集信息前，先从用户输入/会话上下文提取钉钉文档信息：

#### 0.1 识别链接格式

```
https://alidocs.dingtalk.com/i/nodes/{nodeId}?...
```

`nodeId` 为 URL 中 `/nodes/` 后、`?` 前的字符串；**也可直接把完整 URL 传给 MCP**。

#### 0.2 链接分类（自动填充）

| 链接 nodeId | 用途 | 自动动作 |
|-------------|------|----------|
| `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9` | 详细设计模板 1.7.2 | **只读**；仅 copy 源或同步模板时读取，**禁止**写入/留存在开发提测文档目录 |
| `3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6` | 开发提测文档（输出目录） | 仅存放 `<主题>开发设计` 成品；**禁止**出现「详细设计模板*」 |
| **其他 nodeId** | **需求文档** | `get_document_info` 取标题 → `get_document_content` 读正文 |

#### 0.3 多个需求链接时

让用户确认以哪个为准；未指定时取用户消息中**第一个非模板/非输出目录**的链接。

**优先级**：钉钉需求链接 MCP 读取 > 用户粘贴正文 > 本地文件 > search_documents > 本地文档库 > 询问用户

---

### 1. 收集任务信息

从用户输入提取，缺失则主动询问：

| 字段 | 必填 | 说明 | 默认值 |
|------|------|------|--------|
| 需求来源 | 是 | 钉钉 URL / 本地路径 / 粘贴正文 | 步骤 0 自动提取 |
| 需求文档名称 | 是 | 钉钉 `get_document_info.name` 或本地文件名 | 读取需求时自动获取 |
| 需求分组 | 自动 | 名称**第一个横杠**前半段 → 子文件夹名 | [naming-rules.md](references/naming-rules.md) |
| 设计主题 | 自动 | 名称**第一个横杠**后半段；无横杠则为全称 | 同上 |
| 目标项目 | 是 | 项目名或仓库路径 | 仅项目名时解析为 `D:\ProjectCode\{项目名}` |
| 已有设计 nodeId | 否 | 提供则 **覆盖更新** 该文档 | 无则新建 |
| 修改人 | 否 | 变更记录用 | 当前用户 |

用户显式提供的「需求主题」可覆盖「设计主题」；分组仍优先从需求文档名解析。

#### 1.0 解析需求文档名称（必做）

读取需求后立即执行，细则见 [naming-rules.md](references/naming-rules.md)：

1. **钉钉**：`get_document_info(nodeId)` → `name`
2. **本地文件**：文件名去扩展名
3. 按第一个 `-` / `－` / `—` / `–` 拆分为 `{需求分组}` + `{设计主题}`
4. 无横杠 → 不分组，设计主题 = 全称

**示例**：`入职流程改进－业务流程信息变更提效` → 分组 `入职流程改进`，主题 `业务流程信息变更提效`，文档名 `业务流程信息变更提效开发设计`

#### 1.1 读取需求正文

**钉钉链接（首选）** — `CallMcpTool`：

```
server: 钉钉文档
toolName: get_document_content
arguments: { "nodeId": "<需求URL或nodeId>", "format": "markdown" }
```

异常处理：

| 现象 | 处理 |
|------|------|
| MCP 不可用 | 回退：粘贴正文 / 本地文件 / 请用户导出 |
| 401/无权限 | 提示开通钉钉文档读权限 |
| 非在线文档（表格/多维表） | `get_document_info` 判断；请用户导出 Word/Markdown |
| 正文为空 | 换 `format: jsonml` 或请用户粘贴 |

**本地文件**：

- `.docx` → `python scripts/extract_docx.py <路径>`
- `.xlsx` → `python scripts/extract_xlsx.py <路径>`
- `.md` → 直接读取

**输出**：需求摘要（功能清单、业务规则、接口/数据、非功能、本期/二期范围）。

---

### 2. 匹配目标项目

**代码根目录：`D:\ProjectCode`**。用户只给项目名时，仓库路径 = `D:\ProjectCode\{项目名}`。

#### 2.1 路径解析

| 用户输入 | 解析结果 |
|----------|----------|
| `paypro-platform-be` | `D:\ProjectCode\paypro-platform-be` |
| `D:\ProjectCode\ioa-be` | 原样使用 |
| 相对路径 `xxx` | 先尝试 `D:\ProjectCode\xxx` |
| 目录不存在 | 在 `D:\ProjectCode` 下列出相似目录，请用户确认 |

#### 2.2 常用项目（完整列表见 [references/project-mapping.md](references/project-mapping.md)）

| 项目名 | 仓库路径 | 技术栈 |
|--------|----------|--------|
| paypro-platform-be | `D:\ProjectCode\paypro-platform-be` | ABP 8 + .NET 8 + SqlSugar + MySQL + Vue |
| ioa-be | `D:\ProjectCode\ioa-be` | ABP 8 + .NET 8 + SqlSugar + 多库 |
| ipsa-rra | `D:\ProjectCode\ipsa-rra` | .NET Framework + WebForms + SQL Server |
| ipsa-ioa | `D:\ProjectCode\ipsa-ioa` | 混合架构 |
| ipsa-rms | `D:\ProjectCode\ipsa-rms` | 招聘管理 |
| recruitment-management-be | `D:\ProjectCode\recruitment-management-be` | .NET 后端 |
| entry-approval-mobile-be | `D:\ProjectCode\entry-approval-mobile-be` | 入职审批移动端 |
| employment-relation-be | `D:\ProjectCode\employment-relation-be` | 劳动关系 |
| hr-api / hr-core-api | `D:\ProjectCode\hr-api` 等 | HR 公共 API |
| hr-data-sync-service | `D:\ProjectCode\hr-data-sync-service` | 数据同步 |

#### 2.3 代码检索

在解析后的仓库路径下检索：`CLAUDE.md`/`AGENTS.md`、Controller/Entity/Service、appsettings；参考 `D:\1.CursorProject\文档\<项目>\02-技术文档\` 已有设计；可选 db-* MCP 查表。

---

### 3. 加载详细设计模板

| 模式 | 方式 |
|------|------|
| **默认** | 读 `templates/详细设计模板.md` |
| 同步钉钉最新 | MCP `get_document_content(nodeId=qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9)` |

**结构约束**：

- 正文格式**必须**遵循 [references/format-guide.md](references/format-guide.md)（对齐钉钉模板 1.7.2）
- 正文**不要**写 `# 【开发设计】` 标题；文档名仅用于钉钉 `name` 与本地文件名
- 保密块用红色 `<span style="color: rgb(216, 57, 49);">` + 橙色背景保密声明表
- 章节标题用 `# **1、 设计概述**` 等加粗格式
- 保留 `# 1`～`# 7` 一级章节；删除 `:::` 提示块；无内容写「不涉及」
- **大纲结构**必须遵循 [references/outline-structure.md](references/outline-structure.md)（7 章顺序固定，7.3 为正文末节）

章节映射细则见 [references/section-mapping.md](references/section-mapping.md)。

---

### 4. 生成设计正文

**生成前（必做）** — 按 [review-checklist.md](references/review-checklist.md) §三 完成：

1. 需求条目 → 3.3.1 映射表（未映射标待确认）
2. 代码现状扫描（`D:\ProjectCode\{项目}`）：已实现标「已实现/沿用」，勿重复设计
3. 外部依赖登记表（接口、负责人、预计就绪时间、Mock 策略）
4. 本期/二期剪刀 → 写入 3.2
5. 按项目类型勾选 review-checklist §二 附加项
6. **FMEA 草稿**（必做）— 按 [fmea-guide.md](references/fmea-guide.md) 从 3.3.1 模块、外部依赖、敏感/金额场景列失效模式，再写 2.10 正文

按模板填充，原则：

- 需求有、代码无 → 写设计并标「待开发」
- 需求有、**代码已有** → 标「已实现/沿用」，设计细节写现状与增量改动点（评审常否定过度设计）
- 需求与代码不一致 → 以需求为准，注明差异
- 缺失信息 → 「待确认：xxx」，不虚构
- 每个功能点 `3.3.x`：**设计细节首段**写「现状 → 目标 → 影响范围」；特性设计九项 + 设计细节 + 数据库设计
- `3.3.1` 功能清单与 `7.2` 接口**严格一一对应**（含列表/导出/报表类，漏项是高频评审意见）
- **2.8** 至少 3 行事件，含量化触发频率
- **2.10 故障分析（开发经理重点）** — 必须按 [fmea-guide.md](references/fmea-guide.md) 写完整 FMEA 表 + ≥2 段总结；表体行数 ≥ max(3, 3.3.1 模块数)；每个外部依赖、敏感数据、金额计算至少 1 行；**禁止**全章「不涉及」
- 重构类需求：**1.2/3.1** 写效益量化（时长/人天/成本）；**2.1** 写旧/新架构对比
- 更新已有设计 → 变更记录说明**响应哪条评审意见**（见 review-checklist §五）

**文档头**：按 [format-guide.md](references/format-guide.md) 输出保密块 + 变更记录（v1.0 创建或根据需求文档初版）；**禁止**使用 `_**保密级别**_` 斜体写法。

---

### 5. 本地落盘（必做）

按 [naming-rules.md](references/naming-rules.md) 确定路径，**子目录不存在则新建**：

```
有分组：
D:\1.CursorProject\文档\<项目名>\02-技术文档\<需求分组>\<设计主题>开发设计.md

无分组：
D:\1.CursorProject\文档\<项目名>\02-技术文档\<设计主题>开发设计.md
```

可选：同目录输出 `_需求摘要.md`（与开发设计同文件夹）。

---

### 6. 写入钉钉文档

#### 6.1 判断是否新建或更新

1. 用户提供了 **已有设计 nodeId** → 更新
2. 否则解析目标钉钉文件夹（见 [naming-rules.md](references/naming-rules.md)）：
   - 有 `{需求分组}`：`list_nodes`(folderId=开发提测文档) 查找子文件夹 → 无则 `create_folder`
   - 无分组：目标文件夹 = 开发提测文档根目录
3. 在目标文件夹内 `list_nodes` 查 `{设计主题}开发设计` 同名文档：

```
server: 钉钉文档
toolName: list_nodes
arguments: { "folderId": "<开发提测文档或需求分组子文件夹 nodeId>", "pageSize": 50 }
```

- 同名已存在 → 默认 **覆盖更新**；用户要求时可新建副本
- 不存在 → **新建**

#### 6.2 新建文档（复制模板，保留页面宽度）

**禁止**对开发设计使用 `create_document` 空白新建——空白文档默认**全宽**布局，与模板 1.7.2 的**居中窄页**不一致。

**禁止**将「详细设计模板 1.7.2」以模板名称留存在开发提测文档目录：
- `copy_document` 仅为中间步骤，复制后**必须立即** `rename_document` 为 `{设计主题}开发设计`
- **禁止**用 `create_document` / 本地 `templates/详细设计模板.md` 整篇上传到开发提测文档
- 生成完成后 `list_nodes` 清理名称匹配 `详细设计模板` 的误留副本（`delete_document`）

按顺序执行：

**① 确保钉钉子文件夹（有分组时）**

```
toolName: list_nodes → create_folder（不存在时）
arguments: { "name": "<需求分组>", "folderId": "3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6" }
```

记录子文件夹 `nodeId` 为 `targetFolderId`；无分组时 `targetFolderId` = 开发提测文档根目录。

**② `copy_document`** — 从模板复制到目标文件夹（继承页面宽度/布局）

```
server: 钉钉文档
toolName: copy_document
```

| 参数 | 来源 |
|------|------|
| nodeId | `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9`（详细设计模板，**只读源**） |
| targetFolderId | 需求分组子文件夹 nodeId，或 `3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6`（无分组） |

返回的新 `nodeId` 即为待写入文档。

**③ `rename_document`（不可跳过、不可延后）** — 立即重命名，避免目录出现「详细设计模板1.7.2」

| 参数 | 来源 |
|------|------|
| nodeId | copy 返回的 nodeId |
| newName | `{设计主题}开发设计` |

**④ 块级填充正文（禁止整篇 overwrite）**

> `overwrite` 会破坏模板原生表格样式（变更记录深色表头、正文浅灰表头等）。必须按块填充，详见 [block-fill-guide.md](references/block-fill-guide.md)。

**3a. `list_document_blocks`**

| 参数 | 来源 |
|------|------|
| nodeId | copy 返回的 nodeId |
| pageSize | `120` |

**3b. `update_document_block`** — 更新段落/标题（**不支持 table**）

**3c. 表格替换** — `delete_document_block`(table) → `update_document`(append, index=原index, markdown=§6.1 格式)

**3d. `delete_document_block`** — 删除 callout 占位

> **禁止** `update_document` + `mode=overwrite`。变更记录表（index 8）优先保留原块，用户钉钉 UI 改行。

#### 6.3 更新文档

**已有 nodeId** 时：

1. `list_document_blocks` 定位目标块
2. `update_document_block` 更新表格 cells / 段落 / 标题
3. 变更记录在表格 block 的 cells 追加新行
4. **禁止** `mode=overwrite` 整篇覆盖

| 参数 | 来源 |
|------|------|
| nodeId | 用户提供的已有 nodeId，或 list_nodes 查到的 nodeId |
| blockId + element | 块级更新（见 block-fill-guide.md） |

#### 6.4 大文档与失败

- 多段 append：**首段 index=9**，后续段 append 到 **max(index)**，禁止重复 index（见 block-fill-guide.md）
- 写入后 **必校验**：`get_document_content` 检查 `# **1、 设计概述**` 仅 1 次、文末为 7.3、7.3 后无重复一级标题
- **收尾清理**：`list_nodes`(folderId=开发提测文档) → 删除名称含「详细设计模板」的文档（copy 未 rename 的残留）
- 本地文件必须完整；失败则修正后重传

---

### 7. 后续操作

成功后必须输出：

```markdown
## 开发设计已生成

| 项 | 值 |
|----|-----|
| 目标项目 | {项目名} |
| 代码仓库 | D:\ProjectCode\{项目名} |
| 需求来源 | {需求钉钉链接或文件路径} |
| 需求文档名称 | {从需求文档解析的全名} |
| 需求分组 | {分组名，无则「—」} |
| 设计主题 | {设计主题} |
| 操作 | 新建 / 覆盖更新 |
| 本地设计文档 | D:\1.CursorProject\文档\{项目名}\02-技术文档\{需求分组}\{设计主题}开发设计.md |
| 钉钉文档 | https://alidocs.dingtalk.com/i/nodes/{nodeId} |
| 待确认项 | {列表，无则写「无」} |
```

钉钉 MCP 写失败时：给出本地路径 + [输出目录 URL](https://alidocs.dingtalk.com/i/nodes/3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6)，提示手动导入。

---

## 质量自检（输出前）

**格式与写入**

- [ ] 正文格式符合 [format-guide.md](references/format-guide.md)（红色保密块、加粗章节、表格类型 A/B/C）
- [ ] 本地/钉钉路径符合 [naming-rules.md](references/naming-rules.md)（首横杠分组 + 设计主题）
- [ ] 需求分组子文件夹已创建（本地 mkdir + 钉钉 create_folder）
- [ ] 新建钉钉文档使用 `copy_document`（非 `create_document`），页面宽度与模板 1.7.2 一致
- [ ] 钉钉表格经 delete+append（format-guide §6），**未**整篇 overwrite
- [ ] 章节 1～7 齐全，顺序与 [outline-structure.md](references/outline-structure.md) 一致，7.3 后无重复内容
- [ ] 本地已写入；钉钉 MCP 已调用或已说明失败原因
- [ ] 开发提测文档目录无「详细设计模板*」残留（copy 后已 rename，收尾已 list_nodes 清理）

**开发经理评审门禁**（细则 [review-checklist.md](references/review-checklist.md) §四）

- [ ] 1.2 含需求链接与背景摘要（重构类含效益量化）
- [ ] 3.3.1 ↔ 7.2 逐行对应（含报表/导出/外部接口）
- [ ] 3.2 显式「本期不包含 / 二期」
- [ ] 2.8 ≥3 行事件且含触发频率
- [ ] **2.10 故障分析**：FMEA 表非空、行数达标、外部/敏感/金额有覆盖、总结段含 TOP 风险与上线复核（见 [fmea-guide.md](references/fmea-guide.md)）
- [ ] 3.3.x 设计细节首段含现状→目标→影响范围
- [ ] 代码已实现项标「已实现/沿用」，无过度设计
- [ ] 外部依赖有负责人或「待确认：对接人」
- [ ] 待确认项已在输出表格列出

---

## 注意事项

- **需求正文不可编造**；MCP/文件均失败时必须请用户提供
- 模板 nodeId 固定不变；**需求链接每次不同**
- 输出目录 nodeId 固定；新建用 `copy_document` + 块级填充，**禁止** `create_document` 和整篇 `overwrite`
- 表格样式：见 format-guide §6；钉钉写入见 block-fill-guide（delete+append，禁止 overwrite）
- `markdown` 参数用 Unicode 换行（U+000A），禁止 JSON 字面量 `\n`
- 表格/多维表需求文档 MCP 无法直接读 Markdown，需用户导出
- 修改文件时保持原编码，GB2312/UTF-8 BOM 文件勿擅自改编码
- 复杂需求可额外输出 `_需求摘要.md` 便于评审

## 参考文档

- 项目路径映射：[references/project-mapping.md](references/project-mapping.md)
- MCP 细则：[references/dingtalk-mcp.md](references/dingtalk-mcp.md)
- 钉钉格式规范：[references/format-guide.md](references/format-guide.md)
- 章节映射：[references/section-mapping.md](references/section-mapping.md)
- 用户指南（技能广场）：[references/user-guide.md](references/user-guide.md)
- 用户配置模板：[user-config.example.md](user-config.example.md)
- 命名与目录：[references/naming-rules.md](references/naming-rules.md)
- 大纲结构：[references/outline-structure.md](references/outline-structure.md)
- 块级填充：[references/block-fill-guide.md](references/block-fill-guide.md)
- 开发经理评审关注点：[references/review-checklist.md](references/review-checklist.md)
- **2.10 故障分析（FMEA）**：[references/fmea-guide.md](references/fmea-guide.md)
- 提示语模板：[references/prompt-template.md](references/prompt-template.md)
