import { expect, test } from "@playwright/test";

import mobileDto from "./fixtures/mobile-dto.json" with { type: "json" };

type Capability =
  | "chat"
  | "approval"
  | "interrupt"
  | "fileRead"
  | "shell"
  | "settings"
  | "plugins"
  | "secrets";

interface DeviceFixture {
  id: string;
  name: string;
  platform: string | null;
  createdAtMs: number;
  lastSeenAtMs: number;
  revoked: boolean;
}

interface PendingFixture {
  id: string;
  deviceName: string;
  platform: string | null;
  createdAtMs: number;
  expiresAtMs: number;
}

interface StatusFixture {
  running: boolean;
  host: string | null;
  port: number | null;
  scheme: string | null;
  fingerprint: string | null;
  lanAddresses: string[];
  preferredBindAddress: string | null;
  preferredPort: number;
  connections: number;
  capabilities: Capability[];
  pairing: unknown;
  pendingPairings: PendingFixture[];
  devices: DeviceFixture[];
}

// 桩数据来自 `e2e/fixtures/mobile-dto.json`，与 Rust 侧
// `src-tauri/tests/mobile_dto_contract.rs` 共用同一份线上形状。Rust 端改了字段名，
// 那边会先失败；这里跟着同一份数据重新校验前端读得对不对，两边不会再各自漂移。
const FINGERPRINT = mobileDto.pairing.fingerprint;

const DEVICE = mobileDto.device as DeviceFixture;

const PENDING = mobileDto.pending as PendingFixture;

// 待确认请求不再挂在 pairing 下面：挑战一旦被手机提交就会被服务端标记为已消费，
// `pairing` 随之变成 null，挂在它下面的列表会连同挑战一起消失（见
// `submitted_pairing_stays_visible_after_the_challenge_is_consumed`）。
const PAIRING = mobileDto.pairing;

const BASE_STATUS = mobileDto.status as unknown as StatusFixture;

const THREAD = {
  schemaVersion: 1,
  id: "thread-1",
  title: "Mobile control plane",
  createdAtMs: 1,
  updatedAtMs: 2,
  archived: false,
  inProject: true,
  workspacePath: "D:\\code\\k-coder",
};

const CATALOG = {
  schemaVersion: 1,
  activeProviderId: "openai",
  providers: [
    {
      schemaVersion: 1,
      id: "openai",
      kind: "open_ai_compatible",
      transport: "open_ai_chat_completions",
      name: "OpenAI",
      baseUrl: "https://api.openai.com/v1",
      model: "gpt-4.1",
      models: [{ id: "gpt-4.1", displayName: "GPT-4.1", contextWindow: 128000, fallback: false }],
      endpoints: [],
      hasApiKey: false,
    },
  ],
};

const WORKSPACE = {
  current: { id: "project-1", name: "k-coder", path: "D:\\code\\k-coder", trusted: true, lastOpenedAtMs: 2 },
  recent: [],
};

