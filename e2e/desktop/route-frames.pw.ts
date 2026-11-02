import { test, expect } from '@playwright/test';
import { seed } from './fixtures';
import { mkdirSync, writeFileSync } from 'node:fs';
const evidence = process.env.DEVICE_MANAGEMENT_EVIDENCE ?? '.scratch/device-management';
const phase = process.env.DEVICE_MANAGEMENT_PHASE ?? 'after';
test('route content frame sampling', async ({ page }) => {
  mkdirSync(`${evidence}/${phase}/frames`, { recursive: true });
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:\d+\/\?token=/, socket => socket.onMessage(() => undefined));
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  await seed(page, 'classic', true);
  // Keep the real thread renderer and provide a deterministic idle conversation.
  await page.route(/\/relay\/devices\/device-0\/api\/threads\/t-[01](\?.*)?$/, async route => {
    const id = new URL(route.request().url()).pathname.split('/').at(-1)!;
    if (id === 't-1') await new Promise(resolve => setTimeout(resolve, 350));
    await route.fulfill({ json: { workspace: { id: 'w-0', hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null }, thread: { id, workspaceId: 'w-0', title: `Retained thread ${id}`, provider: 'acp', agentId: 'codex', status: 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }, turns: [{ id: `history-${id}`, status: 'completed', startedAt: new Date().toISOString(), completedAt: new Date().toISOString(), items: [{ id: `question-${id}`, kind: 'userMessage', text: `Read history for ${id}` }, { id: `reply-${id}`, kind: 'agentMessage', responsePhase: 'commentary', text: Array.from({ length: 100 }, (_, i) => `History line ${i} for ${id}.`).join('\n\n') }] }], totalTurnCount: 1, pendingRequests: [], pendingSteers: [] } });
  });
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: {}, sessions: {}, branching: {}, management: {}, usage: {} } } }));
  await page.route('**/api/agent-runtimes/**', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
  await page.route('**/relay/devices/device-0/api/config/runtime', async route => { await new Promise(resolve => setTimeout(resolve, 350)); await route.fulfill({ json: {} }); });
  const go = async (path: string) => page.evaluate(path => { history.pushState({}, '', path); dispatchEvent(new PopStateEvent('popstate')); }, path);
  const session = await page.context().newCDPSession(page);
  const frames: { time: number; file: string }[] = [];
  session.on('Page.screencastFrame', async event => {
    const file = `${evidence}/${phase}/frames/${String(frames.length).padStart(4, '0')}.jpg`;
    writeFileSync(file, Buffer.from(event.data, 'base64')); frames.push({ time: event.metadata.timestamp, file });
    await session.send('Page.screencastFrameAck', { sessionId: event.sessionId }).catch(() => { /* Page teardown can race the last frame acknowledgement. */ });
  });
  await session.send('Page.startScreencast', { format: 'jpeg', quality: 65, everyNthFrame: 1 });
  await page.evaluate(() => {
    const samples: unknown[] = []; (window as any).__routeSamples = samples;
    const start = performance.now();
    function sample() {
      const selectors = ['.desktop-map-scene:not([hidden]) .thread-map', '[data-testid="desktop-devices"]', '[data-testid="thread-scroll-container"]'];
      const visible = (element: Element) => element.getClientRects().length > 0 && !element.closest('[hidden]');
      samples.push({ ms: performance.now() - start, path: location.pathname, content: selectors.some(selector => [...document.querySelectorAll(selector)].some(visible)), threadVisible: [...document.querySelectorAll('[data-testid="thread-scroll-container"]')].some(visible), retained: Boolean(document.querySelector('[data-retained-probe="yes"]')) });
      if (performance.now() - start < 15000) requestAnimationFrame(sample);
    } sample();
  });
  await go('/devices/device-0/threads/t-0');
  await expect(page.locator('[data-testid="thread-scroll-container"]').first()).toBeVisible();
  await page.getByRole('button', { name: '展开', exact: true }).click();
  await expect(page.locator('[data-testid="thread-scroll-container"]').first()).toContainText('History line 99 for t-0.');
  await page.locator('[data-testid="thread-scroll-container"]').first().evaluate(el => { el.setAttribute('data-retained-probe', 'yes'); el.scrollTop = el.scrollHeight; });
  // Let the initial tail-follow frames finish, then model the reader moving
  // upward into history rather than racing the thread's initial layout.
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await page.locator('[data-retained-probe]').hover();
  await page.mouse.wheel(0, -1);
  await page.locator('[data-retained-probe]').evaluate(el => { el.scrollTop = 400; el.dispatchEvent(new Event('scroll')); });
  const scrollTop = await page.locator('[data-retained-probe]').evaluate(el => el.scrollTop);
  await go('/desktop/devices'); await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await go('/devices/device-0/threads/t-0'); await expect(page.locator('[data-testid="thread-scroll-container"]').first()).toBeVisible();
  if (phase !== 'before') {
    await expect(page.locator('[data-retained-probe]')).toBeVisible();
    expect(await page.locator('[data-retained-probe]').evaluate(el => el.scrollTop)).toBe(scrollTop);
  }
  await go('/'); await expect(page.locator('.desktop-map-scene:not([hidden]) .thread-map')).toBeVisible();
  await go('/devices/device-0/threads/t-0'); await expect(page.locator('[data-testid="thread-scroll-container"]').first()).toBeVisible();
  await go('/devices/device-0/threads/t-1');
  if (phase !== 'before') await expect(page.locator('[data-retained-probe]')).toBeVisible();
  await expect(page.getByRole('button', { name: '关闭 Retained thread t-1', exact: true })).toBeVisible();
  await expect(page.locator('[data-testid="thread-scroll-container"]').first()).toBeVisible();
  await go('/desktop/devices'); await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await go('/'); await expect(page.locator('.desktop-map-scene:not([hidden]) .thread-map')).toBeVisible();
  await session.send('Page.stopScreencast');
  const samples = await page.evaluate(() => (window as any).__routeSamples);
  writeFileSync(`${evidence}/${phase}/route-samples.json`, JSON.stringify({ frames, samples }, null, 2));
  if (phase !== 'before') {
    expect(samples.filter((sample: any) => !sample.content)).toEqual([]);
    const warmedReturns = samples.filter((sample: any, index: number) => index > 0 && sample.path.endsWith('/t-0') && samples[index - 1].path !== sample.path && sample.retained);
    expect(warmedReturns).toHaveLength(2);
    expect(warmedReturns.every((sample: any) => sample.threadVisible)).toBe(true);
  }
});
