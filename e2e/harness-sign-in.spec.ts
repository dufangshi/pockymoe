import { expect, test, type Page } from '@playwright/test';

// Settings → Harnesses → Sign in runs the harness's catalog sign-in command in
// a real PTY on the device. The Supervisor under test must name the fixture as
// its Codex: CODEX_COMMAND=$PWD/e2e/fixtures/fake-harness-login.sh.
test.skip(
  !process.env.CODEX_COMMAND?.endsWith('fake-harness-login.sh'),
  'needs CODEX_COMMAND=<repo>/e2e/fixtures/fake-harness-login.sh for the Supervisor',
);

async function openHarnesses(page: Page) {
  await page.goto('/workspaces');
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Harnesses', exact: true }).click();
}

test('signs a harness in through a device terminal and checks it when the command ends', async ({ page }) => {
  await openHarnesses(page);
  const start = page.waitForRequest(request => request.url().endsWith('/api/shells/sign-in') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Sign in to OpenAI Codex' }).click();
  // Only the harness id goes to the device.
  expect((await start).postDataJSON()).toEqual({ agentId: 'codex' });
  const dialog = page.getByRole('dialog', { name: 'Sign in to OpenAI Codex' });
  await expect(dialog).toContainText('login --device-auth');
  const terminal = dialog.getByTestId('single-shell-terminal');
  await expect(terminal.locator('.xterm-rows')).toContainText('Fake device login (--device-auth)');
  await expect(terminal.locator('.xterm-rows')).toContainText('enter ABCD-1234');
  await terminal.locator('.xterm').click();
  await page.keyboard.type('confirm-42');
  // Esc belongs to the program in the terminal, not to the window.
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Enter');
  await expect(terminal.locator('.xterm-rows')).toContainText('Signed in as confirm-42');
  // The command exited: the device checked the harness again.
  await expect(dialog.getByRole('status').filter({ hasText: 'Signed in' })).toBeVisible();
  await dialog.locator('[data-dialog-cancel]').click();
  await expect(dialog).toBeHidden();
  await expect(page.locator('.settings-harness-row').getByRole('status')).toHaveText('Signed in');
});

test('closing the window ends a sign-in that is still running', async ({ page }) => {
  await openHarnesses(page);
  await page.getByRole('button', { name: 'Sign in to OpenAI Codex' }).click();
  const dialog = page.getByRole('dialog', { name: 'Sign in to OpenAI Codex' });
  await expect(dialog.locator('.xterm-rows')).toContainText('Paste the confirmation');
  const terminate = page.waitForRequest(request => /\/api\/shells\/[^/]+\/terminate$/.test(request.url()));
  await dialog.locator('[data-dialog-cancel]').click();
  await terminate;
  await expect(dialog).toBeHidden();
});
