import { selectControl } from './selectControl';
import { expect, test, type APIRequestContext } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import WebSocket from 'ws';

// Real DSH through the bridge without model credentials: a scripted provider
// is inserted into the isolated E2E_DSH_HOME acp profile.
test.skip(
  process.env.E2E_REAL_DSH !== '1' || process.env.E2E_DSH_SCRIPTED !== '1',
  'Use E2E_REAL_DSH=1 E2E_DSH_SCRIPTED=1 with an isolated E2E_DSH_HOME and dsh on PATH',
);
test.describe.configure({ mode: 'serial' });

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const model = '["e2e-scripted","scripted"]';
const dshHome = path.resolve(process.env.E2E_DSH_HOME ?? '.local/missing-dsh-home');
const profile = path.join(dshHome, 'profiles', 'acp');
const workspaceDir = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `dsh-bridge-${Date.now()}`);
let threadId = '';

test.beforeAll(async ({ request }) => {
  if (dshHome === path.join(homedir(), '.dsh')) throw new Error('E2E_DSH_HOME must not be the working DSH home');
  if (!existsSync(path.join(profile, 'package.json'))) {
    execFileSync('dsh', ['--profile', 'acp', '--dump-config'], { env: { ...process.env, DSH_HOME: dshHome }, stdio: 'ignore' });
  }
  // Keyless fixtures: the scripted provider and a harmless plugin command.
  const patch = path.join(profile, 'cordis.patch.yml');
  for (const [id, file] of [
    ['pockymoe-e2e-scripted-llm', 'e2e/fixtures/dsh-scripted-llm.mjs'],
    ['pockymoe-e2e-command', 'e2e/fixtures/dsh-e2e-command.mjs'],
  ]) {
    const current = readFileSync(patch, 'utf8');
    if (current.includes(id)) continue;
    const row = `- insert:\n    - id: ${id}\n      name: ${JSON.stringify(path.resolve(file))}\n`;
    const rows = current.split('\n').filter(line => line.trim() && !line.trim().startsWith('#'));
    writeFileSync(patch, rows.join('').trim() === '[]' ? row : `${current.trimEnd()}\n${row}`);
  }
  mkdirSync(workspaceDir, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath: workspaceDir, label: 'DSH bridge' } })).json();
  const thread = await (await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: workspace.id, provider: 'acp', agentId: 'deepseek', model, title: 'DSH bridge', approvalMode: 'guarded' },
  })).json();
  expect(thread.model).toBe(model);
  threadId = thread.id;
});

const detail = async (request: APIRequestContext) => (await request.get(`${base}/api/threads/${threadId}`)).json();
const harness = async (request: APIRequestContext, id = threadId) =>
  (await (await request.get(`${base}/api/threads/${id}/capabilities`)).json()).negotiated.harness;
async function runTurn(request: APIRequestContext, prompt: string, data: Record<string, unknown> = {}) {
  const before = (await detail(request)).turns.length;
  const accepted = await request.post(`${base}/api/threads/${threadId}/prompt`, { data: { prompt, ...data } });
  expect(accepted.ok(), await accepted.text()).toBeTruthy();
  await expect.poll(async () => {
    const current = await detail(request);
    return current.turns.length > before && current.thread.status !== 'running' ? current.turns.at(-1).status : 'pending';
  }, { timeout: 60_000 }).toBe('completed');
  return (await detail(request)).turns.at(-1);
}

test('product read-only reaches the DSH sandbox', async ({ request }) => {
  expect((await request.patch(`${base}/api/threads/${threadId}/settings`, { data: { sandboxMode: 'read-only' } })).ok()).toBeTruthy();
  expect((await harness(request)).session.projections.permissions.currentValue).toBe('read-only');
  await runTurn(request, 'CALL bash {"description":"probe","command":"echo x > denied.txt"}');
  expect(existsSync(path.join(workspaceDir, 'denied.txt'))).toBe(false);
});

