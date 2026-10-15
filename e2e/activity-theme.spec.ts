import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem(
    `pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`,
    JSON.stringify({ welcomeDismissed: true, completed: [] }),
  ));
});

test('command batches have one icon and thread dialogs follow light and dark themes', async ({ page, request }, testInfo) => {
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Theme regression' } })).json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Theme regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const thread = await response.json();
  const id = thread.id ?? thread.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date().toISOString();
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail,
    thread: { ...detail.thread, status: 'running', activeTurnId: 'theme-turn' },
    totalTurnCount: 1,
    turns: [{ id: 'theme-turn', status: 'inProgress', startedAt, completedAt: null, items: [
      { id: 'prompt', kind: 'userMessage', text: 'Check the commands' },
      { id: 'cmd-a', kind: 'commandExecution', text: 'git status', status: 'completed', createdAt: startedAt },
      { id: 'cmd-b', kind: 'commandExecution', text: 'pnpm test', status: 'in_progress', createdAt: startedAt },
    ] }],
  } }));
  for (const theme of ['light', 'dark']) {
    await page.goto(`/threads/${id}`);
    await page.evaluate(value => localStorage.setItem('remote-codex-theme-mode', value), theme);
    await page.reload();
    await expect(page.locator('html')).toHaveAttribute('data-theme-effective', theme);
    await page.getByRole('button', { name: /Expand turn 1/ }).click();
    const group = page.locator('.tlog-run');
    await expect(group).toBeVisible();
    // One running glyph on the summary; opening reveals each operation's own glyph.
    await expect(group.locator('.tlog-run-head > .tlog-spinner')).toBeVisible();
    const head = group.locator('.tlog-run-head');
    if (await head.getAttribute('aria-expanded') === 'false') await head.click();
    await expect(group.locator('.tlog-row')).toHaveCount(2);
    await expect(group.locator('.tlog-row[data-status="running"] .tlog-spinner')).toBeVisible();
    await expect(group.locator('.tlog-row .tlog-glyph')).toHaveText(['$', '$']);
    await page.screenshot({ path: testInfo.outputPath(`batch-${theme}.png`) });
    for (const action of ['Rename', 'Delete']) {
      await page.getByTestId('recent-chats').getByRole('button', { name: 'Actions for Theme regression', exact: true }).click();
      await page.locator('.recent-thread-menu').getByRole('button', { name: `${action} thread`, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: action === 'Rename' ? 'Rename thread' : 'Delete thread?', exact: true });
      await expect(dialog).toBeVisible();
      const colors = await dialog.evaluate(element => {
        const probe = document.createElement('div');
        probe.style.backgroundColor = 'var(--theme-panel)';
        probe.style.color = 'var(--theme-fg)';
        document.body.append(probe);
        const expected = getComputedStyle(probe), actual = getComputedStyle(element);
        const result = { background: actual.backgroundColor, foreground: actual.color, expectedBackground: expected.backgroundColor, expectedForeground: expected.color };
        probe.remove();
        return result;
      });
      expect(colors.background).toBe(colors.expectedBackground);
      expect(colors.foreground).toBe(colors.expectedForeground);
      await page.screenshot({ path: testInfo.outputPath(`${action.toLowerCase()}-${theme}.png`) });
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
  }
});
