import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { seed } from './fixtures';
const evidence = '.scratch/device-management/actions';
// Issue #114: a device is managed the same way on its card, on its row and on a
// phone, and a workspace the same way as a device. The names, order, separator
// and danger styling belong to the shared action model (deviceActions,
// workspaceActions); the menu is one component (ManagedObjectMenu).
const DEVICE_ACTIONS = ['打开', '新建线程', '重命名', '共享…', '隔离…', '设置…', '移除…'];
const go = (page: Page, path: string) => page.evaluate(target => { history.pushState({}, '', target); dispatchEvent(new PopStateEvent('popstate')); }, path);
const where = (page: Page) => page.evaluate(() => location.pathname + location.search.replace(/[?&]desktopMockHost=1/, ''));
async function prepare(page: Page, pack = 'classic', dark = false, host = true) {
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  await seed(page, pack, dark, false, false, host);
  // MacBook Pro is isolated, and so is its first workspace.
  await page.route('**/api/config/agent-isolation', route => route.fulfill({ json: { device: route.request().url().includes('/device-0/'), workspaces: [] } }));
  await page.route(/\/relay\/devices\/device-\d+\/api\/workspaces$/, route => route.fulfill({ json: [
    { id: 'w-0', hostId: 'host', absPath: '/home/alex/projects/pockymoe', label: 'Pockymoe', isFavorite: true, createdAt: new Date().toISOString(), lastOpenedAt: new Date().toISOString(), agentIsolated: true },
    { id: 'w-1', hostId: 'host', absPath: '/home/alex/projects/a-very-long-directory-name/research-notes-2026', label: 'Research notes', isFavorite: false, createdAt: new Date().toISOString(), lastOpenedAt: null, agentIsolated: false },
  ] }));
}
const trigger = (page: Page, name: string) => page.getByRole('button', { name: `${name} 的更多操作`, exact: true });
const menuOf = (page: Page, name: string) => page.getByRole('menu', { name: `${name} 的操作`, exact: true });
async function expectInViewport(page: Page, name: string) {
  const box = (await menuOf(page, name).boundingBox())!, view = page.viewportSize()!;
  expect(box.x).toBeGreaterThanOrEqual(0); expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(view.width); expect(box.y + box.height).toBeLessThanOrEqual(view.height);
}

test('one menu for a device: its card, a right click, and its row on its own page', async ({ page }) => {
  await prepare(page);
  const card = page.getByRole('article', { name: 'MacBook Pro', exact: true });
  await expect(card.locator('.managed-menu-trigger [data-icon="more"]')).toBeVisible();
  await trigger(page, 'MacBook Pro').click();
  const menu = menuOf(page, 'MacBook Pro');
  // The card adds the two places this surface has: the device's page and the map.
  await expect(menu.getByRole('menuitem')).toHaveText(['打开', '新建线程', '设备详情', '在地图中显示', ...DEVICE_ACTIONS.slice(2)]);
  expect(await menu.evaluate(element => element.parentElement === document.body)).toBe(true);
  await expect(menu.getByRole('menuitem').first()).toBeFocused();
  await expect(menu.getByRole('separator')).toHaveCount(1);
  const remove = menu.getByRole('menuitem', { name: '移除…', exact: true });
  await expect(remove).toHaveAttribute('data-tone', 'danger');
  expect(await remove.evaluate(element => getComputedStyle(element).color)).not.toBe(await menu.getByRole('menuitem', { name: '重命名', exact: true }).evaluate(element => getComputedStyle(element).color));
  await expect(menu.getByRole('menuitem').locator('svg')).toHaveCount(9);
  await page.keyboard.press('Escape');
  await expect(trigger(page, 'MacBook Pro')).toBeFocused();
  // A right click on the card is the same menu.
  await card.click({ button: 'right', position: { x: 60, y: 60 } });
  await expect(menuOf(page, 'MacBook Pro').getByRole('menuitem')).toHaveCount(9);
  await page.keyboard.press('Escape');
  // An offline device offers the same actions; only a new thread waits for it.
  await trigger(page, 'Linux Server').click();
  await expect(menuOf(page, 'Linux Server').getByRole('menuitem', { name: '新建线程', exact: true })).toBeDisabled();
  await expect(menuOf(page, 'Linux Server').getByRole('menuitem').and(page.locator(':disabled'))).toHaveCount(1);
  await page.keyboard.press('Escape');

  // On the device's own page its one ⋯ (in the header) lists the same actions; its
  // menu is at the page root too, so "Remove…" is not cut off at the page's end.
  await go(page, '/desktop/devices/device-0');
  const content = page.getByTestId('desktop-devices');
  await expect(content.locator('article.device-row')).toHaveCount(0);
  await expect(content.locator('.device-status-line .admin-device-state.is-online')).toHaveCount(1);
  await expect(content.getByRole('button', { name: 'MacBook Pro 的更多操作', exact: true })).toHaveCount(1);
  await content.getByRole('button', { name: 'MacBook Pro 的更多操作', exact: true }).click();
  await expect(menuOf(page, 'MacBook Pro').getByRole('menuitem')).toHaveText(['打开', '新建线程', '在地图中显示', ...DEVICE_ACTIONS.slice(2)]);
  expect(await menuOf(page, 'MacBook Pro').evaluate(element => element.parentElement === document.body)).toBe(true);
  await expectInViewport(page, 'MacBook Pro');
});

