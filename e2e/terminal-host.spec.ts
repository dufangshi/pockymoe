import { test, expect, _electron as electron, type Page } from '@playwright/test';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const web = `http://localhost:${process.env.E2E_WEB_PORT ?? 5173}`;
// The headed macOS run also checks the OS pasteboard, outside the browser process.
async function clipboardText(page: Page, browserName = 'chromium') {
  if (browserName === 'webkit' || process.env.E2E_SYSTEM_CLIPBOARD === '1') return execFileSync('pbpaste', { encoding: 'utf8' });
  return page.evaluate(() => navigator.clipboard.readText());
}

async function terminal(page: Page, request: import('@playwright/test').APIRequestContext) {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  const absPath = await mkdtemp(path.join(process.env.E2E_WORKSPACE_ROOT!, 'host-'));
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'Terminal host' } })).json();
  const response = await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Terminal host', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const value = await response.json();
  await page.goto(`${web}/threads/${value.id ?? value.thread.id}`);
  await page.getByRole('navigation', { name: 'Workspace tools', exact: true }).getByRole('button', { name: 'Terminal', exact: true }).click();
  await page.getByTestId('workbench-bottom-panel').getByRole('button', { name: 'New terminal', exact: true }).first().click();
  const rows = page.locator('[data-testid="workbench-terminal-target"] [data-testid="terminal-pane"]:visible .xterm-rows');
  await expect(rows).toContainText(/[$%]/);
  return rows;
}

test('Electron terminal selection reaches the system clipboard and confirmed links reach openExternal', async () => {
  const desktop = process.env.E2E_DESKTOP_DIR;
  test.skip(!desktop, 'Set E2E_DESKTOP_DIR to the isolated built desktop checkout');
  await mkdir(process.env.E2E_WORKSPACE_ROOT!, { recursive: true });
  const profile = await mkdtemp(path.join(tmpdir(), 'terminal-electron-'));
  const env = Object.fromEntries(['PATH', 'HOME', 'TMPDIR', 'USER', 'LOGNAME'].filter(key => process.env[key]).map(key => [key, process.env[key]!]));
  Object.assign(env, { POCKYMOE_DESKTOP_E2E: '1', POCKYMOE_DESKTOP_USER_DATA: profile, POCKYMOE_DESKTOP_WEB_ORIGIN: web, POCKYMOE_DESKTOP_RELAY_ORIGIN: api });
  const app = await electron.launch({ executablePath: path.join(desktop!, 'node_modules/electron/dist/Electron.app/Contents/MacOS/Electron'), args: [desktop!], env });
  try {
    await expect.poll(() => app.windows().some(page => page.url().startsWith(web))).toBe(true);
    const page = app.windows().find(page => page.url().startsWith(web))!;
    await app.evaluate(({ shell, clipboard }) => {
      clipboard.writeText('clipboard sentinel');
      (globalThis as any).terminalOpened = [];
      shell.openExternal = async url => { (globalThis as any).terminalOpened.push(url); };
    });
    await page.goto(`${web}/e2e/terminal-host.html`);
    const rows = page.locator('.xterm-rows');
    const output = rows.locator(':scope > div').filter({ hasText: /^ClipboardMarkerAlpha\s*$/ });
    await expect(output).toHaveCount(1);
    await output.dblclick({ position: { x: 35, y: 7 } });
    await expect(page.locator('.terminal-feedback')).toHaveText('Copied');
    await expect.poll(() => app.evaluate(({ clipboard }) => clipboard.readText())).toBe('ClipboardMarkerAlpha');
    await page.keyboard.press('Meta+c');
    await expect.poll(() => app.evaluate(({ clipboard }) => clipboard.readText())).toBe('ClipboardMarkerAlpha');
    const link = rows.locator(':scope > div').filter({ hasText: /^TerminalLink\s*$/ });
    await expect(link).toHaveCount(1);
    await link.click({ position: { x: 30, y: 7 } });
    await page.getByRole('dialog').getByRole('button', { name: 'Open link', exact: true }).click();
    await expect.poll(() => app.evaluate(() => (globalThis as any).terminalOpened)).toEqual(['https://example.com/terminal']);
  } finally { await app.close(); await rm(profile, { recursive: true, force: true }); }
});


test('Web terminal selection writes the clipboard', async ({ page, request, context, browserName }) => {
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const rows = await terminal(page, request);
  await page.keyboard.type('printf "WebMarkerAlpha\\n"');
  await page.keyboard.press('Enter');
  const output = rows.locator(':scope > div').filter({ hasText: /^WebMarkerAlpha\s*$/ });
  await expect(output).toHaveCount(1);
  await output.dblclick({ position: { x: 35, y: 7 } });
  await expect(page.locator('.terminal-feedback')).toHaveText('Copied');
  await expect.poll(() => clipboardText(page, browserName)).toBe('WebMarkerAlpha');
});

test('terminal selection in a clipboard-restricted iframe reaches the clipboard', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: web });
  await page.goto(web);
  await page.setContent(`<iframe style="width:100%;height:600px" allow="clipboard-write 'none'" src="${web}/e2e/terminal-host.html"></iframe>`);
  const frame = page.frameLocator('iframe');
  const output = frame.locator('.xterm-rows > div').filter({ hasText: /^ClipboardMarkerAlpha\s*$/ });
  await expect(output).toHaveCount(1);
  await output.dblclick({ position: { x: 35, y: 7 } });
  await expect(frame.locator('.terminal-feedback')).toHaveText('Copied');
  await page.locator('body').click({ position: { x: 2, y: 2 } });
  await expect.poll(() => clipboardText(page)).toBe('ClipboardMarkerAlpha');
});

