---
name: kibana-error-log-test
description: 使用 user-kibana-mcp 在 logstash-serilog-test 查询今天日志，支持可视化展示、代码定位与修复建议。默认查询为 application:"<app>" and level:"ERROR"；若指定自定义消息，则切换为 application:"<app>" and msg:"<自定义>"。当用户提到 kibana、错误日志、ERROR、policyGuardian、今日日志、测试环境日志、日志修复时使用。
triggers:
  - kibana error log
  - Kibana错误日志
  - 测试环境错误日志
risk: external
enabled: true
---

# Kibana Error Log Test

## 目标

在测试索引 `logstash-serilog-test` 中查询今天的错误日志，并输出可视化结果、代码定位和可执行修复建议。

## 固定规则

- MCP 服务固定：`user-kibana-mcp`
- 调用工具固定：`execute_kb_api`
- 索引固定：`logstash-serilog-test`
- 默认查询关键词固定：`application:"<application>" and level:"ERROR"`
- 自定义消息搜索模式：`application:"<application>" and msg:"<自定义>"`
- 时间固定为今天（北京时间）：`now/d` 到 `now`，并在 range 中加 `time_zone: "+08:00"`

## 执行步骤

1. 解析 `application`
   - 优先读取 `src/ISS.IPSA.policyGuardian.Api/appsettings.json`。
   - 若存在 `Serilog -> Properties -> application`，取该值作为 `application`。
   - 当前仓库可解析到：`policyGuardian`，可直接作为默认值。
   - 若不存在，则在工作区搜索 `appsettings*.json` 中的 `Serilog.Properties.application`。
   - 若仍未找到，按以下兜底顺序执行：
     1) 在 `logstash-serilog-test` 做今天范围聚合（`terms` on `application.keyword`），给出 Top 应用名供用户选择；
     2) 若聚合也不可用，再引导用户输入应用名。

2. 组装查询字符串
   - 基础：`application:"<application>" and level:"ERROR"`
   - 若用户给了自定义关键词，切换为：`application:"<application>" and msg:"<自定义>"`（不再拼接 `level:"ERROR"`）

3. 执行统计查询（总数 + 模块分布）
   - 使用 `POST /api/console/proxy`
   - `params`:
     - `path`: `logstash-serilog-test/_search`
     - `method`: `POST`
   - `body` 使用以下结构：

```json
{
  "size": 0,
  "track_total_hits": true,
  "query": {
    "bool": {
      "must": [
        {
          "query_string": {
            "query": "application:\"<application>\" and level:\"ERROR\""
          }
        }
      ],
      "filter": [
        {
          "range": {
            "@timestamp": {
              "gte": "now/d",
              "lte": "now",
              "time_zone": "+08:00"
            }
          }
        }
      ]
    }
  },
  "aggs": {
    "by_module": {
      "terms": {
        "field": "module.keyword",
        "size": 3
      }
    }
  }
}
```

4. 执行明细查询（最近错误样本）
   - 同样走 `POST /api/console/proxy`
   - `path`: `logstash-serilog-test/_search`
   - 建议 `size: 50`
   - `_source` 建议包含：
     - `@timestamp`
     - `application`
     - `module`
     - `level`
     - `msg`
     - `create_datetime`
     - `TraceId`
     - `requestId`
   - `sort`: `@timestamp desc`

5. 可视化展示错误信息
   - 优先使用 Canvas 展示（总数卡片 + 模块 Top3 柱状图 + 最近错误列表）。
   - 最近错误样本建议增加“查看 Exception”行按钮，点击后在右侧面板显示该行异常摘要与完整堆栈（默认折叠）。
   - 若当前上下文不便使用 Canvas，则至少输出结构化可视化替代：
     - 错误总数
     - 按 `module` 的 Top3 分布
     - 最近 3 条错误（时间、模块、消息）

6. 代码定位（有仓库/无仓库都可工作）
   - 若工作区存在源码：
     - 优先使用 `Exception` 堆栈中的 `Namespace.Class.Method + line` 精确定位；
     - 再用 `module` / `msg` 关键词在仓库内二次确认，展示代码片段。
   - 若工作区没有对应源码：
     - 仍展示“日志侧定位”信息：异常类型、关键信息、堆栈方法名、文件名（若日志有）、行号（若日志有）；
     - 明确标注“当前工作区无对应源码，无法展示本地代码块”；
     - 引导用户提供源码仓库路径或挂载对应仓库后再做代码块展示。

