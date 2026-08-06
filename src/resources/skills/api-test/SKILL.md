---
name: api-test
description: 生成 REST API 接口测试用例，使用 Playwright 和 TypeScript，包含完整断言、日志、HTML 详细测试报告（含 JSONL 合并、失败摘要、完整入参出参）。当用户需要编写/执行 API 测试、接口测试、生成 HTML 测试报告时使用。
triggers:
  - api test
  - API测试
  - 接口测试
risk: write
enabled: true
---
# REST API 接口测试

基于 Playwright 框架生成 REST API 接口测试用例，包含完整的测试报告系统。

## ⚠️ 重要：生成流程

**在生成测试代码之前，必须先输出测试用例文档！**

### 步骤1：生成测试用例文档

1. 收集接口信息（URL、方法、参数、响应）
2. 分析业务逻辑
3. 设计测试用例（正常、异常、边界场景）
4. **必须包含文档结构中的「5.5 认证与权限测试」「5.6 安全测试」**（与 [testcase-template.md](testcase-template.md) 一致）：
   - **5.5**：无 Token、伪造/非法 Token、过期 Token（可测则测）、**越权**（多角色时：无菜单账号、窄权限账号不得扩大数据范围、带查询条件仍拒绝无菜单）
   - **5.6**：按参数类型设计 —— 字符串/文本参数侧重 **SQL 注入、XSS、超长**；纯数值/ID 数组可写 **类型混淆、非法 JSON、畸形体**，无文本注入面时注明 **N/A** 并仍保留 **鉴权类** 用例
5. **必须包含「5.7 空/null/边界/非法枚举」「5.8 并发与重复提交」**（见下方 **扩展用例设计（强制）**）
6. **输出 `{接口名称}-TestCase.md` 文档**（含上述章节与用例表）
7. 等待用户确认（用户明确说「跳过确认」时可同步生成代码，但文档仍须先写出）

### 步骤2：生成测试代码

用户确认用例文档后，再生成 `.spec.ts` 测试代码。

**硬性要求**：`.spec.ts` 中的用例须与 `TestCase.md` 对齐，**至少覆盖**：

- 文档 **5.5** 中已列出的鉴权场景（能自动化的全部实现）
- 文档 **5.6** 中已列出的安全场景（能自动化的全部实现；不适用的在 spec 里 `test.skip` 并注释原因）

**断言与日志硬性要求**（见下方「断言原则」「响应日志原则」「HTML 测试报告要求」）：

- 断言 **只来源于** `TestCase.md` / 需求文档，**禁止**因接口实测与预期不符而放宽、`if/else` 双分支、「记缺陷但通过」
- 请求体、响应体 **完整输出**，禁止 `substring` 截断
- 执行测试后 **必须生成详细 HTML 报告**（见 [html-report.md](html-report.md)），含完整入参/出参、失败摘要、环境配置

详细模板见 [testcase-template.md](testcase-template.md)

---

## 目录与命名（新需求强制）

**每个新需求**须在对应系统的 `e2e/` 根目录下**新建英文子目录**，不得把不同需求的 spec / TestCase 混放在系统根目录。

### 层级结构

```
e2e/{系统英文目录}/
├── README.md                              # 系统索引（新需求须追加一行）
└── {需求英文目录}/                        # 按需求建文件夹（kebab-case）
    ├── README.md                          # 可选：本需求说明与命令
    ├── {ApiOrModule}-TestCase.md          # 接口用例文档（先文档后代码）
    ├── {api-or-module}.spec.ts            # Playwright spec
    ├── *-db.ts / *-fixture.ts             # 本需求共享造数/夹具（按需）
    ├── *-suite.config.ts                  # 多 spec 套件配置（按需）
    └── run-all-api-tests.js               # 全量串行 + 汇总报告（按需）
```

### 系统 → `e2e` 根目录

| 系统 | `e2e` 根目录 | 代码工程 |
| --- | --- | --- |
| 用工关系管理平台 | `e2e/employment-relation/` | `employment-relation-be` |
| 薪酬福利管理平台 | `e2e/paypro-platform/` | `paypro-platform-be` |
| 考勤 / issclock | `e2e/isslock/` | `issclock-be` 等 |
| 老人事 | 按需新建 `e2e/ipsa-ioa/` 等 | `ipsa-ioa` |

