# 钉钉文档 MCP 参考

SKILL.md 已包含主流程；本文档补充工具清单与异常细节。

## 工具清单

| toolName | 用途 |
|----------|------|
| `get_document_content` | 读需求/模板 Markdown |
| `get_document_info` | 判断文档类型 |
| **`copy_document`** | **新建开发设计（从模板复制，保留页面宽度）** |
| **`list_document_blocks`** | 列出文档块（index、blockId、blockType） |
| **`update_document_block`** | **块级更新（保留表格样式，首选）** |
| **`delete_document_block`** | 删除 callout 等占位块 |
| `update_document` | 仅 `append` 新增章节；**禁止 overwrite 整篇** |
| `rename_document` | 设置文档标题 |
| `list_nodes` | 输出目录查重 |
| `search_documents` | 关键词搜索需求 |
| `create_document` | ⚠️ **勿用于开发设计**（空白文档为全宽布局） |

## 固定 nodeId

见 [dingtalk-config.md](dingtalk-config.md)。

## 新建文档流程（保留页面宽度 + 表格样式）

```
copy_document(模板 nodeId → 需求分组子文件夹 / 开发提测文档根)
  → rename_document(立即，{设计主题}开发设计)
  → list_document_blocks(pageSize=120)
  → update_document_block / delete + append
  → list_nodes → delete_document(「详细设计模板*」残留)
```

有 `{需求分组}` 时先 `list_nodes` + `create_folder`，再 `copy_document` 到子文件夹。命名细则见 [naming-rules.md](naming-rules.md)。

| 步骤 | 工具 | 关键参数 |
|------|------|----------|
| 1 | `copy_document` | nodeId=`qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9`，targetFolderId=`3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6` |
| 2 | `rename_document` | newName=`<主题>开发设计` |
| 3 | `list_document_blocks` | 记录 blockId；index 8=变更记录表，12=1.1 环境表 |
| 4 | `update_document_block` | 段落/标题（非 table） |
| 5 | `delete_document_block` + `append` | 表格：删块后在同 index 插入 §6.1 Markdown |
| 6 | `delete_document_block` | callout 占位 |

**禁止** `update_document` + `mode=overwrite` — 会破坏表格样式。表格用 delete+append 替换。

详细块索引见 [block-fill-guide.md](block-fill-guide.md)。

## 已有 nodeId 更新

直接 `list_document_blocks` → `update_document_block` 更新目标块；变更记录在表格 cells 追加行。**禁止 overwrite。**

## 大文档 append

- 块级填充无 10000 字符限制
- `append` 单段 markdown ≤ **10000** 字符
- HTTP 网关：PowerShell 用 `ConvertTo-Json -Depth 20`

```
get_document_content → 粘贴/本地文件 → search_documents → 本地文档库 → 询问用户
生成 → 本地写入 → copy → list_blocks → update_block → 失败则手动上传
```

## MCP 不可用排查

1. `~/.cursor/mcp.json` 是否配置「钉钉文档」
2. 完全重启 Cursor
3. MCP 面板是否已连接、已授权

## 评论/批注（jsonml 实测）

`get_document_content(format=jsonml)` 与 `list_document_blocks(format=jsonml)` **不含评论正文**，仅在被评论的 span 上带锚点：

```json
{"comment": {"contentId": "mkj9vl0g0y8afbihmpo"}}
```

- 可据此知道「哪段文字有评论」，但 **无法自动读取评论内容**
- 响应评审时：请用户粘贴评论 / 导出评论，或对照变更记录与 [review-checklist.md](review-checklist.md) §五 块级更新
