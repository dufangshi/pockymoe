import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';

import { expect, test } from '@playwright/test';

const workspaceRoot = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright');

async function ensureWorkspaceDir(name: string) {
  const dir = path.join(workspaceRoot, name);
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(path.join(dir, 'README.md'), `# ${name}\n`);
  return dir;
}

function makeWorkspaceName(prefix: string) {
  return `${prefix}-${randomUUID().slice(0, 8)}`;
}

test.describe('Phase 2 acceptance', () => {
  test('can create a workspace, create a thread, and receive a hello response', async ({
    page,
  }) => {
    const workspaceName = makeWorkspaceName('phase2-e2e');
    const workspacePath = await ensureWorkspaceDir(workspaceName);

    await page.goto('/workspaces/new');
    await page.getByRole('button', { name: 'Existing path' }).click();
    await page.getByLabel('Absolute path').fill(workspacePath);
    await page.getByLabel('Display label').fill(workspaceName);
    await page.getByRole('button', { name: 'Add workspace' }).click();

    await expect(page).toHaveURL(/\/threads\?workspaceId=.+/);
    await expect(
      page.getByRole('heading', { level: 1, name: workspaceName }),
    ).toBeVisible();
    await expect(
      page.locator('.matter-workbench'),
    ).toBeVisible();

    await page.getByRole('link', { name: 'Create thread', exact: true }).click();
    await page.getByLabel('Title').fill(`${workspaceName} thread`);
    await page.getByRole('button', { name: 'Create Thread' }).click();

    await expect(page).toHaveURL(/\/threads\/.+/);
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();

    await page
      .getByRole('textbox', { name: 'Prompt' })
      .fill('hello, reply me with hello');
    await page.getByRole('button', { name: 'Send Prompt' }).click();

    await expect(page.getByText('hello', { exact: true })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByText('Showing 1 of 1 turns')).toBeVisible({
      timeout: 30_000,
    });
    await page.getByRole('textbox', { name: 'Prompt' }).fill('A follow-up draft');
    await expect(page.getByRole('button', { name: 'Send Prompt' })).toBeEnabled();
  });
});
