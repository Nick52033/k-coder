# k-Coder 工具映射

此插件副本运行于 k-Coder。本文替代原 Codex 平台映射；无需修改 `~/.codex/config.toml`。

| 原文名称 | k-Coder 接口 |
| --- | --- |
| Skill / activate_skill | `plugin_skill_read({"pluginId":"superpowers@local","skillName":"<名称>"})` |
| 参考文件 | `plugin_resource_read({"pluginId":"superpowers@local","path":"skills/<目录>/<资源>"})` |
| Task / spawn_agent | `create_agent({"task":"明确任务","label":"review","forkTurns":"none"})` |
| 等待任务 | `wait_agent({"agentIds":["<实际返回的 id>"],"timeoutMs":30000})` |
| 后续指令 | `send_agent_message`，参数以当前工具 Schema 为准 |
| 重启已停止任务 | `resume_agent`，参数以当前工具 Schema 为准 |
| 查询与停止 | `list_agents` / `close_agent` |
| TodoWrite / update_plan | `update_plan({"steps":[{"step":"任务","status":"in_progress"}]})` |
| Read / Write / Edit | `read_file` / `write_file` / `apply_patch` |
| 搜索 | `search_repository` / `list_directory` |
| Bash / exec_command | `run_command({"command":"...","cwd":".","timeoutMs":30000})` |

仅使用当前工具目录实际提供的接口。委派不可用时由主任务顺序完成，不下载新的智能体运行时。不要发送模型选择字段或未经当前 Schema 支持的参数。文件参数相对工作区，拒绝绝对路径与 `..`；Shell 仍由宿主策略、审批、取消和进程树清理管理。

先启动独立子任务，再推进不依赖其结果的工作，最后等待。任务 id 来自创建结果；不要自行拼接。项目要求源码留在当前工作区时遵守该边界，不能为了 worktree 指引将改动写到外部目录。

生成的说明、计划、报告放入 `docs/`。用户明确要求实现即按授权完成；不要因 Skill 重复询问是否实施。默认不执行 `git commit`、`git push`、发布、发消息；这些需要对应的用户授权。
