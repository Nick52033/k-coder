// Run against the isolated waitany Tauri dev host (Vite 1454, WebView2 CDP 9394).
// All model responses are local deterministic SSE fixtures; no external model is used.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');

(async () => {
  let stage = 0;
  let childIds = [];
  const children = new Map();
  const checks = [];
  let fixtureError;
  const reply = (response, content, calls = []) => {
    if (response.writableEnded) return;
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    response.end(`data: ${JSON.stringify({ choices: [{ delta: {
      content, ...(calls.length ? { tool_calls: calls.map((call, index) => ({ index, id: call.id, type: 'function', function: { name: call.name, arguments: JSON.stringify(call.args) } })) } : {}),
    }, finish_reason: calls.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
  };
  const release = (kind) => {
    const response = children.get(kind);
    if (response) reply(response, `${kind} child finished`);
    else setTimeout(() => release(kind), 20).unref();
  };
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      try {
        const input = JSON.parse(body);
        const user = input.messages.find(message => message.role === 'user')?.content;
        if (typeof user === 'string' && user.startsWith('wait-native-child:')) {
          children.set(user.split(':')[1], response);
          return;
        }
        const results = input.messages.filter(message => message.role === 'tool');
        const wait = (id, ids, timeoutMs) => ({ id, name: 'wait_agent', args: { agentIds: ids, timeoutMs } });
        if (stage === 0) {
          assert(input.messages.some(message => typeof message.content === 'string' && message.content.includes('<delegation_scheduling>')));
          assert(input.tools.find(tool => tool.function.name === 'wait_agent').function.parameters.properties.agentIds);
          checks.push('real Provider schema and scheduling prompt');
          stage++;
          reply(response, '启动两个独立子任务，并先检查目录。', [
            { id: 'create-slow', name: 'create_agent', args: { task: 'wait-native-child:slow', label: '慢任务', timeoutMs: 60000 } },
            { id: 'create-fast', name: 'create_agent', args: { task: 'wait-native-child:fast', label: '快任务', timeoutMs: 60000 } },
            { id: 'parent-work', name: 'list_directory', args: { path: '.' } },
          ]);
        } else if (stage === 1) {
          childIds = ['create-slow', 'create-fast'].map(id => JSON.parse(results.find(result => result.tool_call_id === id).content).id);
          assert(childIds.every(Boolean));
          assert(results.some(result => result.tool_call_id === 'parent-work'));
          checks.push('parent performs independent work after spawning children');
          stage++;
          reply(response, '独立检查已完成，现在等待任一子任务。', [wait('wait-timeout', childIds, 20)]);
        } else if (stage === 2) {
          const result = JSON.parse(results.find(result => result.tool_call_id === 'wait-timeout').content);
          assert.equal(result.timedOut, true);
          assert(result.agents.every(agent => ['running', 'queued'].includes(agent.state)));
          checks.push('timeout preserves both running children');
          stage++;
          reply(response, '子任务继续运行，等待结果。', [wait('wait-fast', childIds, 5000)]);
          setTimeout(() => release('fast'), 100);
        } else if (stage === 3) {
          const result = JSON.parse(results.find(result => result.tool_call_id === 'wait-fast').content);
          assert.deepEqual(result.finishedAgentIds, [childIds[1]]);
          assert.equal(result.agents[0].state, 'running');
          checks.push('second child finishes first while first child keeps running');
          stage++;
          reply(response, '已收到快任务结果，继续收集慢任务。', [wait('wait-slow', [childIds[0]], 5000)]);
          setTimeout(() => release('slow'), 100);
        } else {
          const result = JSON.parse(results.find(result => result.tool_call_id === 'wait-slow').content);
          assert.deepEqual(result.finishedAgentIds, [childIds[0]]);
          stage++;
          reply(response, '批量等待原生验证完成。');
        }
      } catch (error) {
        fixtureError = error;
        response.writeHead(500); response.end('fixture assertion failed');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9394');
    page = browser.contexts()[0].pages().find(page => page.url().includes(':1454'));
    assert(page, 'Isolated waitany WebView is required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'wait-native-fixture', name: '本地等待验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-wait-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace: path.resolve('.') });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const parentId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '批量等待原生验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '执行 wait native 主任务。');
      return thread.id;
    });
    await expect(page.getByText('批量等待原生验证完成。', { exact: true })).toBeVisible({ timeout: 30000 });
    if (fixtureError) throw fixtureError;
    assert.equal(stage, 5);
    // Check recovered tool results and labels, not just transient events.
    await page.reload();
    await expect(page.getByText('批量等待原生验证完成。', { exact: true })).toBeVisible();
    const executions = page.locator('.turn-execution');
    for (let i = 0; i < await executions.count(); i++) {
      if (await executions.nth(i).getAttribute('open') === null) await executions.nth(i).locator(':scope > summary').click();
    }
    const groups = page.locator('.turn-tool-group');
    for (let i = 0; i < await groups.count(); i++) {
      if (await groups.nth(i).getAttribute('open') === null) await groups.nth(i).locator(':scope > summary').click();
    }
    const timeoutRow = page.locator('.turn-timeline-tool').filter({ hasText: '本次等待结束，子任务仍在运行' });
    await expect(timeoutRow).toHaveCount(1);
    await expect(timeoutRow.locator('.subagent-task-chip')).toHaveCount(2);
    await expect(timeoutRow).toContainText('等待耗时');
    await expect(page.locator('.turn-timeline-tool').filter({ hasText: '已获取结果' })).toHaveCount(2);
    await timeoutRow.getByRole('button', { name: '查看子智能体 task2', exact: true }).click();
    const drawer = page.getByRole('complementary', { name: '子智能体', exact: true });
    await expect(drawer.locator('.subagent-detail-title')).toContainText('快任务');
    await expect(drawer.locator('.subagent-detail-meta')).toContainText('子任务耗时');
    await expect(drawer.locator('.subagent-message--assistant')).toContainText('fast child finished');
    checks.push('history reload, wait duration, all target chips and matching task details');
    await page.screenshot({ path: path.resolve('docs/subagent-wait-native.png') });
    console.log(JSON.stringify({ result: 'PASS', parentId, checks }));
  } finally {
    for (const response of children.values()) if (!response.writableEnded) reply(response, 'fixture cleanup');
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('wait-native-fixture');
      await api.deleteProvider('wait-native-fixture');
      await api.setApprovalMode('ask');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
