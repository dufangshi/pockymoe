import { selectControl } from '../selectControl';
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { DesktopIndex, DesktopThread } from '../../apps/supervisor-web/src/desktop/data/types';
import type { ThreadActionRequestDto } from '@pockymoe/shared';
const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop';
const account = { id: 'desktop-v2-test', username: 'Alex', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
const now = Date.now();
const iso = (age: number) => new Date(now - age).toISOString();
const requestFor = (deviceId: string): ThreadActionRequestDto => ({ id: `${deviceId}-approval`, kind: 'permissionRequest', title: '运行验证', description: '运行定向测试并检查结果', createdAt: iso(60000), turnId: 'live', itemId: null, questions: [{ id: 'permission', header: '审批', question: '允许执行定向测试？', isOther: false, isSecret: false, options: [{ label: 'Allow once', description: 'allow once' }, { label: 'Reject', description: 'reject once' }] }] });
function seeded(count: number): DesktopIndex {
  const devices = ['MacBook Pro', 'Windows Studio', 'Build Box'].map((name, i) => ({ id: `device-${i}`, name, connected: true, lastHeartbeatAt: iso(1000), cachedAt: iso(60000) }));
  const workspaces = devices.flatMap(device => ['Pockymoe', 'Desktop', 'Research'].map((label, i) => ({ id: `w-${i}`, deviceId: device.id, key: `${device.id}:w-${i}`, hostId: 'fixture', absPath: `/workspace/${label}`, label, createdAt: iso(86400000 * 60), lastOpenedAt: iso(60000), isFavorite: i === 0 })));
  const threads = Array.from({ length: count }, (_, i) => {
    const deviceId = `device-${Math.floor(i / (count / 3))}`, ordinal = i % (count / 3), id = `t-${ordinal}`;
    const active = ordinal < 4;
    const at = iso(active ? ordinal * 90000 : ordinal * 3600000 * 4);
    return { key: `${deviceId}:${id}`, deviceId, id, workspaceId: `w-${Math.floor(ordinal / Math.max(1, Math.ceil(count / 9))) % 3}`,
      provider: ['acp', 'claude', 'codex'][ordinal % 3], agentId: ['grok', 'claude', 'codex'][ordinal % 3], providerSessionId: null, source: 'supervisor', title: ['完善桌面地图与活动索引', 'Review security boundaries', '跨设备布局与主题验证', 'Reduce cold start latency'][ordinal % 4] + (ordinal > 3 ? ` · ${ordinal}` : ''), model: ordinal % 2 ? 'claude-sonnet-4-6' : 'gpt-6.1', reasoningEffort: 'high', collaborationMode: 'default', approvalMode: 'guarded', isLoaded: true, isPinned: false,
      status: ordinal === 0 || ordinal === 3 ? 'running' : ordinal === 1 ? 'failed' : 'idle', summaryText: '更新已完成，所有定向检查通过。', lastError: ordinal === 1 ? '连接意外中断，请检查后重试。' : null,
      createdAt: iso(86400000 * 60), updatedAt: at, lastTurnStartedAt: ordinal === 0 || ordinal === 3 ? iso(5_170_000) : iso(480000), lastTurnCompletedAt: ordinal === 0 || ordinal === 3 ? null : at, activeTurnId: ordinal === 0 || ordinal === 3 ? 'live' : null,
      lastTurnId: 'last-turn', lastViewedAt: ordinal === 1 || ordinal === 2 ? null : iso(1000),
      requests: ordinal === 0 ? [{ id: `${deviceId}-approval`, kind: 'approval', summary: '运行定向测试并检查结果', createdAt: iso(60000) }] : [], resultSummary: ordinal === 1 ? '连接意外中断，请检查后重试。' : '更新已完成，所有定向检查通过。',
    } as DesktopThread;
  });
  const activity = Object.fromEntries(devices.map(device => [device.id, { since: iso(86400000), hours: 24, threads: threads.filter(thread => thread.deviceId === device.id).map((thread, i) => ({ threadId: thread.id, turns: 4 + i * 2, activeMs: i < 4 ? 1200000 - i * 180000 : 0, totalMs: 3600000 + i * 60000, hourlyMs: Array.from({ length: 24 }, (_, hour) => hour % 4 === i % 4 ? 900000 : 0), lastPrompt: '检查线程地图的加载、导航与交互，确保所有主题下都清晰稳定。', tokenUsage: { totalTokens: 12000, inputTokens: 8000, cachedInputTokens: 2000, outputTokens: 4000, reasoningOutputTokens: 1000 }, priceEstimate: { currency: 'USD' as const, totalUsd: .36, inputUsd: .12, cachedInputUsd: .02, cacheWriteInputUsd: 0, outputUsd: .22 } })) }]));
  return { accountId: account.id, devices, workspaces, threads, activity, extras: {}, hydrated: true, refreshing: false, error: null, pulseKeys: [] };
}
async function setup(page: Page, count = 12, pack = 'classic', dark = false, locale = 'zh-CN') {
  const state = seeded(count), sockets = new Map<string, WebSocketRoute>();
  const pending = new Set(state.devices.map(device => device.id));
  await page.addInitScript(({ pack, dark, locale }) => {
    localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light'); localStorage.setItem('pockymoe-theme-preset', pack); if (!localStorage.getItem('remote-codex.locale') && locale !== 'auto') localStorage.setItem('remote-codex.locale', locale);
    window.addEventListener('desktopMock:appearance', event => Object.assign(window, { desktopAppearanceLocale: (event as CustomEvent).detail.locale }));
    const seen = { badFrame: false, mapSeen: false }; Object.assign(window, { desktopWarmEvidence: seen });
    new MutationObserver(() => {
      const map = document.querySelector('.desktop-map-scene:not([hidden]) .thread-map'); if (!map) return;
      seen.mapSeen = true;
      if (map.querySelector('.thread-map-empty, .thread-map-loading')) seen.badFrame = true;
    }).observe(document, { childList: true, subtree: true });
  }, { pack, dark, locale });
  // Scope: UI/data integration. Account cryptography has dedicated upstream
  // regressions; this test-only module boundary provisions no credentials.
  await page.route('**/src/components/AccountSessionGate.tsx*', route => route.fulfill({ contentType: 'application/javascript', body: 'export function AccountSessionGate({children}) { return children; }' }));
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:18925\/\?token=/, socket => socket.onMessage(() => undefined));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: account } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user: account, devices: state.devices.map(device => ({ ...device, ownerUserId: account.id })), sharedWithMe: [], sharedByMe: [] } }));
  await page.route('**/relay/account/notifications/preferences', route => route.fulfill({ json: { scope: 'all', devices: [], workspaces: [] } }));
  await page.route('**/relay/api/config/runtime', route => route.fulfill({ json: { defaultBackend: 'acp', agentBackends: [], workspaceRoot: '/fixture' } }));
  await page.route('**/relay/api/plugins*', route => route.fulfill({ json: [] }));
  await page.route('**/relay/account/workbench**', route => route.fulfill({ json: { threads: [], notifications: [] } }));
  await page.route('**/relay/devices/*/presence*', route => route.fulfill({ json: { connected: true } }));
  await page.route(/\/relay\/devices\/device-[0-2]$/, route => route.fulfill({ status: 405, headers: { allow: 'PATCH, DELETE' }, json: { code: 'method_not_allowed' } }));
  await page.route('**/relay/devices/*/api/**', async route => {
    const url = new URL(route.request().url()), deviceId = url.pathname.split('/')[3]!;
    if (url.pathname.endsWith('/transport/key')) return route.fulfill({ status: 404, json: { code: 'not_found' } });
    if (url.pathname.endsWith('/api/workspaces')) return route.fulfill({ json: state.workspaces.filter(workspace => workspace.deviceId === deviceId) });
    if (url.pathname.endsWith('/api/threads')) return route.fulfill({ json: state.threads.filter(thread => thread.deviceId === deviceId) });
    if (url.pathname.endsWith('/thread-activity')) return route.fulfill({ json: state.activity[deviceId] });
    if (url.pathname.endsWith('/management/supervisor')) return route.fulfill({ json: { runningVersion: '0.13.41', latestVersion: '0.13.42', canUpdate: true } });
    if (url.pathname.endsWith('/device/metrics')) return route.fulfill({ json: { sampledAt: iso(0), cpu: { usagePercent: 18 }, memory: { usagePercent: 42 }, gpus: [] } });
    if (url.pathname.endsWith('/stats/summary')) return route.fulfill({ json: { totals: { costUsd: 1.24 } } });
    if (url.pathname.endsWith('/config/runtime')) return route.fulfill({ json: { defaultBackend: 'acp' } });
    if (url.pathname.endsWith('/plugins')) return route.fulfill({ json: [] });
    const id = url.pathname.match(/\/threads\/(t-\d+)/)?.[1];
    if (id) {
      const thread = state.threads.find(thread => thread.key === `${deviceId}:${id}`)!;
      if (url.pathname.endsWith('/respond')) {
        expect(route.request().postDataJSON()).toEqual({ answers: { permission: { answers: ['Allow once'] } } }); pending.delete(deviceId); thread.requests = [];
      }
      return route.fulfill({ json: { thread, workspace: state.workspaces.find(workspace => workspace.key === `${deviceId}:${thread.workspaceId}`), turns: [], totalTurnCount: 12, pendingSteers: [], pendingRequests: id === 't-0' && pending.has(deviceId) ? [requestFor(deviceId)] : [] } });
    }
    return route.fulfill({ json: {} });
  });
  await page.route('**/api/plugins*', route => route.fulfill({ json: [] }));
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, socket => { sockets.set(new URL(socket.url()).pathname.split('/')[3]!, socket); socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' }))); });
  await page.route('**/_desktop_seed', route => route.fulfill({ contentType: 'text/html', body: '<!doctype html><html><head><base href="/"></head><body></body></html>' }));
  await page.goto('/_desktop_seed');
  await page.evaluate(async snapshot => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open('pockymoe-desktop-index', 1); r.onupgradeneeded = () => r.result.createObjectStore('accounts'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    await new Promise<void>(resolve => { const tx = db.transaction('accounts', 'readwrite'); tx.objectStore('accounts').put(snapshot, `${location.origin}:${location.origin}:${snapshot.accountId}`); tx.oncomplete = () => { db.close(); resolve(); }; });
  }, state);
  await page.goto('/?desktopMockHost=1');
  await expect(page.locator('.thread-map[data-presentation="page"]')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
  await page.evaluate(() => document.fonts.ready);
  await expect.poll(() => sockets.size).toBe(3);
  return { state, send: (deviceId: string, event: unknown) => sockets.get(deviceId)!.send(JSON.stringify(event)) };
}

test('warm index, live tile/sidebar updates, approval and revealInMap', async ({ page }) => {
  const backend = await setup(page);
  await expect(page.locator('[data-map-id="thread:device-0:t-0"]')).toHaveAttribute('data-waiting', 'true');
  expect(await page.evaluate(() => (window as any).desktopWarmEvidence)).toEqual({ badFrame: false, mapSeen: true });
  const tile = page.locator('[data-map-id="thread:device-0:t-3"]'); await expect(tile).toBeVisible();
  const before = await page.locator('.thread-map-tile').evaluateAll(tiles => tiles.map(tile => ({ id: tile.getAttribute('data-map-id'), style: tile.getAttribute('style') })));
  const point = await tile.boundingBox(); await page.mouse.move(point!.x + 6, point!.y + 6);
  backend.send('device-0', { type: 'thread.turn.completed', threadId: 't-3', timestamp: new Date().toISOString(), payload: { turnId: 'live', status: 'completed', error: null } });
  await expect(tile).toHaveAttribute('data-unread', 'true');
  await expect(page.locator('[data-activity-group="completed"] [data-activity-key="device-0:t-3"]')).toBeVisible();
  expect(await page.locator('.thread-map-tile').evaluateAll(tiles => tiles.map(tile => ({ id: tile.getAttribute('data-map-id'), style: tile.getAttribute('style') })))).toEqual(before);
  const approval = page.locator('.thread-map-attention-card').filter({ hasText: 'MacBook Pro' });
  await approval.getByRole('button', { name: '批准', exact: true }).click();
  await expect(page.locator('[data-map-id="thread:device-0:t-0"]')).not.toHaveAttribute('data-waiting', 'true');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('desktopMock:command', { detail: { id: 'reveal', type: 'revealInMap', key: 'device-1:t-2' } })));
  await expect(page.locator('[data-map-id="thread:device-1:t-2"]')).toBeVisible();
  await expect(page.locator('[data-map-id="thread:device-1:t-2"]')).toHaveAttribute('data-pulse', /waiting|done/);
});