> 功能用例 CSV/MD 仍放 `测试用例/{系统中文名}/`；**接口自动化与 `*-TestCase.md` 放 `e2e/{系统}/{需求英文}/`**。

### 需求子目录命名规则

| 规则 | 说明 |
| --- | --- |
| **语言** | 仅 **英文**，禁止中文目录名 |
| **格式** | **kebab-case**（小写 + 连字符） |
| **语义** | 业务/模块简称，与需求名对应，非 Controller 类名 |
| **长度** | 建议 2～4 个单词，≤40 字符 |
| **禁止** | 中文、空格、驼峰目录名、与无关需求共用同一目录 |

### 命名示例（用工关系）

| 需求 | 推荐目录名 | 说明 |
| --- | --- | --- |
| §4.1 非核查人员名单管控 | `non-verification-personnel` | 已建，含 `eda-blueblack-*` |
| §4.2 用工单据变更 | `employment-doc-change` | 取消/修改申请、审批、查询 |
| （反例） | `非核查人员`、`EdaBlueblack` | ❌ 中文 / 纯 API 类名不宜作目录名 |

### 文件命名（目录内）

| 类型 | 命名 | 示例 |
| --- | --- | --- |
| 用例文档 | `{接口或模块 PascalCase}-TestCase.md` | `EdaBlueblack-ImportExcel-TestCase.md` |
| Spec | `{api-kebab}.spec.ts` | `eda-blueblack-import-excel.spec.ts` |
| 共享模块 | `{前缀}-db.ts`、`*-fixture.ts` | 同需求内复用，不跨需求引用 |

### 新需求 Checklist（Agent 必做）

1. 确认系统 → 选定 `e2e/{系统}/` 根目录  
2. 按上表取 **英文 kebab-case** 目录名；若已存在则在该目录内追加，**不**新建第二套同名目录  
3. 将 `{Api}-TestCase.md` 与 `.spec.ts` 写入 **需求子目录**  
4. 更新 `e2e/{系统}/README.md` 索引表  
5. 更新 `代码分析/README.md` 该需求行的 **e2e 路径**  
6. 多 spec 套件时：在本需求目录内维护 `*-suite.config.ts`、`run-all-api-tests.js`，并在 `package.json` 增加 `test:{前缀}:*` script（路径含完整子目录）

**范例目录**：[`e2e/employment-relation/non-verification-personnel/`](../../employment-relation/non-verification-personnel/README.md)

---

## 断言原则（强制）

接口自动化测试的目的是 **暴露缺陷**，不是让用例「跑绿」。

### 必须遵守

1. **断言唯一来源**：`TestCase.md`、需求文档、功能用例 CSV 中的「预期结果」；不得根据某次联调的实际返回值反推断言。
2. **禁止自适应通过**：
   - ❌ `if (result.data?.code === '0') { recordAssertion('缺陷但通过') } else { expect... }`
   - ❌ `expect(['1','2']).toContain(code)` 替代明确的 `toBe('2')`（除非文档明确允许多值）
   - ❌ 用宽泛正则 `toMatch(/A|B|C/)` 掩盖本应唯一的错误文案
3. **实测与需求不一致时**：测试应 **fail**，由测试人员提 Bug；**不得**修改断言或 TestCase 预期来迁就实现。
4. **允许 skip 的唯一情况**：环境客观不可测（如无过期 Token、无对应测试账号），使用 `test.skip(true, '原因')` 并在 TestCase 中注明。
5. **每个用例**应对 HTTP 状态码、业务 `code`/`Code`、关键 `message`、行级 `error`、核心出参字段做 **精确断言**（`toBe` / `toContain` 完整字符串）。
6. **禁止仅断言 HTTP 200**：查询类接口成功时须同时断言 `Code=200`、`Message` 业务文案、`Data` 结构、`Count` 与行级字段（如筛选条件、脱敏、权限子集）；失败时须断言 `Code=500`（或约定码）及完整 `Message`，不得只验状态码。

