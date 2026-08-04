---
name: third-party-interface-spec
description: 生成第三方接口对接规格，并按现有 C# 项目结构生成代码。支持 SAP Excel/Invoke、iDataPlatform 页面采集、curl、Apifox MCP OAS 或混合资料，覆盖字段映射、认证配置、token、Header、待确认问题和开发指令。资料缺失时先询问支持的资料类型；多个 SAP sheet、接口或 FunctionName 时先列候选并等待选择；页面采集必须使用本次明确的报表名称/搜索目标，不得沿用历史搜索词。
triggers:
  - third-party-interface-spec
  - 第三方接口
risk: external
enabled: true
---

# 第三方接口开发规格

将原始接口资料整理成“可直接指导 AI 写 C# 对接代码”的开发规格，并默认在当前 C# 项目中按现有代码结构编码落地。

默认行为：

- 当用户说“执行 third-party-interface-spec”“执行技能”“生成接口规格”或类似请求，并且本次已提供原始资料、文件路径、curl 或明确的中台采集目标时，默认执行端到端闭环：采集/解析资料 -> 生成规格文档 -> 识别当前 C# 项目结构 -> 生成接口代码 -> 运行可行的编译/静态验证。
- 端到端闭环不得越过 SAP 候选确认门禁。只要 SAP Excel/SAP 文档中存在多个 sheet、多个接口、多个 FunctionName、多个表参数组，或用户没有明确指定唯一目标，先停在候选清单，不要生成完整规格、不要写代码、不要把多个候选自动全量执行。
- 只有用户明确说“只生成规格”“只要文档”“不要写代码”“先别改代码”时，才停在规格文档阶段。
- 如果生成规格过程中必须先让用户选择 SAP sheet/接口，用户选择后继续完成规格文档和代码落地，不要再次询问是否编码。
- 如果资料存在高风险不一致或缺失配置，仍然生成可编译的 DTO、Options、Client/Service、接口和必要注册；把真实调用所需配置和业务映射问题写入文档、代码注释或最终说明，不要因为待确认问题而停止编码。
- 请求/响应字段提取必须优先使用 MCP（OAS schema/example）、`xlsx` skill（用于 SAP Excel 前置解析）和用户本次提供资料；禁止通过获取 token 后执行业务接口来探测 response。

## 输入来源

支持以下来源，可单独使用，也可混合使用：

- SAP Excel：用户上传 Excel 或提供本机路径，并说明接口编码、FunctionName 或 sheet 名。
- SAP 统一 Invoke 示例：用户提供公司封装接口的请求示例，例如 `POST http://apidemo.isoftstone.com/PSA/SAP/v1/201/Invoke`，body 中包含 `FunctionName` 和表参数。
- 中台 iDataPlatform 页面采集：用户提供报表名称/搜索名称，要求从 `https://ipsapro.isoftstone.com/iDataPlatform/serviceHome/serviceSearch` 搜索接口、进入详情页并自动采集 `getApiByDeatail`/`getApiByDetail` 网络响应。
- curl：用户粘贴原始 curl，用于抽取 method、url、headers、query、body 和认证位置。
- Apifox MCP 接口定义：用户通过 Apifox MCP 提供接口 path/method/schema/example（OAS）信息，用于抽取请求和响应结构。
- 补充说明：业务规则、默认值、枚举含义、字段来源、项目偏好。

## 入口询问

当用户只说“执行 third-party-interface-spec”“执行技能”或类似请求，但没有同时提供原始资料、文件路径、curl 或明确的中台页面采集目标时，先询问用户选择本次资料类型，不要直接读取历史文件或沿用上次搜索条件。询问内容使用简短编号清单：

```text
这次要按哪类资料生成接口规格？
1. SAP 文档/Excel 或 SAP Invoke 示例
2. 中台接口“报表名称/搜索名称”（作为浏览器搜索条件）
3. 自定义 curl
4. Apifox MCP 接口定义（OAS/schema/example）
5. 混合资料（例如 SAP + 中台 + curl + Apifox MCP）
```

如果用户选择 2，继续询问“报表名称/搜索名称是什么”。该值就是访问 iDataPlatform 后输入到搜索框的浏览器搜索条件。若用户给出的是明确的报表名称/搜索名称，默认 API 分类、API 来源、排序方式均按平台默认值处理，并默认点击列表第一个结果的“详情”，不要再单独询问是否点击第一条；只有用户主动指定筛选条件、排序方式、结果序号或接口名称时，才按用户指定条件处理。

