import { test, expect, type Locator, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

/** Every action of a connection row stays inside the row, the dialog and the screen. */
async function expectActionsInside(page: Page, row: Locator) {
  const viewport = page.viewportSize()!;
  const bounds = [
    (await row.boundingBox())!,
    (await page.getByTestId('settingsDialog').boundingBox())!,
    { x: 0, width: viewport.width },
  ];
  const buttons = row.getByRole('button');
  expect(await buttons.count()).toBeGreaterThan(0);
  for (const button of await buttons.all()) {
    const box = (await button.boundingBox())!;
    for (const outer of bounds) {
      expect(box.x).toBeGreaterThanOrEqual(outer.x - 1);
      expect(box.x + box.width).toBeLessThanOrEqual(outer.x + outer.width + 1);
    }
  }
}

/** Cancel and Save share one row inside the dialog, on any width and language. */
async function expectFormActionsInline(page: Page, dialog: Locator) {
  const actions = dialog.getByTestId('connection-form-actions');
  const [cancel, save] = await Promise.all(
    [
      actions.getByRole('button').first(),
      actions.getByRole('button').last(),
    ].map((button) => button.boundingBox()),
  );
  const box = (await dialog.boundingBox())!;
  expect(Math.abs(cancel!.y - save!.y)).toBeLessThanOrEqual(1);
  for (const button of [cancel!, save!]) {
    expect(button.x).toBeGreaterThanOrEqual(box.x - 1);
    expect(button.x + button.width).toBeLessThanOrEqual(box.x + box.width + 1);
    expect(button.y + button.height).toBeLessThanOrEqual(
      box.y + box.height + 1,
    );
    expect(button.x + button.width).toBeLessThanOrEqual(
      page.viewportSize()!.width + 1,
    );
  }
}

test('connections save without applying, apply apart from sync, and template preview stays usable', async ({
  page,
}, testInfo) => {
  page.setDefaultTimeout(15000);
  const remote = {
    id: 'team',
    name: 'Team Gemini',
    protocol: 'gemini',
    revision: 'r2',
    local: false,
    sources: [
      { deviceId: 'laptop', name: 'Laptop', online: true, revision: 'r2' },
    ],
  };
  let connections: any[] = [remote];
  let bindings: Record<string, unknown> = {};
  const writes: string[] = [];
  await page.route('**/api/management/**', async (route) => {
    const request = route.request();
    const p = new URL(request.url()).pathname;
    const method = request.method();
    if (method !== 'GET') writes.push(`${method} ${p}`);
    let result: unknown = {};
    let status = 200;
    if (p.endsWith('/supervisor'))
      result = { runningVersion: 'test', canUpdate: false };
    else if (p.endsWith('/harnesses'))
      result = [
        { id: 'codex', name: 'OpenAI Codex', base: { installed: true } },
        { id: 'grok', name: 'Grok Build', base: { installed: true } },
      ];
    else if (p.endsWith('/management/connections/models'))
      result = {
        models: [{ id: 'test-model', name: 'Test model' }],
        truncated: false,
      };
    else if (p.endsWith('/management/connections') && method === 'POST') {
      const { apiKey, ...saved } = request.postDataJSON();
      result = {
        ...saved,
        id: 'personal',
        revision: 'r1',
        hasApiKey: !!apiKey,
        local: true,
        sources: [],
      };
      connections = [...connections, result];
    } else if (p.endsWith('/management/connections'))
      result = {
        connections,
        bindings,
        deviceId: 'desk',
        catalogAvailable: true,
        harnessProtocols: {
          codex: ['openai_responses'],
          grok: ['openai_responses', 'openai_chat'],
        },
      };
    else if (p.endsWith('/connections/personal/models'))
      result = { models: [{ id: 'test-model' }], truncated: false };
    else if (p.endsWith('/connections/personal/apply')) {
      bindings = {
        codex: {
          connectionId: 'personal',
          model: 'test-model',
          revision: 'r1',
        },
      };
      result = { ok: true, profileId: 'p', restartedSessions: 0 };
    } else if (p.endsWith('/connections/personal') && method === 'DELETE') {
      status = 409;
      result = { message: 'in use', code: 'connection_in_use' };
    } else if (p.endsWith('/upstreams'))
      result = { profiles: [], active: {}, backups: [] };
    else if (p.endsWith('/templates'))
      result = { harnesses: ['codex'], profiles: [] };
    await route.fulfill({ status, json: result });
  });
  await page.goto('/workspaces');
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Harnesses', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Add connection', exact: true }),
  ).toHaveCount(0);
  await page.getByRole('tab', { name: 'Upstreams', exact: true }).click();
  const binding = page.getByTestId('harness-binding');
  await expect(binding).toContainText(
    'its own sign-in or native configuration',
  );

  await page
    .getByRole('button', { name: 'Add connection', exact: true })
    .click();
  const editor = page.getByRole('dialog', {
    name: 'Add connection',
    exact: true,
  });
  await expect(editor).toBeVisible();
  await editor.getByLabel('Name', { exact: true }).fill('Personal API');
  await editor.getByLabel('API format').selectOption('openai_responses');
  await editor.getByLabel('Base URL').fill('https://api.example.test/v1');
  await editor.getByLabel('API key').fill('synthetic-key');
  await editor.getByRole('button', { name: 'Find models' }).click();
  await editor
    .getByLabel('Choose a discovered model')
    .selectOption('test-model');
  await expect(editor.getByLabel('Default model')).toHaveValue('test-model');
  await expectFormActionsInline(page, editor);
  await page.screenshot({ path: testInfo.outputPath('connection-form.png') });
  await editor.getByRole('button', { name: 'Save connection' }).click();
  await expect(editor).toBeHidden();
  await expect(
    page.getByText('Personal API saved. Nothing was applied.'),
  ).toBeVisible();
  expect(writes.filter((w) => w.includes('/apply'))).toEqual([]);

  const personal = page.getByRole('listitem', { name: 'Personal API' });
  const applyButton = personal.getByRole('button', {
    name: 'Apply Personal API to OpenAI Codex',
  });
  // The harness is the page context: the row button says only "Apply".
  await expect(applyButton).toHaveText('Apply');
  await applyButton.click();
  const apply = page.getByRole('dialog', {
    name: 'Apply Personal API to OpenAI Codex',
  });
  await expect(apply.getByLabel('Default model')).toHaveValue('test-model');
  await apply.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(apply).toBeHidden();
  await expect(binding).toContainText('Personal API');
  await expect(personal.getByText('In use')).toBeVisible();
  await expect(
    personal.getByRole('button', {
      name: 'Reapply Personal API to OpenAI Codex',
    }),
  ).toBeEnabled();

  const team = page.getByRole('listitem', { name: 'Team Gemini' });
  await expect(team.getByText('Gemini · On Laptop')).toBeVisible();
  await expect(team.getByRole('button', { name: /^(Re)?apply/i })).toHaveCount(
    0,
  );
  await page.screenshot({
    path: testInfo.outputPath('settings-upstreams.png'),
  });
  // Sync itself is an encrypted relay request; it is covered by component tests
  // and the integrated relay run. Here it must stay separate from Apply.
  await expect(
    team.getByRole('button', { name: 'Sync Team Gemini to this device' }),
  ).toBeEnabled();
  expect(writes.filter((w) => w.includes('/apply'))).toHaveLength(1);
  await expectActionsInside(page, personal);
  await expectActionsInside(page, team);
  // Edit lives in the More actions menu: by touch on phones, by keyboard on desktop.
  const more = page.getByRole('button', {
    name: 'More actions for Personal API',
  });
  if (testInfo.project.name === 'mobile-chromium') {
    await more.tap();
    await page.getByRole('button', { name: 'Edit Personal API' }).tap();
  } else {
    await more.focus();
    await page.keyboard.press('Enter');
    await page.keyboard.press('Tab');
    await expect(
      page.getByRole('button', { name: 'Edit Personal API' }),
    ).toBeFocused();
    await page.keyboard.press('Enter');
  }
  const edit = page.getByRole('dialog', { name: 'Edit connection' });
  await expect(edit.getByLabel('Name', { exact: true })).toHaveValue(
    'Personal API',
  );
  await edit.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(edit).toBeHidden();

  await page
    .getByRole('button', { name: 'More actions for Personal API' })
    .click();
  await page
    .getByRole('button', { name: 'Remove from this device', exact: true })
    .click();
  const removal = page.getByRole('dialog', {
    name: 'Remove connection from this device?',
  });
  await expect(removal).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(removal).toBeHidden();
  await expect(page.getByTestId('settingsDialog')).toBeVisible();
  await page
    .getByRole('button', { name: 'More actions for Personal API' })
    .click();
  await page
    .getByRole('button', { name: 'Remove from this device', exact: true })
    .click();
  await removal
    .getByRole('button', { name: 'Remove from this device', exact: true })
    .click();
  await expect(removal.getByRole('alert')).toContainText(
    'A harness or plugin still uses this connection',
  );
  await removal.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(removal).toBeHidden();
  await page.getByRole('button', { name: 'Grok Build', exact: true }).click();
  await expect(
    personal.getByRole('button', {
      name: 'Apply Personal API to Grok Build',
    }),
  ).toBeEnabled();
  await page.getByRole('tab', { name: 'Device', exact: true }).click();
  await page
    .getByRole('button', { name: 'Import template', exact: true })
    .click();
  const preview = page.getByRole('dialog', { name: 'Import device template' });
  await preview.getByRole('button', { name: 'Preview template' }).click();
  await expect(preview.getByText('Install if missing: codex')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('template-preview.png') });
  const box = await preview.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(
    page.viewportSize()!.width + 1,
  );
  await preview.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(preview).toBeHidden();
  await page
    .getByTestId('settingsDialog')
    .getByRole('button', { name: 'Close', exact: true })
    .click();
  await expect(
    page.getByRole('button', { name: 'Open Navigation' }),
  ).toBeEnabled();

  // Reopening Settings in zh-CN keeps the short labels and one-row footer, with
  // the widest row state (unapplied changes plus Reapply); phones also at 320.
  bindings = {
    codex: { connectionId: 'personal', model: 'test-model', revision: 'r0' },
  };
  await page.evaluate(() =>
    localStorage.setItem('remote-codex.locale', 'zh-CN'),
  );
  await page.reload();
  // Count main-frame loads after this reload: a dev-server HMR reload (a watched
  // file changed on disk) would silently close the dialog and look like a UI bug.
  const loads: string[] = [];
  page.on('load', () => loads.push(page.url()));
  // The tour was dismissed in this browser, so the shell is ready once its
  // navigation button is; reopen Settings and wait for it before using tabs.
  await page.getByRole('button', { name: '打开导航' }).click();
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const zhSettings = page.getByTestId('settingsDialog');
  await expect(zhSettings).toBeVisible();
  await zhSettings.getByRole('tab', { name: '上游', exact: true }).click();
  await expect(
    zhSettings.getByRole('tab', { name: '上游', exact: true }),
  ).toHaveAttribute('aria-selected', 'true');
  const zhRow = page.getByRole('listitem', { name: 'Personal API' });
  // One compact status, shown once.
  await expect(zhRow.getByText('待应用')).toBeVisible();
  await expect(page.getByText('待应用')).toHaveCount(1);
  await expect(
    zhRow.getByRole('button', {
      name: '将 Personal API 重新应用到 OpenAI Codex',
    }),
  ).toHaveText('重新应用');
  const widths =
    testInfo.project.name === 'mobile-chromium'
      ? [393, 320]
      : [page.viewportSize()!.width];
  for (const width of widths) {
    await page.setViewportSize({ width, height: page.viewportSize()!.height });
    await expectActionsInside(page, zhRow);
    await expectActionsInside(
      page,
      page.getByRole('listitem', { name: 'Team Gemini' }),
    );
    await page.screenshot({
      path: testInfo.outputPath(`settings-upstreams-zh-${width}.png`),
    });
    await page.getByRole('button', { name: '添加连接', exact: true }).click();
    const zhAdd = page.getByRole('dialog', { name: '添加连接' });
    await expectFormActionsInline(page, zhAdd);
    await page.screenshot({
      path: testInfo.outputPath(`connection-form-zh-${width}.png`),
    });
    await zhAdd.getByRole('button', { name: '取消', exact: true }).click();
    await expect(zhAdd).toBeHidden();
  }
  await page.getByRole('button', { name: 'Personal API 的更多操作' }).click();
  await page.getByRole('button', { name: '编辑 Personal API' }).click();
  await expect(page.getByRole('dialog', { name: '编辑连接' })).toBeVisible();
  expect(loads, 'the page reloaded while reopening Settings').toEqual([]);
});