test('streams live text and keeps one committed answer', async ({ request }) => {
  const socket = new WebSocket(`${base.replace('http', 'ws')}/ws`);
  const deltas: string[] = [];
  socket.on('message', raw => {
    const event = JSON.parse(String(raw));
    if (event.threadId === threadId && event.type === 'thread.output.delta') deltas.push(event.payload.delta);
  });
  await new Promise(resolve => socket.once('open', resolve));
  const turn = await runTurn(request, 'stream please');
  socket.close();
  expect(deltas.length).toBeGreaterThan(1);
  expect(deltas.join('')).toBe('Plain answer SCRIPTED_OK for: stream please');
  expect(turn.items.filter((item: { kind: string }) => item.kind === 'agentMessage').map((item: { text: string }) => item.text))
    .toEqual(['Plain answer SCRIPTED_OK for: stream please']);
});

test('reviews a DSH plan in Pockymoe and follows its exit', async ({ request }) => {
  expect((await request.patch(`${base}/api/threads/${threadId}/settings`, { data: { collaborationMode: 'plan' } })).ok()).toBeTruthy();
  expect((await harness(request)).session.projections.plan.active).toBe(true);
  const accepted = await request.post(`${base}/api/threads/${threadId}/prompt`, {
    data: { prompt: 'CALL exit_plan_mode {"plan":"# Plan\\n- ship"}', collaborationMode: 'plan' },
  });
  expect(accepted.ok()).toBeTruthy();
  await expect.poll(async () => (await detail(request)).pendingRequests?.length ?? 0).toBe(1);
  const [review] = (await detail(request)).pendingRequests;
  expect(review.questions[0].options.map((option: { label: string }) => option.label)).toEqual(['Approve', 'Keep planning']);
  expect(review.questions[0].question).toContain('# Plan');
  const answer = { [review.questions[0].id]: { answers: ['Approve'] } };
  expect((await request.post(`${base}/api/threads/${threadId}/requests/${review.id}/respond`, { data: { allow: true, answers: answer } })).ok()).toBeTruthy();
  await expect.poll(async () => (await detail(request)).thread.status).toBe('idle');
  expect((await harness(request)).session.projections.plan.active).toBe(false);
  expect((await detail(request)).thread.collaborationMode).toBe('default');
});

test('keeps goal rounds inside one stoppable turn', async ({ request }) => {
  const accepted = await request.post(`${base}/api/threads/${threadId}/prompt`, { data: { prompt: 'CALL create_goal {"objective":"e2e goal"}' } });
  expect(accepted.ok()).toBeTruthy();
  await expect.poll(async () => {
    const turn = (await detail(request)).turns.at(-1);
    return turn.items.filter((item: { text: string }) => /Round: \d+/.test(item.text)).length;
  }, { timeout: 30_000 }).toBeGreaterThanOrEqual(2);
  const running = await detail(request);
  expect(running.thread.status).toBe('running');
  expect(running.goal.objective).toBe('e2e goal');
  expect((await request.post(`${base}/api/threads/${threadId}/interrupt`, { data: {} })).ok()).toBeTruthy();
  await expect.poll(async () => (await detail(request)).turns.at(-1).status).toBe('interrupted');
  await expect.poll(async () => (await harness(request)).session.projections.goal.goal.phase).toBe('paused');
  expect((await harness(request)).session.running).toBe(false);
});

