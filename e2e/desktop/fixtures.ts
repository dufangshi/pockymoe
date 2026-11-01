import { expect, type Page } from '@playwright/test';
import type { StatsRange, StatsSummary } from '../../apps/supervisor-web/src/desktop/devices/model';
import type { DeviceMetricsDto } from '@pockymoe/shared';

const account = { id: 'devices-test', username: 'Alex', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
export function hardware(device: number): DeviceMetricsDto {
  const sampledAt = new Date().toISOString(), totalBytes = 32 * 2 ** 30, usedBytes = 12.4 * 2 ** 30;
  const usagePercent = 32 + device * 12;
  return { sampledAt, sampleWindowMs: 2000, platform: device ? 'windows' : 'darwin', environment: 'native', cpu: { model: device ? 'AMD Ryzen' : 'Apple M4 Pro', usagePercent, logicalCoreCount: 4, cores: [0, 1, 2, 3].map(index => ({ index, usagePercent: usagePercent + index })) }, memory: { totalBytes, usedBytes, availableBytes: totalBytes - usedBytes, usagePercent: usedBytes / totalBytes * 100 }, swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null }, cpuPower: { watts: 24.5, source: 'CPU package', reason: null }, cpuTemperature: { celsius: 64, source: 'native', reason: null, sensors: [] }, gpus: [{ id: 'gpu', name: device ? 'NVIDIA RTX 4080' : 'Apple M4 Pro GPU', usagePercent: 18, usedMemoryBytes: 2 * 2 ** 30, totalMemoryBytes: 16 * 2 ** 30, power: { watts: null, source: null, reason: '未提供 GPU 功耗' }, source: 'native' }], hardwareSampledAt: sampledAt, hardwareNotes: [], limits: null };
}
export function summary(range: StatsRange, device = 0): StatsSummary {
  const count = range === 'today' ? 1 : range === '7d' ? 7 : 30;
  const today = new Date();
  const dates = Array.from({ length: count }, (_, i) => { const day = new Date(today); day.setDate(day.getDate() - count + i + 1); return day.toLocaleDateString('en-CA'); });
  const daily = dates.map((date, i) => ({ date, turns: [8, 13, 6, 21, 34, 28, 37][i % 7]! + device * 3, failed: 1, tokens: 64000 + i * 16000, costUsd: .12 + i * .08 }));
  const turns = daily.reduce((n, d) => n + d.turns, 0);
  const lastHour = Number(new Intl.DateTimeFormat('en-GB', { timeZone: 'America/Toronto', hour: '2-digit', hourCycle: 'h23' }).format(today));
  const weights = Array.from({ length: lastHour + 1 }, (_, hour) => 1 + hour % 3 + device);
  const weight = weights.reduce((sum, value) => sum + value, 0);
  const allocated = (value: number, hour: number) => hour > lastHour ? 0 : hour < lastHour ? Math.floor(value * weights[hour]! / weight) : value - weights.slice(0, -1).reduce((sum, w) => sum + Math.floor(value * w / weight), 0);
  const hourly = range === 'today' ? Array.from({ length: 24 }, (_, hour) => ({ hour, turns: allocated(turns, hour), failed: hour === lastHour ? 1 : 0, tokens: allocated(daily[0]!.tokens, hour), costUsd: hour > lastHour ? 0 : daily[0]!.costUsd * weights[hour]! / weight })) : undefined;
  return { range, generatedAt: new Date().toISOString(), totals: { turns, completed: turns - count, failed: count, interrupted: 0, avgDurationMs: 192000, p50DurationMs: 120000, durationSamples: turns, inputTokens: daily.reduce((n, d) => n + d.tokens, 0) - count * 16000, outputTokens: count * 16000, cachedTokens: count * 20000, costUsd: daily.reduce((n, d) => n + d.costUsd, 0) }, daily, ...(hourly ? { hourly } : {}),
    byModel: [{ model: 'GPT-6.1', turns: Math.round(turns * .65), tokens: 40000, costUsd: .12 }, { model: 'Claude Sonnet 4.6', turns: turns - Math.round(turns * .65), tokens: 24000, costUsd: .08 }],
    byWorkspace: ['Pockymoe', 'Desktop App', 'Research', 'Design system', 'Automation'].map((label, i) => ({ workspaceId: `w-${i}`, label, turns: Math.max(1, Math.round(turns / (i + 2))), tokens: 10000, costUsd: .1, lastActivityAt: today.toISOString() })),
    longestThreads: ['Build device overview and statistics', 'Polish desktop navigation', 'Review transport security'].map((title, i) => ({ threadId: `t-${i}`, workspaceId: 'w-0', title, durationMs: 240000 - i * 60000 })), activeThreads: 3, runningTurns: 3 };
}

export async function seed(page: Page, pack = 'classic', dark = false, unsupported = false) {
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
    if (url.pathname.endsWith('/thread-activity')) { await route.fulfill({ json: { since: new Date(Date.now() - 86400000).toISOString(), hours: 24, threads: Array.from({ length: 6 }, (_, i) => ({ threadId: `t-${i}`, turns: 50 + i, activeMs: 3_600_000, totalMs: 7_200_000, hourlyMs: Array.from({ length: 24 }, (_, h) => (h % 4 + 1) * 60000), tokenUsage: { totalTokens: 300, inputTokens: 200, cachedInputTokens: 50, outputTokens: 100, reasoningOutputTokens: 0 }, pricePartial: true })) } }); return; }
    const threads = Array.from({ length: 6 }, (_, i) => ({ id: `t-${i}`, workspaceId: 'w-0', title: `Device statistics task ${i}`, provider: 'acp', agentId: 'codex', providerSessionId: null, source: 'supervisor', model: 'gpt-6.1', reasoningEffort: 'high', collaborationMode: 'default', approvalMode: 'yolo', isLoaded: true, summaryText: '', lastError: null, status: i < 3 ? 'running' : 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), lastTurnStartedAt: new Date(Date.now() - 192000).toISOString(), lastTurnCompletedAt: i >= 3 ? new Date().toISOString() : null, activeTurnId: i < 3 ? `turn-${i}` : null }));
    if (url.pathname.endsWith('/api/threads')) { await route.fulfill({ json: threads }); return; }
    const id = url.pathname.match(/\/threads\/(t-\d+)/)?.[1];
    if (id) { await route.fulfill({ json: { thread: threads.find(t => t.id === id), turns: [], pendingRequests: id === 't-3' ? [{ id: 'req', kind: 'requestUserInput', title: 'Choose an option', description: 'Choose how to proceed' }] : [], pendingSteers: [] } }); return; }
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

