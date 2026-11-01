import { expect, test, type Page } from '@playwright/test';
import type { StatsRange, StatsSummary } from './model';

const evidence = '/tmp/rc-desktop/evidence/devices';
const account = { id: 'devices-test', username: 'Alex', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
function summary(range: StatsRange, device = 0): StatsSummary {
  const count = range === 'today' ? 1 : range === '7d' ? 7 : 30;
  const today = new Date();
  const dates = Array.from({ length: count }, (_, i) => { const day = new Date(today); day.setDate(day.getDate() - count + i + 1); return day.toLocaleDateString('en-CA'); });
  const daily = dates.map((date, i) => ({ date, turns: [8, 13, 6, 21, 34, 28, 37][i % 7]! + device * 3, failed: 1, tokens: 64000 + i * 16000, costUsd: .12 + i * .08 }));
  const turns = daily.reduce((n, d) => n + d.turns, 0);
  return { range, generatedAt: new Date().toISOString(), totals: { turns, completed: turns - count, failed: count, interrupted: 0, avgDurationMs: 192000, p50DurationMs: 120000, durationSamples: turns, inputTokens: daily.reduce((n, d) => n + d.tokens, 0) - count * 16000, outputTokens: count * 16000, cachedTokens: count * 20000, costUsd: daily.reduce((n, d) => n + d.costUsd, 0) }, daily,
    byModel: [{ model: 'GPT-6.1', turns: Math.round(turns * .65), tokens: 40000, costUsd: .12 }, { model: 'Claude Sonnet 4.6', turns: turns - Math.round(turns * .65), tokens: 24000, costUsd: .08 }],
    byWorkspace: ['remoteCodex', 'Desktop App', 'Research', 'Design system', 'Automation'].map((label, i) => ({ workspaceId: `w-${i}`, label, turns: Math.max(1, Math.round(turns / (i + 2))), tokens: 10000, costUsd: .1, lastActivityAt: today.toISOString() })),
    longestThreads: ['构建设备总览与统计', 'Polish desktop navigation', 'Review transport security'].map((title, i) => ({ threadId: `t-${i}`, workspaceId: 'w-0', title, durationMs: 240000 - i * 60000 })), activeThreads: 3, runningTurns: 3 };
}
async function seed(page: Page, dark = false, unsupported = false) {
  const devices = ['MacBook Pro', 'Windows Studio', 'Linux Server'].map((name, i) => ({ id: `device-${i}`, ownerUserId: account.id, name, connected: i !== 2, lastHeartbeatAt: new Date(Date.now() - (i === 2 ? 3 * 3600000 : 0)).toISOString() }));
  await page.addInitScript(({ dark }) => localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light'), { dark });
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: account } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user: account, devices, sharedWithMe: [], sharedByMe: [] } }));
  await page.route(/\/relay\/devices\/device-[0-2]$/, async route => {
    const device = devices.find(d => route.request().url().endsWith(`/${d.id}`))!;
    device.name = route.request().postDataJSON().name;
    await route.fulfill({ json: device });
  });
  await page.route('**/relay/devices/*/healthz', route => route.fulfill({ json: { version: '0.12.59', capabilities: { statsSummary: true } } }));
  await page.route('**/relay/devices/*/presence', route => route.fulfill({ json: { connected: !route.request().url().includes('device-2') } }));
  await page.route('**/relay/devices/*/api/**', async route => {
    const url = new URL(route.request().url()); const deviceId = url.pathname.split('/')[3]!;
    const number = Number(deviceId.slice(-1));
    if (url.pathname.endsWith('/transport/key')) { await route.fulfill({ status: 404, json: { code: 'not_found' } }); return; }
    if (url.pathname.endsWith('/stats/summary')) { await route.fulfill(unsupported ? { status: 404, json: { code: 'not_found', message: 'Unsupported' } } : { json: summary(url.searchParams.get('range') as StatsRange, number) }); return; }
    if (url.pathname.endsWith('/management/supervisor')) { await route.fulfill({ json: { runningVersion: '0.12.59', installedVersion: '0.12.59', latestVersion: number === 1 ? '0.12.60' : '0.12.59', canUpdate: true } }); return; }
    if (url.pathname.endsWith('/management/harnesses')) { await route.fulfill({ json: [] }); return; }
    if (url.pathname.endsWith('/management/templates')) { await route.fulfill({ json: { harnesses: [], profiles: [] } }); return; }
    if (url.pathname.endsWith('/api/workspaces')) { await route.fulfill({ json: [{ id: 'w-0', label: 'remoteCodex', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null }] }); return; }
    const threads = Array.from({ length: 6 }, (_, i) => ({ id: `t-${i}`, workspaceId: 'w-0', title: `设备统计任务 ${i}`, provider: 'codex', status: i < 3 ? 'running' : 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), lastTurnStartedAt: new Date(Date.now() - 192000).toISOString(), lastTurnCompletedAt: i >= 3 ? new Date().toISOString() : null, activeTurnId: i < 3 ? `turn-${i}` : null }));
    if (url.pathname.endsWith('/api/threads')) { await route.fulfill({ json: threads }); return; }
    const id = url.pathname.match(/\/threads\/(t-\d+)/)?.[1];
    if (id) { await route.fulfill({ json: { thread: threads.find(t => t.id === id), turns: [], pendingRequests: id === 't-3' ? [{ id: 'req', kind: 'requestUserInput', title: '需要选择', description: '选择方式' }] : [], pendingSteers: [] } }); return; }
    await route.fulfill({ json: {} });
  });
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, socket => socket.onMessage(() => undefined));
  // Offline devices must restore the same account-scoped IndexedDB snapshot.
  await page.addInitScript(async ({ snapshot }) => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open('rc-desktop-stats', 1); r.onupgradeneeded = () => r.result.createObjectStore('accounts'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    const tx = db.transaction('accounts', 'readwrite');
    tx.objectStore('accounts').put({ 'device-2': { version: '0.12.58', summaries: snapshot } }, `${location.origin}:devices-test:${Intl.DateTimeFormat().resolvedOptions().timeZone}`);
    tx.oncomplete = () => db.close();
  }, { snapshot: Object.fromEntries((['today', '7d', '30d'] as const).map(range => { const s = summary(range, 2); s.generatedAt = new Date(Date.now() - 3 * 3600000).toISOString(); return [range, s]; })) });
  await page.goto('/desktop/devices?desktopMockHost=1');
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await expect(page.getByRole('article', { name: 'Windows Studio', exact: true }).getByText('有更新 0.12.60')).toBeVisible();
}

