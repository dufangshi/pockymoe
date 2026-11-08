import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';

// The Git and isolation inspectors dock beside the conversation. Their data are
// route fixtures: no Dokploy, credentials or real repository. The Supervisor
// (fake runtime) only serves the threads.
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const GIB = 1024 ** 3;
const patch = ['diff --git a/src/main.rs b/src/main.rs', '--- a/src/main.rs', '+++ b/src/main.rs', '@@ -1,3 +1,3 @@', ' fn main() {', '-    let port = 8080;', '+    let port = read_port();', ' }', ''].join('\n');

async function thread(request: import('@playwright/test').APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `isolation-ui-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Isolation UI' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  return started.thread?.id ?? started.id as string;
}

async function open(page: Page, id: string) {
  await page.addInitScript(() => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); });
  await page.goto(`/threads/${id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
}

async function insideViewport(page: Page, selector: string) {
  const viewport = page.viewportSize()!;
  // Phone views rise in; measure once they settle.
  await expect.poll(async () => {
    const box = await page.locator(selector).last().boundingBox();
    return !!box && box.x >= -1 && box.x + box.width <= viewport.width + 1 && box.y + box.height <= viewport.height + 1;
  }).toBe(true);
}

const phone = (page: Page) => (page.viewportSize()?.width ?? 1000) < 640;

/** Open a dock view from the rail (desktop) or the phone top bar. */
async function dockEntry(page: Page, name: string | RegExp) {
  await page.getByRole('button', { name, exact: typeof name === 'string' }).first().click();
  return page.getByTestId('workbench-inspector');
}

async function composerUsable(page: Page) {
  const composer = page.getByTestId('primary-pane').getByRole('textbox').first();
  await composer.click();
  await composer.pressSequentially('still typing');
  await expect.poll(() => composer.evaluate(node => (node as HTMLTextAreaElement).value ?? node.textContent)).toContain('still typing');
}

function gitRoutes(page: Page, patches: Record<string, string>, diffs: string[] = []) {
  return page.route(/\/api\/threads\/[^/]+\/(working-context|git\/)/, route => {
    const url = new URL(route.request().url());
    const threadId = url.pathname.split('/')[3]!;
    if (url.pathname.endsWith('/working-context')) return route.fulfill({ json: { configuredCwd: '/repo', primaryWorkingCopyId: 'main', observations: [],
      workingCopies: [{ id: 'main', repositoryId: 'r', path: `/repo/${threadId}`, branch: 'main', head: 'abcdef0123', managed: false, source: 'configured', lastObservedAt: null }] } });
    if (url.pathname.endsWith('/git/status')) return route.fulfill({ json: { workingCopyId: 'main', branch: 'main', head: 'abcdef0123', truncated: false,
      files: [{ path: 'src/main.rs', indexStatus: ' ', worktreeStatus: 'M' }, { path: 'notes.md', indexStatus: 'A', worktreeStatus: ' ' }] } });
    if (url.pathname.endsWith('/git/diff')) { diffs.push(url.search); return route.fulfill({ json: { workingCopyId: 'main', area: url.searchParams.get('area'), patch: patches[threadId] ?? patch, truncated: false } }); }
    return route.fulfill({ json: { commits: [], hasMore: false } });
  });
}

test('Git docks beside a usable conversation and follows the open thread', async ({ page, request }) => {
  const first = await thread(request);
  const second = await thread(request);
  const diffs: string[] = [];
  await gitRoutes(page, { [first]: patch, [second]: patch.replace('read_port()', 'second_copy_port()') }, diffs);
  await open(page, first);
  const dock = await dockEntry(page, 'Git');
  await expect(dock).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Git' })).toBeVisible();
  const files = dock.getByRole('complementary', { name: 'Changed files' });
  await expect(dock.getByRole('button', { name: 'Unstaged 1' })).toHaveAttribute('aria-pressed', 'true');
  await expect(files.getByRole('button', { name: 'View diff for notes.md' })).toHaveCount(0);
  await files.getByRole('button', { name: 'View diff for src/main.rs' }).click();
  await expect.poll(() => diffs.at(-1)).toContain('path=src%2Fmain.rs');
  const added = dock.locator('[data-kind="add"]', { hasText: 'read_port()' });
  await expect(added).toBeVisible();
  expect(await added.evaluate(node => getComputedStyle(node).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
  if (phone(page)) {
    // Touch targets (UI §10): the scope switch is at least 40px tall.
    for (const name of ['Unstaged 1', 'Staged 1', 'Compare']) {
      expect((await dock.getByRole('button', { name, exact: true }).boundingBox())!.height).toBeGreaterThanOrEqual(40);
    }
    // A long working-copy name truncates instead of running under Refresh.
    const select = (await dock.getByRole('combobox', { name: 'Working copy' }).boundingBox())!;
    const refresh = (await dock.getByRole('button', { name: 'Refresh Git status' }).boundingBox())!;
    expect(select.x + select.width).toBeLessThanOrEqual(refresh.x + 1);
    expect(select.height).toBeGreaterThanOrEqual(40);
    // A phone has no room for both: the dock is a full view and Back closes it.
    await insideViewport(page, '[data-testid="workbench-inspector"]');
    await page.goBack();
    await expect(dock).toHaveCount(0);
    await composerUsable(page);
    return;
  }
  // Not modal: the conversation stays visible, scrollable and typeable beside it.
  const chat = (await page.getByTestId('primary-pane').boundingBox())!;
  const panel = (await dock.boundingBox())!;
  expect(chat.x + chat.width).toBeLessThanOrEqual(panel.x + 1);
  await composerUsable(page);
  // Keyboard resize through the shared separator.
  const separator = dock.getByRole('separator');
  const before = panel.width;
  await separator.focus();
  await page.keyboard.press('ArrowLeft');
  await expect.poll(async () => (await dock.boundingBox())!.width).toBeGreaterThan(before);
  // Another thread: the dock stays open and never shows the previous diff.
  await page.goto(`/threads/${second}`);
  await expect(page.getByTestId('workbench-inspector')).toBeVisible();
  await expect(page.getByTestId('workbench-inspector').locator('[data-kind="add"]', { hasText: 'second_copy_port()' })).toBeVisible();
  await expect(page.getByTestId('workbench-inspector').getByText('read_port()')).toHaveCount(0);
  // Escape inside the dock closes it and returns focus to its rail entry.
  await page.getByRole('button', { name: 'Git', exact: true }).first().click();
  await expect(page.getByTestId('workbench-inspector')).toHaveCount(0);
  await page.getByRole('button', { name: 'Git', exact: true }).first().click();
  await page.getByTestId('workbench-inspector').getByRole('button', { name: 'Unstaged 1' }).focus();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('workbench-inspector')).toHaveCount(0);
});

test('isolation review opens in the dock and approves the exact digest from its pinned decision', async ({ page, request }) => {
  const id = await thread(request);
  const connection = { id: 'lab', name: 'Lab Dokploy', baseUrl: 'https://dokploy.example.test', configured: true, dokployEnvironmentId: 'e', image: 'example/worker:1', storageNode: 'local', allowedRoots: ['/repo'], cpuLimit: 2, memoryLimitBytes: 4 * GIB, pidsLimit: 512, nofileLimit: 4096 };
  const limits = { cpu: 2, memoryBytes: 4 * GIB, pids: 512, nofile: 4096 };
  let state = 'pending';
  const approvals: { url: string; body: unknown }[] = [];
  const proposal = () => ({ id: 'req-1', threadId: id, digest: 'd'.repeat(64), state, environmentId: null, lastError: null, approvedAt: null, continuationQueued: false, createdAt: '2026-11-07T00:00:00Z', updatedAt: '2026-11-07T00:00:00Z',
    spec: { connection, threadId: id, workspaceId: 'w', workspacePath: '/repo', workingPath: '/repo.worktrees/feature', harness: 'codex', model: 'm', reasoningEffort: 'high', name: 'Isolate the feature work', limits,
      egressHosts: ['auth.openai.com', 'chatgpt.com'], authentication: { codex: 'nativeFile', claude: 'apiBroker' },
      credentialMounts: [{ source: '/home/dev/.codex', target: '/credentials/codex', readOnly: false, purpose: 'codex', environment: { CODEX_HOME: '/credentials/codex' } }], credentialMountKinds: ['directory'],
      handoff: 'Continue the feature.', migration: 'sameThreadFreshSession' } });
  await page.route(/\/api\/(environments|environment-requests|threads\/[^/]+\/environment$)/, route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/approve')) { approvals.push({ url: url.pathname, body: route.request().postDataJSON() }); state = 'waitingIdle'; return route.fulfill({ json: proposal() }); }
    if (url.pathname.includes('/environment-requests')) return route.fulfill({ json: [proposal()] });
    if (url.pathname.endsWith('/environment')) return route.fulfill({ json: { environment: null, supportedHarnesses: ['codex'], limitations: [] } });
    return route.fulfill({ json: { connections: [connection], environments: [], budget: { maxRunning: 2, maxCpu: 4, maxMemoryBytes: 8 * GIB, reservedCpu: 1, reservedMemoryBytes: GIB } } });
  });
  await open(page, id);
  const dock = await dockEntry(page, 'Isolated environments · approval needed');
  await dock.getByRole('region', { name: 'Where it runs' }).getByRole('button', { name: 'Review' }).click();
  const review = dock.getByTestId('isolation-request');
  await expect(review.getByRole('heading', { name: 'Isolate the feature work' })).toBeFocused();
  await expect(review.getByText('Native file login')).toBeVisible();
  await expect(review.getByText('Read and write')).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const approve = review.getByRole('button', { name: 'Approve and migrate when idle' });
  // The decision stays in view while the long review scrolls.
  await expect(approve).toBeInViewport();
  await insideViewport(page, '[data-testid="workbench-inspector"]');
  expect(approvals).toEqual([]);
  if (!phone(page)) await composerUsable(page);
  await approve.click();
  await expect(review.getByText('Approved, waiting for the current turn to exit').first()).toBeVisible();
  expect(approvals).toEqual([{ url: '/api/environment-requests/req-1/approve', body: { digest: 'd'.repeat(64) } }]);
  await expect(review.getByRole('list', { name: 'Migration progress' }).locator('[aria-current="step"]')).toContainText('Wait for idle');
  await review.getByRole('button', { name: 'Isolation requests' }).click();
  await expect(dock.getByRole('button', { name: /Isolate the feature work/ })).toBeFocused();
});
