// Isolated pnpm tauri dev host: Vite 1477 / WebView2 CDP 9417.
// Exercises real IPC, persistence, compaction and HTTP 402 recovery using a local model fixture.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

(async () => {
  const output = path.resolve('.tmp-ui/task-continuation');
  fs.mkdirSync(output, { recursive: true });
  const workspace = fs.mkdtempSync(path.join(output, 'workspace-'));
  let stage = 0, scenario = '', fixtureError, pending;
  const requests = [];
  const steps = (phase) => Array.from({ length: 5 }, (_, i) => ({
    id: String(i + 1), step: ['检查入口', '实现组件', '挂载组件', '添加测试', '验证交付'][i],
    status: phase === 'done' ? 'completed' : phase === 'resume' ? (i < 4 ? 'completed' : 'in_progress') : (i ? 'pending' : 'in_progress'),
  }));
  const send = (res, text, calls = []) => {
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.end(`data: ${JSON.stringify({ choices: [{ delta: { content: text,
      ...(calls.length ? { tool_calls: calls.map((c, i) => ({ index: i, id: c.id, type: 'function', function: { name: c.name, arguments: JSON.stringify(c.args) } })) } : {}) },
      finish_reason: calls.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
  };
  const plan = (phase) => ({ id: `plan-${scenario}-${phase}`, name: 'update_plan', args: { steps: steps(phase) } });
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', chunk => body += chunk);
    req.on('end', () => {
      try {
        const input = JSON.parse(body);
        requests.push({ scenario, stage, messages: input.messages });
        for (const message of input.messages.filter(m => m.role === 'tool')) {
          assert(!message.content.includes('tool execution failed'), message.content.slice(0, 200));
        }
        switch (stage++) {
          case 0: send(res, '确认这次任务的目标。', [{ id: `ask-${scenario}`, name: 'request_user_input', args: { questions: [{ question: '状态入口如何处理？', options: ['做成可点击状态面板', '移除入口'] }] } }]); break;
          case 1: send(res, '按照已选择的可点击状态面板方案执行。', [plan('initial')]); break;
          case 2: send(res, '组件已经实现，接着补充回归测试。', [{ id: `write-${scenario}`, name: 'write_file', args: { path: `${scenario}.txt`, content: 'existing work survives quota recovery\n' } }]); break;
          case 3:
            res.writeHead(402, { 'Content-Type': 'application/json' });
            res.end(JSON.stringify({ error: { message: 'You exceeded your current quota (local fixture)' } }));
            break;
          case 4: {
            const system = input.messages.filter(m => m.role === 'system').map(m => m.content).join('\n');
            const history = input.messages.filter(m => m.role !== 'system').map(m => JSON.stringify(m.content)).join('\n');
            assert(system.includes('<interrupted_task_continuation>'));
            assert(system.includes('验证交付') && system.includes('in_progress'));
            assert(history.includes('做成可点击状态面板'));
            assert(history.includes('组件已经实现，接着补充回归测试'));
            assert(history.includes('User clarifications'));
            assert(history.includes('historical reports, not current file contents'));
            send(res, '沿用已确认选择与已有修改，继续剩余验证。', [plan('resume')]);
            break;
          }
          case 5: pending = res; break;
          case 6: send(res, '验证交付已完成，原有修改没有重复执行。'); break;
          default: throw new Error(`unexpected fixture request ${stage - 1}`);
        }
      } catch (error) {
        fixtureError = error;
        res.writeHead(400, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: { message: 'local fixture assertion failed' } }));
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, page;
  const results = [];
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9417');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1477'));
    assert(page, 'isolated task continuation host required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ baseUrl, workspace }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: 'continuation-fixture', name: '本地续做验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl,
        model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-local-continuation', activate: true });
      await api.setApprovalMode('full_access');
    }, { workspace, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    for (const mode of ['continue', 'retry']) {
      scenario = mode;
      stage = 0;
      pending = null;
      const threadId = await page.evaluate(async (mode) => {
        const api = await import('/src/api/runtime.ts');
        const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
        const { useWorkbenchStore } = await import(storeUrl);
        const thread = await api.createThread(true);
        await api.renameThread(thread.id, `额度恢复 ${mode}`);
        await useWorkbenchStore.getState().reloadThreads();
        await useWorkbenchStore.getState().selectThread(thread.id);
        await useWorkbenchStore.getState().sendMessage('完善状态入口', [], 'craft');
        return thread.id;
      }, mode);
      await expect(page.getByText('状态入口如何处理？', { exact: true })).toBeVisible();
      // Use the actual user-input command; model text never supplies the user's answer.
      await page.evaluate(async (threadId) => {
        const api = await import('/src/api/runtime.ts');
        const detail = await api.readThread(threadId);
        const item = detail.userInputs.find(item => !item.resolution);
        if (!item) throw new Error('pending native question missing');
        await api.resolveUserInput(item.request.id, { action: 'answered', answers: [{ question: item.request.questions[0].question, answer: '做成可点击状态面板' }] });
      }, threadId);
      await expect(page.getByText('生成失败', { exact: true })).toBeVisible({ timeout: 30000 });
      const progress = page.locator('.plan-progress-trigger');
      await expect(progress).toContainText('第 1/5 步');
      const summary = await page.evaluate(async threadId => (await import('/src/api/runtime.ts')).compactThread(threadId), threadId);
      assert(summary.userClarifications.join('\n').includes('做成可点击状态面板'));
      assert(summary.recentAssistantProgress.join('\n').includes('接着补充回归测试'));
      await page.reload();
      await expect(progress).toContainText('第 1/5 步');
      if (mode === 'continue') {
        await page.locator('.composer textarea').fill('继续');
        await page.getByRole('button', { name: '发送消息', exact: true }).click();
      } else {
        await page.locator('.turn-execution > summary').last().click();
        await page.getByRole('button', { name: '重试', exact: true }).click();
      }
      await expect(progress).toContainText('第 5/5 步', { timeout: 30000 });
      await expect.poll(() => Boolean(pending)).toBe(true);
      send(pending, '验证完成，同步最终步骤。', [plan('done')]);
      pending = null;
      await expect(page.getByText('验证交付已完成，原有修改没有重复执行。', { exact: true })).toBeVisible();
      if (fixtureError) throw fixtureError;
      await page.reload();
      const result = await page.evaluate(async threadId => {
        const api = await import('/src/api/runtime.ts');
        return { plan: await api.getPlan(threadId), detail: await api.readThread(threadId) };
      }, threadId);
      assert(result.plan.steps.every(step => step.status === 'completed'));
      assert.equal(result.detail.changes.length, 1);
      assert.equal(result.detail.messages.filter(message => message.role === 'user').length, mode === 'continue' ? 2 : 1);
      assert.equal(fs.readFileSync(path.join(workspace, `${mode}.txt`), 'utf8'), 'existing work survives quota recovery\n');
      await page.screenshot({ path: path.join(output, `${mode}.png`) });
      results.push({ mode, providerRequests: requests.filter(request => request.scenario === mode).length, changes: result.detail.changes.length, planCompleted: true, reloadVerified: true });
    }
    fs.writeFileSync(path.join(output, 'result.json'), JSON.stringify({ result: 'PASS', results }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', results }));
  } finally {
    pending?.destroy();
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('continuation-fixture');
      await api.deleteProvider('continuation-fixture');
    }).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
