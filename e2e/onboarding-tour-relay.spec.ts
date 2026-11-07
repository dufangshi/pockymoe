import { test, expect } from '@playwright/test';
import { fixture, ok } from './helpers/account-key-fixture';

// A new Relay account with no devices takes the tour on the sample device
// (docs/guided-tour.md); nothing about it reaches the relay.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

test('a Relay account with no devices tours the sample device without touching the relay', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'Relay tour runs on desktop');
  test.setTimeout(240_000);
  const f = await fixture(browser, info);
  try {
    const reader = await f.account('tour-reader');
    const page = reader.page;
    const relayWrites: string[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (request.method() !== 'GET' && url.pathname.startsWith('/relay/')) relayWrites.push(`${request.method()} ${url.pathname}`);
      if (url.pathname.includes('/pockymoe-tour')) relayWrites.push(`${request.method()} ${url.pathname}`);
    });
    // A computer opens the consolidated device page; the tutorial is in the account menu.
    await page.goto(f.base + '/relay-devices');
    await expect(page).toHaveURL(/\/desktop\/devices/);
    await page.locator('button[aria-label^="Relay account menu for tour-reader"]').first().click();
    await page.getByRole('menuitem', { name: 'Tutorial' }).click();
    const hub = page.locator('#pockymoe-tour-root .pm-tour-hub');
    await hub.getByRole('button', { name: /Devices & workspaces/ }).click();
    const card = page.locator('#pockymoe-tour-root .pm-tour-card');
    const step = (id: string) => page.locator(`#pockymoe-tour-root .pm-tour-card[data-step="${id}"]`);
    const next = () => card.getByRole('button', { name: /^(Next|Done)$/ }).click();

    await expect(step('relay-add-device')).toBeVisible();
    // The form opens in a window; the tour steps aside until it closes.
    await page.locator('button[aria-controls="add-device-form"]').click();
    await expect(page.locator('#add-device-form')).toBeVisible();
    await expect(card).toHaveCount(0);
    await page.keyboard.press('Escape');
    // No devices: the list step reads on its own instead of waiting for one.
    await expect(step('relay-device-list')).toBeVisible();
    await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
    await next();
    await expect(page).toHaveURL(/\/devices\/pockymoe-tour\/workspaces$/);
    await expect(step('sample-device')).toBeVisible();
    await expect(page.locator('header.product-topbar')).toContainText('Sample device');
    await expect(page.locator('article.product-row')).toHaveCount(2);
    await card.getByRole('button', { name: 'Contents' }).click();

    // Sharing works on the sample thread and stays in the page.
    await hub.getByRole('button', { name: /Sharing/ }).click();
    await expect(page).toHaveURL(/\/devices\/pockymoe-tour\/threads\/c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e01$/);
    await page.locator('.matter-tabs-row button.matter-toolbar-toggle').click();
    await page.locator('#matter-thread-tools').getByRole('button', { name: 'Sharing permissions' }).click();
    const form = page.locator('form#thread-actions-share-form');
    await expect(step('scope')).toBeVisible();
    await form.getByPlaceholder('username or email').fill('alice');
    await page.locator('button[type="submit"][form="thread-actions-share-form"]').click();
    await expect(page.locator('.matter-actions-dialog')).toContainText('alice');
    await page.keyboard.press('Escape');
    await card.getByRole('button', { name: 'Contents' }).click();

    // A signed-in PC browser's thread page: the agent chapter finds its controls.
    await hub.getByRole('button', { name: /Agent teamwork/ }).click();
    const pane = page.getByTestId('primary-pane');
    await expect(step('delegate')).toBeVisible();
    await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
    await pane.locator('.thread-worked-toggle').first().click();
    await expect(step('wake')).toBeVisible();
    await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
    await next();
    await expect(step('children')).toBeVisible();
    await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
    await next();
    await expect(page).toHaveURL(/\/threads\/c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e02$/);
    await expect(step('envelope')).toBeVisible();
    await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
    await card.getByRole('button', { name: 'Close tutorial' }).click();
    await expect(page).toHaveURL(/\/desktop\/devices/);

    expect(relayWrites.filter((entry) => !/^POST \/relay\/account\/(preferences|browsers)/.test(entry))).toEqual([]);
    const portal = ok(await f.api('/relay/portal', 'GET', undefined, reader.cookie), 'Portal');
    expect(portal.devices).toEqual([]);
    expect(portal.sharedByMe).toEqual([]);
    const workbench = ok(await f.api('/relay/account/workbench', 'GET', undefined, reader.cookie), 'Workbench');
    expect(JSON.stringify(workbench)).not.toContain('pockymoe-tour');
    expect(await page.evaluate(() => localStorage.getItem('remote-codex-relay-device-id'))).toBeNull();
  } finally {
    await f.close();
  }
});
