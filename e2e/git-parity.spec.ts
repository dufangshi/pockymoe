import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 19110}`;
const shots = '/home/ubuntu/dev/remoteCodex/.scratch/2026-11-26-git-parity/git-ui/screenshots';
const patch = 'diff --git a/src/main.rs b/src/main.rs\nindex abc..def 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n }\n';
test('Git parity controls and themes', async ({ page, request }) => {
  const root = `/home/ubuntu/dev/remoteCodex/.scratch/2026-11-26-git-parity/git-ui/fixture-${randomUUID()}`;
  await mkdir(root, { recursive: true }); await mkdir(shots, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath: root, label: 'Git parity' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.thread?.id ?? started.id;
  const writes: string[] = [];
  await page.route(/\/api\/threads\/[^/]+\/(working-context|git\/|checkpoints)/, route => {
    const url = new URL(route.request().url());
    if (route.request().method() === 'POST') { writes.push(url.pathname); return route.fulfill({ json: url.pathname.endsWith('/preview') ? { files: [{ path: 'src/main.rs', change: 'modified' }], patch } : { status: {}, restored: true, undoId: 'undo-1', message: 'Fix parser' } }); }
    if (url.pathname.endsWith('/working-context')) return route.fulfill({ json: { primaryWorkingCopyId: 'main', configuredCwd: root, observations: [], workingCopies: [{ id: 'main', repositoryId: 'r', path: root, branch: 'feature/parser', head: 'abcdef0123', managed: false, source: 'configured' }] } });
    if (url.pathname.endsWith('/git/status')) return route.fulfill({ json: { workingCopyId: 'main', branch: 'feature/parser', head: 'abcdef0123', upstream: 'origin/main', ahead: 2, behind: 1, stashCount: 1, conflicted: [], files: [{ path: 'src/main.rs', indexStatus: 'M', worktreeStatus: 'M' }], truncated: false } });
    if (url.pathname.endsWith('/checkpoints')) return route.fulfill({ json: { turns: [{ turnId: 'turn-one', turnIndex: 1, status: 'completed', startedAt: '2026-11-26T04:00:00Z', workingCopies: [{ workingCopyId: 'main', available: true, files: 1, additions: 1, deletions: 1 }] }] } });
    if (url.pathname.endsWith('/git/diff')) return route.fulfill({ json: { workingCopyId: 'main', patch, truncated: false, area: url.searchParams.get('area') } });
    return route.fulfill({ json: { commits: [], hasMore: false } });
  });
  for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({ theme, mode }) => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); localStorage.setItem('pockymoe.workbench.inspector', 'git'); localStorage.setItem('pockymoe.phoneDock', 'off'); }, { theme, mode });
    await page.goto(`/threads/${id}`);
    const later = page.getByRole('button', { name: 'Later', exact: true });
    await later.waitFor({ timeout: 3000 }).catch(() => {});
    if (await later.isVisible()) await later.click();
    const dock = page.getByTestId('thread-git');
    await expect(dock.getByText('+    new();', { exact: false })).toBeVisible();
    await dock.getByRole('button', { name: 'Turn', exact: true }).click();
    await expect(dock.getByRole('button', { name: 'Restore before' })).toBeEnabled();
    await page.screenshot({ path: `${shots}/${theme}-${mode}-${width}-turn.png` });
    await dock.getByRole('button', { name: 'Restore before' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.screenshot({ path: `${shots}/${theme}-${mode}-${width}-restore.png` });
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    const added = dock.locator('[data-kind="add"]').first();
    await added.evaluate(node => { const selection = window.getSelection()!; const range = document.createRange(); range.selectNodeContents(node); selection.removeAllRanges(); selection.addRange(range); });
    await added.dispatchEvent('mouseup');
    await dock.getByRole('button', { name: 'Comment', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('textbox', { name: 'Comment', exact: true }).fill('Please handle the error here.');
    await page.screenshot({ path: `${shots}/${theme}-${mode}-${width}-review.png` });
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await dock.getByRole('button', { name: 'Staged 1', exact: true }).click();
    await expect(dock.getByRole('textbox', { name: 'Commit message' })).toBeVisible();
    await page.screenshot({ path: `${shots}/${theme}-${mode}-${width}-commit.png` });
    await dock.getByRole('textbox', { name: 'Commit message' }).fill('Fix parser');
    await dock.getByRole('button', { name: 'Commit', exact: true }).click();
    await expect.poll(() => writes.filter(path => path.endsWith('/git/commit')).length).toBeGreaterThan(0);
  }
});
