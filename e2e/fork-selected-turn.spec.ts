import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';

test('selected-turn fork shows errors at the selected row and retries into the new thread', async ({ page, request }) => {
  page.setDefaultTimeout(10000);
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Fork regression' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Fork source', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  for (const prompt of ['First turn', 'Second turn']) {
    const sent = await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt } });
    expect(sent.ok()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`${base}/api/threads/${id}`)).json()).thread.status).toBe('idle');
  }
  const turns = await (await request.get(`${base}/api/threads/${id}/fork-turns`)).json();
  expect(turns).toHaveLength(2);
  const options = Array.from({ length: 20 }, (_, index) => ({ ...turns[0], turnId: `turn-${index + 1}`, turnIndex: index + 1 }));
  await page.route(`**/api/threads/${id}/fork-turns`, route => route.fulfill({ json: options }));
  for (const endpoint of [`**/api/agent-runtimes/codex/status`, `**/api/threads/${id}/capabilities`]) {
    await page.route(endpoint, async route => {
      const result = await (await route.fetch()).json();
      const caps = result.effectiveCapabilities ?? result.capabilities;
      caps.branching = { ...caps.branching, fork: true, forkAt: true };
      const item = { id: 'fork', action: 'fork', command: '/fork', label: 'Fork', description: 'Fork thread' };
      if (result.toolboxItems) result.toolboxItems = [item];
      if (result.managementSchema) result.managementSchema.toolboxItems = [item];
      await route.fulfill({ json: result });
    });
  }
  const fork = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Fork result', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const forkDetail = await (await request.get(`${base}/api/threads/${fork.id}`)).json();
  let attempts = 0;
  let releaseFork!: () => void;
  const pendingFork = new Promise<void>(resolve => { releaseFork = resolve; });
  await page.route(`**/api/threads/${id}/fork`, async route => {
    attempts++;
    expect(route.request().postDataJSON()).toEqual({ mode: 'turn', turnId: 'turn-20' });
    if (attempts === 1) await pendingFork;
    return attempts === 1
      ? route.fulfill({ status: 409, json: { code: 'conflict', message: 'Codex could not fork the selected turn. Retry after reconnecting.' } })
      : route.fulfill({ json: { thread: forkDetail } });
  });
  await page.goto(`/threads/${id}`);
  await page.getByRole('button', { name: 'Open slash toolbox' }).click();
  await page.getByRole('button', { name: '/fork', exact: false }).click();
  await page.getByRole('button', { name: 'Fork from selected turn' }).click();
  await page.getByRole('button', { name: /^Turn 20\b/ }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Creating fork' })).toBeInViewport();
  await expect(page.getByRole('button', { name: /^Turn 20\b/ })).toBeDisabled();
  releaseFork();
  // The reason is a mark (announced to screen readers), not a paragraph in the panel.
  await expect(page.getByRole('alert').filter({ hasText: 'Codex could not fork' })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Why it failed' })).toBeInViewport();
  await expect(page).toHaveURL(new RegExp(`/threads/${id}$`));
  const reply = page.waitForResponse(r => r.url().endsWith(`/api/threads/${id}/fork`) && r.request().method() === 'POST');
  await page.getByRole('button', { name: /^Turn 20\b/ }).click();
  const response = await reply;
  expect(response.ok(), await response.text()).toBeTruthy();
  const result = await response.json();
  await expect(page).toHaveURL(new RegExp(`/threads/${result.thread.thread.id}$`));
});
