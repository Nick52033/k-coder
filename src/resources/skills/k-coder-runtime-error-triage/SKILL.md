---
name: k-coder-runtime-error-triage
description: 排查 k-coder 桌面端运行时错误日志：一条命令一键巡检 runtime.jsonl 的 error 记录，自动分类「需要修复的产品缺陷 / 按设计或模型侧行为 / 待人工判定」并给出源码证据；也可按时间窗过滤、用 threadId 读 sessions/<threadId>.jsonl 压缩时间线复现单个失败对话。
triggers:
  - 查下运行日志
  - 运行日志报错
  - 最近半小时有没有报错
  - 这轮对话为什么失败
  - 是不是 bug
  - 巡检日志
  - 哪些是 bug
  - 一键巡检
  - turn_failed
  - provider completed without text or a tool call
  - runtime error triage
risk: read
enabled: true
category: observability
---

# k-coder 运行时错误排查

## 日志位置与两种格式（别混用字段名）

运行时数据在 `%APPDATA%\com.kcoder.app\runtime-data\`：

| 文件 | 格式 | 关键字段 |
| --- | --- | --- |
| `logs/runtime.jsonl` | 每条 `{level, timestampMs, event, fields{...}}` | `fields.threadId` 定位对话；`level=="error"` 即错误 |
| `sessions/<threadId>.jsonl` | 每条 `{type, createdAtMs, data{...}}`（schemaVersion 11） | 完整事件流水 |

两条流水的事件名体系不同（如 `turn_failed` 在 runtime 里是顶层 `event`，在 session 里是 `type`），写过滤脚本时注意。

## 步骤

0. **一键巡检（首选）**。用户问「哪些是 bug、哪些不是」时直接跑：

   ```bash
   python <本 skill 目录>/scripts/triage_runtime_errors.py classify [--minutes 1440] [--all] [--verbose]
   ```

   脚本读 `runtime.jsonl` 的 error 记录，按规则表分成三组输出：**已确认需要修复 / 待人工判定 / 按设计或输入问题**。规则只能提出调查线索，不能仅凭历史 error 确认当前源码仍有缺陷；涉及版本和成败时须核对当前源码、运行版本及完整工具结果。`--all` 扫全部历史，默认看 24 小时；`--verbose` 附注命中记录。

1. **时间窗过滤 error 级记录**（默认半小时；用户给别的窗口就用用户的）：

   ```bash
   python <本 skill 目录>/scripts/triage_runtime_errors.py errors --minutes 30
   ```

   输出按 threadId 分组并标注每个会话最近活跃时间——最近活跃且含 error 的那个通常就是用户说的「当前对话」。

2. **复现失败过程**（不要直接 dump 整个 `.jsonl`，大会话上万行会炸上下文）：

   ```bash
   python <本 skill 目录>/scripts/triage_runtime_errors.py session --thread <threadId> --since-ms <cutoff epoch 毫秒>
   ```

   脚本已做压缩：只保留 user_message / turn_* / provider_call_usage / assistant_tool_calls / tool_result / assistant_message 等关键事件并截断文本。若要看某次工具调用的完整命令或完整输出，再单独读该行（`assistant_tool_calls` 里 `calls[].arguments` 是完整参数；`runtime.jsonl` 里的 arguments 可能被截断，别当全文用）。

3. **逐条判定根因**。error 级记录绝大部分落进下面三类，先按表判定，剩不下的再深挖源码。

   ### A. `turn_failed`：provider completed without text or a tool call

   空完成可能来自模型输出预算、协议适配或运行版本，应先列为待核对。读取失败前的 Provider 用量、响应终态和当前收尾源码，不能仅因模型名称或 reasoning 占比就排除适配器缺陷。

   涉及部署版本时，核对当前源码与实际构建版本；文件 mtime 只能提供线索，不能证明正在运行的进程包含某项修复。不要依赖这份文档中历史守卫次数或模型推荐来判断当前行为。

   ### B. `tool_failed`：tool execution denied: path must be relative…

   工作区沙箱按设计拒绝工作区外路径（含符号链接、目录联接逃逸防护），**不是 bug**。模型反复试探绝对路径属模型侧行为；正确姿势是在工作区内操作或先把内容复制进来。

   ### C. `tool_failed`：run_command 有输出但 exitCode 非零

   先读取相关会话完整 `tool_result`，核对退出码、stderr 和失败汇总，不要仅凭有输出判定成功，也不要自动重跑可能写文件、修改系统或访问外部服务的历史命令。

   - **真实失败优先**：`test result: FAILED`、panic、失败断言必须列为待修根因调查，不能被管道或 `Finished test profile` 覆盖。
   - **状态不明需核对**：Windows PowerShell 的 `2>&1 | Select-Object` 可能产生 `NativeCommandError`；构建和测试应直接运行，运行时已有有界输出。不能按输出关键词强制成功。
   - **确认无匹配是正常结果**：只有形态可确认的 rg、退出码 1、完整空输出且无 stderr 才归为 `no_matches`；保留原始退出码。路径不存在、非法正则、错误抑制、复杂脚本和其他非零退出码仍保留失败。
   - **历史现象不代表当前缺陷**：乱码、网关恢复失败等应核对源码和运行版本，不能凭旧日志直接宣称解码或回退缺失。

   Shell 预检与安全评估互相独立：恢复提示不能授予权限，原始命令必须继续审计，不静默改写后重跑。

4. **输出结论**。每条 error 一行：时间 + 事件 + 已确认原因或待核对项；然后给「是否已确认当前源码缺陷」和修复建议。明确区分已确认无须修改、证据不足和已确认需要修复，不能把未确认缺陷写成「没有缺陷」。

## 结论模板（照这个格式写给用户）

```text
时间窗内 N 条 error，集中在会话 <threadId>（<最近活跃时间>）：

1. HH:MM tool_failed run_command
   完整输出包含 test result: FAILED，16 项失败。
   → 真实测试失败；待对照测试夹具、断言和当前源码定位根因，不能作为 Shell 噪声忽略。

2. HH:MM tool_failed read_file 绝对路径
   → 工作区沙箱按设计拒绝；修正调用路径，不放松路径安全检查。

结论：已确认一类输入路径问题；测试失败尚需定位根因。日志本身不足以判断当前源码是否已修复，不重跑有副作用的历史命令。
```

## 注意

- runtime.jsonl 可能含 `logs_cleared` 记录：用户手动清过日志时，窗口内无 error 不等于没发生错误，要如实说明。
- 结论涉及「部署版本是否含某修复」时，核对构建版本和实际运行进程；exe 与源码 mtime / `git status` 只作线索，不是证明。
- 涉及源码判定逻辑的结论，落到具体文件行（如 `src-tauri/src/tools/command_diagnostics.rs`），方便用户复核。