### 业务断言最低要求（查询类接口）

| 场景 | 最低断言 |
|------|----------|
| 成功查询 | `HTTP 200` + `Code=200` + `Message=查询成功` + `Data` 为数组 + `Count≥0` |
| 无匹配 | `Code=200` + `Count=0` + `Data=[]` |
| 入参校验失败 | `Code=500` + `Message` 完整匹配 TestCase |
| 无菜单/越权 | `Code=500` + 权限文案；窄权限同条件 `Count ≤ admin Count` |
| 筛选命中 | 每条 `Data` 行满足筛选字段（如 `EmployeeTypeNo`、脱敏规则） |

---

## 扩展用例设计（强制）

生成 TestCase 与 spec 时，**除 5.5/5.6 外还须覆盖**（不适用时在文档注明 N/A 并在 spec 中 `test.skip`）：

### 5.7 空 / null / 边界 / 非法枚举

| 类型 | 必设计项 | 说明 |
|------|----------|------|
| **空** | `""` 字符串条件 | Employee/Phone/IDCard 空串，等价无筛选或按实码校验 |
| **null** | JSON 显式 `null` | 可选字段传 `null`，应与省略字段行为一致 |
| **边界** | 分页越界 | `PageIndex=0`、超大页码、`PageSize=0` 或极小值 |
| **非法枚举** | 0、负数、超范围 | 如 `EmployeeType=0/-1/3`、`EmployeeSource=-1/99` |

### 5.8 越权（多角色环境）

- 无菜单账号：空条件 + **带查询条件** 均应拒绝（不能只测空 body）
- 窄权限账号：同条件 `Count` **不得大于** admin；结果 ID 集合为 admin 子集
- 不可用高权限 Token 访问低权限不应测（反向）；重点是 **低权限不能扩大数据范围**

### 5.9 并发与重复提交（查询/幂等接口）

- **重复提交**：同一 Token、同一 body 连续 2 次，`Count` 与首屏 `Data` 关键字段一致
- **简单并发**：`Promise.all` 并行 3~5 次相同请求，均 `Code=200` 且 `Count` 一致（允许 `Data` 顺序波动时只比 `Count`）

> 写操作接口另需：重复提交是否重复落库、并发是否脏写（按业务设计）。

### 示例（ImportResult）

```typescript
// ✅ 严格：与 TestCase 一致
expect(result.data?.code).toBe('1');
expect(collectErrors(result)).toContain('未找到手机号对应的员工');

// ❌ 禁止：迁就错误实现
if (result.data?.code === '0') {
  recordAssertion('缺陷但通过');
} else {
  expect(result.data?.code).toBe('1');
}
```

---

## 响应日志原则（强制）

测试过程须 **完整可见** 入参、出参，便于人工核对 Bug。

### 必须遵守

1. **完整请求体**：`JSON.stringify(payload, null, 2)` 写入控制台与 `.log` 文件，不要只打 `list行数=1`。
2. **完整响应体**：输出 **未经截断** 的响应文本或格式化 JSON；禁止 `responseText.substring(0, 500)` 等截断。
3. **HTML 报告**：`recordRequest` 中保存 **完整** `responseBody`（Token 可脱敏 `Authorization`，业务 body 不截断）。
4. **辅助函数**：统一使用 `logFullPayload(label, data)` 输出大块 JSON。

```typescript
function logFullPayload(label: string, payload: unknown) {
  const text = typeof payload === 'string' ? payload : JSON.stringify(payload, null, 2);
  logResult(`${label}:\n${text}`);
}

// 请求后
logFullPayload('📥 完整响应体', responseText);
```

---

## HTML 测试报告要求（强制）

用户要求「执行测试并生成报告」时，须产出 **可独立打开的 HTML 文件**（非仅 Playwright 内置 reporter），作为 Bug 举证与回归依据。

### 必须遵守

1. **自动生成**：`test.afterAll` 中调用 `generateHtmlReport()`；每次执行后落盘，无需手工二次处理。
2. **双路径输出**：
   - 带时间戳：`result/{Endpoint}_{runId}.html`
   - 固定最新：`result/{Endpoint}_latest.html`（便于用户直接打开）
