# 禅道产品 → 代码仓库映射

**代码根目录**：`D:\ProjectCode`

用户指定绝对路径时以用户为准；未登记产品按关键词在 `D:\ProjectCode` 下搜索目录名。

## 映射表

| 禅道产品/关键词（含模块） | 主仓库 | 辅助仓库 | 检索重点 |
|---------------------------|--------|----------|----------|
| IOA、人事运营、入职、调动 | `ioa-be` | `ipsa-ioa`、`ipsa-rra` | Application、Domain、RRAWeb |
| 招聘、RMS、RRA、校招、社招 | `recruitment-management-be` | `ipsa-rms`、`ipsa-rra` | 招聘流程、EDA |
| 薪酬、社保、公积金、Paypro | `paypro-platform-be` | — | Application/Services、SqlSugar 实体 |
| 入职审批、移动端入职 | `entry-approval-mobile-be` | `ioa-be` | 移动端 API |
| 劳动关系、ER | `employment-relation-be` | `ioa-be` | 劳动关系模块 |
| HR API、员工自助 | `hr-api`、`hr-core-api` | `hr-empself-service-master` | 公共 API |
| 数据同步 | `hr-data-sync-service` | — | 同步任务、Mapping |
| BPM、流程、Ibpm | `IbpmGAA` | `ioa-be` | 流程定义与回调 |
| RRA 老系统、WebForms、EDA | `ipsa-rra` | — | `RRAWeb\Common/Controls`、`.ascx` |
| IOA 前端 | `ioa-fe` | — | Vue 页面与 API 调用 |

## IbpmGAA 知识库约定

- **远端已有** `docs/lessons/` 文件：**保留，不删除、不从历史清除**
- **新增**知识库路径：`D:\1.CursorProject\文档\IbpmGAA\lessons\zentao-bugs\`（**不写入** IbpmGAA 仓库）
- 阶段四 commit 时：**只提交代码**，勿 `git add docs/lessons/`
- **禁止** `git rm --cached docs/lessons`、禁止为清理知识库而 force push / 改写历史

## 解析规则

1. 取 Bug `product.name`、`module.name`、`keywords`、`title`、`steps` 与上表匹配
2. 命中多仓库 → 列出候选，请用户确认主仓库
3. 打开主仓库 `AGENTS.md` / `CLAUDE.md` 确认分层与 ORM 约定
4. 在 `D:\1.CursorProject\文档\{项目名}\` 检索是否已有相关设计或历史缺陷文档

## 未命中时

```bash
# 在 D:\ProjectCode 下按关键词搜目录
dir D:\ProjectCode\*{关键词}*
```

仍无法确定 → 暂停流程，请用户提供项目名。
