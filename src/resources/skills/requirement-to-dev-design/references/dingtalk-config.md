# 钉钉文档配置

MCP 服务器名（`~/.cursor/mcp.json`）：**`钉钉文档`**（可在 [user-config.md](../user-config.example.md) 中改为 `mcpServerName`）

## 默认值（软通内网示例）

> **技能广场下载者**：下列 nodeId 仅为作者团队默认值，**必须**复制 `user-config.example.md` → `user-config.md` 并改为你的文件夹 ID。

| 资源 | 配置键 | 默认 nodeId | 完整 URL |
|------|--------|-------------|----------|
| 详细设计模板 1.7.2（**只读源**） | `designTemplateNodeId` | `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9` | https://alidocs.dingtalk.com/i/nodes/qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9 |
| **开发设计输出目录** | **`outputFolderId`** | `3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6` | https://alidocs.dingtalk.com/i/nodes/3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6 |

## 自定义钉钉输出路径

1. 打开钉钉文档中目标文件夹（如「开发提测文档」或团队自定义目录）
2. 从 URL 提取 nodeId：`https://alidocs.dingtalk.com/i/nodes/{nodeId}`
3. 写入 skill 根目录 **`user-config.md`**：

```markdown
outputFolderId: 你的folderId
designTemplateNodeId: 你的模板nodeId
```

或在 Cursor 对话中说明：「钉钉输出 folderId: xxx」

详见 [user-config.example.md](../user-config.example.md) 与 [user-guide.md](user-guide.md)。

## 目录隔离与分组规则

- **模板 1.7.2**：只存在于模板 nodeId 原位；`copy_document` 后**必须立即** `rename_document`
- **开发提测文档**：按需求文档名首横杠分组，子文件夹 `{需求分组}` 内存 `{设计主题}开发设计`；详见 [naming-rules.md](naming-rules.md)
- **禁止** `create_document` + 本地模板 Markdown 上传到开发提测文档

## nodeId 提取

从 URL `https://alidocs.dingtalk.com/i/nodes/{nodeId}?...` 取 `{nodeId}`；**也可直接把完整 URL 传给 MCP**（多数工具支持 URL 或 ID）。

## 页面宽度与表格样式

- 页面宽度：`copy_document` 继承模板窄页布局；`create_document` 为全宽
- 表格样式：模板原生 `table` 块含深色/浅灰表头等样式；**整篇 overwrite 会全部丢失**
- **正确做法**：`copy_document` → `list_document_blocks` → `update_document_block` 只改 cells
- 块级填充详见 [block-fill-guide.md](block-fill-guide.md)

## 前置条件

- `~/.cursor/mcp.json` 中已配置 `钉钉文档` MCP 且 Cursor 已重启
- 钉钉 MCP 已授权当前账号，对需求文档有读权限、对输出目录有写权限
- 调用前必须先读取 MCP 工具 schema（`mcps` 目录或 `tools/list`）
