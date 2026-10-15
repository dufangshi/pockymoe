import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test.use({ actionTimeout: 10_000 });

async function fixture(request: APIRequestContext) {
  if (!process.env.E2E_DATABASE_URL || !process.env.E2E_WORKSPACE_ROOT) throw new Error('Explicit isolated database and workspace required');
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT, `anchoring-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspaceResponse = await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Anchoring regression' } });
  expect(workspaceResponse.ok()).toBeTruthy();
  const workspace = await workspaceResponse.json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id,
    title: 'Anchoring regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const created = await response.json();
  return { id: (created.id ?? created.thread.id) as string, absPath };
}

type Item = { kind: string; text: string; responsePhase?: string; status?: string };
function seed(id: string, items: Item[], { turn = 'anchor-turn', ordinal = 0, start = 0 } = {}) {
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, value => typeof value === 'string' ? value.toLowerCase() : value);
  db.function('search_body', { deterministic: true }, (text, kind, _source) => ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null);
  const turnId = `${id}-${turn}`;
  const at = (index: number) => new Date(Date.UTC(2026, 9, 15, 0, 0, start + index)).toISOString();
  try {
    db.prepare('INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,display_prompt) VALUES (?,?,?,?,?,?,?)')
      .run(turnId, id, 'completed', at(0), at(items.length), ordinal, items[0]?.text ?? 'Inspect');
    for (const [index, item] of items.entries()) {
      const key = `${turnId}-${index}`;
      db.prepare('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)')
        .run(randomUUID(), id, turnId, key, JSON.stringify({ ...item, id: key, status: item.status ?? 'completed', createdAt: at(index), updatedAt: at(index) }), at(index), at(index));
    }
  } finally { db.close(); }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('pockymoe-theme-preset', 'terminal');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

// Sample every animation frame so a temporary jump cannot pass a final-position check.
async function startSampling(target: Locator) {
  return target.evaluate(element => {
    const initial = element.getBoundingClientRect().top;
    const samples: number[] = [];
    let active = true;
    const sample = () => {
      if (!active) return;
      // Read after all rAF layout corrections for this frame have run.
      setTimeout(() => { if (active) samples.push(element.getBoundingClientRect().top); }, 0);
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
    Object.assign(window, { anchorSamples: { initial, samples, stop: () => { active = false; } } });
    return initial;
  });
}
async function assertHeld(page: Page, action: string) {
  const result = await page.evaluate(async () => {
    const sample = (window as unknown as { anchorSamples: { initial: number; samples: number[]; stop: () => void } }).anchorSamples;
    const until = performance.now() + 1050;
    while (performance.now() < until) await new Promise(requestAnimationFrame);
    sample.stop();
    return { initial: sample.initial, last: sample.samples.at(-1), count: sample.samples.length, maxDelta: Math.max(...sample.samples.map(top => Math.abs(top - sample.initial))) };
  });
  expect(result.count).toBeGreaterThan(10);
  expect(result.maxDelta, `${action}: ${JSON.stringify(result)}`).toBeLessThanOrEqual(2);
}

async function workFixture(request: APIRequestContext) {
  const value = await fixture(request);
  seed(value.id, [
    { kind: 'userMessage', text: 'Inspect the command sequence and keep the work control under the pointer.\n'.repeat(8) },
    ...Array.from({ length: 40 }, (_, index) => ({ kind: 'commandExecution', text: `check-operation-${index}`, status: index === 39 ? 'failed' : 'completed' })),
    { kind: 'agentMessage', text: 'All operations checked.', responsePhase: 'final' },
  ]);
  return value;
}

test('work line remains anchored when expanded and collapsed from the tail', async ({ page, request, isMobile }) => {
  test.skip(isMobile, 'Shared toggle geometry is covered on desktop');
  await page.setViewportSize({ width: 1280, height: 500 });
  const { id } = await workFixture(request);
  await page.goto(`/threads/${id}`);
  const scroll = page.getByTestId('thread-scroll-container');
  const toggle = page.locator('.thread-worked-toggle');
  await expect(toggle).toBeVisible();
  await scroll.evaluate(element => { element.scrollTop = element.scrollHeight; });
  expect(await scroll.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  expect(await scroll.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(1);
  for (const expanded of ['true', 'false']) {
    await startSampling(toggle);
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', expanded);
    await assertHeld(page, `expanded=${expanded}`);
  }
});

test('run head remains anchored across open and close', async ({ page, request, isMobile }) => {
  test.skip(isMobile, 'Shared run geometry is covered on desktop');
  const { id } = await workFixture(request);
  await page.goto(`/threads/${id}`);
  await page.locator('.thread-worked-toggle').click();
  const head = page.locator('.tlog-run-head');
  await expect(head).toBeVisible();
  await expect(page.locator('.tlog-collapse').filter({ has: head })).toHaveAttribute('data-settled', 'true');
  const color = await head.locator('[data-kind=commands]').evaluate(element => getComputedStyle(element).color);
  await expect(head.locator('[data-kind=failed]')).toHaveAttribute('aria-label', '1 failed');
  await expect(head.locator('[data-kind=failed]')).toHaveCSS('color', color);
  const muted = await head.evaluate(element => {
    const probe = document.createElement('span');
    probe.style.color = 'var(--theme-fg-muted)';
    element.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  });
  await expect(head.locator('.tlog-chev')).toHaveCSS('color', muted);
  for (const expanded of ['true', 'false']) {
    await startSampling(head);
    await head.click();
    await expect(head).toHaveAttribute('aria-expanded', expanded);
    await assertHeld(page, `expanded=${expanded}`);
  }
});

const longPrompt = Array.from({ length: 100 }, (_, i) => `Requirement ${i}: keep every line available while preserving the reading position.\n`).join('');

test('a long message keeps sticky Show less above the composer and three clicks stay in place', async ({ page, request, isMobile }) => {
  const { id } = await fixture(request);
  // An earlier turn gives the history room above the message.
  seed(id, [
    { kind: 'userMessage', text: 'Earlier question' },
    { kind: 'agentMessage', text: Array.from({ length: 40 }, (_, i) => `Earlier paragraph ${i}.`).join('\n\n'), responsePhase: 'final' },
  ], { turn: 'earlier', ordinal: 0 });
  seed(id, [
    { kind: 'userMessage', text: longPrompt },
    { kind: 'agentMessage', text: 'Requirements received.', responsePhase: 'final' },
  ], { turn: 'anchor-turn', ordinal: 1, start: 100 });
  await page.goto(`/threads/${id}`);
  const toggle = page.locator('[data-role=user] .thread-graph-show-more');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const scroll = page.getByTestId('thread-scroll-container');
  await scroll.hover();
  const before = await scroll.evaluate(element => element.scrollTop);
  await page.mouse.wheel(0, 450);
  await expect.poll(() => scroll.evaluate(element => element.scrollTop)).toBeGreaterThanOrEqual(before + 449);
  await expect(toggle).toHaveCSS('position', 'sticky');
  const composer = page.locator('.thread-graph-composer-host');
  await expect.poll(async () => (await composer.boundingBox())!.y - ((await toggle.boundingBox())!.y + (await toggle.boundingBox())!.height)).toBeGreaterThanOrEqual(8);
  const box = (await toggle.boundingBox())!;
  const x = box.x + box.width / 2, y = box.y + box.height / 2;
  for (const expanded of ['false', 'true', 'false']) {
    await startSampling(toggle);
    if (isMobile) await page.touchscreen.tap(x, y);
    else await page.mouse.click(x, y);
    await expect(toggle).toHaveAttribute('aria-expanded', expanded);
    await assertHeld(page, `expanded=${expanded}`);
    expect(Math.abs((await toggle.boundingBox())!.y - box.y)).toBeLessThanOrEqual(2);
  }
});

test('the first long message folds without leaving a blank above the history', async ({ page, request, isMobile }) => {
  const { id } = await fixture(request);
  seed(id, [
    { kind: 'userMessage', text: longPrompt },
    { kind: 'agentMessage', text: 'Requirements received.', responsePhase: 'final' },
  ]);
  await page.goto(`/threads/${id}`);
  const toggle = page.locator('[data-role=user] .thread-graph-show-more');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const scroll = page.getByTestId('thread-scroll-container');
  await scroll.hover();
  await page.mouse.wheel(0, 450);
  await expect(toggle).toHaveCSS('position', 'sticky');
  const box = (await toggle.boundingBox())!;
  if (isMobile) await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
  else await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  // Nothing is held above the history: the turn starts at the top, the toggle is in view.
  await expect.poll(() => scroll.evaluate(element => element.scrollTop)).toBe(0);
  await expect(page.locator('.thread-scroll-slack')).toHaveCount(0);
  const top = (await scroll.boundingBox())!.y;
  expect((await page.locator('[data-testid=chat-turn]').first().boundingBox())!.y - top).toBeLessThanOrEqual(80);
  await expect(toggle).toBeInViewport();
});

test('count hint appears after one second of mouse hover', async ({ page, request, isMobile }) => {
  test.skip(isMobile, 'Touch counts are covered by unit tests');
  const { id } = await workFixture(request);
  await page.goto(`/threads/${id}`);
  const count = page.locator('.thread-worked-toggle .tlog-count[data-kind=commands]');
  await expect(count).toHaveAttribute('aria-label', '40 commands');
  // install() lets time keep flowing; pause it so only runFor advances the hint's timer.
  await page.clock.install({ time: new Date('2026-10-15T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-10-15T12:00:01Z'));
  await count.hover();
  await page.clock.runFor(999);
  await expect(page.getByRole('tooltip')).toHaveCount(0);
  await page.clock.runFor(1);
  const hint = page.locator('.thread-ui-shell > .tlog-hint[role=tooltip]');
  await expect(hint).toBeVisible();
  await expect(hint).toHaveCSS('position', 'fixed');
  await expect(hint.locator('.tlog-hint-title')).toHaveText('40 commands');
  await page.mouse.move(0, 0);
  await expect(hint).toHaveCount(0);
});

test('envelope sender link navigates to the sending thread', async ({ page, request, isMobile }) => {
  test.skip(isMobile, 'Navigation is covered on desktop');
  const sender = await fixture(request);
  const receiver = await fixture(request);
  seed(receiver.id, [{ kind: 'userMessage', text: `[Pockymoe task from thread ${sender.id} | Check the source]\nPlease inspect the source.` },
    { kind: 'agentMessage', text: 'Received.', responsePhase: 'final' }]);
  await page.goto(`/threads/${receiver.id}`);
  const link = page.locator('a.prompt-envelope-link');
  await expect(link).toHaveAttribute('href', `/threads/${sender.id}`);
  await link.click();
  await expect(page).toHaveURL(new RegExp(`/threads/${sender.id}$`));
  await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
});

test('workspace link reveals a deep target near the top, including after mobile Back to files', async ({ page, request, isMobile }) => {
  const { id, absPath } = await fixture(request);
  await mkdir(path.join(absPath, 'big-directory'), { recursive: true });
  await Promise.all(Array.from({ length: 120 }, (_, i) => writeFile(path.join(absPath, 'big-directory', `file-${String(i).padStart(2, '0')}.txt`), `Workspace file ${i}\n`)));
  seed(id, [{ kind: 'userMessage', text: 'Inspect the linked file.' },
    { kind: 'agentMessage', text: '[Open deep target](big-directory/file-65.txt)', responsePhase: 'final' }]);
  await page.goto(`/threads/${id}`);
  // Establish an old list position before navigating through the transcript link.
  const explorer = page.getByRole('button', { name: 'Toggle Explorer', exact: true });
  await explorer.click();
  const files = page.getByTestId('workspace-panel');
  const tree = files.getByRole('tree', { name: 'Workspace files', exact: true });
  await expect(tree).toBeVisible();
  await tree.evaluate(element => { element.scrollTop = 0; });
  await explorer.click();
  await page.getByRole('link', { name: 'Open deep target', exact: true }).click();
  await expect(files.getByRole('tab', { name: 'file-65.txt', exact: true })).toHaveAttribute('aria-selected', 'true');
  if (isMobile) await files.getByRole('button', { name: 'Back to files', exact: true }).click();
  await expect(tree).toBeVisible();
  const target = tree.getByRole('treeitem', { name: 'file-65.txt', exact: true });
  await expect(target).toHaveAttribute('aria-selected', 'true');
  await expect.poll(async () => {
    const row = await target.boundingBox(), bounds = await tree.boundingBox();
    if (!row || !bounds) return -1;
    return (row.y - bounds.y) / bounds.height;
  }).toBeGreaterThanOrEqual(0);
  await expect.poll(async () => {
    const row = (await target.boundingBox())!, bounds = (await tree.boundingBox())!;
    return (row.y - bounds.y) / bounds.height;
  }).toBeLessThanOrEqual(.25);
});
