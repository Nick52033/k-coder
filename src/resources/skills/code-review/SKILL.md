---
name: code-review
description: 代码评审流程规范。对本地未提交代码 diff 或临时 diff 文件进行代码质量检查；适用于需要检查正确性、回归风险、测试缺口、接口变更影响和 .NET 编译结果的场景。
triggers:
  - code review
  - review code
  - 代码评审
risk: read
enabled: true
---

# 代码评审流程

## 评审目标

- 发现缺陷和潜在问题
- 确保代码质量和可维护性
- 知识共享和团队成长
- 保持代码风格一致性

## 使用方式

```bash
# 方式1: 评审当前目录的本地未提交修改
/code-review

# 方式2: 评审指定目录的本地未提交修改
/code-review /path/to/project

# 方式3: 评审临时 diff 文件
/code-review /tmp/local_diff.txt
```

## 评审流程

### 0. 获取本地未提交 diff（重要！）

**关键规则**：
- 默认评审对象是 **本地未提交修改**
- 优先通过 git 获取 diff：
  - `git diff --cached`：已暂存但未提交的改动
  - `git diff`：未暂存的工作区改动
- 如果同时存在 staged 和 unstaged 改动，需要一起评审，并在报告中说明范围
- 如果没有本地未提交 diff，明确说明“当前工作区没有可评审的本地修改”
- 如果用户传入临时 diff 文件，则直接评审该 diff 文件

### 0.5. 编译检查（重要！）

- 在本地代码评审场景下，默认执行一次与当前改动最相关的编译检查
- .NET 项目优先执行本 skill 自带脚本 `scripts/dotnet_review.py`
- 优先编译最小相关项目；如果用户要求更完整验证，或改动影响面较大，再编译 `.sln`
- 如果当前工作区代码无法通过编译，应在评审报告中明确列为 `P0`
- 即使编译失败看起来可能是历史问题，也不能跳过，必须在报告里明确说明“当前工作区编译失败”
- 报告中必须说明脚本是否执行、编译目标、是否通过、关键错误摘要

建议命令：
```bash
# 默认：自动选择根目录 .sln；没有 .sln 时选择 csproj
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>"

# 指定项目
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>" --project "path\to\affected.csproj"

# 指定解决方案
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>" --solution "path\to\solution.sln"

# 需要同时跑测试时
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>" --run-tests
```

如果脚本无法执行，可退回：
```bash
dotnet build path/to/affected.csproj

# 或
dotnet build path/to/solution.sln
```

### 1. 理解变更范围

首先明确评审范围：
- [ ] 是当前仓库本地未提交 diff，还是指定临时 diff 文件？
- [ ] 涉及哪些文件和模块？
- [ ] 变更的主要目的是什么？
- [ ] 改动是否包含 staged / unstaged 两类状态？

建议先执行：
```bash
git status --short
git diff --cached
git diff
```

.NET 项目继续执行：
```bash
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>"
```

### 1.5. 评审原则（重要）

**避免基于假设的评审**：
- 只评审**可见的代码差异**
- 不要假设 diff 之外的问题
- 不要臆测"可能"存在的 bug
- 如果编译或测试已经证明当前工作区存在问题，可以纳入评审

**错误示例**：
```vue
<!-- diff 显示 -->
<div v-if="detail.is_contract">
  <span>{{contract.code}}</span>
</div>

<!-- 错误评审：假设 contract 可能为 null -->
"问题：contract 对象可能为 null，会导致报错"

<!-- 正确评审：基于原代码分析 -->
"原代码没有 v-if 时能正常运行，说明 contract 要么一定有值，
要么原代码已有数据处理（不在本次改动范围）"
```

**分支名与改动的匹配**：
- 不将"分支名与改动不完全匹配"列为问题
- 分支名通常是开发过程中的粗略描述，与最终改动有差异是正常情况
- 除非分支名严重误导（如写"fix bug"实际是"新功能"），否则不必提及

### 2. 评审维度

| 维度 | 检查点 |
|------|--------|
| **正确性** | 逻辑正确、边界处理、异常处理、业务规则 |
| **可读性** | 命名规范、注释必要、结构清晰 |
| **可维护性** | 单一职责、低耦合、可测试、遵循规范 |
| **性能** | 异步正确使用、避免阻塞、资源管理 |
| **安全** | 输入验证、权限检查、敏感信息保护 |
| **兼容性** | API 兼容、向后兼容、破坏性变更 |
| **测试** | 是否包含对应测试、关键路径覆盖、接口变更是否同步更新/新增测试 |
| **编译** | 当前工作区是否可以正常编译 |

