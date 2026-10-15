import { test, expect, type APIRequestContext } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test.use({ actionTimeout: 10_000 });
async function createThread(request: APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Timeline regression' } })).json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Timeline interactions', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const value = await response.json();
  return value.id ?? value.thread.id as string;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem(
    `pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`,
    JSON.stringify({ welcomeDismissed: true, completed: [] }),
  ));
});

test('workspace tabs include unvisited threads, keep their order and truncate long titles', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const titles = Array.from({ length: 13 }, (_, i) => `Workspace thread ${i + 1} with a deliberately long descriptive title`);
  const ids = [id];
  for (const title of titles) {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: detail.thread.workspaceId, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    ids.push(value.id ?? value.thread.id);
  }
  const outside = await createThread(request);
  await page.goto(`/threads/${outside}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await page.goto(`/threads/${id}`);
  const tabs = page.getByRole('navigation', { name: 'Workspace threads', exact: true });
  await expect(tabs.locator('a')).toHaveCount(14);
  await expect(tabs.locator(`a[href="/threads/${outside}"]`)).toHaveCount(0);
  const order = await tabs.locator('a').evaluateAll(elements => elements.map(e => e.getAttribute('href')));
  const last = tabs.locator('a').last();
  await last.click();
  await expect(last).toHaveAttribute('aria-current', 'page');
  expect(await tabs.locator('a').evaluateAll(elements => elements.map(e => e.getAttribute('href')))).toEqual(order);
  expect(await last.evaluate(e => e.getBoundingClientRect().width)).toBeLessThanOrEqual(testInfo.project.name === 'mobile-chromium' ? 140 : 180);
  expect(await last.locator('span').last().evaluate(e => ({ ellipsis: getComputedStyle(e).textOverflow, clipped: e.scrollWidth > e.clientWidth }))).toEqual({ ellipsis: 'ellipsis', clipped: true });
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme });
    await page.screenshot({ path: testInfo.outputPath(`workspace-tabs-${colorScheme}.png`) });
  }
});

test('turn navigation changes direction reliably and tool details load only on click', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, value => typeof value === 'string' ? value.toLowerCase() : value);
  db.function('search_body', { deterministic: true }, (text, kind, _source) => ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null);
  const at = (turn: number, seconds = 0) => new Date(Date.UTC(2026, 8, 20, 1, turn, seconds)).toISOString();
  try {
    for (let i = -2; i < 3; i++) {
      const turnId = `${id}-turn-${i}`;
      db.prepare('INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,display_prompt) VALUES (?,?,?,?,?,?,?)').run(turnId, id, 'completed', at(i), at(i, 50), i, `Prompt ${i}`);
      const items = [
        { id: 'user', kind: 'userMessage', text: `Prompt ${i}` },
        { id: 'reason-a', kind: 'reasoning', text: '**Checking the first section**' },
        { id: 'reason-b', kind: 'reasoning', text: '**Checking the second section**' },
        { id: 'cmd-a', kind: 'commandExecution', text: 'pnpm test', detailText: 'PASS 42 tests\nActual command output.' },
        { id: 'cmd-b', kind: 'commandExecution', text: 'git status', detailText: 'On branch test\nworking tree clean' },
        { id: 'read', kind: 'fileRead', text: '/home/user/a/very/long/workspace/path/packages/runtime/src/important-file.rs' },
        { id: 'single', kind: 'commandExecution', text: 'cargo check', detailText: 'Finished dev profile successfully' },
        { id: 'edit-a', kind: 'fileChange', text: 'src/main.rs', changedFiles: 1, addedLines: 1, removedLines: 1, detailText: '--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    old();\n+    new();' },
        { id: 'edit-b', kind: 'fileChange', text: 'src/lib.rs', changedFiles: 1, addedLines: 2, removedLines: 0, detailText: '--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1,2 @@\n+pub fn ready() -> bool {\n+    true' },
        { id: 'reply', kind: 'agentMessage', text: `Final reply ${i}.\n\n` + 'A full paragraph for testing navigation through a long conversation.\n\n'.repeat(18) },
      ];
      for (const [index, item] of items.entries()) {
        const key = `${turnId}-${item.id}`;
        const time = at(i, index * 4);
        db.prepare('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)').run(randomUUID(), id, turnId, key, JSON.stringify({ ...item, id: key, createdAt: time, status: 'completed' }), time, time);
      }
    }
  } finally { db.close(); }
  const detailRequests: string[] = [];
  page.on('request', request => { if (request.url().includes(`/api/threads/${id}/items/`)) detailRequests.push(request.url()); });
  await page.goto(`/threads/${id}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await expect(page.locator('.matter-breadcrumb')).toHaveCount(0);
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await expect(page.locator('.matter-breadcrumb')).toBeVisible();
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await expect(page.locator('.matter-breadcrumb')).toHaveCount(0);
  if (testInfo.project.name === 'mobile-chromium') {
    await expect(page.locator('.matter-rail')).toHaveCount(0);
    await expect(page.locator('.matter-topbar').getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
    await expect(page.locator('.matter-topbar').getByRole('button', { name: 'Open settings', exact: true })).toBeVisible();
    expect(await page.locator('.matter-main').evaluate(e => e.getBoundingClientRect().left)).toBeLessThan(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const jump = page.getByRole('group', { name: 'Timeline navigation' });
    await expect(jump).toBeVisible();
    const compact = await jump.evaluate(element => {
      const rect = element.getBoundingClientRect();
      const composer = document.querySelector('.thread-graph-composer-shell')!.getBoundingClientRect();
      return { height: rect.height, width: rect.width, gap: composer.top - rect.bottom };
    });
    expect(compact.height).toBeCloseTo(18, 0);
    expect(compact.width).toBeLessThanOrEqual(80);
    expect(compact.gap).toBeGreaterThanOrEqual(0);
    expect(compact.gap).toBeLessThanOrEqual(2);
  } else {
    await expect(page.locator('.matter-rail')).toBeVisible();
  }
  const previous = page.getByRole('button', { name: 'Jump to previous turn' });
  const next = page.getByRole('button', { name: 'Jump to next turn' });
  const bottom = page.getByRole('button', { name: 'Jump to latest' });
  const aligned = async (index: number) => expect.poll(() => page.locator(`[data-timeline-turn][data-turn-id="${id}-turn-${index}"]`).evaluate(e => Math.abs(e.getBoundingClientRect().top - document.querySelector('[data-testid="thread-scroll-container"]')!.getBoundingClientRect().top - 8))).toBeLessThan(3);
  await bottom.click(); await previous.click(); await aligned(2);
  await previous.click(); await aligned(1);
  await next.click(); await aligned(2);
  await previous.click(); await aligned(1);
  await previous.click(); await aligned(0);
  await previous.click(); await aligned(-1);
  await previous.click(); await aligned(-2);
  await expect(previous).toBeDisabled();
  await next.click(); await aligned(-1);
  await next.click(); await aligned(0);
  await next.click(); await aligned(1);
  await bottom.click(); await previous.click(); await aligned(2);
  // Reverse before smooth scrolling settles.
  await previous.evaluate(e => (e as HTMLButtonElement).click());
  await next.evaluate(e => (e as HTMLButtonElement).click());
  await aligned(2);
  const last = page.locator(`[data-timeline-turn][data-turn-id="${id}-turn-2"]`);
  await last.getByRole('button', { name: /Expand turn 5/ }).click();
  await expect(last.getByText('Checking the first section', { exact: true })).toHaveCount(0);
  await expect(last.getByText('Final reply 2.', { exact: false })).toBeVisible();
  const run = last.locator('.tlog-run');
  const operations = run.locator('.tlog-run-head');
  await expect(operations).toBeVisible();
  await expect(operations.locator('.tlog-count[data-kind=commands]')).toHaveAttribute('aria-label', '3 commands');
  expect(detailRequests).toHaveLength(0);
  await operations.click();
  // Finished runs list their rows closed; output loads only after a row click.
  await expect(last.locator('.tlog-row[data-open]')).toHaveCount(0);
  expect(detailRequests.filter(url => url.includes('cmd-') || url.includes('single'))).toHaveLength(0);
  const command = last.locator('.tlog-row[data-kind="commandExecution"]').filter({ hasText: 'pnpm test' });
  await expect(command.locator('.tlog-row-head')).toHaveAttribute('title', 'pnpm test');
  await command.locator('.tlog-row-head').click();
  await expect(command.locator('.tlog-output')).toContainText('PASS 42 tests');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(detailRequests.filter(url => url.includes('cmd-a'))).toHaveLength(1);
  const single = last.locator('.tlog-row[data-kind="commandExecution"]').filter({ hasText: 'cargo check' });
  await single.locator('.tlog-row-head').click();
  await expect(single.locator('.tlog-output')).toContainText('Finished dev profile successfully');
  const read = last.locator('.tlog-row[data-kind="fileRead"] .tlog-row-head');
  await read.click();
  await expect(read).toHaveAttribute('aria-expanded', 'true');
  await expect(read).toHaveAttribute('title', '/home/user/a/very/long/workspace/path/packages/runtime/src/important-file.rs');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const edit = last.locator('.tlog-row[data-kind="fileChange"]').filter({ hasText: 'src/main.rs' });
  await expect(edit.locator('.is-add').first()).toHaveText('+1');
  await expect(edit.locator('.is-remove').first()).toHaveText('−1');
  await edit.locator('.tlog-row-head').click();
  await expect(edit.locator('.thread-diff')).toHaveAttribute('data-highlighted', 'true');
  await expect(edit.locator('.thread-diff-line.is-add')).toContainText('new();');
  await expect(edit.locator('.thread-diff-line.is-remove')).toContainText('old();');
  expect(detailRequests).toHaveLength(3);
  await page.screenshot({ path: testInfo.outputPath('timeline-diff.png') });
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Preferences', exact: true }).click();
  const setting = page.getByRole('checkbox', { name: /Show agent status summaries/ });
  await expect(setting).not.toBeChecked();
  await setting.check();
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await last.locator('.tlog-run-head[aria-expanded="false"]').click();
  await expect(last.getByText('Checking the first section', { exact: true })).toBeVisible();
  await expect(last.getByText('Checking the second section', { exact: true })).toBeVisible();
  await expect(last.locator('.tlog-row[data-kind="reasoning"]')).toHaveCount(2);
  await page.reload();
  expect(await page.evaluate(() => localStorage.getItem('remote-codex-show-reasoning-summaries'))).toBe('true');
});

test('operation summaries stay compact in Chinese with long timestamps and file names', async ({ page, request }, testInfo) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  const id = await createThread(request);
  const turnId = `${id}-compact-turn`;
  const at = (seconds: number) => new Date(Date.UTC(2026, 8, 20, 1, 30, seconds)).toISOString();
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, value => typeof value === 'string' ? value.toLowerCase() : value);
  db.function('search_body', { deterministic: true }, (text, kind, _source) => ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null);
  try {
    db.prepare('INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,display_prompt) VALUES (?,?,?,?,?,?,?)').run(turnId, id, 'completed', at(0), at(300), 0, '优化操作记录的阅读体验');
    const items = [
      { kind: 'userMessage', text: '优化操作记录的阅读体验', seconds: 0 },
      { kind: 'agentMessage', text: '先检查项目文件，再调整布局。完整运行记录按需展开。', seconds: 78 },
      { kind: 'commandExecution', text: 'cd /tmp && for name in project-a project-b; do npm view "$name" version; done', seconds: 80 },
      { kind: 'commandExecution', text: 'git status --short && git diff --stat', seconds: 117 },
      { kind: 'fileRead', text: '/home/user/workspace/packages/application/src/components/VeryLongComponentName.tsx', seconds: 120 },
      { kind: 'fileRead', text: '/home/user/workspace/packages/application/src/styles/VeryLongStylesheetName.css', seconds: 125 },
      { kind: 'fileChange', text: 'scripts/generate.py', changedFiles: 1, addedLines: 95, removedLines: 8, seconds: 130 },
      { kind: 'fileChange', text: 'scripts/icon_tools.py', changedFiles: 1, addedLines: 82, removedLines: 4, seconds: 135 },
      { kind: 'fileChange', text: '/home/user/.claude/skills/a-very-long-skill-directory-name/SKILL.md', changedFiles: 1, addedLines: 78, removedLines: 0, seconds: 147 },
      { kind: 'commandExecution', text: 'pnpm test', seconds: 267 },
      { kind: 'agentMessage', text: '布局已调整，命令、文件读取和修改都可以继续展开查看。', seconds: 295 },
    ];
    items.forEach(({seconds, ...item}, index) => {
      const key = `${turnId}-${index}`, time = at(seconds);
      db.prepare('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)').run(randomUUID(), id, turnId, key, JSON.stringify({ ...item, id: key, createdAt: time, updatedAt: time, status: 'completed' }), time, time);
    });
  } finally { db.close(); }
  const details: string[] = [];
  page.on('request', r => { if (r.url().includes(`/api/threads/${id}/items/`)) details.push(r.url()); });
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'zh-CN'));
  await page.goto(`/threads/${id}`);
  await page.locator('.thread-worked-toggle').click();
  const run = page.locator('.tlog-run');
  const header = run.locator('.tlog-run-head');
  await expect(header).toContainText('3 个文件');
  await expect(header).toContainText('3m 7s');
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  expect(details).toHaveLength(0);
  await header.click();
  const rows = run.locator('.tlog-row');
  await expect(rows).toHaveCount(8);
  await expect(header.locator('.is-add')).toHaveText('+255');
  await expect(header.locator('.is-remove')).toHaveText('−12');
  // Long paths/commands stay on a single compact line and preserve their full title.
  const heads = run.locator('.tlog-row-head');
  for (const row of await heads.all()) {
    expect((await row.boundingBox())!.height).toBeLessThanOrEqual(36);
    expect(await row.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  }
  const longFile = run.locator('.tlog-row[data-kind="fileChange"] .tlog-title').last();
  await expect(longFile).toHaveCSS('white-space', 'nowrap');
  await expect(longFile).toHaveCSS('text-overflow', 'ellipsis');
  await expect(longFile.locator('..')).toHaveAttribute('title', '/home/user/.claude/skills/a-very-long-skill-directory-name/SKILL.md');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (testInfo.project.name === 'mobile-chromium') {
    await page.setViewportSize({ width: 320, height: 800 });
    expect(await heads.evaluateAll(nodes => nodes.every(n => n.scrollWidth <= n.clientWidth + 1 && n.getBoundingClientRect().height <= 36))).toBe(true);
  }
  await header.click();
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  expect(details).toHaveLength(0);

});

