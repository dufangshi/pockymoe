import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test('device monitor shows cores, memory and real watts, and marks failed samples stale', async ({
  page,
  request,
}, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (
    await request.post(`${base}/api/workspaces`, {
      data: { absPath, label: 'Monitor regression' },
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
  let fail = false;
  await page.route('**/api/device/metrics', (route) =>
    fail
      ? route.fulfill({ status: 503, json: { message: 'Offline' } })
      : route.fulfill({
          json: {
            sampledAt: new Date().toISOString(),
            sampleWindowMs: 2000,
            platform: 'linux',
            environment: 'wsl',
            cpu: {
              model: 'Test CPU',
              usagePercent: 37,
              logicalCoreCount: 4,
              cores: [0, 20, 60, 68].map((usagePercent, index) => ({
                index,
                usagePercent,
              })),
            },
            memory: {
              totalBytes: 16 * 2 ** 30,
              usedBytes: 8 * 2 ** 30,
              availableBytes: 8 * 2 ** 30,
              usagePercent: 50,
            },
            swap: {
              totalBytes: 0,
              usedBytes: 0,
              availableBytes: 0,
              usagePercent: null,
            },
            cpuPower: { watts: 24.5, source: 'CPU package', reason: null },
            cpuTemperature: {
              celsius: 97.5, source: 'Linux hwmon', reason: null,
              sensors: [
                { label: 'Package id 0', celsius: 97.5 },
                { label: 'Core 0', celsius: 94 },
              ],
            },
            gpus: [
              {
                id: 'gpu-1',
                name: 'Test GPU',
                usagePercent: 12,
                usedMemoryBytes: 2 ** 30,
                totalMemoryBytes: 8 * 2 ** 30,
                power: { watts: null, source: null, reason: 'GPU power sensor is not exposed by this driver' },
                source: 'test',
              },
            ],
            hardwareSampledAt: new Date().toISOString(),
            hardwareNotes: [],
            limits: null,
          },
        }),
  );
  await page.goto(`/threads/${thread.id ?? thread.thread.id}`);
  const monitor = page.getByRole('button', {
    name: 'Device monitor',
    exact: true,
  });
  await expect(monitor).toContainText('CPU 37%');
  await expect(monitor).toContainText('RAM 50%');
  const mobile = testInfo.project.name === 'mobile-chromium';
  await expect(
    mobile
      ? page
          .locator('.matter-topbar')
          .getByRole('button', { name: 'Device monitor' })
      : page
          .locator('.matter-rail-bottom')
          .getByRole('button', { name: 'Device monitor' }),
  ).toBeVisible();
  if (!mobile) {
    await page.getByRole('button', { name: 'Toggle shortcuts sidebar' }).click();
    await expect(monitor).toBeVisible();
    const settings = page.locator('.matter-rail-bottom').getByRole('button', { name: 'Open settings' });
    await expect(settings).toBeVisible();
    const monitorBox = await monitor.boundingBox();
    const settingsBox = await settings.boundingBox();
    expect(monitorBox!.y + monitorBox!.height).toBeLessThanOrEqual(settingsBox!.y);
  }
  await monitor.click();
  const dialog = page.getByRole('dialog', { name: 'Device monitor' });
  // Freshness, platform and environment fit beside the title; the environment's
  // explanation lives in its tooltip.
  await expect(dialog.locator('.dm-meta')).toContainText(/\ds ago · linux/);
  await expect(dialog.locator('.dm-meta abbr')).toHaveAttribute('title', /WSL environment/);
  await expect(dialog.getByRole('meter', { name: 'CPU utilization' })).toHaveAttribute('aria-valuenow', '37');
  await expect(dialog.getByRole('meter', { name: 'Memory utilization' })).toHaveAttribute('aria-valuenow', '50');
  await expect(dialog).toContainText('8.0 GiB / 16.0 GiB');
  await expect(dialog).toContainText('24.5 W');
  await expect(dialog).toContainText('97.5 °C');
  // Unavailable readings are a small mark whose name carries the reason.
  await expect(dialog.getByRole('img', { name: /GPU power sensor is not exposed by this driver/ })).toBeVisible();
  await expect(dialog).not.toContainText('GPU power sensor is not exposed');
  // Cores, their charts and the temperature sensors open from the CPU row.
  await expect(dialog.locator('.dm-core')).toHaveCount(0);
  const cpu = dialog.getByRole('button', { name: /CPU/ });
  await cpu.click();
  await expect(cpu).toHaveAttribute('aria-expanded', 'true');
  await expect(dialog.locator('.dm-core')).toHaveCount(4);
  await expect(dialog.getByRole('img', { name: 'Core 1 utilization' })).toBeVisible();
  await expect(dialog.locator('.dm-core').first()).toHaveAttribute('title', 'Core 1 · 0%');
  await expect(dialog.locator('.dm-core').nth(3)).toHaveAttribute('style', /--load: 68/);
  await expect(dialog.getByText('Core 0', { exact: true })).not.toBeVisible();
  await dialog.getByText('CPU temperature sensors (2)').click();
  await expect(dialog.getByText('Core 0', { exact: true })).toBeVisible();
  await expect(dialog).toContainText('94.0 °C');
  fail = true;
  await expect(dialog.getByRole('status')).toContainText(
    'Device metrics unavailable',
  );
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(monitor).toHaveAttribute('data-stale', 'true');
  await expect(monitor).toContainText('CPU —');
  if (mobile) await page.setViewportSize({ width: 320, height: 740 });
  const bounds = await monitor.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(
    page.viewportSize()!.width,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await monitor.click();
  await expect(dialog).toBeVisible();
  expect(
    await dialog.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
});
