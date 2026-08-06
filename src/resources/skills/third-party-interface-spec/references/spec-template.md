# 第三方接口开发规格模板

## 1. 接口身份

| 项目 | 内容 |
| --- | --- |
| 接口名称 |  |
| 接口来源 | SAP / 中台 iDataPlatform / curl / Apifox MCP OAS / 混合 |
| 来源系统 |  |
| 目标系统 |  |
| 接口编码 |  |
| FunctionName | 仅 SAP |
| apiPath | 仅中台 |
| 资料来源 | Excel 文件、sheet、中台页面采集、curl、Apifox MCP OAS/schema/example、用户补充说明 |

## 2. 调用方式

| 项目 | 内容 |
| --- | --- |
| HTTP Method |  |
| 最终 URL |  |
| Content-Type |  |
| 认证方式 |  |
| 请求 Header |  |
| 请求 Body 类型 | JSON / form-data / x-www-form-urlencoded / none |

## 2.1 认证与配置

| 项目 | 内容 |
| --- | --- |
| 配置文件检查范围 | `appsettings.json` / `appsettings.*.json` / `Web.config` / `Web.config.xml` / 项目现有配置类 |
| SAP 测试 Invoke 配置 | 检查是否存在 `https://apidemo.isoftstone.com/PSA/SAP/v1/201` 或完整 `/Invoke` 地址 |
| SAP 测试 Token 配置 | 检查是否存在 `https://apidemo.isoftstone.com/ids/connect/token` 及 token 参数；缺失时提示“增加获取psa网关token配置” |
| SAP Token Header | `Authorization: Bearer {token}` |
| 中台 Token 配置 | 检查是否存在 `https://ipsapro.isoftstone.com/iDataPlatform/idss/sys/getToken`；缺失时提示“增加获取中台token配置” |
| 中台业务 Header | `X-Access-Token: {token}` |
| Apifox URL 判定 | 业务 URL 含 `apimarket` 时按中台处理；否则按 PSA 网关处理 |
| Apifox OAS 来源 | 只记录脱敏后的 OAS 来源、schema/example 与字段结构；不记录真实 token、secret、cookie |
| 密钥处理 | 不写死 `client_secret`、`appSecret`、token；只通过项目配置读取 |

## 3. 请求结构

说明顶层 body 结构、SAP 表参数或中台 request body。

```json
{}
```

## 4. 请求字段

| 层级/路径 | 第三方字段名 | 建议 C# 属性名 | 中文名 | 原始类型 | 建议 C# 类型 | 长度 | 小数位 | 必填 | 默认值/固定值 | 值域/枚举 | 说明 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## 5. 响应结构

说明响应顶层结构、数组路径和业务数据路径。
如果是中台 iDataPlatform，默认响应外层固定为 `message`、`msg`、`data`，业务数据数组路径为 `data[]`，不要默认使用 `records`。

```json
{
  "message": null,
  "msg": "接口访问成功",
  "data": []
}
```

## 6. 响应字段

| 层级/路径 | 第三方字段名 | 建议 C# 属性名 | 中文名 | 原始类型 | 建议 C# 类型 | 长度 | 小数位 | 必填 | 值域/枚举 | 说明 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## 7. 字段转换和业务规则

| 规则项 | 规则 |
| --- | --- |
| 字段命名 | 保留第三方字段名，通过序列化特性映射到 C# 属性 |
| 日期处理 |  |
| 金额/数量处理 |  |
| 空值处理 |  |
| 枚举处理 |  |
| 默认值/固定值 |  |

## 8. 示例请求

```json
{}
```

## 9. 示例响应

```json
{
  "message": null,
  "msg": "接口访问成功",
  "data": []
}
```

## 10. 给 AI 的 C# 开发指令

你现在是 C# 后端开发助手。请阅读本接口开发规格，在当前项目中实现第三方接口对接。

要求：

1. 优先复用当前项目已有的 HTTP、SAP、日志、异常处理和配置读取方式。
2. 根据“请求字段”生成 Request DTO，根据“响应字段”生成 Response DTO。
3. 保留第三方字段名，不要自行改名；如 C# 属性名不同，使用项目现有 JSON 序列化特性映射。
4. 必填字段按规格增加校验。
5. SAP `NUMC` 按 `string` 处理，避免前导 0 丢失；`CURR/DEC/QUAN` 按 `decimal` 处理。
6. 中台接口按最终 URL 和 Method 调用，`apiPath` 拼接规则以本规格为准。
7. 如果项目已配置 SAP 测试 Invoke 和 PSA token 配置，先获取 PSA token，再以 `Authorization: Bearer {token}` 调用 SAP；如果未配置，提示“增加获取psa网关token配置”。
8. 如果项目已配置中台 token 地址，先通过 `appID/appSecret` 获取 token，再以 `X-Access-Token: {token}` 调用中台业务接口；如果未配置，提示“增加获取中台token配置”。
9. 如果资料来自 Apifox MCP OAS/schema/example，按业务 URL 或 path 是否包含 `apimarket` 选择中台或 PSA token 获取方式，并优先复用 OAS 的请求/响应字段生成 DTO。
10. 记录请求、响应和异常日志，敏感 header、token、`client_secret`、`appSecret` 脱敏。
11. 本规格中的“待确认问题”不要强行猜测；如果影响实现，先列出问题。

## 11. 待确认问题

| 优先级 | 问题 | 影响 |
| --- | --- | --- |
