import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem(
    `pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`,
    JSON.stringify({ welcomeDismissed: true, completed: [] }),
  ));
});

test('recent thread menus manage shortcuts and history while glass controls and outer pages share the theme', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `recent-menus-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Glass workspace' } })).json();
  const ids: string[] = [];
  for (const title of ['Keep this conversation', 'Manage from Recent chats']) {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const value = await response.json(); ids.push(value.id ?? value.thread.id);
    await page.goto(`/threads/${ids.at(-1)}`);
    await expect(page.locator('.matter-current-title')).toHaveText(title);
  }
  await page.goto(`/threads/${ids[0]}`);
  if (testInfo.project.name === 'mobile-chromium') await page.getByRole('button', { name: 'Toggle shortcuts sidebar' }).click();
  const recent = page.getByTestId('recent-chats');
  await recent.getByRole('button', { name: 'Actions for Manage from Recent chats' }).click();
  const menu = page.getByRole('dialog', { name: 'Thread actions: Manage from Recent chats', exact: true });
  await menu.getByRole('button', { name: 'Star thread', exact: true }).click();
  await expect(page.getByTestId('shortcuts').getByRole('link', { name: /Manage from Recent chats/ })).toBeVisible();
  await expect(page.locator('.matter-current-title')).toHaveText('Keep this conversation');
  await recent.getByRole('button', { name: 'Actions for Manage from Recent chats' }).click();
  await menu.getByRole('button', { name: 'Rename thread', exact: true }).click();
  const rename = page.getByRole('dialog', { name: 'Rename thread', exact: true });
  await rename.getByRole('textbox', { name: 'Thread title' }).fill('Renamed in sidebar');
  await rename.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(recent.getByRole('button', { name: 'Actions for Renamed in sidebar' })).toBeVisible();
  await page.reload();
  if (testInfo.project.name === 'mobile-chromium') await page.getByRole('button', { name: 'Toggle shortcuts sidebar' }).click();
  await recent.getByRole('button', { name: 'Actions for Renamed in sidebar' }).click();
  await page.getByRole('dialog', { name: 'Thread actions: Renamed in sidebar', exact: true }).getByRole('button', { name: 'Delete thread' }).click();
  await page.getByRole('dialog', { name: 'Delete thread?' }).getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(recent.getByRole('link', { name: /Renamed in sidebar/ })).toHaveCount(0);
  await expect.poll(async () => (await request.get(`${base}/api/threads/${ids[1]}`)).status()).toBe(404);
  if (testInfo.project.name === 'mobile-chromium') await page.getByRole('button', { name: 'Close sidebar', exact: true }).click();
  await page.locator('.matter-thread-menu > summary').click();
  await page.getByRole('button', { name: 'Star thread', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Remove shortcut', exact: true })).toBeVisible();
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => localStorage.setItem('remote-codex-theme-mode', theme), theme);
    await page.reload();
    await expect(page.locator('.thread-graph-composer-shell')).toBeVisible();
    await expect(page.locator('.thread-graph-composer-shell')).toHaveCSS('backdrop-filter', 'blur(24px) saturate(1.35)');
    await expect(page.getByRole('group', { name: 'Timeline navigation' })).toHaveCSS('border-top-width', '0px');
    await page.screenshot({ path: `output/playwright/glass-${theme}-${testInfo.project.name}.png` });
    const conversationBackground = await page.locator('.matter-chat').evaluate(e => getComputedStyle(e).getPropertyValue('--theme-bg').trim());
    await page.goto('/workspaces');
    await expect(page.getByRole('heading', { name: 'Workspaces', exact: true }).first()).toBeVisible();
    expect(await page.locator('body').evaluate(e => getComputedStyle(e).getPropertyValue('--theme-bg').trim())).toBe(conversationBackground);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/outer-${theme}-${testInfo.project.name}.png` });
    await page.goto(`/threads/${ids[0]}`);
  }
});

