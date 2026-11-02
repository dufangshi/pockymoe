import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { isolatedTerminal, shellQuote, dragTerminalText } from './helpers/isolated-terminal';

test.use({ actionTimeout: 15_000 });
test.skip(process.platform !== 'darwin', 'System pasteboard check on this Mac');

for (const framing of ['direct', 'tmux-duplex']) test(`program-owned mouse selection writes UTF-8 with ${framing} framing`, async ({ page, request, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  let output = '';
  page.on('websocket', socket => socket.on('framereceived', event => {
    try { const frame = JSON.parse(event.payload.toString()); if (frame.type === 'shell.output') output += frame.payload?.data ?? ''; } catch { /* Other frames. */ }
  }));
  const { absPath, host, rows } = await isolatedTerminal(page, request, 'Program mouse selection');
  const record = path.join(absPath, 'mouse-selection.json');
  const script = path.resolve('e2e/fixtures/mouse-clipboard.py');
  try {
    await page.keyboard.type(`python3 ${shellQuote(script)} --record ${shellQuote(record)} --framing ${framing}`);
    await page.keyboard.press('Enter');
    const line = rows.locator(':scope > div').filter({ hasText: 'MouseClipboard中文' });
    await expect(line).toHaveCount(1);
    execFileSync('pbcopy', { input: 'mouse selection sentinel' });
    await dragTerminalText(page, line, 'MouseClipboard中文');
    await expect(rows).toContainText('已复制');
    const { readFile } = await import('node:fs/promises');
    const selected = JSON.parse(await readFile(record, 'utf8'));
    expect(selected.release).toBe(true);
    expect(selected.mouseEvents).toBeGreaterThan(2);
    expect(selected.selected).toBe('MouseClipboard中文');
    await expect(host.locator('.xterm-selection > div')).toHaveCount(0);
    await expect.poll(() => execFileSync('pbpaste', { encoding: 'utf8' })).toBe('MouseClipboard中文');
    expect((output.match(/(?<!\x1b)\x1b\]52;c;/g) ?? []).length).toBe(1);
    expect(output.includes('\x1bPtmux;\x1b\x1b]52;c;')).toBe(framing === 'tmux-duplex');
  } finally { await page.keyboard.press('q'); }
});
