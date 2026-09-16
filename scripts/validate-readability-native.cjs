// P10-187（界面阅读层级）与 P10-189（会话区「回到底部」按钮）的原生桌面补验。
//
// 需要隔离的 `pnpm tauri dev` 宿主：
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9421'
//   pnpm tauri dev --no-watch --config src-tauri/target/readability-validation.json
// 然后执行：node scripts/validate-readability-native.cjs
//
// 内容来自脚本内起的 loopback provider（OpenAI Chat Completions 形状的 SSE），
// 因此界面拿到的是真实长度的会话，不调用任何外部模型、不消耗额度。
const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const { writeFileSync } = require('node:fs');
const path = require('node:path');

const REPLY_PARAGRAPHS = 70;
const REPLY = Array.from(
  { length: REPLY_PARAGRAPHS },
  (_, index) => `阅读层级与回到底部验证第 ${index + 1} 段内容，让会话继续向下生长。`,
).join('\n\n');

(async () => {
  const server = createServer((request, response) => {
    request.resume();
    request.on('end', () => {
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.end(
        'data: ' + JSON.stringify({ choices: [{ delta: { content: REPLY }, finish_reason: 'stop' }] })
        + '\n\ndata: [DONE]\n\n',
      );
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));

  const browser = await chromium.connectOverCDP('http://127.0.0.1:9421');
  const page = browser.contexts()[0].pages().find(
    (candidate) => candidate.url().startsWith('http://127.0.0.1:1481/'),
  );
  if (!page) throw new Error('isolated readability WebView is missing');

  const result = {};
  const ready = () => expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();

  try {
    await ready();

    // 一个真实会话：loopback provider + 新会话，随后发送一条会得到长回复的消息。
    const fixtureId = 'readability-loopback-validation';
    const setup = await page.evaluate(async ({ baseUrl, workspacePath, fixtureId }) => {
      try {
        const api = await import('/src/api/runtime.ts');
        // `trusted` 是宿主信任门：隔离宿主里脚本自己确认信任本仓库。
        await api.switchWorkspace(workspacePath, true);
        await api.saveProviderConfig({
          id: fixtureId,
          name: '本地阅读层级验证',
          kind: 'open_ai_compatible',
          transport: 'open_ai_chat_completions',
          baseUrl,
          model: 'readability-fixture',
          models: [],
          endpoints: [],
          apiKey: 'dummy-local-readability-validation',
          activate: true,
        });
        await api.createThread(true);
        return { ok: true };
      } catch (error) {
        return {
          ok: false,
          message: String((error && error.message) || error),
          stack: String((error && error.stack) || ''),
        };
      }
    }, {
      baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
      workspacePath: path.resolve('.'),
      fixtureId,
    });
    if (!setup.ok) throw new Error(`loopback fixture setup failed: ${setup.message}\n${setup.stack}`);
    await page.reload();
    await ready();

    const input = page.getByRole('textbox', { name: '消息', exact: true });
    const area = page.locator('.message-area');
    const button = page.locator('.scroll-to-bottom-button');

    await input.fill('请输出阅读层级验证用的长内容');
    await page.getByRole('button', { name: '发送消息', exact: true }).click();
    // 等最后一段到达：loopback 回复固定 70 段，出现第 70 段即整段已渲染。
    await expect(page.locator('.message--assistant').last()).toContainText('第 70 段内容', { timeout: 90_000 });
    await expect.poll(
      () => area.evaluate((element) => element.scrollHeight - element.clientHeight),
      { timeout: 30_000 },
    ).toBeGreaterThan(1_200);
    // 连续两次读数一致才认为布局稳定（打字机式渲染会让高度持续变化）。
    await expect.poll(async () => {
      const first = await area.evaluate((element) => element.scrollHeight);
      await page.waitForTimeout(400);
      const second = await area.evaluate((element) => element.scrollHeight);
      return first === second;
    }, { timeout: 30_000 }).toBe(true);

    // ------------------------------------------------------------------
    // P10-189：会话区「回到底部」悬浮按钮
    // ------------------------------------------------------------------
    // 贴底时既不显示按钮，也不渲染停靠点。
    await expect(page.locator('.scroll-to-bottom-dock')).toHaveCount(0);
    await expect(button).toHaveCount(0);

    await area.evaluate((element) => {
      element.scrollTop = Math.max(0, element.scrollTop - 240);
      // 与既有用例一致：只有向上滚轮才暂停跟随。
      element.dispatchEvent(new WheelEvent('wheel', { deltaY: -240 }));
      element.dispatchEvent(new Event('scroll'));
    });
    await expect(button).toBeVisible();

    const geometry = await page.evaluate(() => {
      const areaElement = document.querySelector('.message-area');
      const buttonElement = document.querySelector('.scroll-to-bottom-button');
      const dockElement = document.querySelector('.scroll-to-bottom-dock');
      if (!areaElement || !buttonElement || !dockElement) return null;
      const areaRect = areaElement.getBoundingClientRect();
      const buttonRect = buttonElement.getBoundingClientRect();
      return {
        bottomGap: Math.round(areaRect.bottom - buttonRect.bottom),
        centerDelta: Math.round(Math.abs(
          (buttonRect.left + buttonRect.width / 2)
          - (areaRect.left + areaElement.clientLeft + areaElement.clientWidth / 2),
        )),
        dockHeight: Math.round(dockElement.getBoundingClientRect().height),
        buttonWidth: Math.round(buttonRect.width),
        buttonHeight: Math.round(buttonRect.height),
        distanceToBottom: Math.round(
          areaElement.scrollHeight - areaElement.clientHeight - areaElement.scrollTop,
        ),
        position: getComputedStyle(dockElement).position,
      };
    });
    expect(geometry).not.toBeNull();
    expect(geometry.bottomGap).toBeGreaterThanOrEqual(8);
    expect(geometry.bottomGap).toBeLessThanOrEqual(32);
    expect(geometry.centerDelta).toBeLessThanOrEqual(2);
    expect(geometry.dockHeight).toBe(0);
    expect(geometry.buttonWidth).toBe(32);
    expect(geometry.buttonHeight).toBe(32);
    expect(geometry.distanceToBottom).toBeGreaterThan(48);
    expect(geometry.position).toBe('sticky');
    result.scrollToBottom = geometry;

    await area.screenshot({ path: 'docs/回到底部按钮-原生.png' });

    // 点击后恢复跟随并立即回到最新内容，按钮随滚动状态自动隐藏。
    await button.click();
    await expect.poll(() => area.evaluate((element) =>
      element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
    await expect(page.locator('.scroll-to-bottom-dock')).toHaveCount(0);
    await expect(button).toHaveCount(0);

    // ------------------------------------------------------------------
    // P10-187：界面阅读层级（浅/深 × 背景关闭/开启）
    // ------------------------------------------------------------------
    const readings = [];
    for (const theme of ['light', 'dark']) {
      for (const background of [false, true]) {
        await page.evaluate(({ theme, background }) => {
          localStorage.setItem('kcoder_theme', theme);
          localStorage.setItem('kcoder_background_enabled', String(background));
        }, { theme, background });
        await page.reload();
        await ready();
        await expect(page.locator('.message-list')).toHaveCount(1);

        const activeInput = page.getByRole('textbox', { name: '消息', exact: true });
        // 输入框自适应：常规高度有上限，多行草稿增高且在框内滚动。
        const compactHeight = await activeInput.evaluate((element) => element.getBoundingClientRect().height);
        expect(compactHeight).toBeLessThanOrEqual(70);
        await activeInput.fill(Array.from({ length: 18 }, (_, index) => `第 ${index + 1} 行：多行草稿自适应`).join('\n'));
        const expanded = await activeInput.evaluate((element) => ({
          height: element.getBoundingClientRect().height,
          scroll: element.scrollHeight,
          client: element.clientHeight,
        }));
        expect(expanded.height).toBeGreaterThan(compactHeight);
        expect(expanded.height).toBeLessThanOrEqual(240);
        expect(expanded.scroll).toBeGreaterThan(expanded.client);
        await activeInput.fill('');

        const metrics = await page.evaluate(() => {
          const rect = (selector) => document.querySelector(selector).getBoundingClientRect();
          const list = rect('.message-list');
          const composer = rect('.composer');
          const areaRect = rect('.message-area');
          const send = rect('.send-button');
          const conversation = getComputedStyle(document.querySelector('.conversation'));
          const sidebar = getComputedStyle(document.querySelector('.sidebar'));
          const alpha = (value) => {
            const rgba = value.match(/rgba?\(([^)]+)\)/);
            if (rgba) {
              const parts = rgba[1].split(',').map((part) => Number(part.trim()));
              return parts.length > 3 ? parts[3] : 1;
            }
            // Chromium 把 color-mix() 的结果序列化成 color(srgb r g b / a)。
            const slash = value.match(/\/\s*([\d.]+)\s*\)/);
            return slash ? Number(slash[1]) : 1;
          };
          const composerButtons = Array.from(document.querySelectorAll('.composer button'))
            .filter((element) => element.getBoundingClientRect().width > 0)
            .map((element) => (element.getAttribute('aria-label') || element.textContent || '').trim());
          return {
            viewport: innerWidth,
            theme: document.documentElement.dataset.theme,
            listWidth: Math.round(list.width),
            alignLeft: Math.abs(Math.round(list.left - composer.left)),
            alignRight: Math.abs(Math.round(list.right - composer.right)),
            overlap: Math.round(areaRect.bottom - composer.top),
            composerOverflow: document.querySelector('.composer').scrollWidth - document.querySelector('.composer').clientWidth,
            pageOverflow: document.documentElement.scrollWidth - innerWidth,
            sendVisible: send.width > 0 && send.height > 0
              && send.bottom <= innerHeight + 1 && send.right <= innerWidth + 1 && send.left >= -1 && send.top >= -1,
            sendBottomGap: Math.round(innerHeight - send.bottom),
            conversationBlur: conversation.backdropFilter,
            conversationAlpha: alpha(conversation.backgroundColor),
            sidebarBlur: sidebar.backdropFilter,
            sidebarAlpha: alpha(sidebar.backgroundColor),
            backgroundClass: document.querySelector('.workbench').className.includes('workbench--background'),
            composerButtons,
            modelTriggerVisible: !!document.querySelector('button[aria-label="选择模型"]')
              && document.querySelector('button[aria-label="选择模型"]').getBoundingClientRect().width > 0,
          };
        });

        expect(metrics.theme).toBe(theme);
        expect(metrics.alignLeft).toBeLessThanOrEqual(1);
        expect(metrics.alignRight).toBeLessThanOrEqual(1);
        expect(metrics.overlap).toBeLessThanOrEqual(1);
        expect(metrics.composerOverflow).toBeLessThanOrEqual(1);
        expect(metrics.pageOverflow).toBeLessThanOrEqual(1);
        expect(metrics.sendVisible).toBe(true);
        expect(metrics.modelTriggerVisible).toBe(true);
        expect(metrics.composerButtons.length).toBeGreaterThanOrEqual(3);
        if (theme === 'light') {
          // 正文阅读列在宽窗口下达到 820px 上限。
          expect(metrics.listWidth).toBeLessThanOrEqual(820);
          expect(metrics.listWidth).toBeGreaterThanOrEqual(700);
        }
        if (background) {
          expect(metrics.backgroundClass).toBe(true);
          expect(metrics.conversationBlur).toContain('blur(12px)');
          expect(metrics.conversationAlpha).toBeGreaterThan(0.85);
          expect(metrics.conversationAlpha).toBeLessThan(0.91);
          expect(metrics.sidebarBlur).toContain('blur(8px)');
          expect(metrics.sidebarAlpha).toBeGreaterThan(0.89);
          expect(metrics.sidebarAlpha).toBeLessThan(0.95);
        } else {
          expect(metrics.backgroundClass).toBe(false);
        }
        readings.push({ theme, background, compactHeight: Math.round(compactHeight), ...metrics });
      }
    }
    result.readings = readings;

    // 截图（浅色与深色各一张，两种都带背景以体现遮罩）。
    await page.evaluate(() => {
      localStorage.setItem('kcoder_background_enabled', 'true');
      localStorage.setItem('kcoder_theme', 'light');
    });
    await page.reload();
    await ready();
    await expect(page.locator('.message-list')).toHaveCount(1);
    await page.screenshot({ path: 'docs/阅读层级-原生-浅色.png' });
    await page.evaluate(() => localStorage.setItem('kcoder_theme', 'dark'));
    await page.reload();
    await ready();
    await expect(page.locator('.message-list')).toHaveCount(1);
    await page.screenshot({ path: 'docs/阅读层级-原生-深色.png' });

    writeFileSync(
      'src-tauri/target/readability-native-result.json',
      JSON.stringify(result, null, 2),
    );
    console.log('PASS: P10-189 回到底部按钮 + P10-187 阅读层级（2 主题 × 2 背景）原生验证');
  } finally {
    server.close();
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
