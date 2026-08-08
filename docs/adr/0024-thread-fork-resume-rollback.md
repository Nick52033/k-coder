# ADR 0024：Thread fork、resume、rollback 与兼容边界

## 状态

已接受，2026-08-08。

## 背景

完成 Thread/Turn/Item、异步启动、mailbox 和精确控制后，线程仍缺少显式的分支、恢复和历史回退契约。直接复制 JSONL 会复制线程元数据、活动状态甚至可撤销工作区变更；直接删除事件则会破坏审计。对话回退若顺带修改本地文件，还会绕过既有可审阅 change transaction。

同时，旧事件 schema 和阻塞式 `run_turn` 仍需明确兼容边界，避免为了新命令创建第二套历史或智能体循环。

## 决策

1. JSONL schema 升至 8，新增 `ThreadForked` 和 `ThreadRolledBack` 审计事件。旧 schema 继续由既有升级读取路径恢复，不要求重写原文件。
2. `thread_fork` 创建绑定同一规范工作区的新 thread，复制源线程回放后的全部有效对话历史，或复制至指定已完成 `lastTurnId`。原始 JSONL 中已被 rollback 标记隐藏的 Turn 不得重新进入分支。活动 Turn 不能作为分支锚点；源线程有活动 Turn 或 mailbox 中仍有 pending Turn 时拒绝 fork。Turn 接纳、worker 取项和生命周期操作必须按 ADR 0026 在同一 thread operation gate 下串行。
3. fork 为复制事件生成新的事件 ID 和目标 thread ID，但保留历史 Turn、Item 与消息 ID 在目标 thread 作用域内的关联。线程创建/归档元数据不复制。
4. `ChangeApplied`、`ChangeUndone` 和 Change Item 生命周期不进入分支，避免两个 thread 同时声称拥有同一工作区变更的撤销权。分支仍能看到相关助手文本，但工作区真实状态由当前磁盘和原变更审计决定。
5. `thread_resume` 复用统一历史读取和既有孤儿 Turn/交互恢复，不创建第二套恢复流程。
6. `thread_rollback` 要求 `numTurns >= 1`、thread 无活动 Turn 且 mailbox 无 pending 输入。它先回放已有 rollback 标记，再按当前有效历史中的完整终态 Turn 计算保留边界，追加审计标记；原 JSONL 记录保留供审计，连续 rollback 不得重新计入已隐藏的 Turn。
7. rollback 只回退对话历史，不修改工作区文件。需要恢复文件时必须显式使用受哈希保护、可审阅且有独立审计的 change undo。
8. 公共事件 schema 升至 v4，`turn_started.userMessage` 为可选兼容字段；旧 v1-v3 事件缺少该字段时仍可展示和恢复。阻塞 `run_turn`/`TurnOutcome` 在 1.0 前保留为迁移入口，正常桌面客户端只使用 `turn_start`、事件和历史查询；兼容入口继续复用同一执行函数，不能获得独立 mailbox 或控制语义。

## 后果

线程可以在明确的完成 Turn 边界分支、恢复和回退，同时保留追加式审计。对话历史操作不会暗中覆盖用户文件，也不会复制撤销能力。旧会话和旧事件继续可读；代价是 rollback 后的原始事件仍占用存储空间，fork 也会复制选定的对话事实，后续如需压缩必须另行设计可审计的归档流程。
