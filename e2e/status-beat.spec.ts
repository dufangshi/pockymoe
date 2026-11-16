import { expect, test } from '@playwright/test';

test('status indicators share discrete phases in every theme and mode and stop for reduced motion or unmount', async ({ page }) => {
  await page.clock.install({ time: 0 });
  await page.clock.pauseAt(1000);
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.goto(`/e2e/fixtures/status-beat.html?theme=${theme}&mode=${mode}`);
    const root = page.locator('html');
    const mark = page.locator('.fixture-sidebar .matter-status-dot[data-status="running"]').first();
    await expect(root).toHaveAttribute('data-theme-preset', theme);
    await expect(mark).toBeVisible();
    await page.clock.runFor(16);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await expect(mark).toHaveAttribute('data-beat', '0');
    await expect(root).not.toHaveAttribute('data-status-beat');
    await expect(root).not.toHaveAttribute('data-beat');
    expect(await page.locator('*').count()).toBeGreaterThanOrEqual(2500);
    expect(await page.locator('[data-beat]').count()).toBeGreaterThanOrEqual(30);
    const tile = page.locator('.thread-map-dense-tile[data-running]').first();
    await expect(page.locator('.thread-map-dense-tile[data-running]')).toHaveCount(6);
    await expect(page.locator('.matter-group-tab > .matter-status-dot[data-status="running"]')).toHaveCount(5);
    const states = () => page.evaluate(() => ({
      shadow: getComputedStyle(document.querySelector('.thread-map-dense-tile[data-running]')!).boxShadow,
      marks: [...document.querySelectorAll('.matter-status-dot[data-status="running"]')].map(el => ({ opacity: getComputedStyle(el).opacity, transform: getComputedStyle(el).transform })),
      animations: document.getAnimations().filter(animation => animation.effect?.getTiming().iterations === Infinity).length,
    }));
    const inPhase = async (step: string) => {
      const steps = await page.locator('[data-beat]').evaluateAll(elements => [...new Set(elements.map(el => el.getAttribute('data-beat')))]);
      expect(steps).toEqual([step]);
    };
    const on = await states();
    expect(on.animations).toBe(0);
    await inPhase('0');
    await page.clock.runFor(399);
    expect(await states()).toEqual(on);
    await page.clock.runFor(1);
    await expect(mark).toHaveAttribute('data-beat', '1');
    await inPhase('1');
    expect((await states()).shadow).toBe(on.shadow);
    if (theme !== 'terminal') await expect(mark).not.toHaveCSS('transform', on.marks[0]!.transform);
    await page.clock.runFor(400);
    await expect(mark).toHaveAttribute('data-beat', '2');
    await inPhase('2');
    await expect(tile).not.toHaveCSS('box-shadow', on.shadow);
    if (theme === 'terminal') expect((await states()).marks.every(mark => mark.opacity === '0')).toBe(true);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(page.locator('[data-beat]')).toHaveCount(0);
    const still = await states();
    await page.clock.runFor(3200);
    expect(await states()).toEqual(still);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await expect(mark).toHaveAttribute('data-beat', '0');
    await page.evaluate(() => { document.getElementById('root')!.replaceChildren(); });
    await page.clock.runFor(16);
    await expect(page.locator('[data-beat]')).toHaveCount(0);
  }
});
