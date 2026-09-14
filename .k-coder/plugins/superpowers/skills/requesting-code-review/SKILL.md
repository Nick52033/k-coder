---
name: requesting-code-review
description: Use when completing tasks, implementing major features, or before merging to verify work meets requirements
---

## k-Coder 本地适配

本节替代下文其他客户端的工具名、配置目录和交付流程；方法论与质量要求继续适用。系统、开发者及用户/项目指令优先于 Skill；Skill 不授予工具权限。用户已经明确要求实现时直接执行，不因本技能重复请求实施确认；默认不提交、不推送。生成说明、计划和报告放在项目 docs/ 下。

- 加载本插件其他技能：plugin_skill_read({"pluginId":"superpowers@local","skillName":"<名称>"})。不要调用不存在的 Skill、activate_skill 或 skills.list。
- 读取参考资源：plugin_resource_read({"pluginId":"superpowers@local","path":"skills/<技能目录>/<相对资源路径>"})；路径来自实际目录，不猜测。
- 文件：read_file、list_directory、search_repository、apply_patch、write_file；参数路径必须相对当前工作区，不能使用绝对路径或 ..。Shell 使用 run_command，遵守其公开 Schema、审批、超时和取消。
- 可见计划：update_plan({"steps":[{"step":"检查与实现","status":"in_progress"},{"step":"验证","status":"pending"}]})。最多一个 in_progress。
- 委派（仅当当前工具目录存在且任务允许时）：create_agent({"task":"明确子任务","label":"review","forkTurns":"none"})；后续使用 wait_agent、send_agent_message、resume_agent、list_agents、close_agent，参数以当前 Schema 为准。不要调用 spawn_agent/Task，不指定不存在的 model 字段，不创建第二套智能体循环；工具不可用时在主任务顺序完成。
- 本宿主没有 Codex 模式切换、App 手交和 Codex 专用配置工具；不要修改 ~/.codex。隔离与 Git 操作遵守当前项目边界；已有授权不重复询问，提交/推送须由用户明确要求。


# Requesting Code Review

Dispatch a code reviewer subagent to catch issues before they cascade. The reviewer gets precisely crafted context for evaluation — never your session's history. This keeps the reviewer focused on the work product, not your thought process, and preserves your own context for continued work.

**Core principle:** Review early, review often.

## When to Request Review

**Mandatory:**
- After each task in subagent-driven development
- After completing major feature
- Before merge to main

**Optional but valuable:**
- When stuck (fresh perspective)
- Before refactoring (baseline check)
- After fixing complex bug

## How to Request

**1. Get git SHAs:**
```bash
BASE_SHA=$(git rev-parse HEAD~1)  # or origin/main
HEAD_SHA=$(git rev-parse HEAD)
```

**2. Dispatch code reviewer subagent:**

Use Task tool with `general-purpose` type, fill template at `code-reviewer.md`

**Placeholders:**
- `{DESCRIPTION}` - Brief summary of what you built
- `{PLAN_OR_REQUIREMENTS}` - What it should do
- `{BASE_SHA}` - Starting commit
- `{HEAD_SHA}` - Ending commit

**3. Act on feedback:**
- Fix Critical issues immediately
- Fix Important issues before proceeding
- Note Minor issues for later
- Push back if reviewer is wrong (with reasoning)

## Example

```
[Just completed Task 2: Add verification function]

You: Let me request code review before proceeding.

BASE_SHA=$(git log --oneline | grep "Task 1" | head -1 | awk '{print $1}')
HEAD_SHA=$(git rev-parse HEAD)

[Dispatch code reviewer subagent]
  DESCRIPTION: Added verifyIndex() and repairIndex() with 4 issue types
  PLAN_OR_REQUIREMENTS: Task 2 from docs/superpowers/plans/deployment-plan.md
  BASE_SHA: a7981ec
  HEAD_SHA: 3df7661

[Subagent returns]:
  Strengths: Clean architecture, real tests
  Issues:
    Important: Missing progress indicators
    Minor: Magic number (100) for reporting interval
  Assessment: Ready to proceed

You: [Fix progress indicators]
[Continue to Task 3]
```

## Integration with Workflows

**Subagent-Driven Development:**
- Review after EACH task
- Catch issues before they compound
- Fix before moving to next task

**Executing Plans:**
- Review after each task or at natural checkpoints
- Get feedback, apply, continue

**Ad-Hoc Development:**
- Review before merge
- Review when stuck

## Red Flags

**Never:**
- Skip review because "it's simple"
- Ignore Critical issues
- Proceed with unfixed Important issues
- Argue with valid technical feedback

**If reviewer wrong:**
- Push back with technical reasoning
- Show code/tests that prove it works
- Request clarification

See template at: requesting-code-review/code-reviewer.md