### 3. 问题分级

| 级别 | 图标 | 说明 | 示例 |
|------|------|------|------|
| P0 | 🔴 | 必须修复，存在严重风险 | 编译失败、安全漏洞、数据丢失、核心流程不可用 |
| P1 | 🟡 | 应该修复，影响质量 | 逻辑错误、性能问题、明显回归、关键测试缺失 |
| P2 | 🟢 | 建议修复，改进空间 | 代码风格、命名规范、一般测试缺口 |
| P3 | 🔵 | 可选优化 | 架构改进、更好的实现方式 |

### 4. 评审报告格式

**输出要求（强制）**：
- 评审报告必须显式包含 `### 🔴 严重问题 (P0)`、`### 🟡 中等问题 (P1)`、`### 🟢 优化建议 (P2)`、`### 🔵 可选优化 (P3)` 四个小节
- 即使某个级别没有问题，也必须保留该小节，并明确写 `无`
- 不要只写“发现”或“问题列表”后省略分级展示
- 问题应优先按严重级别排序，再在同级别内按重要性排序
- 每个问题尽量带上文件路径和定位信息
- 报告中应说明编译检查是否执行、执行了哪个项目/解决方案、结果是否通过
- 报告中应说明 `dotnet_review.py` 是否执行；如果执行失败，要说明失败原因
- 如果修改了接口、public 方法、DTO、controller action 或流程分支，报告中应说明是否发现对应测试变更

**本地未提交 diff 报告格式**：
```markdown
# 代码评审报告

## 一、改动概览

**评审范围**: 本地未提交 diff
**状态**: staged / unstaged / staged + unstaged
**改动文件**: M 个
**编译检查**: 通过 / 失败 / 未执行
**脚本检查**: dotnet_review.py 已执行 / 未执行
**测试检查**: 已执行 / 未执行 / 未发现对应测试变更

## 二、详细评审

### 🔴 严重问题 (P0)
...

### 🟡 中等问题 (P1)
...

### 🟢 优化建议 (P2)
...

### 🔵 可选优化 (P3)
...

## 三、评审总结

### 必须修复
| 优先级 | 问题 | 文件 |
|--------|------|------|
| P0 | xxx | file.cs |

**总体评价**: ❌不通过 / ⚠️有条件通过 / ✅通过

## 四、残余风险 / 测试缺口

[如有，说明仍需关注的测试或验证点]
```

**无明确问题时的报告格式**：
```markdown
# 代码评审报告

## 一、改动概览

**评审范围**: 本地未提交 diff
**状态**: staged / unstaged / staged + unstaged
**改动文件**: M 个
**编译检查**: 通过 / 失败 / 未执行
**脚本检查**: dotnet_review.py 已执行 / 未执行
**测试检查**: 已执行 / 未执行 / 未发现对应测试变更

## 二、详细评审

### 🔴 严重问题 (P0)
无

### 🟡 中等问题 (P1)
无

### 🟢 优化建议 (P2)
无

### 🔵 可选优化 (P3)
无

## 三、评审总结

未发现明确缺陷。

**总体评价**: ✅通过

## 四、残余风险 / 测试缺口

[如有，说明仍需关注的测试或验证点]
```

## 通用检查清单

### 代码正确性
- [ ] 代码是否符合需求？
- [ ] 是否有明显的逻辑错误？
- [ ] 边界条件是否处理？
- [ ] 错误处理是否完善？
- [ ] 是否有安全风险？

### 代码质量
- [ ] 函数是否过长？（建议 < 50 行）
- [ ] 嵌套是否过深？（建议 < 3 层）
- [ ] 是否有重复代码？
- [ ] 命名是否清晰？
- [ ] 注释是否必要且准确？

### 测试相关
- [ ] 是否有单元测试？
- [ ] 测试覆盖关键路径？
- [ ] 测试用例是否合理？
- [ ] 如果修改了接口/方法签名/返回结构，是否同步修改或新增对应测试？
- [ ] 如果改动影响流程路由、条件分支、仓储查询，是否至少覆盖一个成功场景和一个边界/兜底场景？
- [ ] 当前工作区是否可以正常编译？

---

