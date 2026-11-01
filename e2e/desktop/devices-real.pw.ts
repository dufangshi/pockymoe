import { expect, test } from '@playwright/test';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import type { ActivitySnapshot, HostCommand } from '../../apps/supervisor-web/src/desktop/host';
const appRepo = process.env.POCKYMOE_APP_REPO ?? '../pockymoe-app';
const config = JSON.parse(readFileSync(`${appRepo}/.local/e2e-desktop-env.json`, 'utf8')) as { webOrigin: string; relayOrigin: string; username: string; password: string; devices: { id: string }[] };
const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop/devices-real';
mkdirSync(evidence, { recursive: true });
// Real certified account and encrypted cross-origin transport. No auth traces,
// storage state, recovery codes, credential screenshots or HTTP bodies are saved.
test('certified account usage through the real three-device relay fixture', async ({ page, context }) => {
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

  let encryptedReads = 0;
  page.on('request', request => { if (new URL(request.url()).origin === config.relayOrigin && request.headers()['x-rcd-key']) encryptedReads++; });
  try {
    await page.goto(`${config.webOrigin}/relay-portal?returnTo=%2Fdesktop%2Fdevices`);
    await page.getByLabel('Email or username', { exact: true }).fill(config.username);
    await page.getByLabel('Password', { exact: true }).fill(config.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByTestId('desktop-devices')).toBeVisible();
  } catch {
    await page.goto('about:blank').catch(() => undefined);
    throw new Error('Fixture sign-in or devices initialization failed; authentication details were not retained.');
  }
  await expect(page.locator('.pd-device-card')).toHaveCount(3);
  await expect.poll(async () => (await page.locator('.pd-usage-table tfoot').innerText()).replace(/[^0-9]/g, '')).not.toBe('');
  await expect.poll(() => encryptedReads).toBeGreaterThan(0);
  await page.screenshot({ path: `${evidence}/real-fixture-today.png`, fullPage: true });
  await page.getByRole('button', { name: 'All time', exact: true }).click();
  await expect.poll(async () => (await page.locator('.pd-usage-table tfoot').innerText()).replace(/[^0-9]/g, '')).not.toBe('');
  await expect(page.locator('.pd-usage-table tfoot td:nth-child(3)')).not.toHaveText('0');
  await page.locator('.pd-usage-device-row').first().click();
  await expect(page.locator('.pd-workspace-usage a').first()).toBeVisible();
  await page.screenshot({ path: `${evidence}/real-fixture-all.png`, fullPage: true });
  expect((await page.locator('.pd-page-header').boundingBox())!.height).toBe(40);
  expect(await page.locator('.pd-device-card').evaluateAll(rows => rows.every(row => row.getBoundingClientRect().height <= 100))).toBe(true);
  expect(await page.locator('.pd-page').innerText()).not.toContain('≈ —');
  const cardHeights = await page.locator('.pd-device-card').evaluateAll(rows => rows.map(row => row.getBoundingClientRect().height));
  writeFileSync(`${evidence}/real-fixture.json`, JSON.stringify({ devices: 3, encryptedReads, cardHeights, pageHeaderHeight: (await page.locator('.pd-page-header').boundingBox())!.height }, null, 2));
});