## 执行边界

- 每次用户要求“执行技能”或生成接口规格时，只基于本次对话中新提供、明确粘贴、上传、指定路径或明确要求通过浏览器采集的原始接口资料处理。
- 不要把工作区中已有的历史 `.md` 规格、草稿、上次输出或总结文件当作本次输入来源、事实依据、完成证明或参考资料。
- 如果本次没有提供 SAP Excel、SAP Invoke 示例、curl、明确可读取的原始资料路径或明确的中台页面采集任务，即使目录里存在历史 `.md` 文件，也应按“入口询问”先让用户选择资料类型并补充本次原始资料。
- 只有当用户在本次请求中明确要求读取、改写、对比或更新某个 `.md` 文件时，才处理该 `.md`；此时它只能作为用户指定的文档目标或待修订内容，不应替代原始接口资料。
- 如果用户说通过 Apifox MCP 生成规格和代码，先从本次可用的 Apifox MCP 工具/输出中读取 OAS path/schema/example；当前会话没有 Apifox MCP 工具时，要求用户提供 Apifox 导出的接口定义（或 curl/响应样例）作为补充。
- 判断“Apifox MCP 是否可用”必须基于**工具能力**而不是 MCP server 名称：即使 server 名不含 `apifox`，只要存在 `read_project_oas`、`refresh_project_oas`、`read_project_oas_ref_resources` 或等价 OAS/OpenAPI 工具，也必须按 Apifox MCP 流程执行，不得先要求用户补充资料。

## 来源组合规则

- 只提供一种来源时，生成单来源接口规格。例如只提供 SAP Excel/SAP Invoke、只提供中台 iDataPlatform 页面采集、或只提供 curl。
- 同时提供两种或两种以上来源时，默认生成同一份组合接口规格，不要拆成多份独立规格，除非用户明确要求拆分。
- 组合来源包括但不限于：SAP Excel + 中台页面采集、SAP Excel + curl、中台页面采集 + curl、Apifox MCP OAS + curl、SAP Excel + 中台页面采集 + curl + Apifox MCP OAS。
- 组合规格中保留每个来源各自的调用方式、请求字段、响应字段、示例请求和认证位置，并增加“资料关系与字段映射/差异”小节。
- 如果不同来源的接口名称、业务主题、字段或数据流看起来不一致，不要改成独立规格；在组合规格中写明“不一致/待确认”，并把影响放入“待确认问题”。
- 如果 SAP Excel 有多个候选 sheet/接口/FunctionName，先让用户选择要组合的 SAP sheet、接口或 FunctionName；用户确认前，中台页面采集、curl 等其他来源可以完成采集/解析，但不得生成最终组合规格或写代码。用户确认后，只把选中的 SAP 候选与其他已提供来源放在同一份规格中生成。

## 总体流程

