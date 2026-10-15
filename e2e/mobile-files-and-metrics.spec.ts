import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
async function fixture(request: import('@playwright/test').APIRequestContext, files = false) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(path.join(absPath, 'docs'), { recursive: true });
  if (files) {
    await writeFile(path.join(absPath, 'docs/notes.md'), '# Notes\n\nReadable on a phone.');
    for (let i = 0; i < 70; i++) await writeFile(path.join(absPath, `file-${String(i).padStart(2, '0')}.txt`), `File ${i}`);
  }
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Mobile layout' } })).json();
  const response = await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, title: 'Mobile files', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const value = await response.json(); return value.id ?? value.thread.id as string;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

test('phone file ask bar sends from the list and viewer, keeps the composer draft, and hides at 640px', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Phone file ask bar');
  await page.setViewportSize({ width: 390, height: 844 });
  const id = await fixture(request, true);
  await page.goto(`/threads/${id}`);
  const draft = 'Keep my main composer draft';
  const composer = page.getByRole('textbox', { name: 'Prompt', exact: true });
  await composer.fill(draft);
  const explorer = page.locator('.matter-topbar').getByRole('button', { name: 'Toggle Explorer', exact: true });
  await explorer.tap();
  const files = page.getByTestId('workspace-panel');
  const tree = files.getByRole('tree', { name: 'Workspace files' });
  const bar = page.locator('form.file-ask-bar');
  const input = bar.getByRole('textbox', { name: 'Ask the agent', exact: true });
  await expect(tree).toBeVisible();
  await expect(input).toBeVisible();
  const prompts: string[] = [];
  page.on('request', r => {
    if (r.method() === 'POST' && r.url().endsWith(`/api/threads/${id}/prompt`)) prompts.push(r.postDataJSON().prompt);
  });
  for (const [index, text] of ['Explain the files list', 'Explain this file\nwith context'].entries()) {
    if (index === 1) {
      await files.getByRole('treeitem', { name: 'docs', exact: true }).getByRole('button', { name: 'docs', exact: true }).tap();
      await files.getByRole('treeitem', { name: 'notes.md', exact: true }).getByRole('button', { name: 'notes.md', exact: true }).tap();
      await expect(files.getByRole('heading', { name: 'Notes', exact: true })).toBeVisible();
      await expect(input).toBeVisible();
    }
    await input.fill(index === 0 ? `  ${text}  ` : 'Explain this file');
    if (index === 1) {
      await input.press('Shift+Enter');
      await input.pressSequentially('with context');
      await expect(input).toHaveValue(text);
      expect(prompts).toHaveLength(1);
    }
    const sent = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith(`/api/threads/${id}/prompt`));
    if (index === 0) await bar.getByRole('button', { name: 'Send', exact: true }).tap();
    else await input.press('Enter');
    expect((await sent).ok()).toBeTruthy();
    await expect(bar.locator('.file-ask-bar-note')).toContainText('Sent');
    await expect(input).toHaveValue('');
    await expect(files).toBeVisible();
    await expect.poll(async () => {
      const detail = await (await request.get(base + `/api/threads/${id}`)).json();
      return detail.turns.filter((turn: { items: Array<{ kind: string; text: string }> }) => turn.items.some(item => item.kind === 'userMessage' && item.text === text)).length;
    }).toBe(1);
    await expect.poll(async () => (await (await request.get(base + `/api/threads/${id}`)).json()).turns.at(-1)?.status).toBe('completed');
  }
  expect(prompts).toEqual(['Explain the files list', 'Explain this file\nwith context']);
  await expect(files.getByRole('heading', { name: 'Notes', exact: true })).toBeVisible();
  await page.setViewportSize({ width: 640, height: 844 });
  await expect(bar).toBeHidden();
  await page.setViewportSize({ width: 639, height: 844 });
  await expect(bar).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await explorer.tap();
  await expect(composer).toBeVisible();
  await expect(composer).toHaveText(draft);
});

