import { randomUUID } from 'node:crypto';
import { expect, test, type Locator, type Page } from '@playwright/test';

// Saved connections go through the real local Supervisor: save, failed model
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
  await expect(row.getByRole('button', { name: /apply/i })).toHaveCount(0);
  // Applying is the harness's own, explicit step below the connections.
  const codexRow = page
    .getByRole('region', { name: 'Use in harnesses' })
    .getByRole('listitem', { name: 'OpenAI Codex' });
  await codexRow
    .getByLabel('Connection for OpenAI Codex')
    .selectOption({ label: name });
  await codexRow
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
  // Only saved connections: no profile-file path, and the polish threshold says >=.
  await expect(plugin.getByText(/Connection profile file/)).toHaveCount(0);
  await expect(
    plugin.getByLabel('Auto-polish trigger length (characters)'),
  ).toBeVisible();
  await expect(
    plugin.getByText(/prose, not counting code, is at least this many/),
  ).toBeVisible();
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
  // A connection added from the plugin is saved for real and chosen here only.
  const pluginName = `Plugin ${randomUUID().slice(0, 8)}`;
  await plugin.getByRole('button', { name: 'Add connection' }).click();
  await nested.getByLabel('Name', { exact: true }).fill(pluginName);
  await nested.getByLabel('Base URL').fill('http://127.0.0.1:9/v1');
  await nested.getByLabel('API key').fill('synthetic-key');
  await nested.getByLabel('Default model').fill('plugin-model');
  await nested.getByRole('button', { name: 'Save connection' }).click();
  await expect(nested).toBeHidden();
  await expect(picker.locator('option:checked')).toHaveText(pluginName);
  await expect(plugin.getByLabel('Default model')).toHaveValue('plugin-model');
  await expect(plugin.getByLabel('Reader background')).toHaveValue('Keep me');
  await expectInsideViewport(page, plugin);
  await page.screenshot({ path: testInfo.outputPath('plugin-connection.png') });
  await plugin.getByRole('button', { name: 'Save settings' }).click();
  await expect(plugin.getByText('Settings saved.')).toBeVisible();

  await openSettings(page, 'Plugins');
  await page.getByRole('button', { name: 'Content Quality settings' }).click();
  const reopened = page.getByTestId('pluginSettingsDialog');
  await expect(
    reopened.getByLabel('Saved connection').locator('option:checked'),
  ).toHaveText(pluginName);
  await expect(reopened.getByLabel('Reader background')).toHaveValue('Keep me');
  await page.keyboard.press('Escape');
  await page.getByRole('tab', { name: 'Upstreams', exact: true }).click();
  await expect(page.getByRole('listitem', { name: pluginName })).toBeVisible();
  expect(applied).toHaveLength(1);
});

test('connections can be added when the harness inventory fails', async ({
  page,
}) => {
  const name = `No harness ${randomUUID().slice(0, 8)}`;
  const applied: unknown[] = [];
  await page.route('**/api/management/harnesses', (route) =>
    route.fulfill({ status: 500, json: { message: 'inventory unavailable' } }),
  );
  await page.route('**/api/management/connections/*/apply', (route) => {
    applied.push(route.request().url());
    return route.fulfill({ json: { ok: true } });
  });
  await openSettings(page, 'Upstreams');
  const harnesses = page.getByRole('region', { name: 'Use in harnesses' });
  await expect(harnesses.getByRole('alert')).toContainText(
    'Unable to check installed harnesses',
  );
  await page
    .getByRole('button', { name: 'Add connection', exact: true })
    .click();
  const editor = page.getByRole('dialog', { name: 'Add connection' });
  await editor.getByLabel('Name', { exact: true }).fill(name);
  await editor.getByLabel('API format').selectOption('anthropic');
  await editor.getByLabel('Base URL').fill('http://127.0.0.1:9/anthropic');
  // Anthropic adds an Authentication select whose options mention the API key.
  await editor.getByLabel('API key', { exact: true }).fill('synthetic-key');
  await editor.getByLabel('Default model').fill('any-model');
  await editor.getByRole('button', { name: 'Save connection' }).click();
  await expect(editor).toBeHidden();
  await expect(page.getByRole('listitem', { name })).toContainText(
    'Anthropic · 127.0.0.1:9/anthropic · any-model',
  );
  expect(applied).toEqual([]);
});
