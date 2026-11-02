import { expect, type Page } from '@playwright/test';
import type { StatsRange, StatsSummary } from '../../apps/supervisor-web/src/desktop/devices/model';
import type { DeviceMetricsDto } from '@pockymoe/shared';

const account = { id: 'devices-test', username: 'Alex', role: 'user', enabled: true, createdAt: '2026-10-01T00:00:00Z' };
export function hardware(device: number): DeviceMetricsDto {
  const sampledAt = new Date().toISOString(), totalBytes = 32 * 2 ** 30, usedBytes = 12.4 * 2 ** 30;
  const usagePercent = Math.min(98, 32 + device * 6);
  return { sampledAt, sampleWindowMs: 2000, platform: device ? 'windows' : 'darwin', environment: 'native', cpu: { model: device ? 'AMD Ryzen' : 'Apple M4 Pro', usagePercent, logicalCoreCount: 4, cores: [0, 1, 2, 3].map(index => ({ index, usagePercent: usagePercent + index })) }, memory: { totalBytes, usedBytes, availableBytes: totalBytes - usedBytes, usagePercent: usedBytes / totalBytes * 100 }, swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null }, cpuPower: { watts: 24.5, source: 'CPU package', reason: null }, cpuTemperature: { celsius: 64, source: 'native', reason: null, sensors: [] }, gpus: [{ id: 'gpu', name: device ? 'NVIDIA RTX 4080' : 'Apple M4 Pro GPU', usagePercent: 18, usedMemoryBytes: 2 * 2 ** 30, totalMemoryBytes: 16 * 2 ** 30, power: { watts: null, source: null, reason: '未提供 GPU 功耗' }, source: 'native' }], hardwareSampledAt: sampledAt, hardwareNotes: [], limits: null };
}
export function summary(range: StatsRange, device = 0): StatsSummary {
  const count = range === 'today' ? 1 : range === '7d' ? 7 : range === 'all' ? 60 : 30;
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

export async function seed(page: Page, pack = 'classic', dark = false, unsupported = false, realShape = false, host = true, usageShape = false) {
  const devices = (usageShape ? ['MacBook Pro', 'Windows Studio', 'Linux Server', ...Array.from({ length: 9 }, (_, i) => `Usage device ${i + 3}`)] : realShape ? ['MacBook Pro', 'Windows Studio', 'Linux Server', 'wsl', 'LuisMini2', 'Build Box', 'Studio Linux', 'Mac mini', 'Office Windows', 'Archive', 'Empty device'] : ['MacBook Pro', 'Windows Studio', 'Linux Server']).map((name, i) => ({ id: `device-${i}`, ownerUserId: account.id, name, connected: i !== 2 && i !== 9, lastHeartbeatAt: new Date(Date.now() - (i === 2 ? 3 * 3600000 : 0)).toISOString() }));
  await page.addInitScript(({ pack, dark, host }) => {
    localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light'); localStorage.setItem('pockymoe-theme-preset', pack); localStorage.setItem('remote-codex.locale', 'zh-CN');
    if (host) { const url = new URL(location.href); url.searchParams.set('desktopMockHost', '1'); history.replaceState(null, '', url); }
  }, { pack, dark, host });
  // Other delegates edit the shared tree during QA. Freeze HMR for each page;
  // a fresh case still loads the current source, without reloading mid-dialog.
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:\d+\/\?token=/, socket => socket.onMessage(() => undefined));
  // UI-only tests bypass the account-vault gate in the dev module graph. No
  // credentials are provisioned; authentication has its own upstream regressions.
  await page.route('**/src/components/AccountSessionGate.tsx*', route => route.fulfill({ contentType: 'application/javascript', body: 'export function AccountSessionGate({children}) { return children; }' }));
  await page.route('**/api/auth/session', route => route.fulfill({ json: { authenticated: true, authRequired: false, username: 'Alex', mode: 'local' } }));
  await page.route('**/api/config/runtime', route => route.fulfill({ json: { defaultBackend: 'acp' } }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: account } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user: account, devices, sharedWithMe: [], sharedByMe: [] } }));
  await page.route(/\/relay\/devices\/device-\d+$/, async route => {
    const device = devices.find(d => route.request().url().endsWith(`/${d.id}`))!;
    if (route.request().method() === 'PATCH') device.name = route.request().postDataJSON().name;
    await route.fulfill({ json: device });
  });
  await page.route('**/relay/devices/*/healthz', route => route.fulfill({ json: { version: realShape && /device-[3-9]\//.test(route.request().url()) ? '0.12.73' : '0.13.41', capabilities: { statsSummary: !(realShape && /device-[3-9]\//.test(route.request().url())) } } }));
  await page.route('**/relay/devices/*/presence', route => route.fulfill({ json: { connected: devices.find(device => route.request().url().includes(`/${device.id}/`))?.connected ?? false } }));
  await page.route('**/relay/devices/*/api/**', async route => {
    const url = new URL(route.request().url()); const deviceId = url.pathname.split('/')[3]!;
    const number = Number(deviceId.split('-').at(-1));
    if (url.pathname.endsWith('/transport/key')) { await route.fulfill({ status: 404, json: { code: 'not_found' } }); return; }
    if (url.pathname.endsWith('/stats/summary')) { const data = realShape ? accountSummary(url.searchParams.get('range') as StatsRange, number) : summary(url.searchParams.get('range') as StatsRange, number); if (usageShape) usageStatsShape(data, number); const origin = url.searchParams.get('origin'); if (origin && data.byOrigin) { const row = data.byOrigin.find(row => row.origin === origin)!; data.origin = row.origin; data.byModel = data.byModel.map(model => ({ ...model, turns: Math.round(model.turns * .1), tokens: model.tokens * .1, costUsd: model.costUsd! * .1 })); data.byWorkspace = data.byWorkspace.map(workspace => ({ ...workspace, tokens: workspace.tokens * .1, costUsd: workspace.costUsd! * .1 })); data.totals = { ...data.totals, turns: row.turns, inputTokens: row.tokens * .8, outputTokens: row.tokens * .2, costUsd: row.costUsd }; } await route.fulfill(unsupported || realShape && number >= 3 && number <= 9 ? { status: 404, json: { code: 'not_found', message: 'Unsupported' } } : { json: data }); return; }
    if (url.pathname.endsWith('/management/supervisor')) { await route.fulfill({ json: { runningVersion: realShape && number >= 3 && number <= 9 ? '0.12.73' : '0.13.41', installedVersion: '0.13.41', latestVersion: number === 1 || realShape && number >= 3 && number <= 9 ? '0.13.42' : '0.13.41', canUpdate: true } }); return; }
    if (url.pathname.endsWith('/management/harnesses')) { await route.fulfill({ json: [] }); return; }
    if (url.pathname.endsWith('/management/templates')) { await route.fulfill({ json: { harnesses: [], profiles: [] } }); return; }
    if (url.pathname.endsWith('/api/workspaces')) { await route.fulfill({ json: Array.from({ length: realShape && number === 3 ? 21 : 1 }, (_, i) => ({ id: `w-${i}`, hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null })) }); return; }
    if (url.pathname.endsWith('/device/metrics')) { await route.fulfill({ json: hardware(number) }); return; }
    if (url.pathname.endsWith('/thread-activity')) { await route.fulfill({ json: { since: new Date(Date.now() - 86400000).toISOString(), hours: 24, threads: Array.from({ length: 6 }, (_, i) => ({ threadId: `t-${i}`, turns: 50 + i, activeMs: 3_600_000, totalMs: 7_200_000, hourlyMs: Array.from({ length: 24 }, (_, h) => (h % 4 + 1) * 60000), tokenUsage: { totalTokens: 300, inputTokens: 200, cachedInputTokens: 50, outputTokens: 100, reasoningOutputTokens: 0 }, pricePartial: true })) } }); return; }
    const threads = Array.from({ length: usageShape || realShape && number === 10 ? 0 : 6 }, (_, i) => ({ id: `t-${i}`, workspaceId: 'w-0', title: `Device statistics task ${i}`, provider: 'acp', agentId: 'codex', providerSessionId: null, source: 'supervisor', model: 'gpt-6.1', reasoningEffort: 'high', collaborationMode: 'default', approvalMode: 'yolo', isLoaded: true, summaryText: '', lastError: null, status: i < 3 ? 'running' : 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), lastTurnStartedAt: new Date(Date.now() - 192000).toISOString(), lastTurnCompletedAt: i >= 3 ? new Date().toISOString() : null, activeTurnId: i < 3 ? `turn-${i}` : null }));
    if (url.pathname.endsWith('/api/threads')) { await route.fulfill({ json: threads }); return; }
    const id = url.pathname.match(/\/threads\/(t-\d+)/)?.[1];
    if (id) { await route.fulfill({ json: { thread: threads.find(t => t.id === id), turns: [], pendingRequests: !usageShape && id === 't-3' ? [{ id: 'req', kind: 'requestUserInput', title: 'Choose an option', description: 'Choose how to proceed' }] : [], pendingSteers: [] } }); return; }
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
  }, { snapshot: Object.fromEntries((['today', '7d', '30d', 'all'] as const).map(range => { const s = realShape ? accountSummary(range, 2) : summary(range, 2); if (usageShape) usageStatsShape(s, 2); s.generatedAt = new Date(Date.now() - 3 * 3600000).toISOString(); return [range, s]; })) });
  await page.goto(host ? '/desktop/devices?desktopMockHost=1' : '/desktop/devices');
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  if (!usageShape) await expect(page.getByRole('article', { name: 'Windows Studio', exact: true }).getByRole('button', { name: '有更新 0.13.42', exact: true })).toBeVisible();
}


function accountSummary(range: StatsRange, device: number): StatsSummary {
  const result = summary(range, device);
  const scale = range === 'all' ? 40 : range === '30d' ? 20 : range === '7d' ? 5 : 1;
  const tokens = device === 10 ? 0 : (device + 1) * 1800000 * scale;
  const cost = device === 10 ? 0 : (device === 1 ? 154 : 17) * scale;
  result.totals.inputTokens = tokens * .8; result.totals.outputTokens = tokens * .2;
  result.totals.costUsd = cost; result.totals.unpricedTokens = device === 0 ? tokens * .1 : 0;
  result.byModel = [
    { model: 'GPT-6.1', turns: Math.round(result.totals.turns * .7), tokens: tokens * .7, costUsd: cost * .2, inputTokens: tokens * .5, outputTokens: tokens * .2, cachedTokens: tokens * .3, cacheWriteTokens: 0, reasoningTokens: tokens * .1, unpricedTokens: 0 },
    { model: 'Claude Sonnet 4.6', turns: result.totals.turns - Math.round(result.totals.turns * .7), tokens: tokens * .3, costUsd: cost * .8, inputTokens: tokens * .25, outputTokens: tokens * .05, cachedTokens: tokens * .1, cacheWriteTokens: tokens * .02, reasoningTokens: tokens * .01, unpricedTokens: device === 0 ? tokens * .1 : 0 },
  ];
  result.byWorkspace = result.byWorkspace.map((row, i) => ({ ...row, tokens: tokens * (5 - i) / 15, costUsd: cost * (i + 1) / 15 }));
  result.byOrigin = (['owner', 'guest', 'agent', 'automation'] as const).map((origin, i) => ({ origin, turns: Math.round(result.totals.turns * [ .5, .1, .3, .1 ][i]!), tokens: tokens * [ .5, .1, .3, .1 ][i]!, costUsd: cost * [ .5, .1, .3, .1 ][i]! }));
  if (device === 10) { result.totals.turns = 0; result.totals.completed = 0; result.totals.failed = 0; result.byModel = []; result.byWorkspace = []; result.daily = result.daily.map(row => ({ ...row, turns: 0, failed: 0, tokens: 0, costUsd: 0 })); if (result.hourly) result.hourly = result.hourly.map(row => ({ ...row, turns: 0, failed: 0, tokens: 0, costUsd: 0 })); }
  return result;
}

function usageStatsShape(data: StatsSummary, device: number) {
  const names = ['["remote-codex","grok-4.7"]', 'grok-4.7', '<synthetic>', 'gpt-6.1-sol',
    'provider-a-very-long-model-name-with-specialized-reasoning-and-version-2026-11-02',
    ...Array.from({ length: 8 }, (_, i) => `Model ${i + 5}`)];
  const selected = device === 0 ? names.slice(0, 5) : [names[device % names.length]!, names[(device + 2) % names.length]!];
  const scale = data.daily.length;
  data.byModel = selected.map((model, i) => ({ model, turns: (i + 1) * scale, tokens: (120000 + i * 2000) * scale, inputTokens: 100000 * scale, outputTokens: (20000 + i * 2000) * scale, costUsd: i === 1 ? null : (.2 + i) * scale, unpricedTokens: i === 1 ? 122000 * scale : 0 }));
  const empty = device === 11;
  const tokens = empty ? 0 : data.byModel.reduce((sum, row) => sum + row.tokens, 0);
  const cost = empty ? 0 : data.byModel.reduce((sum, row) => sum + (row.costUsd ?? 0), 0);
  const unpriced = empty ? 0 : data.byModel.reduce((sum, row) => sum + (row.unpricedTokens ?? 0), 0);
  data.totals.tokens = tokens; data.totals.costUsd = cost; data.totals.unpricedTokens = unpriced;
  data.totals.inputTokens = empty ? 0 : data.byModel.reduce((sum, row) => sum + (row.inputTokens ?? 0), 0);
  data.totals.outputTokens = tokens - data.totals.inputTokens;
  const distribute = (rows: Array<{ tokens: number; costUsd: number | null; unpricedTokens?: number }>) => {
    const weight = rows.reduce((sum, row) => sum + row.tokens, 0);
    const last = rows.findLastIndex(row => row.tokens > 0);
    let assigned = 0;
    rows.forEach((row, i) => {
      const share = weight ? row.tokens / weight : 0;
      row.tokens = i === last ? tokens - assigned : Math.floor(tokens * share); assigned += row.tokens;
      row.costUsd = cost * share; row.unpricedTokens = Math.round(unpriced * share);
    });
  };
  distribute(data.daily);
  if (data.hourly) distribute(data.hourly);
  data.byWorkspace = data.byWorkspace.map((row, i) => ({ ...row, tokens: Math.round(tokens * (5 - i) / 15), costUsd: cost * (5 - i) / 15, unpricedTokens: Math.round(unpriced * (5 - i) / 15) }));
  if (device === 1 || empty) delete data.hourly;
  if (empty) {
    Object.assign(data.totals, { turns: 0, completed: 0, failed: 0, interrupted: 0, cachedTokens: 0, durationSamples: 0, avgDurationMs: null, p50DurationMs: null });
    data.daily.forEach(row => { row.turns = 0; row.failed = 0; });
    data.byModel = []; data.byWorkspace = []; data.activeThreads = 0; data.runningTurns = 0;
  }
}
