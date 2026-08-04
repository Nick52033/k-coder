---
name: submit-zentao-bug
description: 通过 Zentao MCP 创建包含文字与正文截图的禅道 Bug，并指派给指定处理人；支持从用户提供的 AIchat 文档或禅道链接补全产品、项目、迭代和版本。用户说“提 Bug”“创建缺陷”“提交禅道”“给某人提 Bug”，或提供 Bug 描述、截图、AIchat 文档、禅道链接并要求录入禅道时使用。
triggers:
  - submit zentao bug
  - 提交禅道
risk: external
enabled: true
---

# 提交禅道 Bug

通过 Zentao MCP 收集、预览、提交并回读验证 Bug。创建 Bug 是外部写操作，必须先取得用户明确确认。

## 读取信息来源

本技能中的 `AIchat 文档` 不是禅道内置对象，而是用户当前提供、当前会话已打开，或当前工作区中明确关联本次需求的 `*AIchat*.md` 需求沟通文档。文档通常在“2.2 禅道信息”章节记录产品名称、`product_id`、迭代名称、`execution_id` 或 Bug 创建路径链接。

不要把 AIchat 文档当成必需依赖。仅在当前任务能够明确定位到相关文档时读取；找不到文档或文档没有禅道信息时，回退到用户提供的禅道链接或手工信息。

按以下优先级取得禅道归属：

1. 用户在当前消息或后续确认中明确给出的产品、项目、迭代或链接
2. 用户当前提供或明确指定的 AIchat 文档中的 Bug 创建路径链接
3. 同一 AIchat 文档“2.2 禅道信息”中的结构化字段
4. 本技能定义的可选默认值

不得沿用其他需求、历史会话或名称相似文档中的禅道信息。两个来源冲突时展示来源和值，让用户确认；不得自行选择。

## 收集必填信息

提交前取得：

- Bug 标题
- 故障现象或当前结果
- 预期结果
- 指派人的禅道域账号
- 禅道迭代链接，或准确的产品名称/产品 ID

复现步骤、环境、测试数据、关键词、模块 ID、需求 ID、任务 ID、操作系统、浏览器和截止日期可选。用户未指定时使用：

- `severity = 3`
- `pri = 2`
- `type = codeerror`
- 无可用版本时 `opened_build = trunk`

缺少必填信息时继续询问。不得猜测产品、指派账号或预期结果。

## 处理正文截图

将截图作为 `steps` 富文本正文的一部分，而不是只分析截图或仅列为附件。

用户提供截图时：

1. 读取截图并提取页面、错误提示、输入值和当前结果
2. 保留用户指定的文字、截图、文字顺序
3. 为每张截图取得稳定图片引用和简短说明
4. 在对应文字段落后插入 `<img>` 标签
5. 在提交预览中显示每张截图的插入位置和地址

支持以下图片引用：

- 禅道图片地址：`https://zentao.isscloud.com/file-read-25133.png`
- 禅道文件引用：`{25133.png}`，将其规范化为对应的 `file-read` 地址
- 禅道服务可访问的 HTTPS 图片地址

聊天图片只有像素内容、没有稳定 URL 或禅道文件引用时，继续询问可访问地址。不得静默删除截图，不得将本地路径、`file://` 地址、Base64 或临时会话地址写入正文。用户明确同意仅提交文字后，才允许去掉图片继续提交。

图片含密码、Token、身份证号、手机号或其他敏感数据时，暂停提交并要求用户先脱敏。

按以下结构生成 HTML，所有普通文本先进行 HTML 转义：

```html
<h3>【故障现象描述】</h3>
<p>{故障现象文字}</p>
<p><img src="{截图1地址}" alt="{截图1说明}" /></p>

<h3>【环境信息】</h3>
<p>{环境信息}</p>

<h3>【故障复现步骤】</h3>
<p>1. {步骤一}</p>
<p>2. {步骤二}</p>
<p><img src="{截图2地址}" alt="{截图2说明}" /></p>

<h3>【当前结果】</h3>
<p>{当前结果}</p>

<h3>【预期结果】</h3>
<p>{预期结果}</p>
```

