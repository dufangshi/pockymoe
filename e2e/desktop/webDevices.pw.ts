import { expect, test, type Page } from '@playwright/test';
import { seed } from './fixtures';

const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? '.scratch/web-devices';
async function seedWeb(page: Page, pack = 'classic', dark = false) {
  await page.addInitScript(() => {
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
    localStorage.setItem('pockymoe.sidebar-sections.v1:desktop', JSON.stringify({ shortcuts: true, workspace: true, recents: true }));
  });
  await page.route('**/relay/account/workbench', route => route.fulfill({ json: { threads: Array.from({ length: 6 }, (_, i) => ({ deviceId: 'device-0', threadId: `t-${i}`, title: `Device statistics task ${i}`, workspaceId: 'w-0', workspaceLabel: 'Pockymoe', favorite: i === 0, visitedAt: new Date().toISOString() })), notifications: [] } }));
  await seed(page, pack, dark, false, false, false);
}
for (const [pack, dark] of [['classic', false], ['classic', true], ['plum-pocket', false], ['plum-pocket', true], ['terminal', false], ['terminal', true]] as const) {
  test(`statistics share browser navigation: ${pack} ${dark ? 'dark' : 'light'}`, async ({ page }) => {
    await seedWeb(page, pack, dark);
    expect(await page.evaluate(() => !!window.pockymoeHost)).toBe(false);
    const rail = page.locator('.matter-rail');
    await expect(rail).toHaveCount(1);
    await expect(rail.getByRole('button', { name: '设备', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('.desktop-activity')).toBeVisible();
    await expect(page.locator('.matter-sidebar')).toHaveCount(1);
    await expect(page.locator('.pd-account-usage')).toBeVisible();
    await expect(page.locator('.pd-usage-device').first()).toBeVisible();
    await expect(page.locator('[data-desktop-titlebar], .desktop-client-update, [data-desktop-surface]')).toHaveCount(0);
    for (const width of [1440, 900, 390]) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1100 });
      if (width === 390) {
        await page.goto('/relay-devices');
        await expect(page).toHaveURL(/\/relay-devices$/);
        await expect(page.locator('[data-navigation-shell], .matter-rail')).toHaveCount(0);
        await expect(page.locator('[data-page="devices"]')).toContainText('MacBook Pro');
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      if (width >= 640) {
        const content = await page.getByTestId('desktop-devices').boundingBox();
        expect(content!.width).toBeGreaterThan(width - 400);
        expect(content!.x + content!.width).toBeLessThanOrEqual(width);
      }
      await page.screenshot({ path: `${evidence}/web-devices-${pack}-${dark ? 'dark' : 'light'}-${width}.png`, animations: 'disabled' });
    }
  });
}

