import { selectControl } from './selectControl';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import {
  expect,
  test,
  type APIRequestContext,
  type Page,
  type Locator,
} from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const modelName = 'GPT-6.1 Sol';

async function openComposer(page: Page, request: APIRequestContext) {
  const absPath = path.resolve(
    process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright',
    `composer-${randomUUID()}`,
  );
  await mkdir(absPath, { recursive: true });
  const workspaceResponse = await request.post(`${api}/api/workspaces`, {
    data: { absPath, label: 'Composer focus fixture' },
  });
  expect(workspaceResponse.ok()).toBeTruthy();
  const workspace = await workspaceResponse.json();
  const threadResponse = await request.post(`${api}/api/threads/start`, {
    data: {
      workspaceId: workspace.id,
      title: 'Composer focus fixture',
      provider: 'codex',
      model: 'default',
      approvalMode: 'yolo',
    },
  });
  expect(threadResponse.ok()).toBeTruthy();
  const started = await threadResponse.json();
  const thread = started.thread ?? started;
  await page.route(`**/api/threads/${thread.id}/models`, (route) =>
    route.fulfill({
      json: [
        {
          id: 'default',
          model: 'default',
          displayName: modelName,
          description: '',
          isDefault: true,
          hidden: false,
          defaultReasoningEffort: 'medium',
          supportedReasoningEfforts: [
            { reasoningEffort: 'low', description: '' },
            { reasoningEffort: 'medium', description: '' },
            { reasoningEffort: 'high', description: '' },
            { reasoningEffort: 'xhigh', description: '' },
          ],
        },
      ],
    }),
  );
  const capabilityResponse = await request.get(
    `${api}/api/threads/${thread.id}/capabilities`,
  );
  expect(capabilityResponse.ok()).toBeTruthy();
  const snapshot = await capabilityResponse.json();
  snapshot.toolboxItems = [
    {
      command: '/model',
      label: 'Session settings',
      description: 'Model and permissions',
      action: 'harness',
    },
  ];
  await page.route(`**/api/threads/${thread.id}/capabilities`, (route) =>
    route.fulfill({ json: snapshot }),
  );
  await page.goto(`/threads/${thread.id}`);
  const composer = page.getByTestId('chat-composer');
  await expect(composer.locator('.composer-model-name')).toHaveText(modelName);
  return {
    composer,
    editor: composer.getByRole('textbox', { name: 'Prompt', exact: true }),
    id: thread.id,
  };
}

async function activate(locator: Locator, touch: boolean) {
  if (touch) await locator.tap();
  else await locator.click();
}

async function screenshot(page: Page, name: string, area?: Locator) {
  if (!process.env.COMPOSER_SCREENSHOT_DIR) return;
  await mkdir(process.env.COMPOSER_SCREENSHOT_DIR, { recursive: true });
  const target = path.join(process.env.COMPOSER_SCREENSHOT_DIR, name);
  if (area) await area.screenshot({ path: target });
  else await page.screenshot({ path: target, fullPage: true });
}

