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
                ...(index < 3 ? { celsius: 70 + index } : {}),
              })),
            },
            memory: {
              totalBytes: 60.9 * 2 ** 30,
              usedBytes: 25.4 * 2 ** 30,
              availableBytes: 35.5 * 2 ** 30,
              usagePercent: 42,
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
  await expect(monitor.locator('[aria-label^=CPU]')).toHaveText('37%');
  await expect(monitor.locator('[aria-label^=DRAM]')).toHaveText('25.4/60.9G');
  await expect(monitor.locator('[aria-label^=VRAM]')).toHaveText('1.0/8.0G');
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
  const fit = await monitor.evaluate(button => {
    const bounds = button.getBoundingClientRect();
    return [...button.querySelectorAll('.dm-summary-reading')].map(cell => {
      const r = cell.getBoundingClientRect();
      return { overflow: cell.scrollWidth - cell.clientWidth, top: r.top - bounds.top, bottom: bounds.bottom - r.bottom };
    });
  });
  for (const cell of fit) {
    expect(cell.overflow).toBeLessThanOrEqual(0);
    expect(cell.top).toBeGreaterThanOrEqual(0);
    expect(cell.bottom).toBeGreaterThanOrEqual(0);
  }
  await monitor.click();
  const dialog = page.getByRole('dialog', { name: 'Device monitor' });
  // Freshness, platform and environment fit beside the title; the environment's
  // explanation lives in its tooltip.
  await expect(dialog.locator('.dm-meta')).toContainText(/\ds ago · linux/);
  await expect(dialog.locator('.dm-meta abbr')).toHaveAttribute('title', /WSL environment/);
  await expect(dialog.getByRole('meter', { name: 'CPU utilization' })).toHaveAttribute('aria-valuenow', '37');
  await expect(dialog.getByRole('meter', { name: 'Memory utilization' })).toHaveAttribute('aria-valuenow', '42');
  await expect(dialog).toContainText('25.4 GiB / 60.9 GiB');
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
  // Each core chart carries its physical core's temperature, so the sensor list
  // is left out; the last core has no reading.
  await expect(dialog.locator('.dm-core').first()).toHaveAttribute('title', 'Core 1 · 0% · 70 °C');
  await expect(dialog.locator('.dm-core').first().locator('.dm-core-temp')).toHaveText('70°');
  await expect(dialog.locator('.dm-core').nth(3)).toHaveAttribute('title', 'Core 4 · 68%');
  await expect(dialog.locator('.dm-core').nth(3)).toHaveAttribute('style', /--load: 68/);
  await expect(dialog.getByText('CPU temperature sensors (2)')).toHaveCount(0);
  // The usage chart has a 0–100% scale and labels the current reading.
  await expect(dialog.locator('.dm-axis')).toContainText('100%');
  await expect(dialog.locator('.dm-now')).toHaveText('37%');
  fail = true;
  await expect(dialog.getByRole('status')).toContainText(
    'Device metrics unavailable',
  );
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(monitor).toHaveAttribute('data-stale', 'true');
  await expect(monitor.locator('[aria-label^=CPU]')).toHaveText('—');
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