1. 如果用户只要求执行技能但未提供本次原始资料或明确采集目标，先按“入口询问”让用户选择资料类型并补充资料；用户补充前不要生成规格。
2. 识别来源类型，不确定时先说明推断依据。
3. 判断输出形态：单来源资料生成单来源规格；两种或两种以上来源同时存在时生成同一份组合规格。
4. 如果来源包含 SAP Excel，先检查当前会话是否可用 `xlsx` skill：可用则先调用 `xlsx` 进行前置解析（sheet、合并单元格、表头区域、FunctionName 候选、表参数候选）；不可用则回退模型直读，并在结果中标注“使用兜底解析”。
5. 如果输入资料可识别出多个候选接口（尤其 SAP Excel 的多个 sheet、多个 FunctionName、多个表参数组），只列出候选清单并询问用户要处理哪个 sheet、接口或 FunctionName；用户确认前不要继续生成完整规格、不要落地代码、不要默认全量执行。
6. 如果用户要求中台页面采集但本次请求没有明确给出搜索关键词、接口名称或数据名称，先询问“本次要搜索哪个接口/数据”；如果用户给出明确的报表名称/搜索名称，按平台默认筛选条件搜索并默认点击列表第一个结果的“详情”，不要再询问是否点击第一条；如果用户主动给出 API 分类、API 来源、排序方式、结果序号或接口名称，则按用户指定条件处理。
7. 如果用户要求通过 Apifox MCP 处理接口，优先读取 MCP 返回的 path/method/schema/example；没有可用 MCP 工具时让用户粘贴业务 curl 或响应样例补充。禁止通过 token + 业务接口实时调用来获取 response。
8. 抽取接口身份：接口名称、来源系统、目标系统、接口编码、FunctionName、apiPath 或 URL。
9. 抽取调用方式：HTTP method、最终 URL、headers、auth、content type、请求 body 结构。
10. 抽取字段契约：请求字段、响应字段、字段名、中文名、类型、长度、小数位、必填、默认值、值域、说明。
11. 检查认证配置：在 C# 项目中查找 `appsettings.json`、`appsettings.*.json`、`Web.config`、`Web.config.xml` 或现有配置类；只检查测试环境配置，生产环境不用处理。
12. 统一类型和命名：保留第三方字段原名，必要时建议 C# 属性名和 JSON 映射。
13. 生成项目无关开发规格，结尾必须包含“给 AI 的 C# 开发指令”和“待确认问题”。
14. 除非用户明确要求只生成规格/只写文档，否则继续按现有代码结构实现 DTO、配置类、token 获取、Header 注入、接口 Client/Service、必要注册和调用入口；不需要真实调用第三方接口做联调测试。
15. 对矛盾或缺失信息不要猜死，写入“待确认问题”、代码注释或代码变更说明；仍要尽量落地可编译的框架代码。
16. 编码后运行可行的项目验证，例如 `dotnet build`；如果依赖、环境或沙箱导致无法验证，说明原因和未验证风险。

### Apifox MCP 检索强约束（接口名直查）

当用户已选择“Apifox MCP 接口定义”，并在本次消息给出一个或多个接口名（例如 `GetInterviewSummary`），必须按以下顺序执行，**不得跳步**：

1. 扫描本次会话可用的 MCP 工具描述，识别是否存在 OAS/OpenAPI 能力工具（如 `refresh_project_oas*`、`read_project_oas*`、`read_project_oas_ref_resources*`）。
2. 只要识别到上述任一工具，先调用 `refresh_project_oas*`（若存在）再调用 `read_project_oas*` 拉取最新 paths。
3. 按用户给出的接口名在 OAS 中检索，匹配范围至少包括：`paths` key、operation `summary`、`operationId`（若有）；匹配应大小写不敏感。
4. 命中后，如路径节点含 `$ref`，必须继续调用 `read_project_oas_ref_resources*` 拉取该接口完整定义（request/response/schema/example）。
5. 仅当“无可用 OAS 工具”或“已完成上述检索仍无命中”时，才允许向用户补充询问或请求额外资料。
6. 在完成步骤 1-4 前，不得因“server 名称不含 apifox”而直接判定不可用，也不得先让用户改走手工粘贴。

## SAP Excel 处理

读取 Excel 时先处理合并单元格语义，再识别表格列。常见列包括：输入/输出、字段/表类型、字段名、数据元素、字段类型、长度、小数位、必输、中文名称、值来源、说明。

### xlsx skill 前置解析（SAP Excel 优先）

- 若当前会话可用 `xlsx` skill，SAP Excel 必须先走 `xlsx` skill 做前置解析，再进入候选确认门禁。
- 前置解析至少覆盖：sheet 列表、合并单元格语义、表头/区域定位、FunctionName 候选、输入/输出表参数候选。
- `xlsx` skill 输出只作为结构化中间结果，不能跳过候选确认门禁，也不能直接生成最终规格或代码。
- 若 `xlsx` skill 不可用或执行失败，可回退模型直读 Excel，但必须在最终说明中标注“已使用兜底解析，准确性需复核”。

### SAP 候选确认门禁

- SAP Excel/SAP 文档的第一步只能是候选扫描：读取 workbook 的 sheet 名、sheet 标题、接口编码、FunctionName、表参数名和输入/输出区概况。
- 如果候选数量大于 1，或用户没有明确指定唯一 sheet/接口编码/FunctionName，必须暂停并询问用户选择。此时最终答复只输出候选清单和一个简短问题。
- 用户没有明确说“全部”“所有 sheet”“全量执行”时，不得把多个 SAP 候选都生成规格或代码。不能把“执行技能”“生成规格”“按这个 Excel 做”理解为“全部执行”。
- 用户只给出接口编码、FunctionName 或 sheet 名时，先用它筛选候选；如果仍匹配多个候选，继续询问，不要自行选择第一条或全部执行。
- 用户确认一个候选后，只处理该候选。用户明确确认“全部”后，才按每个 sheet/接口分别生成；同名 FunctionName 是否合并调用仍需写入待确认问题。

