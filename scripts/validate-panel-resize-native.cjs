// Requires an isolated pnpm tauri dev host on Vite 1459 / WebView2 CDP 9399.
const { chromium, expect } = require('@playwright/test');
const { writeFileSync } = require('node:fs');
const { createServer } = require('node:http');

(async () => {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9399');
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    response.end('<title>面板宽度预览</title><style>body{font:16px sans-serif;padding:24px;background:#f7f8fc}input{max-width:90%;padding:8px}</style><h2>自适应网页预览</h2><p>拖动左侧边界，预览随面板宽度调整。</p><input aria-label="预览输入" value="保留网页状态">');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const page = browser.contexts()[0].pages().find(p => p.url().includes(':1459'));
    if (!page) throw new Error('Isolated panel resize WebView missing');
    await page.evaluate(() => localStorage.removeItem('kcoder_panel_widths_v1'));
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    await page.getByRole('tab', { name: '浏览器' }).click();
    // A local fixture exercises the actual native iframe without a remote service.
    const address = page.getByRole('textbox', { name: '网址' });
    await address.fill(`http://127.0.0.1:${server.address().port}/`);
    await address.press('Enter');
    const input = page.frameLocator('.browser-host iframe').getByRole('textbox', { name: '预览输入' });
    await expect(input).toHaveValue('保留网页状态');
    await page.locator('.composer textarea').fill('调整宽度时保留草稿');
    const width = selector => page.locator(selector).evaluate(e => e.getBoundingClientRect().width);
    const left = page.getByRole('separator', { name: '调整侧边栏宽度' });
    const right = page.getByRole('separator', { name: '调整右侧面板宽度' });
    const drag = async (handle, delta) => {
      const box = await handle.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + 200);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2 + delta, box.y + 200, { steps: 12 });
      await page.mouse.up();
    };
    const initial = { sidebar: await width('.sidebar'), panel: await width('.workbench-panel') };
    await drag(left, 50);
    await expect.poll(() => width('.sidebar')).toBeCloseTo(initial.sidebar + 50, 0);
    await drag(right, -130);
    await expect.poll(() => width('.workbench-panel')).toBeCloseTo(initial.panel + 130, 0);
    await drag(right, 80); // Drag across the mounted iframe while captured.
    await expect.poll(() => width('.workbench-panel')).toBeCloseTo(initial.panel + 50, 0);
    await input.fill('拖动后网页可操作');
    const preferred = { sidebar: await width('.sidebar'), panel: await width('.workbench-panel') };
    await page.screenshot({ path: 'docs/面板拖动原生验证.png' });
    // Exercise native maximize/restore through the same controls as the user.
    const restoredWidth = await page.evaluate(() => innerWidth);
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).not.toBe(restoredWidth);
    await expect(page.locator('.composer textarea')).toHaveValue('调整宽度时保留草稿');
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).toBe(restoredWidth);
    const cdp = await page.context().newCDPSession(page);
    const metrics = [];
    for (const targetWidth of [900, 1097, 1400]) {
      await cdp.send('Emulation.setDeviceMetricsOverride', { width: targetWidth, height: 820, deviceScaleFactor: 1, mobile: false });
      await expect.poll(() => page.evaluate(() => innerWidth)).toBe(targetWidth);
      await expect.poll(() => width('.conversation')).toBeGreaterThanOrEqual(360);
      await expect.poll(() => page.locator('.composer').evaluate(e => e.scrollWidth - e.clientWidth)).toBeLessThanOrEqual(1);
      const bounds = await page.evaluate(() => {
        const chat = document.querySelector('.conversation').getBoundingClientRect();
        const panel = document.querySelector('.workbench-panel').getBoundingClientRect();
        return { viewport: innerWidth, chatWidth: chat.width, panelWidth: panel.width, overlap: chat.right - panel.left, overflow: document.documentElement.scrollWidth - innerWidth };
      });
      expect(bounds.overlap).toBeLessThanOrEqual(1);
      expect(bounds.overflow).toBeLessThanOrEqual(1);
      metrics.push(bounds);
    }
    await expect.poll(() => width('.sidebar')).toBeCloseTo(preferred.sidebar, 0);
    await expect.poll(() => width('.workbench-panel')).toBeCloseTo(preferred.panel, 0);
    await expect(page.locator('.composer textarea')).toHaveValue('调整宽度时保留草稿');
    await expect(input).toHaveValue('拖动后网页可操作');
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    await expect.poll(() => width('.sidebar')).toBeCloseTo(preferred.sidebar, 0);
    await expect.poll(() => width('.workbench-panel')).toBeCloseTo(preferred.panel, 0);
    await left.dblclick();
    await right.dblclick();
    await expect.poll(() => width('.sidebar')).toBe(232);
    await expect.poll(() => width('.workbench-panel')).toBeCloseTo(392, 0);
    await cdp.send('Emulation.clearDeviceMetricsOverride');
    await cdp.detach();
    writeFileSync('src-tauri/target/panel-resize-native-result.json', JSON.stringify({ initial, preferred, metrics, result: 'passed' }, null, 2));
    console.log(JSON.stringify({ initial, preferred, metrics, result: 'passed' }));
  } finally {
    await new Promise(resolve => server.close(resolve));
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
