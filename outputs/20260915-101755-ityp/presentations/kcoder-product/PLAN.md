# k-Coder 产品介绍 — Deck Plan

Profile: product-platform (SaaS / product / platform / security / workflow / ecosystem)
Audience: 技术决策者、资深工程师、平台负责人
Occasion: 产品能力介绍（约 8 分钟讲述）
Deck job: 让受众理解 k-Coder 不是「Shell 加聊天壳」，而是一个具备受控执行、可审计治理和多智能体协作的桌面编程智能体平台。

## Title

k-Coder · 受控的桌面编程智能体平台
Source line: 产品介绍 · 基于仓库路线图与架构文档 · 2026-09

## Claim spine（每页一个主张，含证据锚点）

| # | Section | Claim | Evidence anchor |
|---|---|---|---|
| 1 | Cover | 标题页 | docs/开发路线图.md 当前位置 |
| 2 | 问题 | 通用编码助手把执行权交给模型，风险与不可复现同时放大 | docs/架构.md 目标段 |
| 3 | 定位 | k-Coder 是桌面运行时，不是 Shell 的聊天包装层 | docs/架构.md 首段 |
| 4 | 系统 | 四层职责分离：UI / Tauri 边界 / agent 协调 / providers 协议转换 | docs/架构.md 模块边界 + AGENTS.md 架构边界 |
| 5 | 执行治理 | 每次工具执行都经过策略判定、审批、取消与超时清理 | AGENTS.md 安全约束；Phase 3/4 门槛 |
| 6 | 安全 | 路径规范化、工作区逃逸拒绝、密钥不落日志 | AGENTS.md 安全约束 |
| 7 | 上下文 | 长对话、大仓库、崩溃与升级下保持可靠 | Phase 5 目标与门槛 |
| 8 | 扩展 | 扩展面收敛到同一 ExtensionService，插件不获得隐含信任 | docs/adr/0040；docs/扩展.md |
| 9 | 多智能体 | 委派复用同一运行时，权限、取消与预算随子任务继承 | Phase 8；P10-160/161/162 |
| 10 | 成熟度 | 以证据而非功能计数判断成熟度 | Phase 11 目标；Phase 10 生产加固 |
| 11 | Adoption | 从单仓试点到团队平台的落地路径 | 由前述能力推导 |
| 12 | Close | 下一步与要求 | — |

## Design system

- Ground: 深色墨蓝 `#0B1220`，纸白卡片 `#F7F9FC`
- Accent: 执行/治理用琥珀 `#F0A048`，证据/通过用青绿 `#37B7A4`
- 字体：Arimo（Latin 与数字），工作区中文回退由渲染器处理；所有数值保持同一字体家族
- 版式：左对齐栅格，页眉 claim 一行，页脚页码 + section
- 禁止：圆角卡片堆叠、渐变按钮、阴影浮夸、每页三层卡片、装饰性大数字

## Contact sheet plan（12 页，每页独立主张）

1. Cover — 全幅深色，左对齐标题块，右下极小日期
2. 风险随产能放大 — 双列对照（通用助手 / 受控运行时）
3. 定位 — 单列强主张 + 三条支撑
4. 系统 — 横向四层带状图，箭头表示单向依赖
5. 执行治理 — 时间轴：请求 → 策略 → 审批 → 执行 → 审计
6. 安全 — 边界示意：工作区内外 + 拒绝点标注
7. 上下文 — 三列证据条（长对话 / 大仓库 / 恢复）
8. 扩展 — 插件目录 → ExtensionService → 统一能力面
9. 多智能体 — 主运行时为中心，子任务辐射并标注继承的约束
10. 成熟度 — 阶梯：Phase 10 加固 → Phase 11 证据复核
11. Adoption — 三阶段路径
12. Close — 单句主张 + 下一步

## Verified environment

- artifact-tool runtime 2.8.59（node 与 python 均由运行时提供）
- 渲染器在 Windows 原生下可能以 -1073740791 退出，需在报告中如实记录
