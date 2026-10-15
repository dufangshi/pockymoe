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

test('scripted transcript reads open, folds continuously, and reveals files and command output only on request', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const details: string[] = [];
  const turnDetails: string[] = [];
  page.on('request', r => {
    if (/\/items\/[^/]+\/detail(?:\?|$)/.test(r.url())) details.push(decodeURIComponent(r.url()));
    if (/\/turns\/[^/?]+\/detail(?:\?|$)/.test(r.url())) turnDetails.push(r.url());
  });
  await page.goto(`/threads/${id}`);
  await page.getByRole('textbox', { name: 'Prompt' }).fill(prompt);
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  const turn = page.getByTestId('chat-turn');
  const toggle = turn.locator('.thread-worked-toggle');
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(turn.locator('.tlog-current')).toHaveCount(0);
  await expect(turn.getByText("I'll start by reading the queue and its tests, then separate the storage code.", { exact: true })).toBeVisible();
  await expect(turn.locator('.tlog-single [data-status="running"]')).toContainText('rg -n "PromptQueue" src tests');
  // Observe every DOM commit through completion, including the 260ms closing animation.
  await page.evaluate(() => {
    const samples = { seenWork: false, missingWork: 0, openCommands: 0, closing: false, count: 0 };
    const sample = () => {
      const turn = document.querySelector('[data-testid="chat-turn"]');
      if (!turn) return;
      samples.count++;
      const toggle = turn.querySelector('.thread-worked-toggle');
      if (toggle) samples.seenWork = true;
      else if (samples.seenWork) samples.missingWork++;
      samples.openCommands += turn.querySelectorAll('.tlog-row[data-kind="commandExecution"][data-open]').length;
      if (toggle?.getAttribute('aria-expanded') === 'false' && turn.querySelector('.thread-execution-timeline') &&
          turn.querySelector('.tlog-collapse[data-state="closed"]')) samples.closing = true;
    };
    const observer = new MutationObserver(sample);
    observer.observe(document.body, { childList: true, subtree: true, attributes: true });
    Object.assign(window, { transcriptSamples: samples, stopTranscriptSamples: () => { sample(); observer.disconnect(); return samples; } });
  });
  await expect(turn.locator('.tlog-run[data-open] [data-status="running"]')).toContainText('node --test tests/queue.test.ts');
  await expect(turn.locator('.tlog-run[data-open] [data-kind="fileChange"][data-open] .thread-diff')).toContainText('const text = readFileSync');
  await expect(turn.getByText('The queue tests pass.', { exact: false })).toBeVisible();
  await expect(turn.locator('.tlog-run').filter({ hasText: 'edited 1 file' })).not.toHaveAttribute('data-open');
  await expect(turn.locator('.tlog-run .thread-diff')).toHaveCount(0);
  expect(details.filter(url => !url.includes(':edit/detail'))).toEqual([]);
  await expect(turn.getByText('All queue tests pass.', { exact: false })).toBeVisible();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(turn.locator('.thread-execution-timeline')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);
  const samples = await page.evaluate(() => (window as unknown as { stopTranscriptSamples: () => object }).stopTranscriptSamples());
  await testInfo.attach('completion-observer', { body: JSON.stringify(samples), contentType: 'application/json' });
  expect(samples).toMatchObject({ seenWork: true, missingWork: 0, openCommands: 0, closing: true });
  expect(details.filter(url => !url.includes(':edit/detail'))).toEqual([]);
  details.length = 0;

  // Reload also checks compact server summaries, rather than only streamed items.
  await page.reload();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  const completed = (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1);
  const seconds = Math.round((Date.parse(completed.completedAt) - Date.parse(completed.startedAt)) / 1000);
  await expect(turn.locator('.thread-graph-worked-label')).toHaveText(`Worked for ${seconds}s`);
  await expect(toggle).toContainText('4 commands');
  await expect(toggle).toContainText('Read 1 file');
  await expect(toggle).toContainText('1 failed');
  const filesToggle = turn.locator('.tlog-files-toggle');
  await expect(filesToggle).toContainText('edited 1 file');
  await expect(filesToggle.locator('.is-add')).toHaveText('+2');
  await expect(filesToggle.locator('.is-remove')).toHaveText('−1');
  const receipt = turn.locator('.thread-turn-receipt');
  await expect(receipt).toBeVisible();
  const answer = turn.locator('[data-role="assistant"]').filter({ hasText: 'All queue tests pass.' });
  expect(await receipt.evaluate(el => el.getBoundingClientRect().top)).toBeGreaterThanOrEqual(await answer.evaluate(el => el.getBoundingClientRect().bottom));
  await expect(turn.locator('.thread-graph-worked-summary .thread-turn-usage')).toHaveCount(0);
  await expect(turn.locator('.tlog-files')).toHaveCount(0);
  expect(details).toEqual([]);
  const beforeFiles = turnDetails.length;
  await filesToggle.click();
  const file = turn.locator('.tlog-files .tlog-row[data-kind="fileChange"]');
  await expect(file).toHaveCount(1);
  await expect(file).toContainText('queue-store.ts');
  await expect(file.locator('.thread-diff')).toContainText('const text = readFileSync');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(turnDetails).toHaveLength(beforeFiles);
  expect(details).toHaveLength(1);
  expect(details[0]).toContain(':edit/detail');
  await file.locator('.tlog-viewable').click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
  const bounds = (await dialog.boundingBox())!;
  expect(bounds.width).toBeLessThan(page.viewportSize()!.width);
  expect(bounds.height).toBeLessThan(page.viewportSize()!.height);
  await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();
  await filesToggle.click();
  await expect(turn.locator('.tlog-files')).toHaveCount(0);

  await toggle.click();
  const narration = turn.locator('.thread-execution-timeline [data-role="assistant"]');
  await expect(narration).toHaveCount(3);
  await expect(answer).toHaveCount(1);
  for (const [index, text] of ["I'll start by reading", 'The queue tests pass.', 'One durable-queue test still fails'].entries())
    await expect(narration.nth(index)).toContainText(text);
  const run = turn.locator('.tlog-run-head');
  await expect(run).toHaveCount(1);
  await expect(run).toHaveAttribute('aria-expanded', 'false');
  await expect(run).toContainText('2 commands');
  await expect(run.locator('.is-time')).toContainText(/\d+(ms|s)/);
  await expect(turn.locator('.tlog-row[data-open]')).toHaveCount(0);
  await run.click();
  await expect(turn.locator('.tlog-run .tlog-row')).toHaveCount(4);
  await expect(turn.locator('.tlog-row[data-open]')).toHaveCount(0);
  expect(details).toHaveLength(1);
  const command = turn.locator('.tlog-row[data-kind="commandExecution"]').filter({ hasText: 'rg -n' });
  const copy = command.locator('.tlog-copy');
  await page.mouse.move(0, 0);
  await expect(copy).toHaveCSS('opacity', '0');
  await command.hover();
  await expect(copy).toHaveCSS('opacity', '0.55');
  await command.locator('.tlog-row-head').click();
  await expect(command).toHaveAttribute('data-open');
  await expect(command.locator('.tlog-output')).toContainText('export class PromptQueue');
  expect(details).toHaveLength(2);
  expect(details[1]).toContain(':rg/detail');
  await command.locator('.tlog-viewable').click();
  await expect(dialog).toContainText('export class PromptQueue');
  await expect(dialog.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();
  await expect(turn.locator('.tlog-detail button')).toHaveCount(0);
  await expect(turn.locator('.tlog-row[data-status="failed"] .tlog-tag')).toHaveText('failed');
  await expect(turn.locator('.matter-command-step, .thread-history-direct-row, .thread-graph-history-group')).toHaveCount(0);
});

