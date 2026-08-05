# ADR 0014：`run_command` 使用平台默认 Shell

- 状态：已接受
- 日期：2026-08-05

## 背景

旧的模型工具契约要求模型分别提供 `program` 和 `args`，`CommandRuntime` 直接启动目标程序。该契约适合宿主内部的类型化进程 API，但与用户在终端中书写命令的习惯不同，也使 Windows 模型无法自然使用 PowerShell 管道、变量和 cmdlet。Codex 的 shell 工具在模型边界接收一段命令文本，再由宿主按平台选择 shell 并转换成结构化进程请求。

## 决策

1. 模型可见的 `run_command` 参数改为 `command`、工作区相对 `cwd` 和 `timeoutMs`。`program + args` 不再属于新工具 Schema。
2. Windows 按 `pwsh`、Windows PowerShell、`cmd.exe` 的顺序选择可用 shell。PowerShell 使用 `-NoProfile -Command`，并在用户命令前设置控制台 UTF-8 输出编码。Unix 优先使用受支持的用户 `SHELL`，再按平台回退到 `zsh`、`bash` 或 `sh`，通过 `-c` 执行。
3. shell 选择和参数转换位于 execution 层；工具处理器仍把转换结果交给既有 `CommandRuntime`。工作目录规范化、工作区逃逸拒绝、超时、取消、进程树清理、有界输出和脱敏规则保持不变。宿主 IPC 的 `start_command` 继续使用结构化 `program + args`，不与模型工具契约混合。
4. 策略在转换前评估原始脚本文本。只有单条、可静态拆分且属于已知只读或构建/测试类别的命令可以自动运行；管道、重定向、串联、换行、变量展开、命令替换、未知程序和写入/破坏性命令必须审批。模型参数中的权限声明不能改变该决策。
5. 最终工具结果记录实际 shell 类型。前端直接显示新 `command` 文本；已持久化的旧 `program + args` 工具事件继续只读展示，避免历史会话失去命令详情。

## 影响

- Windows 上 `run_command` 的命令语法与 k-Coder 所在的 PowerShell 环境一致，模型可以自然使用 cmdlet；列表中的每个工具活动仍是一次独立、可审计的命令会话。
- shell 组合能力扩大了命令表达面，因此不能仅根据首个词推断整段脚本风险。无法证明为简单低风险命令的脚本必须进入审批。
- Provider 缓存的旧工具定义不能继续提交 `program + args`；新的请求必须遵循 `command` Schema。旧历史只保证展示兼容，不重新执行。

## 验收门槛

- 覆盖 Windows PowerShell、`cmd.exe` 和 Unix shell 参数转换。
- 覆盖新 Schema、旧参数拒绝、空命令拒绝和实际 shell 元数据。
- 覆盖只读、构建/测试、写入、破坏性、管道和动态脚本的策略分支。
- 通过前端构建、Rust 格式/检查/全量测试、双视口命令详情回归，并启动 `pnpm tauri dev` 验证桌面实例。