test('mobile toolbar opens full-width files; tap, preview, return, upload and action menus preserve navigation', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Mobile navigation regression');
  await page.setViewportSize({ width: 390, height: 844 });
  const id = await fixture(request, true);
  await page.goto(`/threads/${id}`);
  const header = page.locator('.matter-topbar');
  await expect(header.getByRole('button', { name: 'Toggle Explorer', exact: true })).toBeVisible();
  await expect(header.locator('.matter-topbar-end')).toHaveCSS('gap', '0px');
  await header.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  const files = page.getByTestId('workspace-panel');
  const tree = files.getByRole('tree', { name: 'Workspace files' });
  await expect(tree).toBeVisible();
  await expect(files.locator('.thread-graph-viewer')).toHaveCount(0);
  const docs = files.getByRole('treeitem', { name: 'docs', exact: true });
  await docs.getByRole('button', { name: 'docs', exact: true }).tap();
  await expect(docs).toHaveAttribute('aria-expanded', 'true');
  await files.getByRole('treeitem', { name: 'notes.md', exact: true }).getByRole('button', { name: 'notes.md', exact: true }).tap();
  await expect(files.getByRole('heading', { name: 'Notes', exact: true })).toBeVisible();
  await expect(tree).toHaveCount(0);
  await files.getByRole('button', { name: 'Back to files', exact: true }).tap();
  await expect(tree).toBeVisible(); await expect(docs).toHaveAttribute('aria-expanded', 'true');
  // Virtualized directory navigation must preserve the user's viewport.
  await tree.dispatchEvent('touchstart');
  await tree.evaluate(element => { element.scrollTop = 44 * 42; });
  const row = files.getByRole('treeitem', { name: 'file-45.txt', exact: true });
  await row.scrollIntoViewIfNeeded();
  const before = await tree.evaluate(element => element.scrollTop);
  await row.getByRole('button', { name: 'file-45.txt', exact: true }).tap();
  await expect(files.getByRole('region', { name: 'Source code', exact: true })).toContainText('File 45');
  await files.getByRole('button', { name: 'Back to files', exact: true }).tap();
  await expect(tree).toBeVisible();
  await expect.poll(async () => Math.abs((await tree.evaluate(e => e.scrollTop)) - before)).toBeLessThanOrEqual(44);
  await expect(row.getByRole('button', { name: 'Download file-45.txt', exact: true })).toBeHidden();
  await row.getByRole('button', { name: 'More actions for file-45.txt', exact: true }).tap();
  const menu = page.getByRole('menu', { name: 'Actions for file-45.txt', exact: true });
  await expect(menu.getByRole('menuitem', { name: 'Download file' })).toBeVisible();
  const bounds = (await menu.boundingBox())!; expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(390);
  await page.keyboard.press('Escape');
  const uploaded = page.waitForResponse(r => r.request().method() === 'POST' && r.url().includes('/upload'));
  await files.getByTestId('workspace-upload-file-input').setInputFiles({ name: 'upload.txt', mimeType: 'text/plain', buffer: Buffer.from('Uploaded on mobile') });
  expect((await uploaded).ok()).toBeTruthy();
  for (const width of [320, 390]) {
    await page.setViewportSize({ width, height: 844 });
    expect(await header.evaluate(e => e.scrollWidth)).toBeLessThanOrEqual(width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
  }
  await page.screenshot({ path: testInfo.outputPath('mobile-explorer.png') });
  await header.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
});

