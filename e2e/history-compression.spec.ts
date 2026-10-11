import { expect, test } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

// Direct (relay-less) mode: JSON history is HTTP-compressed and compact
// histories defer every tool kind's detail until the item is opened.
test('direct mode compresses compact history and opens deferred tool detail', async ({ page, request }) => {
  if (!process.env.E2E_DATABASE_URL || !process.env.E2E_WORKSPACE_ROOT)
    throw new Error('Use explicit isolated E2E database and workspace');
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT, `compression-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Compression' } })).json();
  const created = await (await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: workspace.id, title: 'Compression', provider: 'claude', model: 'default', approvalMode: 'yolo' },
  })).json();
  const id = created.id ?? created.thread.id;
  const turnId = `${id}:compressed`;
  const at = new Date(Date.now() - 60_000).toISOString();
  const toolDetail = `Input:\n{"query":"compression"}\n\nResult:\n${'tool output line\n'.repeat(400)}END-OF-TOOL-DETAIL`;
  const items = [
    { id: 'prompt', kind: 'userMessage', text: 'Measure the payload' },
    { id: 'read-1', kind: 'fileRead', text: 'src/history.rs', previewText: 'src/history.rs', status: 'completed', detailText: 'fn main() {}\n'.repeat(500) },
    { id: 'tool-1', kind: 'toolCall', text: 'mcp lookup', previewText: 'mcp lookup', status: 'completed', detailText: toolDetail },
    { id: 'cmd-1', kind: 'commandExecution', text: 'cargo test', previewText: 'cargo test', status: 'completed', detailText: 'ok\n'.repeat(500) },
    { id: 'reply', kind: 'agentMessage', text: `Summary of the work. ${'The history payload is now compact. '.repeat(60)}` },
  ];
  execFileSync('python3', ['-c', `
import json, sqlite3, sys
v=json.load(sys.stdin)
with sqlite3.connect(sys.argv[1]) as conn:
  conn.create_function('search_body', 3, lambda text, kind, source: text or '', deterministic=True)
  conn.create_function('search_fold', 1, lambda text: (text or '').lower(), deterministic=True)
  conn.execute('INSERT INTO thread_turns(id,thread_id,status,model,started_at,completed_at,ordinal) VALUES(?,?,?,?,?,?,0)',(v['turn'],v['id'],'completed','default',v['at'],v['at']))
  for n, item in enumerate(v['items']):
    item=dict(item, createdAt=v['at'], sequence=n, sourceTurnId=v['turn'])
    conn.execute('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?)',(v['turn']+item['id'],v['id'],v['turn'],item['id'],json.dumps(item),v['at'],v['at']))
`, path.resolve(process.env.E2E_DATABASE_URL)], { input: JSON.stringify({ id, turn: turnId, at, items }) });

  const summaryResponse = page.waitForResponse((r) => r.url().includes(`/api/threads/${id}?`) && r.url().includes('view=summary'));
  await page.goto(`/threads/${id}`);
  const summary = await summaryResponse;
  expect(summary.url()).toContain('compact=1');
  expect(['gzip', 'br']).toContain((await summary.allHeaders())['content-encoding']);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();

  const detailResponse = page.waitForResponse((r) => r.url().includes(`/turns/${encodeURIComponent(turnId)}/detail`));
  await page.locator('.thread-graph-worked-summary').getByRole('button', { name: /Expand turn 1$/ }).click();
  const detail = await detailResponse;
  expect(detail.url()).toContain('compact=1');
  expect(['gzip', 'br']).toContain((await detail.allHeaders())['content-encoding']);
  const turn = await detail.json();
  for (const item of turn.items.filter((item: { kind: string }) => ['fileRead', 'toolCall', 'commandExecution'].includes(item.kind))) {
    expect(item.detailText).toBeUndefined();
    expect(item.hasDeferredDetail).toBe(true);
    expect(item.previewText).toBeUndefined();
    expect(item.sourceTurnId).toBeUndefined();
  }

  // Batches nest (activity > tool calls); open each level until the tool row shows.
  const toolButton = page.getByRole('button', { name: 'Open full tool call', exact: true });
  for (let pass = 0; pass < 3 && !(await toolButton.isVisible()); pass += 1) {
    const toggles = page.locator('.thread-graph-history-group button[aria-expanded="false"]');
    for (const toggle of await toggles.all()) if (await toggle.isVisible()) await toggle.click();
  }
  const itemResponse = page.waitForResponse((r) => r.url().includes(`/api/threads/${id}/items/tool-1/detail`));
  await toolButton.click();
  expect((await itemResponse).ok()).toBe(true);
  await expect(page.getByText(/END-OF-TOOL-DETAIL/)).toBeVisible();

  // Legacy clients (old Web, native apps) keep the full encoding.
  const legacy = await (await request.get(`${base}/api/threads/${id}/turns/${encodeURIComponent(turnId)}/detail`)).json();
  expect(legacy.items.find((item: { id: string }) => item.id === 'tool-1').detailText).toBe(toolDetail);
});
