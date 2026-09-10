// Isolated `pnpm tauri dev` host: Vite 1473 / WebView2 CDP 9413.
// A loopback provider renders realistic content without an external model.
const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const { writeFileSync } = require('node:fs');
const path = require('node:path');

(async () => {
  const server = createServer((request, response) => {
    request.resume();
    request.on('end', () => {
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.end('data: ' + JSON.stringify({ choices: [{ delta: { content: '界面优化已整理为三个重点：\n\n- 统一导航与正文的字号，中文阅读更清晰。\n- 输入区合并视觉边界，常用配置沿底部对齐。\n- 保留工具执行状态和可展开的变更详情。\n\n下面是样式示例：\n\n```css\n.composer {\n  border-radius: 12px;\n  background: var(--color-surface-raised);\n}\n```\n\n可以继续在右侧工作台查看文件。' }, finish_reason: 'stop' }] }) + '\n\ndata: [DONE]\n\n');
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9413');
  const page = browser.contexts()[0].pages().find(p => p.url().startsWith('http://127.0.0.1:1473/'));
  const fixtureId = 'ui-a-loopback-validation';
  try {
    if (!page) throw new Error('Isolated A direction WebView missing');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspacePath, fixtureId }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspacePath, false);
      await api.saveProviderConfig({ id: fixtureId, name: '本地 UI 验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'ui-fixture', models: [], endpoints: [], apiKey: 'dummy-local-ui-validation', activate: true });
      await api.createThread(true);
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspacePath: path.resolve('.'), fixtureId });
    await page.reload();
    const input = page.getByRole('textbox', { name: '消息', exact: true });
    await input.fill('请整理工作区界面优化建议');
    await page.getByRole('button', { name: '发送消息', exact: true }).click();
    await expect(page.locator('.message--assistant')).toContainText('可以继续在右侧工作台查看文件。', { timeout: 30000 });
    await input.fill('继续检查输入区和工作台布局');
    const screenshots = [];
    const scheme = await page.locator('html').getAttribute('data-theme');
    if (scheme !== 'light') await page.getByRole('button', { name: '切换到浅色模式', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await page.screenshot({ path: 'docs/ui-a-native-light.png' });
    screenshots.push('ui-a-native-light.png');
    await page.getByRole('button', { name: '切换到深色模式', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    await page.screenshot({ path: 'docs/ui-a-native-dark.png' });
    screenshots.push('ui-a-native-dark.png');
    const originalWidth = await page.evaluate(() => innerWidth);
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).not.toBe(originalWidth);
    await page.getByRole('button', { name: '最大化或还原窗口', exact: true }).click();
    await expect.poll(() => page.evaluate(() => innerWidth)).toBe(originalWidth);
    await expect(input).toHaveValue('继续检查输入区和工作台布局');
    await page.getByRole('button', { name: '工作台', exact: true }).click();
    await expect(page.locator('.workbench-panel')).toBeVisible();
    await page.screenshot({ path: 'docs/ui-a-native-workbench.png' });
    screenshots.push('ui-a-native-workbench.png');
    const cdp = await page.context().newCDPSession(page);
    const metrics = [];
    for (const [width, height, scale] of [[1280, 820, 1], [1024, 656, 1.25], [853, 547, 1.5], [900, 640, 1]]) {
      await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: scale, mobile: false });
      await expect.poll(() => page.evaluate(() => innerWidth)).toBe(width);
      await expect(input).toHaveValue('继续检查输入区和工作台布局');
      const bounds = await page.evaluate(() => {
        const composer = document.querySelector('.composer');
        const chat = document.querySelector('.conversation').getBoundingClientRect();
        const panel = document.querySelector('.workbench-panel').getBoundingClientRect();
        const send = document.querySelector('.send-button').getBoundingClientRect();
        return { width: innerWidth, composerOverflow: composer.scrollWidth - composer.clientWidth, pageOverflow: document.documentElement.scrollWidth - innerWidth, panelOverlap: chat.right - panel.left, sendRight: send.right, composerRight: composer.getBoundingClientRect().right, sendBottom: send.bottom, height: innerHeight };
      });
      expect(bounds.composerOverflow).toBeLessThanOrEqual(1);
      expect(bounds.pageOverflow).toBeLessThanOrEqual(1);
      expect(bounds.panelOverlap).toBeLessThanOrEqual(1);
      expect(bounds.sendRight).toBeLessThanOrEqual(bounds.composerRight + 1);
      expect(bounds.sendBottom).toBeLessThanOrEqual(bounds.height);
      metrics.push(bounds);
    }
    await cdp.send('Emulation.clearDeviceMetricsOverride');
    await cdp.detach();
    writeFileSync('src-tauri/target/ui-a-native-result.json', JSON.stringify({ result: 'passed', metrics, screenshots }, null, 2));
    console.log(JSON.stringify({ result: 'passed', metrics, screenshots }));
  } finally {
    await page?.evaluate(async (fixtureId) => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey(fixtureId);
      await api.deleteProvider(fixtureId);
    }, fixtureId).catch(() => {});
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
