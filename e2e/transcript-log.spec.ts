import { expect, test, type APIRequestContext } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const prompt = 'Split prompt-queue.ts into ordering and storage';

async function createThread(request: APIRequestContext) {
  if (!process.env.E2E_WORKSPACE_ROOT || !process.env.E2E_DATABASE_URL)
    throw new Error('Use explicit isolated E2E database and workspace');
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT, `transcript-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspaceResponse = await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Transcript log' } });
  expect(workspaceResponse.ok()).toBeTruthy();
  const workspace = await workspaceResponse.json();
  const response = await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: workspace.id, title: 'Transcript log', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' },
  });
  expect(response.ok()).toBeTruthy();
  const created = await response.json();
  return created.id ?? created.thread.id as string;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('pockymoe-theme-preset', 'terminal');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

test('scripted transcript streams current work and plan, then folds files with lazy inline diffs', async ({ page, request }) => {
  const id = await createThread(request);
  const details: string[] = [];
  page.on('request', r => { if (/\/items\/[^/]+\/detail(?:\?|$)/.test(r.url())) details.push(r.url()); });
  await page.goto(`/threads/${id}`);
  await page.getByRole('textbox', { name: 'Prompt' }).fill(prompt);
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  const turn = page.getByTestId('chat-turn');
  const toggle = turn.locator('.thread-worked-toggle');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(turn.getByText("I'll start by reading the queue and its tests, then separate the storage code.", { exact: true })).toBeVisible();
  await expect(turn.locator('.tlog-current [data-status="running"]')).toContainText('rg -n "PromptQueue" src tests');
  await expect(turn.locator('.tlog-current [data-status="running"]')).toContainText('node --test tests/durable-queue.test.ts');
  await expect(turn.getByText('Persistence now lives in', { exact: false })).toBeVisible();
  expect(details).toEqual([]);
  await expect(turn.getByText('All queue tests pass.', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);

  // Reload exercises the server's compact activity and file summaries, not just streamed items.
  await page.reload();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  const completed = (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1);
  const seconds = Math.round((Date.parse(completed.completedAt) - Date.parse(completed.startedAt)) / 1000);
  // Twelve 900ms beats take about 10.8s; the UI rounds the real elapsed time.
  await expect(turn.locator('.thread-graph-worked-label')).toHaveText(`Worked for ${seconds}s`);
  expect(await toggle.locator('.tlog-part').evaluateAll(parts => parts.map(part => {
    const clone = part.cloneNode(true) as HTMLElement;
    clone.querySelectorAll('.tlog-sep').forEach(sep => sep.remove());
    return clone.textContent!.trim();
  }))).toEqual(['edited 1 file', '+2', '−1', '4 commands', 'Read 1 file', '1 failed']);
  await expect(toggle.locator('.tlog-sep')).toHaveCount(4);
  const receipt = turn.locator('.thread-turn-receipt');
  await expect(receipt).toBeVisible();
  const answer = turn.locator('[data-role="assistant"]').filter({ hasText: 'All queue tests pass.' });
  expect(await receipt.evaluate(el => el.getBoundingClientRect().top)).toBeGreaterThanOrEqual(await answer.evaluate(el => el.getBoundingClientRect().bottom));
  await expect(turn.locator('.thread-graph-worked-summary .thread-turn-usage')).toHaveCount(0);
  const chip = turn.locator('.tlog-file');
  await expect(chip.locator('.tlog-file-name')).toHaveText('queue-store.ts');
  await expect(chip.locator('.is-add')).toHaveText('+2');
  await expect(chip.locator('.is-remove')).toHaveText('−1');
  expect(details).toEqual([]);
  await chip.click();
  await expect(turn.locator('.tlog-files .thread-diff')).toContainText('const text = readFileSync');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(details).toHaveLength(1);
  await toggle.click();
  await expect(turn.locator('.tlog-files')).toHaveCount(0);
  const narration = turn.locator('.thread-execution-timeline [data-role="assistant"]');
  await expect(narration).toHaveCount(4);
  await expect(narration.nth(0)).toContainText("I'll start by reading");
  await expect(narration.nth(1)).toContainText('The queue tests pass.');
  await expect(narration.nth(2)).toContainText('One durable-queue test still fails');
  await expect(narration.nth(3)).toContainText('All queue tests pass.');
  await expect(turn.locator('.tlog-run')).toHaveCount(1);
  const run = turn.locator('.tlog-run-head');
  await expect(run).toHaveAttribute('aria-expanded', 'false');
  // The failed command and final command stand alone between narration messages.
  await expect(turn.locator('.tlog-single [data-kind="commandExecution"][data-status="failed"] .tlog-tag')).toHaveText('failed');
  expect(details).toHaveLength(1);
  await run.click();
  await expect(turn.locator('.tlog-run [data-kind="fileChange"] .tlog-row-head')).toHaveAttribute('aria-expanded', 'true');
  await expect(turn.locator('.tlog-run .thread-diff')).toContainText('const text = readFileSync');
  await expect(turn.locator('.matter-command-step, .thread-history-direct-row, .thread-graph-history-group')).toHaveCount(0);
});

test('phone changed-file chip opens its diff inside the folded turn without horizontal overflow', async ({ page, request }) => {
  const id = await createThread(request);
  const sent = await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt } });
  expect(sent.ok()).toBeTruthy();
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1)?.status).toBe('completed');
  await page.goto(`/threads/${id}`);
  const toggle = page.locator('.thread-worked-toggle');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  const chip = page.locator('.tlog-file');
  await expect(chip).toContainText('queue-store.ts');
  await chip.click();
  await expect(page.locator('.tlog-files .thread-diff')).toContainText('const text = readFileSync');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  for (const selector of ['.tlog-files-row', '[data-testid="thread-scroll-container"]'])
    expect(await page.locator(selector).evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await chip.click();
  await expect(page.locator('.tlog-files .thread-diff')).toHaveCount(0);
});


test('agent prompts render labelled envelopes and expandable notices', async ({ page, request }) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const prompts = [
    '[Pockymoe task from thread 12345678-1234-1234-1234-123456789012 | Review queue]\nCheck persistence boundaries.',
    '[Pockymoe inbox reminder]\n1 message is waiting.\n- result: Queue review finished',
    '[Pockymoe turn notification]\nReview thread finished. This describes the last turn.',
    'The Supervisor was restarted for device maintenance, interrupting this task. Continue reviewing the queue.',
  ];
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: prompts.length, thread: { ...detail.thread, status: 'idle', activeTurnId: null },
    turns: prompts.map((text, index) => ({ id: `envelope-${index}`, status: 'completed', startedAt: new Date().toISOString(), items: [
      { id: `prompt-${index}`, kind: 'userMessage', text },
      { id: `reply-${index}`, kind: 'agentMessage', text: `Acknowledgement ${index}`, responsePhase: 'final' },
    ] })),
  } }));
  await page.goto(`/threads/${id}`);
  const envelope = page.locator('[data-variant="envelope"]');
  await expect(envelope).toContainText('Review queue');
  await expect(envelope).toContainText('Check persistence boundaries.');
  await expect(envelope).not.toContainText('12345678-1234-1234-1234-123456789012');
  const notices = page.locator('[data-variant="notice"]');
  await expect(notices).toHaveCount(3);
  for (const [index, kind] of ['inbox', 'turn', 'resume'].entries()) {
    const notice = notices.nth(index);
    await expect(notice.locator('.prompt-notice')).toHaveAttribute('data-notice', kind);
    const head = notice.locator('.prompt-notice-head');
    await expect(head).toHaveAttribute('aria-expanded', 'false');
    await head.click();
    await expect(head).toHaveAttribute('aria-expanded', 'true');
    await expect(notice.locator('.prompt-notice-body')).toContainText(prompts[index + 1]!.split('\n').slice(kind === 'resume' ? 0 : 1).join('\n'));
  }
});


test('live transcript retains the latest narration and command without advancing the reported plan', async ({ page, request }) => {
  const id = await createThread(request);
  const plans: Array<Array<{ step: string; status: string }>> = [];
  page.on('websocket', socket => socket.on('framereceived', frame => {
    const event = JSON.parse(frame.payload.toString());
    if (event.threadId === id && event.type === 'thread.plan.updated') plans.push(event.payload.plan);
  }));
  try {
    await page.goto(`/threads/${id}`);
    await page.getByRole('textbox', { name: 'Prompt' }).fill(`${prompt} (live)`);
    await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
    const plan = page.locator('.tlog-plan');
    await expect(plan.locator('.tlog-plan-step')).toHaveCount(3);
    expect(plans.at(-1)?.map(step => step.status)).toEqual(['completed', 'inProgress', 'pending']);
    await expect(page.getByText("One durable-queue test still fails because of the fixture path; I'll fix that next.", { exact: true })).toBeVisible();
    await expect(page.locator('.tlog-current [data-status="running"]')).toContainText("sed -i 's#fixtures/queue.json#tests/fixtures/queue.json#'");
    await expect(page.locator('.thread-worked-toggle')).toHaveAttribute('aria-expanded', 'false');
    // Check the held state, after subsequent operation events and summary refreshes.
    // The first render of a plan can briefly be correct before a heuristic advances it.
    await expect(plan.locator('.tlog-plan-step')).toHaveCount(3);
    expect(plans.at(-1)?.map(step => step.status)).toEqual(['completed', 'inProgress', 'pending']);
    const reported = await plan.locator('.tlog-plan-step').evaluateAll(steps => steps.map(step => ({
      text: step.querySelector('.thread-graph-plan-step-text')!.textContent,
      status: step.getAttribute('data-status'),
    })));
    expect.soft(reported).toEqual([
      { text: 'Extract storage', status: 'completed' },
      { text: 'Update durable-queue tests', status: 'in_progress' },
      { text: 'Document the file format', status: 'pending' },
    ]);
  } finally {
    expect((await request.post(`${base}/api/threads/${id}/interrupt`, { data: {} })).ok()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1)?.status).toBe('interrupted');
  }
});
