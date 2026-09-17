// P10-195（移动设备设置页「已授权设备」删除入口）的原生桌面验收。
//
// ⚠️ 2026-09-17 实测：本机 WebView2 的 DevTools 端点不可用，本脚本**跑不到断言**。
//    症状是「端口在 LISTENING、TCP 能连上、但一个字节都不回」：
//    `curl http://127.0.0.1:<port>/json/version` 超时，`chromium.connectOverCDP` 报
//    `<ws preparing> retrieving websocket url` 超时。已逐项排除：独立（含每次全新）的
//    `WEBVIEW2_USER_DATA_FOLDER`、`--remote-allow-origins=*`、`Host: localhost` 头、IPv6、
//    代理（`--noproxy '*'`）、端口被残留浏览器进程占用——同一台机器上自建的 9423 服务
//    curl 正常，说明不是端口级封锁。因此 P10-195 的原生桌面路径**仍未验收**。
//
// 环境恢复后的跑法（`pnpm` 在本机不可用，直接调 CLI）：
//   $env:WEBVIEW2_USER_DATA_FOLDER='<一个空目录>'
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9433'
//   node node_modules/@tauri-apps/cli/tauri.js dev --no-watch --config src-tauri/target/mobile-delete-validation.json
//   node scripts/validate-mobile-delete-native.cjs
// 两个坑：`tauri dev` 会被并行工作流的 cargo 挡在
// `Blocking waiting for file lock on build directory`，要等新的 `k-coder.exe` 出现**且**
// 9433 在 LISTENING 之后再跑脚本；上一次运行残留的浏览器进程会在退出前继续占着端口，
// 所以启动前先等端口空出来，别把「残留进程占着端口」当成「宿主已就绪」。
//
// 这条验收刻意不碰真实设备：配对由脚本自己走真实的 `/pair` HTTP 接口完成
// （挑战密钥从二维码 URI 的 fragment 里取，与手机端拿到的是同一份材料），
// 随后由真实 WebView2 点击设置页的按钮，最后回读落盘的 `mobile/state.json`，
// 证明「删除」不是只把行从界面上抹掉。
const { chromium, expect } = require('@playwright/test');
const { readFileSync, writeFileSync } = require('node:fs');
const path = require('node:path');

const CDP_PORT = Number(process.env.KCODER_CDP_PORT ?? 9433);
const DEV_URL = process.env.KCODER_DEV_URL ?? 'http://127.0.0.1:1481/';
const GATEWAY_PORT = 18787;
const RESULT_PATH = path.join('src-tauri', 'target', 'mobile-delete-native-result.json');
const STATE_PATH = path.join(
  process.env.APPDATA || '',
  'com.kcoder.validation.mobiledelete',
  'runtime-data',
  'mobile',
  'state.json',
);

const checks = [];
function check(name, condition, detail) {
  checks.push({ name, ok: Boolean(condition), detail: detail ?? null });
  if (!condition) throw new Error(`check failed: ${name}${detail ? ` (${detail})` : ''}`);
}

/** 走真实的手机配对接口，把一台设备登记进登记表。 */
async function pairDevice(page, name, platform) {
  const created = await page.evaluate(async (port) => {
    const api = await import('/src/api/runtime.ts');
    await api.startMobileGateway(null, port);
    return api.createMobilePairing();
  }, GATEWAY_PORT);

  // 二维码 URI：`http://127.0.0.1:18787/m/#c=<challengeId>.<challengeSecret>`
  const fragment = created.uri.split('#c=')[1] ?? '';
  const separator = fragment.indexOf('.');
  const challengeId = fragment.slice(0, separator);
  const challengeSecret = fragment.slice(separator + 1);
  check('pairing uri carries a challenge id and secret', Boolean(challengeId && challengeSecret), created.uri);

  const response = await fetch(`http://127.0.0.1:${GATEWAY_PORT}/pair`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      challengeId,
      challengeSecret,
      code: created.code,
      deviceName: name,
      platform,
    }),
  });
  const submitted = await response.json();
  check('pair submit is accepted', response.ok && Boolean(submitted.pendingId), JSON.stringify(submitted));

  const device = await page.evaluate(async (pendingId) => {
    const api = await import('/src/api/runtime.ts');
    return api.approveMobilePairing(pendingId);
  }, submitted.pendingId);
  check('approved device is registered', device.name === name, JSON.stringify(device));
  return device;
}

function readState() {
  try {
    return JSON.parse(readFileSync(STATE_PATH, 'utf8'));
  } catch {
    return null;
  }
}

