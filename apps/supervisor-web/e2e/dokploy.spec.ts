import { expect, test, type Page } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { environmentsFixture, isolatedEnvironment } from '../src/test/environmentFixtures';
import type { EnvironmentDto } from '../src/lib/environmentApi';
import { progressStorageKey } from '../src/components/onboarding/tourStorage';

const screenshots = fileURLToPath(new URL('../../../.scratch/dokploy-ui/screenshots/', import.meta.url));
const workspace = { id: 'workspace-one', hostId: 'test-device', label: 'Environment UI', absPath: '/projects/pockymoe', isFavorite: false, createdAt: '2026-11-07', lastOpenedAt: null };
const makeThread = () => ({ id: 'thread-one', workspaceId: workspace.id, provider: 'codex', agentId: 'codex', providerSessionId: 'existing-native-session', parentThreadId: null as string | null, source: 'created', title: 'Dokploy environment flow', model: 'gpt-6.1-sol', reasoningEffort: 'max', collaborationMode: 'default', approvalMode: 'yolo', status: 'idle', summaryText: null, lastError: null, activeTurnId: null, isLoaded: true, isPinned: false, createdAt: '2026-11-07', updatedAt: '2026-11-07', lastTurnStartedAt: null, lastTurnCompletedAt: null });

async function mockApi(page: Page) {
  const snapshot = environmentsFixture();
  let binding: EnvironmentDto | null = null;
  const writes: { path: string; body: Record<string, unknown> | null }[] = [];
  const requests: { path: string; method: string }[] = [];
  const thread = makeThread();
  let createdThread: ReturnType<typeof makeThread> | null = null;
  let failTest = false;
  let failStart = false;
  await page.routeWebSocket('**/ws*', socket => socket.onMessage(() => {}));
  await page.route('**/healthz', route => route.fulfill({ json: { status: 'ok', timestamp: '2026-11-07', activeTurnCount: 0 } }));
  await page.route('**/api/**', async route => {
    const request = route.request(), url = new URL(request.url()), p = url.pathname;
    requests.push({ path: p, method: request.method() });
    const body = request.postData() ? request.postDataJSON() as Record<string, unknown> : null;
    if (request.method() !== 'GET') writes.push({ path: p, body });
    let result: unknown = {};
    if (p === '/api/threads/start') {
      createdThread = { ...thread, id: 'created-thread', title: String(body?.title || 'New isolated thread'), providerSessionId: '' };
      if (body?.environmentConnectionId) {
        binding = { ...isolatedEnvironment, id: 'new-thread-env', threadId: createdThread.id, connectionId: String(body.environmentConnectionId), state: 'stopped' };
        snapshot.environments.push(binding);
      }
      result = createdThread;
    } else if (p === '/api/environments' && request.method() === 'GET') result = snapshot;
    else if (p === '/api/environments' && request.method() === 'POST') {
      const env = { ...isolatedEnvironment, id: 'created-env', name: String(body?.name || 'New environment'), state: 'creating' as const };
      snapshot.environments.push(env); result = env;
    } else if (p.endsWith('/environment')) {
      if (request.method() === 'PUT') {
        binding = snapshot.environments.find(env => env.id === body?.environmentId) ?? null;
        if (body?.resetSession) thread.providerSessionId = '';
      }
      result = { environment: binding, supportedHarnesses: ['codex'], limitations: [] };
    } else if (p === '/api/environments/budget') { snapshot.budget = body as unknown as typeof snapshot.budget; result = snapshot.budget; }
    else if (p.endsWith('/test')) result = { ok: !failTest, detail: failTest ? 'Mock connection refused' : 'Connected' };
    else if (p.includes('/environments/connections/')) {
      const { apiKey, ...fields } = body!;
      const next = { ...snapshot.connections[0]!, ...fields, configured: true };
      snapshot.connections = [next]; result = next;
    } else if (p.startsWith('/api/environments/')) {
      const id = p.split('/')[3], action = p.split('/').at(-1);
      if (action === 'start' && failStart) { failStart = false; await route.fulfill({ status: 502, json: { code: 'environment_start_failed', message: 'Mock environment startup unavailable' } }); return; }
      const env = snapshot.environments.find(env => env.id === id)!;
      env.state = action === 'destroy' ? 'destroyed' : action === 'stop' ? 'stopped' : 'running';
      if (env.state === 'running') env.observedLimits = env.limits;
      result = env;
    } else if (p === '/api/auth/session') result = { authenticated: true, authRequired: false, username: null, expiresAt: null, mode: 'local' };
    else if (p === '/api/config/runtime') result = { appName: 'Pockymoe', appVersion: 'test', mode: 'local', host: 'localhost', port: 1, workspaceRoot: '/projects', environment: 'test', hostMode: 'standalone', capabilities: { terminal: false, tmux: false, managedSignals: false, windowsTaskScheduler: false } };
    else if (p === '/api/config/workspace-settings') result = { devHome: '/projects' };
    else if (p === '/api/workspaces') result = [workspace];
    else if (p === '/api/workspaces/workspace-one') result = workspace;
    else if (p.endsWith('/files/tree')) result = { path: '', name: 'pockymoe', kind: 'directory', children: [] };
    else if (p === '/api/storage') result = { deviceName: 'Mock device', scan: null, threads: [] };
    else if (p === '/api/threads' || p.endsWith('/threads') || p.endsWith('/group')) result = [thread];
    else if (p === '/api/threads/thread-one') result = { thread, workspace, workspacePathStatus: 'present', turns: [], pendingRequests: [], pendingSteers: [], totalTurnCount: 0 };
    else if (p === '/api/threads/created-thread') result = { thread: createdThread, workspace, workspacePathStatus: 'present', turns: [], pendingRequests: [], pendingSteers: [], totalTurnCount: 0 };
    else if (p.endsWith('/capabilities')) result = { availability: 'available', effectiveCapabilities: null, toolboxItems: [] };
    else if (p === '/api/agent-runtimes') result = ['codex', 'opencode'].map(provider => ({ provider, displayName: provider === 'codex' ? 'Codex' : 'OpenCode', enabled: true, capabilities: { sessions: { resume: true }, turns: { start: true } }, installation: { installed: true }, status: {} }));
    else if (p.endsWith('/models')) result = [{ id: 'model-a', model: 'model-a', displayName: 'Model A', isDefault: true, supportedReasoningEfforts: [], defaultReasoningEffort: null }];
    else if (p.endsWith('/agents') || p.endsWith('/plugins') || p.endsWith('/favorites')) result = [];
    else if (p.endsWith('/status')) result = { id: 'codex', name: 'Codex', status: { available: true, connected: true, installed: true, authenticated: true }, capabilities: { controls: {} } };
    else if (p.endsWith('/subscription-usage')) result = { usage: null };
    else if (p === '/api/thread-activity') result = { since: '2026-11-07', hours: 24, threads: [] };
    else if (p.includes('/context')) result = { enabled: false, supported: false, availability: 'unsupported', snapshot: null };
    else if (p.includes('/subagents')) result = { agents: [] };
    else if (p.includes('/watches')) result = [];
    else { await route.fulfill({ status: 404, json: { code: 'not_found', message: 'Mock endpoint unavailable' } }); return; }
    await route.fulfill({ json: result });
  });
  return { snapshot, writes, requests, thread, bindEnvironment: (environment: EnvironmentDto) => { binding = environment; }, failConnectionTest: () => { failTest = true; }, failNextStart: () => { failStart = true; } };
}
async function openSettings(page: Page) {
  await page.goto('/workspaces');
  await page.getByRole('button', { name: 'Open Navigation', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByTestId('settingsDialog');
  await settings.getByRole('tab', { name: 'Isolated environments', exact: true }).click();
  await expect(settings.getByRole('article', { name: 'Local Dokploy' })).toBeVisible();
  return settings;
}
async function openThreadEnvironment(page: Page) {
  await page.goto('/threads/thread-one');
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await page.getByRole('button', { name: 'Isolated environments', exact: true }).click();
  return page.getByRole('dialog', { name: 'Isolated environments', exact: true });
}
async function openNewThread(page: Page) {
  await page.goto('/threads/thread-one');
  await page.getByRole('button', { name: 'New Chat', exact: true }).last().click();
  const dialog = page.getByTestId('create-thread-dialog');
  await expect(dialog).toHaveCount(1);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('combobox', { name: 'Execution environment' })).toBeEnabled();
  return dialog;
}
test.beforeEach(async ({ page, baseURL }) => {
  await page.addInitScript(() => { localStorage.setItem('remote-codex-locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); });
  await page.addInitScript(key => localStorage.setItem(key, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), progressStorageKey('local:owner', new URL(baseURL!).origin));
});

test('saved connection, write-only replacement, budget and lifecycle follow the actual UI', async ({ page }) => {
  const mock = await mockApi(page);
  const settings = await openSettings(page);
  const connection = settings.getByRole('article', { name: 'Local Dokploy' });
  await connection.getByRole('button', { name: 'Edit', exact: true }).click();
  const editor = page.getByRole('dialog', { name: 'Edit connection', exact: true });
  await expect(editor.getByLabel('API key', { exact: false })).toHaveValue('');
  await editor.getByLabel('Name', { exact: true }).fill('Changed connection');
  await editor.getByRole('button', { name: 'Cancel', exact: true }).click();
  const discard = page.getByRole('dialog', { name: 'Discard unsaved changes?' });
  await expect(discard.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
  await discard.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(editor.getByLabel('Name', { exact: true })).toHaveValue('Changed connection');
  await editor.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(editor).toBeHidden();
  expect(mock.writes.find(write => write.path.includes('/connections/'))!.body).not.toHaveProperty('apiKey');
  const changed = settings.getByRole('article', { name: 'Changed connection' });
  await changed.getByRole('button', { name: 'Edit', exact: true }).click();
  await editor.getByLabel('API key', { exact: false }).fill('synthetic-browser-key');
  await editor.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(editor).toBeHidden();
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain('synthetic-browser-key');
  expect(await page.locator('body').innerText()).not.toContain('synthetic-browser-key');
  await changed.getByRole('button', { name: 'Test connection' }).click();
  await expect(changed.getByText('Connection successful')).toBeVisible();
  mock.failConnectionTest();
  await changed.getByRole('button', { name: 'Test connection' }).click();
  await changed.getByRole('button', { name: 'Why it failed' }).click();
  await expect(page.locator('.error-mark-bubble')).toContainText('Connection test failed');
  await page.keyboard.press('Escape');
  await settings.getByRole('button', { name: 'Edit budget' }).click();
  const budget = page.getByRole('dialog', { name: 'Edit budget', exact: true });
  await budget.getByLabel('Reserved CPU (cores)').fill('99');
  await expect(budget.getByRole('button', { name: 'Save' })).toBeDisabled();
  await budget.getByLabel('Reserved CPU (cores)').fill('2');
  await budget.getByRole('button', { name: 'Save' }).click();
  await expect(budget).toBeHidden();
  expect(mock.snapshot.budget.reservedCpu).toBe(2);
  const env = settings.getByRole('article', { name: 'Feature worktree' });
  await env.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(env.getByText('Running', { exact: true })).toBeVisible();
  await env.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(env.getByText('Stopped', { exact: true })).toBeVisible();
  await env.getByRole('button', { name: 'Rebuild', exact: true }).click();
  const rebuild = page.getByRole('dialog', { name: 'Rebuild this environment?' });
  await expect(rebuild).toContainText('keeping project files and environment state');
  await rebuild.getByRole('button', { name: 'Rebuild', exact: true }).click();
  await expect(env.getByText('Running', { exact: true })).toBeVisible();
  await env.getByRole('button', { name: 'Destroy', exact: true }).click();
  const confirm = page.getByRole('dialog', { name: 'Destroy this environment?' });
  await expect(confirm).toContainText('Keep project files, the original working copy and environment state');
  await expect(confirm.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
  expect(mock.writes.filter(write => write.path.endsWith('/destroy'))).toHaveLength(0);
  await confirm.getByRole('button', { name: 'Destroy', exact: true }).click();
  await expect(env.getByText('Destroyed', { exact: true })).toBeVisible();
});

test('thread entry creates and binds only after explicit native-session release', async ({ page }) => {
  const mock = await mockApi(page);
  const window = await openThreadEnvironment(page);
  await window.getByRole('button', { name: 'Create environment' }).click();
  await window.getByLabel('Environment name (optional)').fill('Thread sandbox');
  await window.getByRole('button', { name: 'Create and use' }).click();
  const confirm = page.getByRole('dialog', { name: 'Release the current session and switch environments?' });
  expect(mock.writes.filter(write => write.path === '/api/environments')).toHaveLength(0);
  await confirm.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(window.getByLabel('Environment name (optional)')).toHaveValue('Thread sandbox');
  await window.getByRole('button', { name: 'Create and use' }).click();
  await confirm.getByRole('button', { name: 'Release session and switch' }).click();
  await expect(window.getByText('Environment applied')).toBeVisible();
  expect(mock.writes.find(write => write.path.endsWith('/environment'))!.body).toEqual({ environmentId: 'created-env', resetSession: true });
  await expect(window.getByRole('article', { name: 'Thread sandbox' })).toContainText('Creating');
});

test('child thread keeps inherited isolation and explains the disabled controls', async ({ page }) => {
  const mock = await mockApi(page);
  mock.thread.parentThreadId = 'parent-thread';
  mock.bindEnvironment({ ...isolatedEnvironment, id: 'inherited-env', threadId: 'parent-thread', name: 'Inherited environment' });
  const window = await openThreadEnvironment(page);
  await expect(window.getByRole('combobox', { name: 'Select environment' })).toContainText('Inherited environment');
  await expect(window.getByRole('combobox', { name: 'Select environment' })).toBeDisabled();
  await expect(window.getByRole('button', { name: 'Create environment', exact: true })).toBeDisabled();
  await expect(window.getByRole('button', { name: 'Start', exact: true })).toBeDisabled();
  await window.getByRole('button', { name: 'Why it is unavailable' }).first().click();
  await expect(page.locator('.error-mark-bubble')).toContainText('This child thread inherits its parent');
  expect(mock.writes).toHaveLength(0);
});

test('New Chat buttons open one accessible dialog and restore focus to each trigger', async ({ page }) => {
  await mockApi(page);
  await page.goto('/threads/thread-one');
  const triggers = page.getByRole('button', { name: 'New Chat', exact: true });
  await expect(triggers).toHaveCount(2);
  for (const index of [0, 1]) {
    const trigger = triggers.nth(index);
    await trigger.click();
    const dialog = page.getByTestId('create-thread-dialog');
    await expect(dialog).toHaveCount(1);
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAccessibleName('Create New Chat');
    if (index === 0) await page.keyboard.press('Escape');
    else await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(trigger).toBeFocused();
  }
});

test('new thread isolation is submitted before startup and a failed startup never creates twice', async ({ page }) => {
  const mock = await mockApi(page); mock.failNextStart();
  const dialog = await openNewThread(page);
  await dialog.getByRole('combobox', { name: 'Execution environment' }).click();
  await page.getByRole('option', { name: 'Local Dokploy', exact: true }).click();
  await dialog.getByRole('textbox', { name: 'Model', exact: true }).fill('environment-model');
  const probesBefore = mock.requests.filter(request => /\/agent-runtimes\/.+\/(models|agents|harness|status|install|restart)$/.test(request.path)).length;
  await expect(dialog.getByText('Installed', { exact: true })).toHaveCount(0);
  await expect(dialog.getByText('ACP is not enabled on this supervisor.', { exact: false })).toHaveCount(0);
  await expect(dialog.getByText('Not yet verified in this environment.', { exact: false })).toBeVisible();
  // Changing a Harness after choosing isolation must not probe the Host.
  await dialog.locator('[id$="-thread-backend"]').click();
  await page.getByRole('option', { name: 'OpenCode', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Create Thread', exact: true })).toBeDisabled();
  await dialog.locator('[id$="-thread-backend"]').click();
  await page.getByRole('option', { name: 'Codex', exact: true }).click();
  await expect(dialog.getByRole('textbox', { name: 'Model', exact: true })).toHaveValue('environment-model');
  await dialog.getByLabel('Title', { exact: true }).fill('Created in isolation');
  await dialog.getByRole('button', { name: 'Create Thread', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Retry environment startup' })).toBeEnabled();
  await expect(dialog.getByRole('combobox', { name: 'Execution environment' })).toBeDisabled();
  await expect(dialog.getByLabel('Title', { exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Why it failed' }).click();
  await expect(page.locator('.error-mark-bubble')).toContainText('The thread was created, but its environment did not start');
  expect(mock.writes.filter(write => write.path === '/api/threads/start')).toHaveLength(1);
  expect(mock.writes.find(write => write.path === '/api/threads/start')!.body).toMatchObject({ title: 'Created in isolation', model: 'environment-model', environmentConnectionId: 'dokploy-local' });
  expect(mock.requests.filter(request => /\/agent-runtimes\/.+\/(models|agents|harness|status|install|restart)$/.test(request.path))).toHaveLength(probesBefore);
  const sequence = mock.requests.filter(request => request.path === '/api/threads/start' || request.path.endsWith('/environment') || request.path === '/api/environments/new-thread-env/start');
  expect(sequence.map(request => `${request.method} ${request.path}`)).toEqual(['POST /api/threads/start', 'GET /api/threads/created-thread/environment', 'POST /api/environments/new-thread-env/start']);
  await page.keyboard.press('Escape');
  await dialog.getByRole('button', { name: 'Retry environment startup' }).click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(/\/threads\/created-thread$/);
  await expect(page.getByTestId('primary-pane')).toBeVisible();
  expect(mock.writes.filter(write => write.path === '/api/threads/start')).toHaveLength(1);
  expect(mock.writes.filter(write => write.path === '/api/environments/new-thread-env/start')).toHaveLength(2);
});

test('unsupported environment download surfaces the server reason in the actual thread UI', async ({ page }) => {
  const mock = await mockApi(page);
  mock.bindEnvironment({ ...isolatedEnvironment, threadId: mock.thread.id });
  await page.route('**/api/workspaces/workspace-one/files/tree*', route => route.fulfill({ json: { path: '', name: 'pockymoe', kind: 'directory', children: [{ path: 'docs', name: 'docs', kind: 'directory', hasChildren: false, children: [] }] } }));
  await page.route('**/api/workspaces/workspace-one/files/download*', route => route.fulfill({ status: 501, json: { code: 'environment_unsupported', message: 'Archive downloads are unsupported in this isolated environment.' } }));
  await page.goto('/threads/thread-one');
  await page.getByRole('navigation', { name: 'Workspace tools', exact: true }).getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const row = page.getByRole('treeitem').filter({ hasText: 'docs' });
  await row.click({ button: 'right' });
  await page.getByRole('menuitem', { name: /Download/ }).click();
  const failure = page.getByRole('button', { name: 'Why it failed' });
  await expect(failure).toBeVisible(); await failure.click();
  await expect(page.locator('.error-mark-bubble')).toContainText('Archive downloads are unsupported in this isolated environment.');
});

for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
  test(`new thread theme ${theme} ${mode}`, async ({ page }, testInfo) => {
    await page.addInitScript(({ theme, mode }) => { localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); }, { theme, mode });
    await mockApi(page);
    const dialog = await openNewThread(page);
    await dialog.getByRole('combobox', { name: 'Execution environment' }).click();
    await page.getByRole('option', { name: 'Local Dokploy', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', theme);
    await expect(page.locator('html')).toHaveAttribute('data-theme-effective', mode);
    const directory = path.join(screenshots, 'new-chat-dialog', testInfo.project.name);
    await mkdir(directory, { recursive: true });
    const environment = dialog.getByRole('combobox', { name: 'Execution environment' });
    await environment.scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-selection.png`), animations: 'disabled', fullPage: true });
    const box = (await environment.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    await expect(dialog.getByRole('button', { name: 'Create Thread', exact: true })).toBeInViewport();
    const dialogBox = (await dialog.boundingBox())!;
    expect(dialogBox.y).toBeGreaterThanOrEqual(0);
    expect(dialogBox.y + dialogBox.height).toBeLessThanOrEqual(page.viewportSize()!.height + 1);
    if (testInfo.project.name === 'mobile-chromium') {
      expect(dialogBox.y + dialogBox.height).toBeCloseTo(page.viewportSize()!.height, 0);
      expect((await dialog.getByRole('button', { name: 'Close', exact: true }).boundingBox())!.height).toBeGreaterThanOrEqual(40);
    }
    await dialog.locator('[id$="-thread-backend"]').click();
    await page.getByRole('option', { name: 'OpenCode', exact: true }).click();
    await environment.click();
    await expect(page.getByRole('option', { name: 'Local Dokploy', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');
    await dialog.getByRole('button', { name: 'Why it is unavailable' }).click();
    await expect(page.locator('.error-mark-bubble')).toContainText('This Harness does not support isolated environments.');
    await expect(page.locator('.error-mark-bubble')).toBeVisible();
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-unsupported.png`), animations: 'disabled' });
  });
}

for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
  test(`theme inspection ${theme} ${mode}`, async ({ page }, testInfo) => {
    await page.addInitScript(({ theme, mode }) => { localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); }, { theme, mode });
    const mock = await mockApi(page);
    mock.snapshot.environments[0]!.state = 'error';
    mock.snapshot.environments[0]!.lastError = 'The configured mount is unavailable. Check the allowed project root.';
    const settings = await openSettings(page);
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', theme);
    await expect(page.locator('html')).toHaveAttribute('data-theme-effective', mode);
    const directory = path.join(screenshots, testInfo.project.name);
    await mkdir(directory, { recursive: true });
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-settings.png`), animations: 'disabled' });
    await settings.getByRole('article', { name: 'Local Dokploy' }).getByRole('button', { name: 'Edit', exact: true }).click();
    const editor = page.getByRole('dialog', { name: 'Edit connection', exact: true });
    await expect(editor.getByLabel('API key', { exact: false })).toBeVisible();
    const box = await editor.boundingBox();
    expect(box!.x).toBeGreaterThanOrEqual(0); expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
    if (testInfo.project.name.startsWith('mobile')) expect(box!.y + box!.height).toBeGreaterThanOrEqual(page.viewportSize()!.height - 1);
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-connection.png`), animations: 'disabled' });
    await editor.getByRole('button', { name: 'Save', exact: true }).scrollIntoViewIfNeeded();
    await expect(editor.getByLabel('CPU (cores)', { exact: true })).toBeVisible();
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-limits.png`), animations: 'disabled' });
    await page.keyboard.press('Escape');
    const env = settings.getByRole('article', { name: 'Feature worktree' });
    await env.getByRole('button', { name: /Feature worktree/ }).click();
    await env.getByRole('button', { name: 'Why it failed' }).click();
    await expect(page.locator('.error-mark-bubble')).toContainText('The configured mount is unavailable');
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-environment.png`), animations: 'disabled' });
    await page.keyboard.press('Escape');
    await env.getByText('Configured limits', { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-resources.png`), animations: 'disabled' });
    await page.keyboard.press('Escape');
    const threadWindow = await openThreadEnvironment(page);
    await expect(threadWindow.getByRole('button', { name: 'Create environment' })).toBeVisible();
    await threadWindow.getByRole('button', { name: 'Create environment' }).click();
    await page.screenshot({ path: path.join(directory, `${theme}-${mode}-thread.png`), animations: 'disabled' });
    const overflow = await threadWindow.evaluate(node => node.scrollWidth > node.clientWidth + 1);
    expect(overflow).toBe(false);
  });
}