不得编造截图说明、复现步骤或业务规则。信息缺失时标记“待补充”。

## 解析产品、项目、迭代和版本

从下列 URL 解析 `execution_id`，并在扩展格式中解析候选 `product_id`：

- `execution-bug-{executionId}.html`
- `execution-bug-{executionId}-{productId}-其他参数.html`
- `execution-task-{executionId}.html`
- `execution-view-{executionId}.html`
- `project-bug-{projectId}.html`

取得 `execution_id` 后调用 `mcp__Zentao__zentao_list_builds_by_execution`：

1. 选择与候选 `product_id` 匹配的版本
2. 从版本的 `product` 取得 `product_id`
3. 从版本的 `project` 取得 `project_id`
4. 使用版本 `id` 作为 `opened_build`
5. 使用 `execution_id` 作为 `execution`
6. 多个版本匹配时让用户选择

只有 `project_id` 时，调用 `mcp__Zentao__zentao_list_executions` 查询项目迭代。只有一个有效迭代时将其作为候选；存在多个迭代时展示名称、ID 和状态，让用户选择。选择后再查询版本。

仅有产品名称时调用 `mcp__Zentao__zentao_list_products`，按完全匹配、包含匹配的顺序查找。多个结果时必须让用户确认。

不得仅依据 URL 文字猜测项目或版本。没有迭代时可省略 `execution` 和 `project_id`。

## 检查指派人和重复 Bug

优先要求用户提供域账号，例如 `mhke`。用户只提供中文姓名时，可使用以下已知映射生成候选：

| 姓名 | 域账号 |
|---|---|
| 王慧 | `huiwangdi` |
| 王华东 | `hdwangh` |
| 柳祚霖 | `zlliuch` |
| 阮俊 | `junruanc` |
| 许敢 | `ganxuc` |
| 刘丽姝 | `lsliu` |
| 昌雅晶 | `yjchang` |
| 李晓仲 | `xzlid` |
| 柯孟浩 | `mhke` |

映射只作为候选，不是永久人员主数据。映射不存在或有重名时，从已有 Bug 的 `assignedTo.account` 与 `assignedTo.realname` 精确反查；无法唯一确认时继续询问域账号。不得按拼音或相似姓名猜测账号。

调用 `mcp__Zentao__zentao_search_user_bugs` 查询指定处理人的相关 Bug。发现同一产品下标题相同或高度相似的未解决 Bug 时，展示 Bug ID、标题和状态，让用户决定是否继续。不得修改或覆盖已有 Bug。

## 提交前确认

调用创建接口前展示：

- 产品、项目、迭代和版本
- 标题、指派人、严重程度、优先级和类型
- 禅道归属和指派账号的信息来源
- 完整 HTML 正文预览
- 图片数量、地址、说明及插入位置
- 未关联的可选字段

只有用户明确回复“确认提交”后，才调用创建接口。用户修改任何内容后，重新展示最终预览并再次确认。

## 创建并验证

调用 `mcp__Zentao__zentao_create_bug`，映射已确认的 `product_id`、`project_id`、`execution`、`opened_build`、`title`、`assigned_to`、`severity`、`pri`、`type`、`steps`、`module`、`story`、`task` 和 `keywords`。没有值的可选字段不要传入。

`module` 必须是数字模块 ID。用户无法提供时不关联模块，不得使用模块名代替。

创建成功后，使用返回的 Bug ID 调用 `mcp__Zentao__zentao_get_bug`，核对标题、产品、状态、指派人、严重程度、优先级以及 `steps` 中每个 `<img>` 地址。报告 Bug ID 和未成功关联的字段。

创建请求超时或返回结果不明确时，先查询是否已经生成相同标题的 Bug，不得直接重复提交。
