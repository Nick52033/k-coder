import { create } from "zustand";

interface ThreadSummary {
  id: string;
  title: string;
}

interface WorkbenchState {
  threads: ThreadSummary[];
  activeThreadId: string | null;
  createThread: () => void;
  selectThread: (threadId: string) => void;
}

const initialThread: ThreadSummary = {
  id: "local-foundation-thread",
  title: "新会话",
};

export const useWorkbenchStore = create<WorkbenchState>((set) => ({
  threads: [initialThread],
  activeThreadId: initialThread.id,
  createThread: () =>
    set((state) => {
      const thread: ThreadSummary = {
        id: crypto.randomUUID(),
        title: "新会话",
      };
      return {
        threads: [thread, ...state.threads],
        activeThreadId: thread.id,
      };
    }),
  selectThread: (threadId) => set({ activeThreadId: threadId }),
}));
