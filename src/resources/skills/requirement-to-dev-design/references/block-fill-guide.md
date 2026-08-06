# 钉钉文档块级填充指南（保留表格样式）

## 问题根因

1. `update_document` + `mode=overwrite` 会**整篇清空并重解析 Markdown**，模板原生 `table` 块样式全部丢失。
2. `update_document_block` **不支持** `blockType=table`（报错 `unsupported type table`）。

**结论：复制模板后禁止整篇 overwrite；表格用「删块 + 同 index append 模板格式 Markdown」替换。**

---

## 写入流程

```
copy_document(模板 → 输出目录)
  → rename_document
  → list_document_blocks(nodeId, pageSize=120)
  → update_document_block(段落/标题块)
  → delete_document_block(callout 占位块)
  → delete_document_block(需改内容的 table 块)
  → update_document(append, index=N, markdown=模板格式表格)
  → update_document(append)  # 新增段落/章节
```

| 步骤 | 工具 | 说明 |
|------|------|------|
| 1 | `copy_document` | 继承页面宽度 + 块结构 |
| 2 | `rename_document` | 设置文档名 |
| 3 | `list_document_blocks` | 记录 `index`、`blockId`、`blockType` |
| 4 | `update_document_block` | **仅** paragraph / heading |
| 5 | `delete_document_block` | 删除 callout；删除待替换的 table |
| 6 | `update_document` append | 在原 `index` 处插入模板格式 Markdown 表格/正文 |

---

## 块类型处理

| blockType | 操作 |
|-----------|------|
| `paragraph` | `update_document_block` → `element.paragraph.text` |
| `heading` | 一般保留；需改时用 `update_document_block` |
| `callout` | `delete_document_block` → append 真实正文 |
| **`table`** | **`delete_document_block` → `update_document`(append, index=原index, markdown=按 format-guide §6 类型写表格)** |
| 新增章节 | `update_document` append |

### 变更记录表（index 8）特殊说明

- 模板原生块带**深色表头**；MCP 无法原地改 cells。
- **方案 A（推荐）**：保留 block 8 不动，提示用户在钉钉 UI 手工改/增行。
- **方案 B**：delete block 8 + append 类型 A Markdown（深色表头**可能**无法完全还原，但优于 overwrite 整篇）。

### 表格 Markdown 格式（append 时必须严格）

| 表格 | append 用的分隔行 |
|------|------------------|
| 变更记录 | `\|----------\|----------\|...\|` + 表头加粗 |
| 1.1 运行环境 | `\|------\|------\|------------\|` + 表头**不加粗** |
| 2.7 等正文表 | `\|----------\|-----------\|` + 表头加粗 |
| 3.3.1 功能清单 | 表头用 `rgb(225,230,237)` 背景 span |

1.1 环境表示例（append 到 index 12）：

```markdown
| 序号 | 项目 | 详细信息 |
|------|------|------------|
| 1 | 后端软件环境 | .NET Framework 4.7.2、... |
| 2 | 前端软件环境 | jQuery、Layui、... |
```

---

## 模板关键块 index

| index | blockType | 章节 | 处理方式 |
|-------|-----------|------|----------|
| 0–4 | paragraph/table | 保密块 | 保留 |
| 7–8 | heading/table | 变更记录 | 保留 block 8 或 delete+append 类型 A |
| 10–12 | heading/table | 1.1 运行环境 | delete block 12 + append 类型 B 表 |
| 14,17,… | callout | 占位 | delete + append 正文 |
| 21+ | paragraph | 2.1 等 | update_document_block |
| 27,30,38… | table | 各节表格 | delete + append 对应类型 |

---

## 多段 append（>10000 字符，关键）

- 单段 markdown ≤ **10000** 字符
- **禁止**多段 append 使用**相同 index** → 后插入段会排在前面，导致 **7.3 后出现「1、 设计概述」**
- **正确顺序**：
  1. 删除 index≥10 的模板块
  2. `append(index=9, markdown=chunk1)` — 紧接变更记录后
  3. `list_document_blocks` → `lastIdx = max(index)`
  4. `append(index=lastIdx, markdown=chunk2)` — 每段后重新取 lastIdx
- **推荐**：按一级章 `# **N、**` 边界拆分，保证每段语义完整

正文范围：`# **1、 设计概述**` ～ `## **7.3 定时服务**` 结束，见 [outline-structure.md](outline-structure.md)。

---

## 已有 nodeId 更新

1. `list_document_blocks` 定位块
2. 段落/标题 → `update_document_block`
3. 表格 → delete + append（同 index）
4. **禁止** overwrite 整篇

---

## 自检

- [ ] 未使用 `update_document` + `overwrite`
- [ ] 表格 append 使用 format-guide §6 对应类型分隔行
- [ ] 1.1 表头未加粗
- [ ] 章节顺序符合 [outline-structure.md](outline-structure.md)（1→7，7.3 后无重复内容）
- [ ] 多段 append 每段使用递增的 lastIndex，非固定 index=10
- [ ] 开发提测文档目录无「详细设计模板*」残留
