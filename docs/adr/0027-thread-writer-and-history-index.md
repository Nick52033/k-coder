# ADR 0027：按线程 writer 与增量历史索引

## 状态

已接受，2026-08-08。

## 背景

JSONL append 当前使用进程级全局锁，每个事件都打开文件、同步落盘、重新加载整条线程并替换 SQLite 投影。历史分页也会全量读取和投影 JSONL。长会话成本持续增长，不同 thread 还会相互阻塞。

## 决策

1. 每个 thread 使用容量有界的 writer channel 和唯一 writer task，严格保持线程内事件顺序；不同 thread writer 可并行。
2. append 必须等待 writer 返回 durable ack。JSONL 写入、flush 和 `sync_data` 成功前不得发布对应公共事件。
3. writer 复用文件句柄并在有界 drain 后关闭；发送失败、写入失败或投影失败使当前 append 失败，不得静默跳过。
4. JSONL 继续是唯一事实来源。SQLite schema 增加可重建的 event、Turn 和 Item 投影索引，用于增量摘要、用量和分页查询。
5. 每次 append 按事件增量更新投影；rollback/fork 等历史重写可以在同一 thread 内显式重建投影，但普通 append 不再整线程 delete/reinsert。
6. 应用启动和显式 rebuild 从 JSONL 重建全部投影；SQLite 损坏不能反向改写 JSONL。

## 后果

同线程保持 durable 顺序，跨线程不再共享文件 append 锁；历史页面从索引读取。实现需要处理 writer 生命周期、重建一致性和 rollback 可见性，但不会引入第二套领域事实。

