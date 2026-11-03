import { expect, test } from '@playwright/test';
import { seed, summary } from './fixtures';
import type { StatsRange } from '../../apps/supervisor-web/src/desktop/devices/model';

const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop';
for (const pack of ['classic']) for (const dark of [false]) for (const width of [1440]) {
  test(`hourly statistics ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1100 });
    await seed(page, pack, dark);
    const name = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
    const capture = async (state: string) => {
      await page.locator('.pd-charts-grid').scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${evidence}/${name}-${state}.png` });
    };
    const turns = page.getByRole('group', { name: '今日每小时轮数，按设备堆叠', exact: true });
    const tokens = page.getByRole('group', { name: '今日每小时 tokens', exact: true });
    await expect(turns.locator('g[tabindex]')).toHaveCount(24);
    await expect(tokens.locator('g[tabindex]')).toHaveCount(24);
    await expect(tokens.locator('[data-chart-bar]')).toHaveCount(24);
    await capture('today-tokens');
    const activeHour = await turns.locator('g[tabindex]').evaluateAll(groups => groups.findIndex(group => Number(group.querySelector('[data-chart-bar]')?.getAttribute('height')) > 0));
    expect(activeHour).toBeGreaterThanOrEqual(0);
    await turns.locator('g[tabindex]').nth(activeHour).focus();
    await expect(page.getByRole('tooltip')).toContainText(`${String(activeHour).padStart(2, '0')}:00–${String(activeHour + 1).padStart(2, '0')}:00`);
    await expect(page.getByRole('tooltip')).toContainText('MacBook Pro');
    // The tip sits beside the hovered column on either side of the chart, never over it.
    for (const index of [1, 22]) {
      const column = turns.locator('g[tabindex]').nth(index);
      await column.focus();
      const [tip, hovered] = await Promise.all([page.getByRole('tooltip').boundingBox(), column.locator('rect').first().boundingBox()]);
      expect(tip!.x + tip!.width <= hovered!.x + 1 || tip!.x >= hovered!.x + hovered!.width - 1).toBe(true);
    }
    await turns.locator('g[tabindex]').nth(1).focus();
    await capture('today-tooltip');
    await page.getByRole('button', { name: '按费用', exact: true }).click();
    const costs = page.getByRole('group', { name: '今日每小时估算费用', exact: true });
    await expect(costs.locator('[data-chart-bar]')).toHaveCount(24);
    await costs.locator('g[tabindex]').nth(1).focus();
    await expect(page.getByRole('tooltip')).toContainText('$');
    await capture('today-cost');
    await page.getByRole('button', { name: '7 天', exact: true }).click();
    await page.getByRole('button', { name: '按 Token', exact: true }).click();
    const dailyTokens = page.getByRole('group', { name: '每日 tokens', exact: true });
    await expect(dailyTokens.locator('[data-chart-bar]')).toHaveCount(7);
    await expect(dailyTokens.locator('path')).toHaveCount(0);
    await capture('7d-tokens');
    await page.getByRole('button', { name: '30 天', exact: true }).click();
    await page.getByRole('button', { name: '按费用', exact: true }).click();
    const dailyCosts = page.getByRole('group', { name: '每日估算费用', exact: true });
    await expect(dailyCosts.locator('[data-chart-bar]')).toHaveCount(30);
    await dailyCosts.locator('g[tabindex]').last().focus();
    await expect(page.getByRole('tooltip')).toContainText('$7.32');
    await capture('30d-cost');
    await page.route('**/api/stats/summary*', route => {
      const url = new URL(route.request().url());
      const device = Number(url.pathname.split('/')[3]!.slice(-1));
      const value = summary(url.searchParams.get('range') as StatsRange, device);
      delete value.hourly;
      return route.fulfill({ json: value });
    });
    // The offline device retains valid hourly data. Scope this separate fallback
    // scenario to a device whose refreshed response genuinely has no hourly data.
    await page.getByRole('button', { name: '今天', exact: true }).click();
    await page.getByRole('combobox', { name: '统计设备', exact: true }).click();
    await page.getByRole('option', { name: 'MacBook Pro', exact: true }).click();
    const runtime = page.getByRole('group', { name: '今日每小时运行时长，按设备堆叠', exact: true });
    await expect(runtime.locator('g[tabindex]')).toHaveCount(24);
    await expect(page.getByText('活动采样估算 · 并行线程时长相加 · 未覆盖时段留空', { exact: true })).toBeVisible();
    const unknownCosts = page.getByRole('group', { name: '今日每小时估算费用', exact: true });
    await expect(unknownCosts.locator('[data-chart-unknown]')).toHaveCount(24);
    await expect(unknownCosts.locator('[data-chart-bar]')).toHaveCount(0);
    await capture('runtime-fallback');
    expect(await page.locator('.pd-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  });
}
