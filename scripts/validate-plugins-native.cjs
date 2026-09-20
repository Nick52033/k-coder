// Requires isolated pnpm tauri dev: Vite 1482 / WebView2 CDP 9422.
const { chromium, expect } = require('@playwright/test');
const { createServer } = require('node:http');
const fs = require('node:fs');

(async () => {
  const skills = [
    ['browser', 'control-browser'], ['documents', 'documents'],
    ['presentations', 'presentations'], ['record-replay', 'record-replay'],
    ['sites', 'sites-building'], ['spreadsheets', 'spreadsheets'],
    ['superpowers', 'using-superpowers'],
  ];
  const calls = skills.map(([name, skillName]) => ({
    name: 'plugin_skill_read', arguments: { pluginId: `${name}@local`, skillName },
  }));
  calls.push({ name: 'update_plan', arguments: { steps: [{ step: '插件适配验收', status: 'completed' }] } });
  calls.push({ name: 'run_command', arguments: { command: 'powershell -NoProfile -File scripts/plugin-artifact-runtime.ps1 -Action info', cwd: '.', timeoutMs: 30000 } });
  const requests = [];
  const frame = value => `data: ${JSON.stringify(value)}\n\n`;
  const server = createServer((req, res) => {
    if (req.url === '/fixture') {
      res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      res.end(`<title>插件浏览器验收</title><label for="input">内容</label><input id="input"><button id="save" onclick="document.querySelector('#result').textContent=document.querySelector('#input').value">保存</button><p id="result"></p>`);
      return;
    }
    if (req.method !== 'POST') { res.writeHead(404); res.end(); return; }
    let body = '';
    req.on('data', value => { body += value; });
    req.on('end', () => {
      const request = JSON.parse(body);
      requests.push(request);
      const next = calls[requests.length - 1];
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      if (next) {
        res.end(frame({ choices: [{ delta: { tool_calls: [{ index: 0, id: `plugin-check-${requests.length}`, type: 'function', function: { name: next.name, arguments: JSON.stringify(next.arguments) } }] }, finish_reason: 'tool_calls' }] }) + 'data: [DONE]\n\n');
      } else {
        res.end(frame({ choices: [{ delta: { content: '本地插件工具链验收完成。' }, finish_reason: 'stop' }] }) + 'data: [DONE]\n\n');
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = `http://127.0.0.1:${server.address().port}`;
  calls.push(
    { name: 'browser_navigate', arguments: { url: `${address}/fixture` } },
    { name: 'browser_snapshot', arguments: {} },
    { name: 'browser_type', arguments: { selector: '#input', text: 'plugin-check-42' } },
    { name: 'browser_click', arguments: { selector: '#save' } },
    { name: 'browser_snapshot', arguments: {} },
    { name: 'browser_screenshot', arguments: { fullPage: false } },
    { name: 'browser_close', arguments: {} },
  );
  let browser;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9422');
    const page = browser.contexts()[0].pages().find(p => p.url().includes(':1482'));
    if (!page) throw new Error('Isolated plugin validation WebView missing');
    const overview = await page.evaluate(async ({ ids, baseUrl }) => {
      const api = await import('/src/api/runtime.ts');
      const { invoke } = await import('/node_modules/@tauri-apps/api/core.js');
      await invoke('switch_workspace', { path: 'D:/code/k-coder', trusted: true });
      for (const id of ids) await api.setPluginEnabled(id, true);
      await api.saveBrowserSettings({ enabled: true, allowLocalhost: true });
      await api.setApprovalMode('full_access');
      await api.saveProviderConfig({ id: 'plugins-local-fixture', name: '插件本地验收', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [{ id: 'fixture', displayName: 'Fixture', contextWindow: 128000, fallback: false }], endpoints: [], apiKey: 'dummy-plugin-validation', activate: true });
      return api.getPluginOverview(true);
    }, { ids: skills.map(([id]) => `${id}@local`), baseUrl: `${address}/v1` });
    expect(overview.plugins).toHaveLength(7);
    for (const plugin of overview.plugins) {
      expect(plugin.enabled).toBe(true);
      expect(plugin.state).toBe(plugin.name === 'sites' ? 'degraded' : 'loaded');
    }
    const history = await page.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      const thread = await api.createThread(true);
      await api.runTurn(thread.id, '逐一验证 @browser @documents @presentations @record-replay @sites @spreadsheets @superpowers，使用本地测试页面。', [], 'craft');
      return api.readThreadHistory(thread.id);
    });
    expect(history.lastTurn.state).toBe('completed');
    expect(requests).toHaveLength(calls.length + 1);
    const toolMessages = requests.at(-1).messages.filter(m => m.role === 'tool');
    expect(toolMessages).toHaveLength(calls.length);
    for (let i = 0; i < skills.length; i++) {
      const output = toolMessages[i].content;
      expect(output).toContain(`name: ${skills[i][1]}`);
      expect(output.length).toBeGreaterThan(100);
    }
    const snapshot = toolMessages[calls.findLastIndex(call => call.name === 'browser_snapshot')].content;
    expect(snapshot).toContain('plugin-check-42');
    const screenshot = JSON.parse(toolMessages[calls.findIndex(call => call.name === 'browser_screenshot')].content);
    expect(screenshot.mediaType).toBe('image/png');
    expect(screenshot.sizeBytes).toBeGreaterThan(0);
    expect(JSON.parse(toolMessages.at(-1).content).closed).toBe(true);
    const runtimeInfo = toolMessages[calls.findIndex(call => call.name === 'run_command')].content;
    expect(runtimeInfo).toContain('artifactToolVersion');
    const registered = requests[0].tools.map(t => t.function.name);
    for (const call of calls) expect(registered).toContain(call.name);
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const refreshed = await page.evaluate(async () => (await import('/src/api/runtime.ts')).getPluginOverview(true));
    expect(refreshed.plugins.every(p => p.enabled)).toBe(true);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await page.getByRole('button', { name: '插件管理', exact: true }).click();
    await expect(page.getByRole('checkbox', { checked: true })).toHaveCount(7);
    await page.screenshot({ path: 'docs/sys/插件适配原生验证.png' });
    fs.writeFileSync('docs/sys/插件适配原生结果.json', JSON.stringify({ overview: refreshed, toolCalls: calls.map(c => c.name), turnState: history.lastTurn.state, providerRequests: requests.length }, null, 2));
    console.log('PASS: seven plugins enabled, seven Skills read through AgentRuntime, plan update, native browser interaction/screenshot, refresh persistence.');
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
