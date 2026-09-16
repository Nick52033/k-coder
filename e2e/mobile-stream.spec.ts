import { readFileSync } from "node:fs";

import { expect, test, type Page } from "@playwright/test";

// 手机端页面是随二进制内联的单文件资产（`include_str!`），因此这里直接读源码文件
// 当页面内容，用假 WebSocket 顶替网关那一侧。这样不需要 Rust 进程就能钉住两件
// 只有真机/真浏览器才会暴露的事：
//   1. 文本分片是否**逐条**渲染（而不是攒到 Turn 结束一次性出现）；
//   2. Turn 结束后已经流出的正文是否还在（曾经在 `turn_completed` 里被整段清空）。
const MOBILE_PAGE = readFileSync(
  new URL("../src-tauri/src/mobile/assets/mobile.html", import.meta.url),
  "utf8",
);

const CREDENTIALS = { deviceId: "device-1", deviceSecret: "secret-1" };

/** 只描述本用例用到的 Playwright `WebSocketRoute` 能力。 */
interface FakeSocket {
  send(data: string): void;
  onMessage(handler: (message: string | Buffer) => void): void;
}

interface ThreadItemFixture {
  id: string;
  kind: string;
  text?: string;
  tool?: { callId: string; name: string; state: string; excerpt?: string | null };
}

interface TurnFixture {
  id: string;
  state: string;
  error?: string | null;
  items: ThreadItemFixture[];
}

interface ThreadFixture {
  threadId: string;
  title: string;
  updatedAtMs: number;
  running: boolean;
  activeTurnId: string | null;
  turns: TurnFixture[];
  todos: unknown[];
  pendingApprovals: unknown[];
  pendingUserInputs: unknown[];
}

function emptyThread(): ThreadFixture {
  return {
    threadId: "t1",
    title: "推送代码",
    updatedAtMs: 1_700_000_000_000,
    running: false,
    activeTurnId: null,
    turns: [],
    todos: [],
    pendingApprovals: [],
    pendingUserInputs: [],
  };
}

/// 假网关：实现手机端页面真正用到的那几个方法，并允许测试主动推送事件帧。
class FakeGateway {
  readonly requests: string[] = [];
  snapshot: ThreadFixture = emptyThread();
  private socket: FakeSocket | null = null;

  attach(socket: FakeSocket): void {
    this.socket = socket;
    socket.onMessage((raw) => {
      let message: { id?: unknown; method?: string; params?: Record<string, unknown> };
      try {
        message = JSON.parse(String(raw));
      } catch {
        return;
      }
      if (typeof message.method !== "string") return;
      this.requests.push(message.method);
      const reply = (result: unknown) =>
        socket.send(JSON.stringify({ jsonrpc: "2.0", id: message.id, result }));
      switch (message.method) {
        case "initialize":
          reply({ accessToken: "token", accessTokenExpiresAtMs: Date.now() + 900_000 });
          break;
        case "thread/list":
          reply({
            threads: [
              {
                id: this.snapshot.threadId,
                title: this.snapshot.title,
                updatedAtMs: this.snapshot.updatedAtMs,
                running: this.snapshot.running,
                pendingApprovals: 0,
              },
            ],
          });
          break;
        case "thread/subscribe":
          reply({ deliverySeq: 0 });
          break;
        case "thread/read":
          reply({ thread: this.snapshot });
          break;
        default:
          // ping / initialized 等通知没有响应体。
          if (message.id !== undefined) reply({});
          break;
      }
    });
  }

  /// 推送一条领域事件。字段名与服务端 `AgentEventEnvelope` 的 camelCase 投影一致。
  emit(params: Record<string, unknown>): void {
    this.socket?.send(JSON.stringify({ jsonrpc: "2.0", method: "event", params }));
  }
}

async function openChat(page: Page, gateway: FakeGateway): Promise<void> {
  await page.addInitScript(
    (credentials) => {
      localStorage.setItem("k-coder-mobile-credentials", JSON.stringify(credentials));
    },
    CREDENTIALS,
  );
  await page.route("**/m/**", (route) =>
    route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body: MOBILE_PAGE }),
  );
  await page.routeWebSocket("**/ws", (socket) => gateway.attach(socket));
  await page.goto("/m/", { waitUntil: "load" });
  await expect(page.locator("#threadList .thread")).toHaveCount(1);
  await page.locator("#threadList .thread").first().click();
  await expect(page.locator("#chatSection")).toBeVisible();
}

function userMessage(text: string): Record<string, unknown> {
  return {
    schemaVersion: 1,
    id: "message-1",
    role: "user",
    createdAtMs: 0,
    content: [{ type: "text", text }],
  };
}

function assistantMessage(text: string): Record<string, unknown> {
  return {
    schemaVersion: 1,
    id: "message-2",
    role: "assistant",
    createdAtMs: 1,
    content: [{ type: "text", text }],
  };
}

