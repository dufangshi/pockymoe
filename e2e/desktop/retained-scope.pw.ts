import { test, expect, type WebSocketRoute } from '@playwright/test';
import { seed } from './fixtures';
async function threadFixture(page: import('@playwright/test').Page) {
  await page.route('**/relay/devices/device-0/api/threads/t-0?*', route => route.fulfill({ json: { workspace: { id: 'w-0', label: 'Fixture', absPath: '/fixture' }, thread: { id: 't-0', workspaceId: 'w-0', title: 'Scoped A', provider: 'acp', agentId: 'codex', status: 'idle', isLoaded: true, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }, turns: [], pendingRequests: [], pendingSteers: [] } }));
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: {}, sessions: {}, branching: {}, management: {}, usage: {} } } }));
  await page.route('**/api/agent-runtimes/**', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
}
test('retained A pauses reads and reconnects while selected device changes to B, then refreshes A', async ({ page }) => {
  await seed(page); await threadFixture(page);
  const reads: string[] = [], sockets: { path: string; route: WebSocketRoute }[] = [];
  page.on('request', request => { const url = new URL(request.url()); if (url.pathname.includes('/threads/t-0') && url.searchParams.get('limit') === '3') reads.push(url.pathname); });
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, route => { sockets.push({ path: new URL(route.url()).pathname, route }); route.onMessage(() => undefined); });
  const go = (path: string) => page.evaluate(path => { history.pushState({}, '', path); dispatchEvent(new PopStateEvent('popstate')); }, path);
  await go('/devices/device-0/threads/t-0'); await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  await expect.poll(() => sockets.some(socket => new URL(socket.route.url()).searchParams.get('threadId') === 't-0')).toBe(true);
  await page.clock.install();
  const socket = sockets.findLast(socket => new URL(socket.route.url()).searchParams.get('threadId') === 't-0')!;
  await socket.route.close({ code: 1011, reason: 'test reconnect' });
  await go('/desktop/devices'); await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await page.evaluate(async () => { const api = await import('/src/lib/api.ts'); api.setSelectedRelayDeviceId('device-1'); window.dispatchEvent(new Event('model-pricing-updated')); });
  const before = reads.length, connected = sockets.length;
  await page.clock.fastForward(6000);
  expect(reads.length).toBe(before); expect(sockets.length).toBe(connected);
  await go('/devices/device-0/threads/t-0');
  await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  await expect.poll(() => reads.length).toBeGreaterThan(before);
  expect(reads.every(path => path.includes('/device-0/'))).toBe(true);
  await expect.poll(() => sockets.length).toBeGreaterThan(connected);
  expect(sockets.slice(connected).every(socket => socket.path.includes('/device-0/'))).toBe(true);
});
test('mobile surface never mounts the retained desktop outlet, including at desktop width', async ({ page }) => {
  await page.addInitScript(() => {
    window.__POCKYMOE_BOOTSTRAP__ = { mode: 'relay' };
    window.pockymoeHost = { capabilities: async () => ({ surface: 'mobile', platform: 'ios' }), activity: { sync() {} }, appearance: { sync() {} }, onCommand: () => () => {}, commandResult() {}, window: { setTitle() {}, openPreferences() {} }, settings: { get: async () => ({ webOrigin: location.origin, relayOrigin: location.origin }) } } as any;
  });
  await seed(page); await threadFixture(page);
  await page.goto('/devices/device-0/threads/t-0');
  for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
    await expect(page.locator('[data-desktop-surface], .desktop-route')).toHaveCount(0);
  }
});