## .NET/C# 代码检查清单

### 编译与可运行性

- [ ] 先执行 `dotnet_review.py` 验证当前改动至少能通过编译
- [ ] 如果脚本不可用，再执行 `dotnet build` 作为兜底
- [ ] 如果编译失败，不要继续给“通过”结论
- [ ] 编译失败应至少列为 `P0`
- [ ] 若只能编译局部项目，应在报告中说明编译范围

### 异步编程

- [ ] ❌禁止使用 `.Result` 或 `.Wait()` 等待异步方法
- [ ] ❌禁止使用 `.GetAwaiter().GetResult()` 同步等待
- [ ] ✅异步方法应使用 `async/await` 到底
- [ ] ✅长时间运行的方法应支持 `CancellationToken`

**常见问题**:
```csharp
// ❌错误：可能导致死锁
var result = someService.GetData().Result;

// ❌错误：线程阻塞
var result = someService.GetData().GetAwaiter().GetResult();

// ✅正确
var result = await someService.GetDataAsync();
```

### ASP.NET Core 控制器

- [ ] ❌路由不应以 `/` 开头（会覆盖控制器级别前缀）
- [ ] ✅使用 `[Route("api/[controller]")]` 统一前缀
- [ ] ✅RESTful API 使用正确的 HTTP 方法

**常见问题**:
```csharp
// ❌错误：路由覆盖前缀
[HttpPost("/users/{id}/roles")]

// ✅正确
[Route("api/[controller]")]
public class UsersController : ControllerBase
{
    [HttpPost("{id}/roles")]  // 完整路由: api/users/{id}/roles
}
```

### 命名规范

- [ ] ✅类名使用 PascalCase（`UserService`）
- [ ] ✅方法名使用 PascalCase（`GetUserById`）
- [ ] ✅属性名使用 PascalCase（`UserName`）
- [ ] ✅参数名使用 camelCase（`userId`）
- [ ] ❌禁止使用下划线命名（`user_name`）

### 常量和魔法数字

- [ ] ❌禁止在方法内硬编码字符串/数字
- [ ] ✅常量应提取为模块级常量或配置项

**常见问题**:
```csharp
// ❌错误：硬编码在方法内
public void Process()
{
    int months = 6;  // 魔法数字
}

// ✅正确
private const int DEFAULT_QUERY_MONTHS = 6;  // 模块级常量

public void Process()
{
    int months = ConfigSettings.QueryMonths.IsInt()
        ? ConfigSettings.QueryMonths.ToInt()
        : DEFAULT_QUERY_MONTHS;
}
```

### 依赖注入

- [ ] ✅ 遵循"万物皆注入"原则
- [ ] ❌ 禁止在服务中 `new` 依赖项
- [ ] ✅ 使用构造函数注入

### 错误处理和日志

- [ ] ✅ 异常应被捕获并记录
- [ ] ✅ 日志应包含关键参数（结构化日志）
- [ ] ❌ 禁止吞掉异常（空 catch 块）

### ViewModel/DTO

- [ ] ✅ 使用 DTO 接收外部数据，而非直接使用 ViewModel
- [ ] ✅ 属性命名应一致（PascalCase 或 camelCase）
- [ ] ✅ 应进行数据验证

### 测试用例评审

- [ ] 修改接口定义（如 `interface`、public service/manager 方法、controller action、DTO 返回结构）时，检查是否包含对应 Test 用例变更
- [ ] 新增 public 方法时，检查是否至少有基础成功路径测试，或在评审结论中明确指出测试缺口
- [ ] 修改分支逻辑时，检查测试是否覆盖新分支和原有兜底分支
- [ ] 不要只看代码能编译；接口变更未同步测试时，应作为评审项指出

**重点规则**：
- 如果 diff 中出现接口签名变更、public 方法新增、DTO 字段新增/替换，需要主动搜索相关测试文件是否同步调整
- 如果没有测试改动，不要默认认为“项目本来就没测试”而跳过，应在评审报告中明确说明“接口已修改，但未发现对应测试变更”
- 对流程类改动，优先关注：
  - 提交流程成功路径
  - 条件分支切换路径
  - 空值/查不到数据时的兜底路径

---

## 常见问题模式

### 1. 同步等待异步方法

**危害**: 线程池饥饿、死锁风险、性能下降

**检测**:
```bash
rg -n "\.Result|\.GetAwaiter\(\)|\.Wait\(\)" src test
```