7. 修复建议与落地能力（默认先建议，按用户指令再改代码）
   - 至少输出 3 类内容：
     1) 根因判断：基于日志证据和代码片段说明“为什么报错”；
     2) 修复方案：最小改动方案 + 稳健方案（含取舍）；
     3) 验证方法：回归步骤、查询条件、期望日志特征。
   - 当用户明确要求“直接修”时再进入代码修改：
     - 仅修改与当前错误直接相关文件；
     - 增加必要防御分支（幂等、判空、重复键处理、异常上下文日志）；
     - 若改了分支逻辑，优先补对应测试；
     - 修改后执行最小验证（编译/测试/关键查询）。
   - 当证据不足时，不臆测修复：先回到日志侧补证据（追加 query、样本、时间窗口）。

8. 修复补丁建议（仅建议，不自动改代码）
   - 当用户要求“补丁建议”时，输出必须包含以下四段：
     1) 影响范围：受影响文件/方法/分支；
     2) 建议补丁：给出可落地的修改点（必要时附伪 diff）；
     3) 风险与回滚：说明潜在副作用和回滚方式；
     4) 验证步骤：编译/测试/日志回归查询。
   - 生成补丁建议时遵循：
     - 优先最小改动（避免大范围重构）；
     - 补丁需解释“为何能解决日志中的具体异常”；
     - 若涉及幂等/唯一键冲突，必须覆盖并发和重复数据场景；
     - 明确标注“建议补丁”与“已实际修改代码”是两种不同状态。
   - 建议补丁输出模板：

```markdown
## 修复补丁建议

### 1) 影响范围
- 文件：`<path>`
- 方法：`<method>`
- 触发条件：`<来自日志的条件>`

### 2) 建议补丁（未落地）
- 修改点A：`<what + why>`
- 修改点B：`<what + why>`

```diff
<可选：关键伪diff，聚焦核心分支>
```

### 3) 风险与回滚
- 风险：`<行为变化/性能/兼容性>`
- 回滚：`<如何快速恢复>`

### 4) 验证清单
- [ ] 编译通过
- [ ] 相关单测/集成测试通过
- [ ] Kibana 查询 `<query>` 在 `<time range>` 下错误显著下降
```

9. 钉钉机器人输出模式（用于告警播报/巡检日报）
   - 默认自动发送：分析完成后直接调用 `user-dingtalk-robot` 发送，不再询问用户是否发送。
   - 若用户明确说“只预览不发送”，才切换为仅输出 Markdown 预览。
   - 发送通道固定：`send_message_by_custom_robot`（webhook 自定义机器人）。
   - webhook 参数处理规则：
    1) 先读取**当前进程**环境变量 `DINGTALK_WEBHOOK_TOKEN` 作为 `robotToken`；
    2) 若为空，必须再读取**User/Machine 级**环境变量 `DINGTALK_WEBHOOK_TOKEN`（例如 PowerShell 用 `[Environment]::GetEnvironmentVariable("DINGTALK_WEBHOOK_TOKEN","User|Machine")`）；
    3) 若仍为空，再按“当前进程 -> User -> Machine”顺序读取 `DINGTALK_WEBHOOK_URL`，若为完整 webhook URL（形如 `https://oapi.dingtalk.com/robot/send?access_token=...`），提取 `access_token` 作为 `robotToken`；
    4) 若环境变量都没有，再使用用户本次输入（完整 webhook URL 或 `robotToken`）；
    5) 若仍无法得到 token，先输出 Markdown 预览并提示补充 token。
   - 若用户明确表示“有环境变量但没读到”，默认执行一次 User/Machine 级重新读取，再决定是否向用户要 token。
   - 自动发送时调用：
     - `toolName`: `send_message_by_custom_robot`
     - `arguments`: `{ "title": "<标题>", "text": "<markdown正文>", "robotToken": "<token>", "isAtAll": false }`
   - 发送失败时回退策略：
     1) 回报错误码与错误信息；
     2) 同时输出可复制 Markdown 内容；
     3) 提示用户补充/更新 token 后重试。
   - 安全要求：
     - 不在仓库文件中硬编码 webhook token；
     - token 仅在当前会话内使用，必要时提醒用户轮换。

