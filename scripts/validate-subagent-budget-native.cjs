// Isolated Tauri host: com.kcoder.validation.subagentbudget, Vite 1488, CDP 9428.
// Uses local deterministic Provider responses, never an external model.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');

(async () => {
  const calls = new Map();
  let schemaChecked = false;
  let fixtureError;
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      try {
        const input = JSON.parse(body);
        const task = [...input.messages].reverse().find(message => message.role === 'user')?.content;
        const child = typeof task === 'string' && task.startsWith('budget-native-child:');
        const index = calls.get(task) ?? 0;
        calls.set(task, index + 1);
        let delta = { content: child ? '预算验证子任务完成。' : '预算验证主会话就绪。' };
        let finish = 'stop';
        let usage = { prompt_tokens: 10, completion_tokens: 2, total_tokens: 12 };
        if (!child) {
          const definition = input.tools.find(tool => tool.function.name === 'create_agent').function;
          assert(definition.description.includes('explicitly requested by the user'));
          assert(definition.parameters.properties.tokenBudget.description.includes('input and output tokens'));
          schemaChecked = true;
        } else if (index === 0) {
          delta = { tool_calls: [{ index: 0, id: 'budget-inspect', type: 'function', function: {
            name: 'list_directory', arguments: JSON.stringify({ path: '.' }),
          } }] };
          finish = 'tool_calls';
          usage = { prompt_tokens: 7476, completion_tokens: 302, total_tokens: 7778 };
        } else {
          assert.equal(index, 1, 'unexpected child retry');
          usage = { prompt_tokens: 56218, completion_tokens: 402, total_tokens: 56620 };
        }
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end(`data: ${JSON.stringify({ choices: [{ delta, finish_reason: finish }], usage })}\n\ndata: [DONE]\n\n`);
      } catch (error) {
        fixtureError = error;
        response.writeHead(500);
        response.end('fixture assertion failed');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9428');
    page = browser.contexts()[0].pages().find(page => page.url().includes(':1488'));
    assert(page, 'isolated subagentbudget WebView is required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'budget-native-fixture', name: '本地预算验证', kind: 'open_ai_compatible',
        transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [],
        apiKey: 'dummy-budget-validation', activate: true });
      await api.setApprovalMode('full_access');
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, workspace: path.resolve('.') });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const parentId = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(entry => entry.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      const thread = await api.createThread(true);
      await api.renameThread(thread.id, '子智能体预算原生验证');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      try { await api.startTurn(thread.id, 'budget-native-parent'); }
      catch (error) { throw new Error(JSON.stringify(error)); }
      return thread.id;
    });
    await expect(page.getByText('预算验证主会话就绪。', { exact: true })).toBeVisible({ timeout: 30000 });
    const results = await page.evaluate(async parentThreadId => {
      const api = await import('/src/api/runtime.ts');
      const results = [];
      for (const tokenBudget of [null, 8000]) {
        const agent = await api.createSubagent({ parentThreadId, task: `budget-native-child:${tokenBudget ?? 'unlimited'}`,
          label: tokenBudget === null ? '无预算子任务' : '预算耗尽子任务', forkTurns: 'all',
          capabilities: ['list_directory'], timeoutMs: 60000, ...(tokenBudget === null ? {} : { tokenBudget }) });
        results.push(await api.waitSubagent(agent.id, 30000));
      }
      return results;
    }, parentId);
    if (fixtureError) throw fixtureError;
    assert(schemaChecked);
    assert.deepEqual(results.map(agent => [agent.state, agent.tokenBudget, agent.tokensUsed]), [
      ['completed', null, 64398], ['failed', 8000, 64398],
    ]);
    const resumeError = await page.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      try { await api.resumeSubagent(id); return null; } catch (error) { return JSON.stringify(error); }
    }, results[1].id);
    assert(resumeError?.includes('token budget is exhausted'));
    assert.equal(calls.get('budget-native-child:8000'), 2);
    await page.reload();
    const summary = page.getByRole('region', { name: '本会话子智能体状态' });
    await summary.getByRole('button').filter({ hasText: '预算耗尽子任务' }).click();
    const drawer = page.getByRole('complementary', { name: '子智能体', exact: true });
    await expect(drawer.getByRole('button', { name: '恢复', exact: true })).toBeDisabled();
    await expect(drawer.getByRole('alert')).toContainText('Token 预算已耗尽');
    await expect(drawer.getByRole('alert')).toContainText('64,398 / 8,000 tokens');
    await page.screenshot({ path: path.resolve('docs/sys/subagent-budget-native.png') });
    await drawer.getByRole('button', { name: '返回列表' }).click();
    await expect(drawer.locator('.subagent-row').filter({ hasText: '预算耗尽子任务' }).getByRole('button', { name: '恢复', exact: true })).toBeDisabled();
    console.log(JSON.stringify({ result: 'PASS', parentId, checks: [
      'Provider receives explicit-user-budget guidance', 'inherited child completes at 64398 with no budget',
      'explicit 8000 budget fails at 64398', 'backend refuses exhausted resume without another Provider call',
      'reload preserves exhausted UI, usage and disabled detail/list resume',
    ] }));
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('budget-native-fixture');
      await api.deleteProvider('budget-native-fixture');
      await api.setApprovalMode('ask');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