test('terminal menu copy survives async clipboard denial and selection clearing', async ({ page, context, browserName }) => {
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.addInitScript(() => {
    localStorage.setItem('pockymoe.terminal', JSON.stringify({ copyOnSelect: false }));
    navigator.clipboard.writeText = async () => { throw new DOMException('Denied', 'NotAllowedError'); };
  });
  await page.goto(`${web}/e2e/terminal-host.html`);
  const output = page.locator('.xterm-rows > div').filter({ hasText: /^ClipboardMarkerAlpha\s*$/ });
  await expect(output).toHaveCount(1);
  await output.dblclick({ position: { x: 35, y: 7 } });
  await output.click({ button: 'right', position: { x: 35, y: 7 } });
  await page.getByRole('menuitem', { name: 'Copy', exact: true }).click();
  await expect(page.locator('.terminal-feedback')).toHaveText('Copied');
  await expect.poll(() => clipboardText(page, browserName)).toBe('ClipboardMarkerAlpha');
});


test('terminal link confirmation opens the actual Web destination and cancellation opens nothing', async ({ page, context }) => {
  await context.route('https://example.com/**', route => route.fulfill({ contentType: 'text/html', body: '<title>Terminal destination</title>' }));
  await page.goto(`${web}/e2e/terminal-host.html`);
  const link = page.locator('.xterm-rows > div').filter({ hasText: /^TerminalLink\s*$/ });
  await link.click({ position: { x: 30, y: 7 } });
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('https://example.com/terminal');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(context.pages()).toHaveLength(1);
  // xterm caches the last hovered cell; re-enter through a different cell after the modal closes.
  await page.locator('.xterm-rows > div').filter({ hasText: /^\$\s*$/ }).hover();
  await link.click({ position: { x: 30, y: 7 } });
  const opened = context.waitForEvent('page', { timeout: 10_000 });
  await dialog.getByRole('button', { name: 'Open link', exact: true }).click();
  const popup = await opened;
  await expect(popup).toHaveURL('https://example.com/terminal');
  await expect(popup).toHaveTitle('Terminal destination');
});

test('terminal copy failure shows failure and leaves the clipboard untouched', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(`${web}/e2e/terminal-host.html`);
  await page.evaluate(() => navigator.clipboard.writeText('failure sentinel'));
  await page.evaluate(() => {
    navigator.clipboard.writeText = async () => { throw new DOMException('Denied', 'NotAllowedError'); };
    document.execCommand = () => false;
  });
  const output = page.locator('.xterm-rows > div').filter({ hasText: /^ClipboardMarkerAlpha\s*$/ });
  await output.dblclick({ position: { x: 35, y: 7 } });
  const feedback = page.locator('.terminal-feedback');
  await expect(feedback).toHaveText('Copy failed');
  await expect(feedback).toHaveAttribute('data-tone', 'failed');
  await expect(feedback.locator('svg')).toBeVisible();
  await expect.poll(() => clipboardText(page)).toBe('failure sentinel');
});

test('phone terminal text overlay copies the actual selected text', async ({ page, context, browserName, isMobile }) => {
  test.skip(!isMobile, 'Touch selection path');
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(`${web}/e2e/terminal-host.html`);
  await expect(page.locator('.xterm-rows')).toContainText('ClipboardMarkerAlpha');
  await page.getByRole('button', { name: 'Select text', exact: true }).tap();
  const selection = page.getByRole('dialog', { name: 'Select text', exact: true });
  await selection.getByRole('button', { name: 'Select all', exact: true }).tap();
  const expected = await page.evaluate(() => window.getSelection()!.toString());
  expect(expected).toContain('ClipboardMarkerAlpha');
  await selection.getByRole('button', { name: 'Copy', exact: true }).tap();
  await expect(page.locator('.terminal-feedback')).toHaveText('Copied');
  await expect.poll(() => clipboardText(page, browserName)).toBe(expected);
});

test('terminal feedback and link dialog visual check across themes and widths', async ({ page }) => {
  const evidence = path.resolve('.scratch/terminal-visual');
  await mkdir(evidence, { recursive: true });
  for (const width of [390, 1280]) for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${web}/e2e/terminal-host.html?pack=${pack}&mode=${mode}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
    await page.evaluate(() => {
      navigator.clipboard.writeText = async () => { throw new DOMException('Denied', 'NotAllowedError'); };
      document.execCommand = () => false;
    });
    const output = page.locator('.xterm-rows > div').filter({ hasText: /^ClipboardMarkerAlpha\s*$/ });
    const outputBox = (await output.boundingBox())!;
    await page.mouse.click(outputBox.x + 35, outputBox.y + 7, { clickCount: 2 });
    await expect(page.locator('.terminal-feedback')).toHaveText('Copy failed');
    await page.setViewportSize({ width, height: 800 });
    await page.screenshot({ path: path.join(evidence, `${width}-${pack}-${mode}-feedback.png`) });
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(page.locator('.shell-pane-host')).not.toHaveClass(/mobile-shell-direct/);
    const link = page.locator('.xterm-rows > div').filter({ hasText: /^TerminalLink\s*$/ });
    const linkBox = (await link.boundingBox())!;
    await page.mouse.click(linkBox.x + 30, linkBox.y + 7);
    const dialog = page.getByRole('dialog', { name: 'Open terminal link?' });
    await expect(dialog).toBeVisible();
    await page.setViewportSize({ width, height: 800 });
    await expect(dialog.getByRole('button', { name: 'Open link', exact: true })).toBeInViewport();
    await page.screenshot({ path: path.join(evidence, `${width}-${pack}-${mode}-dialog.png`) });
    await page.keyboard.press('Escape');
  }
});