10. 钉钉消息模板（Markdown）
   - 标题建议：`[测试环境][policyGuardian] 今日ERROR巡检`
   - 展示风格要求（默认执行）：
     - 优先使用“结论卡片 + 指标表格 + 行动清单”的结构；
     - 暂不使用颜色标签（不使用 `<font color=...>`），全部采用纯文本样式；
     - 模块分布与验证项优先用 Markdown 表格，避免纯长列表；
   - 文本控制在一屏可读，异常详情保留 1~2 条代表样本；
   - Exception 展示采用“默认摘要 + 可选展开原文”两层结构，默认先看摘要，按需查看原文。
   - 正文建议包含：
     - 查询条件（索引、应用、时间范围）
     - ERROR 总数
     - Top3 模块
     - 代表性异常（1~2条）
     - 代码定位（文件+方法+行号）
     - 修复补丁建议摘要（2~3条）
     - 验证清单（3条以内）

```markdown
### [测试环境][<application>] 今日ERROR巡检

> 索引：logstash-serilog-test｜时间：今天（+08:00）｜应用：<application>
>
> 风险等级：高（ERROR 持续增长，建议优先处置 Top 模块）

#### 核心指标

| 指标 | 数值 | 说明 |
| --- | ---: | --- |
| ERROR总数 | **<count>** | 今天累计 |
| Top模块 | **<module1>** | 占比最高 |
| Top模块占比 | **<top_ratio>%** | Top1 / ERROR总数 |

#### 模块Top3

| 模块 | 条数 | 占比 |
| --- | ---: | ---: |
| <module1> | <count1> | <ratio1>% |
| <module2> | <count2> | <ratio2>% |
| <module3> | <count3> | <ratio3>% |

#### 代表性异常（样本）
- **<exception summary 1>**
- **<exception summary 2>**

#### Exception 详情
| 样本 | Exception摘要 | 堆栈定位 |
| --- | --- | --- |
| #1 | `<exception_first_line_1>` | `<stack_location_1>` |
| #2 | `<exception_first_line_2>` | `<stack_location_2>` |

#### Exception 原文（可选展开）
- #1（TraceId=`<trace_id_1>`）：`<exception_raw_1>`
- #2（TraceId=`<trace_id_2>`）：`<exception_raw_2>`

#### 代码定位
| 文件 | 方法 | 行号 |
| --- | --- | --- |
| `<path>` | `<method>` | `<line>` |

#### 修复建议（未落地）
1. **<patch point A>**
2. **<patch point B>**

#### 验证清单
| 项 | 状态 | 说明 |
| --- | --- | --- |
| 编译 | ⏳ 待验证 | `dotnet build` |
| 测试 | ⏳ 待验证 | 关键测试/单测 |
| 日志回归 | ⏳ 待验证 | Kibana ERROR 下降 |
```

11. 发送结果处理
   - 自动发送后必须回报发送结果：
     - 成功：返回 `errcode=0` / `errmsg=ok`；
     - 失败：返回错误码与可执行修复建议（例如签名不匹配、关键词不匹配、限流）。
   - 若发送失败，仍保留可复制的 Markdown 内容，便于用户手动发送。

12. 环境变量优先执行（推荐）
   - 执行“自动发送到钉钉”前，按以下优先级读取：
    1) `DINGTALK_WEBHOOK_TOKEN`（当前进程）
    2) `DINGTALK_WEBHOOK_TOKEN`（User 级）
    3) `DINGTALK_WEBHOOK_TOKEN`（Machine 级）
    4) `DINGTALK_WEBHOOK_URL`（当前进程 / User / Machine，提取 `access_token`）
   - 若能读取到 token，则直接调用 `send_message_by_custom_robot` 发送；
   - 若读取不到，先回传“未发送（缺少 token）”并附 Markdown 预览，再提示用户补充 webhook URL 或 token。

## 输出模板

- 查询条件：
  - 索引：`logstash-serilog-test`
  - application：`<application>`
  - 关键词（默认）：`application:"<application>" and level:"ERROR"`
  - 关键词（自定义消息模式）：`application:"<application>" and msg:"<自定义>"`
  - 时间：今天（`+08:00`）
- 统计结果：
  - ERROR 总数：`<count>`
  - 模块 Top3：`<module/count 列表>`
- 样本日志：
  - `<北京时间> | <module> | <msg>`
- 简短结论：
  - 是否集中在某个模块
  - 是否存在重复报错模式
- 修复建议：
  - 根因判断（证据链）
  - 建议改动点（文件/方法/片段）
  - 验证清单（修复后如何确认）
- 钉钉发送：
  - 发送通道（webhook）
  - 发送结果（成功/失败 + 错误码）

## 注意事项

- 不要把 “今天” 按 UTC 切天；必须加 `time_zone: "+08:00"`。
- `level` 在该索引里通常是大写 `ERROR`，先按大写查。
- 若 `module.keyword` 聚合失败，可回退为仅输出总数与明细样本，并提示字段映射限制。