test('failed sends preserve the prompt and all attachments for correction and retry', async ({ page, request }) => {
  const id = await createThread(request);
  await page.goto(`/threads/${id}`);
  const editor = page.getByRole('textbox', { name: 'Prompt' });
  await editor.fill('Keep this exact draft after any send failure.');
  await page.locator('input[type=file]:not([accept])').setInputFiles(Array.from({ length: 11 }, (_, index) => ({ name: `file-${index}.txt`, mimeType: 'text/plain', buffer: Buffer.from(`attachment ${index}`) })));
  await page.getByRole('button', { name: 'Send Prompt' }).click();
  await expect(page.getByText(/A prompt can include at most 10 attachments/).first()).toBeVisible();
  await expect(editor).toContainText('Keep this exact draft');
  await expect(editor.locator('[data-segment-type="attachment"]')).toHaveCount(11);
  // Remove the attachment chips by replacing the editor, then retry with one file.
  await editor.fill('Keep this exact draft after any send failure.');
  await page.locator('input[type=file]:not([accept])').setInputFiles({ name: 'retry.txt', mimeType: 'text/plain', buffer: Buffer.from('retry attachment') });
  let sends = 0;
  await page.route(`**/api/threads/${id}/prompt`, route => {
    sends++;
    return route.fulfill({ status: 400, json: { code: 'too_many_attachments', message: 'A prompt can include at most 10 attachments.' } });
  });
  await page.getByRole('button', { name: 'Send Prompt' }).click();
  await expect.poll(() => sends).toBe(1);
  await expect(editor).toContainText('Keep this exact draft after any send failure.');
  await expect(editor.locator('[data-segment-type="attachment"]')).toHaveCount(1);
  await page.unroute(`**/api/threads/${id}/prompt`);
  await page.getByRole('button', { name: 'Send Prompt' }).click();
  await expect(editor).toHaveText('');
  await expect(page.getByText('Showing 1 of 1 turns')).toBeVisible();
});

