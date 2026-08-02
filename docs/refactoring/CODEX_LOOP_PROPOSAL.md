# Codex 风格循环机制改进方案

## 当前问题
- 硬编码 `MAX_TOOL_ITERATIONS = 15`
- 复杂任务经常超限失败
- 用户体验：看到"tool_iteration_limit exceeded"错误

## Codex 的设计哲学
1. **无硬迭代限制** - 使用 `loop` 而不是 `for iteration in 0..MAX`
2. **AI 决定何时停止** - 检查 `model_needs_follow_up`（模型是否请求更多工具调用）
3. **自动上下文压缩** - 上下文满了就压缩，继续工作
4. **信任模型** - "只要压缩工作良好，我们不需要担心无限循环"

## 改进方案对比

### 方案 A：完全学习 Codex（激进）
```rust
loop {  // 无限循环
    let response = provider.stream(...).await?;
    
    // AI 决定是否继续
    if !response.needs_follow_up {
        break;  // 模型说完成了就停止
    }
    
    // 只保留循环检测
    if repeated_calls[&tool_signature] > 3 {
        return Err("检测到循环");
    }
    
    continue;
}
```

**优点**：
- ✅ 复杂任务不会中途失败
- ✅ 完全信任 AI 模型的判断

**缺点**：
- ❌ 如果模型出 bug 可能真的无限循环
- ❌ 用户可能担心成本失控
- ❌ 风险较大

---

### 方案 B：保守改进（推荐）⭐
```rust
const MAX_TOOL_ITERATIONS: usize = 50;  // 大幅提高，但仍有上限
const PROGRESS_CHECK_WINDOW: usize = 5; // 每5轮检查进展

let mut no_progress_count = 0;
let mut last_changed_files = HashSet::new();

for iteration in 0..MAX_TOOL_ITERATIONS {
    let response = provider.stream(...).await?;
    
    // 检查是否有实际进展（新增）
    let current_changed_files = get_changed_files();
    if iteration > 0 && iteration % PROGRESS_CHECK_WINDOW == 0 {
        if current_changed_files == last_changed_files {
            no_progress_count += 1;
            if no_progress_count >= 3 {  // 连续15轮无进展
                return Err("检测到无进展，提前终止");
            }
        } else {
            no_progress_count = 0;
        }
        last_changed_files = current_changed_files;
    }
    
    // 模型说完成了就停止（新增）
    if !response.needs_follow_up {
        break;  // 尊重模型的判断
    }
    
    // 保留现有的循环检测
    if repeated_calls[&tool_signature] > MAX_IDENTICAL_TOOL_CALLS {
        return Err("检测到工具调用循环");
    }
}
```

**优点**：
- ✅ 提高限制到 50，绝大多数任务可完成
- ✅ 添加"无进展检测"，15轮没改文件就停止
- ✅ 仍有最终安全网（50 次上限）
- ✅ 尊重 AI 的 `needs_follow_up` 判断
- ✅ 用户心理更安心

**缺点**：
- ⚠️ 理论上仍可能有任务需要 50+ 轮（极少）

---

### 方案 C：动态限制
```rust
let max_iterations = match agent_mode {
    Some("ask") => 10,       // 只分析，不修改
    Some("plan") => 30,      // 需要探索
    _ => 50,                 // craft 默认
};

// 允许用户通过配置调整
if let Some(custom_limit) = config.custom_iteration_limit {
    max_iterations = custom_limit.min(100);  // 最多 100
}
```

**优点**：
- ✅ 根据模式调整限制
- ✅ 用户可自定义

**缺点**：
- ⚠️ 增加配置复杂度

---

## 我的推荐

**实施方案 B + 部分方案 C**：

1. **提高默认限制到 50**
2. **添加无进展检测**
3. **尊重模型的 `needs_follow_up`**
4. **为 Ask 模式设置更低限制 (15)**
5. **添加可选配置项** (未来扩展)

## 实施步骤

