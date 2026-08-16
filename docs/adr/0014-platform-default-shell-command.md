# ADR 0014：`run_command` 使用平台默认 Shell

- 状态：已接受；PowerShell 原生 `rg` 路径通配符诊断于 2026-08-16 由 `P10-109` 修订；Windows PowerShell 原生管道 CRLF 诊断于 2026-08-16 由 `P10-110` 修订
- 日期：2026-08-05

## 背景

旧的模型工具契约要求模型分别提供 `program` 和 `args`，`CommandRuntime` 直接启动目标程序。该契约适合宿主内部的类型化进程 API，但与用户在终端中书写命令的习惯不同，也使 Windows 模型无法自然使用 PowerShell 管道、变量和 cmdlet。Codex 的 shell 工具在模型边界接收一段命令文本，再由宿主按平台选择 shell 并转换成结构化进程请求。

## 决策

1. 模型可见的 `run_command` 参数改为 `command`、工作区相对 `cwd` 和 `timeoutMs`。`program + args` 不再属于新工具 Schema。
2. Windows 按 `pwsh`、Windows PowerShell、`cmd.exe` 的顺序选择可用 shell。PowerShell 使用 `-NoProfile -Command`，并在用户命令前设置控制台 UTF-8 输出编码。Unix 优先使用受支持的用户 `SHELL`，再按平台回退到 `zsh`、`bash` 或 `sh`，通过 `-c` 执行。
3. shell 选择和参数转换位于 execution 层；工具处理器仍把转换结果交给既有 `CommandRuntime`。工作目录规范化、工作区逃逸拒绝、超时、取消、进程树清理、有界输出和脱敏规则保持不变。宿主 IPC 的 `start_command` 继续使用结构化 `program + args`，不与模型工具契约混合。
4. 策略在转换前评估原始脚本文本。只有单条、可静态拆分且属于已知只读或构建/测试类别的命令可以自动运行；管道、重定向、串联、换行、变量展开、命令替换、未知程序和写入/破坏性命令必须审批。模型参数中的权限声明不能改变该决策。
5. 最终工具结果记录实际 shell 类型。前端直接显示新 `command` 文本；已持久化的旧 `program + args` 工具事件继续只读展示，避免历史会话失去命令详情。
6. Windows x86_64 桌面包固定内置 ripgrep 15.2.0。官方发布归档和解压后的 `rg.exe` 都使用固定 SHA-256 校验，许可证随资源分发；Tauri 将内置工具目录传给 execution 层，`CommandRuntime` 与 `NativePtyRuntime` 分别把它置于子进程 `PATH` 首位。该目录由宿主决定，调用方提供的环境不能覆盖其优先级；应用不在 Tauri/Tokio 多线程运行期间修改全局 `PATH`。
7. PowerShell 不替原生可执行程序展开 `dist/assets/CodeEditor-*.js` 这类路径通配符。模型契约和系统提示要求改用 `rg --glob 'CodeEditor-*.js' ... dist/assets`，或先由 `Get-ChildItem` 解析精确路径。失败命令命中“PowerShell + `rg` + 路径通配符”时，`ToolResult` 附加有界 `recoveryHint` 并把同一提示返回 Provider；运行时不得静默改写并重跑模型命令。
8. Windows PowerShell（`powershell.exe`）会把原生命令管道的文本重新编码为 CRLF；因此 `rg --files ... | rg 'name\.js$'` 的下游 `$` 行尾锚点可能得到空结果，而 `pwsh` 不存在同一兼容问题。模型契约和系统提示要求下游使用 `rg --crlf` 或改用 `Select-String`。运行时只有在实际 Shell 为 `powershell.exe`、命令以退出码 1 结束、输出为空、存在真实 Shell 管道且下游 `rg` 使用未转义 `$` 锚点时才附加专用 `recoveryHint`；已有输出、`pwsh`、超时、取消、无锚点或已经传入 `--crlf` 的命令不得误提示，也不得静默改写和重跑。

## 影响

- Windows 上 `run_command` 的命令语法与 k-Coder 所在的 PowerShell 环境一致，模型可以自然使用 cmdlet；列表中的每个工具活动仍是一次独立、可审计的命令会话。
- shell 组合能力扩大了命令表达面，因此不能仅根据首个词推断整段脚本风险。无法证明为简单低风险命令的脚本必须进入审批。
- Provider 缓存的旧工具定义不能继续提交 `program + args`；新的请求必须遵循 `command` Schema。旧历史只保证展示兼容，不重新执行。
- Windows 用户无需单独安装 ripgrep，模型命令和工作台终端可以直接调用 `rg`；其他平台在提供对应的已校验资源前继续使用系统工具。
- Windows 上 `rg pattern dist/assets/CodeEditor-*.js` 仍会按 PowerShell 原生语义失败，但模型会得到可直接执行的 `--glob` 修复提示，并可在同一 Turn 中恢复。
- Windows PowerShell 上原生管道后接带 `$` 锚点的 `rg` 仍保留原始执行和失败审计；Provider 会得到 `--crlf` 恢复提示。PowerShell 7、已有明确错误输出和其他失败状态继续保留原诊断，不会被错误归因为 CRLF。

## 验收门槛

- 覆盖 Windows PowerShell、`cmd.exe` 和 Unix shell 参数转换。
- 覆盖新 Schema、旧参数拒绝、空命令拒绝和实际 shell 元数据。
- 覆盖只读、构建/测试、写入、破坏性、管道和动态脚本的策略分支。
- 覆盖内置工具目录优先于调用方 `PATH`，以及清空调用方 `PATH` 后仍能通过 PowerShell 执行 `rg --version`。
- 覆盖 PowerShell 路径通配符失败返回恢复提示，以及等价 `rg --glob` 命令使用同一内置二进制成功执行。
- 使用真实 `powershell.exe` 与内置 `rg.exe` 覆盖原生管道的 CRLF 行尾行为，并覆盖 `--crlf` 恢复、`pwsh` 排除、非空输出、无锚点和非退出终态分支。
- 通过前端构建、Rust 格式/检查/全量测试、双视口命令详情回归，并启动 `pnpm tauri dev` 验证桌面实例。