test('sidebar survives opening a thread, workspaces and returning without a second device index', async ({ page }) => {
  // Count logical subscriptions at the transport boundary: the fixture's 404
  // key probe intentionally prevents a real encrypted WebSocket from opening.
  await page.route('**/src/lib/relayTransport.ts', route => route.fulfill({ contentType: 'application/javascript', body: `
    import * as actual from '/src/lib/relayTransport.ts?web-devices-original';
    export * from '/src/lib/relayTransport.ts?web-devices-original';
    export function encryptedRelaySocket(url) {
      (window.__deviceSockets ??= []).push(url);
      return actual.encryptedRelaySocket(url);
    }
  ` }));
  await page.setViewportSize({ width: 1440, height: 650 });
  await seedWeb(page);
  // Keep the real thread renderer, with a complete deterministic summary.
  await page.route(/\/relay\/devices\/device-0\/api\/threads\/t-0(\?.*)?$/, route => route.fulfill({ json: {
    workspace: { id: 'w-0', hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null },
    thread: { id: 't-0', workspaceId: 'w-0', title: 'Device statistics task 0', provider: 'acp', agentId: 'codex', status: 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
    turns: [], totalTurnCount: 0, pendingRequests: [], pendingSteers: [],
  } }));
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: {}, sessions: {}, branching: {}, management: {}, usage: {} } } }));
  await page.route('**/api/agent-runtimes/**', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
  const sidebar = page.locator('.matter-sidebar');
  await sidebar.evaluate(el => { el.setAttribute('data-retained-sidebar', 'yes'); });
  const railBefore = await page.locator('.matter-rail > button').evaluateAll(nodes => nodes.slice(0, 5).map(node => ({ icon: node.querySelector('svg')?.getAttribute('data-icon'), top: node.getBoundingClientRect().top })));
  await sidebar.locator('a[href="/devices/device-0/threads/t-0"]').first().click();
  await expect(page).toHaveURL(/\/devices\/device-0\/threads\/t-0/);
  await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  await expect(page.locator('[data-retained-sidebar="yes"]')).toBeVisible();
  await expect(page.locator('.matter-rail')).toHaveCount(1);
  const railThread = await page.locator('.matter-rail > button').evaluateAll(nodes => nodes.slice(0, 5).map(node => ({ icon: node.querySelector('svg')?.getAttribute('data-icon'), top: node.getBoundingClientRect().top })));
  expect(railThread).toEqual(railBefore);
  // Width and collapse belong to the same retained workbench.
  await page.getByRole('separator', { name: '调整侧栏宽度', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  const width = await sidebar.evaluate(el => el.getBoundingClientRect().width);
  const scroller = sidebar.locator('.matter-sidebar-scroll').first();
  await scroller.evaluate(el => { el.scrollTop = 100; });
  const scroll = await scroller.evaluate(el => el.scrollTop);
  expect(scroll).toBeGreaterThan(0);
  await page.locator('.matter-rail').getByRole('button', { name: '设备', exact: true }).click();
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  expect(await sidebar.evaluate(el => el.getBoundingClientRect().width)).toBe(width);
  expect(await scroller.evaluate(el => el.scrollTop)).toBe(scroll);
  await page.getByRole('article', { name: 'MacBook Pro', exact: true }).locator('a.pd-workspaces-arrow').click();
  await expect(page.locator('[data-page="workspaces"]')).toBeVisible();
  await expect(page.locator('[data-retained-sidebar="yes"]')).toBeVisible();
  await page.locator('.matter-rail').getByRole('button', { name: '设备', exact: true }).click();
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await expect(page.locator('[data-retained-sidebar="yes"]')).toBeVisible();
  await page.locator('.matter-rail-bottom').getByRole('button', { name: 'Alex 的 Relay 账号菜单', exact: true }).click();
  await page.getByRole('menuitem', { name: '账号设置', exact: true }).click();
  await expect(page.locator('[data-page="account"]')).toBeVisible();
  await expect(page.locator('[data-retained-sidebar="yes"]')).toBeVisible();
  await page.locator('.matter-rail').getByRole('button', { name: '设备', exact: true }).click();
  await expect(page.getByTestId('desktop-devices')).toBeVisible();
  await page.getByRole('article', { name: 'MacBook Pro', exact: true }).locator('a.pd-device-title').click();
  await expect(page).toHaveURL(/\/desktop\/devices\/device-0/);
  await expect(page.locator('[data-retained-sidebar="yes"]')).toBeVisible();
  // There is one account-wide stream per online device; a focused transcript
  // may additionally own its thread-specific stream, never a second full index.
  const accountStreams = (await page.evaluate(() => (window as any).__deviceSockets as string[])).filter(url => !new URL(url).searchParams.get('threadId'));
  expect(accountStreams).toHaveLength(2);
});

test('phone load keeps the original devices page', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await seedWeb(page);
  await page.goto('/relay-devices');
  await expect(page).toHaveURL(/\/relay-devices$/);
  await expect(page.locator('[data-navigation-shell], .matter-rail')).toHaveCount(0);
  await expect(page.locator('[data-page="devices"]')).toContainText('MacBook Pro');
  await page.screenshot({ path: `${evidence}/phone-devices-390.png`, animations: 'disabled' });
});


test('signed-out browser Devices still redirects to the sign-in portal', async ({ page }) => {
  await page.route('**/api/auth/session', route => route.fulfill({ json: { authenticated: false, mode: 'relay' } }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false, user: null } }));
  await page.goto('/desktop/devices');
  await expect(page).toHaveURL(/\/relay-portal\?returnTo=%2Fdesktop%2Fdevices/);
  await expect(page.locator('[data-navigation-shell]')).toHaveCount(0);
});
