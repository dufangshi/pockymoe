import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test('workspace opens chat directly and deleting its last thread preserves an empty chat shell', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Direct workspace entry' } })).json();
  async function create(title: string) {
    const result = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(result.ok()).toBeTruthy();
    const value = await result.json();
    return value.id ?? value.thread.id;
  }
  const first = await create('First workspace conversation');
  const background = await create('Background workspace conversation');
  const second = await create('Second workspace conversation');
  await page.goto(`/threads/${background}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await page.goto(`/threads/${first}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await page.goto('/workspaces');
  // A workspace card opens its latest conversation directly.
  await page.getByRole('link', { name: /^Direct workspace entry / }).first().click();
  await expect(page).toHaveURL(new RegExp(`/threads/${second}$`));
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await expect(page.getByText('Recent Threads', { exact: true })).toHaveCount(0);
  async function removeCurrent(title: string) {
    const sidebar = page.getByRole('button', { name: 'Toggle shortcuts sidebar' });
    if (testInfo.project.name === 'mobile-chromium' && await sidebar.getAttribute('aria-expanded') !== 'true') await sidebar.click();
    const recent = page.getByTestId('workspace-threads');
    await recent.getByRole('button', { name: `Actions for ${title}`, exact: true }).click();
    await page.getByRole('button', { name: 'Delete thread', exact: true }).click();
    await page.getByRole('dialog', { name: 'Delete thread?' }).getByRole('button', { name: 'Delete', exact: true }).click();
  }
  await removeCurrent('Background workspace conversation');
  await expect(page.getByTestId('workspace-threads').getByRole('button', { name: 'Actions for Background workspace conversation', exact: true })).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`/threads/${second}$`));
  await removeCurrent('Second workspace conversation');
  await expect(page).toHaveURL(new RegExp(`/threads/${first}$`));
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await removeCurrent('First workspace conversation');
  await expect(page).toHaveURL(new RegExp(`/threads\\?workspaceId=${workspace.id}$`));
  await expect(page.locator('.matter-workbench')).toBeVisible();
  const createLink = page.getByRole('link', { name: 'Create thread', exact: true });
  await expect(createLink).toBeVisible();
  await expect(createLink).toHaveAttribute('href', `/threads/new?workspaceId=${workspace.id}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Thread tools', exact: true })).toHaveCount(0);
  await page.reload();
  await expect(createLink).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('empty-workspace.png') });
  await createLink.click();
  await expect(page).toHaveURL(new RegExp(`/threads/new\\?workspaceId=${workspace.id}$`));
});
