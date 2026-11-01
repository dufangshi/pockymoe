import { expect, test, type Page } from '@playwright/test';
import { seed } from './fixtures';

const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop';
async function switchLanguage(page: Page, locale: 'en' | 'zh-CN') {
  const english = await page.locator('html').getAttribute('lang') === 'en';
  await page.locator('.matter-rail').getByRole('button', { name: english ? 'Settings' : '设置', exact: true }).click();
  const settings = page.getByTestId('settingsDialog');
  await settings.getByRole('tab', { name: english ? 'Preferences' : '偏好设置', exact: true }).click();
  await settings.getByRole('combobox', { name: english ? 'Language' : '语言', exact: true }).selectOption(locale);
  await expect(page.locator('.pd-page h1')).toHaveText(locale === 'en' ? 'Devices' : '设备', { timeout: 1000 });
  await expect.poll(() => page.evaluate(() => (window as unknown as { deviceLocaleEvents: { locale: string }[] }).deviceLocaleEvents.at(-1)?.locale), { timeout: 1000, intervals: [20, 50, 100] }).toBe(locale);
  await page.keyboard.press('Escape');
  await expect(settings).toHaveCount(0);
}
async function prepare(page: Page, pack: string, dark: boolean) {
  await page.addInitScript(() => {
    const events: { locale: string; at: number }[] = [];
    Object.assign(window, { deviceLocaleEvents: events });
    window.addEventListener('desktopMock:appearance', event => {
      const appearance = (event as CustomEvent<{ locale: string }>).detail;
      events.push({ locale: appearance.locale, at: performance.now() });
    });
  });
  await seed(page, pack, dark);
  await page.locator('.pd-page').evaluate(element => { element.setAttribute('data-locale-preserved', 'yes'); });
}
for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 900]) {
  test(`one shared language ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1100 });
    await prepare(page, pack, dark);
    for (const locale of ['en', 'zh-CN'] as const) {
      const english = locale === 'en', name = `${pack}-${dark ? 'dark' : 'light'}-${width}-${locale}`;
      await switchLanguage(page, locale);
      await expect(page.locator('.pd-page')).toHaveAttribute('data-locale-preserved', 'yes');
      const offline = page.getByRole('article', { name: 'Linux Server', exact: true });
      await expect(offline).toContainText(english ? 'Cached · 3 hours ago' : '缓存 · 3 小时前');
      await expect(page.getByRole('heading', { name: english ? 'Usage statistics' : '使用统计', exact: true })).toBeVisible();
      if (english) {
        // Fixture names/titles are English; real user content is never translated.
        expect(await page.locator('.pd-page').innerText()).not.toMatch(/\p{Script=Han}/u);
        expect(await page.locator('.desktop-online-count').innerText()).not.toMatch(/\p{Script=Han}/u);
      }
      await page.locator('.pd-page-header').scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${evidence}/${name}-overview.png` });
      const chart = page.getByRole('group', { name: english ? 'Turns by hour today, stacked by device' : '今日每小时轮数，按设备堆叠', exact: true });
      await chart.scrollIntoViewIfNeeded(); await chart.locator('g[tabindex]').nth(1).focus();
      await expect(page.getByRole('tooltip')).toContainText(english ? 'turn' : '轮');
      await page.screenshot({ path: `${evidence}/${name}-statistics.png` });
      await page.getByRole('button', { name: english ? 'Actions for MacBook Pro' : 'MacBook Pro 的操作', exact: true }).click();
      await expect(page.getByRole('menuitem', { name: english ? 'Show in map' : '在地图中显示', exact: true })).toBeVisible();
      await page.screenshot({ path: `${evidence}/${name}-menu.png` });
      await page.getByRole('menuitem', { name: english ? 'Rename' : '重命名', exact: true }).click();
      await expect(page.getByRole('dialog', { name: english ? 'Rename device' : '重命名设备', exact: true })).toBeVisible();
      await expect(page.getByRole('textbox', { name: english ? 'Device name' : '设备名称', exact: true })).toHaveValue('MacBook Pro');
      await page.screenshot({ path: `${evidence}/${name}-rename.png` });
      const input = page.getByRole('textbox', { name: english ? 'Device name' : '设备名称', exact: true });
      await input.fill('Temporary draft');
      await page.keyboard.press('Escape');
      const discard = page.getByRole('dialog', { name: english ? 'Discard name changes?' : '放弃名称修改？', exact: true });
      await expect(discard).toBeVisible();
      await page.screenshot({ path: `${evidence}/${name}-discard.png` });
      await discard.getByRole('button', { name: english ? 'Cancel' : '取消', exact: true }).click();
      await expect(input).toHaveValue('Temporary draft');
      await input.fill('MacBook Pro');
      await page.keyboard.press('Escape');
      await page.getByRole('article', { name: 'MacBook Pro', exact: true }).locator('.device-monitor-button').click();
      await expect(page.getByRole('dialog', { name: english ? 'Device monitor' : '设备监控', exact: true })).toBeVisible();
      await page.screenshot({ path: `${evidence}/${name}-monitor.png` });
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: english ? 'Update 0.13.42' : '有更新 0.13.42', exact: true }).click();
      await expect(page.getByRole('dialog')).toContainText(english ? "Check versions and manage this device's runtime." : '检查版本并管理这台设备的运行环境。');
      await page.screenshot({ path: `${evidence}/${name}-update.png` });
      await page.keyboard.press('Escape');
      expect(await page.locator('.pd-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
      expect(await page.locator('.pd-device-card').evaluateAll(cards => cards.every(card => card.scrollWidth <= card.clientWidth))).toBe(true);
    }
  });
}

test('saved notice and existing product errors follow a Settings language change', async ({ page }) => {
  await prepare(page, 'classic', false);
  await switchLanguage(page, 'en');
  await page.getByRole('button', { name: 'Actions for MacBook Pro', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
  await page.getByRole('textbox', { name: 'Device name', exact: true }).fill('Studio Mac');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Device name saved' })).toBeVisible();
  await switchLanguage(page, 'zh-CN');
  await expect(page.getByRole('status').filter({ hasText: '设备名称已保存' })).toBeVisible();
  await page.route(/\/relay\/devices\/device-[0-2]$/, route => route.fulfill({ status: 405, json: { code: 'unsupported', message: 'Legacy Relay' } }));
  await page.getByRole('button', { name: 'Studio Mac 的操作', exact: true }).click();
  await page.getByRole('menuitem', { name: '重命名', exact: true }).click();
  await page.getByRole('textbox', { name: '设备名称', exact: true }).fill('Other Mac');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '重命名设备' })).toHaveCount(0);
  await switchLanguage(page, 'en');
  await page.locator('.pd-header-actions .error-mark').click();
  await expect(page.locator('.error-mark-bubble')).toContainText('This Relay does not support renaming devices yet.');
  await page.screenshot({ path: `${evidence}/saved-error-en.png` });
  await page.keyboard.press('Escape');
  await switchLanguage(page, 'zh-CN');
  await page.locator('.pd-header-actions .error-mark').click();
  await expect(page.locator('.error-mark-bubble')).toContainText('当前 Relay 暂不支持重命名设备。');
  await page.screenshot({ path: `${evidence}/saved-error-zh-CN.png` });
});
