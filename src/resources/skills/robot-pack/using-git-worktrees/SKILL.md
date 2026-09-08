---
name: using-git-worktrees
description: Isolate feature work in a Git worktree when the user or repository workflow requires isolation.
triggers: [git worktree, isolated workspace, isolate branch, feature worktree]
risk: read
category: development_delivery
enabled: true
---
# Using Git Worktrees

First determine whether the current checkout is already isolated and whether the user has authorized creating another worktree. Prefer the platform's supported worktree mechanism. If manual Git worktrees are required, resolve the repository root, choose a repository-approved location, verify project-local worktree directories are ignored, and use the required branch naming convention.

Do not move or delete an existing worktree, rewrite a user's branch, or clean up a worktree you did not create. A worktree is an isolation mechanism, not a prerequisite when the user wants work in the current checkout.
