import { expect, type APIRequestContext, type Page, type Locator } from '@playwright/test';
import { mkdir, mkdtemp } from 'node:fs/promises';
import path from 'node:path';
export const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
export async function isolatedTerminal(page: Page, request: APIRequestContext, title: string) {
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  await mkdir(process.env.E2E_WORKSPACE_ROOT!, { recursive: true });
  const absPath = await mkdtemp(path.join(process.env.E2E_WORKSPACE_ROOT!, 'mouse-selection-'));
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: title } })).json();
  const thread = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('pockymoe.terminal', JSON.stringify({ copyOnSelect: false }));
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.goto(`/threads/${thread.id ?? thread.thread.id}`);
  await page.getByRole('navigation', { name: 'Workspace tools', exact: true }).getByRole('button', { name: 'Terminal', exact: true }).click();
  const panel = page.getByTestId('workbench-bottom-panel');
  await panel.getByRole('button', { name: 'New terminal', exact: true }).first().click();
  const host = page.locator('[data-testid="workbench-terminal-target"] .shell-pane-active .shell-pane-host:visible');
  const rows = host.locator('.xterm-rows');
  await expect(rows).toContainText(/[$%]/);
  await panel.getByTestId('terminal-maximize').click();
  await host.locator('.xterm').click({ position: { x: 35, y: 100 } });
  return { absPath, host, rows };
}

export async function dragTerminalText(page: Page, line: Locator, text: string) {
  const box = await line.evaluate((element, text) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = []; let next: Node | null;
    while ((next = walker.nextNode())) nodes.push(next as Text);
    const contents = nodes.map(node => node.data).join('');
    const start = contents.indexOf(text), end = start + text.length;
    if (start < 0) throw new Error('Expected selectable fixture text');
    const range = document.createRange(); let offset = 0;
    for (const node of nodes) {
      if (start >= offset && start < offset + node.length) range.setStart(node, start - offset);
      if (end > offset && end <= offset + node.length) { range.setEnd(node, end - offset); break; }
      offset += node.length;
    }
    const rect = range.getBoundingClientRect();
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  }, text);
  await page.mouse.move(box.x + 1, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width - 1, box.y + box.height / 2, { steps: 14 });
  await page.mouse.up();
}
