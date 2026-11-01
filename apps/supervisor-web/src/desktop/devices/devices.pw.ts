import { expect, test, type Page } from '@playwright/test';
import type { StatsRange, StatsSummary } from './model';
import type { DeviceMetricsDto } from '@pockymoe/shared';

const evidence = '/tmp/rc-desktop/evidence/devices/v2';
const account = { id: 'devices-test', username: 'Alex', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
function hardware(device: number): DeviceMetricsDto {
  const sampledAt = new Date().toISOString(), totalBytes = 32 * 2 ** 30, usedBytes = 12.4 * 2 ** 30;
  const usagePercent = 32 + device * 12;
  return { sampledAt, sampleWindowMs: 2000, platform: device ? 'windows' : 'darwin', environment: 'native', cpu: { model: device ? 'AMD Ryzen' : 'Apple M4 Pro', usagePercent, logicalCoreCount: 4, cores: [0, 1, 2, 3].map(index => ({ index, usagePercent: usagePercent + index })) }, memory: { totalBytes, usedBytes, availableBytes: totalBytes - usedBytes, usagePercent: usedBytes / totalBytes * 100 }, swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null }, cpuPower: { watts: 24.5, source: 'CPU package', reason: null }, cpuTemperature: { celsius: 64, source: 'native', reason: null, sensors: [] }, gpus: [{ id: 'gpu', name: device ? 'NVIDIA RTX 4080' : 'Apple M4 Pro GPU', usagePercent: 18, usedMemoryBytes: 2 * 2 ** 30, totalMemoryBytes: 16 * 2 ** 30, power: { watts: null, source: null, reason: '未提供 GPU 功耗' }, source: 'native' }], hardwareSampledAt: sampledAt, hardwareNotes: [], limits: null };
}
function summary(range: StatsRange, device = 0): StatsSummary {
  const count = range === 'today' ? 1 : range === '7d' ? 7 : 30;
  const today = new Date();
  const dates = Array.from({ length: count }, (_, i) => { const day = new Date(today); day.setDate(day.getDate() - count + i + 1); return day.toLocaleDateString('en-CA'); });
  const daily = dates.map((date, i) => ({ date, turns: [8, 13, 6, 21, 34, 28, 37][i % 7]! + device * 3, failed: 1, tokens: 64000 + i * 16000, costUsd: .12 + i * .08 }));
  const turns = daily.reduce((n, d) => n + d.turns, 0);
  return { range, generatedAt: new Date().toISOString(), totals: { turns, completed: turns - count, failed: count, interrupted: 0, avgDurationMs: 192000, p50DurationMs: 120000, durationSamples: turns, inputTokens: daily.reduce((n, d) => n + d.tokens, 0) - count * 16000, outputTokens: count * 16000, cachedTokens: count * 20000, costUsd: daily.reduce((n, d) => n + d.costUsd, 0) }, daily,
    byModel: [{ model: 'GPT-6.1', turns: Math.round(turns * .65), tokens: 40000, costUsd: .12 }, { model: 'Claude Sonnet 4.6', turns: turns - Math.round(turns * .65), tokens: 24000, costUsd: .08 }],
    byWorkspace: ['Pockymoe', 'Desktop App', 'Research', 'Design system', 'Automation'].map((label, i) => ({ workspaceId: `w-${i}`, label, turns: Math.max(1, Math.round(turns / (i + 2))), tokens: 10000, costUsd: .1, lastActivityAt: today.toISOString() })),
    longestThreads: ['构建设备总览与统计', 'Polish desktop navigation', 'Review transport security'].map((title, i) => ({ threadId: `t-${i}`, workspaceId: 'w-0', title, durationMs: 240000 - i * 60000 })), activeThreads: 3, runningTurns: 3 };
}