### 2. 破坏性 API 变更

**危害**: 导致调用方报错

**检测**: 检查接口方法签名变更、参数类型变更

### 2.1 接口已改但缺少对应测试

**危害**: 编译虽通过，但行为回归无法被及时发现

**检测思路**:
```bash
# 先看接口/公开方法是否变化
git diff | grep -E "interface|public Task|public ResultApi|public async Task"

# 再搜索对应测试
rg -n "类名|方法名|接口名" test/ src/
```

**评审要求**：
- 若接口、public 方法、DTO 结构有改动，评审时必须检查是否存在对应测试
- 若没有对应测试变更，至少在报告“残余风险 / 测试缺口”中明确指出

### 2.2 当前工作区编译失败

**危害**: 代码不可交付，其他问题分级失去意义

**检测思路**:
```bash
python "C:\Users\nealk\.codex\skills\code-review\scripts\dotnet_review.py" --root "<repo-path>"
```

**评审要求**：
- 只要当前工作区编译失败，就必须在 `P0` 中明确列出
- 需要给出失败命令范围、关键错误摘要，以及是否可能为历史问题的说明
- 不要因为“这次 diff 看起来没碰到报错位置”就忽略编译失败

### 3. 路由前导斜杠

**危害**: 路由混乱，覆盖控制器前缀

**检测**:
```bash
rg -n '\[Http.*\("/' src
```

### 4. 硬编码配置值

**危害**: 不易维护、无法配置

**检测**: 检查数字、字符串字面量

### 5. 不一致的命名风格

**危害**: 代码可读性差

**检测**: 检查属性名是否混用命名风格

### 6. 过度设计

**危害**: 增加不必要的复杂性，降低可维护性

**过度设计的表现**：
- 为了"未来可能"的需求设计了当前不需要的功能
- 引入了不必要的抽象层（接口、抽象类）
- 使用设计模式但场景不合适
- 过度泛化，导致代码难以理解

**检查清单**：
- [ ] 接口是否只有一个实现？（如果有，考虑是否需要接口）
- [ ] 是否引入了设计模式但实际场景用不到？
- [ ] 代码是否"聪明"到难以理解？
- [ ] 是否为了"灵活性"牺牲了简洁性？

**常见问题**：
```csharp
// ❌ 过度设计：只有一个实现的接口
public interface IMessageService
{
    Task SendAsync(Message message);
}

public class EmailMessageService : IMessageService
{
    public async Task SendAsync(Message message) { ... }
}

// ✅ 简化：直接使用类
public class MessageService
{
    public async Task SendAsync(Message message) { ... }
}

// ❌ 过度设计：不必要的抽象工厂
public interface IMessageServiceFactory
{
    IMessageService Create(ServiceType type);
}

public class MessageServiceFactory : IMessageServiceFactory
{
    public IMessageService Create(ServiceType type)
    {
        return type switch
        {
            ServiceType.Email => new EmailMessageService(),
            ServiceType.Sms => new SmsMessageService(),
            _ => throw new ArgumentException()
        };
    }
}

// ✅ 简化：直接依赖注入
public class NotificationService
{
    private readonly IEnumerable<IMessageService> _services;
    public NotificationService(IEnumerable<IMessageService> services)
    {
        _services = services;
    }
}
```

### 7. 冗余代码

**危害**: 代码库膨胀，增加维护成本，容易产生混淆

**冗余代码的表现**：
- 未使用的变量、方法、类
- 重复的代码块（应提取为方法）
- 注释掉的代码（应删除，git 有历史记录）
- 无效的 using/import 语句
- 死代码（永远不会执行的分支）

**检查清单**：
- [ ] 是否有未使用的代码？
- [ ] 是否有重复的代码块？
- [ ] 是否有注释掉的代码？
- [ ] 是否有无用的条件判断？
- [ ] 是否有被注释的调试代码？

