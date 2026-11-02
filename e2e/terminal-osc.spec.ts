import { test, expect, _electron as electron, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';

import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
const web = `http://localhost:${process.env.E2E_WEB_PORT ?? 5173}`;
test.use({ actionTimeout: 15_000 });
test.skip(process.platform !== 'darwin', 'This acceptance suite reads the macOS system pasteboard');

const sequence = (text: string, target = 'c') => `\x1b]52;${target};${Buffer.from(text).toString('base64')}\x07`;
async function exerciseOsc52(page: Page, read: () => Promise<string> | string, needsGesture: boolean) {
  execFileSync('pbcopy', { input: 'OSC sentinel' });
  await page.goto(`${web}/e2e/terminal-host.html?settings&replayClipboard`);
  const rows = page.locator('.xterm-rows');
  await expect(rows).toContainText('ClipboardMarkerAlpha');
  await page.locator('.xterm').click({ position: { x: 35, y: 120 } });
  expect(await read()).toBe('OSC sentinel');
  const previousAttachment = await page.evaluate(() => window.terminalFixture.attachment);
  await page.evaluate(() => window.terminalFixture.reattach());
  await expect.poll(() => page.evaluate(() => window.terminalFixture.attachment)).toBeGreaterThan(previousAttachment);
  await expect(rows).toContainText('ClipboardMarkerAlpha');
  await page.locator('.xterm').click({ position: { x: 35, y: 120 } });
  expect(await read()).toBe('OSC sentinel');
  let step = 0;
  const output = async (data: string, replay = false, replace = false) => {
    const marker = `OSC parsed ${++step}`;
    await page.evaluate(({ data, marker, replay, replace }) => window.terminalFixture.output(data + `\r\n${marker}\r\n`, replay, replace), { data, marker, replay, replace });
    await expect(rows).toContainText(marker);
  };
  const gesture = async () => { if (needsGesture) await page.locator('.xterm').click({ position: { x: 35, y: 120 } }); };
  const program = execFileSync('/bin/sh', ['-c', `printf '\\033[?1000h已复制\\r\\n'; printf '\\033]52;c;%s\\a' "$(printf hello | base64)"`], { encoding: 'utf8' });
  await output(program); await gesture();
  await expect.poll(read).toBe('hello');
  await output(sequence('中文😀', 'p')); await gesture();
  await expect.poll(read).toBe('中文😀');
  await page.evaluate(() => { (window as any).queryReads = 0; navigator.clipboard.readText = async () => { (window as any).queryReads++; throw new Error('Clipboard read forbidden'); }; });
  const before = await page.evaluate(() => window.terminalFixture.inputs.filter(message => (message as { type: string }).type === 'shell.input'));
  await output('\x1b]52;c;?\x07\x1b]52;p;?\x07\x1b]52;;?\x07\x1b]52;c;\x07\x1b]52;c;%%%%\x07');
  expect(await read()).toBe('中文😀');
  expect(await page.evaluate(() => (window as any).queryReads)).toBe(0);
  expect(await page.evaluate(() => window.terminalFixture.inputs.filter(message => (message as { type: string }).type === 'shell.input'))).toEqual(before);
  await output(sequence('x'.repeat(1024 * 1024 + 1)));
  expect(await read()).toBe('中文😀');
  await output(sequence('old replay'), true);
  await output(sequence('old snapshot'), true, true);
  await page.locator('.xterm').click({ position: { x: 35, y: 120 } });
  expect(await read()).toBe('中文😀');
  const toggle = page.getByRole('checkbox', { name: /^Allow terminal programs to write to the clipboard/ });
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('pockymoe.terminal')!).allowProgramClipboard)).toBe(false);
  await output(sequence('disabled')); await gesture();
  expect(await read()).toBe('中文😀');
  await page.reload();
  await expect(toggle).not.toBeChecked();
  await expect(rows).toContainText('ClipboardMarkerAlpha');
  await output(sequence('still disabled')); await gesture();
  expect(await read()).toBe('中文😀');
  await toggle.check();
  await output(sequence('default target', '')); await gesture();
  await expect.poll(read).toBe('default target');
}

test('OSC 52 program writes, UTF-8 and clipboard boundaries', async ({ page, context, browserName }) => {
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await exerciseOsc52(page, () => execFileSync('pbpaste', { encoding: 'utf8' }), browserName === 'webkit');
});

test('Electron OSC 52 writes and clipboard boundaries', async () => {
  const desktop = process.env.E2E_DESKTOP_DIR;
  test.skip(!desktop, 'Set E2E_DESKTOP_DIR to the isolated built desktop checkout');
  const profile = await mkdtemp(path.join(tmpdir(), 'terminal-osc-electron-'));
  const env = Object.fromEntries(['PATH', 'HOME', 'TMPDIR', 'USER', 'LOGNAME'].filter(key => process.env[key]).map(key => [key, process.env[key]!]));
  Object.assign(env, { POCKYMOE_DESKTOP_E2E: '1', POCKYMOE_DESKTOP_USER_DATA: profile, POCKYMOE_DESKTOP_WEB_ORIGIN: web, POCKYMOE_DESKTOP_RELAY_ORIGIN: `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}` });
  const app = await electron.launch({ executablePath: path.join(desktop!, 'node_modules/electron/dist/Electron.app/Contents/MacOS/Electron'), args: [desktop!], env });
  try {
    await expect.poll(() => app.windows().some(page => page.url().startsWith(web))).toBe(true);
    const page = app.windows().find(page => page.url().startsWith(web))!;
    await exerciseOsc52(page, () => app.evaluate(({ clipboard }) => clipboard.readText()), false);
  } finally { await app.close(); await rm(profile, { recursive: true, force: true }); }
});

