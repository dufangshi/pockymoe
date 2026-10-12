import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

test.afterEach(async ({ page }) => { await page.unrouteAll({ behavior: 'ignoreErrors' }); });

async function openPlum(page: Page, request: APIRequestContext, mode: 'light' | 'dark' = 'light', running = false, preset: 'classic' | 'plum-pocket' = 'plum-pocket') {
  // A subscription and a reported context window, so the usage meter and the bar render.
  let usageAvailable = true;
  await page.route('**/api/agent-runtimes/*/subscription-usage*', (route) => route.fulfill({ json: { usage: usageAvailable ? {
    provider: 'claude', authKind: 'subscription', observedAt: new Date().toISOString(), stale: false,
    windows: [
      { id: '5h', durationMinutes: 300, label: '5h', usedPercent: 20, resetsAt: new Date(Date.now() + 3_600_000).toISOString() },
      { id: '7d', durationMinutes: 10_080, label: '7d', usedPercent: 40, resetsAt: new Date(Date.now() + 86_400_000).toISOString() },
    ] } : null } }));
  await page.route(/\/api\/threads\/[0-9a-f-]+\?/, async (route) => {
    const response = await route.fetch();
    const detail = await response.json();
    detail.thread.contextUsage = { availability: 'available', remainingPercent: 60, tokensInContextWindow: 1000, modelContextWindow: 2500, updatedAt: new Date().toISOString() };
    if (running) Object.assign(detail.thread, { status: 'running', activeTurnId: 'turn-running' });
    await route.fulfill({ response, json: detail });
  });
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `plum-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'plum' } })).json();
  const started = await (await request.post(`${api}/api/threads/start`, {
    data: { workspaceId: workspace.id, title: 'Plum composer', provider: 'acp', agentId: 'claude', model: 'ios-e2e-stream', approvalMode: 'yolo' },
  })).json();
  const thread = started.thread ?? started;
  await page.addInitScript(({ mode, preset }) => {
    localStorage.setItem('pockymoe-theme-preset', preset);
    localStorage.setItem('remote-codex-theme-mode', mode);
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([window.location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
  }, { mode, preset });
  await page.goto(`/threads/${thread.id}`);
  const later = page.getByRole('button', { name: 'Later' });
  if (await later.isVisible({ timeout: 3000 }).catch(() => false)) await later.click();
  const composer = page.getByTestId('chat-composer');
  await expect(composer).toBeVisible();
  return { composer, editor: composer.getByRole('textbox', { name: 'Prompt', exact: true }), hideUsage: async () => { usageAvailable = false; await page.reload(); await expect(composer).toBeVisible(); await expect(composer.locator('.thread-composer-usage-tag')).toHaveCount(0); } };
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
    // 36px keys whose hit areas reach 44px.
    expect(g.slash.height).toBeGreaterThanOrEqual(36);
    expect(g.send.width).toBeGreaterThanOrEqual(42);
    const hit = await composer.locator('.thread-graph-composer-icon-button').first().evaluate((el) => getComputedStyle(el, '::after').inset);
    expect(hit).toBe('-4px');
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

for (const preset of ['classic', 'plum-pocket'] as const)
for (const mode of ['light', 'dark'] as const) test(`${preset} usage meter replaces the border without shifting controls (${mode})`, async ({ page, request }, info) => {
  const { composer, editor, hideUsage } = await openPlum(page, request, mode, false, preset);
  const usage = composer.locator('.thread-graph-composer-shell .thread-composer-usage-tag .thread-subscription-usage');
  await expect(usage).toBeVisible();
  await expect(composer.locator('.thread-composer-usage-footer')).toHaveCount(0);
  const g = await composer.evaluate((node) => {
    const r = (s: string) => node.querySelector(s)!.getBoundingClientRect();
    return { shell: r('.thread-graph-composer-shell'), usage: r('.thread-subscription-usage'), chip: r('.composer-model-label'), bar: r('.thread-context-progress-track'), size: getComputedStyle(node.querySelector('.thread-subscription-usage')!).fontSize };
  });
  // A legend centred on the bottom border, with no separate row or pill.
  expect(Math.abs((g.usage.top + g.usage.bottom) / 2 - g.shell.bottom)).toBeLessThanOrEqual(1.5);
  expect(g.usage.bottom).toBeLessThan(page.viewportSize()!.height - 4);
  expect(g.usage.right).toBeLessThan(g.shell.right - 15);
  expect(g.usage.left).toBeGreaterThan(g.shell.left + 15);
  await expect(usage).toHaveCSS('border-top-width', '0px');
  await expect(usage).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  expect(await usage.evaluate((el) => getComputedStyle(el).backgroundImage)).toContain('rgba(0, 0, 0, 0)');
  expect(parseFloat(g.size)).toBeLessThanOrEqual(9);
  if (preset === 'plum-pocket') {
    // The context bar sits inside the chip with equal insets.
    expect(Math.abs((g.bar.left - g.chip.left) - (g.chip.right - g.bar.right))).toBeLessThanOrEqual(1);
    expect(g.bar.bottom).toBeLessThanOrEqual(g.chip.bottom);
  }
  await editor.click();
  const focused = await composer.locator('.thread-graph-composer-shell').boundingBox();
  await usage.click();
  await expect(usage).toHaveAttribute('aria-expanded', 'true');
  const details = page.getByRole('tooltip');
  await expect(details).toBeVisible();
  await expect(details).toContainText('80% remaining');
  await expect(details).toContainText('60% remaining');
  const surface = await details.evaluate((el) => {
    const bounds = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const topmost = document.elementFromPoint(bounds.left + bounds.width / 2, bounds.top + bounds.height / 2);
    return { topLayer: el.matches(':popover-open'), bg: style.backgroundColor, opacity: style.opacity, topmost: !!topmost && el.contains(topmost), bounds };
  });
  expect(surface.topLayer).toBe(true);
  expect(surface.opacity).toBe('1');
  expect(surface.bg).not.toMatch(/rgba|transparent|\/\s*0[.]/);
  expect(surface.topmost).toBe(true);
  expect(surface.bounds.left).toBeGreaterThanOrEqual(0);
  expect(surface.bounds.right).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(surface.bounds.bottom).toBeLessThan(focused!.y);
  // Opening the usage panel keeps the prompt geometry stable.
  expect(await composer.locator('.thread-graph-composer-shell').boundingBox()).toEqual(focused);
  await page.screenshot({ path: info.outputPath(`usage-${preset}-${mode}-open.png`), scale: 'css' });
  await page.keyboard.press('Escape');
  await expect(details).toHaveCount(0);
  await expect(usage).toHaveAttribute('aria-expanded', 'false');
  await dismiss(page);
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  await page.screenshot({ path: info.outputPath(`usage-${preset}-${mode}-closed.png`), scale: 'css' });
  const geometry = () => composer.evaluate((node) =>
    ['.thread-graph-composer-shell', '.thread-graph-composer-toolbar', '.composer-model-label', '.thread-graph-composer-send-button']
      .map((selector) => node.querySelector(selector)!.getBoundingClientRect().toJSON()));
  const withUsage = await geometry();
  await hideUsage();
  // Adding allowance data must never add padding, raise buttons, or move a pane.
  expect(await geometry()).toEqual(withUsage);
});

test('plum settings fit every tab on one line and keep hints for screen readers', async ({ page, request }, info) => {
  await openPlum(page, request);
  await page.getByRole('button', { name: 'Open settings', exact: true }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(dialog).toBeVisible();
  const nav = dialog.locator('.settings-navigation');
  const fits = await nav.evaluate((el) => el.scrollWidth <= el.clientWidth + 1);
  expect(fits).toBe(true);
  for (const tab of await nav.getByRole('tab').all()) {
    const box = (await tab.boundingBox())!;
    if (info.project.name === 'mobile-chromium') expect(box.height).toBeGreaterThanOrEqual(44);
    await expect(tab).toHaveAccessibleName(/\S/);
  }
  await nav.getByRole('tab', { name: 'Preferences' }).click();
  const hint = dialog.locator('.settings-hint').first();
  await expect(hint).toHaveCount(1);
  expect(await hint.evaluate((el) => el.getBoundingClientRect().width)).toBeLessThanOrEqual(1);
  // Toggles are switches and buttons never wrap.
  const toggle = dialog.locator('.settings-panel input[type=checkbox]').first();
  await expect(toggle).toHaveCSS('width', '34px');
  const wrapped = await dialog.locator('.settings-panel').evaluate((panel) => [...panel.querySelectorAll('button')]
    .filter((b) => b.offsetParent && b.textContent!.trim() && b.scrollWidth > b.clientWidth + 1).map((b) => b.textContent));
  expect(wrapped).toEqual([]);
});

test('plum composer shows a quiet stop ring and a labelled send-after-turn key while a turn runs', async ({ page, request }, info) => {
  const mobile = info.project.name === 'mobile-chromium';
  const { composer } = await openPlum(page, request, 'light', true);
  const stop = composer.getByRole('button', { name: 'Stop current turn' });
  const queue = composer.getByRole('button', { name: 'Send after this turn', exact: true });
  await expect(stop).toBeVisible();
  await expect(queue).toHaveAttribute('title', 'Send after this turn');
  await expect(queue).toHaveAttribute('data-send-intent', 'queue');
  await expect(queue.locator('.composer-send-later')).toBeVisible();
  await expect(stop).toHaveCSS('border-top-left-radius', '50%');
  const g = await composer.evaluate((node) => {
    const r = (s: string) => node.querySelector(s)!.getBoundingClientRect();
    const stopEl = node.querySelector('.thread-graph-composer-stop-button')!;
    const after = getComputedStyle(stopEl, '::after');
    return { stop: r('.thread-graph-composer-stop-button'), queue: r('.thread-graph-composer-send-button'), chip: r('.composer-model-label'), hit: after.inset };
  });
  // Neither key outweighs the model chip; stop is the smallest, before queue.
  expect(g.stop.width).toBeLessThan(g.queue.width);
  expect(g.queue.height).toBeLessThanOrEqual(g.chip.height + 1);
  expect(g.stop.right).toBeLessThanOrEqual(g.queue.left);
  expect(g.chip.right).toBeLessThanOrEqual(g.stop.left);
  if (mobile) {
    expect(g.stop.width).toBeLessThanOrEqual(32);
    expect(g.hit).toBe('-6px');
  } else {
    expect(g.stop.width).toBeLessThanOrEqual(28);
  }
});

for (const preset of ['classic', 'plum-pocket'] as const) test(`${preset} compact model menu shows six models, reasoning and context together`, async ({ page, request }, info) => {
  await page.route(/\/api\/(agent-runtimes\/[^/]+|threads\/[0-9a-f-]+)\/models(?:\?|$)/, route => route.fulfill({ json:
    ['Opus 5.5', '6.1 Sol', '6 Astra', '6 Sol', '6 Luna', '5.6 Sol'].map((displayName, index) => ({
      id: index ? `model-${index}` : 'ios-e2e-stream', model: index ? `model-${index}` : 'ios-e2e-stream', displayName,
      description: '', hidden: false, isDefault: index === 0, defaultReasoningEffort: 'medium',
      supportedReasoningEfforts: ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'].map(reasoningEffort => ({ reasoningEffort, description: '' })),
    }))
  }));
  const { composer } = await openPlum(page, request, 'dark', false, preset);
  await composer.getByTestId('composer-model-label').click();
  const menu = page.locator('.composer-model-menu');
  await expect(menu).toBeVisible();
  await expect(menu.getByRole('menuitemradio')).toHaveCount(6);
  await expect(menu.locator('.composer-context-value')).toHaveText('1K / 2.5K');
  await expect(menu.getByRole('slider')).toBeVisible();
  await expect(menu).toHaveCSS('opacity', '1');
  const background = await menu.evaluate(element => getComputedStyle(element).backgroundColor);
  expect(background).not.toMatch(/(?:\/|,)\s*0\.\d+\s*\)$/);
  const geometry = await menu.evaluate(element => {
    const rect = element.getBoundingClientRect();
    const context = element.querySelector('.composer-context-summary')!.getBoundingClientRect();
    const last = element.querySelector('[role=menuitemradio]:last-child')!.getBoundingClientRect();
    return { top: rect.top, bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right,
      contextTop: context.top, contextBottom: context.bottom, lastBottom: last.bottom,
      rowHeights: [...element.querySelectorAll('[role=menuitemradio]')].map(button => button.getBoundingClientRect().height) };
  });
  expect(geometry.height, JSON.stringify(geometry)).toBeLessThanOrEqual(340);
  expect(geometry.top).toBeGreaterThanOrEqual(8);
  expect(geometry.left).toBeGreaterThanOrEqual(8);
  expect(geometry.right).toBeLessThanOrEqual(page.viewportSize()!.width - 8);
  expect(geometry.lastBottom).toBeLessThan(geometry.contextTop);
  expect(geometry.contextBottom).toBeLessThanOrEqual(geometry.bottom);
  expect(Math.max(...geometry.rowHeights)).toBeLessThanOrEqual(32);
  await page.screenshot({ path: info.outputPath(`model-menu-${preset}.png`), scale: 'css' });
  await menu.getByRole('button', { name: 'low', exact: true }).click();
  await expect(composer.getByTestId('composer-model-label')).toContainText('low');
  await composer.getByTestId('composer-model-label').click();
  await expect(menu.locator('.composer-reasoning')).toHaveAttribute('data-effort', 'low');
});
