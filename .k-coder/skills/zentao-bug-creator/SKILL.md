---
name: "zentao-bug-creator"
description: "在禅道上创建Bug，自动匹配产品、指派处理人、关联迭代。当用户说'创建bug'、'提bug'、'禅道提缺陷'时使用。"
triggers:
  - 创建 bug
  - 提 bug
  - 禅道提缺陷
risk: external
enabled: true
---

# 禅道Bug创建

在禅道上创建Bug，自动完成产品匹配、处理人指派、迭代关联。

## 触发条件

- 用户说"创建bug"、"提bug"、"禅道提缺陷"、"提单"
- 用户提供了bug描述，需要录入禅道

## 执行流程

### 0. 从AIchat读取禅道信息（优先）

在收集Bug信息前，先检查当前会话中是否已打开需求相关的AIchat文档，从中读取禅道信息：

#### 0.1 从Bug创建路径链接提取（最高优先级）

如果AIchat中提供了Bug创建路径的链接，**必须优先使用该链接**提取迭代信息：

1. **识别链接格式**：AIchat中可能出现的Bug创建路径链接格式：
   - `https://zentao.isscloud.com/execution-bug-{执行ID}.html` → 提取执行ID
   - `https://zentao.isscloud.com/execution-task-{执行ID}.html` → 提取执行ID
   - `https://zentao.isscloud.com/execution-view-{执行ID}.html` → 提取执行ID
   - `https://zentao.isscloud.com/project-bug-{项目ID}.html` → 提取项目ID
2. **提取执行ID**：从链接中解析出 `execution_id`
3. **自动查询关联信息**：调用 `mcp_Zentao_zentao_list_builds_by_execution(execution_id)` 获取：
   - `project_id`：从版本列表的 `project` 字段获取
   - `opened_build`：从版本列表中选择对应产品的版本ID
   - 关联产品列表：从版本列表的 `product` 字段获取
4. **自动填充**：将提取到的执行ID、项目ID、版本ID自动填入Bug创建参数

**示例**：
- AIchat中链接：`https://zentao.isscloud.com/execution-bug-399.html`
- 提取：`execution_id = 399`
- 调用 `list_builds_by_execution(399)` → 获取 `project_id=398`、版本列表
- 如果版本列表有多个产品对应的版本，需让用户确认产品

#### 0.2 从AIchat文本读取禅道信息

如果AIchat中没有Bug创建路径链接，则从AIchat文本中读取：

1. **检查AIchat文档**：查找当前会话上下文中是否包含AIchat文档（`*AIchat*.md`）
2. **提取禅道信息**：从AIchat的「2.2 禅道信息」章节读取：
   - 产品名称和 `product_id`
   - 迭代/执行 `execution_id`
3. **自动填充**：将读取到的产品ID和执行ID自动填入Bug创建参数，无需再询问用户
4. **未找到时回退**：如果当前会话没有AIchat文档，或AIchat中没有禅道信息，再按步骤1~4询问用户

**优先级**：Bug创建路径链接 > AIchat文本禅道信息 > 用户手动提供 > 默认值

### 1. 收集Bug信息

从用户输入中提取以下信息，缺失则主动询问：

#### 1.1 图片Bug分析（优先）

如果用户提供了bug截图（图片文件路径），**必须先分析图片内容**：

1. **读取图片**：使用 Read 工具读取图片文件（需多模态大模型支持）
2. **分析图片内容**：从截图中提取：
   - Bug现象（页面显示了什么错误/异常）
   - 错误提示信息（如有）
   - 涉及的表单字段、按钮、菜单等UI元素
   - 输入值与预期值的差异
3. **生成Bug描述**：根据图片分析结果自动填充：
   - Bug标题：从图片中提炼核心问题
   - 故障现象描述：图片中看到的问题
   - 当前结果：图片显示的实际状态
   - 预期结果：根据业务逻辑推断的正确状态
4. **图片无法读取时**：如果当前模型不支持图片理解，提示用户用文字描述bug内容

