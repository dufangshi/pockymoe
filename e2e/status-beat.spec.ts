import { expect, test } from '@playwright/test';

test('status indicators share discrete phases in every theme and stop for reduced motion or unmount', async ({ page }) => {
  await page.clock.install({ time: 0 });
  await page.clock.pauseAt(1000);
  for (const theme of ['classic', 'plum-pocket', 'terminal']) {
    await page.goto(`/e2e/fixtures/status-beat.html?theme=${theme}&mode=dark`);
    const root = page.locator('html');
    await expect(root).toHaveAttribute('data-theme-preset', theme);
    await expect(root).toHaveAttribute('data-status-beat', '0');
    const tile = page.locator('.thread-map-dense-tile[data-running]').first();
    await expect(page.locator('.thread-map-dense-tile[data-running]')).toHaveCount(6);
    await expect(page.locator('.matter-group-tab > .matter-status-dot[data-status="running"]')).toHaveCount(5);
    const mark = page.locator('.fixture-sidebar .matter-status-dot[data-status="running"]');
    const states = () => page.evaluate(() => ({
      shadow: getComputedStyle(document.querySelector('.thread-map-dense-tile[data-running]')!).boxShadow,
      marks: [...document.querySelectorAll('.matter-status-dot[data-status="running"]')].map(el => ({ opacity: getComputedStyle(el).opacity, transform: getComputedStyle(el).transform })),
      animations: document.getAnimations().filter(animation => animation.effect?.getTiming().iterations === Infinity).length,
    }));
    const on = await states();
    expect(on.animations).toBe(0);
    await page.clock.runFor(399);
    expect(await states()).toEqual(on);
    await page.clock.runFor(1);
    await expect(root).toHaveAttribute('data-status-beat', '1');
    expect((await states()).shadow).toBe(on.shadow);
    if (theme !== 'terminal') await expect(mark).not.toHaveCSS('transform', on.marks[0]!.transform);
    await page.clock.runFor(400);
    await expect(root).toHaveAttribute('data-status-beat', '2');
    await expect(tile).not.toHaveCSS('box-shadow', on.shadow);
    if (theme === 'terminal') {
      const off = await states();
      expect(off.marks.every(mark => mark.opacity === '0')).toBe(true);
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(root).not.toHaveAttribute('data-status-beat');
    const still = await states();
    await page.clock.runFor(3200);
    expect(await states()).toEqual(still);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await expect(root).toHaveAttribute('data-status-beat', '0');
    await page.evaluate(() => { document.getElementById('root')!.replaceChildren(); });
    await expect(root).not.toHaveAttribute('data-status-beat');
  }
});
