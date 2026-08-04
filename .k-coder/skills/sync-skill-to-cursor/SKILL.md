---
name: sync-skill-to-cursor
description: Sync a Codex skill into Cursor's user skills directory by skill name. Use when the user provides a skill name and wants that skill copied from Codex skills to Cursor skills, or wants an existing Cursor copy updated/replaced.
triggers:
  - sync skill
  - 同步 skill
risk: external
enabled: true
---

# Sync Skill To Cursor

Use this skill when the user asks to sync, copy, update, or install a Codex skill into Cursor.

## Workflow

1. Get the skill name from the user request. If the request does not include a concrete skill name, ask for it.
2. Prefer the bundled script for deterministic copying:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "<this-skill>\scripts\sync_skill_to_cursor.ps1" -SkillName "<skill-name>"
```

3. Treat the default source directory as `%USERPROFILE%\.codex\skills` unless `CODEX_SKILLS_DIR` is set.
4. Treat the default Cursor target directory as `%USERPROFILE%\.cursor\skills` unless `CURSOR_SKILLS_DIR` is set.
5. If the target skill already exists, update it by replacing the target skill folder after confirming the resolved target remains inside the Cursor skills directory.
6. Report the source path, target path, and whether the target was created or updated.

## Options

- Use `-SourceRoot "<path>"` to sync from a non-default Codex skills directory.
- Use `-TargetRoot "<path>"` to sync to a non-default Cursor skills directory.
- Use `-DryRun` to inspect what would happen without changing files.

## Safety

Do not manually delete or overwrite Cursor directories outside the resolved target root. If a command fails because the source skill is missing, list the available skills in the source root and ask the user to choose the correct name.
