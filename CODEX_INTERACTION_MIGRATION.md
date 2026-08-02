# Codex 对话交互逻辑完整移植计划

## 目标
将 Codex 的完整对话交互体验移植到 k-coder，包括：
1. ✅ **流式工具执行**（已完成）
2. ⏳ **任务清单（Todo List）**
3. ⏳ **实时进度显示**
4. ⏳ **思考过程可视化**
5. ⏳ **工具调用折叠/展开**
6. ⏳ **更好的错误提示**

---

## Codex 核心交互特性分析

### 1. 任务清单（Todo List）✨
**位置**: 截图中的 "任务清单 4/10 已完成"

**特性：**
- AI 自动生成任务分解（S1-1, S1-2, S2-1...）
- 实时更新状态：○ 待处理 / ✓ 已完成 / ✗ 失败
- 显示当前进度（4/10）
- 可折叠/展开

**实现方式（Codex）：**
```typescript
// 通过 TodoWrite 工具
interface Todo {
  content: string;
  status: "pending" | "in_progress" | "completed";
  activeForm: string; // "正在执行..." 的形式
}
```

### 2. 流式工具执行 ✅
**已完成**，见 `STREAMING_REFACTOR_SUMMARY.md`

### 3. 工具调用可视化
**特性：**
- 显示工具名称（Read File, Search Content）
- 显示工具参数摘要
- 可展开查看详细参数
- 显示执行时间

### 4. 思考过程（Reasoning）
**特性：**
- 显示 AI 的内部思考（extended thinking）
- 可折叠/展开
- 与用户消息区分开

### 5. 错误处理
**特性：**
- 明确的错误提示
- 重试按钮
- 错误堆栈可展开

---

## 实施计划

### 阶段 1：任务清单功能（2-3 小时）
**后端改动：**
1. 在 `protocol/mod.rs` 添加 `TodoList` 事件类型
2. 在 `agent/mod.rs` 添加 `TodoWrite` 工具
3. 修改系统提示词，引导 AI 使用任务清单

**前端改动：**
1. 在 `src/types/runtime.ts` 添加 `TodoItem` 类型
2. 在 `src/stores/workbenchStore.ts` 添加 todo 状态管理
3. 在 `src/App.tsx` 添加任务清单 UI 组件

### 阶段 2：工具调用可视化优化（1-2 小时）
**前端改动：**
1. 优化 `ToolActivity` 显示
2. 添加折叠/展开功能
3. 显示执行时间
4. 美化样式

### 阶段 3：思考过程显示（1 小时）
**前端改动：**
1. 区分 `reasoning` 和普通文本
2. 添加折叠功能
3. 使用不同颜色区分

### 阶段 4：错误处理优化（1 小时）
**前端改动：**
1. 更清晰的错误提示
2. 添加重试按钮
3. 错误详情可展开

---

## 开始实施：阶段 1 - 任务清单

### 步骤 1.1：后端添加 TodoList 协议
```rust
// src-tauri/src/protocol/mod.rs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AgentEvent {
    // ... 现有事件
    TodoUpdated {
        thread_id: String,
        turn_id: String,
        todos: Vec<TodoItem>,
    },
}
```

### 步骤 1.2：前端类型定义
```typescript
// src/types/runtime.ts

export type TodoStatus = "pending" | "in_progress" | "completed";

export interface TodoItem {
  content: string;
  status: TodoStatus;
  activeForm: string;
}

export interface TodoUpdateEvent {
  type: "todo_updated";
  threadId: string;
  turnId: string;
  todos: TodoItem[];
}
```

### 步骤 1.3：前端状态管理
```typescript
// src/stores/workbenchStore.ts

interface WorkbenchState {
  // ... 现有状态
  todos: Map<string, TodoItem[]>; // key: threadId
}

// 添加 action
setTodos: (threadId: string, todos: TodoItem[]) => void;
```

### 步骤 1.4：前端 UI 组件
```typescript
// src/components/TodoList.tsx

export function TodoList({ todos }: { todos: TodoItem[] }) {
  const completed = todos.filter(t => t.status === "completed").length;
  const total = todos.length;
  
  return (
    <div className="todo-list">
      <div className="todo-header">
        任务清单 {completed}/{total} 已完成
      </div>
      {todos.map((todo, i) => (
        <div key={i} className={`todo-item todo-${todo.status}`}>
          {todo.status === "completed" && "✓"}
          {todo.status === "in_progress" && "○"}
          {todo.status === "pending" && "○"}
          <span>{todo.content}</span>
        </div>
      ))}
    </div>
  );
}
```

---

## 优先级排序

1. **最高优先级**：任务清单（用户最关注）
2. **高优先级**：工具调用可视化优化
3. **中优先级**：思考过程显示
4. **低优先级**：错误处理优化（当前已经够用）

---

## 时间估算

- 阶段 1（任务清单）：2-3 小时
- 阶段 2（工具可视化）：1-2 小时
- 阶段 3（思考过程）：1 小时
- 阶段 4（错误处理）：1 小时

**总计：5-7 小时**

---

## 开始实施？

从**阶段 1：任务清单功能**开始？这是最核心的交互改进。
