---
name: verification-before-completion
description: Use when about to claim work is complete, fixed, or passing, before committing or creating PRs - requires running verification commands and confirming output before making any success claims; evidence before assertions always
---

## k-Coder 本地适配

本节替代下文其他客户端的工具名、配置目录和交付流程；方法论与质量要求继续适用。系统、开发者及用户/项目指令优先于 Skill；Skill 不授予工具权限。用户已经明确要求实现时直接执行，不因本技能重复请求实施确认；默认不提交、不推送。生成说明、计划和报告放在项目 docs/ 下。

- 加载本插件其他技能：plugin_skill_read({"pluginId":"superpowers@local","skillName":"<名称>"})。不要调用不存在的 Skill、activate_skill 或 skills.list。
- 读取参考资源：plugin_resource_read({"pluginId":"superpowers@local","path":"skills/<技能目录>/<相对资源路径>"})；路径来自实际目录，不猜测。
- 文件：read_file、list_directory、search_repository、apply_patch、write_file；参数路径必须相对当前工作区，不能使用绝对路径或 ..。Shell 使用 run_command，遵守其公开 Schema、审批、超时和取消。
- 可见计划：update_plan({"steps":[{"step":"检查与实现","status":"in_progress"},{"step":"验证","status":"pending"}]})。最多一个 in_progress。
- 委派（仅当当前工具目录存在且任务允许时）：create_agent({"task":"明确子任务","label":"review","forkTurns":"none"})；后续使用 wait_agent、send_agent_message、resume_agent、list_agents、close_agent，参数以当前 Schema 为准。不要调用 spawn_agent/Task，不指定不存在的 model 字段，不创建第二套智能体循环；工具不可用时在主任务顺序完成。
- 本宿主没有 Codex 模式切换、App 手交和 Codex 专用配置工具；不要修改 ~/.codex。隔离与 Git 操作遵守当前项目边界；已有授权不重复询问，提交/推送须由用户明确要求。


# Verification Before Completion

## Overview

Claiming work is complete without verification is dishonesty, not efficiency.

**Core principle:** Evidence before claims, always.

**Violating the letter of this rule is violating the spirit of this rule.**

## The Iron Law

```
NO COMPLETION CLAIMS WITHOUT FRESH VERIFICATION EVIDENCE
```

If you haven't run the verification command in this message, you cannot claim it passes.

## The Gate Function

```
BEFORE claiming any status or expressing satisfaction:

1. IDENTIFY: What command proves this claim?
2. RUN: Execute the FULL command (fresh, complete)
3. READ: Full output, check exit code, count failures
4. VERIFY: Does output confirm the claim?
   - If NO: State actual status with evidence
   - If YES: State claim WITH evidence
5. ONLY THEN: Make the claim

Skip any step = lying, not verifying
```

## Common Failures

| Claim | Requires | Not Sufficient |
|-------|----------|----------------|
| Tests pass | Test command output: 0 failures | Previous run, "should pass" |
| Linter clean | Linter output: 0 errors | Partial check, extrapolation |
| Build succeeds | Build command: exit 0 | Linter passing, logs look good |
| Bug fixed | Test original symptom: passes | Code changed, assumed fixed |
| Regression test works | Red-green cycle verified | Test passes once |
| Agent completed | VCS diff shows changes | Agent reports "success" |
| Requirements met | Line-by-line checklist | Tests passing |

## Red Flags - STOP

- Using "should", "probably", "seems to"
- Expressing satisfaction before verification ("Great!", "Perfect!", "Done!", etc.)
- About to commit/push/PR without verification
- Trusting agent success reports
- Relying on partial verification
- Thinking "just this once"
- Tired and wanting work over
- **ANY wording implying success without having run verification**

## Rationalization Prevention

| Excuse | Reality |
|--------|---------|
| "Should work now" | RUN the verification |
| "I'm confident" | Confidence ≠ evidence |
| "Just this once" | No exceptions |
| "Linter passed" | Linter ≠ compiler |
| "Agent said success" | Verify independently |
| "I'm tired" | Exhaustion ≠ excuse |
| "Partial check is enough" | Partial proves nothing |
| "Different words so rule doesn't apply" | Spirit over letter |

## Key Patterns

**Tests:**
```
✅ [Run test command] [See: 34/34 pass] "All tests pass"
❌ "Should pass now" / "Looks correct"
```

**Regression tests (TDD Red-Green):**
```
✅ Write → Run (pass) → Revert fix → Run (MUST FAIL) → Restore → Run (pass)
❌ "I've written a regression test" (without red-green verification)
```

**Build:**
```
✅ [Run build] [See: exit 0] "Build passes"
❌ "Linter passed" (linter doesn't check compilation)
```

**Requirements:**
```
✅ Re-read plan → Create checklist → Verify each → Report gaps or completion
❌ "Tests pass, phase complete"
```

**Agent delegation:**
```
✅ Agent reports success → Check VCS diff → Verify changes → Report actual state
❌ Trust agent report
```

## Why This Matters

From 24 failure memories:
- your human partner said "I don't believe you" - trust broken
- Undefined functions shipped - would crash
- Missing requirements shipped - incomplete features
- Time wasted on false completion → redirect → rework
- Violates: "Honesty is a core value. If you lie, you'll be replaced."

## When To Apply

**ALWAYS before:**
- ANY variation of success/completion claims
- ANY expression of satisfaction
- ANY positive statement about work state
- Committing, PR creation, task completion
- Moving to next task
- Delegating to agents

**Rule applies to:**
- Exact phrases
- Paraphrases and synonyms
- Implications of success
- ANY communication suggesting completion/correctness

## The Bottom Line

**No shortcuts for verification.**

Run the command. Read the output. THEN claim the result.

This is non-negotiable.
