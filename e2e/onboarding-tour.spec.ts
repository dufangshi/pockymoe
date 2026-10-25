import { test, expect, type Locator, type Page } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

// The tour runs on the in-page sample device (docs/guided-tour.md): no seeding,
// and the Supervisor's own workspaces and threads are never touched.
test.use({ actionTimeout: 15_000 });
const screenshots = process.env.TOUR_SCREENSHOT_DIR;
const MAIN = 'c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e01';
const HELPER = 'c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e02';
const SAMPLE = '/devices/pockymoe-tour';

async function snapshot(page: Page, name: string) {
  if (!screenshots) return;
  await mkdir(path.dirname(path.join(screenshots, name)), { recursive: true });
  await page.screenshot({ path: path.join(screenshots, name), scale: 'css', animations: 'disabled' });
}

function shots(prefix: string) {
  let index = 0;
  return (name: string) => `${prefix}/${String(++index).padStart(2, '0')}-${name}.png`;
}

const tourCard = (page: Page) => page.locator('#pockymoe-tour-root .pm-tour-card');

async function expectInViewport(page: Page, locator: Locator) {
  const box = (await locator.boundingBox())!;
  const viewport = page.viewportSize()!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(viewport.width + 1);
  expect(box.y + box.height).toBeLessThanOrEqual(viewport.height + 1);
}

/** Asserts the card for `id` is on screen, rings `target` (when given) and saves a screenshot. */
async function showStep(page: Page, id: string, name: string, target?: Locator, options: { prerequisite?: boolean } = {}) {
  const card = page.locator(`#pockymoe-tour-root .pm-tour-card[data-step="${id}"]`);
  await expect(card).toBeVisible();
  await expect(card).toHaveAttribute('data-placement', /.+/);
  await expectInViewport(page, card);
  const spotlight = page.locator('#pockymoe-tour-root .pm-tour-spotlight');
  if (options.prerequisite) await expect(card.locator('.pm-tour-prerequisite')).toBeVisible();
  else await expect(card.locator('.pm-tour-prerequisite')).toHaveCount(0);
  if (target) {
    await expect(spotlight).toBeVisible();
    await expect
      .poll(async () => {
        const ring = await spotlight.boundingBox();
        const box = await target.boundingBox();
        return Boolean(ring && box && Math.abs(ring.x + 6 - box.x) < 2 && Math.abs(ring.y + 6 - box.y) < 2 && Math.abs(ring.width - 12 - box.width) < 2);
      })
      .toBe(true);
  }
  await page.waitForTimeout(200); // let the 160 ms card/spotlight transition settle for the picture
  await snapshot(page, name);
  return card;
}

async function next(page: Page) {
  await tourCard(page).getByRole('button', { name: /^(下一步|完成)$/ }).click();
}

async function openChapter(page: Page, title: string) {
  const hub = page.locator('#pockymoe-tour-root .pm-tour-hub');
  await expect(hub).toBeVisible();
  await hub.getByRole('button', { name: new RegExp(title) }).click();
}

async function contents(page: Page) {
  await tourCard(page).getByRole('button', { name: '目录' }).click();
}

function prepare(page: Page) {
  return page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'dark');
  });
}

/** Every request a sample page made that reached a server: only the account's own session and preferences may. */
function watchRequests(page: Page) {
  const seen: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (/^\/(api|relay|ws)(\/|$)/.test(url.pathname) && page.url().includes(SAMPLE)) seen.push(`${request.method()} ${url.pathname}`);
  });
  return () => seen.filter((entry) => !/\/auth\/session$|^GET \/relay\/account\/preferences$/.test(entry));
}

