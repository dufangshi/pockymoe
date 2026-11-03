import { expect, test } from '@playwright/test';

test('phone dialog filters, preserves zoom, folds once and keeps readable touch targets', async ({ page }) => {
  await page.goto('/map.html?presentation=dialog&dataset=phone&chrome=0&lang=en&theme=terminal&mode=dark');
  const map = page.locator('.thread-map[data-phone]');
  await expect(map).toBeVisible();
  await expect(map.locator('.thread-map-stage > .thread-map-section')).toHaveCount(11);
  const sizes = () => map.locator('.thread-map-section, .thread-map-tile').evaluateAll(items => items.map(item => { const r = item.getBoundingClientRect(); return { width: r.width, height: r.height }; }));
  await expect.poll(async () => (await sizes()).every(size => size.width >= 96 && size.height >= 44)).toBe(true);
  expect(await map.locator('.thread-map-stage').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  expect(await map.locator('.thread-map-section').evaluateAll(items => items.every(item => [...item.children].filter(child => child.hasAttribute('data-phone-fold')).length <= 1))).toBe(true);
  await map.locator('[data-phone-fold]').first().tap();
  await expect(map.getByRole('button', { name: 'Up one level' })).toBeVisible();
  const title = await map.locator('.thread-map-phone-title').innerText();
  const search = map.getByRole('searchbox');
  await search.fill('APP');
  await expect(map.locator('[data-dimmed]')).toHaveCount(0);
  const titleFilter = await map.locator('.thread-map-phone-title').innerText();
  expect(titleFilter).toBe('Thread map');
  await search.fill('地图');
  await expect(map.getByRole('status')).toContainText('matches');
  await search.fill('no-such-thread-xyz');
  await expect(map.locator('.thread-map-empty')).toContainText('no-such-thread-xyz');
  await expect(map.locator('.thread-map-section, .thread-map-tile')).toHaveCount(0);
  await map.locator('.thread-map-empty').getByRole('button', { name: 'Clear search' }).tap();
  await expect(map.locator('.thread-map-phone-title')).toHaveText(title);
  await map.getByRole('button', { name: 'Map actions' }).tap();
  await expect(page.getByRole('menu', { name: 'Map actions' })).toBeVisible();
  await page.getByRole('menuitemradio', { name: 'All', exact: true }).tap();
  await expect(page.getByRole('menu')).toHaveCount(0);
  // Zoom into a workspace and open a real tile with one tap.
  const workspace = map.locator('.thread-map-section[data-kind="workspace"] .thread-map-section-head').first();
  await workspace.tap();
  const tile = map.locator('.thread-map-tile-link').first();
  await expect(tile).toBeVisible();
  await expect.poll(async () => (await sizes()).every(size => size.width >= 96 && size.height >= 44)).toBe(true);
  await tile.tap();
  await expect(page.locator('.map-fixture-notice')).toContainText('打开');
});
