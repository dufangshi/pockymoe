import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';

// The Git panel and the isolation review run against route fixtures: no Dokploy,
// credentials or real repository. The Supervisor (fake runtime) only serves the thread.
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

async function tool(page: Page, name: string | RegExp) {
  await page.locator('[aria-controls="matter-thread-tools"]').first().click();
  await page.getByRole('button', { name }).first().click();
}

async function insideViewport(page: Page, selector: string) {
  const viewport = page.viewportSize()!;
  // Phone sheets rise in; measure once they settle.
  await expect.poll(async () => {
    const box = await page.locator(selector).last().boundingBox();
    return !!box && box.x >= -1 && box.x + box.width <= viewport.width + 1 && box.y + box.height <= viewport.height + 1;
  }).toBe(true);
}

test('Git panel lists files per scope and shows a colored, file-scoped diff', async ({ page, request }) => {
  const id = await thread(request);
  const diffs: string[] = [];
  await page.route(/\/api\/threads\/[^/]+\/(working-context|git\/)/, route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/working-context')) return route.fulfill({ json: { configuredCwd: '/repo', primaryWorkingCopyId: 'main', observations: [],
      workingCopies: [{ id: 'main', repositoryId: 'r', path: '/repo', branch: 'main', head: 'abcdef0123', managed: false, source: 'configured', lastObservedAt: null }] } });
    if (url.pathname.endsWith('/git/status')) return route.fulfill({ json: { workingCopyId: 'main', branch: 'main', head: 'abcdef0123', truncated: false,
      files: [{ path: 'src/main.rs', indexStatus: ' ', worktreeStatus: 'M' }, { path: 'notes.md', indexStatus: 'A', worktreeStatus: ' ' }] } });
    if (url.pathname.endsWith('/git/diff')) { diffs.push(url.search); return route.fulfill({ json: { workingCopyId: 'main', area: url.searchParams.get('area'), patch, truncated: false } }); }
    return route.fulfill({ json: { commits: [], hasMore: false } });
  });
  await open(page, id);
  await tool(page, 'View Git changes');
  const dialog = page.getByRole('dialog', { name: 'Git' });
  const files = dialog.getByRole('complementary', { name: 'Changed files' });
  await expect(dialog.getByRole('button', { name: 'Unstaged 1' })).toHaveAttribute('aria-pressed', 'true');
  await expect(files.getByRole('button', { name: 'View diff for src/main.rs' })).toBeVisible();
  await expect(files.getByRole('button', { name: 'View diff for notes.md' })).toHaveCount(0);
  await files.getByRole('button', { name: 'View diff for src/main.rs' }).click();
  await expect.poll(() => diffs.at(-1)).toContain('path=src%2Fmain.rs');
  const added = dialog.locator('[data-kind="add"]', { hasText: 'read_port()' });
  await expect(added).toBeVisible();
  expect(await added.evaluate(node => getComputedStyle(node).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
  await insideViewport(page, '.thread-git-dialog');
});

test('isolation review keeps its decision reachable and approves the exact digest', async ({ page, request }) => {
  const id = await thread(request);
  const connection = { id: 'lab', name: 'Lab Dokploy', baseUrl: 'https://dokploy.example.test', configured: true, dokployEnvironmentId: 'e', image: 'example/worker:1', storageNode: 'local', allowedRoots: ['/repo'], cpuLimit: 2, memoryLimitBytes: 4 * GIB, pidsLimit: 512, nofileLimit: 4096 };
  const limits = { cpu: 2, memoryBytes: 4 * GIB, pids: 512, nofile: 4096 };
  let state = 'pending';
  const approvals: unknown[] = [];
  const proposal = () => ({ id: 'req-1', threadId: id, digest: 'd'.repeat(64), state, environmentId: null, lastError: null, approvedAt: null, continuationQueued: false, createdAt: '2026-11-07T00:00:00Z', updatedAt: '2026-11-07T00:00:00Z',
    spec: { connection, threadId: id, workspaceId: 'w', workspacePath: '/repo', workingPath: '/repo.worktrees/feature', harness: 'codex', model: 'm', reasoningEffort: 'high', name: 'Isolate the feature work', limits,
      egressHosts: ['auth.openai.com', 'chatgpt.com'], authentication: { codex: 'nativeFile', claude: 'apiBroker' },
      credentialMounts: [{ source: '/home/dev/.codex', target: '/credentials/codex', readOnly: false, purpose: 'codex', environment: { CODEX_HOME: '/credentials/codex' } }], credentialMountKinds: ['directory'],
      handoff: 'Continue the feature.', migration: 'sameThreadFreshSession' } });
  await page.route(/\/api\/(environments|environment-requests|threads\/[^/]+\/environment$)/, route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/approve')) { approvals.push(route.request().postDataJSON()); state = 'waitingIdle'; return route.fulfill({ json: proposal() }); }
    if (url.pathname.includes('/environment-requests')) return route.fulfill({ json: [proposal()] });
    if (url.pathname.endsWith('/environment')) return route.fulfill({ json: { environment: null, supportedHarnesses: ['codex'], limitations: [] } });
    return route.fulfill({ json: { connections: [connection], environments: [], budget: { maxRunning: 2, maxCpu: 4, maxMemoryBytes: 8 * GIB, reservedCpu: 1, reservedMemoryBytes: GIB } } });
  });
  await open(page, id);
  await tool(page, 'Isolated environments · approval needed');
  await page.getByRole('region', { name: 'Where it runs' }).getByRole('button', { name: 'Review' }).click();
  const review = page.getByRole('dialog', { name: 'Isolate the feature work' });
  await expect(review.getByText('Native file login')).toBeVisible();
  await expect(review.getByText('Read and write')).toBeVisible();
  const approve = review.getByRole('button', { name: 'Approve and migrate when idle' });
  // The decision stays on screen while the long review scrolls.
  await expect(approve).toBeInViewport();
  await insideViewport(page, '.environment-window.is-wide');
  expect(approvals).toEqual([]);
  await approve.click();
  await expect(review.getByText('Approved, waiting for the current turn to exit')).toBeVisible();
  expect(approvals).toEqual([{ digest: 'd'.repeat(64) }]);
  await expect(review.getByRole('list', { name: 'Migration progress' }).locator('[aria-current="step"]')).toContainText('Wait for idle');
});