test('real tmux mouse mode sends program clipboard writes', async ({ page, request }) => {
  const { mkdtemp, writeFile } = await import('node:fs/promises');
  const path = await import('node:path');
  const { randomUUID } = await import('node:crypto');
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const workspacePath = await mkdtemp(path.join(process.env.E2E_WORKSPACE_ROOT!, 'tmux-'));
  const config = path.join(workspacePath, 'tmux.conf');
  await writeFile(config, "set -g mouse on\nset -g set-clipboard on\nset -as terminal-features ',xterm*:clipboard'\n");
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath: workspacePath, label: 'OSC tmux' } })).json();
  const thread = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'OSC tmux', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  const socket = `pockymoe-osc-${randomUUID()}`;
  try {
    await page.goto(`${web}/threads/${thread.id ?? thread.thread.id}`);
    await page.getByRole('navigation', { name: 'Workspace tools', exact: true }).getByRole('button', { name: 'Terminal', exact: true }).click();
    await page.getByTestId('workbench-bottom-panel').getByRole('button', { name: 'New terminal', exact: true }).first().click();
    const rows = page.locator('[data-testid="workbench-terminal-target"] .shell-pane-active .xterm-rows:visible');
    await expect(rows).toContainText(/[$%]/);
    await page.keyboard.type(`tmux -L ${socket} -f ${config} new-session -s copy /bin/sh`);
    await page.keyboard.press('Enter');
    await expect.poll(() => { try { execFileSync('tmux', ['-L', socket, 'has-session', '-t', 'copy'], { stdio: 'ignore' }); return true; } catch { return false; } }).toBe(true);
    expect(execFileSync('tmux', ['-L', socket, 'show-options', '-gv', 'mouse'], { encoding: 'utf8' }).trim()).toBe('on');
    execFileSync('pbcopy', { input: 'tmux sentinel' });
    execFileSync('tmux', ['-L', socket, 'load-buffer', '-w', '-'], { input: 'tmux中文' });
    execFileSync('tmux', ['-L', socket, 'display-message', '-d', '30000', '已复制']);
    await expect(rows).toContainText('已复制');
    await expect.poll(() => execFileSync('pbpaste', { encoding: 'utf8' }), { timeout: 3000 }).toBe('tmux中文');
  } finally { try { execFileSync('tmux', ['-L', socket, 'kill-server'], { stdio: 'ignore' }); } catch { /* Fixture may not have started. */ } }
});

test('OSC 52 retries the latest denied write on terminal pointer and key gestures', async ({ page, context, browserName }) => {
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto(`${web}/e2e/terminal-host.html?settings`);
  await expect(page.locator('.xterm-rows')).toContainText('ClipboardMarkerAlpha');
  execFileSync('pbcopy', { input: 'retry sentinel' });
  await page.evaluate(() => {
    const write = navigator.clipboard.writeText.bind(navigator.clipboard), command = document.execCommand.bind(document);
    (window as any).clipboardAttempts = 0;
    (window as any).restoreClipboard = () => { navigator.clipboard.writeText = write; document.execCommand = command; };
    navigator.clipboard.writeText = async () => { (window as any).clipboardAttempts++; throw new DOMException('Denied', 'NotAllowedError'); };
    document.execCommand = () => false;
  });
  await page.evaluate(data => window.terminalFixture.output(data), sequence('old'));
  await expect.poll(() => page.evaluate(() => (window as any).clipboardAttempts)).toBe(1);
  await page.evaluate(data => window.terminalFixture.output(data), sequence('latest 中文'));
  await expect.poll(() => page.evaluate(() => (window as any).clipboardAttempts)).toBe(2);
  expect(execFileSync('pbpaste', { encoding: 'utf8' })).toBe('retry sentinel');
  await expect(page.locator('.terminal-feedback')).toHaveText('');
  await page.locator('.xterm').click({ position: { x: 35, y: 120 } });
  await expect(page.locator('.terminal-feedback')).toHaveText('Copy failed');
  expect(execFileSync('pbpaste', { encoding: 'utf8' })).toBe('retry sentinel');
  await page.evaluate(() => (window as any).restoreClipboard());
  await page.keyboard.press('ArrowRight');
  await expect.poll(() => execFileSync('pbpaste', { encoding: 'utf8' })).toBe('latest 中文');
  await expect(page.locator('.terminal-feedback')).toHaveText('Copied');
  await page.evaluate(() => {
    navigator.clipboard.writeText = async () => { (window as any).clipboardAttempts++; throw new DOMException('Denied', 'NotAllowedError'); };
    document.execCommand = () => false;
  });
  await page.evaluate(data => window.terminalFixture.output(data), sequence('must be discarded'));
  await expect.poll(() => page.evaluate(() => (window as any).clipboardAttempts)).toBe(4);
  const toggle = page.getByRole('checkbox', { name: /^Allow terminal programs to write to the clipboard/ });
  await toggle.uncheck(); await toggle.check();
  await page.evaluate(() => (window as any).restoreClipboard());
  await page.locator('.xterm').click({ position: { x: 35, y: 120 } });
  await page.keyboard.press('ArrowRight');
  expect(execFileSync('pbpaste', { encoding: 'utf8' })).toBe('latest 中文');
});
