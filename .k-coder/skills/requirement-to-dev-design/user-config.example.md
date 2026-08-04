# 用户配置（首次使用请复制为 user-config.md）

将本文件复制到 skill 根目录并重命名为 **`user-config.md`**，按你的环境修改。  
Agent 执行 skill 时会**优先读取 `user-config.md`**，未配置项则使用 [references/dingtalk-config.md](references/dingtalk-config.md) 中的默认值。

> **不要**把含密钥的 `user-config.md` 上传到技能广场或 Git 仓库；仅上传 `user-config.example.md`。

---

## 钉钉文档 MCP

| 配置项 | 说明 | 示例 |
|--------|------|------|
| `mcpServerName` | `~/.cursor/mcp.json` 中的 MCP 服务器名 | `钉钉文档` |

---

## 钉钉 nodeId（从 URL `/nodes/` 后、`?` 前复制）

| 配置项 | 说明 | 默认值（软通示例，请改为你的） |
|--------|------|--------------------------------|
| `designTemplateNodeId` | 详细设计模板 1.7.2，**只读**，仅 copy 用 | `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9` |
| `outputFolderId` | **开发设计输出目录**（钉钉写入目标文件夹） | `3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6` |

**如何改钉钉输出路径：**

1. 在钉钉文档中打开你的团队「开发提测文档」或任意目标文件夹  
2. 复制浏览器地址栏 URL，提取 nodeId  
3. 将 `outputFolderId` 改为该 nodeId  
4. 确保 MCP 账号对该文件夹有**写权限**

可选：在对话中临时指定——「钉钉输出到 https://alidocs.dingtalk.com/i/nodes/你的folderId」

---

## 本地路径

| 配置项 | 说明 | 示例 |
|--------|------|------|
| `codeRoot` | 代码仓库根目录 | `D:\ProjectCode` |
| `localDocRoot` | 本地设计文档根目录（其下为 `{项目名}/02-技术文档/`） | `D:\1.CursorProject\文档` |

---

## 可选行为

| 配置项 | 说明 | 默认 |
|--------|------|------|
| `defaultAuthor` | 变更记录「修改人」 | 空（用当前用户） |
| `skipDingtalkWrite` | `true` 时只生成本地 Markdown，不调钉钉 MCP 写入 | `false` |
| `outputFolderName` | 输出目录显示名（仅文档用，不影响 nodeId） | `开发提测文档` |

---

## 配置示例（user-config.md）

```markdown
# user-config.md

mcpServerName: 钉钉文档

designTemplateNodeId: qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9
outputFolderId: 你的输出文件夹nodeId

codeRoot: E:\Work\ProjectCode
localDocRoot: E:\Work\Docs

defaultAuthor: 张三
skipDingtalkWrite: false
```

也可在 Cursor 对话里一次性说明，无需建文件：

```
按 requirement-to-dev-design skill 生成开发设计。
代码根目录 E:\Work\ProjectCode
本地文档 E:\Work\Docs
钉钉输出目录 nodeId: xxxxxxxxx
```