test('panel toggles a plugin after backing up the profile', async ({ page, request }) => {
  // Start from the shipped state even if an earlier run stopped midway.
  const action = (data: Record<string, unknown>) => request.post(`${base}/api/threads/${threadId}/harness`, { data });
  const shipped = (await (await action({ kind: 'refresh' })).json()).harness.plugins
    .find((entry: { id: string }) => entry.id === 'include:repeat-tool-reminder');
  if (!shipped.enabled) {
    expect((await action({ kind: 'setPluginEnabled', id: 'include:repeat-tool-reminder', enabled: true })).ok()).toBeTruthy();
    expect((await action({ kind: 'restart' })).ok()).toBeTruthy();
    expect((await action({ kind: 'refresh' })).ok()).toBeTruthy();
  }
  const backups = path.join(profile, '.remote-codex', 'backups');
  // The profile keeps its newest 10 backups, so look for a new one, not a count.
  const before = new Set(existsSync(backups) ? readdirSync(backups) : []);
  await page.goto(`/threads/${threadId}`);
  await page.getByRole('button', { name: 'Open slash toolbox' }).click();
  await page.getByRole('button', { name: /\/harness/ }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Harness settings' });
  await expect(dialog.getByText(/DeepSeek Harness .* acp profile/)).toBeVisible();
  await expect(dialog.getByText('e2e goal · paused', { exact: false })).toBeVisible();
  await dialog.getByLabel('Filter plugins').fill('repeat-tool-reminder');
  const plugin = dialog.getByRole('checkbox', { name: 'Enable @deepseek-ai/dsh-repeat-tool-reminder' });
  await plugin.uncheck();
  await expect(dialog.getByText('Disabled · applies after reconnect')).toBeVisible();
  // The reconnect offer appears once DSH confirms the saved change.
  await expect(dialog.getByRole('button', { name: 'Reconnect to apply' })).toBeVisible();
  expect(readdirSync(backups).filter(name => !before.has(name))).toHaveLength(1);
  expect(readFileSync(path.join(profile, 'cordis.patch.yml'), 'utf8')).toContain('id: repeat-tool-reminder');
  // Reconnect restarts the DSH process; the new one loads the saved profile.
  await dialog.getByRole('button', { name: 'Reconnect to apply' }).click();
  await expect(dialog.getByRole('button', { name: 'Reconnect to apply' })).toHaveCount(0);
  await expect(plugin).not.toBeChecked();
  await expect(dialog.getByText('Disabled · applies after reconnect')).toHaveCount(0);
  // The old process kept running the plugin; only a new one reports it off.
  const live = (await harness(request)).plugins.find((entry: { id: string }) => entry.id === 'include:repeat-tool-reminder');
  expect(live.enabled).toBe(false);
  const restored = await (await request.post(`${base}/api/threads/${threadId}/harness`, {
    data: { kind: 'setPluginEnabled', id: 'include:repeat-tool-reminder', enabled: true },
  })).json();
  expect(restored.result.application).toBe('restart-required');
  expect((await request.post(`${base}/api/threads/${threadId}/harness`, { data: { kind: 'restart' } })).ok()).toBeTruthy();
  const blocked = await request.post(`${base}/api/threads/${threadId}/harness`, {
    data: { kind: 'setBundleEnabled', name: '@deepseek-ai/dsh-headless', enabled: true },
  });
  expect(blocked.ok()).toBeFalsy();
});

async function startThread(request: APIRequestContext, title: string) {
  const workspaces = await (await request.get(`${base}/api/workspaces`)).json();
  const workspace = workspaces.find((entry: { absPath: string }) => entry.absPath === workspaceDir);
  return (await (await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: workspace.id, provider: 'acp', agentId: 'deepseek', model, title, approvalMode: 'guarded' },
  })).json()).id as string;
}
async function answer(request: APIRequestContext, id: string, prompt: string) {
  const before = (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.length;
  expect((await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt } })).ok()).toBeTruthy();
  await expect.poll(async () => {
    const current = await (await request.get(`${base}/api/threads/${id}`)).json();
    return current.turns.length > before && current.thread.status !== 'running' ? current.turns.at(-1).status : 'pending';
  }, { timeout: 60_000 }).toBe('completed');
  const turn = (await (await request.get(`${base}/api/threads/${id}`)).json()).turns.at(-1);
  return turn.items.filter((item: { kind: string }) => item.kind === 'agentMessage').map((item: { text: string }) => item.text).join('');
}

