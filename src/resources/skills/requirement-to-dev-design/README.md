# requirement-to-dev-design

**需求文档 → 开发详细设计（模板 1.7.2）**，本地 Markdown + 钉钉文档双写，内置开发经理评审门禁（含 2.10 FMEA）。

---

## 能做什么

- 从 **钉钉需求链接** 或 **本地 Word/Markdown** 读取需求
- 检索 **代码仓库**，对齐现有实现，生成 1～7 章开发设计
- **3.3.1 功能清单** 与 **7.2 接口** 一一对应；**2.10 故障分析** 按 FMEA 规范填写
- 钉钉：`copy_document` 公司模板 → 块级填充（保留表格样式）
- 本地：按需求名首横杠自动分文件夹

---

## 快速开始

1. 安装 skill 到 `%USERPROFILE%\.cursor\skills\requirement-to-dev-design\`
2. 配置「钉钉文档」MCP，重启 Cursor
3. 复制 **`user-config.example.md` → `user-config.md`**，修改 **钉钉输出 folderId** 与 **本地 codeRoot / localDocRoot**
4. 在 Cursor 对话：

```
按 requirement-to-dev-design skill，根据钉钉需求
https://alidocs.dingtalk.com/i/nodes/{需求nodeId}
为 {项目名} 生成开发设计。
```

详细说明见 **[references/user-guide.md](references/user-guide.md)**。

---

## 自定义钉钉输出路径？

**可以。** 编辑 `user-config.md` 中的 `outputFolderId`（目标文件夹的 nodeId），或在对话中指定「钉钉输出 folderId: xxx」。  
模板 nodeId 用 `designTemplateNodeId` 修改。详见 [user-config.example.md](user-config.example.md)。

默认 nodeId 为作者团队示例，**他人下载后必须改为自己的文件夹 ID**。

---

## 目录结构

| 路径 | 说明 |
|------|------|
| `SKILL.md` | Agent 主流程 |
| `user-config.example.md` | 用户配置模板（复制为 `user-config.md`） |
| `references/user-guide.md` | **使用指南（推荐阅读）** |
| `references/dingtalk-config.md` | 默认钉钉 nodeId |
| `references/fmea-guide.md` | 2.10 故障分析 |
| `templates/detailed-design-template.md` | 离线模板 |

---

## 打包发布

```bash
python path/to/skill-creator/scripts/package_skill.py path/to/requirement-to-dev-design ./dist
```

生成 `requirement-to-dev-design.skill` 上传技能广场。
