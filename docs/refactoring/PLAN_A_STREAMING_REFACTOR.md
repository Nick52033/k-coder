# 方案 A：流式工具执行重构

> 状态：并发调度部分已由 `P10-033` 取代。运行时继续流式发布当前工具的进度，但同一次 Provider 响应中的多个工具现在按返回顺序串行执行。

## 目标
将 k-coder 的同步批量工具执行改造为 Codex 风格的异步流式执行，让用户实时看到每个工具的执行进度。

## 当前架构 vs 目标架构

### 当前（同步批量）
```
AI 输出完成 → 收集所有 ToolCall
              ↓
          执行工具 1
              ↓
          执行工具 2
              ↓
          执行工具 N
              ↓
       返回结果给 AI
```

### 目标（异步流式）
```
AI 输出中... ──→ 收到 ToolCall 1 ──→ 立即启动异步执行 ──→ 继续接收 AI 输出
                      ↓                      ↓
                 发送进度事件          收到 ToolCall 2 ──→ 立即启动
                      ↓                      ↓
                 完成后发送结果          并发执行中...
```

## 核心改动

### 1. 引入 FuturesOrdered（类似 Codex）
```rust
use futures::stream::FuturesOrdered;

let mut in_flight: FuturesOrdered<BoxFuture<'static, Result<ToolExecutionResult>>> = 
    FuturesOrdered::new();
```

### 2. 修改主循环结构
**当前：**
```rust
loop {
    // 1. 收集所有 ToolCall
    let mut calls = Vec::new();
    while let Some(event) = stream.next().await {
        if let ProviderEvent::ToolCall { call } = event {
            calls.push(call);
        }
    }
    
    // 2. 批量执行
    for call in calls {
        let result = execute_tool(call).await;
    }
}
```

**目标：**
```rust
loop {
    tokio::select! {
        // 分支 1：接收 AI 流式输出
        event = stream.next() => {
            match event {
                ProviderEvent::ToolCall { call } => {
                    // 立即启动异步执行
                    let future = spawn_tool_execution(call);
                    in_flight.push(future);
                    // 继续接收下一个事件
                }
                ProviderEvent::Completed => break;
            }
        }
        
        // 分支 2：处理完成的工具
        result = in_flight.next(), if !in_flight.is_empty() => {
            // 工具执行完成，发送结果
            persist_tool_result(result).await;
        }
    }
}
```

### 3. 工具执行改造
**当前：**
```rust
async fn execute_tool_call(&self, call: &ToolCall) -> Result<ToolResult> {
    // 同步执行
    self.tools.dispatch(call).await
}
```

**目标：**
```rust
fn spawn_tool_execution(&self, call: ToolCall) -> BoxFuture<'static, ToolExecutionResult> {
    let tools = self.tools.clone();
    let publisher = self.publisher.clone();
    
    Box::pin(async move {
        // 发送开始事件
        publisher.publish(ToolStarted { call: call.clone() });
        
        // 执行工具
        let result = tools.dispatch(&call).await;
        
        // 返回结果（不发送事件，由主循环统一处理）
        ToolExecutionResult {
            call,
            result,
        }
    })
}
```

## 实施步骤

### 阶段 1：准备工作（30 分钟）✅
- [x] 分析 Codex 源码
- [x] 添加必要的依赖（futures crate）
- [x] 创建新的类型定义

### 阶段 2：核心重构（3-4 小时）✅
- [x] 创建 `ToolExecutionResult` 结构体
- [x] 创建 `spawn_tool_execution` 方法返回 Future
- [x] 重写主循环，使用 `FuturesOrdered`
- [x] 实现异步工具执行管理

### 阶段 3：事件流优化（1-2 小时）✅
- [x] 在工具启动时立即发送 `ToolStarted` 事件
- [x] 在工具完成时通过主循环发送事件
- [x] 确保事件顺序正确

### 阶段 4：测试验证（2-3 小时）⏳ 进行中
- [ ] 测试简单任务（单个工具）
- [ ] 测试复杂任务（多个工具并发）
- [ ] 测试错误处理
- [ ] 测试取消机制

### 阶段 5：边缘情况（1-2 小时）
- [x] 处理工具执行失败
- [x] 处理用户取消
- [ ] 处理超时
- [x] 处理进度检测逻辑兼容

## 技术细节

### 新增依赖
```toml
[dependencies]
futures = "0.3"  # 已有
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync"] }  # 已有
```

### 新增类型
```rust
use std::pin::Pin;
use futures::Future;

pub type ToolExecutionFuture = Pin<Box<dyn Future<Output = ToolExecutionResult> + Send + 'static>>;

pub struct ToolExecutionResult {
    pub call: ToolCall,
    pub result: Result<ToolResult, AgentRuntimeError>,
}
```

## 风险和缓解

### 风险 1：并发执行导致竞态条件
**缓解：** 使用 `FuturesOrdered` 保证执行顺序，结果按启动顺序返回

### 风险 2：事件发送顺序混乱
**缓解：** 所有事件通过统一的 publisher 发送，保证原子性

### 风险 3：破坏现有功能
**缓解：** 逐步重构，保留原有逻辑作为参考

### 风险 4：内存泄漏
**缓解：** 确保所有 Future 都会被 poll 完成或取消

## 测试计划

### 测试用例 1：单工具执行
```
输入：读取一个文件
预期：立即看到 "正在读取文件..." → 看到文件内容
```

### 测试用例 2：多工具并发
```
输入：读取 3 个文件
预期：同时看到 3 个 "正在读取..." → 依次完成
```

### 测试用例 3：工具失败
```
输入：读取不存在的文件
预期：看到错误提示，任务继续
```

### 测试用例 4：用户取消
```
输入：执行长时间任务 → 用户点击取消
预期：所有工具立即停止，清理资源
```

## 成功标准

✅ 用户能实时看到每个工具的执行状态
✅ 多个工具能并发执行（如果适用）
✅ 响应时间明显改善（不再有"loading 消失"的问题）
✅ 所有现有功能正常工作
✅ 没有引入内存泄漏或资源泄漏

## 回滚计划

如果重构失败，可以：
1. Git revert 到重构前
2. 或者保留重构代码，添加 feature flag 切换
3. 实施方案 B 作为临时方案