3. **汇总区**：总用例、通过、失败、跳过、通过率、总耗时。
4. **失败用例摘要**：所有失败用例置顶汇总（用例 ID、名称、断言错误原文），标注「疑似接口缺陷」；**失败即 Bug 证据，不得改断言迁就**。
5. **环境配置区**：Token 地址、目标接口、主账号及多角色账号（如有）、超时等。
6. **用例详情**（点击展开/收起）：
   - 状态徽章、耗时、请求数、断言数、开始/结束时间
   - **完整请求体** + **完整响应体**（JSON 格式化，`escapeHtml` 防 XSS；仅 `Authorization` 可脱敏）
   - `recordAssertion` 记录的断言列表
   - 失败时展示 Playwright 错误信息
7. **交互**：筛选按钮（全部/通过/失败/跳过）；**失败用例默认展开**。
8. **Playwright Worker 重启**：失败后会拆分 Worker，须 **JSONL 持久化** 合并报告（见 [html-report.md](html-report.md)），确保 HTML 含 **全部用例** 而非最后一个 Worker 的子集。
9. **同步日志**：`.log` 与 HTML 使用同一 `runId`，日志中也须完整输出请求/响应。

### 执行与交付

```bash
cd midscene-demo
npx playwright test e2e/xxx/your-api.spec.ts
# 报告：result/YourEndpoint_latest.html
```

用户说「执行并生成详细测试报告 / HTML 报告」时：**运行测试 → 确认 HTML 已生成 → 告知绝对路径与通过/失败统计**。

参考实现：`e2e/employment-relation/non-verification-personnel/eda-blueblack-import-excel.spec.ts`

---

## 测试文件完整模板

