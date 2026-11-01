import { writeFileSync } from 'node:fs';
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';

const evidence = '/tmp/rc-desktop/evidence/webnav';
const account = { id: 'desktop-test', username: 'Alex', email: 'desktop@example.invalid', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
const devices = ['MacBook Pro', 'Windows Studio', 'Linux Server'].map((name, i) => ({ id: `device-${i}`, ownerUserId: account.id, name, connected: true, lastHeartbeatAt: new Date().toISOString() }));
const titles = ['修复登录流程与身份验证', 'Implement desktop navigation', 'Review API security', '设计跨设备同步', '优化工作区加载性能', 'Polish command palette'];
function seededThreads(deviceId: string) {
  return Array.from({ length: 54 }, (_, i) => ({
    id: `thread-${i}`, workspaceId: `workspace-${Math.floor(i / 18)}`, title: `${titles[i % titles.length]}${i > 5 ? ` · ${i}` : ''}`,
    provider: ['codex', 'claude', 'acp'][i % 3], agentName: i === 8 ? 'reviewer' : null,
    parentThreadId: i === 8 || i === 9 ? 'thread-0' : null, rootThreadId: i === 8 || i === 9 ? 'thread-0' : null,
    closedAt: i === 9 ? new Date(Date.now() - 86400000).toISOString() : null,
    status: i === 0 || i === 8 ? 'running' : i === 3 ? 'failed' : 'idle', isPinned: i === 4,
    summaryText: '已完成更新，所有检查通过。', lastError: i === 3 ? 'Network connection interrupted' : null,
    createdAt: '2026-10-01T00:00:00Z', updatedAt: new Date(Date.now() - i * 1800000).toISOString(),
    lastTurnStartedAt: i === 0 || i === 8 ? new Date(Date.now() - 192000).toISOString() : null,
    lastTurnCompletedAt: i === 3 || (i > 3 && i < 7) ? new Date(Date.now() - 300000).toISOString() : null,
    activeTurnId: i === 0 || i === 8 ? 'turn' : null, deviceId,
  }));
}
async function seed(page: Page, dark = false) {
  const sockets = new Map<string, WebSocketRoute>();
  await page.addInitScript(({ dark }) => {
    localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light');
  }, { dark });
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: account } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user: account, devices, sharedWithMe: [], sharedByMe: [] } }));
  await page.route('**/relay/devices/*/api/**', async route => {
    const url = new URL(route.request().url());
    const deviceId = url.pathname.split('/')[3]!;
    if (url.pathname.endsWith('/transport/key')) { await route.fulfill({ status: 404, json: { code: 'not_found' } }); return; }
    if (url.pathname.endsWith('/api/workspaces')) { await route.fulfill({ json: Array.from({ length: 3 }, (_, i) => ({ id: `workspace-${i}`, label: ['remoteCodex', 'Desktop App', 'Research'][i], isFavorite: i === 0, createdAt: '2026-10-01T00:00:00Z', lastOpenedAt: null })) }); return; }
    if (url.pathname.endsWith('/api/threads')) { await route.fulfill({ json: seededThreads(deviceId) }); return; }
    const id = url.pathname.match(/\/threads\/(thread-\d+)/)?.[1];
    if (id) {
      const thread = seededThreads(deviceId).find(thread => thread.id === id)!;
      await route.fulfill({ json: { thread, workspace: { id: thread.workspaceId, label: 'remoteCodex', absPath: '/fixture', isFavorite: true }, turns: [], pendingSteers: [], workspacePathStatus: 'present', pendingRequests: id === 'thread-1' || id === 'thread-2' ? [{ id: 'request', kind: id === 'thread-1' ? 'permissionRequest' : 'requestUserInput', title: id === 'thread-1' ? '批准运行测试' : '选择实现方式', description: '请确认后继续', createdAt: new Date().toISOString(), questions: [] }] : [] } }); return;
    }
    await route.fulfill({ json: {} });
  });
  await page.route('**/relay/devices/*/presence', route => route.fulfill({ json: { connected: true } }));
  await page.route('**/api/plugins', route => route.fulfill({ json: [] }));
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, socket => {
    sockets.set(new URL(socket.url()).pathname.split('/')[3]!, socket);
    socket.onMessage(() => undefined);
  });
  // The mock transport is legacy and device-scoped. Production encryption is
  // separately tested through the real backend fixture when its binary is ready.
  await page.goto('/desktop?desktopMockHost=1');
  await expect(page.locator('[data-desktop-surface]')).toBeVisible();
  await expect(page.getByRole('treeitem').filter({ hasText: 'Linux Server' })).toBeVisible();
  await expect.poll(() => sockets.size).toBe(3);
  return { send: (deviceId: string, event: unknown) => sockets.get(deviceId)!.send(JSON.stringify(event)) };
}

