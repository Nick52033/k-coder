# 子智能体空 parentAgentId 误报深度上限修复验证（P10-171）

## 现象

对话时间线出现失败项：

```text
创建子智能体
tool execution failed: subagent limit exceeded: maximum subagent depth is 3   耗时 16ms
```

失败发生在毫秒级，说明没有任何真实委派链被创建。

## 根因

从生产会话记录可以确认模型对可选字段 `parentAgentId` 发送了空字符串，而不是省略该字段：

```json
{"capabilities":[],"forkTurns":"all","label":"ui-test-map","parentAgentId":"",
 "task":"仅做仓库探索，不修改文件。…","timeoutMs":120000,"tokenBudget":10000}
```

`AgentToolHandler` 的 `optional_string_arg` 会把 `""` 原样传给 `MultiAgentCoordinator::create`，随后旧校验逻辑是：

```rust
if parent_agent_id.is_some_and(|id| {
    manager.get(id).map_or(true, |parent| parent.depth >= MAX_SUBAGENT_DEPTH)
}) {
    return Err(MultiAgentError::Limit("maximum subagent depth is …"));
}
```

`manager.get("")` 返回 `NotFound`，`map_or(true, …)` 把它折叠成 `true`，于是：

1. 真正的“父智能体不存在”被误报为“委派深度超限”，错误信息与事实不符；
2. 运行时把合法的直接子智能体创建请求判为失败，产生用户可见的红色失败项。

同一数据目录的 `subagents.jsonl` 中不存在 `depth` 为 2 或 3 的记录，可排除真实的深度上限触发。

## 修复

1. `MultiAgentCoordinator::create`：在领域边界把空白 `parentAgentId` 规范化为“无父智能体”（直接子智能体），不再用空 id 查询记录。
2. `validate_request`：父智能体查询失败时返回 `MultiAgentError::NotFound`，只有父智能体确实达到 `MAX_SUBAGENT_DEPTH` 时才返回 `Limit`。
3. 深度上限的提示改为可执行信息，包含上限、父智能体当前深度和可选做法（自己做或从更浅的智能体派生）。
4. 工具边界 `optional_string_arg`：空白字符串统一视为未提供，`forkTurns`、`label`、`resume_agent` 的 `message` 同步受益。
5. `create_agent` 的 `parentAgentId` 描述改为“省略该字段（不要发送空字符串）即为直接子智能体；仅在需要更深一层嵌套时填已存在的子智能体 id，嵌套上限 3 层”。

## 回归测试

- `blank_parent_agent_id_creates_a_direct_child`：`parentAgentId` 为纯空白时创建成功，`parentAgentId` 为 `null`、`depth` 为 1、`agentPath` 以 `/root/` 开头。
- `unknown_parent_agent_id_is_reported_as_missing`：不存在的父智能体返回 `NotFound`，不再返回 `Limit`。
- `blank_optional_agent_arguments_are_treated_as_absent`：空白 `parentAgentId`、`forkTurns`、`label` 都按未提供处理。
- 既有 `nested_delegation_is_allowed_up_to_the_depth_limit`、`active_count_and_delegation_depth_are_bounded` 继续通过，深度上限与并发上限行为不变。

## 验证结果

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 533 passed，0 failed |

`pnpm build` 未执行：本次改动只涉及 Rust 后端，未修改前端代码。

## 未覆盖 / 后续

- 未启动 `pnpm tauri dev` 做原生验收；需要在真实桌面会话中让模型再次发送空 `parentAgentId`，确认时间线不再出现失败项。
- 未提交、未部署安装目录。
