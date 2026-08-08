# ADR 0029：结构化 TurnError 与显式运行状态

## 状态

已接受，2026-08-08。

## 背景

Turn 失败事件和历史主要保存字符串消息，客户端难以可靠区分限流、认证、权限、存储、协议和不可重试错误。Item 的可选终态也迫使客户端用缺失值推断运行中状态。

## 决策

1. 公共协议新增 `TurnError`，包含稳定 code、用户可见 message、retryable、category 和可选有界 details。
2. `TurnFailed`、历史 Turn 和兼容 `TurnOutcome` 使用结构化错误；旧 JSONL 字符串失败在读取时升级为 `legacy_failure`。
3. Turn 与 Item 状态显式包含 queued、in_progress 和终态。旧载荷缺少状态时按现有事实确定性推导。
4. Provider、策略、工具、存储和运行时错误映射到稳定分类；敏感请求头、API Key、完整环境变量和内部密钥不得进入载荷。
5. 前端只根据 code、category 和 retryable 决定操作入口，不解析 message 文案。

## 后果

客户端可以稳定呈现恢复和重试操作，错误文案可以独立本地化。协议和 JSONL 需要兼容升级，但旧会话继续可读。