```typescript
import { test, expect, APIRequestContext } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

/**
 * [接口名称] 接口测试
 * 测试接口：[接口路径]
 * 请求方式：POST
 * 请求格式：application/json
 * 功能：[接口功能描述]
 * 请求体格式：
 * {
 *   "param1": "string",
 *   "param2": number
 * }
 */

// ==================== 配置区域 ====================

// API配置
const API_CONFIG = {
  tokenBaseUrl: 'http://10.136.5.201:30912',      // Token服务地址
  targetBaseUrl: 'http://10.136.129.76:60015',    // 目标接口地址
  tokenEndpoint: '/api/Mock/GetToken',             // Token接口路径
  targetEndpoint: '/api/xxx/xxx/YourEndpoint',     // 目标接口路径
  timeout: 30000
};

// 登录配置
const LOGIN_CONFIG = {
  IsProd: 0,
  ClientId: 'a0b2da4f-5966-47e6-a5b6-8535fab8148c',
  Scope: 'MasterData MasterData2 Common ErAPI PayproPlatform openid',
  UserName: 'xzlid',
  RedirectUri: 'http://ipsademo.isoftstone.com/xxx/oidc-callback.html'
};

// 测试数据
const TEST_DATA = {
  validParam: 'value1',        // 有效参数
  invalidParam: 'INVALID_XXX', // 无效参数
  emptyParam: ''               // 空参数
};

// ==================== 报告系统 ====================

// 创建结果目录
const RESULT_DIR = path.join(process.cwd(), 'result');
if (!fs.existsSync(RESULT_DIR)) {
  fs.mkdirSync(RESULT_DIR, { recursive: true });
}

// 生成结果文件名
const timestamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
const resultFileName = `YourEndpoint_${timestamp}.log`;
const resultFilePath = path.join(RESULT_DIR, resultFileName);
const htmlReportPath = path.join(RESULT_DIR, `YourEndpoint_${timestamp}.html`);

// 测试报告数据结构
interface TestCaseReport {
  id: string;
  name: string;
  status: 'passed' | 'failed' | 'skipped';
  startTime: string;
  endTime: string;
  duration: number;
  requests: Array<{
    url: string;
    method: string;
    headers: Record<string, string>;
    requestBody: any;
    responseStatus: number;
    responseHeaders?: Record<string, string>;
    responseBody: any;
    responseTime: number;
  }>;
  assertions: string[];
  error?: string;
}

const testReports: TestCaseReport[] = [];
let currentTestReport: TestCaseReport | null = null;

// 日志记录函数
function logResult(message: string) {
  const logMessage = `[${new Date().toISOString()}] ${message}\n`;
  console.log(message);
  fs.appendFileSync(resultFilePath, logMessage);
}

function logFullPayload(label: string, payload: unknown) {
  const text =
    payload === null || payload === undefined
      ? String(payload)
      : typeof payload === 'string'
        ? payload
        : JSON.stringify(payload, null, 2);
  logResult(`${label}:\n${text}`);
}

// 开始记录测试用例
function startTestCase(id: string, name: string) {
  currentTestReport = {
    id, name,
    status: 'passed',
    startTime: new Date().toISOString(),
    endTime: '',
    duration: 0,
    requests: [],
    assertions: []
  };
}

// 记录请求信息
function recordRequest(
  url: string, method: string, headers: Record<string, string>,
  requestBody: any, responseStatus: number, responseBody: any,
  responseTime: number, responseHeaders?: Record<string, string>
) {
  if (currentTestReport) {
    currentTestReport.requests.push({
      url, method,
      headers: { ...headers, Authorization: headers.Authorization ? '[BEARER TOKEN]' : undefined } as any,
      requestBody, responseStatus, responseHeaders, responseBody, responseTime
    });
  }
}

// 记录断言
function recordAssertion(assertion: string) {
  if (currentTestReport) {
    currentTestReport.assertions.push(assertion);
  }
}

// 结束测试用例（须 persist 到 jsonl，见 html-report.md）
function endTestCase(status: 'passed' | 'failed' | 'skipped', error?: string) {
  if (currentTestReport) {
    currentTestReport.endTime = new Date().toISOString();
    currentTestReport.duration = new Date(currentTestReport.endTime).getTime() - new Date(currentTestReport.startTime).getTime();
    currentTestReport.status = status;
    if (error) currentTestReport.error = error;
    testReports.push(currentTestReport);
    persistTestReport(currentTestReport); // JSONL 持久化，Worker 重启后合并报告
    currentTestReport = null;
  }
}

// ==================== Token获取函数 ====================

async function getToken(request: APIRequestContext, loginConfig = LOGIN_CONFIG): Promise<string> {
  const url = `${API_CONFIG.tokenBaseUrl}${API_CONFIG.tokenEndpoint}`;
  logResult(`📤 获取Token: POST ${url}`);

  const response = await request.post(url, {
    data: loginConfig,
    headers: { 'Content-Type': 'application/json' },
    timeout: API_CONFIG.timeout
  });

  const responseText = await response.text();
  const responseJson = JSON.parse(responseText);
  const accessToken = responseJson?.Data?.access_token || responseJson?.access_token;
  
  if (!accessToken) {
    throw new Error('Token响应中缺少access_token');
  }

  logResult(`✅ Token获取成功 (用户: ${loginConfig.UserName})`);
  return accessToken;
}

// ==================== API请求函数 ====================

interface RequestData {
  // 根据接口定义请求参数
  param1?: string;
  param2?: number;
}

async function makeApiRequest(
  request: APIRequestContext, 
  requestData: RequestData, 
  token: string, 
  options: any = {}
) {
  const url = `${API_CONFIG.targetBaseUrl}${API_CONFIG.targetEndpoint}`;
  const startTime = Date.now();
  
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    'User-Agent': 'Playwright-AutoTest/1.0',
    ...options.headers
  };

  if (options.includeAuth !== false && token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const requestBody = options.emptyBody ? {} : 
                      options.noBody ? undefined : 
                      requestData;

  logResult(`📤 发送请求: POST ${url}`);
  logFullPayload('📤 完整请求体', requestBody);

  const response = await request.post(url, {
    timeout: options.timeout || API_CONFIG.timeout,
    headers,
    data: requestBody
  });

  const responseTime = Date.now() - startTime;
  const statusCode = response.status();
  const responseText = await response.text();
  const responseHeaders = response.headers();

  logResult(`📥 响应状态码: ${statusCode}`);
  logFullPayload('📥 完整响应体', responseText);

  let responseJson = null;
  try {
    responseJson = JSON.parse(responseText);
  } catch (e) {}

  recordRequest(url, 'POST', headers, requestBody, statusCode, responseJson || responseText, responseTime, responseHeaders);

  return {
    status: statusCode,
    data: responseJson,
    text: responseText,
    headers: responseHeaders,
    response,
    responseTime
  };
}

// ==================== 测试用例 ====================

test.describe('YourEndpoint 接口测试', () => {
  let testStats = { total: 0, passed: 0, failed: 0, skipped: 0 };
  let accessToken: string = '';

  test.beforeAll(async () => {
    logResult('🚀 开始 YourEndpoint 接口测试');
    logResult(`📋 Token接口: ${API_CONFIG.tokenBaseUrl}${API_CONFIG.tokenEndpoint}`);
    logResult(`📋 目标接口: ${API_CONFIG.targetBaseUrl}${API_CONFIG.targetEndpoint}`);
  });

  test.afterAll(async () => {
    logResult('\n📊 测试统计:');
    logResult(`总用例数: ${testStats.total}`);
    logResult(`通过数: ${testStats.passed}`);
    logResult(`失败数: ${testStats.failed}`);
    logResult(`通过率: ${testStats.total > 0 ? ((testStats.passed / testStats.total) * 100).toFixed(2) : 0}%`);
    generateHtmlReport();
  });

  test.beforeEach(async ({}, testInfo) => {
    testStats.total++;
    const match = testInfo.title.match(/^(TC-\w+-\d+[A-Z]?):?\s*(.*)$/);
    startTestCase(match ? match[1] : testInfo.title, match ? match[2] : testInfo.title);
  });

  test.afterEach(async ({}, testInfo) => {
    const status = testInfo.status === 'passed' ? 'passed' : 
                   testInfo.status === 'skipped' ? 'skipped' : 'failed';
    endTestCase(status, testInfo.error?.message);
  });

  // TC-XXX-001: 登录获取Token
  test('TC-XXX-001: 登录获取Token', async ({ request }) => {
    logResult('\n🧪 TC-XXX-001: 登录获取Token');
    accessToken = await getToken(request);
    expect(accessToken).toBeTruthy();
    expect(accessToken.length).toBeGreaterThan(0);
    recordAssertion('Token获取成功');
    logResult(`✅ TC-XXX-001 通过`);
    testStats.passed++;
  });

  // TC-XXX-002: 正常查询
  test('TC-XXX-002: 正常查询', async ({ request }) => {
    logResult('\n🧪 TC-XXX-002: 正常查询');
    if (!accessToken) accessToken = await getToken(request);

    const result = await makeApiRequest(request, {
      param1: TEST_DATA.validParam
    }, accessToken);

    expect(result.status).toBe(200);
    expect(result.data?.Code).toBe(200);
    expect(result.data?.Data).toBeDefined();
  
    recordAssertion('HTTP状态码为200');
    recordAssertion('响应Code为200');
    recordAssertion('Data字段存在');
  
    logResult(`✅ TC-XXX-002 通过`);
    testStats.passed++;
  });

  // TC-XXX-003: 无效参数测试
  test('TC-XXX-003: 无效参数测试', async ({ request }) => {
    logResult('\n🧪 TC-XXX-003: 无效参数测试');
    if (!accessToken) accessToken = await getToken(request);

    const result = await makeApiRequest(request, {
      param1: TEST_DATA.invalidParam
    }, accessToken);

    expect(result.status).toBe(200);
    // 根据业务逻辑验证返回
  
    logResult(`✅ TC-XXX-003 通过`);
    testStats.passed++;
  });

  // TC-XXX-004: 无Token访问
  test('TC-XXX-004: 无Token访问', async ({ request }) => {
    logResult('\n🧪 TC-XXX-004: 无Token访问');

    const result = await makeApiRequest(request, {
      param1: TEST_DATA.validParam
    }, '', { includeAuth: false });

    expect(result.status).toBe(401);
    recordAssertion('无Token返回401');
  
    logResult(`✅ TC-XXX-004 通过`);
    testStats.passed++;
  });
});

// ==================== HTML报告生成（见 html-report.md：须 loadAllTestReports + _latest.html）====================
function generateHtmlReport() {
  // 见 html-report.md — 含失败摘要、筛选、完整请求/响应、JSONL 合并
}
function persistTestReport(report: TestCaseReport) { /* 见 html-report.md */ }
function loadAllTestReports(): TestCaseReport[] { /* 见 html-report.md */ }
```

