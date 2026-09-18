// Isolated pnpm tauri dev: com.kcoder.validation.disclosure, Vite 1499 / CDP 9499.
// A local SSE provider pauses between real file tool batches for UI interaction.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

(async () => {
  const requests = [];
  const providerId = 'tool-disclosure-native-fixture';
  const workspace = path.resolve('.tmp-ui/tool-disclosure/workspace');
  fs.mkdirSync(workspace, { recursive: true });
  fs.writeFileSync(path.join(workspace, 'sample.txt'), 'Native disclosure fixture\n');
  const server = http.createServer((req, res) => {
    req.resume();
    req.on('end', () => requests.push(res));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, page;
  const send = async (index, ids, missing = false) => {
    await expect.poll(() => requests.length, { timeout: 30000 }).toBe(index + 1);
    requests[index].writeHead(200, { 'Content-Type': 'text/event-stream' });
    requests[index].end(`data: ${JSON.stringify({ choices: [{ delta: ids.length ? {
      tool_calls: ids.map((id, index) => ({ index, id, type: 'function', function: {
        name: 'read_file', arguments: JSON.stringify({ path: missing ? 'missing.txt' : 'sample.txt' }),
      } })),
    } : { content: '手动展开验证完成。' }, finish_reason: ids.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
  };
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9499');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1499'));
    assert(page, 'isolated WebView required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ workspace, providerId, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: providerId, name: '本地展开状态验收', kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [],
        apiKey: 'dummy-local-disclosure', activate: true });
      await api.setApprovalMode('full_access');
    }, { workspace, providerId, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threadId = await page.evaluate(async workspace => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      // Let the app release the old thread's workspace selection before preparing a new one.
      useWorkbenchStore.setState({ activeThreadId: null });
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      await api.switchWorkspace(workspace, true);
      const thread = await api.createThread(true);
      if (!thread.workspacePath?.replaceAll('\\', '/').endsWith('/.tmp-ui/tool-disclosure/workspace')) {
        throw new Error('Validation thread must be bound to the isolated fixture workspace');
      }
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '检查文件并验证连续操作的展开状态。', [], 'craft');
      return thread.id;
    }, workspace);
    await send(0, ['read-1', 'read-2']);
    await expect.poll(() => requests.length, { timeout: 30000 }).toBe(2);
    const group = page.locator('.turn-tool-group');
    const summary = group.locator(':scope > summary');
    await expect(group.locator('.turn-timeline-tool')).toHaveCount(2);
    await expect(group).toHaveClass(/--completed/);
    await expect(group).toHaveJSProperty('open', false);
    await summary.click();
    await expect(group).toHaveJSProperty('open', true);

    await send(1, ['read-3']);
    await expect.poll(() => requests.length, { timeout: 30000 }).toBe(3);
    await expect(group.locator('.turn-timeline-tool')).toHaveCount(3);
    await expect(group).toHaveClass(/--completed/);
    await expect(group).toHaveJSProperty('open', true);

    await send(2, ['read-4'], true);
    await expect.poll(() => requests.length, { timeout: 30000 }).toBe(4);
    await expect(summary).toContainText('包含失败');
    await expect(group).toHaveJSProperty('open', true);
    await summary.focus();
    await summary.press('Enter');
    await expect(group).toHaveJSProperty('open', false);
    await summary.press('Space');
    await expect(group).toHaveJSProperty('open', true);
    await page.screenshot({ path: '.tmp-ui/tool-disclosure/native-expanded.png' });

    await send(3, []);
    await expect.poll(async () => page.evaluate(async id =>
      (await (await import('/src/api/runtime.ts')).readThread(id)).lastTurn?.state, threadId)).toBe('completed');
    const execution = page.locator('.turn-execution');
    await expect(execution).toHaveJSProperty('open', false);
    await execution.locator(':scope > summary').click();
    await expect(group).toHaveJSProperty('open', true);
    await expect(group.locator('.turn-timeline-tool')).toHaveCount(4);
    const result = { result: 'PASS', providerRequests: requests.length, operations: 4,
      checks: ['default collapse', 'manual expand', 'append and complete', 'failure', 'Enter/Space', 'turn completion preserves group choice'] };
    fs.writeFileSync('.tmp-ui/tool-disclosure/native-result.json', JSON.stringify(result, null, 2));
    console.log(JSON.stringify(result));
  } finally {
    await page?.evaluate(async providerId => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey(providerId);
      await api.deleteProvider(providerId);
    }, providerId).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
