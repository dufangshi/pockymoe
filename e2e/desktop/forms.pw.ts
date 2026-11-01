import { expect, test } from '@playwright/test';
import { seed } from './fixtures';
const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop/webnav';

for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 390]) {
  test(`custom form controls ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem('pockymoe.phoneDock', 'off'));
    // Freeze dev HMR for each case while peers edit this shared worktree.
    await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:\d+\/\?token=/, socket => socket.onMessage(() => undefined));
    await page.route('**/relay/api/config/runtime', route => route.fulfill({ json: { defaultBackend: 'acp', agentBackends: [] } }));
    await page.route('**/relay/api/plugins*', route => route.fulfill({ json: [] }));
    await page.route('**/relay/account/workbench**', route => route.fulfill({ json: { threads: [], notifications: [] } }));
    await seed(page, pack, dark, false, true);
    await expect(page.locator('.desktop-map-scene .pd-page')).toBeVisible();
    if (width < 640) {
      const tools = page.locator('.desktop-map-scene .matter-topbar [data-icon=settings]').locator('..');
      if (await tools.isVisible()) await tools.click();
      else {
        await page.getByRole('button', { name: '打开导航', exact: true }).first().click();
        await page.getByRole('button', { name: '设置', exact: true }).first().click();
      }
    } else await page.locator('.desktop-map-scene .matter-rail [data-icon=settings]').locator('..').click();
    const dialog = page.getByTestId('settingsDialog'); await expect(dialog).toBeVisible();
    await page.getByRole('tab', { name: '偏好设置', exact: true }).click();
    const language = dialog.getByRole('combobox', { name: '语言', exact: true }); await language.click();
    const popup = page.getByRole('listbox', { name: '语言', exact: true }); await expect(popup).toBeVisible();
    await expect(popup.getByRole('option', { name: '简体中文', exact: true })).toBeFocused();
    const bounds = await popup.boundingBox(); expect(bounds!.x).toBeGreaterThanOrEqual(0); expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    const panelMatches = await popup.evaluate(element => {
      const probe = document.createElement('span'); probe.style.background = 'var(--theme-panel)'; element.append(probe);
      const expected = getComputedStyle(probe).backgroundColor; probe.remove(); return getComputedStyle(element).backgroundColor === expected;
    });
    expect(panelMatches).toBe(true);
    await page.screenshot({ path: `${evidence}/select-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    await page.keyboard.press('ArrowUp'); await page.keyboard.press('Enter');
    await expect(dialog.getByRole('combobox', { name: 'Language', exact: true })).toHaveText('English'); await expect(page.locator('html')).toHaveAttribute('lang', 'en');
    await expect(dialog).toBeVisible();
    expect(await dialog.evaluate(element => element.scrollLeft)).toBe(0);
    await expect(dialog.getByRole('slider', { name: 'Text size', exact: true })).toBeVisible();
    const appearance = await dialog.getByRole('slider', { name: 'Text size', exact: true }).evaluate(element => getComputedStyle(element).appearance);
    expect(appearance).toBe('none');
    await dialog.getByRole('slider', { name: 'Text size', exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/forms-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    await expect(page.locator('select')).toHaveCount(0);
  });
}
