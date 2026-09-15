// Isolated pnpm tauri dev host: Vite 1459 / WebView2 CDP 9399.
const { chromium, expect } = require('@playwright/test');
const { writeFileSync } = require('node:fs');

(async () => {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9399');
  try {
    const page = browser.contexts()[0].pages().find(p => p.url().includes(':1459'));
    if (!page) throw new Error('Isolated card layout WebView missing');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const cdp = await page.context().newCDPSession(page);
    const results = [];
    for (const theme of ['light', 'dark']) {
      for (const background of [false, true]) {
        await page.evaluate(({ theme, background }) => {
          localStorage.setItem('kcoder_theme', theme);
          localStorage.setItem('kcoder_background_enabled', String(background));
          localStorage.removeItem('kcoder_panel_widths_v1');
        }, { theme, background });
        await page.reload();
        await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
        await page.locator('.composer textarea').fill('卡片之间留出空隙，输入草稿在调整窗口后保留。');
        await page.getByRole('button', { name: '工作台', exact: true }).click();
        for (const viewport of [1400, 900, 722, 700, 376]) {
          await cdp.send('Emulation.setDeviceMetricsOverride', { width: viewport, height: 900, deviceScaleFactor: 1, mobile: false });
          await expect.poll(() => page.evaluate(() => innerWidth)).toBe(viewport);
          const metrics = await page.evaluate(() => {
            const box = selector => document.querySelector(selector).getBoundingClientRect();
            const chat = box('.conversation');
            const panel = box('.workbench-panel');
            const sidebar = box('.sidebar');
            return {
              titleTop: box('.titlebar').top, titleLeft: box('.titlebar').left,
              titleRight: innerWidth - box('.titlebar').right,
              titleOverflow: document.querySelector('.titlebar').scrollWidth - document.querySelector('.titlebar').clientWidth,
              panelTop: panel.top - box('.titlebar').bottom,
              viewport: innerWidth, theme: document.documentElement.dataset.theme,
              chatWidth: chat.width, gap: panel.left - chat.right,
              left: innerWidth > 1180 ? sidebar.left : chat.left,
              top: chat.top - box('.titlebar').bottom, right: innerWidth - panel.right,
              bottom: innerHeight - panel.bottom,
              overflow: document.documentElement.scrollWidth - innerWidth,
              composerOverflow: document.querySelector('.composer').scrollWidth - document.querySelector('.composer').clientWidth,
            };
          });
          expect(metrics.theme).toBe(theme);
          for (const inset of [metrics.titleTop, metrics.titleLeft, metrics.titleRight, metrics.panelTop]) {
            expect(inset).toBeCloseTo(8, 0);
          }
          expect(metrics.left).toBeCloseTo(8, 0);
          expect(metrics.top).toBeCloseTo(8, 0);
          expect(metrics.right).toBeCloseTo(8, 0);
          expect(metrics.bottom).toBeCloseTo(8, 0);
          expect(metrics.overflow).toBeLessThanOrEqual(1);
          expect(metrics.titleOverflow).toBeLessThanOrEqual(1);
          expect(metrics.composerOverflow).toBeLessThanOrEqual(1);
          if (viewport > 720) {
            expect(metrics.gap).toBeCloseTo(8, 0);
            expect(metrics.chatWidth).toBeGreaterThanOrEqual(viewport > 1180 ? 419.5 : 359.5);
          }
          await expect(page.locator('.composer textarea')).toHaveValue('卡片之间留出空隙，输入草稿在调整窗口后保留。');
          results.push({ background, ...metrics });
          if (viewport === 1400) await page.screenshot({ path: `docs/卡片布局-${theme}-${background ? '背景' : '纯色'}.png` });
          if (viewport === 376 && theme === 'light' && !background) await page.screenshot({ path: 'docs/卡片布局-窄屏.png' });
        }
      }
    }
    await cdp.send('Emulation.clearDeviceMetricsOverride');
    await cdp.detach();
    const restoredWidth = await page.evaluate(() => innerWidth);
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).not.toBe(restoredWidth);
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).toBe(restoredWidth);
    writeFileSync('src-tauri/target/card-layout-native-result.json', JSON.stringify(results, null, 2));
    console.log(`PASS: ${results.length} native theme/background/viewport combinations`);
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });

