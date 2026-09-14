---
name: executing-plans
description: Use when you have a written implementation plan to execute in a separate session with review checkpoints
---

## k-Coder 本地适配

本节替代下文其他客户端的工具名、配置目录和交付流程；方法论与质量要求继续适用。系统、开发者及用户/项目指令优先于 Skill；Skill 不授予工具权限。用户已经明确要求实现时直接执行，不因本技能重复请求实施确认；默认不提交、不推送。生成说明、计划和报告放在项目 docs/ 下。

- 加载本插件其他技能：plugin_skill_read({"pluginId":"superpowers@local","skillName":"<名称>"})。不要调用不存在的 Skill、activate_skill 或 skills.list。
- 读取参考资源：plugin_resource_read({"pluginId":"superpowers@local","path":"skills/<技能目录>/<相对资源路径>"})；路径来自实际目录，不猜测。
- 文件：read_file、list_directory、search_repository、apply_patch、write_file；参数路径必须相对当前工作区，不能使用绝对路径或 ..。Shell 使用 run_command，遵守其公开 Schema、审批、超时和取消。
- 可见计划：update_plan({"steps":[{"step":"检查与实现","status":"in_progress"},{"step":"验证","status":"pending"}]})。最多一个 in_progress。
- 委派（仅当当前工具目录存在且任务允许时）：create_agent({"task":"明确子任务","label":"review","forkTurns":"none"})；后续使用 wait_agent、send_agent_message、resume_agent、list_agents、close_agent，参数以当前 Schema 为准。不要调用 spawn_agent/Task，不指定不存在的 model 字段，不创建第二套智能体循环；工具不可用时在主任务顺序完成。
- 本宿主没有 Codex 模式切换、App 手交和 Codex 专用配置工具；不要修改 ~/.codex。隔离与 Git 操作遵守当前项目边界；已有授权不重复询问，提交/推送须由用户明确要求。


# Executing Plans

## Overview

Load plan, review critically, execute all tasks, report when complete.

**Announce at start:** "I'm using the executing-plans skill to implement this plan."

**Note:** Tell your human partner that Superpowers works much better with access to subagents. The quality of its work will be significantly higher if run on a platform with subagent support (such as Claude Code or Codex). If subagents are available, use superpowers:subagent-driven-development instead of this skill.

## The Process

### Step 1: Load and Review Plan
1. Read plan file
2. Review critically - identify any questions or concerns about the plan
3. If concerns: Raise them with your human partner before starting
4. If no concerns: Create TodoWrite and proceed

### Step 2: Execute Tasks

For each task:
1. Mark as in_progress
2. Follow each step exactly (plan has bite-sized steps)
3. Run verifications as specified
4. Mark as completed

### Step 3: Complete Development

After all tasks complete and verified:
- Announce: "I'm using the finishing-a-development-branch skill to complete this work."
- **REQUIRED SUB-SKILL:** Use superpowers:finishing-a-development-branch
- Follow that skill to verify tests, present options, execute choice

## When to Stop and Ask for Help

**STOP executing immediately when:**
- Hit a blocker (missing dependency, test fails, instruction unclear)
- Plan has critical gaps preventing starting
- You don't understand an instruction
- Verification fails repeatedly

**Ask for clarification rather than guessing.**

## When to Revisit Earlier Steps

**Return to Review (Step 1) when:**
- Partner updates the plan based on your feedback
- Fundamental approach needs rethinking

**Don't force through blockers** - stop and ask.

## Remember
- Review plan critically first
- Follow plan steps exactly
- Don't skip verifications
- Reference skills when plan says to
- Stop when blocked, don't guess
- Never start implementation on main/master branch without explicit user consent

## Integration

**Required workflow skills:**
- **superpowers:using-git-worktrees** - Ensures isolated workspace (creates one or verifies existing)
- **superpowers:writing-plans** - Creates the plan this skill executes
- **superpowers:finishing-a-development-branch** - Complete development after all tasks
