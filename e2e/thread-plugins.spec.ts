import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

// Per-conversation plugin scope against the real Supervisor API: one conversation
// switches a plugin off without touching another, a globally-off plugin cannot be
// switched on, and its own settings can be customized and restored.

test.use({ actionTimeout: 15_000 });

const base = () => `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function startThread(request: APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (
    await request.post(`${base()}/api/workspaces`, {
      data: { absPath, label: 'Plugin scope' },
    })
  ).json();
  const response = await request.post(`${base()}/api/threads/start`, {
    data: {
      workspaceId: workspace.id,
      provider: 'acp',
      agentId: 'codex',
      model: 'ios-e2e-stream',
      approvalMode: 'yolo',
    },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as { id: string };
}

async function openThread(page: Page, id: string) {
  await page.goto(`/threads/${id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
}

const panelButton = (page: Page) =>
  page.getByRole('button', { name: /^Plugins in this conversation/ });

test('a conversation turns a plugin off for itself only and edits its own settings', async ({
  page,
  request,
}, testInfo) => {
  const first = await startThread(request);
  const second = await startThread(request);
  await openThread(page, first.id);
  await expect(page.getByRole('button', { name: 'Terminal', exact: true })).toBeVisible();
  await panelButton(page).click();
  const panel = page.getByRole('dialog', { name: 'Plugins in this conversation' });
  const terminal = panel.getByRole('listitem', { name: 'Terminal' });
  await expect(terminal.getByText('Active', { exact: true })).toBeVisible();
  await terminal.getByRole('radio', { name: 'Off in this conversation' }).check();
  await expect(terminal.getByText('Not active', { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('thread-plugins-desktop.png') });
  await page.keyboard.press('Escape');
  // The plugin's panel follows this conversation's switch.
  await expect(page.getByRole('button', { name: 'Terminal', exact: true })).toHaveCount(0);

  await page.reload();
  await expect(page.getByRole('button', { name: 'Terminal', exact: true })).toHaveCount(0);
  await panelButton(page).click();
  await expect(
    panel
      .getByRole('listitem', { name: 'Terminal' })
      .getByRole('radio', { name: 'Off in this conversation' }),
  ).toBeChecked();
  // Content Quality is off device-wide: this conversation cannot switch it on.
  const quality = panel.getByRole('listitem', { name: 'Content Quality' });
  await expect(
    quality.getByRole('radio', { name: 'On in this conversation' }),
  ).toBeDisabled();
  await quality.getByRole('button', { name: 'Settings here' }).click();
  const editor = page.getByRole('dialog', {
    name: 'Content Quality · this conversation',
  });
  await expect(
    editor.getByLabel('Auto-polish trigger length (characters)'),
  ).toBeVisible();
  await editor.getByLabel('Reader background').fill('Scoped reader');
  await editor.getByRole('button', { name: 'Save settings' }).click();
  await expect(editor.getByText('Settings saved.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(quality.getByText('Custom settings', { exact: true })).toBeVisible();
  await quality.getByRole('button', { name: 'Restore global settings' }).click();
  await expect(quality.getByText('Custom settings', { exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // Another conversation keeps the global switch.
  await openThread(page, second.id);
  await expect(page.getByRole('button', { name: 'Terminal', exact: true })).toBeVisible();

  await openThread(page, first.id);
  await panelButton(page).click();
  await panel
    .getByRole('listitem', { name: 'Terminal' })
    .getByRole('radio', { name: 'Use the global setting' })
    .check();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Terminal', exact: true })).toBeVisible();
});

test('the conversation plugin panel is a reachable bottom sheet on phones', async ({
  page,
  request,
}, testInfo) => {
  const thread = await startThread(request);
  await openThread(page, thread.id);
  // Phones keep conversation controls in the More sheet.
  await page.getByRole('button', { name: 'More actions', exact: true }).tap();
  const button = panelButton(page);
  await expect(button).toBeVisible();
  const viewport = page.viewportSize()!;
  const buttonBox = (await button.boundingBox())!;
  expect(buttonBox.x).toBeGreaterThanOrEqual(0);
  expect(buttonBox.x + buttonBox.width).toBeLessThanOrEqual(viewport.width + 1);
  await button.tap();
  const panel = page.getByRole('dialog', { name: 'Plugins in this conversation' });
  await expect(panel.getByRole('listitem').first()).toBeVisible();
  const box = (await panel.boundingBox())!;
  // A sheet from the bottom edge, full width (docs/ui-principles.md).
  expect(Math.abs(box.y + box.height - viewport.height)).toBeLessThanOrEqual(1);
  expect(box.width).toBeGreaterThanOrEqual(viewport.width - 1);
  for (const option of await panel.locator('.thread-plugins-choice label').all()) {
    expect((await option.boundingBox())!.height).toBeGreaterThanOrEqual(40);
  }
  await page.screenshot({ path: testInfo.outputPath('thread-plugins-phone.png') });
  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
  await expect(button).toBeFocused();
});