test('substantial answers survive refresh between folded work sections', async ({ page, request }, info) => {
  const id = await createThread(request);
  const turnId = `${id}-answer-turn`;
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, value => typeof value === 'string' ? value.toLowerCase() : value);
  db.function('search_body', { deterministic: true }, (text, kind, _source) => ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null);
  const at = (index: number) => new Date(Date.UTC(2026, 9, 12, 12, index)).toISOString();
  const answer = '## 先回答你的问题\n\n' + '身份和授权基于公钥签名，私钥由用户保管，设备独立验证收到的请求。'.repeat(23);
  try {
    db.prepare('INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal,display_prompt) VALUES (?,?,?,?,?,?,?)').run(turnId, id, 'completed', at(0), at(7), 1, 'Explain and implement');
    const items = [
      { id: 'prompt', kind: 'userMessage', text: 'Explain and implement' },
      { id: 'before-1', kind: 'commandExecution', text: 'check-before-one' },
      { id: 'before-2', kind: 'commandExecution', text: 'check-before-two' },
      { id: 'answer', kind: 'agentMessage', text: answer, responsePhase: 'commentary' },
      { id: 'after-1', kind: 'commandExecution', text: 'check-after-one' },
      { id: 'after-2', kind: 'commandExecution', text: 'check-after-two' },
      { id: 'final', kind: 'agentMessage', text: 'Implementation verified.', responsePhase: 'final' },
    ];
    for (const [index, item] of items.entries()) {
      const key = `${turnId}-${item.id}`;
      db.prepare('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)').run(randomUUID(), id, turnId, key, JSON.stringify({ ...item, id: key, createdAt: at(index), status: 'completed' }), at(index), at(index));
    }
  } finally { db.close(); }
  await page.addInitScript(() => {
    localStorage.setItem('pockymoe-theme-preset', 'plum-pocket');
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([window.location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
  });
  const turnRequests: string[] = [];
  page.on('request', request => { if (request.url().includes(`/turns/${turnId}`)) turnRequests.push(request.url()); });
  await page.goto(`/threads/${id}`);
  const turn = page.locator(`[data-timeline-turn][data-turn-id="${turnId}"]`);
  await expect(turn.locator('.thread-answer-section')).toHaveCount(2);
  await expect(turn.getByText('先回答你的问题', { exact: true })).toBeVisible();
  await expect(turn.getByText('Implementation verified.', { exact: true })).toHaveCount(1);
  await expect(turn.getByText('Implementation verified.', { exact: true })).toBeVisible();
  expect(turnRequests).toHaveLength(0);
  await page.reload();
  await expect(turn.locator('.thread-answer-section')).toHaveCount(2);
  expect(turnRequests).toHaveLength(0);
  await turn.getByRole('button', { name: /Expand turn 1/ }).click();
  await expect(turn.locator('.thread-execution-timeline .tlog-run')).toHaveCount(2);
  expect(turnRequests).toHaveLength(1);
  const order = await turn.locator('.thread-execution-timeline').locator('.tlog-run, [data-role="assistant"]').evaluateAll(elements => elements.map(element => element.matches('.tlog-run') ? 'run' : 'answer'));
  expect(order).toEqual(['run', 'answer', 'run', 'answer']);
  const batches = turn.locator('.tlog-run-head');
  await expect(batches).toHaveCount(2);
  await page.screenshot({ path: info.outputPath('substantial-answer-between-steps.png'), scale: 'css' });
  await batches.last().click();
  await expect(turn.getByText('check-after-one', { exact: true })).toBeVisible();
  await expect(turn.getByText('check-before-one', { exact: true })).toHaveCount(0);
  await turn.getByRole('button', { name: /Collapse turn 1/ }).click();
  await expect(turn.getByText('先回答你的问题', { exact: true })).toBeVisible();
  await expect(turn.getByText('Implementation verified.', { exact: true })).toHaveCount(1);
  await expect(turn.getByText('Implementation verified.', { exact: true })).toBeVisible();
});

for (const preset of ['classic', 'plum-pocket'] as const) test(`thread loading stays visible until content arrives (${preset})`, async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const at = new Date().toISOString();
  const fullTurn = {
    id: `${id}-loading`, threadId: id, status: 'completed', startedAt: at, completedAt: at,
    items: [
      { id: 'prompt', kind: 'userMessage', text: 'Inspect this project', status: 'completed', createdAt: at },
      { id: 'command', kind: 'commandExecution', text: 'git status', status: 'completed', createdAt: at },
      { id: 'final', kind: 'agentMessage', text: 'Project inspected.', responsePhase: 'final', status: 'completed', createdAt: at },
    ],
  };
  const summary = { ...fullTurn, items: [fullTurn.items[0], fullTurn.items[2]], hasDeferredItems: true, deferredItemCount: 1 };
  let releaseInitial!: () => void;
  let releaseHistory!: () => void;
  const initialGate = new Promise<void>(resolve => { releaseInitial = resolve; });
  const historyGate = new Promise<void>(resolve => { releaseHistory = resolve; });
  await page.addInitScript(value => {
    localStorage.setItem('pockymoe-theme-preset', value);
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([window.location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
  }, preset);
  await page.route(new RegExp(`/api/threads/${id}\\?`), async route => {
    await initialGate;
    await route.fulfill({ json: { ...detail, turns: [summary], totalTurnCount: 1 } });
  });
  await page.route(`**/api/threads/${id}/turns/${fullTurn.id}/detail?*`, async route => {
    await historyGate;
    await route.fulfill({ json: fullTurn });
  });
  try {
    await page.goto(`/threads/${id}`);
    const indicator = page.locator('.thread-loading-indicator:not(.is-inline)');
    await expect(indicator).toBeVisible();
    await expect(indicator).toHaveAttribute('role', 'status');
    await expect(indicator.locator('.thread-loading-ring')).toHaveCSS('animation-name', 'thread-loading-rotate');
    await expect(page.getByText('Send the first prompt to start the thread.', { exact: true })).toHaveCount(0);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(indicator.locator('.thread-loading-ring')).toHaveCSS('animation-name', 'none');
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.screenshot({ path: testInfo.outputPath(`thread-loading-${preset}.png`), scale: 'css' });
    releaseInitial();
    await expect(indicator).toHaveCount(0);
    await expect(page.getByText('Project inspected.', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
    const actions = page.locator('.matter-thread-actions');
    await expect(actions.getByRole('button', { name: 'Download transcript', exact: true })).toBeVisible();
    await expect(actions.locator('.matter-thread-menu')).toHaveCount(0);
    const worked = page.locator('.thread-graph-worked-summary button[aria-expanded]');
    await worked.click();
    await expect(worked).toHaveAttribute('aria-busy', 'true');
    await expect(worked.locator('.thread-loading-indicator.is-inline')).toBeVisible();
    await expect(worked).toBeDisabled();
    releaseHistory();
    await expect(worked).toHaveAttribute('aria-busy', 'false');
    await expect(worked.locator('.thread-loading-indicator')).toHaveCount(0);
    await expect(worked).toBeEnabled();
    await expect(page.getByText('git status', { exact: true })).toBeVisible();
  } finally { releaseInitial(); releaseHistory(); }
});
