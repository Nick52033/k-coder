// Isolated pnpm tauri dev: com.kcoder.validation.userinputwait, Vite 1496 / CDP 9436.
// Uses a local SSE fixture. Rust virtual-time tests cover waits beyond 24 hours;
// this script verifies real IPC, persisted pending state, reload, answer and stop.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

(async () => {
  const output = path.resolve('.tmp-ui/user-input-wait');
  const workspace = path.join(output, 'workspace');
  fs.mkdirSync(workspace, { recursive: true });
  const providerId = 'user-input-wait-native-fixture';
  const question = '请选择处理方式，暂时不回答会保持等待。';
  let mode = 'question';
  let calls = 0;
  let fixtureError;
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      try {
        const input = JSON.parse(body);
        calls += 1;
        const first = calls === 1;
        if (first) {
          assert(input.tools.some(tool => tool.function?.name === 'request_user_input'));
        } else if (mode === 'question') {
          assert(input.messages.some(message => message.role === 'tool'
            && message.content.includes('保留入口')), 'answer must reach the Provider');
        }
        const tool = mode === 'continuation'
          ? { name: 'list_directory', arguments: JSON.stringify({ path: '.' }) }
          : { name: 'request_user_input', arguments: JSON.stringify({ questions: [
            { question, options: ['保留入口', '调整入口'] },
          ] }) };
        const delta = first ? { content: '等待你的选择。', tool_calls: [{
          index: 0, id: `input-${mode}`, type: 'function', function: tool,
        }] } : { content: '收到回答，任务已继续完成。' };
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end(`data: ${JSON.stringify({
          choices: [{ delta, finish_reason: first ? 'tool_calls' : 'stop' }],
          // Trigger the existing soft continuation gate without 100 fixture calls.
          usage: { prompt_tokens: 100, completion_tokens: 20,
            total_tokens: first && mode === 'continuation' ? 5_000_000 : 120 },
        })}\n\ndata: [DONE]\n\n`);
      } catch (error) {
        fixtureError = error;
        response.writeHead(500, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify({ error: { message: 'Local fixture assertion failed' } }));
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, page, currentThread;
  const results = [];
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9436');
    page = browser.contexts()[0].pages().find(candidate => candidate.url().includes(':1496'));
    assert(page, 'isolated WebView is required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 60000 });
    await page.evaluate(async ({ workspace, providerId, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: providerId, name: '本地用户回答验收', kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [],
        apiKey: 'dummy-local-user-input', activate: true });
    }, { workspace, providerId, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 30000 });
    for (const scenario of ['question', 'continuation', 'cancel']) {
      mode = scenario;
      calls = 0;
      currentThread = await page.evaluate(async scenario => {
        const api = await import('/src/api/runtime.ts');
        const storeUrl = performance.getEntriesByType('resource')
          .find(entry => entry.name.includes('/src/stores/workbenchStore.ts')).name;
        const { useWorkbenchStore } = await import(storeUrl);
        const thread = await api.createThread(true);
        await api.renameThread(thread.id, `等待回答验收：${scenario}`);
        await useWorkbenchStore.getState().reloadThreads();
        await useWorkbenchStore.getState().selectThread(thread.id);
        await api.startTurn(thread.id, '验证等待用户回答及继续处理。');
        return thread.id;
      }, scenario);
      const read = () => page.evaluate(async id => {
        const api = await import('/src/api/runtime.ts');
        return api.readThread(id);
      }, currentThread);
      await expect.poll(async () => (await read()).userInputs.length, { timeout: 30000 }).toBe(1);
      if (fixtureError) throw fixtureError;
      const pending = await read();
      assert.equal(pending.userInputs[0].request.expiresAtMs, null);
      assert.equal(pending.userInputs[0].resolution, null);
      assert.equal(calls, 1, 'waiting must not call the Provider again');
      const requestId = pending.userInputs[0].request.id;
      await page.reload();
      await expect(page.locator('.user-input-question-text')).toBeVisible({ timeout: 30000 });
      const restored = await read();
      assert.equal(restored.userInputs[0].request.id, requestId);
      assert.equal(restored.userInputs[0].resolution, null);
      assert(!['cancelled', 'completed', 'failed'].includes(restored.lastTurn.state));
      if (scenario === 'question') {
        await page.screenshot({ path: path.join(output, 'pending-question.png') });
        await page.getByRole('button', { name: '保留入口', exact: true }).click();
        await page.getByRole('button', { name: '提交回答', exact: true }).click();
      } else if (scenario === 'continuation') {
        await page.getByRole('button', { name: '继续执行', exact: true }).click();
      } else {
        await page.getByRole('button', { name: '停止生成', exact: true }).click();
      }
      const expected = scenario === 'cancel' ? 'cancelled' : 'completed';
      await expect.poll(async () => (await read()).lastTurn.state, { timeout: 30000 }).toBe(expected);
      if (fixtureError) throw fixtureError;
      const completed = await read();
      assert.equal(completed.lastTurn.turnId, pending.lastTurn.turnId);
      assert.equal(completed.userInputs[0].resolution.action, scenario === 'cancel' ? 'cancelled' : 'answered');
      assert.equal(calls, scenario === 'cancel' ? 1 : 2);
      await expect(page.locator('.user-input-question-text')).toHaveCount(0);
      await page.reload();
      await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 30000 });
      assert.equal((await read()).lastTurn.state, expected);
      results.push({ scenario, state: expected, providerCalls: calls, restoredPending: true, restoredTerminal: true });
    }
    fs.writeFileSync(path.join(output, 'native-result.json'), JSON.stringify({ result: 'PASS', results }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', results }));
  } finally {
    await page?.evaluate(async ({ id, providerId }) => {
      const api = await import('/src/api/runtime.ts');
      if (id) await api.cancelTurn(id).catch(() => {});
      await api.deleteProviderApiKey(providerId);
      await api.deleteProvider(providerId);
    }, { id: currentThread, providerId }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
