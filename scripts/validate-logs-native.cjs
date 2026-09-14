const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

// Isolated Tauri dev: Vite 1476 / WebView2 CDP 9416. No external model calls.
(async () => {
  const server = createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(400, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: { message: 'local runtime log validation failure' } }));
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9416');
  try {
    const page = browser.contexts()[0].pages().find(p => p.url().startsWith('http://127.0.0.1:1476'));
    if (!page) throw new Error('Isolated log validation WebView missing');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threads = await page.evaluate(async baseUrl => {
      const api = await import('/src/api/runtime.ts');
      await api.saveProviderConfig({ id: 'log-local-fixture', name: '日志验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [{ id: 'fixture', displayName: 'Fixture', contextWindow: 128000, fallback: false }], endpoints: [], apiKey: 'dummy-log-validation', activate: true });
      const a = await api.createThread(false);
      const b = await api.createThread(false);
      const { invoke } = await import('/node_modules/@tauri-apps/api/core.js');
      await invoke('rename_thread', { threadId: a.id, title: '日志来源验证 A' });
      await invoke('rename_thread', { threadId: b.id, title: '日志来源验证 B' });
      for (const thread of [a, b]) await api.runTurn(thread.id, '触发本地模型错误', [], 'ask');
      return [a.id, b.id];
    }, `http://127.0.0.1:${server.address().port}/v1`);
    await page.getByTitle('查看本地运行日志').click();
    const dialog = page.getByRole('dialog', { name: '本地运行日志' });
    await expect(dialog.getByLabel('级别').locator('option')).toHaveText(['全部级别', 'Info', 'Error']);
    await expect(dialog.locator('.log-source').filter({ hasText: threads[0] })).toContainText('日志来源验证 A');
    await expect(dialog.locator('.log-source').filter({ hasText: threads[1] })).toContainText('日志来源验证 B');
    expect(await dialog.locator('.log-time').allTextContents()).not.toContain('Invalid Date');
    await page.screenshot({ path: 'docs/本地运行日志原生验证.png' });
    const root = path.join(process.env.APPDATA, 'com.kcoder.validation.logs/runtime-data/logs');
    for (let n = 1; n <= 3; n++) fs.writeFileSync(path.join(root, `runtime.jsonl.${n}`), JSON.stringify({ timestampMs: n, level: n === 1 ? 'warn' : 'info', event: 'old_rotation', fields: {} }) + '\n');
    await dialog.getByRole('button', { name: '刷新', exact: true }).click();
    await expect(dialog.locator('.log-badge').filter({ hasText: 'warn' })).toHaveCount(0);
    await dialog.getByRole('button', { name: '清理日志', exact: true }).click();
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    expect(fs.existsSync(path.join(root, 'runtime.jsonl.3'))).toBe(true);
    await dialog.getByRole('button', { name: '清理日志', exact: true }).click();
    await dialog.getByRole('button', { name: '确认清理', exact: true }).click();
    await expect(dialog.locator('.log-row')).toHaveCount(1);
    await expect(dialog.locator('.log-event')).toHaveText('logs_cleared');
    expect(fs.readdirSync(root)).toEqual(['runtime.jsonl']);
    const persisted = fs.readFileSync(path.join(root, 'runtime.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    expect(persisted).toHaveLength(1);
    expect(persisted[0].event).toBe('logs_cleared');
    const history = await page.evaluate(async ids => {
      const api = await import('/src/api/runtime.ts');
      return Promise.all(ids.map(id => api.readThread(id)));
    }, threads);
    expect(history.every(thread => thread.messages.length > 0)).toBe(true);
    await page.getByRole('button', { name: '关闭日志查看器' }).click();
    await page.evaluate(async threadId => {
      const api = await import('/src/api/runtime.ts');
      await api.runTurn(threadId, '清理后继续记录', [], 'ask');
    }, threads[0]);
    await page.getByTitle('查看本地运行日志').click();
    await expect(dialog.locator('.log-source').filter({ hasText: threads[0] })).toContainText('日志来源验证 A');
    console.log('PASS: two real conversation errors, level filtering, cancel/confirm clear, all rotations removed, audit, history preserved, logging resumes.');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
