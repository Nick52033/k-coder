// Requires the isolated read-convergence Tauri host on Vite 1456 / CDP 9396.
// Real runtime, IPC and workflow persistence; all model responses use local deterministic SSE.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');

(async () => {
  let stage = 0;
  let fixtureError;
  const checks = [];
  const readCall = id => ({ id, name: 'read_file', args: { path: 'Cargo.toml' } });
  const reply = (response, content, calls = []) => {
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    response.end(`data: ${JSON.stringify({ choices: [{ delta: {
      content,
      ...(calls.length ? { tool_calls: calls.map((call, index) => ({
        index,
        id: call.id,
        type: 'function',
        function: { name: call.name, arguments: JSON.stringify(call.args) },
      })) } : {}),
    }, finish_reason: calls.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
  };
  const toolOutput = (input, id) => {
    const message = input.messages.find(item => item.role === 'tool' && item.tool_call_id === id);
    assert(message, `missing tool result ${id} at stage ${stage}`);
    return message.content;
  };
  const toolPayload = (input, id) => {
    const output = toolOutput(input, id);
    const payloadStart = output.indexOf('{');
    assert.notEqual(payloadStart, -1, `missing JSON tool payload ${id} at stage ${stage}`);
    return JSON.parse(output.slice(payloadStart));
  };
  const observationType = (input, id) => toolPayload(input, id).type;
  const hasRecovery = input => input.messages.some(message =>
    message.role === 'system'
      && typeof message.content === 'string'
      && message.content.includes('[Host-enforced read recovery]'));

  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      try {
        const input = JSON.parse(body);
        const current = stage++;
        if (current === 0) {
          reply(response, '读取第一节点输入。', [readCall('node-1-read')]);
        } else if (current === 1) {
          assert(toolOutput(input, 'node-1-read').includes('name = "k-coder"'));
          reply(response, '检查第一节点已有观察。', [readCall('node-1-repeat-1')]);
        } else if (current === 2) {
          assert.equal(observationType(input, 'node-1-repeat-1'), 'read_observation_already_covered');
          reply(response, '触发第一节点恢复。', [readCall('node-1-repeat-2')]);
        } else if (current === 3) {
          assert.equal(observationType(input, 'node-1-repeat-2'), 'read_observation_recovery_required');
          assert(hasRecovery(input));
          checks.push('first node receives a bounded recovery correction');
          reply(response, '完成第一节点并进入下一节点。', [{
            id: 'complete-node-1',
            name: 'complete_workflow_node',
            args: {
              nodeId: 'requirements-analysis',
              summary: '本地重复读取收敛验证完成第一节点',
              evidence: ['deterministic local native validation'],
            },
          }]);
        } else if (current === 4) {
          const transition = toolPayload(input, 'complete-node-1');
          assert.equal(transition.nextNode.id, 'interface-architecture-design');
          assert(!hasRecovery(input));
          reply(response, '第二节点重新读取所需正文。', [readCall('node-2-read')]);
        } else if (current === 5) {
          assert(toolOutput(input, 'node-2-read').includes('name = "k-coder"'));
          checks.push('successful workflow transition starts a fresh read scope');
          reply(response, '检查第二节点已有观察。', [readCall('node-2-repeat-1')]);
        } else if (current === 6) {
          assert.equal(observationType(input, 'node-2-repeat-1'), 'read_observation_already_covered');
          reply(response, '触发第二节点恢复。', [readCall('node-2-repeat-2')]);
        } else if (current === 7) {
          assert.equal(observationType(input, 'node-2-repeat-2'), 'read_observation_recovery_required');
          assert(hasRecovery(input));
          reply(response, '纠偏后先完成其他有效检查。', [{
            id: 'intervening-list',
            name: 'list_directory',
            args: { path: '.' },
          }]);
        } else if (current === 8) {
          assert(toolOutput(input, 'intervening-list').includes('Cargo.toml'));
          assert(!hasRecovery(input));
          reply(response, '后续批次再次读取。', [readCall('node-2-late-read')]);
        } else if (current === 9) {
          assert.equal(observationType(input, 'node-2-late-read'), 'read_observation_recovery_required');
          assert(hasRecovery(input));
          checks.push('stale correction cannot hard-stop a later provider batch');
          reply(response, '原生重复读取收敛验证完成。');
        } else {
          throw new Error(`unexpected provider request ${current}`);
        }
      } catch (error) {
        fixtureError = error;
        response.writeHead(500, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify({ error: { message: error.message } }));
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));

  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9396');
    page = browser.contexts()[0].pages().find(candidate => candidate.url().includes(':1456'));
    assert(page, 'isolated read-convergence WebView is required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({
        id: 'read-convergence-native-fixture',
        name: '本地重复读取验证',
        kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions',
        baseUrl,
        model: 'fixture',
        models: [],
        endpoints: [],
        apiKey: 'dummy-read-convergence-validation',
        activate: true,
      });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace: path.resolve('.') });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threadId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource')
        .find(entry => entry.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '重复读取收敛原生验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '执行跨节点重复读取收敛验证。', [], 'craft', 'fullstack-delivery');
      return thread.id;
    });
    await expect(page.getByText('原生重复读取收敛验证完成。', { exact: true }))
      .toBeVisible({ timeout: 60000 });
    if (fixtureError) throw fixtureError;
    assert.equal(stage, 10);
    await expect(page.getByText('生成失败', { exact: true })).toHaveCount(0);
    const history = await page.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      return api.readThread(id);
    }, threadId);
    assert.equal(history.lastTurn.state, 'completed');
    await page.screenshot({ path: path.resolve('docs/read-convergence-native.png') });
    console.log(JSON.stringify({ result: 'PASS', threadId, providerRequests: stage, checks }));
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('read-convergence-native-fixture');
      await api.deleteProvider('read-convergence-native-fixture');
      await api.setApprovalMode('ask');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
