# 禅道 REST API（Bug 修复专用）

与 `zentao-bugs` 共用凭据，本文档补充**详情**与**备注**接口。

## 凭据

- **服务地址**: `https://zentao.isscloud.com`
- **配置文件**: `%USERPROFILE%\.cursor\zentao.config.json`
- **环境变量**（优先）: `ZENTAO_name` / `ZENTAO_ACCOUNT`、`ZENTAO_PASSWORD`

## 登录

```
POST https://zentao.isscloud.com/api.php/v1/tokens
Content-Type: application/json

{"account":"{{account}}","password":"{{password}}"}
```

成功：HTTP 201，`{"token":"<TOKEN>"}`。后续请求 Header：`Token: <TOKEN>`。

## 当前用户

```
GET /api.php/v1/user
```

用于「我的 bug」过滤：`profile.view.products`、账号 `account`。

## 产品列表

```
GET /api.php/v1/products?limit=200
```

## Bug 列表（按产品）

我的 bug：

```
GET /api.php/v1/products/{productID}/bugs?browseType=assigntome&limit=200
```

未解决（团队）：

```
GET /api.php/v1/products/{productID}/bugs?browseType=unresolved&limit=200
```

### 列表过滤（必须）

- 跨产品按 Bug `id` 去重
- 只保留 `status` = `"active"`
- 「我的 bug」再过滤 `assignedTo.account` = 当前用户账号

## Bug 详情

```
GET /api.php/v1/bugs/{bugID}
```

常用字段：

| 字段 | 说明 |
|------|------|
| `id` | Bug ID |
| `title` | 标题 |
| `steps` | 重现步骤（HTML，展示时转纯文本） |
| `severity` | 严重程度 1-4 |
| `pri` | 优先级 1-4 |
| `status` | active / resolved / closed |
| `confirmed` | 0 未确认 / 1 已确认 |
| `product` | `{id, name}` |
| `module` | `{id, name}` 或 null |
| `project` | 关联项目 |
| `assignedTo` | `{account, realname}` |
| `openedBy` | 创建人 |
| `keywords` | 关键词 |
| `type` | 类型 |
| `os` / `browser` | 环境 |

页面链接：`https://zentao.isscloud.com/bug-view-{bugID}.html`

## Bug 备注（阶段四，需用户授权）

```
POST /api.php/v1/bugs/{bugID}/comments
Content-Type: application/json

{"comment": "修复说明：...\n文档：docs/lessons/zentao-bugs/zentao-bug-{id}.md"}
```

## 严重程度 / 优先级对照

| severity | 标签 |
|----------|------|
| 1 | 致命 |
| 2 | 严重 |
| 3 | 主要 |
| 4 | 次要 |

| pri | 标签 |
|-----|------|
| 1 | 紧急 |
| 2 | 高 |
| 3 | 中 |
| 4 | 低 |

## PowerShell 调用示例

```powershell
$config = Get-Content "$env:USERPROFILE\.cursor\zentao.config.json" | ConvertFrom-Json
$token = (Invoke-RestMethod -Uri "https://zentao.isscloud.com/api.php/v1/tokens" -Method Post -ContentType 'application/json' -Body (@{ account = $config.account; password = $config.password } | ConvertTo-Json)).token
$bug = Invoke-RestMethod -Uri "https://zentao.isscloud.com/api.php/v1/bugs/12345" -Headers @{ Token = $token }
$bug | ConvertTo-Json -Depth 5
```