## 用例命名规范

- 用例ID格式: `TC-[模块]-NNN`
- 示例: `TC-HRDEPT-001`, `TC-PAYCONFIG-002`
- 标题格式: `TC-XXX-001: 描述文字`

## 测试用例类型

| 类型      | 说明               | 示例           |
| --------- | ------------------ | -------------- |
| 登录Token | 获取访问令牌       | TC-XXX-001     |
| 正常查询  | 有效参数正常流程   | TC-XXX-002     |
| 参数验证  | 必填/格式/范围     | TC-XXX-003~005 |
| 分页测试  | pageIndex/pageSize | TC-XXX-006     |
| 搜索测试  | keyword模糊搜索    | TC-XXX-007     |
| 权限测试  | 无Token/无效Token  | TC-XXX-008     |
| 边界测试  | 空值/特殊字符/分页越界 | TC-XXX-009     |
| 空/null/枚举 | 空串、null、非法枚举、分页边界 | TC-XXX-050~059 |
| 越权测试  | 无菜单+带条件、窄权限子集 | TC-XXX-020~029 |
| 并发/重复 | 重复提交、并行请求 Count 一致 | TC-XXX-060~069 |

## 执行命令

```bash
# 执行单个测试文件（执行后自动生成 HTML + .log，见 result/ 目录）
npx playwright test e2e/paypro-platform/your-api.spec.ts

# 执行特定用例
npx playwright test -g "TC-XXX-002"

# 查看最新 HTML 报告（Windows）
start result/YourEndpoint_latest.html
```

