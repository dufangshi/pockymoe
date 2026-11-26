import { test, expect } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
test.skip(!process.env.E2E_GIT_REAL_LAB, 'Requires an isolated two-turn DSH lab');
test('real two-turn checkpoints open from transcript and restore with undo', async ({ page }) => {
  const lab = process.env.E2E_GIT_REAL_LAB!;
  const { threadId } = JSON.parse(await readFile(`${lab}/identity.json`, 'utf8'));
  await page.addInitScript(() => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); });
  await page.goto(`/threads/${threadId}`);
  const later = page.getByRole('button', { name: 'Later', exact: true }); await later.waitFor({ timeout: 3000 }).catch(() => {}); if (await later.isVisible()) await later.click();
  const turnResponse = page.waitForResponse(response => { const url = new URL(response.url()); return url.pathname.endsWith('/git/diff') && url.searchParams.get('area') === 'turn'; });
  await page.getByRole('button', { name: '1 file +1 −1', exact: true }).last().click();
  const dock = page.getByTestId('thread-git');
  await expect(dock.getByRole('button', { name: 'Turn', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const response = await turnResponse;
  expect(response.ok()).toBe(true);
  const result = await response.json();
  expect(result.truncated).toBe(false);
  expect(result.patch).toContain('diff --git a/file.txt b/file.txt');
  expect(result.patch).toContain('-turn-1\n+turn-2');
  await test.info().attach('real-turn-response', { body: JSON.stringify({ url: response.url(), status: response.status(), ...result }), contentType: 'application/json' });
  // The picker and toolbar render before the real Git request finishes. Wait
  // for both the file list and patch; toolbar presence is not diff readiness.
  const assertTurnChanges = async () => {
    await expect(dock.getByRole('complementary', { name: 'Changed files' }).getByRole('button', { name: 'View diff for file.txt', exact: true })).toBeVisible();
    await expect(dock.locator('[data-kind="del"]')).toContainText('turn-1');
    await expect(dock.locator('[data-kind="add"]')).toContainText('turn-2');
  };
  await assertTurnChanges();
  await dock.getByRole('button', { name: 'Restore before' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Restore before', exact: true }).click();
  await expect(dock.getByRole('button', { name: 'Undo restore', exact: true })).toBeVisible();
  expect(await readFile(`${lab}/ws/repo/file.txt`, 'utf8')).toContain('turn-1');
  await dock.getByRole('button', { name: 'Undo restore', exact: true }).click();
  await expect(dock.getByRole('button', { name: 'Undo restore', exact: true })).toHaveCount(0);
  expect(await readFile(`${lab}/ws/repo/file.txt`, 'utf8')).toContain('turn-2');
  await page.screenshot({ path: `${lab}/real-turn.png` });
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({ theme, mode }) => { localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); localStorage.removeItem('pockymoe.workbench.inspector'); }, { theme, mode });
    await page.goto(`/threads/${threadId}`);
    const edit = page.getByRole('button', { name: 'Edit from here', exact: true }).last();
    await edit.locator('xpath=ancestor::*[contains(@class,"thread-graph-message-bubble")][1]').hover();
    await edit.click();
    await expect(page.getByRole('dialog', { name: 'Edit from here' })).toBeVisible();
    await expect(page.getByRole('radio', { name: 'Rewind conversation and files', exact: true })).toBeEnabled();
    await page.screenshot({ path: `${lab}/${theme}-${mode}-${width}-rewind.png` });
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  }
});

test('real Turn diffs list files and render patches in every theme', async ({ page }) => {
  const lab = process.env.E2E_GIT_REAL_LAB!;
  const { threadId } = JSON.parse(await readFile(`${lab}/identity.json`, 'utf8'));
  const turnShots = process.env.E2E_GIT_TURN_SCREENSHOTS ?? lab;
  await mkdir(turnShots, { recursive: true });
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({ theme, mode }) => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); localStorage.removeItem('pockymoe.workbench.inspector'); }, { theme, mode });
    await page.goto(`/threads/${threadId}`);
    const later = page.getByRole('button', { name: 'Later', exact: true });
    await later.waitFor({ timeout: 1500 }).catch(() => {});
    if (await later.isVisible()) await later.click();
    await page.getByRole('button', { name: '1 file +1 −1', exact: true }).last().click();
    const dock = page.getByTestId('thread-git');
    await expect(dock.getByRole('button', { name: 'Turn', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(dock.getByRole('complementary', { name: 'Changed files' }).getByRole('button', { name: 'View diff for file.txt', exact: true })).toBeVisible();
    await expect(dock.locator('[data-kind="del"]')).toContainText('turn-1');
    await expect(dock.locator('[data-kind="add"]')).toContainText('turn-2');
    await page.screenshot({ path: `${turnShots}/${theme}-${mode}-${width}-turn.png`, animations: 'disabled' });
  }
});
