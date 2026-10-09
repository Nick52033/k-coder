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

   脚本读 `runtime.jsonl` 的全部 error 记录，按内置规则表自动分成三组输出：**需要修复（产品缺陷）/ 待人工判定 / 不是 bug**，每条带时间、会话、根因、源码证据和修复建议，末尾给总结论和复核入口。`--all` 扫全部历史，默认看 24 小时；`--verbose` 附注每条命中记录。规则表与脚本 `RULES` / `OBSERVATIONS` 一一对应，新增类别时两边同步改，别让脚本和文档给出两套结论。

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

   模型侧空响应，**不是 k-coder 对话 bug**。判定方法：看失败前最后一次 `provider_call_usage`——若 `details.reasoningOutputTokens` ≈ `usage.outputTokens`（模型把输出预算几乎全用于 reasoning），且模型是阶跃星辰（baseUrl `https://api.stepfun.com/step_plan/v1`）的 `step-5-preview` / `step-3.7-flash`，即可坐实。健康的 provider 失败长这样：HTTP 4xx/5xx、流中断；干净的 completed + 空内容只在该模型上见过。

   还要区分**部署版本行为**：源码 `src-tauri/src/agent/mod.rs` 已有空响应守卫（`MAX_EMPTY_RESPONSE_RETRIES=2`，先有界重试、仍空则 `finish_completed` + `empty_response_message()` 优雅收尾，不判整轮失败）。对比 `D:\apps\k-coder\k-coder.exe` 的 mtime 与源码 mtime：exe 早于源码改动时，运行中的仍是旧逻辑，`turn_failed` 红卡片会复现——这是部署滞后，不是代码缺陷。结论引导用户「重试」或换 `gpt-6-sol`（同 provider 下实测正常）。

   ### B. `tool_failed`：tool execution denied: path must be relative…

   工作区沙箱按设计拒绝工作区外路径（含符号链接、目录联接逃逸防护），**不是 bug**。模型反复试探绝对路径属模型侧行为；正确姿势是在工作区内操作或先把内容复制进来。

   ### C. `tool_failed`：run_command 有输出但 exitCode 非零

   先在 Windows PowerShell 里原样复现该命令，看真实退出码，再下结论。两个高频模型侧原因：

   - 命令里带 `2>$null` 把 rg 对不存在路径的报错吞了（模型把别的仓库的目录结构套到本仓库，rg 对缺失路径退出非零）。stdout 看着正常，退出码是真的失败。
   - `rg … | Select-Object -First N` 扫描大目录时，接收端提前收手会终止 rg，PowerShell 进程退出码变 1（broken pipe）。行数是完整的，但退出码非零。

   k-coder 的判定是**有意保守**：只对「无错误抑制、形状无歧义的 `rg … | Select-Object -First N`」判为 `bounded_output` 成功（`src-tauri/src/tools/command_diagnostics.rs` 的 `is_bounded_rg_search` / `is_unambiguous_rg_search`）；带 `2>$null`、分号串联、中间夹 `Where-Object` 的一律保持失败——否则模型永远学不到自己的路径写错了。所以这类 `tool_failed` **也不是判定 bug**，是模型命令自身的问题。

4. **输出结论**。每条 error 一行：时间 + 事件 + 一句话根因；然后给「是否对话 bug」总判断和修复建议。没有代码缺陷就明说「无需修复」，不要把按设计行为包装成待修问题。

## 结论模板（照这个格式写给用户）

```text
时间窗内 N 条 error，集中在会话 <threadId>（<最近活跃时间>）：

1. HH:MM turn_failed「provider completed without text or a tool call」
   最后一次调用 out=58 / thinking=56，模型 step-5-preview 把预算全用于 reasoning 后空完成
   → 模型侧空响应，非对话 bug。部署 exe（14:18）早于源码空响应修复（15:52），跑的是旧逻辑。

2. HH:MM tool_failed read_file 绝对路径
   → 工作区沙箱按设计拒绝，非 bug。

结论：不是 k-coder 对话 bug。原因 1 换 gpt-6-sol 或点重试即可；该会话在用户切换模型后已自行完成。
```

## 注意

- runtime.jsonl 可能含 `logs_cleared` 记录：用户手动清过日志时，窗口内无 error 不等于没发生错误，要如实说明。
- 结论涉及「部署版本是否含某修复」时，必须对比 exe mtime 与源码 mtime / `git status`，别凭记忆断言。
- 涉及源码判定逻辑的结论，落到具体文件行（如 `src-tauri/src/tools/command_diagnostics.rs`），方便用户复核。
