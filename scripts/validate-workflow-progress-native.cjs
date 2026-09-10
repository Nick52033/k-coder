// Requires the isolated workflowprogress Tauri dev host on Vite 1455 / CDP 9395.
// Real runtime, IPC and persistence; the model uses deterministic local SSE.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');

(async () => {
  let stage = 0, pending, fixtureError;
  const send = (res, text, calls = [], finish) => {
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.end(`data: ${JSON.stringify({ choices: [{ delta: { content: text, ...(calls.length ? { tool_calls: calls.map((c, i) => ({ index: i, id: c.id, type: 'function', function: { name: c.name, arguments: JSON.stringify(c.args) } })) } : {}) }, finish_reason: finish || (calls.length ? 'tool_calls' : 'stop') }] })}\n\ndata: [DONE]\n\n`);
  };
  const advance = (id, nodeId) => ({ id, name: 'complete_workflow_node', args: { nodeId, summary: '本地验证节点完成', evidence: ['deterministic local validation'] } });
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', c => body += c);
    req.on('end', () => {
      try {
        const input = JSON.parse(body);
        for (const message of input.messages.filter(m => m.role === 'tool')) {
          assert(!message.content.includes('tool execution failed'), message.content.slice(0, 120));
        }
        const current = stage++;
        if (current === 0) send(res, '建立初始计划。', [{ id: 'plan', name: 'update_plan', args: { steps: Array.from({ length: 8 }, (_, i) => ({ id: String(i + 1), step: `旧计划 ${i + 1}`, status: i === 0 ? 'in_progress' : 'pending' })) } }]);
        else if (current === 1) send(res, '需求已确认。', [advance('requirements', 'requirements-analysis')]);
        else if (current === 2) send(res, '已进入设计，模拟本次响应中断。', [], 'length');
        else if (current === 3) send(res, '重试后完成设计。', [advance('design', 'interface-architecture-design')]);
        else if (current === 4) pending = res;
        else throw new Error(`unexpected request ${current}`);
      } catch (e) { fixtureError = e; res.writeHead(500); res.end('local fixture failed'); }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9395');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1455'));
    assert(page, 'isolated workflowprogress host required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'workflow-progress-fixture', name: '本地进度验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-local-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace: path.resolve('.') });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threadId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '机器人进度原生验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '验证节点进度', [], 'craft', 'fullstack-delivery');
      return thread.id;
    });
    const trigger = page.locator('.plan-progress-trigger');
    const control = page.getByLabel('机器人工作流 全栈开发机器人');
    await expect(page.getByText('生成失败', { exact: true })).toBeVisible({ timeout: 30000 });
    await page.reload();
    await expect(trigger).toContainText('第 2/8 步');
    await page.locator('.turn-execution > summary').last().click();
    await expect(page.getByRole('button', { name: '重试', exact: true })).toBeVisible();
    await expect(trigger).toHaveCount(1);
    await expect(trigger).toContainText('第 2/8 步');
    await expect(control).toContainText('2 / 8');
    await page.getByRole('button', { name: '重试', exact: true }).click();
    await expect(trigger).toHaveCount(1);
    await expect(trigger).toContainText('第 3/8 步', { timeout: 30000 });
    await expect(control).toContainText('3 / 8');
    await trigger.click();
    const details = page.getByRole('dialog', { name: '执行计划详情' });
    await expect(details.locator('.plan-progress-step--completed')).toHaveCount(2);
    await expect(details.locator('.plan-progress-step--in_progress')).toContainText('原型 HTML');
    assert(pending, 'prototype request is active');
    send(pending, '进度验证完成，保留机器人原型节点。');
    await expect(page.getByText('进度验证完成，保留机器人原型节点。', { exact: true })).toBeVisible();
    await page.reload();
    await expect(trigger).toContainText('第 3/8 步');
    await expect(control).toContainText('3 / 8');
    const persisted = await page.evaluate(async (id) => {
      const api = await import('/src/api/runtime.ts');
      return { plan: await api.getPlan(id), run: await api.getWorkflowRun(id) };
    }, threadId);
    assert.equal(persisted.plan.steps[0].status, 'in_progress');
    assert.equal(persisted.run.currentNodeIndex, 2);
    await trigger.click();
    await expect(details.locator('.plan-progress-step--completed')).toHaveCount(2);
    await page.screenshot({ path: path.resolve('docs/workflow-progress-native.png') });
    if (fixtureError) throw fixtureError;
    assert.equal(stage, 5);
    console.log(JSON.stringify({ result: 'PASS', threadId, checks: ['real node transitions', 'failure at step 2', 'UI retry continues at step 3', 'single progress capsule', 'popover completed nodes', 'reload with stale persisted plan'] }));
  } finally {
    if (pending && !pending.writableEnded) send(pending, 'cleanup');
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('workflow-progress-fixture');
      await api.deleteProvider('workflow-progress-fixture');
      await api.setApprovalMode('ask');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
