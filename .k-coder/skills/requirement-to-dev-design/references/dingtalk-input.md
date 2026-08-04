# 钉钉文档输入与输出（简要）

主流程见 **SKILL.md 步骤 0～6**。**用户使用说明**见 `D:\1.CursorProject\文档\cursor-skills\requirement-to-dev-design使用说明.md`

## 链接 → nodeId

`https://alidocs.dingtalk.com/i/nodes/{nodeId}?...` → 提取 `{nodeId}` 或传完整 URL。

## 固定资源

| 资源 | nodeId |
|------|--------|
| 模板 | `qnYMoO1rWxDlAnXpc3AwgNEnW47Z3je9` |
| 输出目录 | `3xRN9bGQyw4Jbo6OQa6N8zXPADKnorv6` |

## 本地提取脚本

| 格式 | 脚本 |
|------|------|
| .docx | `scripts/extract_docx.py` |
| .xlsx | `scripts/extract_xlsx.py` |