确认接口范围：

- 读取 SAP Excel 后，优先基于 `xlsx` skill 的前置解析结果扫描 workbook 的 sheet 名、sheet 标题、接口编码、FunctionName、表参数名和输入/输出区概况；未使用 `xlsx` 时再回退模型直读。
- 如果用户没有明确指定 sheet、接口编码或 FunctionName，或文档中存在多个 sheet/接口/FunctionName，必须先输出候选清单并询问用户要生成哪一个接口规格。
- 候选清单至少包含：序号、sheet 名、标题/接口名称、FunctionName、主要输入表参数、主要输出表参数。
- 用户确认前只做候选识别和必要的资料关系判断，不要全量展开字段表，不要生成最终规格，不要写代码，也不要把多个 SAP sheet 合并成一个规格。
- 只有用户明确选择“全部”时，才按每个 sheet/接口分别生成规格；同名 FunctionName 是否合并调用仍需写入待确认问题。

抽取规则：

- 从 sheet 标题、sheet 名或用户说明识别接口编码和 FunctionName，例如 `ZISS_HR_087`。
- 将“输入”区域整理成请求结构，将“输出”区域整理成响应结构。
- 对 SAP 表参数保留原表名，例如 `IT_9063`、`ET_RESULT`。
- 对结构字段和表字段分层展示，数组表参数要明确标注为 array。
- 必填列出现 `√`、`Y`、`是`、`必填` 等都视为必填。
- `说明` 或 `值来源` 中的枚举、固定值、默认值要提取到字段规则。

SAP 类型到 C# 的建议：

- `CHAR`、`NUMC`、`CLNT`、`LANG` -> `string`，尤其 `NUMC` 不要用数字类型，避免前导 0 丢失。
- `DATS` -> `DateTime` 或 `string`，按项目既有风格；规格中标注 SAP 原始格式通常为 `yyyyMMdd`。
- `TIMS` -> `TimeSpan` 或 `string`，按项目既有风格。
- `CURR`、`DEC`、`QUAN` -> `decimal`。
- `INT1`、`INT2`、`INT4` -> `int`。

## 认证与配置检查

只处理测试环境认证配置，生产环境不用检查、生成或提示。

### SAP PSA 网关

SAP 测试环境 InvokePath：

```text
http://apidemo.isoftstone.com/PSA/SAP/v1/201/Invoke
```

SAP 测试环境 token 地址：

```text
https://apidemo.isoftstone.com/ids/connect/token
```

SAP 调用规则：

- 在项目配置文件中查找是否已有上述测试环境 PSA InvokePath：`http://apidemo.isoftstone.com/PSA/SAP/v1/201/Invoke`。
- 配置文件范围包括 `appsettings.json`、`appsettings.*.json`、`Web.config`、`Web.config.xml`，以及项目已有配置类或 Options 绑定。
- 如果没有配置该 Invoke 地址、token 地址或相关 PSA 网关 token 参数，必须在规格或最终答复中提示：`增加获取psa网关token配置`。
- 如果已有配置，生成代码时先按项目配置读取 token 地址、client_id、client_secret、grant_type、scope 等测试环境参数，调用 token 接口获取 token，再调用 SAP Invoke。
- SAP Invoke 请求 Header 必须包含 `Authorization: Bearer {token}`。
- 不要把 `client_secret`、真实 token 或生产参数写死到技能、规格文档或生成代码；只生成配置读取和占位配置键。

### 中台 iDataPlatform

中台 token 地址：

```text
https://ipsapro.isoftstone.com/iDataPlatform/idss/sys/getToken
```

中台调用规则：

- 在项目配置文件中查找是否已有上述中台 token 地址。
- 如果没有配置该地址或相关中台 token 配置，必须在规格或最终答复中提示：`增加获取中台token配置`。
- 如果已有配置，生成代码时先读取 `appID`、`appSecret`，调用 token 接口获取 token，再调用中台业务接口。
- 中台业务接口请求 Header 必须包含 `X-Access-Token: {token}`。
- 中台接口响应外层固定按 `{ "message": null, "msg": "接口访问成功", "data": [] }` 建模，业务数据路径为 `data[]`。
- 不要把 `appSecret` 或真实 token 写死到技能、规格文档或生成代码；只生成配置读取和占位配置键。

