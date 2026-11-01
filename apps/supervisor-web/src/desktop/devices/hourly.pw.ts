import { expect, test } from '@playwright/test';
import { seed, summary } from './fixtures';
import type { StatsRange } from './model';

const evidence = '/tmp/rc-desktop/evidence/devices/v21';
for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 900]) {
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
    await turns.locator('g[tabindex]').nth(1).focus();
    await expect(page.getByRole('tooltip')).toContainText('01:00–02:00');
    await expect(page.getByRole('tooltip')).toContainText('MacBook Pro');
    await capture('today-tooltip');
    await page.getByRole('button', { name: '费用', exact: true }).click();
    const costs = page.getByRole('group', { name: '今日每小时估算费用', exact: true });
    await expect(costs.locator('[data-chart-bar]')).toHaveCount(24);
    await costs.locator('g[tabindex]').nth(1).focus();
    await expect(page.getByRole('tooltip')).toContainText('≈ $');
    await capture('today-cost');
    await page.getByRole('button', { name: '7 天', exact: true }).click();
    await page.getByRole('button', { name: 'Tokens', exact: true }).click();
    const dailyTokens = page.getByRole('group', { name: '每日 tokens', exact: true });
    await expect(dailyTokens.locator('[data-chart-bar]')).toHaveCount(7);
    await expect(dailyTokens.locator('path')).toHaveCount(0);
    await capture('7d-tokens');
    await page.getByRole('button', { name: '30 天', exact: true }).click();
    await page.getByRole('button', { name: '费用', exact: true }).click();
    const dailyCosts = page.getByRole('group', { name: '每日估算费用', exact: true });
    await expect(dailyCosts.locator('[data-chart-bar]')).toHaveCount(30);
    await dailyCosts.locator('g[tabindex]').last().focus();
    await expect(page.getByRole('tooltip')).toContainText('≈ $7.32');
    await capture('30d-cost');
    await page.route('**/api/stats/summary*', route => {
      const url = new URL(route.request().url());
      const device = Number(url.pathname.split('/')[3]!.slice(-1));
      const value = summary(url.searchParams.get('range') as StatsRange, device);
      delete value.hourly;
      return route.fulfill({ json: value });
    });
    await page.getByRole('button', { name: '今天', exact: true }).click();
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
