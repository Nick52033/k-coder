# AGENTS.md

本说明适用于整个 k-Coder 仓库。

## 必读文档

修改代码前必须阅读：

1. `docs/开发路线图.md`
2. `docs/架构.md`
3. `docs/adr/` 下与当前工作相关的 ADR

## 开发流程

1. 在路线图中找到“当前位置”。
2. 除非用户明确调整优先级，否则只处理其中列出的“下一项任务”。
3. 当前阶段未通过验收门槛时，不得提前开发后续阶段功能。
4. 每个公共契约和安全分支都必须新增或更新测试。
5. 执行当前阶段规定的验证命令。
6. 完成任务前同步更新路线图复选框、当前位置和变更记录。

## 架构边界

- `src/` 只包含界面和展示状态。
- `src-tauri/src/commands/` 是轻量 Tauri 边界，不得包含智能体循环逻辑。
- `agent/` 负责协调 Turn，不实现具体模型协议或工具行为。
- `providers/` 负责把外部模型协议转换为内部 Provider 事件。
- `tools/` 负责工具 Schema 和处理器，授权决策属于 `policy/`。
- `storage/` 负责持久化领域事件并提供 Repository，界面不得直接访问数据库。
- `protocol/` 存放跨边界传输的版本化载荷。
- 子智能体必须复用主智能体运行时，不得另写第二套智能体循环。

## 安全约束

- 文件访问前必须解析并规范化所有路径。
- 必须拒绝工作区之外的路径，包括符号链接和目录联接逃逸。
- 模型传入的权限字段不能作为授权依据。
- Shell 命令必须经过策略判断，并支持超时、取消和进程树清理。
- 禁止记录 API Key、授权请求头、完整环境变量或用户密钥。
- 破坏性操作必须经过明确审批，并生成可审计事件。

## 质量门槛

至少执行：

```powershell
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

未启动 `pnpm tauri dev` 并实际验证变更路径时，不得声称桌面工作流已经完成。