> **前提条件**：图片分析功能依赖多模态大模型（如GLM-4V、GPT-4V等），非多模态模型无法读取图片。

#### 1.2 信息收集表

| 字段 | 必填 | 说明 | 默认值 |
|------|------|------|--------|
| 产品名称 | 是 | 禅道产品名，必须准确（系统不会根据执行ID自动纠正产品） | - |
| Bug标题 | 是 | 简明描述问题 | - |
| 严重程度 | 否 | 1-4（1最高） | 3 |
| 优先级 | 否 | 1-4（1最高） | 2 |
| Bug类型 | 否 | codeerror/config/install/security/performance/standard/automation/designdefect/others | codeerror |
| 复现步骤 | 否 | HTML格式 | 模板结构 |
| 指派给 | **是** | 处理人账号或姓名，**必填**，缺失时必须询问用户 | 无默认值 |
| 所属模块 | 否 | 模块ID（非模块名），禅道MCP无法查询模块列表，需用户提供 | 不关联 |
| 迭代/执行 | 否 | 迭代ID | 不关联 |
| 关键词 | 否 | 逗号分隔 | 空 |

### 2. 匹配产品

**优先从AIchat获取**：如果步骤0已从AIchat读取到 `product_id`，直接使用，跳过本步骤。

否则，调用 `mcp_Zentao_zentao_list_products` 获取产品列表，根据用户描述模糊匹配产品名，获取 `product_id`。

匹配规则：
- 精确匹配优先
- 包含匹配次之（如用户说"人事运营"，匹配"人事运营"产品）
- 多个匹配时让用户确认

### 3. 匹配指派人

**指派给为必填字段**，如果用户未提供处理人信息，必须使用 AskUserQuestion 询问用户。

匹配流程：

1. **用户提供姓名**：在已知映射表中查找对应账号
2. **用户提供域账号**：直接使用（如 huiwangdi）
3. **重名处理**：如果映射表中存在重名或匹配到多个结果，提示用户提供域账号以精确匹配
4. **未知人员**：不在映射表中时，通过方法一反查，或直接询问用户域账号

**方法一**：通过已有Bug反查
- 调用 `mcp_Zentao_zentao_search_user_bugs` 搜索可能的关键词
- 从返回的Bug中提取 `assignedTo.account` 和 `assignedTo.realname`

**方法二**：通过用户ID查询
- 调用 `mcp_Zentao_zentao_get_user` 尝试已知ID

**常见账号映射**（根据历史数据积累）：

| 姓名 | 账号 |
|------|------|
| 王慧 | huiwangdi |
| 王华东 | hdwangh |
| 柳祚霖 | zlliuch |
| 阮俊 | junruanc |
| 许敢 | ganxuc |
| 刘丽姝 | lsliu |
| 昌雅晶 | yjchang |
| 李晓仲 | xzlid |

> 注意：此映射表不完整，遇到未知人员时需通过方法一反查或询问域账号。

### 4. 关联迭代

**优先从AIchat获取**：如果步骤0已从AIchat读取到 `execution_id`，直接使用，跳过本步骤。

否则，调用 `mcp_Zentao_zentao_list_executions` 获取迭代列表，根据项目ID或迭代名称匹配。如果接口返回空，则：

1. 询问用户是否知道执行ID（迭代ID）
2. 如果用户不知道，提示用户从禅道网页URL中获取：
   - 执行页面：`/execution-task-{执行ID}.html` → 执行ID
   - 迭代视图：`/execution-view-{执行ID}.html` → 执行ID
3. 获取到执行ID后，在创建Bug时传入 `execution` 参数

**验证结果**（2026-05-12）：
- `list_executions` 接口已修复，可正常返回迭代列表 ✅
- `execution` 参数可成功关联迭代 ✅

### 4.1 关联项目（project_id）

**关联迭代后，自动查询并关联项目**：

