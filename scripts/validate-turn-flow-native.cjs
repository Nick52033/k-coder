// Isolated Tauri host: com.kcoder.validation.turnflow, Vite 1492, CDP 9432.
// A real 605-second local command crosses the former 600-second pause boundary.
// No external model, git commit or git push is used.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

(async () => {
  const workspace = path.resolve('.tmp-ui/turn-flow/workspace');
  fs.mkdirSync(workspace, { recursive: true });
  const calls = [];
  let fixtureError;
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      try {
        const input = JSON.parse(body);
        calls.push(Date.now());
        assert(input.messages.some(message => message.role === 'system'
          && typeof message.content === 'string' && message.content.includes('<task_execution>')),
        'project Provider request must include task guidance');
        let delta;
        let finish;
        if (calls.length === 1) {
          delta = { content: '正在执行本地耗时验证，完成后自动继续。', tool_calls: [{ index: 0,
            id: 'long-local-check', type: 'function', function: { name: 'run_command',
              arguments: JSON.stringify({ command: 'Start-Sleep -Seconds 605; Write-Output "long-check-complete"',
                cwd: '.', timeoutMs: 660000 }) } }] };
          finish = 'tool_calls';
        } else {
          assert.equal(calls.length, 2, 'unexpected extra Provider request');
          assert(calls[1] - calls[0] >= 600000, 'must cross the real ten-minute boundary');
          assert(input.messages.some(message => message.role === 'tool'
            && message.content.includes('long-check-complete')), 'command result must reach Provider');
          delta = { content: '本地验证完成：超过十分钟后自动继续，无需确认。' };
          finish = 'stop';
        }
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end(`data: ${JSON.stringify({ choices: [{ delta, finish_reason: finish }],
          usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 } })}\n\ndata: [DONE]\n\n`);
      } catch (error) {
        fixtureError = error;
        response.writeHead(500, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify({ error: { message: 'Local validation failed' } }));
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  let page;
  let threadId;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9432');
    page = browser.contexts()[0].pages().find(page => page.url().includes(':1492'));
    assert(page, 'isolated turnflow WebView is required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 60000 });
    await page.evaluate(async ({ workspace, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'turn-flow-fixture', name: '本地续跑验证', kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [],
        apiKey: 'dummy-turn-flow-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { workspace, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 30000 });
    threadId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '十分钟后自动续跑验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '验证耗时操作完成后自动继续，使用本地测试命令。');
      return thread.id;
    });
    console.log(JSON.stringify({ phase: 'running', threadId, waitSeconds: 605 }));
    const deadline = Date.now() + 690000;
    for (;;) {
      if (fixtureError) throw fixtureError;
      const status = await page.evaluate(async id => {
        const api = await import('/src/api/runtime.ts');
        const detail = await api.readThread(id);
        return { turn: detail.lastTurn, inputs: detail.userInputs.length };
      }, threadId);
      assert.equal(status.inputs, 0, 'unexpected continuation request');
      assert(!['failed', 'cancelled'].includes(status.turn?.state), JSON.stringify(status.turn));
      if (status.turn?.state === 'completed') break;
      assert(Date.now() < deadline, 'native turn did not complete before validation deadline');
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    await expect(page.getByText('本地验证完成：超过十分钟后自动继续，无需确认。', { exact: true }))
      .toBeVisible({ timeout: 10000 });
    const detail = await page.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      return api.readThread(id);
    }, threadId);
    assert.equal(detail.userInputs.length, 0, 'must never ask for turn continuation');
    assert.equal(calls.length, 2);
    await page.reload();
    await expect(page.getByText('本地验证完成：超过十分钟后自动继续，无需确认。', { exact: true }))
      .toBeVisible({ timeout: 30000 });
    await page.screenshot({ path: path.resolve('docs/turn-flow-native.png') });
    console.log(JSON.stringify({ result: 'PASS', threadId, providerCalls: calls.length,
      elapsedMs: calls[1] - calls[0], continuationRequests: detail.userInputs.length, refreshed: true }));
  } finally {
    await page?.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      if (id) await api.cancelTurn(id).catch(() => {});
      await api.deleteProviderApiKey('turn-flow-fixture');
      await api.deleteProvider('turn-flow-fixture');
      await api.setApprovalMode('ask');
    }, threadId).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