test("mobile settings page drives gateway, pairing and device control", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 820 });

  const initialStatus: StatusFixture = {
    ...BASE_STATUS,
    pairing: PAIRING,
    pendingPairings: [PENDING],
    devices: [DEVICE],
  };

  await page.addInitScript((fixture) => {
    const callbacks = new Map<number, (...args: unknown[]) => void>();
    let callbackId = 1;
    const state = JSON.parse(JSON.stringify(fixture.status)) as StatusFixture;
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    (window as unknown as { __mobileCalls: unknown }).__mobileCalls = calls;

    const record = (command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args });
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback: (callback: (...args: unknown[]) => void) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args?: Record<string, unknown>) => {
        if (command === "plugin:event|listen") return 1;
        if (command === "runtime_status") {
          return { ready: true, phase: "agent", version: "0.10.0", uptimeSeconds: 1, capabilities: [] };
        }
        if (command === "get_approval_mode") return "ask";
        if (command === "get_reasoning_effort") return "medium";
        if (command === "get_provider_catalog") return fixture.catalog;
        if (command === "list_builtin_workflows") return [];
        if (command === "list_threads") return [fixture.thread];
        if (command === "read_thread_history") return null;
        if (command === "read_thread") return null;
        if (command === "get_plan" || command === "get_goal" || command === "get_workflow_run") return null;
        if (command === "read_thread_mailbox") {
          return { schemaVersion: 1, threadId: fixture.thread.id, revision: 0, activeTurnId: null, pending: [] };
        }
        if (command === "list_subagents") return [];
        if (command === "workspace_state") return fixture.workspace;
        if (command === "list_workspace_directory") return [];
        if (command === "git_status") {
          return { isRepository: true, branch: "main", upstream: null, ahead: 0, behind: 0, files: [] };
        }
        if (command === "git_branches") return { current: "main", branches: ["main"] };

        if (!command.startsWith("mobile_")) return null;
        record(command, args);

        if (command === "mobile_status") return state;
        if (command === "mobile_stop") {
          state.running = false;
          state.host = null;
          state.port = null;
          state.scheme = null;
          state.fingerprint = null;
          state.pairing = null;
          return state;
        }
        if (command === "mobile_start") {
          const bind = (args?.bindAddress as string | null) ?? null;
          state.running = true;
          state.host = bind ?? "127.0.0.1";
          state.port = (args?.port as number | null) ?? 8787;
          state.scheme = bind ? "https" : "http";
          state.fingerprint = bind ? fixture.fingerprint : null;
          return state;
        }
        if (command === "mobile_create_pairing") {
          state.pairing = JSON.parse(JSON.stringify(fixture.pairing)) as unknown;
          // 与服务端 `PairingStore::create_challenge` 一致：新挑战会让旧的待确认请求失效。
          state.pendingPairings = [];
          return state.pairing;
        }
        if (command === "mobile_approve_pairing") {
          const pendingId = args?.pendingId as string;
          const request = state.pendingPairings.find((item) => item.id === pendingId) ?? null;
          state.pendingPairings = state.pendingPairings.filter((item) => item.id !== pendingId);
          const approved: DeviceFixture = {
            id: pendingId,
            name: request?.deviceName ?? pendingId,
            platform: request?.platform ?? null,
            createdAtMs: 1_760_000_700_000,
            lastSeenAtMs: 0,
            revoked: false,
          };
          state.devices = [...state.devices, approved];
          return approved;
        }
        if (command === "mobile_deny_pairing") {
          const pendingId = args?.pendingId as string;
          state.pendingPairings = state.pendingPairings.filter((item) => item.id !== pendingId);
          return null;
        }
        if (command === "mobile_revoke_device") {
          const deviceId = args?.deviceId as string;
          state.devices = state.devices.map((device) =>
            device.id === deviceId ? { ...device, revoked: true } : device,
          );
          return state.devices.find((device) => device.id === deviceId) ?? null;
        }
        if (command === "mobile_set_capabilities") {
          state.capabilities = (args?.capabilities as Capability[]) ?? [];
          return state;
        }
        return null;
      },
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: () => undefined,
    };
  }, {
    status: initialStatus,
    pairing: PAIRING,
    fingerprint: FINGERPRINT,
    thread: THREAD,
    catalog: CATALOG,
    workspace: WORKSPACE,
  });

  await page.goto("/");
  await page.locator('button[aria-label="设置"]:visible').click();
  const settings = page.getByRole("dialog", { name: "设置" });
  await settings.getByRole("button", { name: "移动设备" }).click();

  // 概览：运行中的地址、证书指纹与设备列表。
  await expect(settings.getByRole("heading", { name: "移动设备" })).toBeVisible();
  await expect(settings.locator(".mobile-settings__card-head .mobile-settings__pill.is-on")).toHaveText(
    "https://192.168.1.20:8787",
  );
  await expect(settings.locator(".mobile-settings__fingerprint code")).toHaveText(FINGERPRINT);
  await expect(settings.locator(".mobile-settings__device").filter({ hasText: "Pixel 9" })).toContainText("已授权");
  await expect(settings.getByText("1 台在用")).toBeVisible();

  // 已有配对挑战：展示人工校验码、二维码与配对链接。
  await expect(settings.locator(".mobile-settings__code strong")).toHaveText("482913");
  await expect(settings.locator(".mobile-settings__qr img")).toBeVisible();
  await expect(settings.locator(".mobile-settings__uri code")).toHaveText(PAIRING.uri);

  // 待确认设备批准后进入设备列表。
  const pendingRow = settings.locator(".mobile-settings__pending-row").filter({ hasText: "iPhone 16" });
  await expect(pendingRow).toBeVisible();
  await pendingRow.getByRole("button", { name: "允许" }).click();
  await expect(settings.locator(".mobile-settings__device").filter({ hasText: "iPhone 16" })).toBeVisible();
  await expect(settings.getByText("2 台在用")).toBeVisible();

  // 能力清单区分已实现与本期未实现。
  const interruptToggle = settings
    .locator(".mobile-settings__capability-toggle")
    .filter({ hasText: "停止运行中的 Turn" });
  await expect(interruptToggle).toHaveAttribute("aria-pressed", "true");
  const lockedCapabilities = settings.locator(".mobile-settings__capabilities li.is-locked");
  await expect(lockedCapabilities).toHaveCount(5);
  await expect(lockedCapabilities.first()).toContainText("读取工作区文件");
  await expect(lockedCapabilities.first()).toContainText("本期未实现");
  await interruptToggle.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              __mobileCalls: Array<{ command: string; args?: Record<string, unknown> }>;
            }
          ).__mobileCalls.filter((call) => call.command === "mobile_set_capabilities").at(-1)?.args,
      ),
    )
    .toEqual({ capabilities: ["chat", "approval"] });

  // 设备撤销立即反映到列表。
  const revoked = settings.locator(".mobile-settings__device").filter({ hasText: "Pixel 9" });
  await revoked.getByRole("button", { name: "撤销" }).click();
  await expect(revoked).toContainText("已撤销");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as { __mobileCalls: Array<{ command: string; args?: Record<string, unknown> }> }
          ).__mobileCalls.some((call) => call.command === "mobile_revoke_device"),
      ),
    )
    .toBe(true);

  // 重新生成一次性挑战。
  await settings.getByRole("button", { name: "重新生成" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as { __mobileCalls: Array<{ command: string; args?: Record<string, unknown> }> }
          ).__mobileCalls.some((call) => call.command === "mobile_create_pairing"),
      ),
    )
    .toBe(true);

  // 窄屏不出现横向溢出。
  await page.setViewportSize({ width: 700, height: 820 });
  const narrow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(narrow.scrollWidth).toBeLessThanOrEqual(narrow.clientWidth + 1);
  await page.screenshot({ path: testInfo.outputPath("mobile-settings-narrow.png") });

  // 停止网关后回到未启动状态，配对入口禁用。
  await page.setViewportSize({ width: 1280, height: 820 });
  await settings.getByRole("button", { name: "停止", exact: true }).click();
  await expect(settings.getByText("未启动")).toBeVisible();
  await expect(settings.getByRole("button", { name: "生成配对二维码" })).toBeDisabled();
  await page.screenshot({ path: testInfo.outputPath("mobile-settings-stopped.png") });
});
