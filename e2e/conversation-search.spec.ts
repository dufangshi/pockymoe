import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

for (const preset of ['classic', 'plum-pocket'] as const) test(`topbar search queries excerpts lazily and opens only the selected older turn (${preset})`, async ({ page, request }, testInfo) => {
  await page.addInitScript(value => localStorage.setItem('pockymoe-theme-preset', value), preset);
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `search-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Search review' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Search review', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
  try {
    // The history triggers call the Supervisor's search functions.
    db.function('search_body', { deterministic: true }, (text: unknown, _kind: unknown, _source: unknown) => (text as string | null) ?? '');
    db.function('search_fold', { deterministic: true }, (text: unknown) => ((text as string | null) ?? '').toLowerCase());
    const addTurn = db.prepare("INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal) VALUES(?,?,'completed',?,?,?)");
    const addItem = db.prepare('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?)');
    for (let n = 0; n < 6; n++) {
      const turnId = `${id}-search-${n}`;
      const time = new Date(Date.now() - (6 - n) * 60_000).toISOString();
      addTurn.run(turnId, id, time, time, n);
      for (const [kind, text] of [
        ['userMessage', `Review ${n}`],
        ['agentMessage', n === 0 ? 'The hidden cobalt decision is in this earlier message.' : `Visible response ${n}.\n\n` + 'A readable paragraph.\n\n'.repeat(12)],
      ]) {
        const itemId = `${turnId}-${kind}`;
        addItem.run(itemId, id, turnId, itemId, JSON.stringify({ id: itemId, kind, text, createdAt: time }), time, time);
      }
    }
  } finally { db.close(); }
  const searches: string[] = [];
  const hydrations: string[] = [];
  const fullPages: string[] = [];
  page.on('request', req => {
    const url = new URL(req.url());
    if (url.pathname === `/api/threads/${id}/search`) searches.push(req.url());
    if (url.pathname.startsWith(`/api/threads/${id}/turns/`)) hydrations.push(req.url());
    if (url.pathname === `/api/threads/${id}` && url.searchParams.get('view') === 'full') fullPages.push(req.url());
  });
  await page.goto(`/threads/${id}`);
  const later = page.getByRole('button', { name: 'Later' });
  if (await later.isVisible({ timeout: 3000 }).catch(() => false)) await later.click();
  await expect(page.locator(`[data-turn-id="${id}-search-0"]`)).toHaveCount(0);
  if (testInfo.project.name === 'mobile-chromium') await page.setViewportSize({ width: 320, height: 720 });
  await page.getByRole('button', { name: 'Search conversation', exact: true }).click();
  const input = page.getByRole('combobox', { name: 'Search messages' });
  await expect(input).toBeFocused();
  expect(searches).toHaveLength(0);
  expect(fullPages).toHaveLength(0);
  expect(hydrations).toHaveLength(0);
  await input.fill('hidden cobalt');
  const result = page.getByRole('option', { name: /hidden cobalt decision/ });
  await expect(result).toBeVisible();
  expect(searches).toHaveLength(1);
  expect(fullPages).toHaveLength(0);
  expect(hydrations).toHaveLength(0);
  // The desktop's only bar is the tabs row; a phone keeps its top bar.
  const header = (await page.locator(testInfo.project.name === 'mobile-chromium' ? '.matter-topbar' : '.matter-tabs-row').boundingBox())!;
  const field = (await input.boundingBox())!;
  const dropdown = (await page.locator('.workbench-search-dropdown').boundingBox())!;
  expect(field.y).toBeGreaterThanOrEqual(header.y);
  expect(field.y + field.height).toBeLessThanOrEqual(header.y + header.height);
  expect(dropdown.y).toBeGreaterThanOrEqual(field.y + field.height);
  expect(dropdown.x).toBeGreaterThanOrEqual(0);
  expect(dropdown.x + dropdown.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
  // Results span the dropdown: top-bar icon-button sizing must not reach them.
  const option = (await result.boundingBox())!;
  expect(option.width).toBeGreaterThan(dropdown.width * 0.85);
  expect(option.height).toBeLessThan(120);
  await page.screenshot({ path: testInfo.outputPath(`topbar-search-${preset}.png`) });
  await input.press('Escape');
  await expect(page.getByRole('button', { name: 'Search conversation', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Search conversation', exact: true }).click();
  await expect(result).toBeVisible();
  expect(searches).toHaveLength(1);
  await input.press('Enter');
  const message = page.locator(`[data-message-id="${id}-search-0-agentMessage"]`);
  await expect(message).toBeVisible();
  expect(hydrations).toHaveLength(1);
  expect(hydrations[0]).toContain(`/${id}-search-0/detail`);
  await expect(input).toHaveCount(0);
  await expect.poll(async () => message.evaluate(element => {
    const box = element.getBoundingClientRect();
    const viewport = document.querySelector('.thread-graph-scroll-container')!.getBoundingClientRect();
    return box.top >= viewport.top && box.bottom <= viewport.bottom;
  })).toBe(true);
});