test('desktop guided tour walks every chapter on the sample device', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-chromium', 'Desktop chapters');
  test.setTimeout(300_000);
  await prepare(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  const shot = shots('desktop');
  const reachedServer = watchRequests(page);

  // First run, from the reader's own (possibly empty) workspace list.
  await page.goto('/workspaces');
  const welcome = page.locator('#pockymoe-tour-root .pm-tour-welcome');
  await expect(welcome).toBeVisible();
  await expectInViewport(page, welcome);
  await snapshot(page, shot('welcome'));
  await welcome.getByRole('button', { name: '开始教程' }).click();
  await expect(page.locator('#pockymoe-tour-root .pm-tour-hub')).toBeVisible();
  await snapshot(page, shot('hub'));

  // 1. Devices and workspaces, on the sample device.
  await openChapter(page, '设备与工作区');
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/workspaces$`));
  await showStep(page, 'sample-device', shot('devices-sample'), page.locator('header.product-topbar'));
  await expect(page.locator('header.product-topbar')).toContainText('样例设备');
  await next(page);
  await showStep(page, 'local-device', shot('devices-local'), page.locator('header.product-topbar'));
  await next(page);
  await showStep(page, 'workspace-list', shot('devices-list'), page.locator('article.product-row').first());
  await next(page);
  const addWorkspace = page.getByRole('link', { name: '添加工作区' });
  await showStep(page, 'add-workspace', shot('devices-add'), addWorkspace);
  await addWorkspace.click(); // the real link; the tour follows
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/workspaces/new$`));
  await showStep(page, 'workspace-source', shot('devices-source'), page.locator('[aria-label="工作区来源"]'));
  await next(page);
  await showStep(page, 'workspace-form', shot('devices-submit'), page.locator('form:has(#workspace-target) button[type="submit"]'));
  await next(page);

  // 2. Threads.
  await openChapter(page, '对话');
  const importLink = page.getByRole('link', { name: '导入会话' });
  await showStep(page, 'import', shot('threads-import'), importLink);
  await importLink.click();
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/import$`));
  await showStep(page, 'import-form', shot('threads-import-form'), page.locator('form:has(#backend-provider)'));
  await next(page);
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/${MAIN}$`));
  await showStep(page, 'history', shot('threads-history'), page.locator('aside.matter-sidebar'));
  await next(page);
  await showStep(page, 'new-thread', shot('threads-new'), page.locator('button.matter-new-thread:visible').first());
  await next(page);
  const pane = page.getByTestId('primary-pane');
  await showStep(page, 'work', shot('threads-work'), pane.locator('.thread-worked-toggle').first());
  await next(page);
  const prompt = pane.getByRole('textbox', { name: '提示词' });
  await showStep(page, 'prompt', shot('threads-prompt'), prompt);
  await prompt.click();
  await page.keyboard.type('你好，样例 Agent');
  await next(page);
  await showStep(page, 'model', shot('threads-model'), pane.getByTestId('composer-model-label'));
  await next(page);
  const send = pane.getByRole('button', { name: '发送提示词' });
  await showStep(page, 'send', shot('threads-send'), send);
  await send.click(); // the tour follows the real click
  const stop = pane.locator('button.thread-graph-composer-stop-button');
  await showStep(page, 'stop', shot('threads-stop-running'), stop);
  await expect(pane).toContainText('收到，我先看一下工作区。');
  await stop.click();
  await expect(stop).toHaveCount(0);
  await next(page);
  // Selecting part of a reply offers Quote; the ring moves to it.
  const reply = pane.locator('[data-testid="chat-message"][data-role="assistant"]').filter({ hasText: '完成了' }).locator('p').first();
  await reply.scrollIntoViewIfNeeded();
  await reply.selectText();
  const pill = page.locator('button.quote-selection-pill');
  await expect(pill).toBeVisible();
  await showStep(page, 'quote', shot('threads-quote'), pill);
  await next(page);
  await showStep(page, 'search', shot('threads-search'), page.locator('button.matter-search-trigger:visible').first());
  await next(page);

  // 3. Terminal: the sample shell answers.
  await openChapter(page, '终端');
  const rail = page.getByRole('navigation', { name: '工作区工具' });
  const terminalButton = rail.getByRole('button', { name: '终端', exact: true });
  await showStep(page, 'open', shot('terminal-open'), terminalButton);
  await terminalButton.click();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(panel).toBeVisible();
  const xterm = panel.locator('[data-testid="terminal-panel"] .xterm').first();
  await showStep(page, 'type', shot('terminal-type'), xterm);
  await xterm.click();
  await page.keyboard.type('cat README.md');
  await page.keyboard.press('Enter');
  await expect(panel.locator('.xterm-rows').first()).toContainText('A small sample project for the Pockymoe tour.');
  await next(page);
  await showStep(page, 'resize', shot('terminal-resize'), page.getByTestId('workbench-panel-sash'));
  await next(page);
  await showStep(page, 'new', shot('terminal-new'), panel.getByTestId('terminal-new'));
  await panel.getByTestId('terminal-new').click();
  await next(page);
  await showStep(page, 'tabs', shot('terminal-tabs'), panel.getByTestId('terminal-tabs'));
  await next(page);
  await showStep(page, 'split', shot('terminal-split'), panel.getByTestId('terminal-split'));
  await next(page);
  await showStep(page, 'target', shot('terminal-target'), panel.getByTestId('terminal-target'));
  await expect(panel.getByTestId('terminal-target')).toContainText('样例设备');
  await next(page);
  await showStep(page, 'hide', shot('terminal-hide'), panel.getByTestId('workbench-close-tools'));
  await panel.getByTestId('workbench-close-tools').click();
  await expect(panel).toBeHidden();
  await next(page);

  // 4. Split view with the sample docs thread.
  await openChapter(page, '双会话分屏');
  // It lives in the thread tools menu: the card points at the menu first.
  const toolsToggle = page.locator('.matter-tabs-row button.matter-toolbar-toggle');
  await showStep(page, 'trigger', shot('split-tools'), toolsToggle, { prerequisite: true });
  await toolsToggle.click();
  const splitTrigger = page.getByTestId('workbench-split-trigger');
  await showStep(page, 'trigger', shot('split-trigger'), splitTrigger);
  await splitTrigger.click();
  const picker = page.getByTestId('workbench-thread-picker');
  await showStep(page, 'picker', shot('split-picker'), picker);
  await picker.locator('button.workbench-thread-picker-row', { hasText: 'docs-site' }).click();
  await picker.locator('button.workbench-thread-picker-row', { hasText: '更新文档首页' }).click();
  const reference = page.getByTestId('reference-pane');
  await showStep(page, 'picker', shot('split-picked'), reference);
  await next(page);
  await showStep(page, 'panes', shot('split-panes'), reference);
  await next(page);
  await showStep(page, 'focus', shot('split-focus'), page.getByTestId('make-primary'));
  await next(page);
  await showStep(page, 'tabs', shot('split-tabs'), page.locator('.matter-tabs-row .matter-thread-group').first());
  await next(page);

  // 5. Files: edit and save in the sample workspace.
  await openChapter(page, '文件浏览器');
  const explorerButton = rail.getByRole('button', { name: '切换文件浏览器' });
  await showStep(page, 'open', shot('files-open'), explorerButton);
  await explorerButton.click();
  const drawer = page.locator('aside.workbench-tool-drawer');
  const tree = drawer.getByRole('tree');
  await showStep(page, 'tree', shot('files-tree'), tree);
  await tree.getByRole('button', { name: '展开 docs' }).click();
  await expect(tree.getByRole('treeitem', { name: 'notes.md' })).toBeVisible();
  await next(page);
  await showStep(page, 'filter', shot('files-filter'), drawer.getByRole('button', { name: '筛选工作区' }));
  await next(page);
  await showStep(page, 'new-file', shot('files-new'), drawer.getByRole('button', { name: '新建文件' }));
  await next(page);
  await showStep(page, 'edit', shot('files-edit-first'), undefined, { prerequisite: true });
  await tree.getByRole('treeitem', { name: 'package.json' }).click();
  const more = drawer.locator('button.workspace-file-more');
  const edit = drawer.getByRole('button', { name: '编辑文件' });
  if (!(await edit.isVisible())) await more.click();
  await showStep(page, 'edit', shot('files-edit'), edit);
  await edit.click();
  await showStep(page, 'edit', shot('files-editing'), drawer.getByRole('button', { name: '保存文件' }));
  await next(page);
  await showStep(page, 'save', shot('files-save'));
  await next(page);
  await showStep(page, 'close', shot('files-close'), page.getByTestId('workbench-close-files'));
  await page.getByTestId('workbench-close-files').click();
  await next(page);

  // 6. Agent teamwork: delegation, the wake, the helper's envelope.
  await openChapter(page, '协作');
  const workLine = pane.locator('.thread-worked-toggle').first();
  await showStep(page, 'delegate', shot('agents-delegate'), workLine);
  await workLine.click();
  const wake = pane.locator('.thread-peer-work-toggle');
  await showStep(page, 'wake', shot('agents-wake'), wake);
  await expect(pane).toContainText('已把测试交给「补充单元测试」线程');
  await wake.click();
  await expect(pane.locator('.thread-peer-report')).toContainText('tests/login.test.ts');
  await next(page);
  const agentToggle = page.locator('aside.matter-sidebar button.matter-agent-toggle:visible').first();
  await showStep(page, 'children', shot('agents-children'), agentToggle);
  await next(page);
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/${HELPER}$`));
  await showStep(page, 'envelope', shot('agents-envelope'), pane.locator('.prompt-envelope-head'));
  await expect(pane.locator('.prompt-envelope-head')).toContainText('重构登录流程');
  await next(page);
  await showStep(page, 'parent', shot('agents-parent'), pane.locator('.thread-relation-bar'));
  await next(page);
  await showStep(page, 'cli', shot('agents-cli'), pane.getByRole('textbox', { name: '提示词' }));
  await next(page);

  // 7. Sharing, kept in the page.
  await openChapter(page, '共享');
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/${MAIN}$`));
  await showStep(page, 'open', shot('sharing-tools'), page.locator('.matter-tabs-row button.matter-toolbar-toggle'), { prerequisite: true });
  await page.locator('.matter-tabs-row button.matter-toolbar-toggle').click();
  const shareButton = page.locator('#matter-thread-tools').getByRole('button', { name: '共享权限' });
  await showStep(page, 'open', shot('sharing-open'), shareButton);
  await shareButton.click();
  const form = page.locator('form#thread-actions-share-form');
  await showStep(page, 'scope', shot('sharing-scope'), form.locator('fieldset.matter-share-scopes'));
  await next(page);
  const invite = form.getByPlaceholder('用户名或电子邮箱');
  await showStep(page, 'invite', shot('sharing-invite'), invite);
  await invite.fill('alice');
  await next(page);
  await showStep(page, 'access', shot('sharing-access'));
  await form.getByRole('radio', { name: '协作者' }).check();
  await next(page);
  const submit = page.locator('button[type="submit"][form="thread-actions-share-form"]');
  await showStep(page, 'submit', shot('sharing-submit'), submit);
  await submit.click();
  await expect(page.locator('.matter-actions-dialog')).toContainText('alice');
  await snapshot(page, shot('sharing-created'));
  await next(page);
  await showStep(page, 'elsewhere', shot('sharing-elsewhere'));
  await page.keyboard.press('Escape');
  await next(page);

  // 8. Automation and notifications.
  await openChapter(page, '自动化与通知');
  await showStep(page, 'automation', shot('more-tools'), toolsToggle, { prerequisite: true });
  await toolsToggle.click();
  const automation = page.locator('button.matter-watches-toggle[aria-label="自动化"]');
  await showStep(page, 'automation', shot('more-automation'), automation);
  await next(page);
  await showStep(page, 'subagents', shot('more-subagents'));
  await next(page);
  await showStep(page, 'notifications', shot('more-notifications'), page.getByRole('button', { name: '通知' }).first());
  await next(page);
  await showStep(page, 'finish', shot('more-finish'));
  await next(page);
  const hub = page.locator('#pockymoe-tour-root .pm-tour-hub');
  await expect(hub.locator('.pm-tour-chapter-index.is-done')).toHaveCount(8);
  await snapshot(page, shot('hub-all-seen'));

  // Closing returns to the page the tour started from; nothing reached a server.
  await hub.getByRole('button', { name: '关闭教程' }).click();
  await expect(page).toHaveURL(/\/workspaces$/);
  expect(reachedServer()).toEqual([]);
  const own = await page.evaluate(() => Object.entries(localStorage).filter(([key, value]) => !key.endsWith(':pockymoe-tour') && value.includes('pockymoe-tour')).map(([key]) => key));
  expect(own).toEqual([]);

  // The rail entry reopens it; the progress survives a reload.
  await page.reload();
  await expect(page.locator('#pockymoe-tour-root .pm-tour-welcome')).toHaveCount(0);
  const stored = await page.evaluate(() => Object.entries(localStorage).filter(([key]) => key.startsWith('pockymoe.onboarding.v1:')));
  expect(stored).toHaveLength(1);
  expect(JSON.parse(stored[0]![1]).completed).toHaveLength(8);
});

