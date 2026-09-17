// Isolated pnpm tauri dev host: Vite 1466, WebView2 CDP 9466.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');
const assert = require('node:assert/strict');

(async () => {
  let stage = 0, fixtureError, browser, page;
  const titles = ['新增公共 Token Service 与 DTO', 'PPService 接入', '强类型解析', '更新测试并验证'];
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', chunk => body += chunk);
    req.on('end', () => {
      try {
        const request = JSON.parse(body);
        const system = request.messages.filter(m => m.role === 'system').map(m => m.content).join('\n');
        assert(system.includes('[执行计划同步]'));
        assert(system.includes('success=true'));
        const current = stage++;
        assert(current < 3, 'fixture must finish in three requests');
        if (current > 0) {
          assert(system.includes(`"revision":${current}`));
          const snapshotLine = system.split('\n').find(line => line.startsWith('{"revision":'));
          const snapshot = JSON.parse(snapshotLine);
          assert.deepEqual(snapshot.steps.map(s => s.status), current === 1
            ? ['in_progress', 'pending', 'pending', 'pending'] : Array(4).fill('completed'));
        }
        const calls = current < 2 ? [{ index: 0, id: `sync-${current}`, type: 'function', function: {
          name: 'update_plan', arguments: JSON.stringify({ steps: titles.map((step, index) => ({
            id: String(index + 1), step,
            status: current === 1 ? 'completed' : index === 0 ? 'in_progress' : 'pending',
          })) }),
        } }] : [];
        res.writeHead(200, { 'Content-Type': 'text/event-stream' });
        res.end(`data: ${JSON.stringify({ choices: [{ delta: {
          content: current === 2 ? '四项验证计划已同步完成。' : '同步验证计划状态。',
          ...(calls.length ? { tool_calls: calls } : {}),
        }, finish_reason: calls.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
      } catch (error) {
        fixtureError = error;
        res.writeHead(500); res.end('fixture failed');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9466');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1466'));
    assert(page, 'isolated plan-sync host required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const workspace = path.resolve('outputs/plan-sync-native-workspace');
    fs.mkdirSync(workspace, { recursive: true });
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'plan-sync-fixture', name: '本地计划同步验收', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-local-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threadId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '四步计划同步原生验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '仅用 update_plan 验证四步状态同步，不执行实际代码修改。', [], 'craft');
      return thread.id;
    });
    const trigger = page.locator('.plan-progress-trigger');
    await expect(trigger).toContainText('第 4/4 步', { timeout: 60000 });
    await expect(page.getByText('四项验证计划已同步完成。', { exact: true })).toBeVisible();
    await trigger.click();
    const details = page.getByRole('dialog', { name: '执行计划详情' });
    await expect(details.locator('.plan-progress-step--completed')).toHaveCount(4);
    const plan = await page.evaluate(async id => (await import('/src/api/runtime.ts')).getPlan(id), threadId);
    assert.equal(plan.revision, 2);
    assert(plan.steps.every(step => step.status === 'completed'));
    await page.reload();
    await expect(trigger).toContainText('第 4/4 步');
    await trigger.click();
    await expect(details.locator('.plan-progress-step--completed')).toHaveCount(4);
    await page.screenshot({ path: path.resolve('outputs/plan-sync-native.png') });
    if (fixtureError) throw fixtureError;
    assert.equal(stage, 3);
    console.log(JSON.stringify({ result: 'PASS', threadId, checks: ['runtime instructions', 'live snapshot revisions', 'real update_plan tool and persistence', 'four completed steps', 'reload restoration'] }));
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('plan-sync-fixture');
      await api.deleteProvider('plan-sync-fixture');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections(); server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
