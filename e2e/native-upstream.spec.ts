import { expect, test } from '@playwright/test';

// Filesystem restoration is covered by Rust fixtures. Route both mutation paths
// here so phone interaction never changes the operator's real harness login.
test('a phone can explicitly switch API → account sign-in and keep that selection after reload', async ({ page }, testInfo) => {
  let nativeActive = true;
  let busyOnce = true;
  const calls: { path: string; body: unknown }[] = [];
  await page.route('**/api/management/harnesses', (route) => route.fulfill({
    json: [
      { id: 'claude', name: 'Claude Agent', base: { installed: true } },
      { id: 'codex', name: 'OpenAI Codex', base: { installed: true } },
    ],
  }));
  await page.route('**/api/management/connections', (route) => route.fulfill({
    json: {
      connections: [{
        id: 'anthropic-fixture', name: 'Temporary API', protocol: 'anthropic',
        baseUrl: 'https://fixture.invalid/v1', model: 'fixture-model', revision: 'c1',
        hasApiKey: true, local: true, sources: [],
      }],
      bindings: nativeActive ? {} : {
        claude: { connectionId: 'anthropic-fixture', model: 'fixture-model', revision: 'c1' },
      },
      native: { claude: { available: true, active: nativeActive, kind: 'oauth', revision: 'n1' } },
      catalogAvailable: false,
    },
  }));
  await page.route('**/api/management/connections/*/apply', (route) => {
    calls.push({ path: 'api', body: route.request().postDataJSON() });
    nativeActive = false;
    return route.fulfill({ json: { ok: true } });
  });
  await page.route('**/api/management/upstreams/native/claude/activate', (route) => {
    calls.push({ path: 'native', body: route.request().postDataJSON() });
    if (busyOnce) {
      busyOnce = false;
      return route.fulfill({ status: 409, json: { code: 'harness_busy', message: 'Harness is busy' } });
    }
    nativeActive = true;
    return route.fulfill({ json: { ok: true } });
  });
  async function openUpstreams() {
    await page.goto('/workspaces');
    const later = page.getByRole('button', { name: 'Later', exact: true });
    await later.waitFor({ timeout: 3_000 }).catch(() => {});
    if (await later.isVisible()) await later.click();
    await page.getByRole('button', { name: 'Open Navigation' }).click();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('tab', { name: 'Upstreams', exact: true }).click();
  }
  await openUpstreams();
  const region = page.getByRole('region', { name: 'Use in harnesses' });
  const row = region.getByRole('listitem', { name: 'Claude Agent' });
  const pick = row.getByLabel('Connection for Claude Agent');
  await expect(pick.locator('option:checked')).toHaveText('Account sign-in');
  await expect(page.getByRole('listitem', { name: 'Account sign-in', exact: true })).toHaveCount(0);
  await expect(region.getByRole('listitem', { name: 'OpenAI Codex' }).getByRole('option', { name: 'Account sign-in' })).toHaveCount(0);
  await pick.selectOption({ label: 'Temporary API' });
  expect(calls).toEqual([]);
  await row.getByRole('button', { name: 'Apply Temporary API to Claude Agent' }).click();
  const apiDialog = page.getByRole('dialog', { name: 'Apply Temporary API to Claude Agent' });
  await apiDialog.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(apiDialog).toBeHidden();
  await expect(row.getByTestId('harness-current')).toHaveText('Temporary API · fixture-model');
  await pick.selectOption({ label: 'Account sign-in' });
  expect(calls).toHaveLength(1);
  await row.getByRole('button', { name: 'Apply Account sign-in to Claude Agent' }).click();
  const nativeDialog = page.getByRole('dialog', { name: 'Apply Account sign-in to Claude Agent' });
  await expect(nativeDialog.getByLabel('Default model')).toHaveCount(0);
  await nativeDialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(nativeDialog).toBeHidden();
  expect(calls).toHaveLength(1);
  await expect(row.getByTestId('harness-current')).toHaveText('Temporary API · fixture-model');
  await row.getByRole('button', { name: 'Apply Account sign-in to Claude Agent' }).click();
  await nativeDialog.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(nativeDialog.getByRole('alert')).toContainText('The harness is busy');
  // The modal makes the background inert, excluding it from role queries.
  await expect(page.getByTestId('harness-current').filter({ hasText: 'Temporary API' })).toHaveText('Temporary API · fixture-model');
  expect(nativeActive).toBe(false);
  await nativeDialog.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(nativeDialog).toBeHidden();
  await expect(pick.locator('option:checked')).toHaveText('Account sign-in');
  await expect(row.getByTestId('harness-current')).toHaveText('Account sign-in');
  expect(calls).toEqual([
    { path: 'api', body: { harness: 'claude', model: 'fixture-model' } },
    { path: 'native', body: { revision: 'n1' } },
    { path: 'native', body: { revision: 'n1' } },
  ]);
  await openUpstreams();
  await expect(pick.locator('option:checked')).toHaveText('Account sign-in');
  await expect(row.getByRole('button', { name: 'Apply Account sign-in to Claude Agent' })).toBeDisabled();
  for (const element of [page.getByRole('dialog', { name: 'Settings', exact: true }), row, pick]) {
    const box = (await element.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(-1);
    expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  }
  await page.screenshot({ path: testInfo.outputPath('native-sign-in-phone.png') });
});