test('devices overview, ranges, exact tooltips and Settings update target in light and dark', async ({ page }) => {
  for (const dark of [false, true]) {
    await seed(page, dark);
    const theme = dark ? 'dark' : 'light';
    const offline = page.getByRole('article', { name: 'Linux Server', exact: true });
    await expect(offline).toContainText('离线'); await expect(offline).toContainText('缓存 · 3 小时前');
    await page.getByRole('button', { name: '7 天', exact: true }).click();
    await expect(page.locator('.rd-kpi').first().locator('strong')).toHaveText(String([0, 1, 2].reduce((n, d) => n + summary('7d', d).totals.turns, 0)));
    const chart = page.getByRole('group', { name: '每日轮数，按设备堆叠', exact: true });
    await expect(chart.locator('g[tabindex]')).toHaveCount(7);
    await chart.locator('g[tabindex]').last().focus();
    await expect(page.getByRole('tooltip')).toContainText('MacBook Pro');
    await page.screenshot({ path: `${evidence}/overview-7d-tooltip-${theme}.png`, fullPage: true });
    await page.getByRole('button', { name: '30 天', exact: true }).click();
    await expect(chart.locator('g[tabindex]')).toHaveCount(30);
    await page.screenshot({ path: `${evidence}/overview-30d-${theme}.png`, fullPage: true });
    await page.locator('.rd-breakdowns').scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/breakdowns-${theme}.png` });
    await page.locator('.rd-page-header').scrollIntoViewIfNeeded();
    await page.getByLabel('统计设备', { exact: true }).selectOption('device-1');
    await expect(page.locator('.rd-kpi').first()).toContainText('总轮数');
    const targetStatus = page.waitForRequest(request => request.url().endsWith('/relay/devices/device-1/api/management/supervisor'));
    await page.getByRole('button', { name: '有更新 0.12.60', exact: true }).click();
    await targetStatus;
    await expect(page.getByRole('dialog', { name: 'Windows Studio · Runtime' })).toBeVisible();
    await expect(page.getByRole('dialog')).toContainText('0.12.59');
    await page.screenshot({ path: `${evidence}/runtime-update-${theme}.png` });
    await page.keyboard.press('Escape');
    await page.setViewportSize({ width: 900, height: 900 });
    await page.locator('.rc-main').evaluate(el => { el.scrollTop = 0; });
    expect(await page.locator('.rd-page').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await page.screenshot({ path: `${evidence}/overview-900-${theme}.png` });
    await page.setViewportSize({ width: 1440, height: 1120 });
  }
});

test('device rename targets the selected device and unsupported relays hide it cleanly', async ({ page }) => {
  await seed(page);
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
  await page.getByLabel('设备名称', { exact: true }).fill('Studio Mac');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Studio Mac', exact: true })).toBeVisible();
  await page.route(/\/relay\/devices\/device-[0-2]$/, route => route.fulfill({ status: 405, json: { code: 'unsupported', message: 'Not supported' } }));
  await page.getByRole('button', { name: 'Studio Mac 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
  await page.getByLabel('设备名称', { exact: true }).fill('Other name');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '重命名设备' })).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: '暂不支持重命名' })).toBeVisible();
  await page.getByRole('button', { name: 'Studio Mac 的操作', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: '重命名', exact: true })).toHaveCount(0);
});

test('unsupported summaries degrade to partial index data and cached offline values persist', async ({ page }) => {
  await seed(page, false, true);
  await expect(page.locator('.rd-data-note')).toContainText('部分数据');
  await expect(page.getByRole('article', { name: 'MacBook Pro', exact: true })).toContainText('部分数据 · 最近轮次');
  await expect(page.getByRole('article', { name: 'Linux Server', exact: true })).toContainText('缓存 · 3 小时前');
  await page.getByLabel('统计设备', { exact: true }).selectOption('device-0');
  await expect(page.locator('.rd-kpi').filter({ hasText: '估算费用' })).toContainText('—');
  await page.screenshot({ path: `${evidence}/partial-data-light.png` });
  await page.reload();
  await expect(page.getByRole('article', { name: 'Linux Server', exact: true })).toContainText('缓存 · 3 小时前');
  await page.goto('/desktop?desktopMockHost=1');
  await expect(page.locator('[data-desktop-surface]')).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('remote-codex:logout')));
  await expect.poll(() => page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open('rc-desktop-stats'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    return await new Promise<number>(resolve => { const r = db.transaction('accounts').objectStore('accounts').count(); r.onsuccess = () => { resolve(r.result); db.close(); }; });
  })).toBe(0);
});