test('rename, share, isolation and removal open in place, settings on the device page; old ?action= links still work', async ({ page }) => {
  await prepare(page);
  const popups: string[] = [];
  page.on('popup', popup => { popups.push('external browser'); void popup.close(); });
  await page.evaluate(() => { window.pockymoeHost!.shell.openExternal = () => { throw new Error('Management opened an external browser'); }; });
  const choose = async (item: string) => { await trigger(page, 'MacBook Pro').click(); await menuOf(page, 'MacBook Pro').getByRole('menuitem', { name: item, exact: true }).click(); };

  // Rename: over the overview, which stays the page after cancelling.
  await choose('重命名');
  const rename = page.getByRole('dialog', { name: '重命名设备', exact: true });
  await expect(rename.getByRole('textbox', { name: '设备名称', exact: true })).toHaveValue('MacBook Pro');
  expect(await where(page)).toBe('/desktop/devices');
  await page.keyboard.press('Escape');
  await expect(rename).toHaveCount(0);
  expect(await where(page)).toBe('/desktop/devices');
  await expect(trigger(page, 'MacBook Pro')).toBeFocused();

  await choose('共享…');
  const recipient = page.getByRole('dialog', { name: '共享 MacBook Pro', exact: true }).getByPlaceholder('用户名或电子邮箱');
  await recipient.fill('fixture-recipient');
  await expect(recipient).toBeFocused();
  await page.keyboard.press('Escape');
  const discard = page.getByRole('dialog', { name: '放弃未保存的更改？', exact: true });
  await discard.getByRole('button', { name: '取消', exact: true }).click();
  await expect(recipient).toHaveValue('fixture-recipient');
  await page.keyboard.press('Escape');
  await discard.getByRole('button', { name: '放弃更改', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await where(page)).toBe('/desktop/devices');

  await choose('隔离…');
  const isolation = page.getByRole('dialog', { name: '隔离 · MacBook Pro', exact: true });
  await expect(isolation.getByRole('checkbox', { name: '隔离此设备', exact: true })).toBeChecked();
  await expect(isolation).not.toContainText('Agent');
  await page.keyboard.press('Escape');
  expect(await where(page)).toBe('/desktop/devices');

  // Settings are the Settings tab of the device's page (issue #115): the setup commands and the token are there.
  await choose('设置…');
  expect(await where(page)).toBe('/desktop/devices/device-0?tab=settings');
  const settings = page.getByTestId('desktop-devices');
  await expect(settings.getByRole('button', { name: '复制 macOS/Linux 设置命令', exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: '复制 Windows 设置命令', exact: true })).toBeVisible();
  const replace = settings.getByRole('button', { name: '替换设备令牌', exact: true });
  await expect(replace.locator('svg')).toHaveCount(1);
  expect(await replace.evaluate(element => getComputedStyle(element).color)).toBe(await page.evaluate(() => { const probe = document.createElement('span'); probe.style.color = 'var(--status-danger-fg)'; document.body.append(probe); const color = getComputedStyle(probe).color; probe.remove(); return color; }));
  // Reached with the keyboard, it shows where focus is.
  await settings.getByRole('button', { name: '复制 Windows 设置命令', exact: true }).focus();
  await page.keyboard.press('Tab');
  await expect(replace).toBeFocused();
  expect(await replace.evaluate(element => getComputedStyle(element).outlineStyle)).not.toBe('none');
  await replace.click();
  const confirmReplace = page.getByRole('dialog', { name: '替换设备令牌？', exact: true });
  await expect(confirmReplace.getByRole('button', { name: '取消', exact: true })).toBeFocused();
  await confirmReplace.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  // The card's update badge opens the same settings, on the Supervisor's version.
  await go(page, '/desktop/devices');
  await page.getByRole('article', { name: 'Windows Studio', exact: true }).getByRole('button', { name: '有更新 0.13.42', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-1?tab=settings&section=device');
  await expect(settings).toContainText('0.13.42');
  await go(page, '/desktop/devices');

  await choose('移除…');
  const confirm = page.getByRole('dialog', { name: '移除设备', exact: true });
  await expect(confirm).toContainText('MacBook Pro');
  await expect(confirm.getByRole('button', { name: '取消', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await where(page)).toBe('/desktop/devices');

  // A link from an older client names the window in the address: it opens there, and the parameter goes.
  for (const [action, name] of [['rename', '重命名设备'], ['share', '共享 MacBook Pro'], ['isolate', '隔离 · MacBook Pro'], ['remove', '移除设备']] as const) {
    await go(page, `/desktop/devices/device-0?action=${action}`);
    await expect(page.getByRole('dialog', { name, exact: true })).toBeVisible();
    expect(await where(page)).toBe('/desktop/devices/device-0');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
  }
  // …and "settings" or "update" are the Settings tab now.
  await go(page, '/desktop/devices/device-0?action=update');
  await expect.poll(() => where(page)).toBe('/desktop/devices/device-0?tab=settings&section=device');
  await go(page, '/desktop/devices');

  await page.getByRole('button', { name: '添加设备', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '添加设备', exact: true }).getByRole('textbox', { name: '设备名称', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.locator('.desktop-map-scene:not([hidden]) .matter-rail').getByRole('button', { name: /账号菜单/ }).click();
  await page.getByRole('menuitem', { name: '账号设置', exact: true }).click();
  await expect(page).toHaveURL(/relay-account/);
  expect(popups).toEqual([]);
});

test('the map lists the same actions for a device and opens their windows over the map, settings on its page', async ({ page }) => {
  await prepare(page);
  await go(page, '/');
  const map = page.locator('.desktop-map-persistent');
  await expect(map).toBeVisible();
  const key = map.getByRole('button', { name: '设备操作：MacBook Pro', exact: true }).first();
  await key.click();
  const menu = page.getByRole('menu', { name: 'MacBook Pro', exact: true });
  await expect(menu.getByRole('menuitem')).toHaveText(['打开', '新建线程', '设备详情', '在地图中显示', ...DEVICE_ACTIONS.slice(2)]);
  await menu.getByRole('menuitem', { name: '隔离…', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '隔离 · MacBook Pro', exact: true }).getByRole('checkbox', { name: '隔离此设备', exact: true })).toBeChecked();
  expect(await where(page)).toBe('/');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(map).toBeVisible();
  // Settings are the device page's Settings tab.
  await key.click();
  await menu.getByRole('menuitem', { name: '设置…', exact: true }).click();
  expect(await where(page)).toBe('/desktop/devices/device-0?tab=settings');
  await expect(page.getByRole('button', { name: '替换设备令牌', exact: true })).toBeVisible();
});

test('an offline device opens and says it is offline; a workspace is managed like a device', async ({ page }) => {
  await prepare(page);
  const offlineArrow = page.getByRole('link', { name: '打开 Linux Server 的工作区', exact: true });
  await expect(offlineArrow).not.toHaveAttribute('aria-disabled', 'true');
  await offlineArrow.click();
  const workspaces = page.locator('[data-page="workspaces"]');
  await expect(workspaces.locator('.admin-device-state.is-offline')).toContainText('离线');
  await go(page, '/devices/device-0/workspaces');
  const row = page.getByRole('article', { name: 'Pockymoe', exact: true });
  // The path is a path chip; "Isolated" is a lock with the word, and a tap says what it means.
  await expect(row.locator('.path-chip')).toBeVisible();
  const isolated = row.getByRole('button', { name: '已隔离', exact: true });
  await expect(isolated.locator('svg')).toHaveCount(1);
  await isolated.click();
  await expect(page.getByRole('dialog', { name: '已隔离', exact: true })).toContainText('工作区以外的智能体无法列出、读取其中的线程');
  expect(await where(page)).toBe('/devices/device-0/workspaces');
  await page.keyboard.press('Escape');
  await trigger(page, 'Pockymoe').click();
  const menu = menuOf(page, 'Pockymoe');
  // The same actions in the same order as a device; the path stands where a device has its settings.
  await expect(menu.getByRole('menuitem')).toHaveText(['打开', '新建线程', '重命名', '共享…', '取消隔离', '查看路径', '移除…']);
  expect(await menu.evaluate(element => element.parentElement === document.body)).toBe(true);
  await expect(menu.getByRole('menuitem', { name: '移除…', exact: true })).toHaveAttribute('data-tone', 'danger');
  await page.keyboard.press('Escape');
  await row.click({ button: 'right', position: { x: 300, y: 10 } });
  await expect(menuOf(page, 'Pockymoe').getByRole('menuitem')).toHaveCount(7);
  await menuOf(page, 'Pockymoe').getByRole('menuitem', { name: '移除…', exact: true }).click();
  const confirm = page.getByRole('dialog', { name: '移除工作区？', exact: true });
  await expect(confirm).toContainText('从此 Supervisor 移除 Pockymoe 及其线程。不会删除磁盘文件。');
  await expect(confirm).not.toContainText('删除工作区');
  await expect(confirm.getByRole('button', { name: '移除工作区', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await trigger(page, 'Research notes').click();
  await menuOf(page, 'Research notes').getByRole('menuitem', { name: '打开', exact: true }).click();
  await expect(page).toHaveURL(/\/devices\/device-0\/threads\?workspaceId=w-1/);
});

test('the phone manages a device from the same menu on the same device center, renames it there, and opens it offline', async ({ browser }) => {
  // A phone: touch, as a long press needs.
  const page = await (await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true })).newPage();
  await prepare(page, 'classic', false, false);
  await go(page, '/relay-devices');
  await expect.poll(() => where(page)).toBe('/desktop/devices');
  const card = page.getByRole('article', { name: 'MacBook Pro', exact: true });
  await expect(card).toBeVisible();
  await trigger(page, 'MacBook Pro').click();
  const menu = menuOf(page, 'MacBook Pro');
  await expect(menu.getByRole('menuitem')).toHaveText(['打开', '新建线程', '设备详情', '在地图中显示', ...DEVICE_ACTIONS.slice(2)]);
  expect(await menu.evaluate(element => element.parentElement === document.body)).toBe(true);
  await expectInViewport(page, 'MacBook Pro');
  // Fingers get rows of at least 44px.
  expect(await menu.getByRole('menuitem').evaluateAll(items => Math.min(...items.map(item => item.getBoundingClientRect().height)))).toBeGreaterThanOrEqual(44);
  await menu.getByRole('menuitem', { name: '重命名', exact: true }).click();
  const rename = page.getByRole('dialog', { name: '重命名设备', exact: true });
  await rename.getByRole('textbox', { name: '设备名称', exact: true }).fill('Kitchen Mac');
  await rename.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Kitchen Mac', exact: true })).toBeVisible();
  expect(await where(page)).toBe('/desktop/devices');
  // A long press on a card is the menu too.
  const box = (await page.getByRole('article', { name: 'Windows Studio', exact: true }).boundingBox())!;
  await page.getByRole('article', { name: 'Windows Studio', exact: true }).locator('.pd-card-readings').dispatchEvent('touchstart', { touches: [{ identifier: 0, clientX: box.x + 40, clientY: box.y + 60 }] });
  await expect(menuOf(page, 'Windows Studio')).toBeVisible({ timeout: 2000 });
  await page.keyboard.press('Escape');
  await page.getByRole('link', { name: '打开 Linux Server 的工作区', exact: true }).click();
  await expect(page).toHaveURL(/\/devices\/device-2\/workspaces/);
  await expect(page.locator('[data-page="workspaces"] .admin-device-state.is-offline')).toContainText('离线');
  await page.context().close();
});

test('menus, the device page and its settings in every theme, light and dark, desktop and phone', async ({ page, browser }) => {
  test.setTimeout(420_000);
  mkdirSync(evidence, { recursive: true });
  for (const width of [1440, 390]) {
    const phone = width < 640;
    // One page per width: a seeded page keeps its host (addInitScript adds up).
    const view = phone ? await (await browser.newContext({ viewport: { width, height: 900 }, hasTouch: true })).newPage() : page;
    view.setDefaultTimeout(15_000);
    await view.setViewportSize({ width, height: 900 });
    for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) {
      await prepare(view, pack, dark, !phone);
      const name = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
      await expect(view.locator('html')).toHaveAttribute('data-theme-preset', pack);
      await trigger(view, 'MacBook Pro').first().click();
      await expect(menuOf(view, 'MacBook Pro').getByRole('menuitem', { name: '移除…', exact: true })).toBeVisible();
      await expectInViewport(view, 'MacBook Pro');
      await view.screenshot({ path: `${evidence}/device-menu-${name}.png` });
      // "Settings…" opens the device page's Settings tab.
      await menuOf(view, 'MacBook Pro').getByRole('menuitem', { name: '设置…', exact: true }).click();
      await expect(view.getByRole('button', { name: '替换设备令牌', exact: true })).toBeVisible();
      await view.screenshot({ path: `${evidence}/device-settings-${name}.png` });
      // The device page's one ⋯, in its header.
      await go(view, '/desktop/devices/device-0');
      await view.getByTestId('desktop-devices').getByRole('button', { name: 'MacBook Pro 的更多操作', exact: true }).click();
      await expectInViewport(view, 'MacBook Pro');
      await view.screenshot({ path: `${evidence}/device-page-menu-${name}.png` });
      await view.keyboard.press('Escape');
      await go(view, '/devices/device-0/workspaces');
      await trigger(view, 'Pockymoe').click();
      await expect(menuOf(view, 'Pockymoe').getByRole('menuitem', { name: '移除…', exact: true })).toBeVisible();
      await expectInViewport(view, 'Pockymoe');
      await view.screenshot({ path: `${evidence}/workspace-menu-${name}.png` });
      await view.keyboard.press('Escape');
      await view.getByRole('article', { name: 'Pockymoe', exact: true }).getByRole('button', { name: '已隔离', exact: true }).click();
      await expect(view.getByRole('dialog', { name: '已隔离', exact: true })).toBeVisible();
      await view.screenshot({ path: `${evidence}/workspace-isolated-${name}.png` });
      await view.keyboard.press('Escape');
    }
    if (phone) await view.context().close();
  }
});
