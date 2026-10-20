import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

test('workspace links and map breadcrumbs open an unselected workspace with its threads', async ({ page, request }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspaceResponse = await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'Navigation workspace' } });
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = await workspaceResponse.json();
  const threadResponse = await request.post(`${api}/api/threads/start`, { data: {
    workspaceId: workspace.id, title: 'Choose this conversation', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo',
  } });
  expect(threadResponse.ok()).toBe(true);
  const started = await threadResponse.json();
  const id = started.id ?? started.thread.id;
  const workspaceUrl = `/threads?workspaceId=${workspace.id}`;

  await page.goto('/workspaces');
  await page.getByRole('link', { name: /^Navigation workspace/ }).click();
  await expect(page).toHaveURL(new RegExp(`/threads\\?workspaceId=${workspace.id}$`));
  await expect(page.getByRole('heading', { name: workspace.label, exact: true })).toBeVisible();
  await expect(page.getByText('Choose a thread, or create a new one.')).toBeVisible();
  await expect(page.getByTestId('workspace-threads').getByRole('link', { name: /Choose this conversation/ })).toBeVisible();
  await expect(page.locator('[data-testid="workspace-threads"] [aria-current="page"]')).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('heading', { name: workspace.label, exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/threads\\?workspaceId=${workspace.id}$`));

  await page.locator('.matter-rail').getByRole('button', { name: 'Thread map', exact: true }).click();
  const map = page.getByRole('dialog');
  const deviceLink = map.getByRole('link', { name: 'Open device workspaces: This device', exact: true });
  await expect(deviceLink).toHaveAttribute('href', '/workspaces');
  await deviceLink.click();
  await expect(page).toHaveURL(/\/workspaces$/);
  await expect(map).toHaveCount(0);

  await page.getByRole('button', { name: 'Thread map', exact: true }).click();
  await map.getByRole('button', { name: /^Open Navigation workspace/ }).click();
  const workspaceLink = map.getByRole('link', { name: 'Open workspace: Navigation workspace', exact: true });
  await expect(workspaceLink).toHaveAttribute('href', workspaceUrl);
  await workspaceLink.click();
  await expect(map).toHaveCount(0);
  await expect(page.getByRole('heading', { name: workspace.label, exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/threads\\?workspaceId=${workspace.id}$`));
  await page.getByTestId('workspace-threads').getByRole('link', { name: /Choose this conversation/ }).click();
  await expect(page).toHaveURL(new RegExp(`/threads/${id}$`));
  await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
});
