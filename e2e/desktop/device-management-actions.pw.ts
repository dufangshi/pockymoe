import { expect, test } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { seed } from './fixtures';
const evidence = '.scratch/device-management/actions';
test('device management stays in app; card menu and arrow in every theme', async ({ page }) => {
  mkdirSync(evidence, { recursive: true });
  const popups: string[] = [];
  page.on('popup', popup => { popups.push('external browser'); void popup.close(); });
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:\d+\/\?token=/, socket => socket.onMessage(() => undefined));
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await seed(page, pack, dark);
    await page.evaluate(() => { (window as any).__externalAttempts = 0; window.pockymoeHost!.shell.openExternal = () => { (window as any).__externalAttempts++; }; });
    const card = page.getByRole('article', { name: 'MacBook Pro', exact: true });
    expect(await card.locator('.pd-workspaces-arrow').evaluate(el => [getComputedStyle(el, '::after').width, getComputedStyle(el, '::after').height])).toEqual(['40px', '40px']);
    await expect(card.locator('.pd-workspaces-arrow svg')).toHaveAttribute('width', '14');
    await card.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
    // The names, order, separator and danger styling belong to the shared action model.
    await expect(page.locator('.pd-device-menu [role="menuitem"]')).toHaveText(['在地图中显示', '设备详情', '打开工作区', '新建线程', '检查更新', '重命名', '共享…', '隔离…', '移除设备…']);
    await page.screenshot({ path: `${evidence}/menu-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    await page.keyboard.press('Escape');
    const offlineArrow = page.getByRole('link', { name: '打开 Linux Server 的工作区', exact: true });
    await expect(offlineArrow).not.toHaveAttribute('aria-disabled', 'true');
    await offlineArrow.click();
    await expect(page.locator('[data-page="workspaces"]')).toContainText('离线');
    await page.evaluate(() => { history.pushState({}, '', '/desktop/devices/device-0'); dispatchEvent(new PopStateEvent('popstate')); });
    const access = page.getByRole('region', { name: '访问', exact: true });
    await expect(access).toBeVisible();
    await access.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/access-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
    await page.getByRole('menuitem', { name: '共享…', exact: true }).click();
    await expect(page.getByPlaceholder('用户名或电子邮箱')).toBeVisible();
    if (width === 390) {
      const dialog = page.getByRole('dialog', { name: '共享 MacBook Pro', exact: true });
      await expect.poll(async () => Math.round((await dialog.boundingBox())!.y + (await dialog.boundingBox())!.height)).toBe(1000);
      expect((await dialog.boundingBox())!.width).toBe(390);
    }
    await page.screenshot({ path: `${evidence}/share-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
    expect(await page.evaluate(() => (window as any).__externalAttempts)).toBe(0);
    expect(popups).toEqual([]);
  }
});

test('desktop add, share, isolation, removal and account entry are reachable without a browser', async ({ page }) => {
  await seed(page);
  const popups: string[] = [];
  page.on('popup', popup => { popups.push('external browser'); void popup.close(); });
  await page.evaluate(() => { window.pockymoeHost!.shell.openExternal = () => { throw new Error('Management opened an external browser'); }; });
  await page.getByRole('button', { name: '添加设备', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '添加设备', exact: true }).getByRole('textbox', { name: '设备名称', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '共享…', exact: true }).click();
  const recipient = page.getByRole('dialog').getByPlaceholder('用户名或电子邮箱');
  await expect(recipient).toBeVisible();
  await recipient.fill('fixture-recipient');
  await expect(recipient).toBeFocused();
  await expect(recipient).toHaveValue('fixture-recipient');
  await page.keyboard.press('Escape');
  const discard = page.getByRole('dialog', { name: '放弃未保存的更改？', exact: true });
  await discard.getByRole('button', { name: '取消', exact: true }).click();
  await expect(recipient).toHaveValue('fixture-recipient');
  await page.keyboard.press('Escape');
  await discard.getByRole('button', { name: '放弃更改', exact: true }).click();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '隔离…', exact: true }).click();
  await expect(page.getByRole('checkbox')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '移除设备…', exact: true }).click();
  const confirm = page.getByRole('dialog', { name: '移除设备', exact: true });
  await expect(confirm).toContainText('MacBook Pro');
  await expect(confirm.getByRole('button', { name: '取消', exact: true })).toBeFocused();
  await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
  await page.locator('.desktop-map-scene:not([hidden]) .matter-rail').getByRole('button', { name: /账号菜单/ }).click();
  await page.getByRole('menuitem', { name: '账号设置', exact: true }).click();
  await expect(page).toHaveURL(/relay-account/);
  expect(popups).toEqual([]);
});
