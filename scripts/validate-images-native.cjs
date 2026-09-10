// Run after pnpm tauri dev --no-watch --config src-tauri/target/image-validation.json.
// Real Tauri IPC and Provider HTTP; the fixture never contacts a paid model.
const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  let png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
  const calls = [];
  const localImage = 'target/native-image-fixture.png';
  fs.writeFileSync(path.join('src-tauri', localImage), Buffer.from(png.split(',')[1], 'base64'));
  const server = createServer((req, res) => {
    if (req.url === '/picture.png') { res.writeHead(200, { 'Content-Type': 'image/png' }); res.end(Buffer.from(png.split(',')[1], 'base64')); return; }
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      calls.push(JSON.parse(body));
      const text = `原图已收到。\n\n![本地结果](${localImage})\n\n![网络结果](http://127.0.0.1:${server.address().port}/picture.png)\n\n![内嵌结果](${png})\n\n1750000000000-0123456789abcdef.png`;
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.end(`data: ${JSON.stringify({ choices: [{ delta: { content: text }, finish_reason: 'stop' }] })}\n\ndata: [DONE]\n\n`);
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const artifactDir = path.join(process.env.APPDATA, 'com.kcoder.validation.images/runtime-data/advanced/browser-artifacts');
  fs.mkdirSync(artifactDir, { recursive: true });
  fs.writeFileSync(path.join(artifactDir, '1750000000000-0123456789abcdef.png'), Buffer.from(png.split(',')[1], 'base64'));
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9402');
  try {
    const page = browser.contexts()[0].pages().find(p => p.url().includes(':1462'));
    if (!page) throw new Error('Isolated image validation host missing');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    png = await page.evaluate(() => {
      const canvas = document.createElement('canvas'); canvas.width = 640; canvas.height = 360;
      const c = canvas.getContext('2d');
      c.fillStyle = '#f0f5ff'; c.fillRect(0, 0, 640, 360);
      c.fillStyle = '#2754ba'; c.fillRect(36, 36, 568, 62);
      c.font = '26px sans-serif'; c.fillStyle = '#ffffff'; c.fillText('k-Coder 图片预览验证', 58, 77);
      c.fillStyle = '#35a078'; c.fillRect(60, 148, 140, 140);
      c.fillStyle = '#f0a63a'; c.beginPath(); c.arc(320, 218, 70, 0, Math.PI*2); c.fill();
      c.fillStyle = '#5867cb'; c.beginPath(); c.moveTo(510, 148); c.lineTo(580, 288); c.lineTo(440, 288); c.fill();
      c.fillStyle = '#344157'; c.font = '18px sans-serif'; c.fillText('原图发送 · 缩略图展示 · 点击打开', 161, 331);
      return canvas.toDataURL('image/png');
    });
    fs.writeFileSync(path.join('src-tauri', localImage), Buffer.from(png.split(',')[1], 'base64'));
    fs.writeFileSync(path.join(artifactDir, '1750000000000-0123456789abcdef.png'), Buffer.from(png.split(',')[1], 'base64'));
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async baseUrl => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      await api.saveProviderConfig({ id: 'image-native-fixture', name: '本地图片验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'deepseek-flash', models: [{ id: 'deepseek-flash', displayName: 'Vision fixture', contextWindow: 128000, fallback: false, supportsVision: true }], endpoints: [], apiKey: 'dummy-image-validation', activate: true });
      await useWorkbenchStore.getState().loadProviderCatalog();
      const thread = await api.createThread(true);
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      window.imageValidation = { api, thread, useWorkbenchStore };
    }, `http://127.0.0.1:${server.address().port}/v1`);
    await page.locator('.composer textarea').evaluate((element, png) => {
      const bytes = Uint8Array.from(atob(png.split(',')[1]), c => c.charCodeAt(0));
      const transfer = new DataTransfer();
      transfer.items.add(new File([bytes], 'user-upload.png', { type: 'image/png' }));
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }));
    }, png);
    await expect(page.locator('.attachment-strip')).toContainText('user-upload.png');
    await page.locator('.composer textarea').fill('请分析这张图片');
    await page.getByRole('button', { name: '发送消息', exact: true }).click();
    await expect(page.getByRole('button', { name: '查看图片 本地结果', exact: true }).last()).toBeVisible({ timeout: 30000 });
    expect(calls.length).toBe(1);
    expect(calls[0].messages.some(m => Array.isArray(m.content) && m.content.some(c => c.type === 'image_url' && c.image_url.url === png))).toBe(true);
    for (const name of ['user-upload.png', '本地结果', '网络结果', '内嵌结果', '1750000000000-0123456789abcdef.png']) {
      const button = page.getByRole('button', { name: `查看图片 ${name}`, exact: true }).last();
      await expect.poll(() => button.locator('img').evaluate(img => img.naturalWidth)).toBe(640);
      await button.click();
      await expect(page.getByRole('dialog', { name, exact: true })).toBeVisible();
      await expect.poll(() => page.getByRole('dialog').locator('img').evaluate(img => img.naturalWidth)).toBe(640);
      if (name === '本地结果') await page.screenshot({ path: 'docs/图片预览原生验证.png' });
      await page.keyboard.press('Escape');
    }
    await page.evaluate(async () => { const { api, thread } = window.imageValidation; await api.compactThread(thread.id); });
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page.getByRole('button', { name: '查看图片 本地结果', exact: true })).toBeVisible();
    await page.locator('.composer textarea').fill('继续分析刚才的图片');
    await page.getByRole('button', { name: '发送消息', exact: true }).click();
    await expect(page.getByRole('button', { name: '查看图片 本地结果', exact: true })).toHaveCount(2, { timeout: 30000 });
    expect(calls.length).toBe(2);
    expect(calls[1].messages.some(m => Array.isArray(m.content) && m.content.some(c => c.type === 'image_url' && c.image_url.url === png))).toBe(true);
    const security = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const threads = await api.listThreads();
      const project = threads.find(t => t.inProject);
      const denied = [];
      for (const file of ['../outside.png', 'C:/Windows/win.ini', 'src/App.tsx']) {
        try { await api.readMessageImage(project.id, file); denied.push(false); } catch { denied.push(true); }
      }
      const standalone = await api.createThread(false);
      try { await api.readMessageImage(standalone.id, 'target/native-image-fixture.png'); denied.push(false); } catch { denied.push(true); }
      return denied;
    });
    expect(security).toEqual([true, true, true, true]);
    fs.writeFileSync('src-tauri/target/image-native-result.json', JSON.stringify({ requests: calls.length, exactImageBytes: true, restoredAfterCompaction: true, previewTypes: 5, security, result: 'passed' }, null, 2));
    console.log('Native image flow passed: upload bytes, five previews, compaction/reload, workspace boundaries.');
  } finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
})().catch(error => { console.error(error); process.exitCode = 1; });

