import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { isolatedTerminal, shellQuote, dragTerminalText } from './helpers/isolated-terminal';

// External onboarding may show auth URLs in other Claude versions; keep it out of traces.
test.use({ actionTimeout: 15_000, trace: 'off' });
test.skip(process.platform !== 'darwin', 'System pasteboard check on this Mac');

test('real credential-free mouse selection emits OSC 52', async ({ page, request, context }) => {
  test.skip(process.env.E2E_CLAUDE_MOUSE_SELECTION !== '1', 'Opt in to the installed Claude Code onboarding test');
  const workspaceRoot = path.resolve(process.env.E2E_WORKSPACE_ROOT!);
  test.skip(workspaceRoot === process.cwd() || workspaceRoot.startsWith(process.cwd() + path.sep), 'Use a workspace outside the repository to avoid ancestor Claude settings');
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  let claude = '';
  try { claude = execFileSync('/bin/sh', ['-c', 'command -v claude'], { encoding: 'utf8' }).trim(); } catch { /* Optional installed program. */ }
  test.skip(!claude, 'Claude Code is not installed');
  let output = '';
  page.on('websocket', socket => socket.on('framereceived', event => {
    try { const frame = JSON.parse(event.payload.toString()); if (frame.type === 'shell.output') output += frame.payload?.data ?? ''; } catch { /* Other frames. */ }
  }));
  const { absPath, host, rows } = await isolatedTerminal(page, request, 'Claude selection probe');
  const config = path.join(absPath, 'empty-claude-config'); await mkdir(config);
  const input = host.locator('.xterm-helper-textarea');
  const command = ['env', '-i', 'PATH=/usr/bin:/bin:/opt/homebrew/bin', 'TERM=xterm-256color', 'COLORTERM=truecolor', 'LANG=en_US.UTF-8', `CLAUDE_CONFIG_DIR=${shellQuote(config)}`, 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1', 'DISABLE_AUTOUPDATER=1', shellQuote(claude), '--bare', '--setting-sources', "''", '--strict-mcp-config', '--tools', "''", '--no-chrome'].join(' ');
  await page.keyboard.type(command); await input.press('Enter');
  // Theme choice is local onboarding, not account authentication.
  await expect.poll(async () => /Syntax theme|API[_ ]KEY|API key|log.?in|sign in|authenticate/i.test(await rows.innerText())).toBe(true);
  if (/Syntax theme/.test(await rows.innerText())) {
    await input.press('Enter');
    await expect.poll(async () => (await rows.innerText()).includes('Syntax theme')).toBe(false);
  }
  if (/Press Enter to continue/.test(await rows.innerText()) && /responsible for Claude|prompt injection/i.test(await rows.innerText())) {
    await input.press('Enter');
    await expect.poll(async () => /responsible for Claude|prompt injection/i.test(await rows.innerText())).toBe(false);
  }
  if (/Quick safety check/.test(await rows.innerText())) {
    const trust = await rows.innerText();
    expect(trust).not.toContain('pre-approves');
    await expect.poll(async () => {
      const choice = await rows.innerText();
      if (/❯[^\n]*Yes, I trust this folder/.test(choice)) return true;
      if (/❯[^\n]*No, exit/.test(choice)) await input.press('ArrowDown');
      return false;
    }).toBe(true);
    const before = output.length;
    await input.press('Enter');
    await expect.poll(() => output.length).toBeGreaterThan(before);
    await expect.poll(async () => /Not logged in|\/login|API[_ ]KEY|API key|subscription|for shortcuts|terminal setup|auto mode on/i.test(await rows.innerText())).toBe(true);
  }
  const screen = await rows.innerText();
  const summary = {
    version: execFileSync(claude, ['--version'], { encoding: 'utf8', env: { PATH: '/usr/bin:/bin:/opt/homebrew/bin', CLAUDE_CONFIG_DIR: config } }).trim(),
    authRequired: /not logged in|log.?in|API[_ ]KEY|API key|authenticate|sign in|subscription|Console account|Bedrock|Vertex|Foundry/i.test(screen),
    themeOnboarding: /choose.*theme|select.*theme|text style|Syntax theme/i.test(screen),
    mouseModeSeen: /\x1b\[\?(?:1000|1002|1003)(?:;[0-9]+)*h/.test(output),
    directOsc52Frames: (output.match(/\x1b\]52;/g) ?? []).length,
    tmuxDcsFrames: (output.match(/\x1bPtmux;/g) ?? []).length,
    terminator: 'none',
    browserWriteVerified: false,
  };
  if (summary.authRequired || !summary.mouseModeSeen) {
    await mkdir('.scratch', { recursive: true });
    await writeFile('.scratch/claude-selection-probe.json', JSON.stringify(summary, null, 2));
    await input.press('Control+c'); await input.press('Control+c');
    test.skip(true, 'Credential-free Claude did not reach its mouse-tracking UI; use the deterministic TUI cases');
  }
  expect(summary.mouseModeSeen).toBe(true);
  const selectionText = 'Claude Code';
  const selectedLine = rows.locator(':scope > div').filter({ hasText: /Auto mode is now Claude Code/ });
  await expect(selectedLine).toHaveCount(1);
  await page.evaluate(() => {
    const write = navigator.clipboard.writeText.bind(navigator.clipboard);
    (window as any).successfulClipboardWrites = [];
    navigator.clipboard.writeText = async text => { await write(text); (window as any).successfulClipboardWrites.push(text); };
  });
  const beforeSelection = output.length;
  execFileSync('pbcopy', { input: 'Claude selection sentinel' });
  await dragTerminalText(page, selectedLine, selectionText);
  await expect.poll(() => execFileSync('pbpaste', { encoding: 'utf8' })).toBe(selectionText);
  await expect(host.locator('.xterm-selection > div')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).successfulClipboardWrites)).toContain(selectionText);
  summary.browserWriteVerified = true;
  const selectionOutput = output.slice(beforeSelection);
  const frame = selectionOutput.match(/(?<!\x1b)\x1b\]52;([^;]*);([A-Za-z0-9+/=]+)(\x07|\x1b\\)/);
  expect(frame).not.toBeNull();
  expect(Buffer.from(frame![2]!, 'base64').toString('utf8')).toBe(selectionText);
  summary.terminator = frame![3] === '\x07' ? 'BEL' : 'ST';
  summary.directOsc52Frames = (selectionOutput.match(/(?<!\x1b)\x1b\]52;/g) ?? []).length;
  summary.tmuxDcsFrames = (selectionOutput.match(/\x1bPtmux;/g) ?? []).length;
  expect(summary.directOsc52Frames).toBeGreaterThan(0);
  await mkdir('.scratch', { recursive: true });
  await writeFile('.scratch/claude-selection-probe.json', JSON.stringify(summary, null, 2));
  console.log('Claude mouse selection:', summary);
  await input.press('Control+c'); await input.press('Control+c');
});