(async () => {
  // 端口刚 LISTENING 不等于 DevTools 已经能应答：上一次运行残留的浏览器进程会在退出前
  // 继续占着端口，此时连上去只会卡到超时。所以这里带重试，而不是一次连不上就判失败。
  let browser = null;
  let connectError = null;
  for (let attempt = 0; attempt < 6 && !browser; attempt += 1) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`, { timeout: 10000 });
    } catch (error) {
      connectError = error;
      await new Promise((resolve) => setTimeout(resolve, 4000));
    }
  }
  if (!browser) throw connectError ?? new Error('could not attach to the isolated WebView2');

  // 窗口往往还没导航到 devUrl（首帧启动屏也要时间），所以这里轮询而不是只查一次；
  // 顺便把所有 page 的 URL 打出来，便于区分「还没导航」和「根本没起来」。
  let page = null;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => candidate.url().startsWith(DEV_URL));
    if (page) break;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  if (!page) {
    const urls = browser.contexts().flatMap((context) => context.pages()).map((candidate) => candidate.url());
    throw new Error(`isolated mobile-delete WebView is missing; pages = ${JSON.stringify(urls)}`);
  }

  const result = { checks, devices: {}, statePath: STATE_PATH, ok: false };
  try {
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();

    // 场景一：撤销过的设备（截图里那种只剩标签、无任何操作的行）也要能删掉。
    const revokedDevice = await pairDevice(page, 'Pixel 9 验证机', 'Android 验证');
    result.devices.revoked = revokedDevice.id;
    await page.evaluate(async (deviceId) => {
      const api = await import('/src/api/runtime.ts');
      await api.revokeMobileDevice(deviceId);
    }, revokedDevice.id);
    check(
      'revoked device is still persisted before deletion',
      readState()?.devices?.some((entry) => entry.id === revokedDevice.id && entry.revoked === true),
      JSON.stringify(readState()?.devices ?? null),
    );

    // 场景二：在用的设备，删除等价于「撤销 + 遗忘」。
    const activeDevice = await pairDevice(page, 'iPhone 16 验证机', 'iOS 验证');
    result.devices.active = activeDevice.id;

    const settings = page.getByRole('dialog', { name: '设置' });
    await page.locator('button[aria-label="设置"]:visible').click();
    await settings.getByRole('button', { name: '移动设备' }).click();
    await expect(settings.getByRole('heading', { name: '移动设备' })).toBeVisible();
    await expect(settings.getByText('1 台在用')).toBeVisible();

    const revokedRow = settings.locator('.mobile-settings__device').filter({ hasText: 'Pixel 9 验证机' });
    const activeRow = settings.locator('.mobile-settings__device').filter({ hasText: 'iPhone 16 验证机' });
    await expect(revokedRow).toContainText('已撤销');
    await expect(activeRow).toContainText('已授权');
    check(
      'both rows expose the delete action',
      (await revokedRow.getByRole('button', { name: '删除' }).count()) === 1 &&
        (await activeRow.getByRole('button', { name: '删除' }).count()) === 1,
    );
    check(
      'only the active row still exposes revoke',
      (await revokedRow.getByRole('button', { name: '撤销' }).count()) === 0 &&
        (await activeRow.getByRole('button', { name: '撤销' }).count()) === 1,
    );

    // 取消不能改数据。
    const dialog = settings.getByRole('alertdialog', { name: '删除设备' });
    await revokedRow.getByRole('button', { name: '删除' }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Pixel 9 验证机');
    await dialog.getByRole('button', { name: '取消' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(revokedRow).toBeVisible();
    check(
      'cancel keeps the record on disk',
      readState()?.devices?.some((entry) => entry.id === revokedDevice.id),
      JSON.stringify(readState()?.devices ?? null),
    );

    // 删除已撤销的设备：记录连同密钥摘要一起从落盘状态里消失。
    await revokedRow.getByRole('button', { name: '删除' }).click();
    await dialog.getByRole('button', { name: '删除' }).click();
    await expect(revokedRow).toHaveCount(0);
    await expect(settings.getByText('1 台在用')).toBeVisible();
    const afterRevokedRemoval = readState();
    check(
      'deleted revoked record leaves the state file',
      !afterRevokedRemoval?.devices?.some((entry) => entry.id === revokedDevice.id),
      JSON.stringify(afterRevokedRemoval?.devices ?? null),
    );

    // 删除在用的设备：弹窗文案必须点明「立即失去访问权限」，删完列表与计数同步回落。
    await activeRow.getByRole('button', { name: '删除' }).click();
    await expect(dialog).toContainText('立即失去访问权限');
    await dialog.getByRole('button', { name: '删除' }).click();
    await expect(activeRow).toHaveCount(0);
    await expect(settings.getByText('0 台在用')).toBeVisible();

    const status = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      return api.mobileStatus();
    });
    check('status snapshot no longer lists any device', status.devices.length === 0, JSON.stringify(status.devices));
    const finalState = readState();
    check(
      'state file no longer lists any device',
      Array.isArray(finalState?.devices) && finalState.devices.length === 0,
      JSON.stringify(finalState?.devices ?? null),
    );
    check(
      'deleted device secrets are gone with the record',
      !JSON.stringify(finalState ?? {}).includes(activeDevice.id),
    );

    // 收尾：停掉网关，别把端口留给下一次验收。
    await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.stopMobileGateway();
    });
    result.ok = true;
  } catch (error) {
    result.error = String((error && error.message) || error);
  } finally {
    writeFileSync(RESULT_PATH, `${JSON.stringify(result, null, 2)}\n`);
    await browser.close();
  }

  for (const entry of checks) {
    console.log(`${entry.ok ? 'ok  ' : 'FAIL'} ${entry.name}`);
  }
  console.log(result.ok ? `\n全部通过（${checks.length} 项）` : `\n失败：${result.error}`);
  process.exit(result.ok ? 0 : 1);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
