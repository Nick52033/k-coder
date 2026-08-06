---
name: workspace-review
description: 审查当前工作区的代码变更，优先识别正确性、安全性、兼容性和测试覆盖问题；适用于用户要求 review、代码审查或检查本地修改的场景。
triggers:
  - workspace review
  - review changes
  - 代码审查
  - 审查修改
risk: read
enabled: true
---

# Workspace Review

Review the current workspace without modifying files.

- Inspect the relevant status, diffs, contracts, call sites, and existing tests before drawing conclusions.
- Prioritize concrete correctness, security, compatibility, data-loss, and regression risks over style preferences.
- Report findings first, ordered by severity, with precise file and line references.
- Explain the triggering condition and practical impact for each finding.
- Call out missing tests when they leave a changed public contract or safety branch unverified.
- Keep the summary brief and place it after the findings.
- If no actionable issue is found, say so explicitly and identify any remaining test gap or residual risk.

This Skill supplies review instructions only. It does not grant additional tool permissions.
