import { selectControl } from './selectControl';
import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });

test('Upstreams tab filters installations and manages existing profiles without leaving settings', async ({
  page,
}, testInfo) => {
  const mobile = testInfo.project.name === 'mobile-chromium';
  await page.setViewportSize(
    mobile ? { width: 390, height: 844 } : { width: 1440, height: 1000 },
  );
  let profiles = [
    {
      id: 'work',
      name: 'Work gateway',
      harness: 'codex',
      baseUrl: 'https://gateway.example.test/v1',
      model: 'gpt-work',
      apiType: 'responses',
      contextWindow: 100000,
      hasApiKey: true,
    },
    {
      id: 'personal',
      name: 'Personal provider',
      harness: 'codex',
      baseUrl: 'https://api.example.test/v1',
      model: 'gpt-personal',
      apiType: 'responses',
      contextWindow: 100000,
      hasApiKey: true,
    },
    {
      id: 'claude',
      name: 'Claude gateway',
      harness: 'claude',
      baseUrl: 'https://claude.example.test',
      model: 'claude-test',
      apiType: 'responses',
      contextWindow: 100000,
      hasApiKey: true,
    },
  ];
  const active: Record<string, string> = { codex: 'work' };
  const actions: string[] = [];
  let conflictNext = false;
  await page.route('**/api/management/**', async (route) => {
    const p = new URL(route.request().url()).pathname;
    const body =
      route.request().method() === 'POST'
        ? route.request().postDataJSON()
        : null;
    let result: unknown = {};
    if (p.endsWith('/harnesses'))
      result = [
        { id: 'codex', name: 'OpenAI Codex', base: { installed: true } },
        {
          id: 'claude',
          name: 'Claude Agent',
          base: { path: '/isolated/claude' },
        },
        { id: 'gemini', name: 'Gemini CLI', base: { installed: false } },
        {
          id: 'deepseek',
          name: 'DeepSeek Harness',
          base: null,
          adapter: { installed: true },
        },
      ];
    else if (p.endsWith('/upstreams/live')) {
      const imported = {
        ...profiles[0]!,
        id: 'live',
        name: 'Imported live provider',
      };
      profiles.push(imported);
      result = imported;
    } else if (p.endsWith('/upstreams/models'))
      result = {
        models: [{ id: 'gpt-personal', name: 'gpt-personal' }],
        truncated: false,
      };
    else if (p.endsWith('/upstreams')) {
      if (body) {
        const { apiKey, ...saved } = body;
        if (conflictNext) {
          conflictNext = false;
          await route.fulfill({
            status: 409,
            json: {
              code: 'edit_conflict',
              message: 'Native configuration changed: reload before applying.',
            },
          });
          return;
        }
        profiles = profiles.filter((p) => p.id !== saved.id);
        profiles.push({ ...saved, id: saved.id || 'new', hasApiKey: true });
        result = profiles.at(-1);
      } else
        result = {
          profiles,
          active,
          backups: [
            {
              id: 'backup',
              harness: 'codex',
              createdAt: '2026-10-09T00:00:00Z',
            },
          ],
        };
    } else if (p.includes('/upstreams/')) {
      const id = p.split('/').at(-1)!;
      if (route.request().method() === 'DELETE')
        profiles = profiles.filter((v) => v.id !== id);
      else {
        actions.push(`${id}:${body.action}`);
        if (body.action === 'activate') active.codex = id;
        if (body.action === 'restore') delete active.codex;
        if (body.action === 'test') result = { latencyMs: 12 };
        if (body.action === 'speed') result = { latencyMs: 8, status: 200 };
        if (body.action === 'edit')
          result = {
            ...profiles.find((p) => p.id === id),
            revision: 'stored-revision',
            liveRevision: 'live-revision',
          };
        if (body.action === 'up' || body.action === 'down') {
          const index = profiles.findIndex((p) => p.id === id),
            other = body.action === 'up' ? index - 1 : index + 1;
          [profiles[index], profiles[other]] = [
            profiles[other]!,
            profiles[index]!,
          ];
        }
      }
    }
    await route.fulfill({ json: result });
  });
  await page.goto('/workspaces');
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByTestId('settingsDialog');
  await settings.getByRole('tab', { name: 'Upstreams', exact: true }).click();
  await expect(
    settings.getByRole('tab', { name: 'Upstreams', exact: true }),
  ).toHaveAttribute('aria-selected', 'true');
  const chooser = settings.getByRole('group', { name: 'Choose harness' });
  await expect(chooser.getByRole('button')).toHaveCount(2);
  await expect(
    chooser.getByRole('button', { name: 'OpenAI Codex', exact: true }),
  ).toHaveAttribute('aria-pressed', 'true');
  await expect(
    settings.getByText('Work gateway', { exact: true }),
  ).toBeVisible();
  await expect(
    settings.getByText('Claude gateway', { exact: true }),
  ).toHaveCount(0);
  const personal = settings
    .getByRole('article')
    .filter({ hasText: 'Personal provider' });
  await personal.getByRole('button', { name: 'Enable', exact: true }).click();
  await expect(
    personal.getByRole('button', { name: 'Using', exact: true }),
  ).toBeVisible();
  expect(actions).toContain('personal:activate');
  await settings
    .getByRole('searchbox', { name: 'Search upstreams' })
    .fill('work');
  await expect(settings.getByRole('article')).toHaveCount(1);
  await settings.getByRole('searchbox').fill('no-match');
  await expect(
    settings.getByText('No upstreams match your search.'),
  ).toBeVisible();
  await settings.getByRole('searchbox').fill('');
  const panel = settings.locator('.settings-panel');
  expect(await panel.evaluate((e) => e.scrollWidth <= e.clientWidth + 1)).toBe(
    true,
  );
  await settings.getByRole('searchbox').blur();
  await panel.evaluate((e) => {
    e.scrollTop = 0;
  });
  const shots = path.resolve('.local/upstreams-screenshots');
  await mkdir(shots, { recursive: true });
  await page.screenshot({
    path: path.join(shots, `${mobile ? 'mobile' : 'desktop'}-list.png`),
  });
  await personal.getByLabel('More actions for Personal provider').click();
  await expect(personal.getByRole('button', { name: 'Move up' })).toBeVisible();
  await settings.getByRole('searchbox').click();
  await expect(personal.getByRole('button', { name: 'Move up' })).toHaveCount(
    0,
  );
  await personal.getByLabel('More actions for Personal provider').click();
  await personal.getByRole('button', { name: 'Move up' }).click();
  await expect(settings.getByRole('article').first()).toContainText(
    'Personal provider',
  );
  await personal.getByLabel('More actions for Personal provider').click();
  await personal.getByRole('button', { name: 'Endpoint speed test' }).click();
  await expect(settings.getByRole('status')).toContainText('8 ms (HTTP 200)');
  await personal.getByLabel('More actions for Personal provider').click();
  await personal.getByRole('button', { name: 'Test connection' }).click();
  await expect(settings.getByRole('status')).toContainText(
    'connection succeeded',
  );
  expect(actions).toContain('personal:test');
  await personal.getByLabel('More actions for Personal provider').click();
  await personal
    .getByRole('button', { name: 'Duplicate Personal provider' })
    .click();
  const editor = page.getByRole('dialog', {
    name: 'Add upstream',
    exact: true,
  });
  await expect(editor.getByLabel('API key')).toHaveValue('');
  await expect(editor.getByLabel('Harness').locator('option')).toHaveCount(1);
  await editor.getByLabel('Name', { exact: true }).fill('New provider');
  await editor
    .getByLabel('API key', { exact: true })
    .fill('synthetic-test-key');
  await editor
    .getByLabel('Model ID', { exact: true })
    .fill('manual-model-not-in-discovery');
  await editor.getByRole('button', { name: 'Save upstream' }).click();
  await expect(editor).toBeHidden();
  await expect(
    settings.getByText('New provider', { exact: true }),
  ).toBeVisible();
  await settings
    .getByRole('button', { name: 'Import current configuration', exact: true })
    .click();
  await expect(
    settings.getByText('Imported live provider', { exact: true }),
  ).toBeVisible();
  await chooser
    .getByRole('button', { name: 'Claude Agent', exact: true })
    .click();
  await expect(
    settings.getByText('Claude gateway', { exact: true }),
  ).toBeVisible();
  await expect(settings.getByText('New provider', { exact: true })).toHaveCount(
    0,
  );
  await settings
    .getByRole('article')
    .filter({ hasText: 'Claude gateway' })
    .getByRole('button', { name: 'Edit', exact: true })
    .click();
  const claudeForm = page.getByRole('dialog', {
    name: 'Edit upstream',
    exact: true,
  });
  if (mobile)
    await page.screenshot({ path: path.join(shots, 'mobile-form-top.png') });
  await claudeForm.getByLabel('Haiku model').fill('gateway-haiku');
  await claudeForm.getByLabel('Sonnet model').fill('gateway-sonnet');
  await claudeForm.getByLabel('Opus model').fill('gateway-opus');
  await claudeForm
    .getByLabel('Custom request headers')
    .fill('X-Gateway-Region: staging');
  await page.screenshot({
    path: path.join(shots, `${mobile ? 'mobile' : 'desktop'}-form.png`),
  });
  conflictNext = true;
  await claudeForm.getByRole('button', { name: 'Save upstream' }).click();
  const conflict = page.getByRole('dialog', {
    name: 'Configuration changed',
    exact: true,
  });
  await expect(conflict).toBeVisible();
  await conflict
    .getByRole('button', { name: 'Reload current configuration' })
    .click();
  await expect(claudeForm).toBeVisible();
  await expect(claudeForm.getByLabel('Haiku model')).toHaveValue('');
  await page.keyboard.press('Escape');
  await settings.getByRole('checkbox', { name: 'Show uninstalled' }).check();
  await expect(chooser.getByRole('button')).toHaveCount(4);
  await chooser.getByRole('button', { name: 'DeepSeek Harness' }).click();
  await settings.getByRole('button', { name: 'Add upstream' }).click();
  const dsh = page.getByRole('dialog', { name: 'Add upstream', exact: true });
  await selectControl(page, dsh.getByLabel('Provider preset'), 'dsh-official');
  await expect(dsh.getByLabel('Base URL')).toHaveValue(
    'https://api.deepseek.com/anthropic',
  );
  await expect(dsh.getByLabel('API format')).toHaveValue('anthropic');
  await selectControl(page, dsh.getByLabel('Provider preset'), 'dsh-compatible');
  await expect(dsh.getByLabel('API format')).toHaveValue('chat_completions');
  await page.keyboard.press('Escape');
  await settings.getByRole('checkbox', { name: 'Show uninstalled' }).uncheck();
  await expect(chooser.getByRole('button')).toHaveCount(2);
  await chooser
    .getByRole('button', { name: 'OpenAI Codex', exact: true })
    .click();
  await settings.getByText('Configuration backups', { exact: true }).click();
  await settings.getByRole('button', { name: 'Restore previous' }).click();
  await expect(
    settings
      .getByRole('article')
      .getByRole('button', { name: 'Using', exact: true }),
  ).toHaveCount(0);
  expect(actions).toContain('backup:restore');
});
