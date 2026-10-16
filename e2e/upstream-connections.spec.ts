import { randomUUID } from 'node:crypto';
import { expect, test, type Locator, type Page } from '@playwright/test';

// A saved connection goes through the real local Supervisor: save, failed model
// discovery with a typed model, inline test, persistence across reload, and reuse
// from Content Quality's settings. Only the harness inventory and Apply are routed,
// so the run never rewrites the operator's native harness configuration.

test.use({ actionTimeout: 15_000 });

async function openSettings(page: Page, tab: string) {
  await page.goto('/workspaces');
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: tab, exact: true }).click();
}

async function expectInsideViewport(page: Page, element: Locator) {
  const box = (await element.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(-1);
  expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
}

test('a real local connection saves without applying and Content Quality reuses the same form', async ({
  page,
}, testInfo) => {
  const name = `Local ${randomUUID().slice(0, 8)}`;
  const applied: unknown[] = [];
  await page.route('**/api/management/harnesses', (route) =>
    route.fulfill({
      json: [{ id: 'codex', name: 'OpenAI Codex', base: { installed: true } }],
    }),
  );
  await page.route('**/api/management/connections/*/apply', (route) => {
    applied.push(route.request().postDataJSON());
    return route.fulfill({ json: { ok: true, profileId: 'p' } });
  });

  await openSettings(page, 'Upstreams');
  await page
    .getByRole('button', { name: 'Add connection', exact: true })
    .click();
  const editor = page.getByRole('dialog', { name: 'Add connection' });
  await editor.getByLabel('Name', { exact: true }).fill(name);
  await editor.getByLabel('API format').selectOption('openai_responses');
  // Nothing listens on the discard port: discovery and the test fail for real.
  await editor.getByLabel('Base URL').fill('http://127.0.0.1:9/v1');
  await editor.getByLabel('API key').fill('synthetic-key');
  await editor.getByRole('button', { name: 'Find models' }).click();
  await expect(editor.getByText(/Couldn't list models/)).toBeVisible();
  await editor.getByLabel('Default model').fill('manual-model');
  await editor.getByRole('button', { name: 'Test connection' }).click();
  await expect(
    editor.getByRole('alert').filter({ hasNotText: "Couldn't list models" }),
  ).toBeVisible();
  await editor.getByRole('button', { name: 'Save connection' }).click();
  await expect(editor).toBeHidden();
  await expect(
    page.getByText(`${name} saved. Nothing was applied.`),
  ).toBeVisible();
  expect(applied).toEqual([]);

  await openSettings(page, 'Upstreams');
  const row = page.getByRole('listitem', { name });
  await expect(row).toContainText('Responses · 127.0.0.1:9/v1 · manual-model');
  await row
    .getByRole('button', { name: `Apply ${name} to OpenAI Codex` })
    .click();
  const apply = page.getByRole('dialog', {
    name: `Apply ${name} to OpenAI Codex`,
  });
  await expect(apply.getByLabel('Default model')).toHaveValue('manual-model');
  await apply.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(apply).toBeHidden();
  expect(applied).toEqual([{ harness: 'codex', model: 'manual-model' }]);

  await page.getByRole('tab', { name: 'Plugins', exact: true }).click();
  await page.getByRole('button', { name: 'Content Quality settings' }).click();
  const plugin = page.getByTestId('pluginSettingsDialog');
  await plugin.getByLabel('Reader background').fill('Keep me');
  const picker = plugin.getByLabel('Saved connection');
  await expect(picker.locator('option', { hasText: name })).toHaveCount(1);
  // Cancelling a dirty nested form asks first and keeps the plugin form intact.
  await plugin.getByRole('button', { name: 'Add connection' }).click();
  const nested = page.getByRole('dialog', { name: 'Add connection' });
  await nested.getByLabel('Name', { exact: true }).fill('Abandoned');
  await expectInsideViewport(page, nested);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Discard', exact: true }).click();
  await expect(nested).toBeHidden();
  await expect(plugin.getByLabel('Reader background')).toHaveValue('Keep me');
  await picker.selectOption({ label: name });
  await expect(plugin.getByLabel('Default model')).toHaveValue('manual-model');
  await expectInsideViewport(page, plugin);
  await page.screenshot({ path: testInfo.outputPath('plugin-connection.png') });
  await plugin.getByRole('button', { name: 'Save settings' }).click();
  await expect(plugin.getByText('Settings saved.')).toBeVisible();

  await openSettings(page, 'Plugins');
  await page.getByRole('button', { name: 'Content Quality settings' }).click();
  const reopened = page.getByTestId('pluginSettingsDialog');
  await expect(
    reopened.getByLabel('Saved connection').locator('option:checked'),
  ).toHaveText(name);
  await expect(reopened.getByLabel('Reader background')).toHaveValue('Keep me');
  expect(applied).toHaveLength(1);
});
