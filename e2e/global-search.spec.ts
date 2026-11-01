import { selectControl } from './selectControl';
import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';

const apiBase = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const workspaceRoot = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright');

test('searches device and workspace histories and opens the original message with the keyboard', async ({ page, request }) => {
  const suffix = randomUUID().slice(0, 8);
  const workspaces: { id: string; label: string }[] = [];
  const threads: { id: string }[] = [];
  const needle = `中文检索-${suffix}`;
  for (const index of [0, 1]) {
    const directory = path.join(workspaceRoot, `search-${suffix}-${index}`);
    await fs.mkdir(directory, { recursive: true });
    const wsResponse = await request.post(`${apiBase}/api/workspaces`, { data: { absPath: directory, label: `Project ${index} ${suffix}` } });
    expect(wsResponse.ok()).toBeTruthy();
    const ws = await wsResponse.json();
    workspaces.push(ws);
    const threadResponse = await request.post(`${apiBase}/api/threads/start`, {
      data: { workspaceId: ws.id, title: `Decision ${index} ${suffix}`, provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' },
    });
    expect(threadResponse.ok()).toBeTruthy();
    const thread = await threadResponse.json();
    threads.push(thread);
    const sent = await request.post(`${apiBase}/api/threads/${thread.id}/prompt`, {
      data: { prompt: `Reply with exactly ${needle} ${index} 100% _.` },
    });
    expect(sent.ok()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`${apiBase}/api/threads/${thread.id}?view=summary`)).json()).thread.status).toBe('idle');
  }
  await page.goto(`/threads/${threads[0]!.id}`);
  await page.getByRole('button', { name: 'Search conversation', exact: true }).click();
  const input = page.getByRole('combobox', { name: 'Search messages' });
  const scope = page.getByRole('button', { name: 'Search scope' });
  await input.fill(needle);
  const results = page.getByRole('listbox', { name: 'Matching messages' });
  await expect(results.getByRole('option')).toHaveCount(2);
  await selectControl(page, scope, 'workspace');
  await expect(results.getByRole('option')).toHaveCount(2);
  await expect(results).toContainText(workspaces[0]!.label);
  await expect(results).not.toContainText(workspaces[1]!.label);
  await selectControl(page, scope, 'device');
  await expect(results.getByRole('option')).toHaveCount(4);
  await expect(results).toContainText(workspaces[1]!.label);
  await expect(page.getByText('Searches saved conversations on this device only.', { exact: false })).toBeVisible();
  // Scope + input remain operable on narrow screens, without horizontal overflow.
  await expect(scope).toBeVisible();
  await expect(input).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
  // Restrict the query to the second thread's assistant reply, then press Enter.
  await input.fill(`${needle} 1 100% _`);
  await expect(results.getByRole('option')).toHaveCount(2);
  await input.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/threads/${threads[1]!.id}\\?searchTurn=.*searchItem=`));
  const messageId = new URL(page.url()).searchParams.get('searchItem')!;
  const original = page.locator(`[data-message-id="${messageId}"]`);
  await expect(original).toBeVisible();
  await expect(original).toContainText(`${needle} 1 100% _`);
  await page.reload();
  await expect(page.locator(`[data-message-id="${messageId}"]`)).toBeVisible();
  await page.getByRole('button', { name: 'Search conversation', exact: true }).click();
  await selectControl(page, page.getByRole('button', { name: 'Search scope' }), 'device');
  await page.getByRole('combobox', { name: 'Search messages' }).fill(`Decision 0 ${suffix}`);
  await expect(page.getByRole('listbox').getByRole('option')).toHaveCount(1);
  await page.getByRole('combobox', { name: 'Search messages' }).press('Enter');
  await expect(page).toHaveURL(new RegExp(`/threads/${threads[0]!.id}$`));
});