### Apifox MCP URL 判定

当输入来自 Apifox MCP 的接口定义（OAS），或用户明确要求“通过 Apifox MCP 生成接口规格和编码”时，按接口 URL/path 判定网关类型：

- URL 包含 `apimarket`：认为是中台 iDataPlatform 接口。token 地址使用 `https://ipsapro.isoftstone.com/iDataPlatform/idss/sys/getToken`，业务请求 Header 使用 `X-Access-Token: {token}`，响应外层优先按 `{ "message": null, "msg": "接口访问成功", "data": [] }` 建模。
- URL 不包含 `apimarket`：认为是 PSA 网关接口。token 地址使用 `https://apidemo.isoftstone.com/ids/connect/token`，业务请求 Header 使用 `Authorization: Bearer {token}`。
- 如果 OAS 的 `servers` 为空，只能使用 path、method、header、request/response 示例生成“待补 baseUrl 的 curl 草稿”；不得自行猜测域名并做真实业务请求。
- 当 schema 为空但 example 存在时，用 example 建模字段；schema 与 example 都缺失时写入“待确认问题”，不要用实时接口调用补齐。

### 代码生成约束

- 优先复用项目已有 HTTP Client、SAP Client、认证、缓存、日志、异常处理、Options/config、依赖注入和结果包装风格。
- 如果项目已有 token 缓存机制，复用它；否则生成轻量缓存逻辑，按 token 返回过期时间或配置化默认 TTL 提前刷新。
- 日志中必须脱敏 `Authorization`、`X-Access-Token`、`client_secret`、`appSecret`。
- token 获取逻辑只用于“代码落地时的调用链设计”，不用于技能执行时探测第三方接口返回。
- 不需要真实访问 token 接口或业务接口做联调测试；但编码落地后需要运行可行的本地编译或静态验证。

默认落地清单：

- DTO：按规格生成 Request/Response DTO；SAP、中台、curl 等不同来源默认分别建模，不要强行合并不一致的业务模型。
- Options/config：按项目现有配置方式新增或扩展配置类；只写占位配置键，不写真实密钥。
- Token：SAP 优先复用项目已有 PSA token 获取；中台按 `appID/appSecret` 获取 `X-Access-Token`，没有现成机制时生成轻量缓存。
- Client/Service：生成接口和实现类，复用项目已有 HTTP helper、日志、异常和依赖注入风格。
- 注册：如果项目通过特性、扫描或扩展方法注册服务，按既有方式接入；不要额外引入不必要框架。
- 调用入口：若项目已有 Controller/API 风格，增加轻量 Controller 或应用服务入口，方便后续联调；如果用户明确不要暴露 API，则只生成 Service。
- 验证：编码后优先运行 `dotnet build` 或项目对应构建命令；无法验证时说明原因。

注意：上方“不需要真实访问 token 接口或业务接口做联调测试”包括“不要为了补 response 字段而实时调 token + 业务接口”；不表示可以跳过本地编译验证。

## SAP 统一 Invoke 处理

公司封装调用示例通常是：

```http
POST http://apidemo.isoftstone.com/PSA/SAP/v1/201/Invoke
Content-Type: application/json
```

```json
{
  "FunctionName": "ZISS_HR_087",
  "IT_9063": [
    {
      "pernr": "2401"
    }
  ]
}
```

规格中必须写明：

- `FunctionName` 固定值。
- 每个 SAP 表参数名和是否数组。
- 请求 body 顶层结构。
- 测试环境调用必须从项目配置读取 PSA 网关 token 配置，先获取 token，再在 SAP Invoke 请求 Header 中加入 `Authorization: Bearer {token}`。
- SAP Excel 中字段名与 JSON 示例字段大小写不一致时，保留第三方实际字段名，并把差异列入待确认。
- Excel、sheet、Invoke 示例中的 FunctionName 不一致时，列为高优先级待确认问题。

## 中台 iDataPlatform 处理

中台接口只接受用户提供报表名称/搜索名称作为页面搜索条件；不要要求用户手工提供 F12 response 或接口详情 JSON。页面采集时优先解析点击“详情”后自动捕获的 `getApiByDeatail`/`getApiByDetail` 网络响应，不优先解析 HTML。中台详情通常包含：

