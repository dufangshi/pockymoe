import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { expect, test } from '@playwright/test';

// Disposable fixture credentials are consumed directly, never printed or traced.
const fixture = JSON.parse(readFileSync('/Users/mac/dev/remote-codex-app/.local/e2e-desktop-env.json', 'utf8')) as {
  relayOrigin: string; username: string; password: string; accountId: string;
  devices: { id: string; index: number; name: string; scenarios: Record<string, string> }[];
};
test.use({ baseURL: fixture.relayOrigin, trace: 'off' });
const script = '/Users/mac/dev/remote-codex-app/scripts/e2e-desktop-backend.sh';
const control = (type: string, device: number, threadId: string) => execFileSync(script, [type, String(device), threadId], { stdio: 'ignore' });

test('real three-device relay index, cold requests, encrypted WS, thread surface and warm cache', async ({ page, context }) => {
  test.setTimeout(60_000);
  const auth = await context.request.post(`${fixture.relayOrigin}/relay/auth/login`, { data: { identifier: fixture.username, password: fixture.password } });
  expect(auth.ok()).toBe(true);
  await page.addInitScript(() => {
    const state = { snapshots: [] as unknown[], results: [] as unknown[], commands: null as ((command: unknown) => void) | null };
    Object.assign(window, { desktopFixtureState: state });
    localStorage.setItem('remote-codex-theme-mode', 'light');
    window.remoteCodexHost = {
      capabilities: async () => ({ surface: 'desktop', bridgeVersion: 1, platform: 'darwin', shellVersion: 'fixture', notifications: 'local', windowChrome: { kind: 'hiddenInset', trafficLightsWidth: 78, toolbarHeight: 38, controlsSide: 'left' }, features: { pet: true, multiWindow: true, nativeSave: true, badge: true, quickWindow: false } }),
      activity: { sync: snapshot => { state.snapshots.push(snapshot); } },
      onCommand: listener => { state.commands = listener as (command: unknown) => void; return () => { state.commands = null; }; },
      commandResult: (id, result) => { state.results.push({ id, ...result }); },
      window: { setTitle: () => undefined, openThread: () => undefined },
      shell: { openExternal: () => undefined }, files: { saveAs: async () => ({ saved: false }) },
      badge: { set: () => undefined }, pet: { setVisible: () => undefined, isVisible: async () => false },
      settings: { get: async () => ({} as never), set: async () => ({} as never) },
    };
  });
  const sockets = new Set<string>();
  page.on('websocket', socket => { const match = socket.url().match(/\/relay\/devices\/([^/]+)\/ws/); if (match) sockets.add(match[1]!); });
  await page.goto('/desktop');
  await expect(page.locator('[data-desktop-surface]')).toBeVisible();
  await expect.poll(() => sockets.size, { timeout: 20_000 }).toBe(3);
  const snapshot = () => page.evaluate(() => (window as any).desktopFixtureState.snapshots.at(-1));
  await expect.poll(async () => (await snapshot())?.items.filter((item: any) => item.state === 'needsInput').length, { timeout: 20_000 }).toBeGreaterThanOrEqual(6);
  const current = await snapshot();
  expect(new Set(current.items.map((item: any) => item.deviceId)).size).toBe(3);
  expect(current.items.every((item: any) => (item.request?.summary.length ?? 0) <= 140)).toBe(true);
  await page.screenshot({ path: '/tmp/rc-desktop/evidence/webnav/real-three-devices-light.png' });
  const target = fixture.devices[0]!;
  const eventThread = target.scenarios.events!;
  control('start-turn', target.index, eventThread);
  await expect.poll(async () => (await snapshot())?.items.find((item: any) => item.key === `${target.id}:${eventThread}`)?.state).toBe('running');
  control('complete', target.index, eventThread);
  await expect.poll(async () => (await snapshot())?.items.find((item: any) => item.key === `${target.id}:${eventThread}`)?.state).toBe('completed');
  await page.evaluate(({ deviceId, threadId }) => (window as any).desktopFixtureState.commands({ id: 'open-live', type: 'navigate', path: `/devices/${deviceId}/threads/${threadId}` }), { deviceId: target.id, threadId: eventThread });
  await expect(page.locator('[data-testid="thread-scroll-container"]')).toBeVisible();
  await expect(page.locator('.rc-main .matter-sidebar')).toBeHidden();
  await expect(page.locator('.rc-main .matter-topbar')).toBeHidden();
  const layout = await page.evaluate(() => Object.fromEntries(['.rc-main', '.thread-ui-shell', '.matter-workbench', '.matter-chat', '.thread-graph-composer-send-button'].map(selector => { const rect = document.querySelector(selector)?.getBoundingClientRect(); return [selector, rect ? { top: rect.top, bottom: rect.bottom, height: rect.height } : null]; })));
  writeFileSync('/tmp/rc-desktop/evidence/webnav/thread-layout.json', JSON.stringify(layout, null, 2));
  await test.info().attach('thread-layout', { body: JSON.stringify(layout), contentType: 'application/json' });
  expect(layout['.thread-graph-composer-send-button']?.bottom).toBeLessThanOrEqual(850);
  await page.screenshot({ path: '/tmp/rc-desktop/evidence/webnav/real-thread-light.png' });
  await page.getByRole('button', { name: '切换检查器' }).click();
  await expect(page.locator('.matter-explorer')).toBeVisible();
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('dialog', { name: '命令面板' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await snapshot())?.items.some((item: any) => item.key === `${target.id}:${eventThread}`)).toBe(false);
  await page.reload();
  await expect(page.locator('[data-desktop-surface]')).toBeVisible();
  await expect(page.locator('[data-testid="desktop-skeleton"]')).toHaveCount(0);
  const timing = await page.evaluate(() => performance.getEntriesByName('rc-desktop-tree-paint').at(-1)?.duration);
  expect(timing).toBeLessThan(150);
  writeFileSync('/tmp/rc-desktop/evidence/webnav/real-relay-evidence.json', JSON.stringify({ devices: sockets.size, attention: current.items.filter((item: any) => item.state === 'needsInput').length, warmTreeMs: timing }, null, 2));
  await test.info().attach('real-relay-evidence', { body: JSON.stringify({ devices: sockets.size, attention: current.items.filter((item: any) => item.state === 'needsInput').length, warmTreeMs: timing }), contentType: 'application/json' });
});