test('New Chat starts a DSH thread in the chosen run mode', async ({ page, request }, testInfo) => {
  await page.goto(`/threads/${threadId}`);
  await page.getByRole('button', { name: 'New Chat', exact: true }).first().click();
  const dialog = page.getByTestId('create-thread-dialog');
  await selectControl(page, dialog.locator('[role=combobox][id$="thread-backend"]'), 'acp');
  await dialog.locator('label').filter({ hasText: 'DeepSeek Harness' }).click();
  await selectControl(page, dialog.getByLabel('Model', { exact: true }), model);
  const runMode = dialog.getByLabel('Run mode', { exact: true });
  // PTC needs a Node.js build with TypeScript support; without it the mode is shown disabled.
  await expect(runMode.locator('option')).toHaveText(['Standard (default)', /^PTC( \(unavailable: .+\))?$/, 'Minimal', 'Creator']);
  await selectControl(page, runMode, 'minimal');
  await expect(dialog.getByText(/Only a persistent terminal/)).toBeVisible();
  await dialog.getByLabel('Title', { exact: true }).fill('DSH minimal mode');
  await page.screenshot({ path: testInfo.outputPath('new-chat-run-mode.png') });
  await dialog.getByRole('button', { name: 'Create Thread', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const id = page.url().split('/threads/')[1]!;
  // DSH offers the Minimal tool set to the model from the first turn.
  expect(await answer(request, id, 'TOOLS?')).toBe('TOOLS=bash');
  const locked = await request.post(`${base}/api/threads/${id}/harness`, { data: { kind: 'selectRunMode', id: 'standard' } });
  expect(locked.ok()).toBeFalsy();
  // Minimal registers no plan mode, goals or compaction: the thread hides them.
  const caps = await (await request.get(`${base}/api/threads/${id}/capabilities`)).json();
  expect(caps.effectiveCapabilities.controls.planMode).toBe(false);
  expect(caps.effectiveCapabilities.turns.compact).toBe(false);
});

test('the DeepSeek Harness plugin panel switches modes, runs plugin commands and opens the native console', async ({ page, request, context }, testInfo) => {
  const id = await startThread(request, 'DSH plugin panel');
  await page.goto(`/threads/${id}`);
  await page.getByRole('button', { name: 'DeepSeek Harness', exact: true }).click();
  const panel = page.getByTestId('dsh-plugin-panel');
  const runMode = panel.getByRole('combobox', { name: 'Run mode' });
  await expect(runMode).toBeEnabled();
  // Creator mode works on any Node.js; PTC needs one with TypeScript support.
  await selectControl(page, runMode, 'cordis');
  await expect.poll(async () => (await harness(request, id)).session.projections.agentPreset).toBe('cordis');
  const commands = panel.getByRole('region', { name: 'Commands' });
  await expect(commands).toContainText('/planThread control');
  await expect(commands).toContainText('/exportIn the native console');
  await commands.getByRole('textbox', { name: 'Arguments for /e2e-echo' }).fill('from the panel');
  await commands.getByRole('button', { name: 'Run /e2e-echo' }).click();
  await expect(commands.getByRole('status')).toHaveText('E2E_ECHO from the panel');
  await page.screenshot({ path: testInfo.outputPath('dsh-plugin-panel.png') });
  // The native console opens in a new tab through the loopback proxy.
  const [consolePage] = await Promise.all([
    context.waitForEvent('page'),
    panel.getByRole('button', { name: 'Open console' }).click(),
  ]);
  // The token redirect lands on the clean index with DSH's own login cookie.
  await consolePage.waitForURL(/^http:\/\/(localhost|127\.0\.0\.1):\d+\/$/);
  await consolePage.waitForTimeout(3000);
  await consolePage.screenshot({ path: testInfo.outputPath('dsh-native-console.png') });
  await expect(consolePage.locator('body')).not.toContainText('authentication required');
  await consolePage.close();
  // The first turn fixes the mode.
  expect(await answer(request, id, 'hello')).toContain('SCRIPTED_OK');
  await expect.poll(async () => (await harness(request, id)).session.presetLocked).toBe(true);
});


test('a run mode without plan mode cannot silently drop the thread plan', async ({ request }) => {
  const id = await startThread(request, 'DSH plan and run mode');
  expect((await request.patch(`${base}/api/threads/${id}/settings`, { data: { collaborationMode: 'plan' } })).ok()).toBeTruthy();
  expect((await harness(request, id)).session.projections.plan.active).toBe(true);
  const refused = await request.post(`${base}/api/threads/${id}/harness`, { data: { kind: 'selectRunMode', id: 'minimal' } });
  expect(refused.ok()).toBeFalsy();
  expect((await refused.json()).message).toMatch(/has no plan mode/);
  // DSH is back on the previous mode, with plan mode still in force.
  const after = await harness(request, id);
  expect(after.session.projections.agentPreset).toBe('standard');
  expect(after.session.projections.plan.active).toBe(true);
});

async function runTurnOn(request: APIRequestContext, id: string, prompt: string) {
  const accepted = await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt } });
  expect(accepted.ok(), await accepted.text()).toBeTruthy();
  await expect.poll(async () => {
    const current = await (await request.get(`${base}/api/threads/${id}`)).json();
    return current.turns.length && current.thread.status !== 'running' ? current.turns.at(-1).status : 'pending';
  }, { timeout: 60_000 }).toBe('completed');
}