test('reading area uses most of a wide viewport and fills the space beside Explorer', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `wide-reading-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Wide conversation' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'A wider conversation', provider: 'acp', agentId: 'grok', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt: 'hello' } });
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${id}`)).json()).thread.status).toBe('idle');
  await page.setViewportSize({ width: 2560, height: 1200 });
  await page.goto(`/threads/${id}`);
  await expect(page.locator('[data-role="assistant"]').getByText('hello', { exact: true })).toBeVisible();
  const measure = () => page.evaluate(() => {
    const chat = document.querySelector('.matter-chat')!.getBoundingClientRect();
    const reply = document.querySelector('.thread-graph-message-stack.is-assistant')!.getBoundingClientRect();
    const composer = document.querySelector('.thread-graph-composer-shell')!.getBoundingClientRect();
    return { reply: reply.width / chat.width, composer: composer.width / chat.width, leftDifference: Math.abs(reply.left - composer.left), rightDifference: Math.abs(reply.right - composer.right), overflow: document.documentElement.scrollWidth > innerWidth };
  });
  await expect.poll(async () => (await measure()).reply).toBeGreaterThanOrEqual(.70);
  expect((await measure()).composer).toBeGreaterThanOrEqual(.70);
  expect((await measure()).reply).toBeLessThan(.90);
  expect((await measure()).leftDifference).toBeLessThan(4);
  expect((await measure()).rightDifference).toBeLessThan(4);
  await page.screenshot({ path: `output/playwright/wide-reading-${testInfo.project.name}.png` });
  await page.getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  await expect(page.getByRole('complementary', { name: 'Explorer', exact: true })).toBeVisible();
  await expect.poll(async () => (await measure()).reply).toBeGreaterThan(.94);
  expect((await measure()).composer).toBeGreaterThan(.94);
  await page.screenshot({ path: `output/playwright/wide-reading-explorer-${testInfo.project.name}.png` });
  await page.getByRole('button', { name: 'Hide Explorer', exact: true }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect.poll(async () => (await measure()).reply).toBeGreaterThan(.90);
  expect((await measure()).overflow).toBe(false);
});

