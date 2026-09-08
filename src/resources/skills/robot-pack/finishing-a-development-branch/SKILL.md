---
name: finishing-a-development-branch
description: Prepare completed branch work for handoff after verification and explicit integration instructions.
triggers: [finish branch, prepare pull request, branch handoff, integrate changes]
risk: read
category: development_delivery
enabled: true
---
# Finishing a Development Branch

Inspect the final diff, run the repository-required verification, and report failures or unrelated dirty files before integration. Confirm that documentation, migrations, generated assets, and public contracts are synchronized.

Only commit, push, merge, open a pull request, delete a branch, or remove a worktree when the user explicitly requests that action. Preserve unrelated user changes and never force-push or force-delete without separate clear authorization. When integration is requested, report the branch, commit, remote target, exact checks, and any residual risk.
