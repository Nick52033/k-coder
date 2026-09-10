// Run against the isolated dev host on WebView2 port 9381.
// Uses a loopback fixture and a dummy credential, never a hosted model.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');

(async () => {
  const calls = [];
  const server = http.createServer((request, response) => {
    request.resume();
    request.on('end', () => {
      calls.push(Date.now());
      if (calls.length === 1) {
        response.writeHead(429, { 'Content-Type': 'application/json', 'Retry-After': '10' });
        response.end(JSON.stringify({ error: { message: 'Free-tier request limit reached (validation fixture)' } }));
      } else {
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end('data: {"choices":[{"delta":{"content":"验证完成：冷却后恢复请求。"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9381');
  const page = browser.contexts()[0].pages().find(p => p.url().includes(':1441'));
  try {
    if (!page) throw new Error('Isolated validation WebView was not found');
    await page.reload();
    await page.waitForLoadState('networkidle');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async (baseUrl) => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      await api.saveProviderConfig({ id: 'quota-native-fixture', name: '本地限流验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-quota-validation', activate: true });
      await useWorkbenchStore.getState().loadProviderCatalog();
      const thread = await api.createThread(true);
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      window.quotaValidation = { api, useWorkbenchStore, thread };
      await api.createSubagent({ parentThreadId: thread.id, task: '请只回复验证完成，不调用工具。', label: '项目文档验证', capabilities: [], timeoutMs: 60000 });
    }, `http://127.0.0.1:${server.address().port}/v1`);
    const summary = page.getByRole('region', { name: '本会话子智能体状态' });
    await expect(summary).toContainText('限流等待');
    await page.evaluate(async () => {
      const { api, thread, useWorkbenchStore } = window.quotaValidation;
      await api.createSubagent({ parentThreadId: thread.id, task: '请只回复验证完成，不调用工具。', label: '架构文档验证', capabilities: [], timeoutMs: 60000 });
      await useWorkbenchStore.getState().sendMessage('本地请求冷却验证，请只回复验证完成。');
    });
    await expect(summary.locator('li')).toHaveCount(2);
    await expect(page.locator('.turn-execution--live > summary')).toContainText('限流等待');
    await summary.getByRole('button').filter({ hasText: '架构文档验证' }).click();
    await expect(page.locator('.agent-retry-wait')).toContainText('限流等待');
    if (calls.length !== 1) throw new Error(`Cooldown bypass: ${calls.length} HTTP calls`);
    await page.screenshot({ path: path.resolve('docs/quota-native-waiting.png') });
    await expect(summary).toContainText('已完成', { timeout: 35000 });
    await expect(summary.getByRole('button').filter({ hasText: '项目文档验证' })).toContainText('已完成', { timeout: 35000 });
    await expect(summary.getByRole('button').filter({ hasText: '架构文档验证' })).toContainText('已完成', { timeout: 35000 });
    await expect(page.locator('.turn-execution--live')).toHaveCount(0, { timeout: 35000 });
    if (calls.length !== 4) throw new Error(`Expected one 429 plus three successful requests, got ${calls.length}`);
    const gaps = calls.slice(1).map((time, i) => time - calls[i]);
    if (gaps[0] < 9900 || gaps.slice(1).some(gap => gap < 3900)) throw new Error(`Requests not spaced: ${gaps}`);
    console.log(JSON.stringify({ result: 'PASS', httpCalls: calls.length, requestGapsMs: gaps, nativeParentAndChildrenCompleted: true }));
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('quota-native-fixture');
      await api.deleteProvider('quota-native-fixture');
    }).catch(() => {});
    await browser.close();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