### Step 1: 修改常量
```rust
// src-tauri/src/agent/mod.rs
const MAX_TOOL_ITERATIONS: usize = 50;              // 从 15 提高到 50
const MAX_TOOL_CALLS: usize = 100;                  // 从 24 提高到 100
const MAX_IDENTICAL_TOOL_CALLS: usize = 3;          // 从 2 提高到 3
const PROGRESS_CHECK_WINDOW: usize = 5;             // 新增：每5轮检查进展
const MAX_NO_PROGRESS_WINDOWS: usize = 3;           // 新增：允许3个窗口无进展
```

### Step 2: 添加进展检测
```rust
#[derive(Clone, PartialEq, Eq, Hash)]
struct ProgressSnapshot {
    changed_files: HashSet<String>,
    tool_outputs_hash: u64,
}

impl ProgressSnapshot {
    fn from_events(events: &[StoredEvent]) -> Self {
        let changed_files = events.iter()
            .filter_map(|e| match &e.kind {
                StoredEventKind::FileChanged { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        
        let mut hasher = DefaultHasher::new();
        for event in events {
            event.hash(&mut hasher);
        }
        
        Self {
            changed_files,
            tool_outputs_hash: hasher.finish(),
        }
    }
}
```

### Step 3: 修改循环逻辑
```rust
let mut no_progress_count = 0;
let mut last_snapshot: Option<ProgressSnapshot> = None;

for iteration in 0..MAX_TOOL_ITERATIONS {
    // ... 现有代码 ...
    
    // 每5轮检查一次进展
    if iteration > 0 && iteration % PROGRESS_CHECK_WINDOW == 0 {
        let events = self.repository.load(&thread_id).await?;
        let current_snapshot = ProgressSnapshot::from_events(&events);
        
        if let Some(ref last) = last_snapshot {
            if current_snapshot == *last {
                no_progress_count += 1;
                if no_progress_count >= MAX_NO_PROGRESS_WINDOWS {
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCompleted {
                        // ... 标记为提前终止 ...
                    }));
                    return Err(AgentError::NoProgressDetected(
                        "连续多轮无实质进展，提前终止任务".to_string()
                    ));
                }
            } else {
                no_progress_count = 0;
            }
        }
        
        last_snapshot = Some(current_snapshot);
    }
    
    // ... 处理响应 ...
    
    // 如果模型没有工具调用且有文本响应，说明完成了
    if tool_calls.is_empty() && !text_content.is_empty() {
        // 模型认为任务完成，提前 break
        break;
    }
}
```

### Step 4: 为 Ask 模式降低限制
```rust
// src-tauri/src/commands/mod.rs
let max_iterations = match request.agent_mode.as_deref() {
    Some("ask") => 15,   // Ask 模式：只分析，不需要太多轮
    _ => 50,             // Plan/Craft 模式：允许更多探索
};

// 将 max_iterations 传递给 agent
```

## 预期效果

### 当前（MAX_ITERATIONS = 15）
```
任务：增加图片功能
轮次：1-5   探索代码
轮次：6-10  理解架构
轮次：11-14 开始实现
轮次：15    ❌ 超限失败！
```

### 改进后（MAX_ITERATIONS = 50 + 进展检测）
```
任务：增加图片功能
轮次：1-5   探索代码
轮次：6-10  理解架构  
轮次：11-20 实现功能
轮次：21-25 测试修复
轮次：26    ✅ 模型说完成，自动停止

或者：
轮次：1-10  反复读同一个文件
轮次：11-15 还是读同一个文件
轮次：16-20 仍然没进展
轮次：21    ❌ 检测到无进展，提前终止
```

## 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 真的跑 50 轮，成本高 | 低 | 中 | 进展检测会在 ~20 轮提前终止 |
| 进展检测误判 | 低 | 低 | 只读任务可能被误判（但 Ask 模式限制 15 轮） |
| 用户不理解为何跑这么久 | 中 | 低 | UI 显示实时进度（已有） |

## 总结

这个方案在**保守和激进之间取得平衡**：
- ✅ 大幅提高限制（15 → 50）
- ✅ 添加智能检测（无进展自动停止）
- ✅ 尊重模型判断（完成即停止）
- ✅ 保留安全网（50 次上限）
- ✅ 学习 Codex 的设计哲学

你觉得这个方案如何？要我帮你实现吗？
