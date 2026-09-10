// Run against an isolated `pnpm tauri dev --no-watch` host (Vite 1451, WebView2 9391).
// Exercises real IPC, persistence, lifecycle events and native UI with a loopback model fixture.
const { chromium, expect } = require('@playwright/test');
const http = require('node:http');
const path = require('node:path');

(async () => {
  const server = http.createServer((request, response) => {
    request.resume();
    request.on('end', () => {
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.end('data: {"choices":[{"delta":{"content":"子任务导航验证完成"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n');
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const prefix = `导航验证${Date.now()}`;
  const parentTitle = `${prefix}主会话`;
  const childTitle = `${prefix}子任务`;
  let browser;
  let page;
  try {
    browser = await chromium.connectOverCDP('http://127.0.0.1:9391');
    page = browser.contexts()[0].pages().find(p => p.url().includes(':1451'));
    if (!page) throw new Error('Isolated validation WebView was not found');
    await page.evaluate(() => localStorage.removeItem('kcoder_hidden_project_groups'));
    await page.reload();
    await expect(page.getByText('运行时就绪', { exact: true })).toBeVisible();
    const fixture = await page.evaluate(async ({ baseUrl, parentTitle, childTitle }) => {
      const api = await import('/src/api/runtime.ts');
      const storeUrl = performance.getEntriesByType('resource').find(e => e.name.includes('/src/stores/workbenchStore.ts')).name;
      const { useWorkbenchStore } = await import(storeUrl);
      await api.saveProviderConfig({ id: 'navigation-native-fixture', name: '本地导航验证', kind: 'open_ai_compatible', transport: 'open_ai_chat_completions', baseUrl, model: 'fixture', models: [], endpoints: [], apiKey: 'dummy-navigation-validation', activate: true });
      await useWorkbenchStore.getState().loadProviderCatalog();
      const parent = await api.createThread(true);
      await api.renameThread(parent.id, parentTitle);
      await useWorkbenchStore.getState().reloadThreads();
      await useWorkbenchStore.getState().selectThread(parent.id);
      const child = await api.createSubagent({ parentThreadId: parent.id, task: '请只回复验证完成，不调用工具。', label: childTitle, capabilities: [], timeoutMs: 60000 });
      await api.renameThread(child.threadId, childTitle);
      return { parentId: parent.id, childId: child.threadId, workspacePath: parent.workspacePath };
    }, { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, parentTitle, childTitle });
    const summary = page.getByRole('region', { name: '本会话子智能体状态' });
    await expect(summary).toContainText('已完成', { timeout: 15000 });
    // A real repository read must still serve the child while navigation IPC hides it.
    const listing = await page.evaluate(async ({ childId, prefix }) => {
      const api = await import('/src/api/runtime.ts');
      return { list: await api.listThreads(), search: await api.searchThreads(prefix), child: await api.readThread(childId) };
    }, { ...fixture, prefix });
    for (const threads of [listing.list, listing.search]) {
      if (threads.some(thread => thread.id === fixture.childId)) throw new Error('Child leaked into navigation IPC');
      if (!threads.some(thread => thread.id === fixture.parentId)) throw new Error('Parent missing from navigation IPC');
    }
    if (listing.child.summary.id !== fixture.childId) throw new Error('Child history unavailable');
    // Reload selects the newest parent, even though its child is newer in storage.
    await page.reload();
    await expect(page.getByRole('heading', { name: parentTitle, exact: true })).toBeVisible();
    await page.getByRole('tab', { name: '项目', exact: true }).click();
    const projects = page.getByRole('navigation', { name: '项目列表' });
    const group = projects.locator('.project-group').first();
    await group.getByRole('button', { name: '展开项目', exact: true }).click();
    await expect(projects.locator('.thread-item-main').filter({ hasText: parentTitle })).toHaveCount(1);
    await expect(projects.locator('.thread-item-main').filter({ hasText: childTitle })).toHaveCount(0);
    await page.getByRole('textbox', { name: '搜索会话', exact: true }).fill(prefix);
    await expect(projects.locator('.thread-item-main')).toHaveCount(1);
    await expect(group.locator('.project-group-count')).toHaveText('1');
    // Hidden project groups return parents to the conversation list, never children.
    await page.evaluate(({ workspacePath }) => localStorage.setItem('kcoder_hidden_project_groups', JSON.stringify([workspacePath])), fixture);
    await page.reload();
    const conversations = page.getByRole('navigation', { name: '会话列表' });
    await expect(conversations.locator('.thread-item-main').filter({ hasText: parentTitle })).toHaveCount(1);
    await expect(conversations.locator('.thread-item-main').filter({ hasText: childTitle })).toHaveCount(0);
    await page.getByRole('textbox', { name: '搜索会话', exact: true }).fill(childTitle);
    await expect(conversations).toContainText('没有匹配的会话');
    await page.getByRole('button', { name: '清除搜索', exact: true }).click();
    await summary.getByRole('button').filter({ hasText: childTitle }).click();
    const drawer = page.getByRole('complementary', { name: '子智能体', exact: true });
    await expect(drawer.locator('.subagent-detail-title')).toContainText(childTitle);
    await expect(drawer.getByRole('article').getByText('子任务导航验证完成', { exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: parentTitle, exact: true })).toBeVisible();
    await page.screenshot({ path: path.resolve('docs/subagent-navigation-native.png') });
    console.log(JSON.stringify({ result: 'PASS', checks: ['real list/search IPC', 'child history retained', 'reload selects parent', 'project list/count/search', 'conversation list/search', 'parent opens child details'] }));
  } finally {
    await page?.evaluate(async () => {
      const api = await import('/src/api/runtime.ts');
      await api.deleteProviderApiKey('navigation-native-fixture');
      await api.deleteProvider('navigation-native-fixture');
    }).catch(() => {});
    await browser?.close();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
