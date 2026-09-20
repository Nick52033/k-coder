// Requires isolated pnpm tauri dev: Vite 1501, WebView2 CDP 9501.
const { chromium, expect } = require('@playwright/test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

(async () => {
  const outputDir = path.resolve('.tmp-ui/command-diagnostics');
  const workspace = path.join(outputDir, 'workspace');
  fs.mkdirSync(workspace, { recursive: true });
  fs.writeFileSync(path.join(workspace, 'sample.cs'), 'class NativeMarker {}\n');
  const fixtures = [
    ['search-without-path', 'rg -n NativeMarker --glob \'*.cs\' -l'],
    ['no-matches', 'rg AbsentMarker .'],
    ['bounded-search', 'rg AbsentMarker . | Select-Object -First 10'],
    ['partial-output', "Write-Output 'sample.cs ParserError'; rg AbsentMarker ."],
    ['missing-file', 'rg x missing.cs'],
    ['parser', 'rg "unclosed ./sample*.cs'],
    ['glob', 'rg NativeMarker ./sample*.cs'],
    ['missing-count', 'rg -n NativeMarker sample.cs | Select-Object -First'],
    ['combined-search', 'rg -n NativeMarker sample.cs | Select-Object -First 20; rg -n AbsentMarker sample.cs | Select-Object -First 40'],
    ['timeout', 'Start-Sleep -Seconds 2', 100],
  ];
  const calls = fixtures.map(([id, command, timeoutMs]) => ({ id, name: 'run_command', arguments: { command, timeoutMs: timeoutMs ?? 10000 } }));
  calls.push({ id: 'traversal', name: 'read_file', arguments: { path: '../outside.txt' } });
  const corrections = [
    ['corrected-count', 'rg -n NativeMarker sample.cs | Select-Object -First 20'],
    ['corrected-glob', "rg -n NativeMarker . --glob '*.cs' | Select-Object -First 10"],
    ['separate-search-match', 'rg -n NativeMarker sample.cs | Select-Object -First 20'],
    ['separate-search-absent', 'rg -n AbsentMarker sample.cs | Select-Object -First 40'],
  ].map(([id, command]) => ({ id, name: 'run_command', arguments: { command, timeoutMs: 10000 } }));
  const requests = [];
  const providerId = 'command-diagnostics-native-fixture';
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', chunk => chunks.push(chunk));
    req.on('end', () => {
      requests.push(JSON.parse(Buffer.concat(chunks).toString()));
      const pending = requests.length === 1 ? calls : requests.length === 2 ? corrections : [];
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.end(`data: ${JSON.stringify({ choices: [{ delta: pending.length ? {
        tool_calls: pending.map((call, index) => ({ index, id: call.id, type: 'function', function: {
          name: call.name, arguments: JSON.stringify(call.arguments),
        } })),
      } : { content: '命令诊断原生验证完成。' }, finish_reason: pending.length ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9501');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1501'));
    assert(page, 'isolated validation WebView required');
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    await page.evaluate(async ({ workspace, providerId, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      await api.switchWorkspace(workspace, true);
      await api.saveProviderConfig({ id: providerId, name: '本地命令诊断验收', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-local-command-diagnostics', activate: true });
      await api.setApprovalMode('full_access');
    }, { workspace, providerId, baseUrl: `http://127.0.0.1:${server.address().port}/v1` });
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const threadId = await page.evaluate(async workspace => {
      const api = await import('/src/api/runtime.ts');
      const url = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(url);
      useWorkbenchStore.setState({ activeThreadId: null });
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      await api.switchWorkspace(workspace, true);
      const thread = await api.createThread(true);
      const assertWorkspace = thread.workspacePath?.replaceAll('\\', '/').endsWith('/.tmp-ui/command-diagnostics/workspace');
      if (!assertWorkspace) throw new Error('Wrong validation workspace');
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(thread.id);
      await api.startTurn(thread.id, '验证搜索、命令错误与工作区边界。', [], 'craft');
      return thread.id;
    }, workspace);
    const read = () => page.evaluate(async id => (await import('/src/api/runtime.ts')).readThread(id), threadId);
    await expect.poll(async () => (await read()).lastTurn?.state, { timeout: 90000 }).toBe('completed');
    const detail = await read();
    const activities = detail.turnTimeline.filter(item => item.type === 'tool').map(item => item.activity);
    assert.equal(activities.length, calls.length + corrections.length);
    const get = id => activities.find(activity => activity.call.id === id);
    assert.equal(get('search-without-path').result.success, true);
    assert.match(get('search-without-path').result.output, /sample.cs/);
    for (const id of ['no-matches', 'bounded-search', 'separate-search-absent']) {
      assert.equal(get(id).result.success, false);
      assert.equal(get(id).result.metadata.exitCode, 1);
      assert.equal(get(id).result.metadata.resultKind, 'no_matches');
    }
    for (const id of ['partial-output', 'missing-file', 'parser', 'timeout', 'traversal']) {
      assert.equal(get(id).result.success, false);
      assert.equal(get(id).result.metadata.resultKind, undefined);
    }
    const parserHasDiagnostic = typeof get('parser').result.metadata.recoveryHint === 'string';
    if (parserHasDiagnostic) {
      assert.match(get('parser').result.metadata.recoveryHint, /语法错误/);
    } else {
      // Some pwsh versions return exit 1 without emitting a parser diagnostic.
      assert.equal(get('parser').result.metadata.exitCode, 1);
      assert.deepEqual(get('parser').result.metadata.outputChunks, []);
      assert.match(get('parser').result.output, /exited with code 1/);
    }
    assert.match(get('glob').result.metadata.recoveryHint, /--glob/);
    for (const id of ['glob', 'missing-count', 'combined-search']) {
      const { result } = get(id);
      assert.equal(result.success, false);
      assert.equal(result.metadata.resultKind, 'invalid_command');
      assert.equal(result.metadata.executed, false);
      assert.equal(result.metadata.sessionId, undefined);
      assert.equal(result.metadata.exitCode, undefined);
    }
    for (const id of ['corrected-count', 'corrected-glob', 'separate-search-match']) {
      assert.equal(get(id).result.success, true);
      assert.match(get(id).result.output, /NativeMarker/);
    }
    assert.equal(get('timeout').result.metadata.state.state, 'timed_out');
    assert.match(get('traversal').result.output, /parent traversal/);
    const verifyDisplay = async () => {
      await expect(page.locator('.turn-timeline-tool--no-matches')).toHaveCount(3);
      const meta = page.locator('.turn-command-summary .turn-tool-meta');
      await expect(meta.filter({ hasText: /^未匹配$/ })).toHaveCount(3);
      await expect(meta.filter({ hasText: /^未执行：/ })).toHaveCount(3);
      await expect(meta.filter({ hasText: '已有部分输出' })).toHaveCount(1);
      await expect(meta.filter({ hasText: parserHasDiagnostic ? '字符串引号未闭合' : '退出码 1（无错误详情）' })).toHaveCount(1);
      await expect(meta.filter({ hasText: '运行超时' })).toHaveCount(1);
    };
    await verifyDisplay();
    await page.reload();
    await verifyDisplay();
    await page.locator('.turn-execution > summary').click();
    for (const summary of await page.locator('.turn-tool-group > summary').all()) await summary.click();
    await page.screenshot({ path: path.join(outputDir, 'native-result.png'), fullPage: true, animations: 'disabled' });
    assert.equal(requests.length, 3);
    const toolResponses = requests[1].messages.filter(message => message.role === 'tool');
    assert.equal(toolResponses.length, calls.length);
    assert(toolResponses.some(message => String(message.content).includes('no matches')));
    for (const id of ['glob', 'missing-count', 'combined-search']) {
      const response = toolResponses.find(message => message.tool_call_id === id);
      assert(response, `repair guidance for ${id} must reach the next provider request`);
      assert.match(String(response.content), /命令未执行/);
    }
    fs.writeFileSync(path.join(outputDir, 'native-result.json'), JSON.stringify({ result: 'PASS', threadId, providerRequests: requests.length, operations: activities.length, results: activities.map(({ call, result }) => ({ id: call.id, success: result.success, exitCode: result.metadata.exitCode, resultKind: result.metadata.resultKind, executed: result.metadata.executed, durationMs: result.metadata.durationMs })) }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', threadId, operations: activities.length, providerRequests: requests.length }));
  } finally {
    await page?.evaluate(async id => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey(id);
      await api.deleteProvider(id);
    }, providerId).catch(() => {});
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