test('tree, local filtering, keyboard navigation, palette and resize', async ({ page }) => {
  await seed(page);
  await expect(page.locator('[data-row-id="smart:attention"]')).toContainText('需要你');
  await expect(page.locator('[data-row-id="smart:attention"] .rc-count')).not.toHaveText('—');
  await page.screenshot({ path: `${evidence}/cold-start-light.png` });
  await page.screenshot({ path: `${evidence}/many-threads-light.png` });
  await page.screenshot({ path: `${evidence}/attention-light.png` });
  await page.getByRole('button', { name: '过滤线程', exact: true }).click();
  await page.getByRole('textbox', { name: '过滤线程', exact: true }).fill('palette');
  await expect(page.getByRole('treeitem').filter({ hasText: 'Polish command palette' }).first()).toBeVisible();
  await expect(page.locator('.rc-row-label mark').first()).toBeVisible();
  await expect(page.locator('.rc-row-exiting')).toHaveCount(0);
  await page.locator('.rc-tree-row').evaluateAll(rows => Promise.all(rows.flatMap(row => row.getAnimations()).map(animation => animation.finished)));
  await page.screenshot({ path: `${evidence}/filter-light.png` });
  await page.getByRole('textbox', { name: '过滤线程', exact: true }).press('ArrowDown');
  const tree = page.getByRole('tree'); await expect(tree).toBeFocused();
  await tree.press('ArrowDown'); await expect(tree).toHaveAttribute('aria-activedescendant', /rc-row-/);
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: '命令面板' })).toBeVisible();
  await page.getByRole('combobox', { name: '搜索线程、工作区、命令' }).fill('desktop');
  await expect(page.getByRole('dialog', { name: '命令面板' }).getByRole('option').first()).toContainText('Implement desktop navigation');
  await page.locator('.rc-palette').evaluate(element => Promise.all(element.getAnimations().map(animation => animation.finished)));
  await page.screenshot({ path: `${evidence}/palette-light.png` });
  await page.keyboard.press('ArrowDown'); await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: '命令面板' })).toHaveCount(0);
  await page.getByRole('separator', { name: '侧栏宽度' }).focus(); await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('separator')).toHaveAttribute('aria-valuenow', '274');
  await page.keyboard.press('Control+\\'); await expect(page.getByRole('complementary', { name: '桌面侧栏' })).toHaveCount(0);
  await page.keyboard.press('Control+\\'); await expect(page.getByRole('complementary', { name: '桌面侧栏' })).toBeVisible();
  await page.keyboard.press('Control+Shift+n');
  await expect(page.getByRole('dialog', { name: '选择线程位置' })).toBeVisible();
  await expect(page.getByRole('combobox', { name: '新线程设备' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: '选择线程位置' })).toHaveCount(0);
});

test('warm cache never renders a skeleton or empty tree while slow revalidation runs', async ({ page }) => {
  await seed(page);
  await expect.poll(() => page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const req = indexedDB.open('rc-desktop-index'); req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error); });
    return await new Promise<boolean>(resolve => { const req = db.transaction('accounts').objectStore('accounts').get(`${location.origin}:desktop-test`); req.onsuccess = () => { resolve(Boolean(req.result?.threads?.length === 162)); db.close(); }; });
  })).toBe(true);
  await page.route('**/relay/portal', async route => { await new Promise(resolve => setTimeout(resolve, 1000)); await route.fulfill({ json: { user: account, devices, sharedWithMe: [], sharedByMe: [] } }); });
  await page.addInitScript(() => {
    const evidence = { bad: false, paintedAt: 0 }; Object.assign(window, { desktopWarmEvidence: evidence });
    new MutationObserver(() => {
      const shell = document.querySelector('[data-desktop-surface]');
      if (!shell) return;
      if (!evidence.paintedAt && shell.querySelector('[data-row-id="device:device-2"]')) evidence.paintedAt = performance.now();
      if (shell.querySelector('[data-testid="desktop-skeleton"], .rc-tree-empty')) evidence.bad = true;
    }).observe(document, { childList: true, subtree: true });
  });
  await page.reload(); await expect(page.locator('[data-row-id="device:device-2"]')).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { desktopWarmEvidence: { bad: boolean } }).desktopWarmEvidence.bad)).toBe(false);
  await expect(page.locator('[data-testid="desktop-skeleton"]')).toHaveCount(0);
  const timing = await page.evaluate(() => performance.getEntriesByName('rc-desktop-tree-paint').at(-1)?.duration);
  expect(timing).toBeLessThan(150);
  writeFileSync(`${evidence}/warm-cache-paint.json`, JSON.stringify({ durationMs: timing }));
  await test.info().attach('warm-cache-paint-ms', { body: JSON.stringify({ durationMs: timing }), contentType: 'application/json' });
});

