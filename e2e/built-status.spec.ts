import { expect, test } from '@playwright/test';

test('production bundle applies running rings, steady agent colour and request spinners', async ({ page }) => {
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.goto(`/e2e/fixtures/built-status.html?theme=${theme}&mode=${mode}`);
    await expect(page.locator('html')).toHaveAttribute('data-audit-ready', 'true');
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', theme);
    const avatar = page.locator('[data-audit="tabs"] .matter-thread-avatar').first();
    if (theme === 'classic') {
      await expect(page.locator('[data-audit="tabs"] .matter-status-dot[data-status="running"]')).toBeVisible();
    } else {
      await expect(avatar).toBeVisible();
      const ring = await avatar.evaluate(el => ({ shadow: getComputedStyle(el).boxShadow, token: getComputedStyle(el).getPropertyValue('--tab-ring').trim() }));
      expect(ring.token).not.toBe('');
      expect(ring.shadow).toContain('2px');
      expect(ring.shadow).toContain('4.5px');
    }
    const running = page.locator('[data-audit="status"] .matter-status-dot[data-status="running"]');
    await expect(running).toHaveCSS('opacity', '1');
    if (theme === 'terminal') await expect(running).toHaveCSS('border-top-width', '0px');
    else await expect(running).toHaveCSS('border-top-color', 'rgba(0, 0, 0, 0)');
    await expect(page.locator('[data-audit="transcript"] .thread-turn-head-dot')).toHaveCSS('opacity', '1');
    await expect(page.locator('[data-audit="thought"] .tlog-title')).toHaveCSS('background-image', 'none');
    for (const text of ['.tlog-title', '.thread-graph-history-group-verb']) {
      await expect(page.locator(`[data-audit="thought"] ${text}`)).not.toHaveCSS('color', 'rgba(0, 0, 0, 0)');
    }
    await expect(page.locator('[data-audit="map"] .thread-map-dense-tile')).not.toHaveCSS('box-shadow', 'none');
    const badge = page.locator('[data-audit="badge"] [data-running] > svg');
    const idle = page.locator('[data-audit="badge"] button:not([data-running]) > svg');
    const color = await badge.evaluate(el => getComputedStyle(el).color);
    expect(color).not.toBe(await idle.evaluate(el => getComputedStyle(el).color));
    expect(color).toBe(await badge.evaluate(el => {
      const probe = document.createElement('span'); probe.style.color = 'var(--matter-agents-glow, #20bc87)'; el.parentElement!.append(probe);
      const value = getComputedStyle(probe).color; probe.remove(); return value;
    }));
    const spinner = page.locator('[data-audit="busy"] .request-spin');
    await expect(spinner).toHaveCSS('animation-name', 'request-spin');
    const initial = await spinner.evaluate(el => getComputedStyle(el).transform);
    await expect.poll(() => spinner.evaluate(el => getComputedStyle(el).transform)).not.toBe(initial);

    await expect(badge).toHaveCSS('color', color);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    if (theme !== 'classic') await expect(avatar).not.toHaveCSS('box-shadow', 'none');
    await expect(badge).toHaveCSS('color', color);
    await expect(spinner).toHaveCSS('animation-name', 'none');
    await page.emulateMedia({ reducedMotion: 'no-preference' });
  }
});


test('thirty running threads keep no animation, status timer or beat attributes', async ({ page }) => {
  await page.addInitScript(() => {
    const stats = { scheduled: 0, fired: 0 };
    Object.assign(window, { auditTimers: stats });
    const interval = window.setInterval, timeout = window.setTimeout;
    window.setInterval = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
      stats.scheduled++;
      return interval(() => { stats.fired++; if (typeof callback === 'function') callback(...args); }, delay);
    }) as typeof window.setInterval;
    window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
      stats.scheduled++;
      return timeout(() => { stats.fired++; if (typeof callback === 'function') callback(...args); }, delay);
    }) as typeof window.setTimeout;
  });
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.goto(`/e2e/fixtures/built-status.html?idle=1&theme=${theme}&mode=${mode}`);
    await expect(page.locator('html')).toHaveAttribute('data-audit-ready', 'true');
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', theme);
    await expect(page.locator('.fixture-sidebar .matter-status-dot[data-status="running"]')).toHaveCount(30);
    await expect.poll(() => page.evaluate(() => document.getAnimations().length)).toBe(0);
    await expect(page.locator('[data-beat]')).toHaveCount(0);
    const stats = () => page.evaluate(() => (window as unknown as { auditTimers: { scheduled: number; fired: number } }).auditTimers);
    const settled = await stats();
    // Observe more than three cycles of the removed clock; no page timer may run.
    await page.waitForTimeout(1300);
    expect(await stats()).toEqual(settled);
    expect(await page.evaluate(() => document.getAnimations().length)).toBe(0);
  }
});


test('terminal cursors and Mermaid edges stay static in the production bundle', async ({ page }) => {
  await page.goto('/e2e/fixtures/built-status.html?libraries=terminal&mode=dark');
  const input = page.locator('.xterm-helper-textarea');
  await expect(input).toBeAttached();
  await input.focus();
  await input.pressSequentially('echo steady');
  await expect.poll(() => page.evaluate(() => (window as unknown as { vendorInput: string }).vendorInput)).toContain('echo steady');
  // Programs can enable cursor blink after construction. Exercise DECSET 12 and
  // blinking block/underline/bar requests through the real shell adapter.
  for (const style of [1, 3, 5]) {
    await page.evaluate(style => (window as unknown as { vendorWrite(data: string): void }).vendorWrite(`\x1b[?12h\x1b[${style} q`), style);
    await expect.poll(() => page.evaluate(() => document.getAnimations().length)).toBe(0);
    await expect(page.locator('.xterm-cursor')).toBeVisible();
  }
  await page.locator('#vendor-blur').click();
  expect(await page.evaluate(() => document.getAnimations().length)).toBe(0);

  await page.goto('/e2e/fixtures/built-status.html?libraries=mermaid&mode=dark');
  await expect(page.locator('.thread-graph-mermaid')).toHaveAttribute('data-state', 'ready');
  const edge = page.locator('.thread-graph-mermaid-canvas .edge-animation-fast');
  await expect(page.locator('.thread-graph-mermaid-canvas > svg')).toBeVisible();
  await expect(edge).toBeAttached();
  await expect(edge).toHaveCSS('animation-name', 'none');
  await expect(edge).toHaveCSS('stroke-dashoffset', '0px');
  const svg = await page.locator('.thread-graph-mermaid-canvas > svg').evaluate(el => el.outerHTML);
  const expanded = await page.context().newPage();
  try {
    await expanded.goto(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`);
    await expect(expanded.locator('.edge-animation-fast')).toHaveCSS('animation-name', 'none');
    expect(await expanded.evaluate(() => document.getAnimations().length)).toBe(0);
  } finally { await expanded.close(); }

  await page.goto('/e2e/fixtures/built-status.html?libraries=mermaid-custom&mode=dark');
  await expect(page.locator('.thread-graph-mermaid')).toHaveAttribute('data-state', 'ready');
  await expect(page.locator('.thread-graph-mermaid-canvas .moving').first()).toHaveCSS('animation-name', 'none');
  expect(await page.evaluate(() => document.getAnimations().length)).toBe(0);

});
