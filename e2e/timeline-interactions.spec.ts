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
  const operations = last.getByRole('button', { name: 'Expand 6 operations', exact: true });
  await expect(operations).toBeVisible();
  expect(detailRequests).toHaveLength(0);
  await operations.scrollIntoViewIfNeeded();
  await mkdir('.temp/activity-groups/screenshots', { recursive: true });
  await page.screenshot({ path: `.temp/activity-groups/screenshots/${testInfo.project.name}-collapsed.png` });
  await operations.click();
  await page.screenshot({ path: `.temp/activity-groups/screenshots/${testInfo.project.name}-expanded.png` });
  await last.getByRole('button', { name: 'Expand 2 command entries' }).click();
  expect(detailRequests).toHaveLength(0);
  const command = last.getByRole('button', { name: 'Open grouped command 1', exact: true });
  await expect(command).toContainText('12s');
  await command.getByRole('button', { name: /Toggle timestamp/ }).click();
  await expect(command).not.toContainText('12s');
  expect(detailRequests).toHaveLength(0);
  await command.click();
  await expect(page.getByRole('dialog').getByText(/PASS 42 tests/)).toBeVisible();
  expect(detailRequests).toHaveLength(1);
  await page.getByRole('button', { name: 'Close dialog', exact: true }).click();
  await last.getByRole('button', { name: 'Open full command', exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Finished dev profile successfully');
  expect(detailRequests).toHaveLength(2);
  await page.getByRole('button', { name: 'Close dialog', exact: true }).click();
  const read = last.getByRole('button', { name: 'Show full file path' });
  await read.click();
  await expect(read).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await last.getByRole('button', { name: 'Expand 2 file change entries' }).click();
  expect(detailRequests).toHaveLength(2);
  const edit = last.getByRole('button', { name: 'Open grouped file change 1', exact: true });
  await expect(edit).toContainText('src/main.rs');
  await expect(edit.locator('.is-add')).toHaveText('+1');
  await expect(edit.locator('.is-remove')).toHaveText('-1');
  await edit.click();
  await expect(page.locator('.thread-diff')).toHaveAttribute('data-highlighted', 'true');
  await expect(page.locator('.thread-diff-line.is-add')).toContainText('new();');
  await expect(page.locator('.thread-diff-line.is-remove')).toContainText('old();');
  expect(detailRequests).toHaveLength(3);
  await page.screenshot({ path: `output/playwright/timeline-diff-${testInfo.project.name}.png` });
  await page.getByRole('button', { name: 'Close dialog', exact: true }).click();
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Preferences', exact: true }).click();
  const setting = page.getByRole('checkbox', { name: /Show agent status summaries/ });
  await expect(setting).not.toBeChecked();
  await setting.check();
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await last.getByRole('button', { name: 'Expand 8 operations', exact: true }).click();
  await expect(last.getByText('Checking the first section', { exact: true })).toBeVisible();
  await expect(last.getByText('Checking the second section', { exact: true })).toBeVisible();
  await expect(last.locator(`[data-message-id$="reason-a"]`)).toContainText('4s');
  await expect(last.locator(`[data-message-id$="reason-b"]`)).toContainText('8s');
  await page.reload();
  expect(await page.evaluate(() => localStorage.getItem('remote-codex-show-reasoning-summaries'))).toBe('true');
});

