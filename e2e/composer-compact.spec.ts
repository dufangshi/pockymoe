import { selectControl } from './selectControl';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const screenshots = process.env.COMPOSER_SCREENSHOT_DIR;
const longPrompt = '请检查工作台中每个会话的输入体验，确认中文输入、长段落和附件不会丢失。'.repeat(5);

async function fixture(request: APIRequestContext, page: Page) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `composer-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label: '远程工作台 · 输入体验' } })).json();
  const threads = [];
  for (const title of ['实现：紧凑输入与独立草稿', '评审：输入体验与验收记录']) {
    const response = await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'codex', model: 'default', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    const thread = value.thread ?? value;
    threads.push(thread);
    expect((await request.post(`${api}/api/threads/${thread.id}/prompt`, { data: { prompt: 'hello' } })).ok()).toBeTruthy();
    await expect.poll(async () => (await (await request.get(`${api}/api/threads/${thread.id}`)).json()).thread.status).toBe('idle');
    // Normal product history rendering, with explicitly isolated fake-harness prose.
    await page.route(`**/api/threads/${thread.id}?*`, async route => {
      const response = await route.fetch();
      const detail = await response.json();
      for (const turn of detail.turns) for (const item of turn.items) {
        if (item.kind === 'userMessage' && item.text === 'hello') item.text = '请检查紧凑输入体验，并保留双会话的独立草稿。';
        if (item.kind === 'agentMessage' && ['ok: hello', 'hello'].includes(item.text)) item.text = '## 输入体验检查\n\n工作台已准备好，可以继续实现和评审。\n\n### 本轮验收\n\n- 空白和短提示词保持一行。\n- 聚焦长正文时展开，离开输入框后收起。\n- 两个会话分别保留自己的草稿与附件。\n\n### 下一步\n\n补充中文输入和手机布局的检查记录，再继续讨论。\n\n> 本页使用隔离 fake harness 验收数据。';
      }
      await route.fulfill({ response, json: detail });
    });
  }
  return { a: threads[0]!, b: threads[1]! };
}

const editor = (pane: Locator) => pane.getByRole('textbox', { name: '提示词', exact: true });
const form = (input: Locator) => input.locator('xpath=ancestor::form');

async function assertCompactGeometry(input: Locator) {
  await expect(form(input)).toHaveAttribute('data-composer-layout', 'collapsed');
  const geometry = await form(input).evaluate(node => {
    const rect = (selector: string) => { const r = node.querySelector(selector)!.getBoundingClientRect(); return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height }; };
    return { input: rect('[role=textbox]'), tools: rect('.composer-tools'), model: rect('.composer-model-control'), send: rect('.thread-graph-composer-send-button'), toolbar: rect('.thread-graph-composer-toolbar'), shell: rect('.thread-graph-composer-shell') };
  });
  expect(geometry.input.height, JSON.stringify(geometry)).toBeLessThanOrEqual(25);
  expect(geometry.input.width).toBeGreaterThan(40);
  expect(geometry.input.right).toBeLessThanOrEqual(geometry.tools.left - 3);
  expect(geometry.tools.right).toBeLessThanOrEqual(geometry.model.left + 1);
  expect(geometry.model.right).toBeLessThanOrEqual(geometry.send.left);
  expect(geometry.toolbar.right).toBeLessThanOrEqual(geometry.shell.right);
  expect(Math.abs(geometry.send.bottom - geometry.tools.bottom)).toBeLessThanOrEqual(3);
}

async function screenshot(page: Page, name: string) {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await page.screenshot({ path: path.join(screenshots, name), scale: 'css' });
}

test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'light');
  });
});

test('compact layout follows actual focus and rendered width in both conversation panes', async ({ page, request }, testInfo) => {
  const mobile = testInfo.project.name === 'mobile-chromium';
  await page.setViewportSize(mobile ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
  const { a, b } = await fixture(request, page);
  await page.goto(`/threads/${a.id}`);
  const primary = page.getByTestId('primary-pane');
  const reference = page.getByTestId('reference-pane');
  const left = editor(primary), right = editor(reference);
  await expect(left).toBeVisible();
  await selectControl(page, page.getByRole('combobox', { name: '对照此设备的会话' }), b.id);
  await expect(right).toBeVisible();
  await expect(reference.locator('.workbench-composer-target')).toHaveCount(0);
  const views = page.getByRole('navigation', { name: '工作台视图' });
  const show = async (title: string) => { if (mobile) await views.getByRole('button', { name: title, exact: true }).click(); };
  for (const [input, title] of [[right, b.title], [left, a.title]] as const) {
    await show(title);
    await input.focus();
    await assertCompactGeometry(input);
    await input.fill('继续');
    await assertCompactGeometry(input);
    await input.fill(longPrompt);
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
    await input.press('Home'); await input.press('ArrowRight');
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
    // A click on the model trigger blurs the editor. Its bottom/right baseline
    // stays fixed through collapse, so that same click still opens the menu.
    const trigger = form(input).locator('.composer-model-control > button');
    const before = await trigger.boundingBox();
    await trigger.click();
    await expect(trigger).toHaveAttribute('aria-expanded', 'true');
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'collapsed');
    const after = await trigger.boundingBox();
    expect(Math.abs(before!.x - after!.x)).toBeLessThan(1);
    expect(Math.abs(before!.y - after!.y)).toBeLessThan(1);
    await expect(input).toHaveText(longPrompt);
    await trigger.click();
    await input.focus();
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
    await input.fill('');
    await input.evaluate(node => {
      const clipboardData = new DataTransfer();
      clipboardData.setData('text/plain', '第一行\n第二行');
      node.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }));
    });
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
    await input.evaluate(node => (node as HTMLElement).blur());
    await assertCompactGeometry(input);
    await expect(input).toHaveText('第一行\n第二行', { useInnerText: true });
    await input.focus();
    await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
    await input.press('Control+A');
    await input.press('Backspace');
    await assertCompactGeometry(input);
    await input.pressSequentially('继续');
    await assertCompactGeometry(input);
  }
  await show(a.title);
  // A real running fake turn exposes stop + send together on the far right.
  expect((await request.post(`${api}/api/threads/${a.id}/prompt`, { data: { prompt: 'inspect this repository' } })).ok()).toBeTruthy();
  const stop = primary.getByRole('button', { name: '停止当前轮次', exact: true });
  await expect(stop).toBeVisible();
  await assertCompactGeometry(left);
  if (mobile) expect((await primary.locator('.composer-model-control > button').boundingBox())!.width).toBeLessThanOrEqual(54);
  const stopBox = await stop.boundingBox();
  const sendBox = await primary.locator('.thread-graph-composer-send-button').boundingBox();
  expect(stopBox!.x + stopBox!.width).toBeLessThanOrEqual(sendBox!.x);
  expect(Math.abs(stopBox!.y - sendBox!.y)).toBeLessThan(1);
  await left.evaluate(node => (node as HTMLElement).blur());
  await screenshot(page, mobile ? 'mobile-collapsed.png' : 'desktop-collapsed.png');
  await left.fill('请整理本轮输入体验的验收记录。\n\n' + longPrompt);
  await expect(form(left)).toHaveAttribute('data-composer-layout', 'expanded');
  await left.evaluate(node => { node.scrollTop = 0; });
  await screenshot(page, mobile ? 'mobile-expanded.png' : 'desktop-expanded.png');
  await stop.click();
  await expect(stop).toHaveCount(0);
  // Magnification and font metrics can change the available line budget.
  // Scope zoom to the composer so this assertion does not switch workbench tabs.
  await left.press('Control+A'); await left.press('Backspace');
  await left.pressSequentially('宽度检查');
  await assertCompactGeometry(left);
  await form(left).evaluate(node => { (node as HTMLElement).style.zoom = '1.25'; });
  await left.evaluate(node => { (node as HTMLElement).style.fontSize = '64px'; window.dispatchEvent(new Event('resize')); });
  await expect(form(left)).toHaveAttribute('data-composer-layout', 'expanded');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(mobile ? 390 : 1440);
  await left.evaluate(node => { (node as HTMLElement).style.fontSize = ''; });
  await form(left).evaluate(node => { (node as HTMLElement).style.zoom = ''; });
});

test('composition, attachment menus and an accepted send preserve the next draft', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-chromium', 'Protocol and IME mechanism covered once; mobile layout has its own regression.');
  const { a } = await fixture(request, page);
  await page.goto(`/threads/${a.id}`);
  const pane = page.getByTestId('primary-pane'), input = editor(pane);
  await input.fill('中文组合');
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  let sends = 0;
  await page.route(`**/api/threads/${a.id}/prompt`, async route => { sends++; await held; await route.continue(); });
  await input.dispatchEvent('compositionstart', { data: '组合' });
  await input.dispatchEvent('keydown', { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 });
  expect(sends).toBe(0);
  await expect(input).toHaveText('中文组合');
  await input.dispatchEvent('compositionend', { data: '组合' });
  await input.fill('提交旧草稿');
  await input.press('Control+Enter');
  await expect.poll(() => sends).toBe(1);
  await input.fill('在途的新草稿\n下一行');
  const add = pane.getByRole('button', { name: '添加附件', exact: true });
  await add.click();
  await expect(pane.getByRole('button', { name: '文件', exact: true })).toBeVisible();
  await expect(form(input)).toHaveAttribute('data-composer-layout', 'collapsed');
  await add.click();
  await pane.locator('input[type=file]').last().setInputFiles({ name: 'next-draft.txt', mimeType: 'text/plain', buffer: Buffer.from('next draft attachment') });
  await expect(input.locator('[data-segment-type=attachment]')).toHaveCount(1);
  const accepted = page.waitForResponse(response => response.url().endsWith(`/api/threads/${a.id}/prompt`));
  release();
  await accepted;
  await expect(pane.getByRole('button', { name: '发送提示词', exact: true })).not.toHaveAttribute('title', '正在发送…');
  await expect(input).toContainText('在途的新草稿');
  await expect(input.locator('[data-segment-type=attachment]')).toHaveCount(1);
  await input.focus();
  await expect(form(input)).toHaveAttribute('data-composer-layout', 'expanded');
  await page.unroute(`**/api/threads/${a.id}/prompt`);
  await pane.getByRole('button', { name: '发送提示词', exact: true }).click();
  await expect(input).toHaveText('');
  await expect.poll(async () => (await (await request.get(`${api}/api/threads/${a.id}`)).json()).turns.length).toBe(3);
  const detail = await (await request.get(`${api}/api/threads/${a.id}`)).json();
  const message = detail.turns.at(-1).items.find((item: { kind: string }) => item.kind === 'userMessage').text;
  expect(message).toMatch(/在途的新草稿\s*\n下一行/);
  expect(message).toContain(`/threads/${a.id}/next-draft-`);
});