test("文本分片逐条呈现，且只续写同一个气泡节点", async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => consoleErrors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });

  const gateway = new FakeGateway();
  await openChat(page, gateway);

  gateway.emit({
    type: "turn_started",
    threadId: "t1",
    turnId: "turn-1",
    userMessage: userMessage("推送代码"),
  });
  await expect(page.locator("#timeline .bubble.user")).toHaveText("推送代码");

  const bubble = page.locator("#timeline .bubble.agent");
  const chunks = ["我先", "确认当前", "工作区状态", "和待推送内容。"];
  for (let index = 0; index < chunks.length; index += 1) {
    gateway.emit({
      type: "text_delta",
      threadId: "t1",
      turnId: "turn-1",
      itemId: "item-assistant",
      delta: chunks[index],
    });
    // 每个分片都必须立刻可读；攒到最后一次性出现的话这条断言必然先失败。
    await expect(bubble).toHaveText(chunks.slice(0, index + 1).join(""));
  }

  // 分片是往同一个节点上追加文本，而不是每个分片重建整个时间线。
  await page.evaluate(() => {
    document.querySelector("#timeline .bubble.agent")?.setAttribute("data-probe", "kept");
  });
  gateway.emit({
    type: "text_delta",
    threadId: "t1",
    turnId: "turn-1",
    itemId: "item-assistant",
    delta: "（续）",
  });
  await expect(bubble).toHaveText(chunks.join("") + "（续）");
  await expect(page.locator('#timeline .bubble.agent[data-probe="kept"]')).toHaveCount(1);
  expect(consoleErrors).toEqual([]);
});

test("Turn 结束后已流出的正文仍然保留且不重复", async ({ page }) => {
  const gateway = new FakeGateway();
  await openChat(page, gateway);

  const answer = "我先确认当前工作区状态和待推送内容。";
  gateway.emit({
    type: "turn_started",
    threadId: "t1",
    turnId: "turn-1",
    userMessage: userMessage("推送代码"),
  });
  for (const delta of ["我先确认", "当前工作区状态", "和待推送内容。"]) {
    gateway.emit({
      type: "text_delta",
      threadId: "t1",
      turnId: "turn-1",
      itemId: "item-assistant",
      delta,
    });
  }

  gateway.emit({
    type: "turn_completed",
    threadId: "t1",
    turnId: "turn-1",
    message: assistantMessage(answer),
    startedAtMs: 0,
    completedAtMs: 1,
    durationMs: 1,
  });

  const bubble = page.locator("#timeline .bubble.agent");
  await expect(bubble).toHaveCount(1);
  await expect(bubble).toHaveText(answer);

  // Turn 结束后端会补一次持久化，客户端会静默重读一次：快照里已经有了这条助手
  // 消息，此时不能再多出第二条气泡。
  gateway.snapshot = {
    ...emptyThread(),
    turns: [
      {
        id: "turn-1",
        state: "completed",
        items: [
          { id: "message-1", kind: "user_message", text: "推送代码" },
          { id: "item-assistant", kind: "agent_message", text: answer },
        ],
      },
    ],
  };
  await expect(page.locator("#timeline .bubble.user")).toHaveCount(1);
  await expect(bubble).toHaveCount(1);
  await expect(bubble).toHaveText(answer);
  await expect(page.locator("#turnState")).toHaveText("空闲");
});

test("快照与增量按条目身份合并：用户消息与工具卡片都不会重复", async ({ page }) => {
  const gateway = new FakeGateway();
  await openChat(page, gateway);

  gateway.emit({
    type: "turn_started",
    threadId: "t1",
    turnId: "turn-1",
    userMessage: userMessage("推送代码"),
  });
  gateway.emit({
    type: "tool_started",
    threadId: "t1",
    turnId: "turn-1",
    call: { id: "call-1", name: "run_command", arguments: null, metadata: null },
  });
  const tool = page.locator("#timeline details.tool");
  await expect(tool).toHaveCount(1);
  await expect(tool.locator("summary")).toContainText("运行命令");
  await expect(tool.locator("summary .pill")).toHaveText("运行中");

  gateway.emit({
    type: "tool_completed",
    threadId: "t1",
    turnId: "turn-1",
    callId: "call-1",
    name: "run_command",
    result: { success: true, output: "## codex/robot-workflow-skills\nM package.json" },
  });
  await expect(tool.locator("summary .pill")).toHaveText("完成");
  await expect(tool.locator("pre")).toContainText("M package.json");

  // 重读会话后：同一条用户消息与同一个工具调用（按 callId 对齐）都只保留一份。
  gateway.snapshot = {
    ...emptyThread(),
    turns: [
      {
        id: "turn-1",
        state: "running",
        items: [
          { id: "message-1", kind: "user_message", text: "推送代码" },
          {
            id: "item-tool",
            kind: "tool",
            tool: { callId: "call-1", name: "run_command", state: "completed", excerpt: "M package.json" },
          },
        ],
      },
    ],
  };
  await page.locator("#reloadThread").click();
  await expect(page.locator("#timeline .bubble.user")).toHaveCount(1);
  await expect(page.locator("#timeline details.tool")).toHaveCount(1);
  await expect(page.locator("#timeline details.tool pre")).toContainText("M package.json");
});

test("输入区高度被测量并让出底部空间，长草稿不会撑破布局", async ({ page }) => {
  const gateway = new FakeGateway();
  await openChat(page, gateway);

  const draft = Array.from({ length: 12 }, (_, index) => `第 ${index + 1} 行草稿`).join("\n");
  await page.locator("#input").fill(draft);
  await expect(page.locator("#input")).toHaveValue(draft);

  const measured = await page.evaluate(() => {
    const composer = document.getElementById("composer") as HTMLElement;
    const reserved = getComputedStyle(document.documentElement).getPropertyValue("--composer-h");
    return {
      composerHeight: composer.getBoundingClientRect().height,
      mainPaddingBottom: parseFloat(getComputedStyle(document.querySelector("main") as HTMLElement).paddingBottom),
      reserved: parseFloat(reserved),
    };
  });

  // 正文区必须为输入区让出空间，否则最后一条消息会被输入框永久压住。
  expect(measured.reserved).toBeGreaterThan(0);
  expect(Math.abs(measured.mainPaddingBottom - measured.composerHeight)).toBeLessThan(40);
  await expect(page.locator("#sendButton")).toBeVisible();
});