test('turn metrics keep the live footer compact and the finished average in its receipt in both themes', async ({ page, request }, testInfo) => {
  await page.setViewportSize(testInfo.project.name === 'mobile-chromium' ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
  const id = await fixture(request);
  const detail = await (await request.get(base + `/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 147_000).toISOString();
  const turn = { id: 'metric-turn', status: 'inProgress', startedAt, model: 'gpt-6.1-sol', reasoningEffort: 'high',
    tokenUsage: { total: { totalTokens: 1800000, inputTokens: 1795000, cachedInputTokens: 1700000, outputTokens: 5000 }, generationSpeed: { averageOutputTokensPerSecond: 28.1, latestOutputTokensPerSecond: 48.3 } },
    priceEstimate: { currency: 'USD', totalUsd: 0.61 }, items: [{ id: 'prompt', kind: 'userMessage', text: 'Review this work.' }, { id: 'command', kind: 'commandExecution', text: 'Check layout', status: 'completed' }, { id: 'reply', kind: 'agentMessage', text: 'Checking the implementation.' }] };
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => { socket.send(JSON.stringify({ type: 'supervisor.connected' })); socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' }))); });
  let completed = false;
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: 1, thread: { ...detail.thread, status: completed ? 'idle' : 'running', activeTurnId: completed ? null : turn.id }, turns: [{ ...turn, status: completed ? 'completed' : 'inProgress', completedAt: completed ? new Date().toISOString() : null }] } }));
  for (const theme of ['light', 'dark']) {
    completed = false;
    await page.addInitScript(theme => localStorage.setItem('remote-codex-theme-mode', theme), theme);
    await page.goto(`/threads/${id}`);
    const footer = page.locator('.thread-graph-turn-footer');
    await expect(footer).toBeVisible(); await expect(footer.locator('time')).toHaveCount(0);
    await expect(footer.locator('.thread-graph-turn-footer-meta')).toHaveText(/2m \d+s/);
    await expect(footer.getByTestId('turn-token-speed')).toHaveText('48.3');
    await expect(page.locator('.thread-graph-worked-summary').getByTestId('turn-token-speed')).toHaveCount(0);
    await expect(footer.locator('.thread-token-metric-icon')).toHaveCount(2);
    await expect(footer.locator('.thread-turn-usage-tokens')).toHaveAttribute('aria-label', 'Total tokens: 1,800,000');
    const geometry = await footer.evaluate(e => { const boxes = [...e.querySelectorAll('.thread-turn-usage-tokens,.thread-turn-usage-price,.thread-turn-token-speed,.thread-graph-turn-footer-meta')].map(n => n.getBoundingClientRect()); return { tops: boxes.map(r => r.top), right: Math.max(...boxes.map(r => r.right)), height: e.getBoundingClientRect().height }; });
    expect(Math.max(...geometry.tops) - Math.min(...geometry.tops)).toBeLessThan(6);
    expect(geometry.right).toBeLessThanOrEqual(page.viewportSize()!.width);
    expect(geometry.height).toBeLessThan(35);
    await page.screenshot({ path: testInfo.outputPath(`token-metrics-${theme}.png`) });
    completed = true;
    await page.reload();
    await expect(footer).toHaveCount(0);
    const receipt = page.locator('.thread-turn-receipt');
    await expect(receipt.getByTestId('turn-token-speed')).toHaveText('28.1');
    await expect(receipt.locator('.thread-turn-usage-tokens')).toHaveAttribute('aria-label', 'Total tokens: 1,800,000');
    const bounds = (await receipt.boundingBox())!;
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  }
});

test('collapsed long prompts fade without losing text and expand on focus', async ({ page, request }, testInfo) => {
  await page.setViewportSize(testInfo.project.name === 'mobile-chromium' ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
  const id = await fixture(request);
  const text = '检查输入文字与附件，保持顺序和完整内容。'.repeat(20);
  for (const theme of ['light', 'dark']) {
    await page.addInitScript(theme => localStorage.setItem('remote-codex-theme-mode', theme), theme);
    await page.goto(`/threads/${id}`);
    const input = page.getByRole('textbox', { name: 'Prompt', exact: true });
    const form = input.locator('xpath=ancestor::form');
    await input.fill(text);
    await expect(form).toHaveAttribute('data-composer-layout', 'expanded');
    await input.evaluate(e => (e as HTMLElement).blur());
    await expect(form).toHaveAttribute('data-composer-layout', 'collapsed');
    await expect(form).toHaveAttribute('data-composer-overflow', '');
    await expect.poll(() => input.evaluate(e => getComputedStyle(e).maskImage)).toContain('linear-gradient');
    await expect(input).toHaveText(text);
    await input.focus();
    await expect(form).toHaveAttribute('data-composer-layout', 'expanded');
    await expect(input).toHaveCSS('mask-image', 'none');
    await input.fill('Short'); await input.evaluate(e => (e as HTMLElement).blur());
    await expect(form).not.toHaveAttribute('data-composer-overflow');
    await expect(input).toHaveCSS('mask-image', 'none');
  }
});

test('image links return to their real parent directory, including outside the workspace', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Mobile preview Back navigation');
  await page.setViewportSize({ width: 390, height: 844 });
  const root = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  const absPath = path.join(root, 'project');
  const outside = path.join(root, 'proposals', 'naming');
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC', 'base64');
  await mkdir(path.join(absPath, 'docs', 'assets'), { recursive: true });
  await mkdir(outside, { recursive: true });
  await writeFile(path.join(absPath, 'docs', 'assets', 'inside.png'), png);
  await writeFile(path.join(outside, 'icon-felt-cream.png'), png);
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Image links' } })).json();
  const started = await (await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, title: 'Image links', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const detail = await (await request.get(base + `/api/threads/${id}`)).json();
  const reply = `The icon is \`${outside}/icon-felt-cream.png\`; the project copy is \`docs/assets/inside.png\`.`;
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: 1, turns: [{
    id: 'image-turn', status: 'completed', startedAt: new Date().toISOString(), completedAt: new Date().toISOString(),
    items: [{ id: 'image-prompt', kind: 'userMessage', text: 'Where is the icon?' }, { id: 'image-reply', kind: 'agentMessage', text: reply }],
  }] } }));
  await page.goto(`/threads/${id}`);
  const files = page.getByTestId('workspace-panel');
  const tree = files.getByRole('tree', { name: 'Workspace files' });
  await page.getByRole('link', { name: `${outside}/icon-felt-cream.png`, exact: true }).tap();
  await expect(files.locator('img').first()).toBeVisible();
  await files.getByRole('button', { name: 'Back to files', exact: true }).tap();
  // The host directory that really contains the file, never a virtual folder.
  const parent = tree.locator(`[data-explorer-path="${outside}"]`);
  await expect(parent).toHaveAttribute('aria-expanded', 'true');
  await expect(parent.locator('.workspace-linked-directory-label')).toHaveAttribute('title', new RegExp(`^${outside}\\n`));
  const icon = tree.locator(`[data-explorer-path="${outside}/icon-felt-cream.png"]`);
  await expect(icon).toHaveAttribute('aria-selected', 'true');
  await expect(icon).toBeInViewport();
  await expect(tree.getByText('Linked files', { exact: true })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('linked-image-parent.png') });
  await page.goBack();
  await page.getByRole('link', { name: 'docs/assets/inside.png', exact: true }).tap();
  await expect(files.locator('img').first()).toBeVisible();
  await files.getByRole('button', { name: 'Back to files', exact: true }).tap();
  await expect(tree.locator('[data-explorer-path="docs/assets"]')).toHaveAttribute('aria-expanded', 'true');
  await expect(tree.locator('[data-explorer-path="docs/assets/inside.png"]')).toHaveAttribute('aria-selected', 'true');
});