1. 获取到 `execution_id` 后，调用 `mcp_Zentao_zentao_list_builds_by_execution` 查询该迭代下的版本列表
2. 从返回的版本信息中提取 `project` 字段，即为 `project_id`
3. 将 `project_id` 传入 `create_bug` 的 `project_id` 参数

**验证结果**（2026-05-12）：
- `project_id` 参数可成功关联项目 ✅
- 通过 `list_builds_by_execution` 返回的版本信息中包含 `project` 字段 ✅
- 返回 JSON 中不显示 project 字段，但禅道页面上已关联 ✅

### 4.2 关联版本（opened_build）

**关联迭代后，自动查询并关联版本**：

1. 获取到 `execution_id` 后，调用 `mcp_Zentao_zentao_list_builds_by_execution` 查询该迭代下的版本列表
2. 如果返回版本列表：
   - **只有一个版本**：自动使用该版本的ID作为 `opened_build` 参数
   - **多个版本**：让用户确认使用哪个版本
   - **无版本**：`opened_build` 使用默认值 "trunk"
3. 将版本ID传入 `create_bug` 的 `opened_build` 参数

**验证结果**（2026-05-12）：
- `list_builds_by_execution` 接口已修复，可正常返回版本列表 ✅
- `opened_build` 参数可成功关联版本 ✅

### 5. 构建Bug步骤

按以下HTML模板构建复现步骤：

```html
<h3>【故障现象描述】</h3>
<p>{现象描述}</p>

<h3>【故障复现步骤】</h3>
<p>1. {步骤1}</p>
<p>2. {步骤2}</p>
<p>3. {步骤3}</p>

<h3>【当前结果】</h3>
<p>{实际结果}</p>

<h3>【预期结果】</h3>
<p>{期望结果}</p>

<h3>【建议修复】</h3>
<p>{修复建议}</p>

<h3>【关联文件】</h3>
<p>{相关代码文件}</p>
```

### 6. 创建Bug

调用 `mcp_Zentao_zentao_create_bug`，参数映射：

| 禅道参数 | 来源 |
|----------|------|
| product_id | 步骤0 AIchat读取 或 步骤2匹配的产品ID |
| project_id | 步骤4.1通过版本列表自动获取的项目ID |
| title | 用户提供的Bug标题 |
| severity | 严重程度（1-4） |
| pri | 优先级（1-4） |
| type | Bug类型 |
| steps | 步骤5构建的HTML |
| assigned_to | 步骤3匹配的账号 |
| execution | 步骤0 AIchat读取 或 步骤4获取的迭代ID（如有） |
| keywords | 关键词 |
| opened_build | 步骤4.2自动查询的版本ID，无版本时默认"trunk" |

### 7. 后续操作

创建成功后：
1. 输出Bug ID、标题、指派人、状态、关联项目
2. 如果未关联迭代，提醒用户手动关联
3. 如果未指派处理人，提醒用户手动指派

## 注意事项

- **指派给为必填字段**，用户未提供时必须询问，不可跳过
- 重名人员必须让用户提供域账号以精确匹配
- 用户提供了bug图片时，优先从图片分析bug内容（需多模态模型支持）
- 非多模态模型无法读取图片时，应提示用户用文字描述
- `list_executions` 接口已修复，可正常返回迭代列表
- `list_builds_by_execution` 接口已修复，可正常返回版本列表
- 关联迭代后自动查询版本并关联 `opened_build`
- 关联迭代后自动通过版本列表获取 `project_id` 并关联项目
- `project_id` 参数可成功关联项目，返回JSON不显示但禅道页面已关联
- **禅道MCP无法查询模块列表**：`module` 参数需要模块ID（非模块名），需用户提供，MCP无接口查询
- `product_id` 必须准确，系统不会根据执行ID自动纠正产品归属
- 禅道MCP无法查询用户列表，指派人需通过反查或已知映射获取
- Bug步骤支持HTML格式，可使用h3/p/img等标签
- severity和pri都是1-4，1最高4最低
- 创建Bug后无法自动关联需求（story），需手动操作