for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 900]) {
  test(`map screenshots ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    for (const count of process.env.DESKTOP_SCREEN_COUNT === '12' ? [12] : [12, 150, 600]) {
      await setup(page, count, pack, dark, process.env.DESKTOP_LOCALE ?? 'zh-CN');
      const name = `${pack}-${dark ? 'dark' : 'light'}-${width}-${count}`;
      await page.screenshot({ path: `${evidence}/map-${name}.png` });
      if (count === 12) {
        await page.evaluate(() => window.dispatchEvent(new CustomEvent('desktopMock:command', { detail: { id: 'look', type: 'revealInMap', key: 'device-0:t-0' } })));
        const tile = page.locator('[data-map-id="thread:device-0:t-0"]'); await expect(tile).toBeVisible();
        await tile.locator('.thread-map-quick-look').click();
        await expect(page.getByRole('dialog', { name: /详情|Details/ })).toBeVisible();
        await page.screenshot({ path: `${evidence}/quicklook-${name}.png` });
        await page.keyboard.press('Escape');
        if (process.env.DESKTOP_SETTINGS_SHOTS === '1') {
          await page.getByRole('button', { name: process.env.DESKTOP_LOCALE === 'en' ? 'Settings' : '设置', exact: true }).first().click();
          await expect(page.getByTestId('settingsDialog')).toBeVisible();
          await page.getByRole('tab', { name: /通知|Notifications/ }).click();
          const native = page.getByRole('button', { name: process.env.DESKTOP_LOCALE === 'en' ? 'System notifications · Pet' : '系统通知 · 桌宠', exact: true });
          await expect(native).toBeEnabled();
          await page.evaluate(() => window.addEventListener('desktopMock:preferences', event => Object.assign(window, { desktopPreferenceSection: (event as CustomEvent).detail })));
          await native.click();
          expect(await page.evaluate(() => (window as any).desktopPreferenceSection)).toBe('general');
          await page.screenshot({ path: `${evidence}/notifications-${name}.png` });
          await page.keyboard.press('Escape');
        }
      }
    }
  });
}


test('desktop language follows navigator and saved preference, updates live, and formats long clocks', async ({ page }) => {
  await setup(page, 12, 'classic', false, 'auto');
  expect(await page.evaluate(() => navigator.language)).toMatch(/^en/);
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const activity = page.locator('.desktop-map-scene .desktop-activity');
  await expect(activity).toHaveAttribute('aria-label', 'Activity');
  await expect(page.locator('.desktop-online-count').first()).toHaveText('3 online');
  if (process.env.DESKTOP_COUNT_SHOTS === '1') await page.screenshot({ path: `${evidence}/online-count-en.png` });
  await expect(activity.locator('[data-activity-group="attention"]')).toContainText('Needs you');
  await expect(activity.locator('[data-activity-group="running"] .desktop-activity-time').first()).toHaveText(/^1:\d{2}:\d{2}$/);
  await page.getByRole('button', { name: 'Settings', exact: true }).first().click();
  await page.getByRole('tab', { name: 'Preferences', exact: true }).click();
  await selectControl(page, page.getByRole('combobox', { name: 'Language', exact: true }), 'zh-CN');
  await expect.poll(() => page.evaluate(() => (window as any).desktopAppearanceLocale), { timeout: 1000 }).toBe('zh-CN');
  await page.keyboard.press('Escape');
  await expect(activity).toHaveAttribute('aria-label', '活动');
  await expect(page.locator('.desktop-online-count').first()).toHaveText('3 台在线');
  if (process.env.DESKTOP_COUNT_SHOTS === '1') await page.screenshot({ path: `${evidence}/online-count-zh-CN.png` });
  await expect(activity.locator('[data-activity-group="attention"]')).toContainText('需要你');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await page.getByRole('button', { name: '设置', exact: true }).first().click();
  await page.getByRole('tab', { name: '偏好设置', exact: true }).click();
  await selectControl(page, page.getByRole('combobox', { name: '语言', exact: true }), 'en');
  await expect.poll(() => page.evaluate(() => (window as any).desktopAppearanceLocale), { timeout: 1000 }).toBe('en');
  await page.keyboard.press('Escape');
  await expect(activity).toHaveAttribute('aria-label', 'Activity');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('desktopMock:command', { detail: { id: 'look', type: 'revealInMap', key: 'device-0:t-0' } })));
  await page.locator('[data-map-id="thread:device-0:t-0"] .thread-map-quick-look').click();
  await expect(page.getByRole('dialog', { name: /Details/ })).toContainText('Waiting for approval');
  await expect(page.getByRole('dialog', { name: /Details/ }).getByRole('button', { name: 'Open thread', exact: true })).toBeVisible();
});
