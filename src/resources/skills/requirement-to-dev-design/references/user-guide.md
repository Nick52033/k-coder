# requirement-to-dev-design 使用指南

## 这个 Skill 是干什么的？

**一句话**：把「需求文档」自动变成符合公司模板（详细设计 1.7.2）的**开发详细设计**，并**本地 + 钉钉双写**。

| 输入 | 输出 |
|------|------|
| 钉钉需求链接 / 本地 Word·Markdown | 本地 `{项目}/02-技术文档/{分组}/{主题}开发设计.md` |
| 目标项目名 + 代码仓库 | 钉钉 `{输出目录}/{分组}/{主题}开发设计` |
| （可选）已有设计 nodeId | 覆盖更新 + 变更记录 |

**Agent 会自动完成：**

- 读需求 → 检索 `codeRoot` 下代码（已实现标「沿用」，避免过度设计）
- 按模板写 1～7 章（含 **2.10 故障分析 FMEA**、3.3.1↔7.2 接口对应）
- 钉钉 `copy_document` 模板 → 块级填充（保留表格样式）

**不适用**：只要写代码、只要接口测试（请用其他 skill）。

---

## 安装

1. 下载 `.skill` 包，解压或导入到 Cursor Skills 目录，例如：  
   `%USERPROFILE%\.cursor\skills\requirement-to-dev-design\`
2. 配置 **钉钉文档 MCP**（`~/.cursor/mcp.json`），**完全重启 Cursor**
3. **（推荐）** 复制 `user-config.example.md` → `user-config.md`，修改钉钉输出目录与本地路径

---

## 能否自定义钉钉输出路径？

**可以。** 三种方式（任选）：

| 方式 | 做法 |
|------|------|
| **配置文件（推荐）** | 编辑 skill 根目录 `user-config.md` 的 `outputFolderId` |
| **对话指定** | 「钉钉输出到 nodeId: xxx」或粘贴文件夹 URL |
| **单次覆盖** | 「已有设计 nodeId: xxx」更新指定文档，不经过输出目录 |

`designTemplateNodeId`（详细设计模板）也可在 `user-config.md` 中改为贵司模板 nodeId。

默认值见 [dingtalk-config.md](dingtalk-config.md)（软通内网示例，**下载后务必改为你自己的 nodeId**）。

---

## 怎么用

### 最简（复制改链接和项目名）

```
按 requirement-to-dev-design skill，根据钉钉需求
https://alidocs.dingtalk.com/i/nodes/你的需求nodeId
为 paypro-platform-be 生成开发设计。
```

需求文档名 **首横杠前 = 钉钉/本地子文件夹**，**后半 = 设计主题**。  
例：`入职流程改进－业务流程信息变更提效` → 文件夹 `入职流程改进`，文档 `业务流程信息变更提效开发设计`。

### 自定义路径

```
按 requirement-to-dev-design skill 生成开发设计。
需求：https://alidocs.dingtalk.com/i/nodes/xxx
项目：ioa-be
代码根目录：E:\Work\ProjectCode
本地文档根目录：E:\Work\Docs
钉钉输出 folderId：你的folderId
```

### 只生成本地（不写钉钉）

`user-config.md` 设 `skipDingtalkWrite: true`，或对话中说「只生成本地，不写钉钉」。

### 更新已有设计

```
已有设计 nodeId：https://alidocs.dingtalk.com/i/nodes/yyy
修改人：张三
（可粘贴评审评论，Agent 按 review-checklist 块级更新）
```

更多提示语见 [prompt-template.md](prompt-template.md)。

---

## 前置条件

| 项 | 要求 |
|----|------|
| Cursor | 已安装；Skill 已加载 |
| 钉钉 MCP | 能读需求文档、能写 `outputFolderId` 目录 |
| 代码 | 项目在 `codeRoot` 下可检索 |
| 权限 | 输出文件夹写权限、需求文档读权限 |

---

## 参考文档（Skill 内）

| 文件 | 用途 |
|------|------|
| [user-config.example.md](../user-config.example.md) | 路径与钉钉 nodeId 配置模板 |
| [dingtalk-config.md](dingtalk-config.md) | 默认 nodeId 与 MCP 说明 |
| [review-checklist.md](review-checklist.md) | 开发经理评审门禁 |
| [fmea-guide.md](fmea-guide.md) | 2.10 故障分析写作 |
| [prompt-template.md](prompt-template.md) | 对话提示语 |