- `apiPath`：接口路径。
- `apiMethod`：请求方法，例如 `post`。
- `apiParams`：入参。
- `apiResults`：出参。

中台接口的业务调用响应外层按公司约定固定为：

```json
{
  "message": null,
  "msg": "接口访问成功",
  "data": []
}
```

生成规格时必须把 `apiResults` 的业务字段放在 `data[]` 路径下，示例响应也必须使用 `message`、`msg`、`data` 外层；不要使用 `records`、`rows` 或分页对象作为默认业务数据路径，除非本次资料明确给出了不同响应结构。

### 页面搜索/详情页采集

当用户要求从中台页面采集接口详情时，使用可用的 Browser 插件或 `browser:control-in-app-browser` 技能操作浏览器；不要要求用户手工粘贴 F12 response 或接口详情 JSON，除非用户明确要求改用手工资料。

如果当前工作区存在 `browser-session-crawler` 技能，优先运行其内置脚本，不要在工作区根目录临时生成新的 Playwright runner：

```bash
python .codex/skills/browser-session-crawler/scripts/capture-idata-api-detail.py "<报表名称/搜索名称>" --login-timeout 900 --response-timeout 120 --action-timeout 30 --keep-open-on-error
```

在 Windows 的 Codex 终端中，优先使用 `runpy` 启动同一个脚本，避免直接执行
`C:\Users\...\scripts\capture-idata-api-detail.py` 时被 unified exec 启动器拒绝，
确保下次执行技能时可以直接拉起可见浏览器：

```bash
python -X utf8 -c "import runpy, sys; sys.argv=[r'C:\Users\nealk\.codex\skills\browser-session-crawler\scripts\capture-idata-api-detail.py','<报表名称/搜索名称>','--login-timeout','900','--response-timeout','120','--action-timeout','30','--keep-open-on-error']; runpy.run_path(sys.argv[0], run_name='__main__')"
```

默认等待策略：登录等待至少 900 秒；详情网络响应等待至少 120 秒；Playwright 动作超时至少 30 秒。用户明确要求更长等待时，优先按用户要求调大这些参数，不要缩短。若浏览器打开后马上关闭，使用 `--keep-open-on-error` 保留失败现场，并读取 `.tmp-browser-session-crawler/output/error.json`、`error.png`、`error.txt` 判断失败步骤。

采集入口：

```text
https://ipsapro.isoftstone.com/iDataPlatform/serviceHome/serviceSearch
```

执行要求：

- 如果用户本次没有明确说明要搜索哪个接口/数据，先询问搜索目标；不要沿用历史会话、历史输出文件或上次采集任务中的搜索词。
- 如果用户只说接口名称、数据名称或搜索目标，但该值足以作为明确的报表名称/搜索名称，则按平台默认筛选条件搜索，并默认点击列表第一个结果的“详情”；不要再询问是否点击第一条。只有搜索目标含糊、像截图描述而不是可输入关键词，或用户要求特定筛选/排序/结果时，才继续询问搜索关键词、API 分类、API 来源、排序方式或结果序号。
- 如果打开采集入口后跳转到 `https://ipsapro.isoftstone.com/portal/` 或其他登录页，说明需要登录，打开/展示浏览器让用户在浏览器中手动输入账号密码；不要要求用户把密码粘贴到聊天中。用户确认登录完成后，再跳转回采集入口继续。
- 登录后在采集入口按用户本次给定的搜索目标和条件搜索；按用户指定的筛选项和排序方式过滤结果。
- 如果用户指定“列表页搜索结果第一个”，在确认当前搜索结果列表可见后点击第一条结果的“详情”；如果没有结果或第一条与目标明显不一致，停止并询问用户。
- 点击“详情”前先开始监听网络响应，点击后优先捕获 `getApiByDeatail` 接口返回；兼容可能的修正拼写 `getApiByDetail`。这个接口响应就是中台详情的原始来源，必须优先保存并解析。
- 只有在无法捕获 `getApiByDeatail`/`getApiByDetail` 响应时，才读取页面上展示的接口详情字段；如果页面展示也不足以抽取 `apiPath`、`apiMethod`、`apiParams`、`apiResults`，停止并说明本次页面自动采集失败，不要要求用户手工提供 F12 response/接口详情 JSON。
- 在最终规格的“资料来源”中写明页面采集路径、搜索关键词、筛选条件、点击的结果序号和详情页 URL。
- 页面采集得到的详情 JSON 仍按本节中台详情规则解析；如果只能采集可视页面内容，必须把“未取得 getApiByDeatail/getApiByDetail 原始网络响应”写入待确认问题。
- 生成规格文档并确认已落地后，清理中台临时采集目录和项目专用临时响应文件，不要保留 `.tmp-browser-session-crawler` 目录。例如删除 `.tmp-browser-session-crawler`、`.tmp_idata_project_master_response.json` 等临时文件；必要的采集来源信息应写入最终规格文档的“资料来源”或“待确认问题”，不要依赖临时目录作为交付证据。

