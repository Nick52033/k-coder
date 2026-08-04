# 本机项目代码路径映射

**项目代码根目录（固定）：** `D:\ProjectCode`

用户仅提供**项目名**时，仓库路径解析为：

```
D:\ProjectCode\{项目名}
```

用户提供了绝对路径时，以用户路径为准。

## 路径解析规则

1. 输入为文件夹名（如 `paypro-platform-be`）→ `D:\ProjectCode\paypro-platform-be`
2. 输入为绝对路径 → 直接使用
3. 输入为相对路径 → 先尝试 `D:\ProjectCode\{相对路径}`
4. 目录不存在 → 列出 `D:\ProjectCode` 下相似目录让用户确认

## 已登记项目（常用）

| 项目名 | 仓库路径 | 技术栈 | 代码检索重点 |
|--------|----------|--------|--------------|
| paypro-platform-be | `D:\ProjectCode\paypro-platform-be` | ABP 8 + .NET 8 + SqlSugar + MySQL + Vue | Application/Services, Domain/Entities, HttpApi |
| ioa-be | `D:\ProjectCode\ioa-be` | ABP 8 + .NET 8 + SqlSugar + 多库 | 同上 + Tenant 连接串 |
| ipsa-rra | `D:\ProjectCode\ipsa-rra` | .NET Framework + WebForms + SQL Server | RRAWeb, BusinessRules, AsyNewSystem |
| ipsa-ioa | `D:\ProjectCode\ipsa-ioa` | 混合 | IOA 相关模块 |
| ipsa-rms | `D:\ProjectCode\ipsa-rms` | 招聘管理 | RMS 模块 |
| rra-be | `D:\ProjectCode\rra-be` | .NET 后端 | RRA 新系统 |
| recruitment-management-be | `D:\ProjectCode\recruitment-management-be` | .NET 后端 | 招聘管理 API |
| entry-approval-mobile-be | `D:\ProjectCode\entry-approval-mobile-be` | 移动端后端 | 入职审批 |
| employment-relation-be | `D:\ProjectCode\employment-relation-be` | .NET 后端 | 劳动关系 |
| hr-api | `D:\ProjectCode\hr-api` | API 聚合 | 公共 HR 接口 |
| hr-core-api | `D:\ProjectCode\hr-core-api` | .NET 后端 | HR 核心 API |
| hr-data-sync-service | `D:\ProjectCode\hr-data-sync-service` | 同步服务 | 数据同步 |
| hr-empself-service-master | `D:\ProjectCode\hr-empself-service-master` | 员工自助 | 自助服务 |
| IbpmGAA | `D:\ProjectCode\IbpmGAA` | BPM | 流程相关 |
| ioa-fe | `D:\ProjectCode\ioa-fe` | 前端 | Vue/前端工程 |

## 未登记项目

在 `D:\ProjectCode` 下按目录名检索；读取该目录 `CLAUDE.md`/`AGENTS.md`/`README` 推断技术栈。

## 关联本地文档

设计文档输出（与代码分离），按需求文档名首横杠分组：

```
有分组：
D:\1.CursorProject\文档\<项目名>\02-技术文档\<需求分组>\<设计主题>开发设计.md

无分组：
D:\1.CursorProject\文档\<项目名>\02-技术文档\<设计主题>开发设计.md
```

细则见 [naming-rules.md](naming-rules.md)。

检索已有设计、需求副本时，**同时搜索**代码库与 `D:\1.CursorProject\文档`（含 `{需求分组}` 子目录）。
