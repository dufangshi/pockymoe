import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { seed } from './fixtures';

// Issue #112: on a device's own page the settings window read and saved the
// device remembered in localStorage, and never named the device it acted on.
const evidence = '.scratch/screens/settings-device';
const go = (page: Page, path: string) => page.evaluate(target => { history.pushState({}, '', target); dispatchEvent(new PopStateEvent('popstate')); }, path);

async function prepare(page: Page, pack = 'classic', dark = false, host = true) {
  await page.addInitScript(() => {
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
    // The device this browser opened last, as "Open" leaves it.
    localStorage.setItem('remote-codex-relay-device-id', 'device-1');
  });
  await seed(page, pack, dark, false, false, host);
  // Each device answers with its own values, so a wrong target is visible.
  await page.route('**/api/config/workspace-settings', async route => {
    const device = new URL(route.request().url()).pathname.split('/')[3]!;
    const saved = route.request().method() === 'PATCH' ? route.request().postDataJSON() : {};
    await route.fulfill({ json: { workspaceRoot: `/root-of-${device}`, devHome: `/dev-of-${device}`, ...saved } });
  });
  await page.route('**/api/environments', route => route.fulfill({ json: { connections: [], environments: [], budget: { maxRunning: 3, maxCpu: 8, maxMemoryBytes: 8 * 2 ** 30, reservedCpu: 0, reservedMemoryBytes: 0 } } }));
  await page.route('**/api/environment-requests*', route => route.fulfill({ json: [] }));
}
function deviceRequests(page: Page) {
  const seen: string[] = [];
  page.on('request', request => {
    // What only the settings window asks a device (the cards behind it poll management/supervisor for every device).
    const match = /\/relay\/devices\/([^/]+)\/api\/(config\/workspace-settings|management\/(?:harnesses|connections|jobs|upstreams|templates)|harness-resources\/settings|environments|plugins)/.exec(new URL(request.url()).pathname);
    if (match) seen.push(`${request.method()} ${match[1]} ${match[2]}`);
  });
  return seen;
}
async function openSettings(page: Page) {
  // The rail's avatar on a computer, the header's on a phone: the same account menu.
  await page.getByRole('button', { name: /账号菜单/ }).and(page.locator(':visible')).first().click();
  await page.getByRole('menuitem', { name: '设置', exact: true }).click();
  const dialog = page.getByTestId('settingsDialog');
  await expect(dialog).toBeVisible();
  return dialog;
}

test('a device page\'s settings read and save that device, not the remembered one', async ({ page }) => {
  await prepare(page);
  const seen = deviceRequests(page);
  // Issue #115: a device's own settings are its page's Settings tab.
  await go(page, '/desktop/devices/device-0?tab=settings&section=workspace');
  const content = page.getByTestId('desktop-devices');
  const devHome = content.getByLabel('开发目录');
  await expect(devHome).toHaveValue('/dev-of-device-0');
  await devHome.fill('/elsewhere');
  await content.getByRole('button', { name: '保存工作区默认设置', exact: true }).click();
  await expect(content.getByText('已保存工作区默认设置。', { exact: true })).toBeVisible();
  // Development renders effects twice; what matters is where each request went.
  expect([...new Set(seen.filter(entry => entry.includes('workspace-settings')))]).toEqual(['GET device-0 config/workspace-settings', 'PATCH device-0 config/workspace-settings']);
  for (const tab of ['Harness', '上游', '资源', '隔离环境', 'Supervisor']) {
    await content.getByRole('tab', { name: tab, exact: true }).click();
    await expect(content).not.toContainText('请打开设备');
  }
  // The window keeps the device's plugins, named.
  const dialog = await openSettings(page);
  await dialog.getByRole('tab', { name: '插件', exact: true }).click();
  const bar = dialog.getByRole('group', { name: '这些设置作用的设备', exact: true });
  await expect(bar).toContainText('MacBook Pro');
  await expect(bar).toContainText('在线');
  await expect.poll(() => seen).toContain('GET device-0 plugins');
  expect(seen.some(entry => entry.includes('management/'))).toBe(true);
  expect([...new Set(seen.map(entry => entry.split(' ')[1]))]).toEqual(['device-0']);
});

test('plugins on a page without a device ask which one instead of a dead end', async ({ page }) => {
  await prepare(page);
  const seen = deviceRequests(page);
  const dialog = await openSettings(page);
  await dialog.getByRole('tab', { name: '插件', exact: true }).click();
  const bar = dialog.getByRole('group', { name: '这些设置作用的设备', exact: true });
  const choice = bar.getByRole('combobox');
  await expect(choice).toHaveText('选择设备');
  await expect(dialog).toContainText('请选择要管理的设备');
  expect(seen).toEqual([]);
  await choice.click();
  await page.getByRole('option', { name: 'Windows Studio', exact: true }).click();
  await expect(choice).toHaveText('Windows Studio');
  await expect.poll(() => seen).toContain('GET device-1 plugins');
  expect([...new Set(seen.map(entry => entry.split(' ')[1]))]).toEqual(['device-1']);
});

// One page per width: a seeded page keeps its host (addInitScript adds up); a
// phone is a plain browser on the same device pages.
for (const width of [1440, 390]) test(`the device bar in every theme, light and dark, at ${width}px`, async ({ page }) => {
  test.setTimeout(240_000); page.setDefaultTimeout(15_000);
  mkdirSync(evidence, { recursive: true });
  const phone = width < 640;
  await page.setViewportSize({ width, height: 900 });
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) {
    await prepare(page, pack, dark, !phone);
    const name = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
    await go(page, phone ? '/devices/device-0/workspaces' : '/desktop/devices/device-0');
    let dialog = await openSettings(page);
    await dialog.getByRole('tab', { name: '插件', exact: true }).click();
    const bar = dialog.getByRole('group', { name: '这些设置作用的设备', exact: true });
    await expect(bar).toContainText('MacBook Pro');
    // The mark is drawn, not just named: a dot of its own size beside the state word.
    expect(await bar.locator('.admin-device-state-mark').evaluate(mark => mark.getBoundingClientRect().width)).toBeGreaterThan(6);
    const box = (await bar.boundingBox())!, panel = (await dialog.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(panel.x); expect(box.x + box.width).toBeLessThanOrEqual(panel.x + panel.width + 1);
    await page.screenshot({ path: `${evidence}/named-${name}.png` });
    await page.keyboard.press('Escape');
    await go(page, '/desktop/devices');
    dialog = await openSettings(page);
    await dialog.getByRole('tab', { name: '插件', exact: true }).click();
    await expect(dialog.getByRole('group', { name: '这些设置作用的设备', exact: true }).getByRole('combobox')).toHaveText('选择设备');
    await page.screenshot({ path: `${evidence}/choose-${name}.png` });
    await page.keyboard.press('Escape');
  }
});