最终 URL 规则：

```text
https://ipsapro.isoftstone.com/iDataPlatform/idss/apimarket/onlineApi/getData/{apiPath}
```

处理要求：

- `apiPath` 开头有 `/` 时，拼接时避免双斜杠。
- `apiMethod` 统一转大写；缺失时按 `POST` 处理，但写入待确认。若平台返回数字 `1`，按中台常见约定映射为 `POST`，并在备注中标注需确认。
- `apiParams`、`apiResults` 如果是 JSON 字符串，先二次解析。
- 字段如果有 `children`、`childList`、`properties`、`columns`、`items` 等嵌套，要递归展开。
- 必填字段名可能是 `required`、`isRequired`、`must`、`nullable`、`requiredFlag`，统一转成“是否必填”。
- 类型缺失但存在示例值时，可以推断类型，但必须标注“推断”。
- 中台响应结构固定写为 `{ "message": null, "msg": "接口访问成功", "data": [] }`；响应字段表的层级/路径统一使用 `data[]` 或 `data[].字段名`。
- 测试环境调用必须从项目配置读取中台 token 地址、`appID`、`appSecret`，先获取 token，再在业务接口请求 Header 中加入 `X-Access-Token: {token}`。

可先运行 `scripts/normalize-interface-source.js` 辅助解析：

```bash
node scripts/normalize-interface-source.js --type idata --input path/to/response.json
```

## curl 处理

从 curl 中抽取：

- HTTP method。
- URL、query 参数。
- headers。
- content type。
- auth/token/cookie 所在位置，敏感值脱敏。
- body JSON 或表单参数。
- 示例请求。

可先运行：

```bash
node scripts/normalize-interface-source.js --type curl --input path/to/curl.txt
```

curl 通常缺少中文名、必填、业务说明；这些缺口要写入待确认问题，或结合其他资料补充。

## Apifox MCP OAS 处理

当用户提供 Apifox MCP 的接口资料，或明确说要“通过 Apifox MCP 生成接口规范和编码”时：

1. 优先调用本次会话中可用的 Apifox MCP/OAS 工具读取接口 path、method、requestBody schema/example、response schema/example；当用户提供了接口名时，必须先执行“Apifox MCP 检索强约束（接口名直查）”再进入字段抽取。
2. 字段抽取顺序：`schema` > `example`；若 `schema` 为空但 `example` 存在，则按 `example` 递归展开字段。
3. 若请求或响应的 `schema` 与 `example` 均缺失，写入“待确认问题”，并给出可编译 DTO 骨架（最小字段或占位对象）。
4. 不执行实时业务请求；不得通过 token 接口 + 业务接口探测返回值。
5. 规格文档仅保留脱敏后的来源说明、URL/path、字段结构、token 地址和 Header 名，不保留真实 token、secret 或 cookie。

## 输出格式

使用 `references/spec-template.md` 的结构输出。不要只写自然语言说明，必须包含可供开发使用的字段表和 C# 开发指令。

输出时遵守：

- 单来源资料按模板直接输出。
- 两种或两种以上来源同时提供时，仍输出一份组合规格；在模板结构下分别列出 SAP、中台、curl 等各来源的调用契约，并增加资料关系、字段映射、差异和待确认问题。
- 规格中必须包含“认证与配置”章节，写明配置文件检查结果、缺失配置提示、token 获取方式和 Header 注入方式。
- 字段表用 Markdown 表格。
- 请求字段和响应字段分开。
- 中台 iDataPlatform 响应字段必须放在 `data[]` 下；规范里的业务数据数组不要命名为 `records`。
- 保留第三方字段原名，C# 属性名只作为建议。
- 对接口资料之间的矛盾明确标注。
- 不确定信息放入“待确认问题”，不要隐式补全。
- 规格应能复制到另一个 C# 项目中继续使用。
