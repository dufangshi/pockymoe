import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { expect, test } from '@playwright/test';
import type { ActivitySnapshot, HostCommand } from '../host';
// Credentials stay in local test memory. No auth trace, screenshot, storage-state
// export, response body or credential value is recorded in evidence.
const config = JSON.parse(readFileSync('/Users/mac/dev/remote-codex-app/.local/e2e-desktop-env.json', 'utf8')) as {
  webOrigin: string; relayOrigin: string; username: string; password: string;
  devices: { index: number; id: string; scenarios: Record<string, string> }[];
};
const evidenceDirectory = process.env.DESKTOP_EVIDENCE_DIR ?? '/tmp/rc-desktop/evidence/webnav/v2';
const control = (action: string, device: number, id: string) => execFileSync('/Users/mac/dev/remote-codex-app/scripts/e2e-desktop-backend.sh', [action, String(device), id], { stdio: 'ignore' });
test('real certified account across separate Web/API origins keeps map mounted and receives encrypted events', async ({ page, context }) => {
  await context.grantPermissions(['local-network-access'], { origin: config.webOrigin });
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('remote-codex-theme-mode', 'light');
    const state = { snapshot: null as ActivitySnapshot | null, command: null as ((command: HostCommand) => void) | null, results: [] as unknown[] };
    Object.assign(window, { desktopRealState: state });
    window.pockymoeHost = {
      capabilities: async () => ({ surface: 'desktop', bridgeVersion: 2, platform: 'darwin', shellVersion: 'real-fixture', notifications: 'local', windowChrome: { kind: 'hiddenInset', trafficLightsWidth: 78, toolbarHeight: 38, controlsSide: 'left' }, features: { pet: true, nativeSave: true, badge: true, multiWindow: true, quickWindow: false } }),
      activity: { sync: snapshot => { state.snapshot = snapshot; } }, appearance: { sync: () => undefined },
      onCommand: listener => { state.command = listener; return () => { state.command = null; }; }, commandResult: (id, result) => state.results.push({ id, ...result }),
      window: { setTitle: () => undefined, openThread: () => undefined, openPreferences: () => undefined },
      shell: { openExternal: () => undefined }, files: { saveAs: async () => ({ saved: false }) }, badge: { set: () => undefined },
      pet: { setVisible: () => undefined, isVisible: async () => false }, settings: { get: async () => ({} as never), set: async () => ({} as never) },
    };
  });
  const sockets = new Set<string>(), encryptedSockets = new Set<string>(); let encryptedReads = 0;
  page.on('request', request => { if (new URL(request.url()).origin === config.relayOrigin && request.headers()['x-rcd-key']) encryptedReads++; });
  page.on('websocket', socket => { const url = new URL(socket.url()), id = url.pathname.match(/\/relay\/devices\/([^/]+)\/ws/)?.[1]; if (id) { sockets.add(id); if (url.searchParams.has('channelId')) encryptedSockets.add(id); } });
  try {
    await page.goto(`${config.webOrigin}/relay-portal?returnTo=%2F`);
    await page.getByLabel('Email or username', { exact: true }).fill(config.username);
    await page.getByLabel('Password', { exact: true }).fill(config.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.locator('.thread-map[data-presentation="page"]')).toBeVisible();
  } catch {
    await page.goto('about:blank').catch(() => undefined);
    throw new Error('Fixture sign-in or map initialization failed; authentication details were not retained.');
  }
  const snapshot = () => page.evaluate(() => (window as any).desktopRealState.snapshot as ActivitySnapshot | null);
  await expect.poll(() => sockets.size).toBe(3);
  await expect.poll(async () => (await snapshot())?.items.filter(item => item.state === 'needsInput').length).toBeGreaterThan(0);
  expect(encryptedSockets.size).toBe(3); expect(encryptedReads).toBeGreaterThan(0);
  const device = config.devices[0]!, threadId = device.scenarios.events!;
  control('start-turn', device.index, threadId);
  await expect.poll(async () => (await snapshot())?.items.find(item => item.key === `${device.id}:${threadId}`)?.state).toBe('running');
  control('request-approval', device.index, threadId);
  await expect.poll(async () => (await snapshot())?.items.find(item => item.key === `${device.id}:${threadId}`)?.request?.kind).toBe('approval');
  const requested = (await snapshot())!.items.find(item => item.key === `${device.id}:${threadId}`)!;
  const card = page.locator('.thread-map-attention-card[data-kind="approval"]').filter({ hasText: requested.title }).filter({ hasText: requested.deviceName });
  await expect(card.getByRole('button', { name: 'Approve', exact: true })).toBeVisible();
  await page.screenshot({ path: `${evidenceDirectory}/real-approval-before.png` });
  await card.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect.poll(async () => (await snapshot())?.items.find(item => item.key === requested.key)?.request?.id).not.toBe(requested.request!.id);
  await expect(card).toHaveCount(0);
  // Verify against a second uncached device read, not just the optimistic index:
  // a stale action must now be rejected because it no longer exists on device.
  await page.evaluate(command => (window as any).desktopRealState.command(command), { id: 'verify-cleared', type: 'resolveRequest', deviceId: device.id, threadId, requestId: requested.request!.id, decision: 'approve' });
  await expect.poll(() => page.evaluate(() => (window as any).desktopRealState.results.find((result: any) => result.id === 'verify-cleared'))).toMatchObject({ ok: false, error: 'This request has already been handled.' });
  control('complete', device.index, threadId);
  await expect.poll(async () => (await snapshot())?.items.find(item => item.key === `${device.id}:${threadId}`)?.state).toBe('completed');
  await page.evaluate(key => (window as any).desktopRealState.command({ id: 'reveal', type: 'revealInMap', key }), `${device.id}:${threadId}`);
  const target = page.locator(`[data-map-id="thread:${device.id}:${threadId}"]`); await expect(target).toBeVisible();
  await expect.poll(() => page.locator('.desktop-map-scene .thread-map-crumbs button').count()).toBeGreaterThan(1);
  const crumbs = await page.locator('.desktop-map-scene .thread-map-crumbs').textContent();
  expect(crumbs).toBeTruthy();
  await page.evaluate(() => Object.assign(window, { savedDesktopMap: document.querySelector('.desktop-map-scene .thread-map') }));
  await page.screenshot({ path: `${evidenceDirectory}/real-map.png` });
  await page.evaluate(path => (window as any).desktopRealState.command({ id: 'open', type: 'navigate', path }), `/devices/${device.id}/threads/${threadId}`);
  await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  const toolbar = await page.locator('.desktop-route .matter-tabs-row').boundingBox();
  expect(toolbar?.x).toBe(0); expect(toolbar?.width).toBe(1440);
  await page.screenshot({ path: `${evidenceDirectory}/real-thread.png` });
  await page.evaluate(() => (window as any).desktopRealState.command({ id: 'back', type: 'openMap' }));
  await expect(page.locator('.desktop-map-scene .thread-map')).toBeVisible();
  expect(await page.evaluate(() => (window as any).savedDesktopMap === document.querySelector('.desktop-map-scene .thread-map'))).toBe(true);
  await expect(page.locator('.desktop-map-scene .thread-map-crumbs')).toHaveText(crumbs!);
  const evidence = { devices: sockets.size, encryptedSockets: encryptedSockets.size, encryptedReads, pendingRequests: (await snapshot())?.items.filter(item => item.state === 'needsInput').length, mapPreserved: true, approvalClearedOnDevice: true };
  writeFileSync(`${evidenceDirectory}/real-relay.json`, JSON.stringify(evidence, null, 2));
});