for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 900]) {
  test(`device surfaces ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1100 });
    await seed(page, pack, dark);
    const name = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
    await expect(page.locator('html')).toHaveAttribute('data-theme-effective', dark ? 'dark' : 'light');
    await expect(page.locator('.pd-counts [data-status="waiting"]').first()).toHaveCSS('clip-path', /polygon/);
    await expect(page.getByRole('article', { name: 'Linux Server', exact: true })).toContainText('缓存 · 3 小时前');
    await expect(page.getByRole('article', { name: 'Windows Studio', exact: true }).locator('.device-monitor-button')).toContainText('44%');
    await page.screenshot({ path: `${evidence}/${name}-overview.png` });
    await page.getByRole('button', { name: '7 天', exact: true }).click();
    await expect(page.locator('.pd-kpi').first().locator('strong')).toHaveText('504');
    const chart = page.getByRole('group', { name: '每日轮数，按设备堆叠', exact: true });
    await expect(chart.locator('g[tabindex]')).toHaveCount(7);
    await chart.scrollIntoViewIfNeeded(); await chart.locator('g[tabindex]').last().focus();
    await expect(page.getByRole('tooltip')).toContainText('MacBook Pro');
    await page.screenshot({ path: `${evidence}/${name}-tooltip.png` });
    await page.getByRole('button', { name: '30 天', exact: true }).click();
    await expect(chart.locator('g[tabindex]')).toHaveCount(30);
    await page.locator('.pd-breakdowns').scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/${name}-statistics.png` });
    await page.getByLabel('统计设备', { exact: true }).selectOption('device-1');
    await expect(page.locator('.pd-device-card')).toHaveCount(1);
    await page.locator('.pd-page-header').scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/${name}-detail.png` });
    const metrics = page.waitForRequest(r => r.url().includes('/relay/devices/device-1/api/device/metrics'));
    await page.locator('.pd-device-card .device-monitor-button').click(); await metrics;
    await expect(page.locator('.device-monitor-dialog')).toContainText('NVIDIA RTX 4080');
    await page.screenshot({ path: `${evidence}/${name}-monitor.png` });
    await page.keyboard.press('Escape');
    const management = page.waitForRequest(r => r.url().includes('/relay/devices/device-1/api/management/supervisor'));
    await page.getByRole('button', { name: '有更新 0.13.42', exact: true }).click(); await management;
    await expect(page.getByRole('dialog')).toContainText('0.13.41');
    await page.screenshot({ path: `${evidence}/${name}-update.png` });
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Windows Studio 的操作', exact: true }).click();
    await expect(page.getByRole('menu')).toBeVisible();
    await expect(page.getByRole('menuitem', { name: '在地图中显示' })).toBeFocused();
    await page.keyboard.press('End');
    await expect(page.getByRole('menuitem', { name: '在浏览器中管理' })).toBeFocused();
    await page.screenshot({ path: `${evidence}/${name}-menu.png` });
    await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
    await expect(page.getByRole('dialog', { name: '重命名设备' })).toBeVisible();
    await page.screenshot({ path: `${evidence}/${name}-rename.png` });
    await page.keyboard.press('Escape');
    await page.route('**/api/stats/summary*', route => route.fulfill({ status: 404, json: { code: 'not_found', message: '旧版设备' } }));
    await page.locator('.pd-page-header').getByRole('button', { name: '刷新', exact: true }).click();
    await expect(page.getByRole('region', { name: '累计活动回退' })).toBeVisible();
    await page.getByRole('region', { name: '累计活动回退' }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/${name}-partial.png` });
    expect(await page.locator('.pd-page').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  });
}

