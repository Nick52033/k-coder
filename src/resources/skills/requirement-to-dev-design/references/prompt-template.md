# 开发详细设计 — 提示语模板

> 完整使用说明见：`D:\1.CursorProject\文档\cursor-skills\requirement-to-dev-design使用说明.md`

---

## 最简（仅钉钉需求链接 + 项目名）

```
按 requirement-to-dev-design skill，根据钉钉需求 https://alidocs.dingtalk.com/i/nodes/xxx 为 paypro-platform-be 生成开发设计。
```

---

## 完整版

```
请按 requirement-to-dev-design skill 生成开发详细设计。

## 需求来源
- 需求链接：https://alidocs.dingtalk.com/i/nodes/xxx
- 需求主题：<主题名>
- （可选）已有设计 nodeId（覆盖更新）：<留空则新建>

## 目标项目
- 项目名：paypro-platform-be

## 执行要求
1. get_document_info 取需求文档名 → 首横杠拆分组文件夹与设计主题
2. MCP 读取需求 → 检索 D:\ProjectCode\{项目}\ 代码（已实现标「已实现/沿用」）
3. 按 review-checklist 完成需求→3.3.1 映射、2.8、**2.10 FMEA（重点，见 fmea-guide）**、3.2 本期二期剪刀
4. 本地：02-技术文档\{分组}\{设计主题}开发设计.md
5. 钉钉：开发提测文档下 create_folder(分组) → copy → rename → 块级填充
6. 回复本地路径 + 钉钉 URL + 待确认项
```

---

## 无钉钉链接（本地需求）

```
按 requirement-to-dev-design skill，项目 ioa-be，需求文件 D:\1.CursorProject\文档\xxx\需求.md，生成开发设计并 MCP 写入开发提测文档目录。
```

---

## 覆盖更新已有设计

```
按 requirement-to-dev-design skill 更新开发设计。
需求：https://alidocs.dingtalk.com/i/nodes/需求nodeId
已有设计：https://alidocs.dingtalk.com/i/nodes/设计nodeId
项目：paypro-platform-be
修改人：张三
```
