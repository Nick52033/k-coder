# permission-fe 最新对话报错排查

排查日期：2026-09-18。范围为读取真实会话、核对 k-coder 实现及复现只读搜索行为；未修改业务代码或 k-coder 运行时代码，未部署。

以上为初次排查阶段的范围。用户随后要求修改，已完成 k-coder 执行与诊断修复；具体改动、测试及原生验收见 [命令搜索与错误诊断修复验证](命令搜索与错误诊断修复验证.md)。

## 对应会话与结果

- 项目：`D:\code\运维平台\华为云\permission-fe`。
- 用户任务：在薪酬福利管理平台菜单下增加「基数调整同步 SAP」页面，提供单号、同步 0014/9063 按钮与结果展示。
- Thread：`1d676643-15d4-4b6c-9079-0d3e56f7a46f`。
- Turn：`590833f0-0191-49e6-93b0-90ab711008e4`。
- 实际运行：2026-09-18 10:40:55 至 10:53:01（本机时间），最终事件为 `turn_completed`。
- Provider/模型：日志记录为 `TokenHub / deepseek-flash`。
- 共 157 次工具结果：135 次成功、22 次失败。119 次模型调用。
- 事件来源：`C:\Users\nealk\AppData\Roaming\com.kcoder.app\runtime-data\sessions\1d676643-15d4-4b6c-9079-0d3e56f7a46f.jsonl`，共 1382 行。下文行号均为该 JSONL 的一基行号。

## 22 次失败的实际分类

| 分类 | 次数 | JSONL 行号 | 结论 |
| --- | ---: | --- | --- |
| 搜索没有匹配 | 10 | 368、380、480、501、508、524、531、691、991、1344 | 8 次为空输出后补上 `rg: no matches`；2 次组合命令已有前段正常输出，最后搜索没有匹配而使整条命令标为失败 |
| PowerShell 下把通配符直接传给 rg | 3 | 348、871、1332 | `Filter/*.cs`、`Domain\*\PPService.cs`、`Model/*.cs` 不由 PowerShell 为原生程序展开，报 Windows 错误 123 |
| 猜测了不存在的文件路径 | 1 | 248 | `Permission.Util/Permission.Util/Model/ReturnModel.cs` 不存在，报错误 2 |
| PowerShell 引号语法错误 | 1 | 659 | 双引号字符串中使用 `\"`，未按 PowerShell 规则转义，报 `TerminatorExpectedAtEndOfString` |
| 搜索等待标准输入而超时 | 1 | 632 | rg 没有显式搜索路径，宿主保持输入管道打开，约 120969ms 后 `timed_out` |
| 文件工具跨工作区读取被拒绝 | 1 | 864 | `read_file` 请求 `../permission-be/.../PPController.cs`，含父目录穿越，触发既有安全校验 |
| 无效补丁片段 | 1 | 1088 | 第一个 `@@` 片段只有上下文，没有增加/删除行，解析器拒绝 |
| 业务项目测试失败及复查 | 4 | 1216、1228、1276、1312 | 围绕同一组 SaveCompanyConfig 既有失败反复验证；并非 4 组不同缺陷 |

只有一次是 `read_file` 权限拒绝。大部分截图红叉来自搜索结果或模型生成的 Shell 命令，不应全部归因为权限配置。

## 三张截图对应原因

### 第一张：DeletePmApplyByBillNo 与 SsrRecord 搜索

第一个命令用分号串联两次 rg。前半段成功输出若干路径；后半段去 `Er/PmApplyBll.cs` 搜索 `DeletePmApplyByBillNo`，没有匹配，整条工具结果为 `success=false`。

后两次对 `PmApplyBll.cs` 和 `SsrRecordList.cshtml` 的查询也没有匹配。以安装端内置 rg 对当前文件进行只读复查，均为退出码 1、stdout/stderr 都为空。

### 第二张：读取其他项目

`read_file` 只接受工作区内的精确相对路径，拒绝 `..`。请求的 `../permission-be/...` 被正确拒绝；即使完整访问模式自动批准工具执行，也不改变文件工具的工作区边界。

紧接着的 Shell 查询失败原因是传入 `..\..\permission-be\src\ISS.IPSA.Role.Domain\*\PPService.cs`，其中 `*` 被原样传给 rg，报文件名语法错误。它与上一条文件工具拒绝具有不同原因。

### 第三张：组合命令与错误摘要

包含 `<ProjectReference|<Compile|\.cs\"` 的命令首先遇到 PowerShell 引号解析错误，实际尚未执行搜索。虽然该命令也有 `*.csproj` 的通配符问题，当前 recoveryHint 只从命令形状判断，给出了通配符提示，遮蔽了首先发生的语法错误。