> Playwright 内置 `--reporter=html` 仅作辅助；**接口测试的主报告**为 spec 内 `generateHtmlReport()` 生成的自定义 HTML。

## 参考资源

- **测试用例文档模板**，见 [testcase-template.md](testcase-template.md)
- 请求方法示例，见 [request-examples.md](request-examples.md)
- 断言模式，见 [assertions.md](assertions.md)
- HTML报告生成，见 [html-report.md](html-report.md)

---

## 测试用例文档结构

文档命名: `{接口名称}-TestCase.md`

```
一、接口概述
   - 接口路径、HTTP方法、功能说明、认证方式

二、环境配置
   - 测试环境地址、登录配置、测试账号

三、登录流程
   - Token获取、Token使用

四、接口详细说明
   - 请求参数、响应参数、业务逻辑

五、测试用例
   - 5.1 前置条件（登录Token）
   - 5.2 正常场景测试
   - 5.3 分页测试（如适用）
   - 5.4 异常场景测试
   - 5.5 认证与权限测试
   - 5.6 安全测试
   - 5.7 空/null/边界/非法枚举
   - 5.8 越权（多角色）
   - 5.9 并发与重复提交

六、测试数据准备
七、测试执行顺序（P0/P1/P2）
八、测试报告模板
```

## 用例命名规范

| 分类     | 用例ID范围     | 说明                     |
| -------- | -------------- | ------------------------ |
| 前置条件 | TC-XXX-001     | 登录Token                |
| 正常场景 | TC-XXX-002~009 | 基本查询、分页、搜索     |
| 异常场景 | TC-XXX-010~019 | 无效参数、空参数、边界值 |
| 认证权限 | TC-XXX-020~029 | 无Token、无效Token       |
| 安全测试 | TC-XXX-030~039 | SQL注入、XSS             |
| 空/null/边界/枚举 | TC-XXX-050~059 | 空串、null、分页越界、非法枚举 |
| 并发/重复 | TC-XXX-060~069 | 重复提交、简单并行         |