test('multiline composer stays expanded across plus/slash menus and keyboard focus, and collapses outside', async ({
  page,
  request,
  isMobile,
}) => {
  const { composer, editor } = await openComposer(page, request);
  await editor.fill('First line\nSecond line\nThird line');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  const plus = composer.getByRole('button', {
    name: 'Add attachment',
    exact: true,
  });
  await activate(plus, isMobile);
  await expect(
    composer.getByRole('button', { name: 'Photo', exact: true }),
  ).toBeVisible();
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await composer.getByRole('button', { name: 'Photo', exact: true }).focus();
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await page.keyboard.press('Escape');
  await expect(composer.locator('[data-composer-menu-surface]')).toHaveCount(0);
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');

  const slash = composer.getByRole('button', {
    name: 'Open slash toolbox',
    exact: true,
  });
  await activate(slash, isMobile);
  const settings = composer.getByRole('button', { name: /\/model/ });
  await expect(settings).toBeVisible();
  await settings.focus();
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-menu.png`,
  );
  await page.keyboard.press('Escape');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await slash.focus();
  await slash.press('Tab');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await activate(page.locator('.matter-workbench').first(), isMobile);
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  await expect(editor).toHaveText('First line\nSecond line\nThird line', {
    useInnerText: true,
  });

  await editor.fill('short');
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  await editor.fill('');
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  const label = composer.getByTestId('composer-model-label');
  expect(
    await label.evaluate((node) => node.scrollWidth <= node.clientWidth),
  ).toBe(true);
  await expect(composer.locator('.composer-sandbox-control')).toHaveCount(0);
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-compact.png`,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

test('collapsed composer model, plus and slash controls are clickable and preserve expanded drafts', async ({
  page,
  request,
  isMobile,
}) => {
  await page.addInitScript(() =>
    localStorage.setItem('remote-codex-theme-mode', 'dark'),
  );
  const { composer, editor, id } = await openComposer(page, request);
  const model = composer.getByTestId('composer-model-label');
  await expect(composer).toHaveAttribute('data-composer-layout', 'collapsed');
  await expect(model).toBeEnabled();
  await expect(model).toHaveCSS('cursor', 'pointer');
  await activate(model, isMobile);
  const effort = composer.getByRole('slider', { name: 'Effort', exact: true });
  await expect(effort).toBeVisible();
  await expect(effort).toHaveAttribute('aria-valuemax', '3');
  await effort.focus();
  await page.keyboard.press('ArrowRight');
  await expect
    .poll(
      async () =>
        (await (await request.get(`${api}/api/threads/${id}`)).json()).thread
          .reasoningEffort,
    )
    .toBe('high');
  await expect(model.locator('.composer-model-effort')).toHaveText('high');
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-effort-label.png`,
    composer,
  );
  await activate(
    composer.getByRole('button', { name: 'Add attachment', exact: true }),
    isMobile,
  );
  await expect(
    composer.getByRole('button', { name: 'Photo', exact: true }),
  ).toBeVisible();
  await page.keyboard.press('Escape');
  await activate(
    composer.getByRole('button', { name: 'Open slash toolbox', exact: true }),
    isMobile,
  );
  await expect(composer.getByRole('button', { name: /\/model/ })).toBeVisible();
  await page.keyboard.press('Escape');
  await editor.fill('First line\nSecond line\nThird line');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await activate(model, isMobile);
  await expect(
    composer.getByRole('menuitemradio', { name: modelName, exact: true }),
  ).toBeVisible();
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-model-menu.png`,
  );
  const beforeColor = await composer
    .locator('.composer-reasoning')
    .evaluate((el) => getComputedStyle(el).getPropertyValue('--effort-color'));
  const thumb = await effort.boundingBox();
  const track = await composer
    .locator('.composer-reasoning-slider')
    .boundingBox();
  const start = {
    x: thumb!.x + thumb!.width / 2,
    y: thumb!.y + thumb!.height / 2,
  };
  const end = { x: track!.x + track!.width - 11, y: start.y };
  const client = isMobile ? await page.context().newCDPSession(page) : null;
  if (client) {
    await client.send('Input.dispatchTouchEvent', {
      type: 'touchStart',
      touchPoints: [start],
    });
    await client.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [end],
    });
  } else {
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    await page.mouse.move(end.x, end.y, { steps: 5 });
  }
  await expect(effort).toHaveAttribute('aria-valuetext', 'xhigh');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await expect
    .poll(() =>
      composer
        .locator('.composer-reasoning')
        .evaluate((el) =>
          getComputedStyle(el).getPropertyValue('--effort-color'),
        ),
    )
    .not.toBe(beforeColor);
  expect(
    (await (await request.get(`${api}/api/threads/${id}`)).json()).thread
      .reasoningEffort,
  ).toBe('high');
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-slider-preview.png`,
  );
  if (client) {
    await client.send('Input.dispatchTouchEvent', {
      type: 'touchEnd',
      touchPoints: [],
    });
    await client.detach();
  } else await page.mouse.up();
  await expect
    .poll(
      async () =>
        (await (await request.get(`${api}/api/threads/${id}`)).json()).thread
          .reasoningEffort,
    )
    .toBe('xhigh');
  await expect(model.locator('.composer-model-effort')).toHaveText('xhigh');
  await page.route(`**/api/threads/${id}/models`, (route) =>
    route.fulfill({
      json: [
        {
          id: 'default',
          model: 'default',
          displayName: modelName,
          description: '',
          isDefault: true,
          hidden: false,
          defaultReasoningEffort: null,
          supportedReasoningEfforts: [],
        },
      ],
    }),
  );
  await page.reload();
  await expect(composer.locator('.composer-model-name')).toHaveText(modelName);
  await expect(model.locator('.composer-model-effort')).toHaveCount(0);
  await expect(model).toHaveAccessibleName(modelName);
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-model-only.png`,
    composer,
  );
});

test('composer image preview portal retains expansion and settings keeps model, reasoning and permissions', async ({
  page,
  request,
  isMobile,
}) => {
  const { composer, editor, id } = await openComposer(page, request);
  await editor.fill('First line\nSecond line');
  await page.locator('input[type="file"][accept="image/*"]').setInputFiles({
    name: 'preview.png',
    mimeType: 'image/png',
    buffer: Buffer.from(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6qX8AAAAASUVORK5CYII=',
      'base64',
    ),
  });
  await editor.focus();
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await activate(
    composer.locator('[data-segment-type="attachment"] img'),
    isMobile,
  );
  const preview = page.getByRole('dialog');
  await expect(preview).toBeVisible();
  expect(await preview.evaluate((node) => !node.closest('form'))).toBe(true);
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await activate(
    preview.getByRole('button', { name: 'Zoom in', exact: true }),
    isMobile,
  );
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await activate(
    preview.getByRole('button', { name: 'Close image preview', exact: true }),
    isMobile,
  );
  await expect(preview).toHaveCount(0);
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');

  await activate(
    composer.getByRole('button', { name: 'Open slash toolbox', exact: true }),
    isMobile,
  );
  await activate(composer.getByRole('button', { name: /\/model/ }), isMobile);
  const settings = page.getByRole('dialog', {
    name: 'Harness settings',
    exact: true,
  });
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await expect(
    settings.getByRole('combobox', { name: 'Harness model', exact: true }),
  ).toHaveValue('default');
  await expect(
    settings.getByRole('combobox', {
      name: 'Harness reasoning effort',
      exact: true,
    }),
  ).toBeEnabled();
  const permissions = settings.getByRole('combobox', {
    name: 'Workspace permissions',
    exact: true,
  });
  await expect(permissions).toHaveValue('danger-full-access');
  await selectControl(page, permissions, 'workspace-write');
  await expect(composer).toHaveAttribute('data-composer-layout', 'expanded');
  await expect
    .poll(
      async () =>
        (await (await request.get(`${api}/api/threads/${id}`)).json()).thread
          .sandboxMode,
    )
    .toBe('workspace-write');
  await expect(permissions).toHaveValue('workspace-write');
  await screenshot(
    page,
    `${isMobile ? 'mobile' : 'desktop'}-composer-settings.png`,
  );
});