`Get-ChildItem ...; rg ... SsrRecordList.cshtml` 中，列目录成功；最后一段搜索没有匹配。界面拿整段输出的第一个非空行作为失败摘要，因此可能显示正常文件名，让人误以为文件本身有错误。

## k-coder 实现中确认的问题

### 1. 工具事实和界面缺少「未匹配」语义

- `src-tauri/src/tools/mod.rs:1326`：仅 `Exited { code: 0 }` 被判定成功。
- 同文件 `1389`：空输出、Shell 退出码 1 且命令含 rg 时，补上 `rg: no matches (exit code 1).`。
- `src/stores/reducers/agentEventReducer.ts:433`：`success=false` 直接转为 `failed`。
- `src/components/ConversationActivity.tsx:1131` 附近：失败一律展示为「运行失败」。

因此普通未匹配和真正执行错误共享红色失败状态。Shell 的退出码不能直接等同于内部某个 rg 进程的退出码，尤其涉及管道、串联和被丢弃的 stderr 时；后续修复不能把所有包含 rg 的退出码 1 都无条件当成功。

### 2. 非交互命令的输入管道一直保持打开

- `src-tauri/src/execution.rs:630`：进程输入使用 `Stdio::piped()`。
- 同文件 `652`：保存输入 writer；`683` 才在命令结束后释放。
- `run_command` 的本次调用没有发送输入或主动结束输入的操作。

失败命令为：

```powershell
rg -n "SyncSsrRecord|class SsrRecordDelInput|CompanyConfigSaveInput" --glob '*.cs' -l
```

在临时目录创建一个包含匹配内容的 `sample.cs`，以 Windows PowerShell、安装端同一个 `D:\apps\k-coder\tools\rg.exe`、打开的 stdin 管道复现：

| 命令差异 | 复现结果 |
| --- | --- |
| 不写搜索目录 | 等待 4 秒仍未结束，stdout/stderr 均为空；随后清理本次复现进程树 |
| 命令末尾加 `.` | 约 2.74 秒完成，退出码 0，输出 `./sample.cs`（Windows 路径分隔符） |

这支持超时来自标准输入等待，而非仓库搜索耗时。后续应明确区分非交互工具与可写入输入的进程会话，并引导仓库搜索显式给出目录。

### 3. 错误摘要没有可靠定位失败来源

`src/components/ConversationActivity.tsx:1138` 的 `commandFailureSummary` 优先显示 recoveryHint，否则使用输出的第一个非空行。它不能区分前一段命令的正常 stdout、后一段的失败或 PowerShell 解析错误。

`src-tauri/src/tools/mod.rs:1408` 的恢复提示仅根据 Shell、失败状态和命令文本判断通配符，没有先验证 stderr 的实际错误种类。真实日志中引号语法错误被附加为通配符恢复建议。

后续应以类型化失败原因和相应 stderr 为依据展示摘要，并避免正常 stdout 或不相干恢复提示覆盖真实诊断。

## 真正的业务测试结果

以下结果来自原会话执行日志，本次排查没有重新运行构建或修改 permission-fe：

- 构建：0 个错误、69 个警告，JSONL 1204 行。
- SyncSsrRecord 专项：11 个通过、0 个失败，JSONL 1300 行。
- 完整改动后的全量测试：74 项，71 通过、3 失败，JSONL 1312 行。
- 会话曾暂存业务改动和测试改动进行基线复查：62 项，59 通过、同样 3 失败，JSONL 1252、1264、1276 行；随后恢复暂存，JSONL 1288 行。
- 三个失败分别覆盖 SaveCompanyConfig 的后端空响应、非法 JSON、调用异常分支。测试数据只提供 CompanyNo，缺少 CompanyName，被前置校验提前返回「公司名称不能为空」，未进入原本要验证的后端异常分支。空响应测试的具体断言记录见 JSONL 1228 行，测试代码位于 `Permission.Business/Permission.Business.Tests/SsrRecordBllTests.cs`。

这组测试失败在原会话的基线复查中已存在。后续真实 SAP 联调与页面验收不能由专项单元测试通过替代。

## 建议顺序

1. 修复非交互命令标准输入生命周期，避免无路径 rg 等待输入直到超时；保留交互进程接口的既有能力。
2. 区分确认的搜索未匹配、权限拒绝、超时和命令执行错误；准确展示失败段与 stderr，保留审计中的原始退出事实。
3. 改进 PowerShell 语法与路径恢复指引，针对真实错误输出，不再用通用通配符建议遮蔽引号错误；继续保持工作区边界。
4. 减少猜测文件和重复 Shell 搜索，优先复用已发现路径、现有 search_repository 与有界读取。

本次仅新增此排查文档。未提交、未部署；未执行 k-coder 构建和原生桌面验收，不据此声称修复已经交付。
