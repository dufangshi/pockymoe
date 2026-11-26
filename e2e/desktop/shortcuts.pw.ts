import { expect, test, type Page } from '@playwright/test';
import { seed } from './fixtures';

// docs/ui-principles.md §19: Mod is ⌘ on a Mac and Ctrl elsewhere. On a Mac,
// Ctrl+F and Ctrl+K are text-editing and terminal keys and must reach the field.
type Platform = 'mac' | 'windows';
const AGENTS: Record<Platform, { userAgent: string; platform: string; hinted: string }> = {
  mac: { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36', platform: 'MacIntel', hinted: 'macOS' },
  windows: { userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36', platform: 'Win32', hinted: 'Windows' },
};
/** The browser reports this platform in the user agent, `navigator.platform` and the UA client hints. */
async function emulate(page: Page, platform: Platform) {
  const agent = AGENTS[platform];
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setUserAgentOverride', {
    userAgent: agent.userAgent, platform: agent.platform,
    userAgentMetadata: { platform: agent.hinted, platformVersion: '15.0.0', architecture: 'x86', model: '', mobile: false, brands: [{ brand: 'Chromium', version: '140' }] },
  });
}
async function openThread(page: Page, options: { host: boolean; hostPlatform?: 'win32' }) {
  await page.addInitScript(({ hostPlatform }) => {
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
    if (hostPlatform) { const url = new URL(location.href); url.searchParams.set('desktopMockPlatform', hostPlatform); history.replaceState(null, '', url); }
    // Whether the page kept the key from its default action, read after every listener ran.
    addEventListener('keydown', event => { if (event.key !== 'Control' && event.key !== 'Meta') setTimeout(() => { (window as unknown as { lastChord?: boolean }).lastChord = event.defaultPrevented; }, 0); });
  }, { hostPlatform: options.hostPlatform ?? null });
  await page.route(/\/(relay|api)\//, route => {
    const path = new URL(route.request().url()).pathname;
    return /\.(ts|tsx|js|css)$/.test(path) || path.includes('/src/') || path.includes('node_modules') ? route.fallback() : route.fulfill({ status: 404, json: { code: 'not_found', message: 'not found' } });
  });
  await seed(page, 'classic', false, false, false, options.host);
  // Every thread the workbench lists, and a resume, answers with its workspace, as a Supervisor does.
  await page.route(/\/relay\/devices\/device-\d+\/api\/threads\/t-\d+(\/resume)?(\?.*)?$/, async route => {
    const now = new Date().toISOString(), id = new URL(route.request().url()).pathname.match(/\/threads\/(t-\d+)/)![1]!;
    await route.fulfill({ json: { workspace: { id: 'w-0', hostId: 'host', absPath: '/fixture', label: 'Pockymoe', isFavorite: true, createdAt: now, lastOpenedAt: null }, thread: { id, workspaceId: 'w-0', title: `Shortcuts ${id}`, provider: 'acp', agentId: 'codex', providerSessionId: null, status: 'idle', isPinned: false, createdAt: now, updatedAt: now }, turns: [{ id: 'h', status: 'completed', startedAt: now, completedAt: now, items: [{ id: 'q', kind: 'userMessage', text: 'hi' }, { id: 'r', kind: 'agentMessage', responsePhase: 'commentary', text: 'reply text' }] }], totalTurnCount: 1, pendingRequests: [], pendingSteers: [] } });
  });
  await page.route('**/relay/access?*', route => route.fulfill({ json: { kind: 'owner', shareId: null, scope: 'owner', threadAccess: 'control', workspaceAccess: 'write', workspaceId: null } }));
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: { start: true }, sessions: {}, branching: {}, management: {}, usage: {} } } }));
  await page.route('**/api/agent-runtimes/**', route => route.fulfill({ status: 404, json: { code: 'not_found' } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
  await page.evaluate(() => { history.pushState({}, '', '/devices/device-0/threads/t-0'); dispatchEvent(new PopStateEvent('popstate')); });
  await expect(prompt(page)).toBeVisible();
  await expect(page.getByText('reply text')).toBeVisible();
  await expect(prompt(page)).toHaveAttribute('contenteditable', 'plaintext-only');
}
const prompt = (page: Page) => page.getByRole('textbox', { name: '提示词' }).first();
const searchField = (page: Page) => page.getByRole('combobox', { name: '搜索消息' });
const searchButton = (page: Page) => page.locator('.matter-tabs-search .matter-search-trigger:visible').first();
/** Presses a chord with the caret in the prompt; answers whether the page took the key. */
async function pressInPrompt(page: Page, chord: string) {
  await prompt(page).click();
  await expect(prompt(page)).toBeFocused();
  await page.evaluate(() => { delete (window as unknown as { lastChord?: boolean }).lastChord; });
  await page.keyboard.press(chord);
  await expect.poll(() => page.evaluate(() => (window as unknown as { lastChord?: boolean }).lastChord)).not.toBeUndefined();
  return page.evaluate(() => (window as unknown as { lastChord?: boolean }).lastChord);
}
/** Puts the focus in a field inside a stand-in terminal or code editor. */
async function focusInside(page: Page, className: string) {
  await page.evaluate(className => {
    document.getElementById('shortcut-probe')?.remove();
    const holder = document.createElement('div');
    holder.id = 'shortcut-probe'; holder.className = className;
    holder.style.cssText = 'position:fixed;left:480px;top:320px;z-index:99999;width:200px;height:60px';
    const field = document.createElement('textarea'); field.setAttribute('aria-label', 'probe');
    holder.append(field); document.body.append(holder); field.focus();
    delete (window as unknown as { lastChord?: boolean }).lastChord;
  }, className);
}
const closeSearch = async (page: Page) => { await page.keyboard.press('Escape'); await expect(searchField(page)).toHaveCount(0); };

test('Mac browser: ⌘F opens the conversation search and Ctrl+F stays with the prompt', async ({ page }) => {
  await emulate(page, 'mac');
  await openThread(page, { host: false });
  expect(await page.evaluate(() => [navigator.platform, (navigator as Navigator & { userAgentData?: { platform: string } }).userAgentData?.platform])).toEqual(['MacIntel', 'macOS']);
  await expect(searchButton(page)).toHaveAttribute('title', '搜索线程 (⌘F)');
  await expect(page.locator('.thread-composer-placeholder-hint').first()).toContainText('⌘Enter');

  expect(await pressInPrompt(page, 'Control+f')).toBe(false);
  await expect(prompt(page)).toBeFocused();
  await expect(searchField(page)).toHaveCount(0);

  expect(await pressInPrompt(page, 'Meta+f')).toBe(true);
  await expect(searchField(page)).toBeFocused();
  await closeSearch(page);

  // The web page has no ⌘K or Ctrl+K of its own.
  expect(await pressInPrompt(page, 'Control+k')).toBe(false);
  expect(await pressInPrompt(page, 'Meta+k')).toBe(false);
  await expect(searchField(page)).toHaveCount(0);
});

test('Windows browser: Ctrl+F opens the conversation search and the Windows key does not', async ({ page }) => {
  await emulate(page, 'windows');
  await openThread(page, { host: false });
  await expect(searchButton(page)).toHaveAttribute('title', '搜索线程 (Ctrl+F)');
  await expect(page.locator('.thread-composer-placeholder-hint').first()).toContainText('Ctrl+Enter');

  expect(await pressInPrompt(page, 'Meta+f')).toBe(false);
  await expect(prompt(page)).toBeFocused();
  await expect(searchField(page)).toHaveCount(0);

  expect(await pressInPrompt(page, 'Control+f')).toBe(true);
  await expect(searchField(page)).toBeFocused();
  await closeSearch(page);

  // A terminal and a code editor keep their own find.
  for (const className of ['xterm', 'monaco-editor']) {
    await focusInside(page, className);
    await page.keyboard.press('Control+f');
    await expect.poll(() => page.evaluate(() => (window as unknown as { lastChord?: boolean }).lastChord)).toBe(false);
    await expect(page.getByRole('textbox', { name: 'probe' })).toBeFocused();
    await expect(searchField(page)).toHaveCount(0);
  }
});

test('Mac desktop client: ⌘K and ⌘F reach the search, Ctrl+K and Ctrl+F stay with the prompt, and a terminal keeps ⌘K', async ({ page }) => {
  // The mock host reports darwin while the browser itself is the test machine's: the host decides.
  await openThread(page, { host: true });
  await expect(searchButton(page)).toHaveAttribute('title', '搜索线程 (⌘F)');

  for (const chord of ['Control+k', 'Control+f']) {
    expect(await pressInPrompt(page, chord)).toBe(false);
    await expect(prompt(page)).toBeFocused();
    await expect(searchField(page)).toHaveCount(0);
  }
  for (const chord of ['Meta+k', 'Meta+f']) {
    expect(await pressInPrompt(page, chord)).toBe(true);
    await expect(searchField(page)).toBeFocused();
    await closeSearch(page);
  }
  for (const className of ['xterm', 'monaco-editor']) for (const chord of ['Meta+k', 'Control+k']) {
    await focusInside(page, className);
    await page.keyboard.press(chord);
    await expect.poll(() => page.evaluate(() => (window as unknown as { lastChord?: boolean }).lastChord)).toBe(false);
    await expect(page.getByRole('textbox', { name: 'probe' })).toBeFocused();
    await expect(searchField(page)).toHaveCount(0);
  }
  // ⌘0 opens the map; Ctrl+0 does not.
  await page.evaluate(() => document.getElementById('shortcut-probe')?.remove());
  expect(await pressInPrompt(page, 'Control+0')).toBe(false);
  expect(new URL(page.url()).pathname).toBe('/devices/device-0/threads/t-0');
  expect(await pressInPrompt(page, 'Meta+0')).toBe(true);
  await expect.poll(() => new URL(page.url()).pathname).toBe('/');
});

test('Windows desktop client: Ctrl+K reaches the search, and a terminal keeps it', async ({ page }) => {
  await emulate(page, 'windows');
  await openThread(page, { host: true, hostPlatform: 'win32' });
  await expect(searchButton(page)).toHaveAttribute('title', '搜索线程 (Ctrl+F)');

  expect(await pressInPrompt(page, 'Meta+k')).toBe(false);
  await expect(searchField(page)).toHaveCount(0);
  expect(await pressInPrompt(page, 'Control+k')).toBe(true);
  await expect(searchField(page)).toBeFocused();
  await closeSearch(page);

  for (const className of ['xterm', 'monaco-editor']) {
    await focusInside(page, className);
    await page.keyboard.press('Control+k');
    await expect.poll(() => page.evaluate(() => (window as unknown as { lastChord?: boolean }).lastChord)).toBe(false);
    await expect(page.getByRole('textbox', { name: 'probe' })).toBeFocused();
    await expect(searchField(page)).toHaveCount(0);
  }
});
