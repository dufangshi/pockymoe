import { expect, test } from '@playwright/test';
const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="180" height="110"><rect width="180" height="110" fill="#879b82"/><circle cx="90" cy="55" r="30" fill="#ead6ad"/></svg>';
const image = { contentType: 'image/svg+xml', body: svg };

test('sent image survives acknowledgement, settlement and thread remount; failed images recover without restart', async ({ page }) => {
  let available = false;
  let calls = 0;
  await page.route('**/chat-image-*.png*', route => {
    calls++;
    return available ? route.fulfill(image) : route.fulfill({ status: 503, body: 'offline fixture' });
  });
  await page.goto('/chat-image-preview.html');
  await page.getByRole('button', { name: 'Send image', exact: true }).click();
  const tile = page.getByRole('region', { name: 'Sent image' });
  await expect(tile.locator('img')).toHaveJSProperty('naturalWidth', 180);
  await page.getByRole('button', { name: 'Acknowledge', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: /^running$/ })).toHaveText('running');
  // Force a new decode after acknowledgement; previously the blob was revoked.
  await tile.locator('img').evaluate(img => { const src = img.getAttribute('src')!; img.removeAttribute('src'); img.setAttribute('src', src); });
  await expect(tile.locator('img')).toHaveJSProperty('naturalWidth', 180);
  await page.getByRole('button', { name: 'Settle', exact: true }).click();
  await expect(tile.locator('.error-mark')).toBeVisible();
  await expect(page.locator('[aria-label="Inline image"] .error-mark')).toBeVisible();
  const stoppedAt = calls;
  await page.waitForTimeout(1400); // Proves exhausted retries do not keep refetching.
  expect(calls).toBe(stoppedAt);
  await tile.locator('.error-mark').click();
  await expect(page.getByRole('dialog', { name: 'Why it failed' })).toContainText('Image unavailable');
  available = true;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(tile.locator('img')).toHaveJSProperty('naturalWidth', 180);
  await page.getByRole('button', { name: 'Reconnect', exact: true }).click();
  await expect(page.locator('[aria-label="Inline image"] img')).toHaveJSProperty('naturalWidth', 180);
  await page.getByRole('button', { name: 'Switch thread', exact: true }).click();
  await page.getByRole('button', { name: 'Switch thread', exact: true }).click();
  await expect(tile.locator('img')).toHaveJSProperty('naturalWidth', 180);
  await tile.locator('.attachment-tile').click();
  await page.locator('.attachment-card-actions').getByRole('button', { name: 'View full size', exact: true }).click();
  const lightbox = page.locator('.thread-graph-image-lightbox');
  await expect(lightbox.locator('img')).toHaveJSProperty('naturalWidth', 180);
  available = false;
  await lightbox.locator('img').evaluate(img => img.dispatchEvent(new Event('error')));
  await expect(lightbox.locator('.error-mark')).toBeVisible();
  await lightbox.locator('.error-mark').click();
  available = true;
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(lightbox.locator('img')).toHaveJSProperty('naturalWidth', 180);
  available = false;
  await lightbox.locator('img').evaluate(img => img.dispatchEvent(new Event('error')));
  await expect(lightbox.locator('.error-mark')).toBeVisible();
  available = true;
  await page.getByRole('button', { name: 'Reconnect', exact: true }).evaluate(button => button.click());
  await expect(lightbox.locator('img')).toHaveJSProperty('naturalWidth', 180);
});

for (const theme of ['classic', 'plum-pocket', 'terminal']) test(`failed state in ${theme}, both modes and viewports`, async ({ page }) => {
  await page.route('**/chat-image-*.png*', route => route.fulfill({ status: 503, body: 'offline fixture' }));
  for (const mode of ['light', 'dark']) for (const device of ['desktop', 'phone']) {
    await page.setViewportSize(device === 'phone' ? { width: 390, height: 844 } : { width: 1100, height: 760 });
    await page.goto(`/chat-image-preview.html?theme=${theme}&mode=${mode}`);
    const tile = page.getByRole('region', { name: 'Sent image' });
    await expect(tile.locator('.error-mark')).toBeVisible();
    await tile.locator('.error-mark').click();
    await expect(page.getByRole('button', { name: 'Retry', exact: true })).toBeVisible();
    await expect(page.locator('button button')).toHaveCount(0);
    await page.screenshot({ path: `.scratch/chat-image/${theme}-${mode}-${device}.png` });
    await page.keyboard.press('Escape');
    await tile.locator('.attachment-tile').click();
    await page.locator('.attachment-card-actions').getByRole('button', { name: 'View full size', exact: true }).click();
    const lightbox = page.locator('.thread-graph-image-lightbox');
    await expect(lightbox.locator('.error-mark')).toBeVisible();
    await lightbox.locator('.error-mark').click();
    await expect(page.getByRole('button', { name: 'Retry', exact: true })).toBeVisible();
    await page.screenshot({ path: `.scratch/chat-image/lightbox-${theme}-${mode}-${device}.png` });
  }
});

test('pre-fix cleanup reproduces a revoked just-sent blob on acknowledgement', async ({ page }) => {
  await page.route('**/chat-image-*.png*', route => route.fulfill(image));
  await page.goto('/chat-image-preview.html?legacy=1');
  await page.getByRole('button', { name: 'Send image', exact: true }).click();
  const img = page.getByRole('region', { name: 'Sent image' }).locator('img');
  await expect(img).toHaveJSProperty('naturalWidth', 180);
  const src = await img.getAttribute('src');
  expect(src).toMatch(/^blob:/);
  await page.getByRole('button', { name: 'Acknowledge', exact: true }).click();
  expect(await page.evaluate(async url => { try { await fetch(url!); return 'readable'; } catch { return 'revoked'; } }, src)).toBe('revoked');
  await page.reload();
  await expect(page.getByRole('region', { name: 'Sent image' }).locator('img')).toHaveJSProperty('naturalWidth', 180);
});