test('pointer engagement freezes reorder until 800ms after leaving', async ({ page }) => {
  const live = await seed(page);
  await page.mouse.move(130, 280);
  await expect(page.locator('.rc-navigation')).toHaveAttribute('data-reorder-frozen', 'true');
  const before = await page.locator('[data-row-id^="thread:"]').evaluateAll(rows => rows.map(row => row.getAttribute('data-row-id')));
  live.send('device-2', { type: 'thread.turn.started', threadId: 'thread-6', timestamp: new Date(Date.now() + 1000).toISOString(), payload: { turnId: 'live-reorder' } });
  await expect(page.locator('[data-row-id="thread:device-2:thread-6"] .rc-status-running')).toBeVisible();
  await expect(page.locator('.rc-navigation')).toHaveAttribute('data-reorder-frozen', 'true');
  expect(await page.locator('[data-row-id^="thread:"]').evaluateAll(rows => rows.map(row => row.getAttribute('data-row-id')))).toEqual(before);
  await page.mouse.move(700, 700);
  await expect(page.locator('.rc-navigation')).toHaveAttribute('data-reorder-frozen', 'false');
  const after = await page.locator('[data-row-id^="thread:"]').evaluateAll(rows => rows.map(row => row.getAttribute('data-row-id')));
  expect(after).not.toEqual(before);
  expect(after.indexOf('thread:device-2:thread-6')).toBeLessThan(after.indexOf('thread:device-2:thread-0'));
});

test('dark palette, attention states and 900px window remain aligned', async ({ page }) => {
  await seed(page, true);
  await expect(page.locator('.rc-status-attention').first()).toBeVisible();
  await page.screenshot({ path: `${evidence}/cold-start-dark.png` });
  await page.screenshot({ path: `${evidence}/many-threads-dark.png` });
  await page.screenshot({ path: `${evidence}/attention-dark.png` });
  await page.getByRole('button', { name: '过滤线程', exact: true }).click();
  await page.getByRole('textbox', { name: '过滤线程', exact: true }).fill('review');
  await expect(page.locator('.rc-row-exiting')).toHaveCount(0);
  await page.locator('.rc-tree-row').evaluateAll(rows => Promise.all(rows.flatMap(row => row.getAnimations()).map(animation => animation.finished)));
  await page.screenshot({ path: `${evidence}/filter-dark.png` });
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: '命令面板' })).toBeVisible();
  await page.locator('.rc-palette').evaluate(element => Promise.all(element.getAnimations().map(animation => animation.finished)));
  await page.screenshot({ path: `${evidence}/palette-dark.png` });
  await page.keyboard.press('Escape');
  await page.setViewportSize({ width: 900, height: 700 });
  await expect(page.getByRole('button', { name: '新建线程', exact: true })).toBeVisible();
  expect(await page.locator('.rc-toolbar').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.setViewportSize({ width: 1280, height: 850 });
  for (const opaque of [false, true]) {
    await page.goto(`/desktop?desktopMockHost=1&desktopMockPlatform=win32${opaque ? '&desktopMockOpaque=1' : ''}`);
    await expect(page.locator('[data-desktop-surface]')).toBeVisible();
    await expect(page.locator('[data-row-id="device:device-2"]')).toBeVisible();
    expect(await page.locator('.rc-toolbar').evaluate(element => getComputedStyle(element).paddingRight)).toBe('138px');
    await page.screenshot({ path: `${evidence}/windows-${opaque ? 'opaque' : 'mica'}-dark.png` });
  }
});


test('normal browser keeps the web surface and never loads desktop code', async ({ page }) => {
  const desktopRequests: string[] = [];
  page.on('request', request => { if (request.url().includes('/src/desktop/')) desktopRequests.push(request.url()); });
  await page.route('**/api/**', route => {
    const path = new URL(route.request().url()).pathname;
    return route.fulfill({ json: path.endsWith('/auth/session') ? { authenticated: true, authRequired: false, mode: 'local' } : path.endsWith('/config/runtime') ? {} : [] });
  });
  await page.goto('/workspaces');
  await expect(page.getByRole('heading', { name: 'Workspaces', exact: true })).toBeVisible();
  await expect(page.locator('[data-desktop-surface]')).toHaveCount(0);
  expect(desktopRequests).toEqual([]);
});