test('operation summaries stay compact in Chinese with long timestamps and file names', async ({ page, request }, testInfo) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  const id = await createThread(request);
  const turnId = `${id}-compact-turn`;
  const at = (seconds: number) => new Date(Date.UTC(2026, 8, 20, 1, 30, seconds)).toISOString();
  const db = new DatabaseSync(path.resolve(process.env.E2E_DATABASE_URL!));
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
  await page.locator('.thread-graph-worked-summary > button').click();
  const activity = page.locator('.thread-graph-history-group-activity');
  const header = activity.locator(':scope > .thread-graph-history-group-card > button');
  await expect(header).toContainText('8 个步骤');
  await expect(header.locator('.thread-graph-history-group-time')).toHaveText('3m 7s');
  await expect(header.locator('.thread-graph-history-group-inline-icon')).toHaveCount(0);
  await expect(header).toHaveAttribute('aria-expanded', 'false');
  expect(details).toHaveLength(0);
  await mkdir('.temp/activity-compact/screenshots', { recursive: true });
  await header.scrollIntoViewIfNeeded();
  await page.screenshot({ path: `.temp/activity-compact/screenshots/${testInfo.project.name}-collapsed.png` });
  await header.click();
  const fileGroup = activity.locator('.thread-graph-history-group-file-change');
  const fileHeader = fileGroup.locator(':scope > .thread-graph-history-group-card > button');
  await expect(fileHeader).toContainText('3 个文件');
  await expect(fileHeader.getByLabel('新增 255 行', { exact: true })).toBeVisible();
  await expect(fileHeader.getByLabel('删除 12 行', { exact: true })).toBeVisible();
  for (const group of ['command', 'file-read', 'file-change']) {
    for (const toggle of await activity.locator(`.thread-graph-history-group-${group} > .thread-graph-history-group-card > button`).all()) await toggle.click();
  }
  expect(details).toHaveLength(0);
  // Full timestamps used to consume all header width and squeeze Chinese into a column.
  await fileHeader.getByRole('button', { name: /切换时间戳/ }).click();
  await expect(fileHeader.locator('time')).not.toHaveText('17s');
  const headers = activity.locator('.thread-graph-history-group-toggle');
  const geometry = await headers.evaluateAll(nodes => nodes.map(n => {
    const label = n.querySelector('.thread-graph-history-group-summary')!, time = n.querySelector('.thread-graph-history-group-time')!;
    return { height: n.getBoundingClientRect().height, labelHeight: label.getBoundingClientRect().height, end: n.getBoundingClientRect().right, timeEnd: time.getBoundingClientRect().right, width: n.clientWidth, scrollWidth: n.scrollWidth };
  }));
  for (const row of geometry) {
    expect(row.height).toBeLessThanOrEqual(36);
    expect(row.labelHeight).toBeLessThanOrEqual(24);
    expect(row.timeEnd).toBeLessThanOrEqual(row.end);
    expect(row.scrollWidth).toBeLessThanOrEqual(row.width + 1);
  }
  const longFile = fileGroup.locator('.matter-step-title').last();
  await expect(longFile).toHaveCSS('white-space', 'nowrap');
  await expect(longFile).toHaveCSS('text-overflow', 'ellipsis');
  await fileHeader.getByRole('button', { name: /切换时间戳/ }).click();
  await expect(fileHeader.locator('time')).toHaveText('17s');
  await fileHeader.scrollIntoViewIfNeeded();
  await page.screenshot({ path: `.temp/activity-compact/screenshots/${testInfo.project.name}-expanded.png` });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (testInfo.project.name === 'mobile-chromium') {
    await page.setViewportSize({ width: 320, height: 800 });
    expect(await headers.evaluateAll(nodes => nodes.every(n => n.scrollWidth <= n.clientWidth + 1 && n.getBoundingClientRect().height <= 36))).toBe(true);
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
  await expect(turn.getByText('Implementation verified.', { exact: true })).toBeVisible();
  expect(turnRequests).toHaveLength(0);
  await page.reload();
  await expect(turn.locator('.thread-answer-section')).toHaveCount(2);
  expect(turnRequests).toHaveLength(0);
  await turn.getByRole('button', { name: /Expand turn 1/ }).click();
  await expect(turn.locator('.thread-answer-sequence > .thread-execution-timeline')).toHaveCount(2);
  expect(turnRequests).toHaveLength(1);
  const order = await turn.locator('.thread-answer-sequence > *').evaluateAll(elements => elements.map(element => element.className));
  expect(order).toEqual(['thread-execution-timeline', 'thread-answer-section', 'thread-execution-timeline', 'thread-answer-section']);
  const batches = turn.getByRole('button', { name: 'Expand 2 command entries', exact: true });
  await expect(batches).toHaveCount(2);
  await page.screenshot({ path: info.outputPath('substantial-answer-between-steps.png'), scale: 'css' });
  await batches.last().click();
  await expect(turn.getByText('check-after-one', { exact: true })).toBeVisible();
  await expect(turn.getByText('check-before-one', { exact: true })).toHaveCount(0);
  await turn.getByRole('button', { name: /Collapse turn 1/ }).click();
  await expect(turn.getByText('先回答你的问题', { exact: true })).toBeVisible();
  await expect(turn.getByText('Implementation verified.', { exact: true })).toBeVisible();
});
