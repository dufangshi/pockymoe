import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
test.skip(!process.env.E2E_GIT_REAL_LAB, 'Requires an isolated two-turn DSH lab');
test('real two-turn checkpoints open from transcript and restore with undo', async ({ page }) => {
  const lab = process.env.E2E_GIT_REAL_LAB!;
  const { threadId } = JSON.parse(await readFile(`${lab}/identity.json`, 'utf8'));
  await page.addInitScript(() => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); });
  await page.goto(`/threads/${threadId}`);
  const later = page.getByRole('button', { name: 'Later', exact: true }); await later.waitFor({ timeout: 3000 }).catch(() => {}); if (await later.isVisible()) await later.click();
  await page.getByRole('button', { name: '1 file +1 −1', exact: true }).last().click();
  const dock = page.getByTestId('thread-git');
  await expect(dock.getByRole('button', { name: 'Turn', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(dock.locator('[data-kind="add"]')).toContainText('turn-2');
  await dock.getByRole('button', { name: 'Restore before' }).click();
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
