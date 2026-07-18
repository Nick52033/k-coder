import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  CircleAlert,
  Code2,
  FolderOpen,
  MessageSquare,
  PanelRight,
  Plus,
  SendHorizontal,
  Settings,
} from "lucide-react";
import { useWorkbenchStore } from "./stores/workbenchStore";
import type { RuntimeStatus } from "./types/runtime";
import "./App.css";

function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [runtimeError, setRuntimeError] = useState("");
  const [draft, setDraft] = useState("");
  const { threads, activeThreadId, createThread, selectThread } = useWorkbenchStore();

  useEffect(() => {
    invoke<RuntimeStatus>("runtime_status")
      .then(setRuntime)
      .catch((error: unknown) => setRuntimeError(String(error)));
  }, []);

  const activeThread = threads.find((thread) => thread.id === activeThreadId) ?? null;

  function submitMessage(event: FormEvent) {
    event.preventDefault();
    if (!draft.trim()) return;
  }

  return (
    <main className="workbench">
      <header className="titlebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true"><Code2 size={17} /></span>
          <strong>k-Coder</strong>
        </div>
        <div className="titlebar-actions">
          <span className={`runtime-state ${runtimeError ? "runtime-state--error" : ""}`}>
            {runtimeError ? <CircleAlert size={14} /> : <Activity size={14} />}
            {runtimeError ? "运行时不可用" : runtime ? "运行时就绪" : "正在连接"}
          </span>
          <button className="icon-button" type="button" aria-label="设置" title="设置">
            <Settings size={17} />
          </button>
        </div>
      </header>

      <aside className="sidebar">
        <button className="new-thread-button" type="button" onClick={createThread}>
          <Plus size={16} />
          新建会话
        </button>
        <div className="section-label">会话</div>
        <nav className="thread-list" aria-label="会话列表">
          {threads.map((thread) => (
            <button
              className={`thread-item ${thread.id === activeThreadId ? "thread-item--active" : ""}`}
              key={thread.id}
              type="button"
              onClick={() => selectThread(thread.id)}
            >
              <MessageSquare size={15} />
              <span>{thread.title}</span>
            </button>
          ))}
        </nav>
        <button className="workspace-button" type="button">
          <FolderOpen size={16} />
          <span>未选择工作区</span>
        </button>
      </aside>

      <section className="conversation">
        <div className="conversation-header">
          <div>
            <h1>{activeThread?.title ?? "新会话"}</h1>
            <span className="mode-label">智能体</span>
          </div>
          <button className="icon-button" type="button" aria-label="切换活动面板" title="切换活动面板">
            <PanelRight size={17} />
          </button>
        </div>

        <div className="message-area">
          <div className="empty-thread">
            <Code2 size={26} />
            <p>暂无消息</p>
          </div>
        </div>

        <form className="composer" onSubmit={submitMessage}>
          <textarea
            aria-label="消息"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="输入消息"
            rows={3}
          />
          <div className="composer-footer">
            <span className="composer-mode">智能体</span>
            <button
              className="send-button"
              type="submit"
              aria-label="发送消息"
              title="发送消息"
              disabled={!draft.trim()}
            >
              <SendHorizontal size={17} />
            </button>
          </div>
        </form>
      </section>

      <aside className="activity-panel">
        <div className="activity-header">
          <Activity size={16} />
          <h2>活动</h2>
        </div>
        <div className="activity-list">
          <div className="activity-row">
            <span className={`activity-dot ${runtime ? "activity-dot--success" : ""}`} />
            <div>
              <strong>运行时</strong>
              <span>{runtime ? `v${runtime.version}` : "等待中"}</span>
            </div>
          </div>
          <div className="activity-row">
            <span className="activity-dot" />
            <div>
              <strong>工作区</strong>
              <span>未选择</span>
            </div>
          </div>
        </div>
      </aside>
    </main>
  );
}

export default App;
