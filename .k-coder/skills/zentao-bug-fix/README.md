# zentao-bug-fix

从禅道查询 Bug 列表供用户选择，拉取详情后在对应代码仓库定位问题，先生成修复处理文档写入项目知识库（docs/lessons），经人工确认后再改代码；用户回复「验证通过」时自动更新文档、commit/push、创建 MR，并调用 zentao-bug-resolve 标记禅道已解决。当用户说「修bug」「修复禅道」「处理bug」「自动修bug」「zentao bug fix」「修复 #123」「验证通过」时触发。纯查询 bug 列表请用 zentao-bugs，勿触发本技能。

**Version**: 1.0.0

## Trigger scenarios

从禅道查询 Bug 列表供用户选择，拉取详情后在对应代码仓库定位问题，先生成修复处理文档写入项目知识库（docs/lessons），经人工确认后再改代码；用户回复「验证通过」时自动更新文档、commit/push、创建 MR，并调用 zentao-bug-resolve 标记禅道已解决。当用户说「修bug」「修复禅道」「处理bug」「自动修bug」「zentao bug fix」「修复 #123」「验证通过」时触发。纯查询 bug 列表请用 zentao-bugs，勿触发本技能。

## Directory structure

- `SKILL.md`
- `references/code-fix-phase.md`
- `references/fix-document-template.md`
- `references/repo-mapping.md`
- `references/verify-passed-phase.md`
- `references/workflow.md`
- `references/zentao-api.md`

---
*Generated for Skill Hub upload at 2026-07-03*

