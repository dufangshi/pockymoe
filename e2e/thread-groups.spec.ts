import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';

test.afterEach(async ({ page }) => {
  await page.unrouteAll({ behavior: 'wait' });
});

test('agent tabs stay grouped after list refreshes and navigate through an unclipped menu', async ({ page, request }, testInfo) => {
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const directory = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `groups-${randomUUID()}`);
  await fs.mkdir(directory, { recursive: true });
  const workspaceResponse = await request.post(`${base}/api/workspaces`, { data: { absPath: directory, label: 'Group regression', source: 'existing' } });
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = await workspaceResponse.json();
  const create = async (title: string, parentThreadId?: string) => {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'codex', model: 'default', parentThreadId } });
    expect(response.ok()).toBe(true);
    const value = await response.json();
    const thread = value.thread ?? value;
    expect(thread.id).toEqual(expect.any(String));
    return thread;
  };
  const root = await create('Research root');
  const child = await create('Search agent', root.id);
  const grandchild = await create('Scoring agent', child.id);
  let descendantRunning = true;
  await page.route('**/api/threads?includeAgentThreads=true', async route => {
    const response = await route.fetch();
    const threads = await response.json();
    await route.fulfill({ response, json: threads.map((thread: { id: string }) =>
      thread.id === grandchild.id ? { ...thread, status: descendantRunning ? 'running' : 'idle' } : thread),
    });
  });
  let snapshots = 0;
  page.on('request', request => { if (request.url().includes('/api/threads?includeAgentThreads=true')) snapshots += 1; });
  await page.goto(`/threads/${root.id}`);
  const tabs = page.getByRole('navigation', { name: 'Workspace threads' });
  const toggle = tabs.getByRole('button', { name: 'Research root: 2 agent threads' });
  await expect(toggle).toBeVisible();
  await expect(tabs.locator('a.matter-group-tab')).toHaveCount(1);
  // Another thread's turn events make the page re-read the list.
  const refreshList = async () => {
    const before = snapshots;
    expect((await request.post(`${base}/api/threads/${grandchild.id}/prompt`, { data: { prompt: 'hello' } })).ok()).toBe(true);
    await expect.poll(() => snapshots).toBeGreaterThan(before);
  };
  await refreshList();
  await expect(toggle).toBeVisible();
  if (testInfo.project.name === 'mobile-chromium') {
    await page.getByRole('button', { name: 'Toggle shortcuts sidebar' }).click();
  }
  const rootLink = page.getByTestId('recent-chats').locator(`a[href="/threads/${root.id}"]`);
  const dot = rootLink.getByRole('img');
  await expect(dot).toBeVisible();
  await expect(dot).toHaveAttribute('data-status', 'agents-running');
  await expect(dot).toHaveAttribute('aria-label', /1 agent thread running/);
  await expect(tabs.locator('.matter-group-tab [role="img"]')).toHaveAttribute('data-status', 'agents-running');
  // The navigation indicator must not turn an idle parent into a running session.
  const parentDetail = await (await request.get(`${base}/api/threads/${root.id}?view=summary`)).json();
  expect(parentDetail.thread.status).toBe('idle');
  descendantRunning = false;
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${grandchild.id}?view=summary&limit=1`)).json()).thread.status).toBe('idle');
  await refreshList();
  await expect(dot).toHaveAttribute('data-status', 'idle');
  await expect(tabs.locator('.matter-group-tab [role="img"]')).toHaveAttribute('data-status', 'idle');
  if (testInfo.project.name === 'mobile-chromium') {
    await page.getByRole('button', { name: 'Close sidebar', exact: true }).click();
  }
  await toggle.click();
  const menu = page.getByRole('region', { name: 'Research root agent threads' });
  await expect(menu).toBeVisible();
  expect(await menu.evaluate(element => element.closest('.matter-thread-tabs') === null)).toBe(true);
  await expect(menu.getByRole('link', { name: 'Scoring agent' })).toBeInViewport();
  await menu.getByRole('link', { name: 'Search agent' }).click();
  await expect(page).toHaveURL(new RegExp(`/threads/${child.id}$`));
  await expect(tabs.locator('a[aria-current="page"]')).toHaveText('Research root · Search agent');
  await expect(menu).toHaveCount(0);
  await toggle.click();
  await expect(menu).toBeVisible();
  await page.screenshot({ path: test.info().outputPath('grouped-tabs.png') });
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(toggle).toBeFocused();
});
