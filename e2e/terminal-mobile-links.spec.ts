import { test, expect } from '@playwright/test';
const web = `http://localhost:${process.env.E2E_WEB_PORT ?? 5173}`;
test.use({ actionTimeout: 15_000 });

test('real mobile link taps open the destination and the viewport still scrolls', async ({ page, context, browserName, isMobile }) => {
  test.skip(!isMobile, 'Mobile native scrolling viewport');
  await context.route('https://example.com/**', route => route.fulfill({ contentType: 'text/html', body: '<title>Mobile terminal destination</title>' }));
  await page.goto(`${web}/e2e/terminal-host.html`);
  await expect(page.locator('.xterm-rows')).toContainText('ClipboardMarkerAlpha');
  const data = Array.from({ length: 150 }, (_, i) => `scroll line ${i}\r\n`).join('') + '\x1b]8;;https://example.com/mobile\x1b\\MobileLink\x1b]8;;\x1b\\\r\n' + 'tail\r\n'.repeat(15);
  await page.evaluate(data => window.terminalFixture.output(data), data);
  const viewport = page.locator('.xterm-viewport');
  await expect.poll(() => viewport.evaluate(node => node.scrollTop)).toBeGreaterThan(300);
  await page.evaluate(async () => { await document.fonts.ready; await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))); });
  // Chromium exposes a real touch-pan stream. Mobile WebKit's Playwright
  // driver supports real taps but no pan/wheel input, so check its actual
  // viewport scroll/render path and that touchmove keeps its default action.
  if (browserName === 'chromium') {
    const before = await viewport.evaluate(node => node.scrollTop);
    const box = (await viewport.boundingBox())!;
    const x = box.x + box.width / 2, y = box.y + box.height / 2;
    const cdp = await context.newCDPSession(page);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
    for (const delta of [30, 65, 100]) {
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: y + delta }] });
      await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
    }
    const ended = viewport.evaluate(node => new Promise<void>(resolve => node.addEventListener('scrollend', () => resolve(), { once: true })));
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await ended;
    await expect.poll(() => viewport.evaluate(node => node.scrollTop)).toBeLessThan(before - 30);
  } else {
    const before = await viewport.evaluate(node => node.scrollTop);
    await viewport.evaluate(node => node.scrollBy(0, -250));
    await expect.poll(() => viewport.evaluate(node => node.scrollTop)).toBeLessThan(before - 30);
  }
  expect(await viewport.evaluate(node => node.dispatchEvent(new Event('touchmove', { bubbles: true, cancelable: true })))).toBe(true);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await viewport.evaluate(node => { node.scrollTop = node.scrollHeight; });
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  const link = page.locator('.xterm-rows > div').filter({ hasText: /^MobileLink\s*$/ });
  await expect(link).toHaveCount(1);
  const box = (await link.boundingBox())!, terminal = (await page.locator('.xterm').boundingBox())!;
  // Target the terminal, allowing its viewport child to receive the real tap.
  await page.locator('.xterm').tap({ position: { x: box.x + 30 - terminal.x, y: box.y + 7 - terminal.y } });
  const dialog = page.getByRole('dialog', { name: 'Open terminal link?' });
  await expect(dialog).toContainText('https://example.com/mobile');
  const opened = context.waitForEvent('page');
  await dialog.getByRole('button', { name: 'Open link', exact: true }).tap();
  const destination = await opened;
  await expect(destination).toHaveURL('https://example.com/mobile');
  await expect(destination).toHaveTitle('Mobile terminal destination');
});