**常见问题**：
```csharp
// ❌冗余：未使用的变量
public void Process()
{
    var result = GetData();
    var unused = CalculateSomething();  // 未使用
    return result;
}

// ❌冗余：重复的代码块
public void ProcessA()
{
    var data = GetData();
    data = data.Where(x => x.IsValid).ToList();
    data = data.OrderBy(x => x.Name).ToList();
    // ... 其他处理
}

public void ProcessB()
{
    var data = GetData();
    data = data.Where(x => x.IsValid).ToList();  // 重复
    data = data.OrderBy(x => x.Name).ToList();   // 重复
    // ... 其他处理
}

// ✅提取公共方法
private List<Data> PrepareData()
{
    return GetData()
        .Where(x => x.IsValid)
        .OrderBy(x => x.Name)
        .ToList();
}

// ❌冗余：注释掉的代码
public void Process()
{
    // var old = GetDataOld();
    // ProcessOld(old);
    var data = GetDataNew();
    ProcessNew(data);
}

// ✅删除注释掉的代码（git 有历史记录）
public void Process()
{
    var data = GetDataNew();
    ProcessNew(data);
}
```

**检测命令**：
```bash
# 查找可能的未使用方法
# （需要配合 IDE 或静态分析工具）

# 查找注释掉的代码
rg -n "^\s*//.*(TODO|FIXME|HACK)" src

# 查找 console/debug 调试代码
rg -n "console\.log|Debug\.Write|Console\.Write" src
```

### 8. 固定代码（硬编码）

**危害**: 无法配置，难以适应不同环境，维护成本高

**固定代码的表现**：
- 业务规则硬编码在代码中
- 环境相关配置硬编码（URL、路径）
- 魔法数字和魔法字符串
- 业务逻辑写死在代码中

**检查清单**：
- [ ] 是否有硬编码的 URL、IP 地址？
- [ ] 是否有硬编码的文件路径？
- [ ] 是否有硬编码的业务规则？
- [ ] 是否有魔法数字和字符串？
- [ ] 时间相关逻辑是否可配置？

**常见问题**：
```csharp
// ❌硬编码：URL 和超时
public async Task<User> GetUserAsync(int id)
{
    var url = $"http://api.example.com/users/{id}";
    using var client = new HttpClient();
    client.Timeout = TimeSpan.FromSeconds(30);
    // ...
}

// ✅配置化
public class UserService
{
    private readonly ApiServiceOptions _options;
    public UserService(IOptions<ApiServiceOptions> options)
    {
        _options = options.Value;
    }

    public async Task<User> GetUserAsync(int id)
    {
        var url = $"{_options.BaseUrl}/users/{id}";
        // ...
    }
}

// ❌硬编码：业务规则
public decimal CalculateDiscount(decimal amount)
{
    if (amount > 1000) return amount * 0.1m;  // 10% 折扣
    if (amount > 500) return amount * 0.05m;  // 5% 折扣
    return 0;
}

// ✅配置化
public class DiscountService
{
    private const decimal DEFAULT_DISCOUNT = 0m;

    public decimal CalculateDiscount(decimal amount, DiscountRule rule)
    {
        return rule.Rates
            .FirstOrDefault(r => amount >= r.MinAmount)?
            .Rate ?? DEFAULT_DISCOUNT;
    }
}

// ❌硬编码：文件路径
public void SaveLog(string message)
{
    File.AppendAllText("/var/log/app.log", message);
}

// ✅配置化
public void SaveLog(string message)
{
    var logPath = _options.LogPath ?? "/var/log/app.log";
    File.AppendAllText(logPath, message);
}

// ❌硬编码：魔法字符串
public void Process(string type)
{
    if (type == "special")  // 魔法字符串
    {
        // ...
    }
}

// ✅使用常量或枚举
private const string SPECIAL_TYPE = "special";
// 或使用枚举
public enum ProcessType { Normal, Special }

public void Process(ProcessType type)
{
    if (type == ProcessType.Special)
    {
        // ...
    }
}
```

**检测命令**：
```bash
# 查找硬编码的 URL
rg -n 'http[s]?://' src

# 查找硬编码的 IP 地址
rg -n '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' src

# 查找硬编码的文件路径
rg -n '(/[a-z]+)+' src
```

---

## 提交前自查

开发者提交代码前必须完成：
- [ ] 代码能够正常编译
- [ ] 单元测试通过
- [ ] 自己 Review 一遍代码
- [ ] 改动说明清晰完整

---

## 评审礼仪

### 评审者
- 对事不对人，建设性反馈
- 解释"为什么"，不只是说"不行"
- 及时响应，不要阻塞太久
- 认可好的代码，不只是挑毛病
- 用中文回复

### 被评审者
- 虚心接受，不要防御性反应
- 不懂就问，评审是学习机会
- 及时响应和修复
- 感谢评审者的时间