test('phone file counts open only inline diffs and command copy appears only for an opened row', async ({ page, request }) => {
  const id = await createThread(request);
  const details: string[] = [];
  page.on('request', r => { if (/\/turns\/[^/?]+\/detail(?:\?|$)/.test(r.url())) details.push(r.url()); });
  expect((await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt } })).ok()).toBeTruthy();
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1)?.status).toBe('completed');
  await page.goto(`/threads/${id}`);
  const toggle = page.locator('.thread-worked-toggle');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  const filesToggle = page.locator('.tlog-files-toggle');
  const before = details.length;
  await filesToggle.tap();
  await expect(page.locator('.tlog-files .tlog-row[data-kind="fileChange"]')).toHaveCount(1);
  await expect(page.locator('.tlog-files .thread-diff')).toContainText('const text = readFileSync');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(details).toHaveLength(before);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  for (const selector of ['.tlog-files', '[data-testid="thread-scroll-container"]'])
    expect(await page.locator(selector).evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await filesToggle.tap();
  await expect(page.locator('.tlog-files')).toHaveCount(0);
  await toggle.tap();
  await page.locator('.tlog-run-head').tap();
  const command = page.locator('.tlog-run .tlog-row[data-kind="commandExecution"]').first();
  await expect(command.locator('.tlog-copy')).toHaveCSS('opacity', '0');
  await command.locator('.tlog-row-head').tap();
  await expect(command.locator('.tlog-copy')).toHaveCSS('opacity', '0.55');
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
    await expect(page.locator('.tlog-single [data-status="running"]')).toContainText("sed -i 's#fixtures/queue.json#tests/fixtures/queue.json#'");
    await expect(page.locator('.thread-worked-toggle')).toHaveAttribute('aria-expanded', 'true');
    await expect(page.locator('.tlog-current')).toHaveCount(0);
    await page.locator('.thread-worked-toggle').click();
    await expect(page.locator('.tlog-current [data-status="running"]')).toContainText("sed -i");
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
