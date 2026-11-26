import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { seed } from './fixtures';

// Issue #115: one device center. A device's page holds its statistics, all its
// settings and its access; one device switcher tops the thread, workspace and
// device sidebars; the settings window keeps the browser's and the account's
// tabs and names where a device's settings went; the phone uses the same pages.
const evidence = '.scratch/screens/device-center';
const go = (page: Page, path: string) => page.evaluate(target => { history.pushState({}, '', target); dispatchEvent(new PopStateEvent('popstate')); }, path);
const where = (page: Page) => page.evaluate(() => location.pathname + location.search.replace(/[?&]desktopMockHost=1/, '').replace(/^&/, '?'));
const shared = { id: 'grant-1', ownerUserId: 'someone', ownerUsername: 'blair', targetUserId: 'devices-test', targetUsername: 'Alex', deviceId: 'device-9', deviceName: 'Blair Studio', deviceConnected: true, scope: 'device', threadId: null, threadTitle: null, workspaceId: null, workspaceLabel: null, workspaceScope: 'all', workspaceIds: [], label: null, threadAccess: 'control', workspaceAccess: 'read', canCreateThreads: true, createdAt: '2026-10-01T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [] };

async function prepare(page: Page, pack = 'classic', dark = false, host = true) {
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  await seed(page, pack, dark, false, false, host, false, { sharedDevicesWithMe: [shared] });
  // Each device answers with its own values, so a wrong target is visible.
  await page.route('**/api/config/workspace-settings', route => route.fulfill({ json: { workspaceRoot: `/root-of-${new URL(route.request().url()).pathname.split('/')[3]}`, devHome: `/dev-of-${new URL(route.request().url()).pathname.split('/')[3]}` } }));
}
async function openSettings(page: Page) {
  await page.getByRole('button', { name: /账号菜单/ }).and(page.locator(':visible')).first().click();
  await page.getByRole('menuitem', { name: '设置', exact: true }).click();
  const dialog = page.getByTestId('settingsDialog').or(page.getByTestId('settings-dialog'));
  await expect(dialog).toBeVisible();
  return dialog;
}

test('a device page is its one home: Overview, Settings and Access, with one menu', async ({ page }) => {
  await prepare(page);
  await page.getByRole('article', { name: 'MacBook Pro', exact: true }).getByRole('link', { name: 'MacBook Pro', exact: true }).click();
  await expect(page).toHaveURL(/\/desktop\/devices\/device-0/);
  const content = page.getByTestId('desktop-devices');
  await expect(content.getByRole('heading', { level: 1 })).toHaveText('MacBook Pro');
  // One "Devices" heading, one ⋯ menu, no one-row table, no second "Protect all devices" (issue #116).
  await expect(content.getByRole('button', { name: 'MacBook Pro 的更多操作', exact: true })).toHaveCount(1);
  await expect(content.locator('article.device-row')).toHaveCount(0);
  await expect(content.getByRole('heading', { name: '设备', exact: true })).toHaveCount(0);
  await expect(content.getByRole('button', { name: '保护所有设备', exact: true })).toHaveCount(0);
  await expect(content.getByRole('tab')).toHaveText(['概览', '设置', '访问']);
  await expect(content.getByRole('tab', { name: '概览', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(content.getByRole('combobox', { name: '统计范围', exact: true })).toHaveText('MacBook Pro');
  await content.getByRole('button', { name: 'MacBook Pro 的更多操作', exact: true }).click();
  await expect(page.getByRole('menu', { name: 'MacBook Pro 的操作', exact: true }).getByRole('menuitem')).toHaveText(['打开', '新建线程', '在地图中显示', '重命名', '共享…', '隔离…', '设置…', '移除…']);
  // "Settings…" is the page's Settings tab.
  await page.getByRole('menuitem', { name: '设置…', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-0?tab=settings');
  const settings = content.getByRole('tablist', { name: '设置分类' });
  await expect(settings.getByRole('tab')).toHaveText(['通用', 'Supervisor', 'Harness', '上游', '资源', '隔离环境', '工作区']);
  await expect(content.getByRole('button', { name: '复制 macOS/Linux 设置命令', exact: true })).toBeVisible();
  await expect(content.getByRole('button', { name: '替换设备令牌', exact: true })).toBeVisible();
  await expect(content.getByRole('checkbox', { name: '隔离此设备', exact: true })).toBeVisible();
  await settings.getByRole('tab', { name: '工作区', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-0?tab=settings&section=workspace');
  await expect(content.getByLabel('开发目录')).toHaveValue('/dev-of-device-0');
  // The page names the device: no device bar to choose another one.
  await expect(content.getByTestId('settings-device')).toHaveCount(0);
  await content.getByRole('tab', { name: '访问', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-0?tab=access');
  await expect(content.getByRole('tab', { name: '访问', exact: true })).toHaveAttribute('aria-selected', 'true');
  // Old links: ?action=settings and ?action=update open the Settings tab; the parameter goes.
  await go(page, '/desktop/devices/device-1?action=update');
  await expect.poll(() => where(page)).toBe('/desktop/devices/device-1?tab=settings&section=device');
  await go(page, '/desktop/devices/device-1?action=rename');
  await expect(page.getByRole('dialog', { name: '重命名设备', exact: true })).toBeVisible();
  expect(await where(page)).toBe('/desktop/devices/device-1');
  await page.keyboard.press('Escape');
  // An offline device opens and says it is offline.
  await go(page, '/desktop/devices/device-2');
  await expect(content.locator('.device-status-line .admin-device-state.is-offline')).toContainText('离线');
  await content.getByRole('link', { name: '打开', exact: true }).click();
  await expect(page).toHaveURL(/\/devices\/device-2\/workspaces/);
});

test('one device switcher tops the workspace, thread and device sidebars, with shared devices marked', async ({ page }) => {
  await prepare(page);
  await go(page, '/devices/device-0/workspaces');
  const switcher = page.getByRole('button', { name: '切换设备，当前：MacBook Pro，在线', exact: true });
  await expect(switcher).toBeVisible();
  await switcher.click();
  const menu = page.getByRole('menu', { name: '切换设备', exact: true });
  // Each with its connection mark (named for a screen reader); the shared one says who shared it.
  await expect(menu.getByRole('menuitemradio')).toHaveText([/MacBook Pro$/, /Windows Studio$/, /Linux Server$/, /Blair Studioblair 共享$/]);
  await expect(menu.getByRole('menuitemradio', { name: 'MacBook Pro' })).toHaveAttribute('aria-checked', 'true');
  await expect(menu.getByRole('menuitem')).toHaveText(['管理此设备…', '所有设备']);
  await menu.getByRole('menuitemradio', { name: /Blair Studio/ }).click();
  await expect(page).toHaveURL(/\/devices\/device-9\/workspaces/);
  // A shared device is opened here, not managed: its owner manages it.
  await page.getByRole('button', { name: /^切换设备，当前：Blair Studio/ }).click();
  await expect(menu.getByRole('menuitem')).toHaveText(['所有设备']);
  await menu.getByRole('menuitemradio', { name: 'Windows Studio' }).click();
  await expect(page).toHaveURL(/\/devices\/device-1\/workspaces/);
  await page.getByRole('button', { name: /^切换设备，当前：Windows Studio/ }).click();
  await menu.getByRole('menuitem', { name: '管理此设备…', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-1?tab=settings');
  // On the device center the same switcher moves between device pages.
  await page.getByRole('button', { name: /^切换设备，当前：Windows Studio/ }).click();
  await menu.getByRole('menuitemradio', { name: 'MacBook Pro' }).click();
  expect(await where(page)).toBe('/desktop/devices/device-0');
  // A thread's sidebar starts with it too (the real thread page, with a minimal thread).
  await page.route(/\/relay\/devices\/device-0\/api\/threads\/t-0(\?.*)?$/, route => route.fulfill({ json: {
    workspace: { id: 'w-0', hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: null },
    thread: { id: 't-0', workspaceId: 'w-0', title: 'Device statistics task 0', provider: 'acp', agentId: 'codex', status: 'idle', isPinned: false, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() },
    turns: [], totalTurnCount: 0, pendingRequests: [], pendingSteers: [],
  } }));
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: {}, sessions: {}, branching: {}, management: {}, usage: {} } } }));
  await page.route('**/api/agent-runtimes/**', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
  await go(page, '/devices/device-0/threads/t-0');
  await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  await expect(page.getByRole('button', { name: '切换设备，当前：MacBook Pro，在线', exact: true })).toBeVisible();
});

test('the settings window keeps this browser and the account, and names where device settings went', async ({ page }) => {
  await prepare(page);
  await go(page, '/desktop/devices/device-1');
  const dialog = await openSettings(page);
  await expect(dialog.getByRole('tab')).toHaveText(['偏好设置', '通知', '插件']);
  // Its subtitle says what it holds now.
  await expect(dialog).toContainText('这个浏览器和你的账号：偏好、通知与插件。设备的设置在它的设备页。');
  const link = dialog.getByTestId('settings-device-link');
  await expect(link).toHaveText(/设备设置 · Windows Studio$/);
  await link.click();
  await expect(page.getByTestId('settingsDialog')).toHaveCount(0);
  expect(await where(page)).toBe('/desktop/devices/device-1?tab=settings');
  // A page about every device lists them to choose from.
  await go(page, '/desktop/devices');
  await expect((await openSettings(page)).getByTestId('settings-device-link')).toHaveText('设备设置 · 所有设备');
});

test('the phone uses the same device center; the old page redirects with its query', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await prepare(page, 'classic', false, false);
  await page.goto('/relay-devices?tab=shared&dir=out');
  await expect.poll(() => where(page)).toBe('/desktop/devices?tab=shared&dir=out');
  await go(page, '/desktop/devices');
  // Devices first; the statistics fold away below them.
  await expect(page.getByRole('article', { name: 'MacBook Pro', exact: true })).toBeVisible();
  const fold = page.locator('details.pd-stats-fold');
  await expect(fold).not.toHaveAttribute('open', '');
  await page.getByRole('article', { name: 'MacBook Pro', exact: true }).getByRole('link', { name: 'MacBook Pro', exact: true }).click();
  await expect(page.getByTestId('desktop-devices').getByRole('heading', { level: 1 })).toHaveText('MacBook Pro');
  await page.getByRole('tab', { name: '设置', exact: true }).click();
  await expect(page.getByRole('button', { name: '替换设备令牌', exact: true })).toBeVisible();
  const box = (await page.getByTestId('desktop-devices').boundingBox())!;
  expect(box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test('the device center in every theme, light and dark, desktop and phone', async ({ page, browser }) => {
  test.setTimeout(480_000);
  mkdirSync(evidence, { recursive: true });
  for (const width of [1440, 390]) {
    const phone = width < 640;
    const view = phone ? await (await browser.newContext({ viewport: { width, height: 900 }, hasTouch: true })).newPage() : page;
    view.setDefaultTimeout(15_000);
    await view.setViewportSize({ width, height: 900 });
    for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) {
      await prepare(view, pack, dark, !phone);
      const name = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
      await expect(view.locator('html')).toHaveAttribute('data-theme-preset', pack);
      await view.screenshot({ path: `${evidence}/overview-${name}.png` });
      await go(view, '/desktop/devices/device-0');
      await expect(view.getByTestId('desktop-devices').getByRole('heading', { level: 1 })).toHaveText('MacBook Pro');
      await view.screenshot({ path: `${evidence}/device-${name}.png` });
      await go(view, '/desktop/devices/device-0?tab=settings');
      await expect(view.getByRole('button', { name: '替换设备令牌', exact: true })).toBeVisible();
      await view.screenshot({ path: `${evidence}/settings-${name}.png` });
      const dialog = await openSettings(view);
      await expect(dialog.getByTestId('settings-device-link')).toBeVisible();
      await view.screenshot({ path: `${evidence}/window-${name}.png` });
      await view.keyboard.press('Escape');
      if (!phone) {
        await go(view, '/devices/device-0/workspaces');
        await view.getByRole('button', { name: /^切换设备，当前：MacBook Pro/ }).click();
        await expect(view.getByRole('menu', { name: '切换设备', exact: true })).toBeVisible();
        await view.screenshot({ path: `${evidence}/switcher-${name}.png` });
        await view.keyboard.press('Escape');
      }
    }
    if (phone) await view.context().close();
  }
});