test('search reveals an older collapsed message, Explorer resizes and connection details stay anchored', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `interactions-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Interaction review' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Interaction review', provider: 'acp', agentId: 'grok', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  let running = false;
  const turns = Array.from({ length: 6 }, (_, index) => ({
    id: `search-turn-${index}`, status: 'completed', startedAt: new Date(Date.now() - (7 - index) * 60_000).toISOString(), completedAt: new Date(Date.now() - (7 - index) * 60_000 + 20_000).toISOString(),
    items: [
      { id: `prompt-${index}`, kind: 'userMessage', text: `Review step ${index}` },
      { id: `progress-${index}`, kind: 'agentMessage', text: index === 0 ? 'The hidden cobalt decision belongs in this earlier message.' : `Progress for step ${index}` },
      { id: `command-${index}`, kind: 'commandExecution', text: 'Inspect files', command: 'pwd', status: 'completed' },
      { id: `reply-${index}`, kind: 'agentMessage', text: `Final response ${index}.\n\n` + 'A readable paragraph to create enough scrolling space.\n\n'.repeat(8) },
    ],
  }));
  await page.route(`**/api/threads/${id}?**`, route => {
    const limit = Number(new URL(route.request().url()).searchParams.get('limit') ?? 3);
    return route.fulfill({ json: { ...detail, thread: { ...detail.thread, ...(running ? { status: 'running', activeTurnId: 'search-turn-5' } : {}) }, totalTurnCount: turns.length, turns: turns.slice(-limit).map(turn => ({ ...turn, ...(running && turn.id === 'search-turn-5' ? { status: 'inProgress', completedAt: null } : {}), hasDeferredItems: true, deferredItemCount: 2, items: [turn.items[0], turn.items[3]] })) } });
  });
  await page.route(`**/api/threads/${id}/turns/*/detail*`, route => route.fulfill({ json: turns.find(turn => route.request().url().includes(`/${turn.id}/`)) }));
  await page.route(`**/api/threads/${id}/search?**`, route => route.fulfill({ json: { matches: [{ turnId: turns[0].id, itemId: 'progress-0', role: 'Assistant', text: turns[0].items[1].text }], hasMore: false } }));
  await page.addInitScript(() => localStorage.setItem('remote-codex-theme-mode', 'light'));
  await page.goto(`/threads/${id}`);
  await expect(page.locator('[data-turn-id="search-turn-0"]')).toHaveCount(0);
  await page.getByRole('button', { name: 'Search conversation', exact: true }).click();
  await page.getByRole('combobox', { name: 'Search messages' }).fill('hidden cobalt');
  const result = page.locator('.workbench-search-results > button').filter({ hasText: 'hidden cobalt' });
  await expect(result).toHaveCount(1);
  await result.click();
  const target = page.locator('[data-message-id="progress-0"]');
  await expect(target).toBeVisible();
  await expect.poll(async () => target.evaluate(element => {
    const viewport = document.querySelector('.thread-graph-scroll-container')!.getBoundingClientRect();
    const box = element.getBoundingClientRect();
    return box.top >= viewport.top && box.bottom <= viewport.bottom;
  })).toBe(true);
  await page.getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const explorer = page.getByRole('region', { name: 'Workspace files', exact: true });
  const separator = page.getByRole('separator', { name: 'Resize Explorer' });
  const width = (await explorer.boundingBox())!.width;
  const handle = (await separator.boundingBox())!;
  await page.mouse.move(handle.x + handle.width / 2, handle.y + 80);
  await page.mouse.down();
  await page.mouse.move(handle.x - 100, handle.y + 80, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => (await explorer.boundingBox())!.width).toBeGreaterThan(width + 80);
  await separator.focus();
  await page.keyboard.press('ArrowRight');
  await page.getByRole('button', { name: 'Hide Explorer', exact: true }).click();
  // Healthy connections hide their indicator; offline exposes its details.
  await page.context().setOffline(true);
  const connection = page.locator('.matter-connection .device-connection-button');
  await expect(connection).toBeVisible();
  const bell = page.getByRole('button', { name: 'Notifications', exact: true });
  const connectionBox = (await connection.boundingBox())!;
  const bellBox = (await bell.boundingBox())!;
  expect(Math.abs(connectionBox.y + connectionBox.height / 2 - bellBox.y - bellBox.height / 2)).toBeLessThanOrEqual(1);
  await connection.click();
  const popover = page.locator('.device-connection-popover');
  await expect(popover).toBeVisible();
  const popoverBox = (await popover.boundingBox())!;
  expect(popoverBox.y - connectionBox.y - connectionBox.height).toBeCloseTo(8, 0);
  expect(popoverBox.x + popoverBox.width).toBeLessThanOrEqual(connectionBox.x + connectionBox.width + 1);
  await page.keyboard.press('Escape');
  await expect(popover).toHaveCount(0);
  await page.screenshot({ path: `output/playwright/workbench-interactions-${testInfo.project.name}.png` });
  await page.context().setOffline(false);
  running = true;
  await page.reload();
  const stop = page.locator('.thread-graph-composer-stop-button');
  const send = page.locator('.thread-graph-composer-send-button');
  await expect(stop).toBeVisible();
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await expect.poll(async () => {
      const a = (await stop.boundingBox())!, b = (await send.boundingBox())!;
      return Math.abs(a.x + a.width / 2 - b.x - b.width / 2);
    }).toBeLessThan(1);
    await expect(stop).toHaveCSS('box-shadow', 'none');
    await expect(stop).toHaveCSS('width', '32px');
    await expect(send).toHaveCSS('width', '32px');
    await page.screenshot({ path: `output/playwright/workbench-running-${width}.png` });
  }
});

test('workbench keeps tab positions, fills the viewport and separates session identifiers in themed settings', async ({ page, request, context }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `workbench-tabs-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Workbench review' } })).json();
  const ids: string[] = [];
  for (const title of ['First review', 'Second review']) {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const thread = await response.json();
    ids.push(thread.id ?? thread.thread.id);
  }
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(`/threads/${ids[0]}`);
  await expect(page.locator('.matter-thread-tabs a')).toHaveCount(1);
  await page.goto(`/threads/${ids[1]}`);
  await expect(page.locator('.matter-thread-tabs a')).toHaveCount(2);
  const expected = ids.map(id => `/threads/${id}`);
  const tabOrder = () => page.locator('.matter-thread-tabs a').evaluateAll(links => links.map(link => link.getAttribute('href')));
  expect(await tabOrder()).toEqual(expected);
  await page.locator(`.matter-thread-tabs a[href="${expected[0]}"]`).click();
  await expect(page.locator('.matter-current-title')).toHaveText('First review');
  expect(await tabOrder()).toEqual(expected);
  await page.evaluate(() => localStorage.setItem('remote-codex-theme-mode', 'dark'));
  await page.reload();
  await expect(page.locator('.matter-thread-tabs a')).toHaveCount(2);
  expect(await tabOrder()).toEqual(expected);
  // A parent's old bottom padding clipped the workbench even when its own bounds were correct.
  expect(await page.evaluate(() => document.elementsFromPoint(innerWidth / 2, innerHeight - 1).some(e => e.classList.contains('matter-workbench')))).toBe(true);
  await page.locator('.matter-thread-menu > summary').click();
  await page.getByRole('button', { name: 'Copy Pockymoe session ID', exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(ids[0]);
  const detail = await (await request.get(`${base}/api/threads/${ids[0]}`)).json();
  expect(detail.thread.providerSessionId).toBeTruthy();
  await page.getByRole('button', { name: 'Copy harness session ID', exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(detail.thread.providerSessionId);
  await page.getByRole('button', { name: 'Copy Codex deeplink', exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`codex://threads/${detail.thread.providerSessionId}`);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  const settings = page.getByTestId('settings-dialog');
  await expect(settings).toHaveClass(/matter-settings-dialog/);
  await expect(settings).toHaveAttribute('data-theme-effective', 'dark');
  await expect(settings.getByText('Pockymoe session ID', { exact: true })).toBeVisible();
  await expect(settings.getByText('Harness session ID', { exact: true })).toBeVisible();
  await page.screenshot({ path: `output/playwright/matter-settings-dark-${testInfo.project.name}.png` });
  await settings.getByRole('button', { name: 'Global', exact: true }).click();
  await expect(settings.locator('.thread-graph-settings-global-content')).toBeVisible();
  await settings.getByTestId('theme-mode-light').click();
  await expect(settings).toHaveAttribute('data-theme-effective', 'light');
  await expect(settings.getByTestId('theme-mode-light')).toHaveAttribute('aria-pressed', 'true');
  await expect(settings).toHaveCSS('background-color', 'oklch(0.946 0.006 145)');
  await page.screenshot({ path: `output/playwright/matter-settings-global-${testInfo.project.name}.png` });
  await page.keyboard.press('Escape');
  await page.evaluate(() => localStorage.setItem('remote-codex-theme-mode', 'dark'));
  await page.reload();
  await expect(page.locator('.matter-thread-tabs a')).toHaveCount(2);
  await page.screenshot({ path: `output/playwright/matter-workbench-final-${testInfo.project.name}.png` });
});

test('Matter workbench floats the composer, persists shortcuts, searches history and gates Terminal', async ({
  page,
  request,
}, testInfo) => {
  const absPath = path.resolve(
    process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright',
    `matter-${randomUUID()}`,
  );
  await mkdir(absPath, { recursive: true });
  const wsResponse = await request.post(`${base}/api/workspaces`, {
    data: { absPath, label: 'Design workspace' },
  });
  expect(wsResponse.ok()).toBeTruthy();
  const ws = await wsResponse.json();
  const createdResponse = await request.post(`${base}/api/threads/start`, {
    data: {
      workspaceId: ws.id,
      title: 'Refine the conversation experience',
      provider: 'acp',
      agentId: 'grok',
      model: 'ios-e2e-stream',
      approvalMode: 'yolo',
    },
  });
  expect(createdResponse.ok()).toBeTruthy();
  const created = await createdResponse.json();
  const id = created.id ?? created.thread.id;
  await page.goto(`/threads/${id}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await page
    .getByRole('textbox', { name: 'Prompt' })
    .fill('hello, bring some clarity to this workspace');
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect(page.getByText('hello', { exact: true })).toBeVisible();
  const metrics = await page.evaluate(() => {
    const composer = document
      .querySelector('[data-testid="chat-composer"]')!
      .getBoundingClientRect();
    const transcript = document
      .querySelector('.thread-graph-scroll-container')!
      .getBoundingClientRect();
    return {
      composer: {
        x: composer.x,
        y: composer.y,
        right: composer.right,
        bottom: composer.bottom,
      },
      transcript: { bottom: transcript.bottom },
      mainRight: document.querySelector('.matter-main')!.getBoundingClientRect()
        .right,
      width: innerWidth,
      documentWidth: document.documentElement.scrollWidth,
    };
  });
  expect(metrics.transcript.bottom).toBeGreaterThan(metrics.composer.y + 20);
  expect(metrics.composer.x).toBeGreaterThanOrEqual(0);
  expect(metrics.composer.right).toBeLessThanOrEqual(metrics.width);
  expect(metrics.documentWidth).toBe(metrics.width);
  expect(metrics.mainRight).toBe(metrics.width);
  await page.getByTestId('recent-chats').getByRole('button', { name: 'Actions for A calmer coding workspace', exact: true }).click();
  await page.locator('.recent-thread-menu').getByRole('button', { name: 'Star thread', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Remove shortcut' }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole('button', { name: 'Remove shortcut' }),
  ).toBeVisible();
  if (testInfo.project.name === 'mobile-chromium')
    await page
      .getByRole('button', { name: 'Toggle shortcuts sidebar' })
      .click();
  await expect(
    page
      .getByTestId('shortcuts')
      .getByRole('link', { name: /Refine the conversation/ }),
  ).toBeVisible();
  if (testInfo.project.name === 'mobile-chromium')
    await page
      .getByRole('button', { name: 'Close sidebar', exact: true })
      .click();
  await page
    .getByRole('button', { name: 'Search conversation', exact: true })
    .click();
  await page.getByRole('combobox', { name: 'Search messages' }).fill('clarity');
  await expect(page.getByText('1 matching messages')).toBeVisible();
  await expect(page.locator('.workbench-search-results mark')).toHaveText(
    'clarity',
  );
  await page.keyboard.press('Escape');
  await expect(
    page.getByRole('combobox', { name: 'Search messages' }),
  ).not.toBeVisible();
  await page
    .getByRole('button', { name: 'Notifications', exact: true })
    .click();
  await expect(
    page.locator(`.matter-notifications a[href="/threads/${id}"]`),
  ).toContainText('Refine the conversation experience completed');
  await page.getByRole('button', { name: 'Close notification panel' }).click();
  await page.getByRole('button', { name: 'Toggle Explorer' }).click();
  await expect(
    page.getByRole('complementary', { name: 'Explorer', exact: true }),
  ).toBeVisible();
  await page
    .getByRole('button', { name: 'Close Explorer', exact: true })
    .click();
  await page.getByRole('button', { name: 'Download transcript' }).click();
  await expect(
    page.getByRole('dialog', { name: 'Thread actions', exact: true }),
  ).toBeVisible();
  await expect(page.locator('.thread-export-dialog-title')).toHaveText('Download HTML');
  await expect(page.locator('.matter-actions-dialog')).toBeVisible();
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export HTML', exact: true }).click();
  expect((await download).suggestedFilename()).toMatch(/\.html$/);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Share as link', exact: true }).click();
  await expect(page.locator('.thread-export-dialog-title')).toHaveText('Share read-only link');
  await expect(page.getByRole('status').filter({ hasText: 'Open this device through your Relay account' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Sharing permissions', exact: true }).click();
  await expect(page.locator('.thread-export-dialog-title')).toHaveText('Sharing permissions');
  await expect(page.getByRole('status').filter({ hasText: 'invite other users' })).toBeVisible();
  await page.keyboard.press('Escape');
  // Preserve the user's plugin settings: this API belongs only to the isolated test server.
  const plugins = await (await request.get(`${base}/api/plugins`)).json();
  const terminal = plugins.find(
    (p: { id: string }) => p.id === 'remote-codex.terminal',
  );
  expect(terminal).toBeTruthy();
  try {
    expect(
      (
        await request.patch(`${base}/api/plugins/${terminal.id}`, {
          data: { enabled: false },
        })
      ).ok(),
    ).toBeTruthy();
    await page.reload();
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Terminal', exact: true }),
    ).toHaveCount(0);
    expect(
      (
        await request.patch(`${base}/api/plugins/${terminal.id}`, {
          data: { enabled: true },
        })
      ).ok(),
    ).toBeTruthy();
    await page.reload();
    await expect(
      page.getByRole('button', { name: 'Terminal', exact: true }),
    ).toBeVisible();
    await page.getByRole('button', { name: 'Terminal', exact: true }).click();
    await expect(page.locator('.xterm-screen')).toBeVisible();
    await page.getByRole('button', { name: 'Chat', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  } finally {
    await request.patch(`${base}/api/plugins/${terminal.id}`, {
      data: { enabled: terminal.enabled },
    });
  }
  await page.screenshot({
    path: `output/playwright/matter-workbench-${testInfo.project.name}.png`,
    fullPage: true,
  });
  await page.locator('summary[aria-label="Thread actions"]').click();
  await page
    .getByRole('button', { name: 'Rename thread', exact: true })
    .click();
  await page
    .getByRole('textbox', { name: 'Thread Title', exact: true })
    .fill('Renamed workspace review');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.locator('.matter-current-title')).toHaveText(
    'Renamed workspace review',
  );
  await page
    .getByRole('link', { name: 'Back to workspaces', exact: true })
    .click();
  await expect(page).toHaveURL(/\/workspaces$/);
});

test('execution timeline expands deferred work and keeps the last reply above the floating composer', async ({
  page,
  request,
}, testInfo) => {
  const absPath = path.resolve(
    process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright',
    `matter-preview-${randomUUID()}`,
  );
  await mkdir(absPath, { recursive: true });
  const ws = await (
    await request.post(`${base}/api/workspaces`, {
      data: { absPath, label: 'Pockymoe' },
    })
  ).json();
  const created = await (
    await request.post(`${base}/api/threads/start`, {
      data: {
        workspaceId: ws.id,
        title: 'A calmer coding workspace',
        provider: 'acp',
        agentId: 'grok',
        model: 'ios-e2e-stream',
        approvalMode: 'yolo',
      },
    })
  ).json();
  const id = created.id ?? created.thread.id;
  expect(
    (
      await request.post(`${base}/api/threads/${id}/prompt`, {
        data: { prompt: 'hello' },
      })
    ).ok(),
  ).toBeTruthy();
  await expect
    .poll(
      async () =>
        (await (await request.get(`${base}/api/threads/${id}`)).json()).thread
          .status,
    )
    .toBe('idle');
  const database = path.resolve(
    process.env.E2E_DATABASE_URL ??
      `.local/e2e-${process.env.E2E_API_PORT ?? 8787}.sqlite`,
  );
  const db = new DatabaseSync(database);
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, value => typeof value === 'string' ? value.toLowerCase() : value);
  db.function('search_body', { deterministic: true }, (text, kind, _source) => ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null);
  const now = Date.now() - 30000;
  const timestamp = (offset: number) => new Date(now + offset).toISOString();
  const prompt =
    'Give our workspace a calmer, more focused interface. Keep the tools close and let the conversation breathe.';
  const reply =
    '## A calmer place to work\n\nThe conversation now sits at the center, with the tools you need just one click away.\n\n### What changed\n\n- **A floating composer.** Messages scroll behind the prompt, with room to read the final reply.\n- **Your threads, together.** Shortcuts and recent chats keep useful conversations close across projects and devices.\n- **Less noise.** Thin icons, quiet surfaces and a restrained green accent make each state easier to read.\n\n### Built around your workflow\n\nThe left rail switches between chat and Terminal. Search finds earlier decisions, the bell brings completed work back to your attention, and the Explorer opens without leaving the conversation.\n\n```tsx\n<Workspace>\n  <Conversation />\n  <FloatingComposer />\n</Workspace>\n```\n\nThe design also adapts to smaller screens: navigation becomes a drawer, while the conversation keeps its full width.\n\nReady for your review.';
  try {
    const turn = db
      .prepare('SELECT id FROM thread_turns WHERE thread_id=?')
      .get(id) as { id: string };
    expect(turn).toBeTruthy();
    db.prepare(
      'UPDATE thread_turns SET display_prompt=?,started_at=?,completed_at=? WHERE id=?',
    ).run(prompt, timestamp(0), timestamp(23000), turn.id);
    const rows = db
      .prepare(
        'SELECT id,item_json FROM thread_history_items WHERE thread_id=? AND turn_id=?',
      )
      .all(id, turn.id) as { id: string; item_json: string }[];
    for (const row of rows) {
      const item = JSON.parse(row.item_json);
      const isUser = item.kind === 'userMessage';
      item.text = isUser ? prompt : reply;
      item.createdAt = timestamp(isUser ? 0 : 23000);
      db.prepare(
        'UPDATE thread_history_items SET item_json=?,created_at=? WHERE id=?',
      ).run(JSON.stringify(item), item.createdAt, row.id);
    }
    const steps = [
      {
        kind: 'reasoning',
        text: 'I’ll inspect the current layout and trace how the composer, navigation and execution history fit together.',
      },
      {
        kind: 'commandExecution',
        text: 'Read workspace structure',
        command: 'ls apps/supervisor-web/src',
        detailText: 'components/  pages/  lib/  index.css',
        exitCode: 0,
      },
      {
        kind: 'commandExecution',
        text: 'Inspect the composer and navigation',
        command:
          'rg -n "ThreadComposer|ThreadWorkspaceLayout" packages/thread-ui/src',
        detailText:
          'Located the composer, thread navigation and timeline surfaces.',
        exitCode: 0,
      },
      {
        kind: 'commandExecution',
        text: 'Verify responsive layout',
        command:
          'pnpm exec playwright test matter-workbench --project=desktop-chromium',
        detailText:
          'Passed: floating composer, shortcut persistence, search and plugin gating.',
        exitCode: 0,
      },
    ];
    for (const [index, step] of steps.entries()) {
      const itemId = randomUUID();
      const item = {
        id: itemId,
        ...step,
        status: 'completed',
        createdAt: timestamp(1000 + index * 3000),
      };
      db.prepare(
        'INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)',
      ).run(
        randomUUID(),
        id,
        turn.id,
        itemId,
        JSON.stringify(item),
        item.createdAt,
        item.createdAt,
      );
    }
  } finally {
    db.close();
  }
  await page.setViewportSize({ width: 1440, height: 960 });
  await page.goto(`/threads/${id}`);
  const expand = page.getByRole('button', { name: /Worked.*Expand turn 1/ });
  await expand.scrollIntoViewIfNeeded();
  await expand.click();
  await expect(page.locator('.thread-execution-timeline')).toBeVisible();
  await expect(page.locator('.thread-graph-worked-summary')).toContainText('3 commands');
  await page.locator('.tlog-run-head').click();
  await expect(page.getByText('Verify responsive layout', { exact: true })).toBeVisible();
  const rows = page.locator('.tlog-row[data-kind="commandExecution"]');
  await expect(rows).toHaveCount(3);
  await expect(page.locator('.tlog-row[data-kind="commandExecution"][data-status="done"]')).toHaveCount(3);
  await expect(rows.first()).not.toContainText('completed');
  expect((await rows.first().boundingBox())!.height).toBeLessThanOrEqual(36);
  expect((await page.locator('.matter-thread-tabs').boundingBox())!.height).toBeLessThanOrEqual(36);
  await expect(page.getByRole('navigation', { name: 'Workspace threads', exact: true }).locator('[aria-current="page"]')).toContainText('A calmer coding workspace');
  await page.locator('.thread-graph-scroll-container').evaluate((e) => {
    e.scrollTop = e.scrollHeight;
  });
  await expect(
    page.getByText('Ready for your review.', { exact: true }),
  ).toBeVisible();
  const final = await page
    .getByText('Ready for your review.', { exact: true })
    .boundingBox();
  const composer = await page.getByTestId('chat-composer').boundingBox();
  expect(final!.y + final!.height).toBeLessThanOrEqual(composer!.y);
  await page.getByTestId('recent-chats').getByRole('button', { name: 'Actions for A calmer coding workspace', exact: true }).click();
  await page.locator('.recent-thread-menu').getByRole('button', { name: 'Star thread', exact: true }).click();
  await page.locator('.thread-graph-scroll-container').evaluate((e) => {
    e.scrollTop = 0;
  });
  await page.screenshot({
    path: 'output/playwright/matter-preview-light.png',
    fullPage: true,
  });
  await page.evaluate(() =>
    localStorage.setItem('remote-codex-theme-mode', 'dark'),
  );
  await page.reload();
  await expect(page.locator('.thread-ui-shell').first()).toHaveAttribute(
    'data-theme-effective',
    'dark',
  );
  await page.getByRole('button', { name: /Worked.*Expand turn 1/ }).click();
  await page.locator('.tlog-run-head').click();
  await page.locator('.thread-graph-scroll-container').evaluate((e) => {
    e.scrollTop = 0;
  });
  await page.screenshot({
    path: 'output/playwright/matter-preview-dark.png',
    fullPage: true,
  });
  await testInfo.attach('preview-url', {
    body: `/threads/${id}`,
    contentType: 'text/plain',
  });
});
