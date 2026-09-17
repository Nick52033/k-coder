// Requires isolated pnpm tauri dev: identifier com.kcoder.validation.readnonfatal,
// Vite 1496, WebView2 CDP 9496. Real runtime/IPC/files/UI, local deterministic SSE.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

(async () => {
  let browser, page, fixtureError, scenario, stage = 0;
  const checks = [];
  const workspace = path.resolve('outputs/read-convergence-workspace');
  const dataRoot = path.join(process.env.APPDATA, 'com.kcoder.validation.readnonfatal', 'runtime-data');
  const providerId = 'read-convergence-native-fixture';
  fs.mkdirSync(workspace, { recursive: true });
  fs.writeFileSync(path.join(workspace, 'source.txt'), Array.from({ length: 2000 }, (_, i) =>
    `line ${String(i + 1).padStart(4, '0')} context ${'x'.repeat(20)}\n`).join(''));
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      try {
        const request = JSON.parse(body), current = stage++;
        assert(current < 26, 'loop must remain bounded');
        assert(!request.messages.filter(m => m.role === 'system').some(m => m.content.includes('[Host-enforced read recovery]')));
        if (scenario === 'complete' && current > 0 && current <= 5) {
          const result = request.messages.find(m => m.role === 'tool' && m.tool_call_id === `read-${current - 1}`);
          assert(result?.content.includes('line 0001 context'), 'read body must reach the next request');
          if (current > 1) assert(result.content.includes('already observed'));
        }
        if (scenario === 'complete' && current === 7) {
          const result = request.messages.find(m => m.role === 'tool' && m.tool_call_id === 'read-6');
          assert(result?.content.includes('line 0001 context'));
          assert(!result.content.includes('already observed'), 'node transition must reset the advisory context');
        }
        let calls = [{ id: `read-${current}`, name: 'read_file', args: {
          path: 'source.txt', startLine: scenario === 'complete' ? 1 : current + 1,
          lineCount: scenario === 'complete' ? 3 : 2000 - current,
        } }];
        if (scenario === 'complete' && current === 5) calls = [{ id: 'advance-node', name: 'complete_workflow_node', args: {
          nodeId: 'requirements-analysis', summary: '读取验证完成', evidence: ['local native fixture'],
        } }];
        if ((scenario === 'complete' && current === 7) || (scenario === 'compaction' && current === 12)) calls = [];
        res.writeHead(200, { 'Content-Type': 'text/event-stream' });
        res.end(`data: ${JSON.stringify({ choices: [{ delta: {
          content: calls.length ? '检查需要的正文。' : `${scenario} 原生读取验证完成。`,
          ...(calls.length ? { tool_calls: calls.map((c, index) => ({ index, id: c.id, type: 'function',
            function: { name: c.name, arguments: JSON.stringify(c.args) } })) } : {}),
        }, finish_reason: calls.length ? 'tool_calls' : 'stop' }],
        ...(scenario.includes('compaction') ? { usage: { prompt_tokens: 100000, completion_tokens: 10, total_tokens: 100010 } } : {}),
        })}\n\ndata: [DONE]\n\n`);
      } catch (error) { fixtureError = error; res.writeHead(500); res.end('local fixture failed'); }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9496');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1496'));
    assert(page, 'isolated WebView required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace, providerId }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: providerId, name: '本地读取收敛验收', kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [],
        apiKey: 'dummy-local-read-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace, providerId });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    for (const name of ['complete', 'compaction', 'no-progress', 'no-progress-compaction']) {
      scenario = name; stage = 0;
      const threadId = await page.evaluate(async ({ name, workspace }) => {
        const api = await import('/src/api/runtime.ts');
        await api.switchWorkspace(workspace, true);
        const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
        const { useWorkbenchStore } = await import(storeUrl);
        const thread = await api.createThread(true);
        await api.renameThread(thread.id, `读取收敛验收 ${name}`);
        await useWorkbenchStore.getState().reloadThreads();
        await useWorkbenchStore.getState().selectThread(thread.id);
        await api.startTurn(thread.id, '验证本地文件读取与任务收敛。', [], 'craft', name === 'complete' ? 'fullstack-delivery' : undefined);
        return thread.id;
      }, { name, workspace });
      const expectedState = name.startsWith('no-progress') ? 'failed' : 'completed';
      let actualState;
      await expect.poll(async () => {
        actualState = await page.evaluate(async id =>
          (await (await import('/src/api/runtime.ts')).readThread(id)).lastTurn?.state, threadId);
        return actualState;
      }, { timeout: 90000 }).toMatch(/^(completed|failed|cancelled)$/);
      if (fixtureError) throw fixtureError;
      assert.equal(actualState, expectedState);
      const events = fs.readFileSync(path.join(dataRoot, 'sessions', `${threadId}.jsonl`), 'utf8').trim().split('\n').map(JSON.parse);
      const reads = events.filter(e => e.type === 'tool_result' && e.data.name === 'read_file').map(e => e.data.result);
      const compactions = events.filter(e => e.type === 'context_compacted').length;
      assert(reads.every(r => r.success && r.metadata.contentSuppressed !== true));
      assert(reads.every(r => r.output.includes(name === 'complete' ? 'line 0001 context' : 'line 2000 context')));
      if (name.includes('compaction')) assert(compactions >= 2);
      if (name === 'compaction') { assert.equal(reads.length, 12); }
      else if (name === 'complete') {
        assert.equal(stage, 8); assert.equal(reads.length, 6);
        assert(events.some(e => e.type === 'tool_result' && e.data.name === 'complete_workflow_node' && e.data.result.success));
      } else {
        assert.equal(stage, 20);
        assert(events.some(e => e.type === 'turn_failed' && e.data.message.includes('无实质进展')));
      }
      const terminalText = expectedState === 'completed'
        ? page.getByText(`${name} 原生读取验证完成。`, { exact: true })
        : page.getByText(/连续 15 轮（3 个检查窗口）无实质进展/).first();
      if (expectedState === 'failed') {
        const disclosure = page.locator('details.turn-execution--failed');
        await expect(disclosure).toHaveJSProperty('open', false);
        await disclosure.locator(':scope > summary').click();
      }
      await expect(terminalText).toBeVisible();
      await terminalText.scrollIntoViewIfNeeded();
      await expect(terminalText).toBeInViewport();
      await page.screenshot({ path: path.resolve(`outputs/read-convergence-${name}.png`) });
      checks.push({ name, state: expectedState, providerRequests: stage, reads: reads.length, compactions });
    }
    fs.writeFileSync(path.resolve('outputs/read-convergence-native-result.json'), JSON.stringify({ result: 'PASS', checks }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', checks }));
  } finally {
    await page?.evaluate(async providerId => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey(providerId); await api.deleteProvider(providerId);
    }, providerId).catch(() => {});
    await browser?.close(); server.closeAllConnections(); server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
