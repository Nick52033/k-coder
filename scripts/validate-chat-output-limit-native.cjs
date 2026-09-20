// Isolated Tauri dev: com.kcoder.validation.outputlimit / Vite 1494 / CDP 9434.
// Only a loopback SSE fixture is used; reported token counts simulate a gateway.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const { createServer } = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

(async () => {
  const workspace = path.resolve('src-tauri');
  const relativeFile = `target/output-limit-native/${Date.now()}/deck.js`;
  const outputFile = path.join(workspace, relativeFile);
  fs.mkdirSync(path.dirname(outputFile), { recursive: true });
  const content = 'const slide = "native-output-limit-validation";\n'.repeat(400);
  const args = JSON.stringify({ path: relativeFile, content });
  const calls = [];
  let scenario = 'default';
  let fixtureError;
  const frame = value => `data: ${JSON.stringify(value)}\n\n`;
  const server = createServer((req, res) => {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      try {
        const request = JSON.parse(body);
        calls.push({ scenario, model: request.model, maxTokens: request.max_tokens ?? null });
        assert.equal(request.model, 'deepseek-flash');
        assert.equal(request.thinking, undefined, 'vision declaration must keep the ordinary dialect');
        assert.equal(request.max_completion_tokens, undefined);
        if (scenario === 'default') assert.equal(request.max_tokens, undefined);
        else assert.equal(request.max_tokens, 65355, 'configured output limit must reach the wire');
        res.writeHead(200, { 'Content-Type': 'text/event-stream' });
        if (scenario === 'default' || scenario === 'clamped') {
          res.end(frame({ choices: [{ delta: { tool_calls: [{ index: 0, id: 'partial-write',
            function: { name: 'write_file', arguments: args.slice(0, -30) } }] }, finish_reason: 'length' }],
            usage: { prompt_tokens: 12354, completion_tokens: 8192 } }) + 'data: [DONE]\n\n');
        } else if (request.messages.some(message => message.role === 'tool')) {
          assert(request.messages.some(message => message.role === 'tool' && message.content.includes('applied approved change')));
          res.end(frame({ choices: [{ delta: { content: '最大输出参数已生效，完整文件写入验证通过。' }, finish_reason: 'stop' }],
            usage: { prompt_tokens: 1200, completion_tokens: 30 } }) + 'data: [DONE]\n\n');
        } else {
          res.write(frame({ choices: [{ delta: { tool_calls: [{ index: 0, id: 'complete-write',
            function: { name: 'write_file', arguments: args.slice(0, 5000) } }] } }] }));
          res.end(frame({ choices: [{ delta: { tool_calls: [{ index: 0,
            function: { arguments: args.slice(5000) } }] }, finish_reason: 'tool_calls' }],
            usage: { prompt_tokens: 12354, completion_tokens: 9000 } }) + 'data: [DONE]\n\n');
        }
      } catch (error) {
        fixtureError = error;
        res.statusCode = 500;
        res.end('Local output limit validation failed');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9434');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1494'));
    assert(page, 'isolated outputlimit WebView required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 30000 });
    await page.evaluate(async ({ workspace, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'output-limit-fixture', name: '输出上限验证',
        kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl,
        model: 'deepseek-flash', models: [{ id: 'deepseek-flash', displayName: 'deepseek-flash',
          contextWindow: 200000, supportsVision: true, fallback: false }], endpoints: [],
        apiKey: 'dummy-output-limit-fixture', activate: true });
      await api.setApprovalMode('full_access');
    }, { workspace, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible({ timeout: 30000 });
    const threadId = await page.evaluate(async workspace => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '输出上限与截断恢复验证');
      return thread.id;
    }, workspace);
    const failed = await page.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      return api.runTurn(id, '生成演示用脚本文件', [], 'craft');
    }, threadId);
    if (fixtureError) throw fixtureError;
    assert.equal(failed.state, 'failed');
    assert.equal(calls.length, 1, 'length must not blindly replay');
    assert.equal(fs.existsSync(outputFile), false, 'partial call must not execute');
    await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const config = await api.getProviderConfig();
      await api.saveProviderConfig({ ...config, models: config.models.map(model => ({ ...model, maxOutputTokens: 65355 })), activate: true });
    });
    scenario = 'recover';
    await page.evaluate(async id => (await import('/src/api/runtime.ts')).retryTurn(id), threadId);
    await expect.poll(() => page.evaluate(async id =>
      (await import('/src/api/runtime.ts')).readThreadHistory(id).then(h => h.lastTurn?.state), threadId),
    { timeout: 20000 }).toBe('completed');
    if (fixtureError) throw fixtureError;
    assert.equal(calls.length, 3);
    assert.equal(fs.readFileSync(outputFile, 'utf8'), content);
    await page.evaluate(async id => {
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(id);
    }, threadId);
    await expect(page.getByText('最大输出参数已生效，完整文件写入验证通过。', { exact: true })).toBeVisible();
    await page.reload();
    await expect(page.getByText('最大输出参数已生效，完整文件写入验证通过。', { exact: true })).toBeVisible({ timeout: 20000 });
    await page.screenshot({ path: path.resolve('docs/sys/Chat输出上限原生验证.png') });
    scenario = 'clamped';
    const clamped = await page.evaluate(async workspace => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      const thread = await api.createThread(true);
      return api.runTurn(thread.id, '验证上游仍强制截断时保留失败', [], 'craft');
    }, workspace);
    if (fixtureError) throw fixtureError;
    assert.equal(clamped.state, 'failed');
    assert.equal(calls.length, 4);
    assert.equal(fs.readFileSync(outputFile, 'utf8'), content);
    const facts = { threadId, workspace, outputFile, calls, fileBytes: Buffer.byteLength(content),
      defaultState: failed.state, recoveredState: 'completed', clampedState: clamped.state };
    fs.writeFileSync(path.resolve('docs/sys/Chat输出上限原生结果.json'), JSON.stringify(facts, null, 2));
    console.log('PASS: missing-limit truncation, 65355 on wire, fragmented complete write, explicit retry, refresh recovery, clamped-limit rejection.');
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('output-limit-fixture');
      await api.deleteProvider('output-limit-fixture');
    }).catch(() => {});
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
