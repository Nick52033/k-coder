# ADR 0026：Thread 生命周期操作门

## 状态

已接受，2026-08-08。

## 背景

`turn_start`、mailbox worker、fork、rollback 和恢复操作分别检查活动 Turn、pending 输入和历史状态。检查与后续操作不共享同一按线程临界区时，新输入可能在检查后进入，造成 fork/rollback 观察到不稳定边界。

## 决策

1. `AppState` 持有按 thread 的 operation gate。Turn 入队、worker 取项、fork、rollback 和需要稳定线程边界的恢复操作必须通过该 gate。
2. gate 只协调线程生命周期，不进入 Provider、工具或授权实现；不同 thread 的 gate 可以并行。
3. 固定锁序为 operation gate、活动 Turn、mailbox。持有后重新校验所有前置条件，`commands/` 不再组合“先检查再调用 Repository”。
4. fork/rollback 持有 gate 直到 Repository 操作完成。worker 只有在 gate 内成功取出工作项后才可以开始 Turn。
5. gate 条目使用弱引用或在空闲后清理，不能随历史 thread 数量无限增长。

## 后果

同一 thread 的生命周期转换得到确定性顺序；跨 thread 并行不受影响。代价是长时间 fork/rollback 会短暂阻塞该 thread 的新输入接纳，但不会阻塞其他线程。

