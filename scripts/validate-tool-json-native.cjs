// Isolated Tauri dev: Vite 1477 / WebView2 CDP 9417. Loopback fixtures only.
const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const path = require('node:path');

(async () => {
  const calls = [];
  let scenario = 'recover';
  let attempt = 0;
  const questions = [
    { question: '测试插件如何获取？', options: ['手写最小插件', '下载现成插件'] },
    { question: '验证范围？', options: ['读取 Skill', '检查启用状态'] },
    { question: '文件写入范围？', options: ['隔离测试目录', '仅分析'] },
  ];
  const malformed = JSON.stringify({ questions: [questions[0]] }) + ', ' + JSON.stringify(questions[1]) + ', ' + JSON.stringify(questions[2]) + ']}';
  const frame = data => `data: ${JSON.stringify(data)}\n\n`;
  const tool = args => frame({ choices: [{ delta: { tool_calls: [{ index: 0, id: 'fixture-question', function: { name: 'request_user_input', arguments: args } }] }, finish_reason: 'tool_calls' }] });
  const server = createServer((req, res) => {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      const request = JSON.parse(body);
      calls.push({ scenario, request });
      attempt++;
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      if (scenario === 'exhaust' || scenario === 'cancel' || (scenario === 'recover' && attempt === 1)) {
        res.end(tool(malformed) + 'data: [DONE]\n\n');
      } else if (scenario === 'partial') {
        res.end(frame({ choices: [{ delta: { content: '已收到请求。' } }] }) + tool(malformed) + 'data: [DONE]\n\n');
      } else if (scenario === 'recover' && attempt === 2) {
        res.end(tool(JSON.stringify({ questions })) + 'data: [DONE]\n\n');
      } else {
        res.end(frame({ choices: [{ delta: { content: '参数恢复验证完成。' }, finish_reason: 'stop' }] }) + 'data: [DONE]\n\n');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9417');
    page = browser.contexts()[0].pages().find(p => p.url().startsWith('http://127.0.0.1:1477'));
    if (!page) throw new Error('Isolated validation WebView missing');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async baseUrl => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      await api.saveProviderConfig({ id: 'tool-json-fixture', name: '工具 JSON 验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [{ id: 'fixture', displayName: 'Fixture', contextWindow: 128000, fallback: false }], endpoints: [], apiKey: 'dummy-tool-json-validation', activate: true });
      await useWorkbenchStore.getState().loadProviderCatalog();
      window.toolJsonValidation = { api, useWorkbenchStore };
    }, `http://127.0.0.1:${server.address().port}/v1`);
    const selectThread = () => page.evaluate(async () => {
      const { api, useWorkbenchStore } = window.toolJsonValidation;
      const thread = await api.createThread(true);
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      window.toolJsonValidation.threadId = thread.id;
      return thread.id;
    });
    const threadId = await selectThread();
    await page.evaluate(() => window.toolJsonValidation.useWorkbenchStore.getState().sendMessage('验证提问参数恢复'));
    await expect(page.getByText('k-Coder 提问', { exact: true })).toBeVisible({ timeout: 15000 });
    expect(attempt).toBe(2);
    expect(calls[0].request.messages).toEqual(calls[1].request.messages);
    for (const q of questions) await page.getByRole('button', { name: q.options[0], exact: true }).click();
    await page.screenshot({ path: path.resolve('docs/sys/工具JSON恢复原生验证.png') });
    await page.getByRole('button', { name: '提交回答', exact: true }).click();
    await expect.poll(() => page.evaluate(id => window.toolJsonValidation.api.readThreadHistory(id).then(h => h.lastTurn?.state), threadId)).toBe('completed');
    expect(attempt).toBe(3);
    expect(calls[2].request.messages.some(m => m.role === 'tool')).toBe(true);

    scenario = 'partial'; attempt = 0;
    const partialThread = await selectThread();
    const partial = await page.evaluate(id => window.toolJsonValidation.api.runTurn(id, '验证已有正文的错误', [], 'craft'), partialThread);
    expect(partial.state).toBe('failed');
    expect(attempt).toBe(1);
    const failure = await page.evaluate(id => window.toolJsonValidation.api.readThreadHistory(id), partialThread);
    expect(failure.lastTurn.error).toMatchObject({ code: 'provider_invalid_response', retryable: true, details: { protocolRetries: 0, outputAlreadyStarted: true } });
    expect(failure.lastTurn.error.message).not.toContain('Raw arguments');
    expect(failure.lastTurn.error.message).not.toContain('测试插件如何获取');
    scenario = 'manual'; attempt = 0;
    await page.evaluate(id => window.toolJsonValidation.api.retryTurn(id), partialThread);
    await expect.poll(() => page.evaluate(id => window.toolJsonValidation.api.readThreadHistory(id).then(h => h.lastTurn?.state), partialThread)).toBe('completed');
    expect(attempt).toBe(1);

    scenario = 'exhaust'; attempt = 0;
    const exhaustedThread = await selectThread();
    const exhausted = await page.evaluate(id => window.toolJsonValidation.api.runTurn(id, '验证重试次数上限', [], 'craft'), exhaustedThread);
    expect(exhausted.state).toBe('failed');
    expect(attempt).toBe(6);
    const exhaustedHistory = await page.evaluate(id => window.toolJsonValidation.api.readThreadHistory(id), exhaustedThread);
    expect(exhaustedHistory.lastTurn.error).toMatchObject({ code: 'provider_invalid_response', retryable: true, details: { protocolRetries: 5, outputAlreadyStarted: false } });
    const failedDetail = await page.evaluate(id => window.toolJsonValidation.api.readThread(id), exhaustedThread);
    expect(failedDetail.userInputs).toHaveLength(0);
    console.log('PASS: native malformed JSON recovery, three-question UI submission, partial-output no replay, manual retry, six-attempt bound, no raw arguments or malformed question execution.');
  } finally {
    await page?.evaluate(async () => {
      const { api } = window.toolJsonValidation ?? {};
      if (api) { await api.deleteProviderApiKey('tool-json-fixture'); await api.deleteProvider('tool-json-fixture'); }
    }).catch(() => {});
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