// DSH takes inline images only when the model it boots with reads them; a
// photo for any other model arrives as a file link and the turn still finishes.
for (const [name, route, expected] of [
  ['a vision model sees an attached photo inline', '["e2e-scripted","vision"]', /IMAGES=1 LINKS=0 TYPES=image\/png/],
  ['a text-only model gets an attached photo as a file link', '["e2e-scripted","scripted"]', /IMAGES=0 LINKS=1/],
  ['a photo after switching away from the vision model is a file link', '["e2e-scripted","vision"]', /IMAGES=0 LINKS=1/],
] as const) test(name, async ({ request }) => {
  const absPath = path.join(workspaceDir, name.replace(/\W+/g, '-'));
  mkdirSync(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'DSH images' } })).json();
  const thread = await (await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: workspace.id, provider: 'acp', agentId: 'deepseek', model: route, title: name, approvalMode: 'guarded' },
  })).json();
  if (name.includes('switching')) {
    // The process booted with the vision model; the thread now uses a text-only one.
    await runTurnOn(request, thread.id, 'hello');
    const switched = await request.patch(`${base}/api/threads/${thread.id}/settings`, { data: { model: '["e2e-scripted","scripted"]' } });
    expect(switched.ok(), await switched.text()).toBeTruthy();
  }
  // A 1×1 red PNG.
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==', 'base64');
  const accepted = await request.post(`${base}/api/threads/${thread.id}/prompt`, { multipart: {
    prompt: 'IMAGES? [PHOTO red.png]',
    attachmentManifest: JSON.stringify([{ clientId: 'red', kind: 'photo', originalName: 'red.png', placeholder: '[PHOTO red.png]' }]),
    attachments: { name: 'red.png', mimeType: 'image/png', buffer: png },
  } });
  expect(accepted.ok(), await accepted.text()).toBeTruthy();
  await expect.poll(async () => {
    const current = await (await request.get(`${base}/api/threads/${thread.id}`)).json();
    const turn = current.turns.at(-1);
    return turn && current.thread.status !== 'running' ? `${turn.status} ${turn.items.map((item: { text?: string }) => item.text ?? '').join(' ')}` : 'pending';
  }, { timeout: 60_000 }).toMatch(new RegExp(`^completed .*${expected.source}`));
});
