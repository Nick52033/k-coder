import type { TodoItem } from "../types/runtime";
import { CheckCircle2, Circle } from "lucide-react";
import "./TodoList.css";

interface TodoListProps {
  todos: TodoItem[];
}

export function TodoList({ todos }: TodoListProps) {
  if (!todos || todos.length === 0) {
    return null;
  }

  const completed = todos.filter((t) => t.status === "completed").length;
  const total = todos.length;

  return (
    <div className="todo-list">
      <div className="todo-header">
        <span className="todo-icon">☰</span>
        <span className="todo-title">任务清单 {completed}/{total} 已完成</span>
      </div>
      <div className="todo-items">
        {todos.map((todo, index) => (
          <div
            key={index}
            className={`todo-item todo-${todo.status}`}
          >
            <div className="todo-status-icon">
              {todo.status === "completed" && <CheckCircle2 size={16} className="todo-icon-completed" />}
              {todo.status === "in_progress" && <Circle size={16} className="todo-icon-progress" />}
              {todo.status === "pending" && <Circle size={16} className="todo-icon-pending" />}
            </div>
            <div className="todo-content">
              {todo.status === "completed" && <s>{todo.content}</s>}
              {todo.status === "in_progress" && <span className="todo-active">{todo.activeForm}</span>}
              {todo.status === "pending" && <span>{todo.content}</span>}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
