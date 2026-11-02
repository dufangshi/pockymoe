import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { DeviceMetricsDto, HarnessProcessesDto } from '@pockymoe/shared';
import { DeviceMonitor } from '../../components/DeviceMonitor';
import { request, releaseThread } from '../../lib/api';

vi.mock('../../lib/api', () => {
  class ApiError extends Error { constructor(public statusCode: number) { super('test'); } }
  return { ApiError, request: vi.fn(), releaseThread: vi.fn(), fetchHarnessProcesses: vi.fn() };
});
it('keeps metrics, process inspection and harness release on the selected desktop device', async () => {
  const sampledAt = new Date().toISOString();
  const metrics: DeviceMetricsDto = { sampledAt, sampleWindowMs: 1000, platform: 'linux', environment: 'native', cpu: { model: 'Test', usagePercent: 30, logicalCoreCount: 1, cores: [{ index: 0, usagePercent: 30 }] }, memory: { totalBytes: 8 * 2 ** 30, usedBytes: 4 * 2 ** 30, availableBytes: 4 * 2 ** 30, usagePercent: 50 }, swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null }, cpuPower: { watts: null, source: null, reason: null }, gpus: [], hardwareSampledAt: sampledAt, hardwareNotes: [], limits: null };
  const processes: HarnessProcessesDto = { sampledAt, metric: 'pss', supervisorPid: 1, supervisorBytes: 10, harnessBytes: 20, totalBytes: 30, settings: { idleReleaseMinutes: 10, maxLoadedThreads: null, maxHarnessMemoryBytes: null }, overThreadLimit: false, overMemoryLimit: false, other: [], threads: [{ threadId: 'thread', title: 'Scoped harness', workspaceId: 'w', provider: 'codex', agentId: null, loadReason: 'view', loadedAt: sampledAt, lastActivityAt: sampledAt, displayed: false, busy: [], sharesProcessWith: [], rootPid: 2, bytes: 20, processes: [] }] };
  vi.mocked(request).mockImplementation(async (path: RequestInfo) => String(path).endsWith('/processes') ? processes : metrics);
  vi.mocked(releaseThread).mockResolvedValue({ threadId: 'thread', released: true, alsoReleased: [] });
  const view = render(<DeviceMonitor deviceId="other-device" />);
  await waitFor(() => expect(request).toHaveBeenCalledWith('api/device/metrics', expect.anything(), { deviceId: 'other-device' }));
  fireEvent.click(screen.getByRole('button', { name: 'Device monitor' }));
  fireEvent.click(await screen.findByRole('button', { name: /^Memory/ }));
  expect(await screen.findByText('Scoped harness')).toBeVisible();
  expect(request).toHaveBeenCalledWith('api/device/processes', expect.anything(), { deviceId: 'other-device' });
  fireEvent.click(screen.getByRole('button', { name: 'End Scoped harness' }));
  await waitFor(() => expect(releaseThread).toHaveBeenCalledWith('thread', {}, 'other-device'));
  view.unmount();
});