test('partial lifetime data, offline cache, device rename and unsupported relay fallback', async ({ page }) => {
  await seed(page, 'classic', false, true);
  await expect(page.locator('.pd-data-note')).toContainText('部分数据');
  await expect(page.getByRole('region', { name: '累计活动回退' })).toContainText('全部历史，不随上方范围变化');
  await page.getByLabel('统计设备', { exact: true }).selectOption('device-0');
  await expect(page.locator('.pd-kpi').filter({ hasText: '估算费用' })).toContainText('—');
  await expect(page.getByRole('region', { name: '累计活动回退' })).toContainText('315 轮');
  await page.getByRole('region', { name: '累计活动回退' }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${evidence}/partial-lifetime-classic-light.png` });
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
  await page.getByRole('textbox', { name: '设备名称' }).fill('Studio Mac');
  await page.keyboard.press('Escape');
  const discard = page.getByRole('dialog', { name: '放弃名称修改？', exact: true });
  await expect(discard).toBeVisible();
  await discard.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('textbox', { name: '设备名称' })).toHaveValue('Studio Mac');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Studio Mac', exact: true })).toBeVisible();
  await page.route(/\/relay\/devices\/device-[0-2]$/, route => route.fulfill({ status: 405, json: { code: 'unsupported', message: '旧版 Relay' } }));
  await page.getByRole('button', { name: 'Studio Mac 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
  await page.getByRole('textbox', { name: '设备名称' }).fill('Another Mac');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '重命名设备' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Studio Mac 的操作', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: '重命名', exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.goto('/?desktopMockHost=1');
  await expect(page.locator('[data-desktop-surface]')).toBeVisible();
  await expect(page.getByTestId('desktop-devices')).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new Event('pockymoe-account-logout')));
  await expect.poll(() => page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open('pockymoe-desktop-stats'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    return await new Promise<number>(resolve => { const r = db.transaction('accounts').objectStore('accounts').count(); r.onsuccess = () => { resolve(r.result); db.close(); }; });
  })).toBe(0);
});
async function seed(page: Page, pack = 'classic', dark = false, unsupported = false) {
  const devices = ['MacBook Pro', 'Windows Studio', 'Linux Server'].map((name, i) => ({ id: `device-${i}`, ownerUserId: account.id, name, connected: i !== 2, lastHeartbeatAt: new Date(Date.now() - (i === 2 ? 3 * 3600000 : 0)).toISOString() }));
  await page.addInitScript(({ pack, dark }) => {
    localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light'); localStorage.setItem('pockymoe-theme-preset', pack); localStorage.setItem('remote-codex.locale', 'zh-CN');
    const url = new URL(location.href); url.searchParams.set('desktopMockHost', '1'); history.replaceState(null, '', url);
  }, { pack, dark });
  // Other delegates edit the shared tree during QA. Freeze HMR for each page;
  // a fresh case still loads the current source, without reloading mid-dialog.
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:18924\/\?token=/, socket => socket.onMessage(() => undefined));
  // UI-only tests bypass the account-vault gate in the dev module graph. No
  // credentials are provisioned; authentication has its own upstream regressions.
  await page.route('**/src/components/AccountSessionGate.tsx*', route => route.fulfill({ contentType: 'application/javascript', body: 'export function AccountSessionGate({children}) { return children; }' }));
  await page.route('**/api/auth/session', route => route.fulfill({ json: { authenticated: true, authRequired: false, username: 'Alex', mode: 'local' } }));
  await page.route('**/api/config/runtime', route => route.fulfill({ json: { defaultBackend: 'acp' } }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: account } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user: account, devices, sharedWithMe: [], sharedByMe: [] } }));
  await page.route(/\/relay\/devices\/device-[0-2]$/, async route => {
    const device = devices.find(d => route.request().url().endsWith(`/${d.id}`))!;
    if (route.request().method() === 'PATCH') device.name = route.request().postDataJSON().name;
    await route.fulfill({ json: device });
  });
  await page.route('**/relay/devices/*/healthz', route => route.fulfill({ json: { version: '0.13.41', capabilities: { statsSummary: true } } }));
  await page.route('**/relay/devices/*/presence', route => route.fulfill({ json: { connected: !route.request().url().includes('device-2') } }));
  await page.route('**/relay/devices/*/api/**', async route => {
    const url = new URL(route.request().url()); const deviceId = url.pathname.split('/')[3]!;
    const number = Number(deviceId.slice(-1));
    if (url.pathname.endsWith('/transport/key')) { await route.fulfill({ status: 404, json: { code: 'not_found' } }); return; }
    if (url.pathname.endsWith('/stats/summary')) { await route.fulfill(unsupported ? { status: 404, json: { code: 'not_found', message: 'Unsupported' } } : { json: summary(url.searchParams.get('range') as StatsRange, number) }); return; }
    if (url.pathname.endsWith('/management/supervisor')) { await route.fulfill({ json: { runningVersion: '0.13.41', installedVersion: '0.13.41', latestVersion: number === 1 ? '0.13.42' : '0.13.41', canUpdate: true } }); return; }
    if (url.pathname.endsWith('/management/harnesses')) { await route.fulfill({ json: [] }); return; }
    if (url.pathname.endsWith('/management/templates')) { await route.fulfill({ json: { harnesses: [], profiles: [] } }); return; }
    if (url.pathname.endsWith('/api/workspaces')) { await route.fulfill({ json: [{ id: 'w-0', hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null }] }); return; }
    if (url.pathname.endsWith('/device/metrics')) { await route.fulfill({ json: hardware(number) }); return; }
    if (url.pathname.endsWith('/thread-activity')) { await route.fulfill({ json: { since: new Date(Date.now() - 86400000).toISOString(), hours: 24, threads: Array.from({ length: 6 }, (_, i) => ({ threadId: `t-${i}`, turns: 50 + i, activeMs: 60000, totalMs: 900000, tokenUsage: { totalTokens: 300, inputTokens: 200, cachedInputTokens: 50, outputTokens: 100, reasoningOutputTokens: 0 }, pricePartial: true })) } }); return; }
    const threads = Array.from({ length: 6 }, (_, i) => ({ id: `t-${i}`, workspaceId: 'w-0', title: `设备统计任务 ${i}`, provider: 'acp', agentId: 'codex', providerSessionId: null, source: 'supervisor', model: 'gpt-6.1', reasoningEffort: 'high', collaborationMode: 'default', approvalMode: 'yolo', isLoaded: true, summaryText: '', lastError: null, status: i < 3 ? 'running' : 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), lastTurnStartedAt: new Date(Date.now() - 192000).toISOString(), lastTurnCompletedAt: i >= 3 ? new Date().toISOString() : null, activeTurnId: i < 3 ? `turn-${i}` : null }));
    if (url.pathname.endsWith('/api/threads')) { await route.fulfill({ json: threads }); return; }
    const id = url.pathname.match(/\/threads\/(t-\d+)/)?.[1];
    if (id) { await route.fulfill({ json: { thread: threads.find(t => t.id === id), turns: [], pendingRequests: id === 't-3' ? [{ id: 'req', kind: 'requestUserInput', title: '需要选择', description: '选择方式' }] : [], pendingSteers: [] } }); return; }
    await route.fulfill({ json: {} });
  });
  await page.route('**/api/plugins*', route => route.fulfill({ json: [] }));
  await page.route('**/relay/devices/*/api/transport/key*', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, socket => socket.onMessage(() => undefined));
  // Offline devices must restore the same account-scoped IndexedDB snapshot.
  await page.addInitScript(async ({ snapshot }) => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => { const r = indexedDB.open('pockymoe-desktop-stats', 1); r.onupgradeneeded = () => r.result.createObjectStore('accounts'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    const tx = db.transaction('accounts', 'readwrite');
    tx.objectStore('accounts').put({ 'device-2': { version: '0.13.40', summaries: snapshot } }, `${location.origin}:${location.origin}:devices-test:${Intl.DateTimeFormat().resolvedOptions().timeZone}`);
    tx.oncomplete = () => db.close();
  }, { snapshot: Object.fromEntries((['today', '7d', '30d'] as const).map(range => { const s = summary(range, 2); s.generatedAt = new Date(Date.now() - 3 * 3600000).toISOString(); return [range, s]; })) });
  await page.goto('/desktop/devices?desktopMockHost=1');
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await expect(page.getByRole('article', { name: 'Windows Studio', exact: true }).getByText('有更新 0.13.42')).toBeVisible();
}