test('phone guided tour keeps cards on screen on the sample device', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Touch chapters');
  test.setTimeout(240_000);
  await prepare(page);
  const shot = shots('mobile');

  await page.goto('/workspaces');
  const welcome = page.locator('#pockymoe-tour-root .pm-tour-welcome');
  await expect(welcome).toBeVisible();
  await expectInViewport(page, welcome);
  await welcome.getByRole('button', { name: '开始教程' }).tap();

  await openChapter(page, '对话');
  await next(page);
  await showStep(page, 'import-form', shot('threads-import-form'), page.locator('form:has(#backend-provider)'));
  await next(page);
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/${MAIN}$`));
  await showStep(page, 'history', shot('threads-history'));
  await contents(page);

  await openChapter(page, '终端');
  await showStep(page, 'open', shot('terminal-open'));
  await page.locator('nav.matter-phone-dock button[aria-label="终端"], header.matter-topbar button[aria-label="终端"]').first().tap();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(panel).toBeVisible();
  await showStep(page, 'type', shot('terminal-type'), panel.locator('[data-testid="terminal-panel"] .xterm').first());
  await next(page);
  await next(page);
  await next(page);
  await next(page);
  await showStep(page, 'touch', shot('terminal-touch'), panel.getByRole('toolbar', { name: '终端控件' }));
  await contents(page);

  await openChapter(page, '协作');
  const pane = page.getByTestId('primary-pane');
  const workLine = pane.locator('.thread-worked-toggle').first();
  await showStep(page, 'delegate', shot('agents-delegate'), workLine);
  await workLine.tap();
  await showStep(page, 'wake', shot('agents-wake'), pane.locator('.thread-peer-work-toggle'));
  await next(page);
  await next(page);
  await expect(page).toHaveURL(new RegExp(`${SAMPLE}/threads/${HELPER}$`));
  await showStep(page, 'envelope', shot('agents-envelope'), pane.locator('.prompt-envelope-head'));
  await contents(page);

  await openChapter(page, '共享');
  await showStep(page, 'open', shot('sharing-more'), undefined, { prerequisite: true });
  await page.locator('nav.matter-phone-dock button.matter-phone-more').tap();
  const sheet = page.locator('section.matter-phone-sheet[data-open]');
  const shareButton = sheet.getByRole('button', { name: '共享权限' });
  await showStep(page, 'open', shot('sharing-open'), shareButton);
  await shareButton.tap();
  await showStep(page, 'scope', shot('sharing-scope'), page.locator('form#thread-actions-share-form fieldset.matter-share-scopes'));
  await tourCard(page).getByRole('button', { name: '关闭教程' }).tap();
  await expect(tourCard(page)).toHaveCount(0);
  await expect(page).toHaveURL(/\/workspaces$/);
});
