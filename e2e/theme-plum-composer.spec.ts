import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function openPlum(page: Page, request: APIRequestContext, mode: 'light' | 'dark' = 'light') {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `plum-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'plum' } })).json();
  const started = await (await request.post(`${api}/api/threads/start`, {
    data: { workspaceId: workspace.id, title: 'Plum composer', provider: 'acp', agentId: 'claude', model: 'ios-e2e-stream', approvalMode: 'yolo' },
  })).json();
  const thread = started.thread ?? started;
  await page.addInitScript((m) => {
    localStorage.setItem('pockymoe-theme-preset', 'plum-pocket');
    localStorage.setItem('remote-codex-theme-mode', m);
    localStorage.setItem('remote-codex.locale', 'en');
  }, mode);
  await page.goto(`/threads/${thread.id}`);
  const later = page.getByRole('button', { name: 'Later' });
  if (await later.isVisible({ timeout: 3000 }).catch(() => false)) await later.click();
  const composer = page.getByTestId('chat-composer');
  await expect(composer).toBeVisible();
  return { composer, editor: composer.getByRole('textbox', { name: 'Prompt', exact: true }) };
}

// Close an open menu with an outside click on the empty conversation; the page corner
// holds the phone drawer button in the Plum shell.
async function dismiss(page: Page) {
  const viewport = page.viewportSize()!;
  await page.mouse.click(viewport.width / 2, viewport.height / 3);
}

test('plum composer keeps compact geometry, touch targets and the mascot', async ({ page, request }, info) => {
  const mobile = info.project.name === 'mobile-chromium';
  const { composer } = await openPlum(page, request);
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  await expect(composer).toHaveAttribute('data-agent', 'claude');
  const g = await composer.evaluate((node) => {
    const r = (s: string) => node.querySelector(s)!.getBoundingClientRect();
    const peek = getComputedStyle(node, '::before');
    return {
      input: r('[role=textbox]'), tools: r('.composer-tools'), model: r('.composer-model-control'),
      send: r('.thread-graph-composer-send-button'), slash: r('.thread-graph-composer-icon-button'),
      shell: r('.thread-graph-composer-shell'), peek: peek.backgroundImage,
    };
  });
  expect(g.input.height).toBeLessThanOrEqual(25);
  expect(g.input.width).toBeGreaterThan(40);
  expect(g.input.right).toBeLessThanOrEqual(g.tools.left - 3);
  expect(g.tools.right).toBeLessThanOrEqual(g.model.left + 1);
  expect(g.model.right).toBeLessThanOrEqual(g.send.left);
  expect(Math.abs(g.send.bottom - g.tools.bottom)).toBeLessThanOrEqual(3);
  expect(g.peek).toContain('composer-peek-claude.webp');
  if (mobile) {
    expect(g.slash.height).toBeGreaterThanOrEqual(40);
    expect(g.send.width).toBeGreaterThanOrEqual(44);
    const hit = await composer.locator('.thread-graph-composer-send-button').evaluate((el) => getComputedStyle(el, '::after').inset);
    expect(hit).toBe('-2px');
  } else {
    await expect(composer.locator('.composer-model-avatar')).toBeVisible();
  }
});

test('plum menus open by pointer and keyboard and keep their items', async ({ page, request }, info) => {
  const mobile = info.project.name === 'mobile-chromium';
  const { composer, editor } = await openPlum(page, request, 'dark');
  const slash = composer.getByRole('button', { name: 'Open slash toolbox', exact: true });
  if (mobile) await slash.tap(); else await slash.click();
  const goal = page.locator('[data-toolbox-action="goal"]');
  await expect(goal).toBeVisible();
  await expect(goal.locator('.composer-toolbox-icon')).toBeVisible();
  await expect(page.locator('[data-toolbox-action="plan"] .composer-toolbox-status')).toHaveText(/off/i);
  await page.keyboard.press('Escape');
  await expect(goal).toBeHidden();

  const attach = composer.getByRole('button', { name: 'Add attachment', exact: true });
  await attach.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: 'Photo' })).toBeVisible();
  await expect(page.locator('.composer-attach-icon')).toHaveCount(2);
  await dismiss(page);

  const model = composer.getByTestId('composer-model-label');
  await model.click();
  await expect(model).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('.composer-reasoning')).toHaveAttribute('data-effort', 'medium');
  await expect(page.getByRole('menuitemradio', { checked: true })).toBeVisible();
  await dismiss(page);

  await editor.click();
  await page.keyboard.type('hello from plum');
  await expect(composer.locator('.thread-graph-composer-send-button .composer-send-plane')).toBeVisible();
  await page.keyboard.press('Control+Enter');
  await expect(editor).toHaveText('');
});

test('plum composer respects reduced motion', async ({ page, request }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const { composer } = await openPlum(page, request);
  const transition = await composer.locator('.thread-graph-composer-send-button').evaluate((el) => getComputedStyle(el).transitionDuration);
  expect(transition.split(',').every((value) => parseFloat(value) === 0)).toBe(true);
  expect(await composer.evaluate((node) => getComputedStyle(node, '::before').transitionDuration)).toBe('0s');
});