test('thread settings categories and nested dialogs stay usable on narrow screens', async ({
  page,
  request,
}, testInfo) => {
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (
    await request.post(`${base}/api/workspaces`, {
      data: { absPath, label: 'Settings regression' },
    })
  ).json();
  const response = await request.post(`${base}/api/threads/start`, {
    data: {
      workspaceId: workspace.id,
      provider: 'acp',
      agentId: 'codex',
      model: 'ios-e2e-stream',
      approvalMode: 'yolo',
    },
  });
  expect(response.ok()).toBeTruthy();
  const thread = await response.json();
  await page.route('**/api/management/**', (route) =>
    route.fulfill({
      json: route.request().url().endsWith('harnesses')
        ? [{ id: 'codex', name: 'OpenAI Codex', base: { installed: true } }]
        : { profiles: [], active: {}, backups: [] },
    }),
  );
  await page.goto(`/threads/${thread.id}`);
  await page
    .getByRole('button', { name: 'Open settings', exact: true })
    .click();
  const settings = page.getByTestId('settings-dialog');
  await expect(
    settings.getByRole('tab', { name: 'Session', exact: true }),
  ).toBeVisible();
  await settings.getByRole('tab', { name: 'Upstreams', exact: true }).click();
  await settings
    .getByRole('button', { name: 'Add upstream', exact: true })
    .click();
  const editor = page.getByRole('dialog', {
    name: 'Add upstream',
    exact: true,
  });
  await expect(editor.getByLabel('Base URL')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(editor).toBeHidden();
  await expect(settings).toBeVisible();
  await settings.getByRole('tab', { name: 'Session', exact: true }).click();
  await expect(
    settings.getByText('Session details', { exact: true }),
  ).toBeVisible();
  await settings.getByRole('tab', { name: 'Harnesses', exact: true }).click();
  const box = await settings.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(
    page.viewportSize()!.width + 1,
  );
  expect(
    await settings
      .locator('.settings-panel')
      .evaluate((e) => e.scrollWidth <= e.clientWidth + 1),
  ).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('thread-settings.png') });
  await page.route('**/api/agent-runtimes', async (route) => {
    const response = await route.fetch();
    const backends = await response.json();
    await route.fulfill({
      json: backends.map((b: any) =>
        b.provider !== 'codex'
          ? b
          : {
              ...b,
              capabilities: {
                ...b.capabilities,
                management: {
                  ...b.capabilities.management,
                  hostConfigFiles: true,
                },
              },
              managementSchema: {
                ...b.managementSchema,
                hostConfigFiles: [
                  {
                    name: 'config.toml',
                    label: 'Native configuration',
                    description: 'Codex settings',
                  },
                ],
                configArchives: false,
              },
            },
      ),
    });
  });
  let saved = '';
  await page.route(
    '**/api/config/providers/codex/files/config.toml',
    (route) => {
      if (route.request().method() === 'PATCH')
        saved = route.request().postDataJSON().content;
      return route.fulfill({
        json: {
          path: '/isolated/config.toml',
          exists: true,
          content: saved || '# original',
        },
      });
    },
  );
  await settings.getByRole('tab', { name: 'Advanced', exact: true }).click();
  await settings.getByRole('button', { name: /Native configuration/ }).click();
  const nativeEditor = page.getByRole('dialog', {
    name: 'config.toml',
    exact: true,
  });
  await nativeEditor.getByLabel('Edit config.toml').fill('# updated');
  await nativeEditor
    .getByRole('button', { name: 'Save file', exact: true })
    .click();
  await expect(nativeEditor.getByRole('status')).toHaveText('Saved');
  expect(saved).toBe('# updated');
  await page.keyboard.press('Escape');
  await expect(nativeEditor).toBeHidden();
  await expect(settings).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toBeHidden();
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
});
