import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DeviceMetricsDto, HarnessProcessesDto, LoadedThreadDto } from '@pockymoe/shared';
import { AppShellNavContext, type AppShellNavContextValue } from './AppShellNavContext';
import { DeviceMonitor } from './DeviceMonitor';
import { harnessState } from './harnessResourceModel';
import { ApiError, fetchHarnessProcesses, releaseThread, request } from '../lib/api';
import { announceHarnessRuntimeEvent } from '../lib/harnessRuntimeEvents';

vi.mock('../lib/api', () => {
  class ApiError extends Error {
    constructor(public statusCode: number, public payload: { code?: string; message: string } = { message: 'failed' }) {
      super(payload.message);
    }
  }
  return { ApiError, request: vi.fn(), fetchHarnessProcesses: vi.fn(), releaseThread: vi.fn() };
});

const GIB = 2 ** 30;
const metrics: DeviceMetricsDto = {
  sampledAt: new Date().toISOString(),
  sampleWindowMs: 1000,
  platform: 'linux',
  environment: 'native',
  cpu: { model: 'Test CPU', usagePercent: 10, logicalCoreCount: 1, cores: [{ index: 0, usagePercent: 10 }] },
  memory: { totalBytes: 8 * GIB, usedBytes: 4 * GIB, availableBytes: 4 * GIB, usagePercent: 50 },
  swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null },
  cpuPower: { watts: null, source: null, reason: null },
  gpus: [],
  hardwareSampledAt: new Date().toISOString(),
  hardwareNotes: [],
  limits: null,
};
const thread = (patch: Partial<LoadedThreadDto>): LoadedThreadDto => ({
  threadId: 'idle-id',
  title: 'Idle thread',
  workspaceId: 'w',
  provider: 'claude',
  agentId: null,
  loadReason: 'view',
  loadedAt: new Date(Date.now() - 20 * 60_000).toISOString(),
  lastActivityAt: new Date(Date.now() - 12 * 60_000).toISOString(),
  displayed: false,
  busy: [],
  sharesProcessWith: [],
  rootPid: 10,
  bytes: 300 * 2 ** 20,
  processes: [
    { pid: 10, role: 'adapter', name: 'claude-agent-acp', bytes: 60 * 2 ** 20 },
    { pid: 11, role: 'mcp', name: 'bitget-agent-mcp', bytes: 65 * 2 ** 20 },
  ],
  ...patch,
});
const processes = (threads: LoadedThreadDto[] = [thread({}), thread({ threadId: 'busy-id', title: 'Busy thread', provider: 'codex', busy: ['turn'] })]): HarnessProcessesDto => ({
  sampledAt: new Date().toISOString(),
  metric: 'pss',
  supervisorPid: 1,
  supervisorBytes: 80 * 2 ** 20,
  harnessBytes: 600 * 2 ** 20,
  totalBytes: 680 * 2 ** 20,
  settings: { idleReleaseMinutes: 10, maxLoadedThreads: null, maxHarnessMemoryBytes: null },
  overThreadLimit: false,
  overMemoryLimit: false,
  threads,
  other: [],
});

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.mocked(request).mockReset().mockResolvedValue(metrics);
  vi.mocked(fetchHarnessProcesses).mockReset().mockResolvedValue(processes());
  vi.mocked(releaseThread).mockReset().mockResolvedValue({ threadId: 'x', released: true, alsoReleased: [] });
});
afterEach(() => {
  vi.useRealTimers();
});

async function openMonitor(nav?: Partial<AppShellNavContextValue>) {
  render(
    <AppShellNavContext.Provider value={(nav ?? null) as AppShellNavContextValue | null}>
      <DeviceMonitor />
    </AppShellNavContext.Provider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Device monitor' }));
  return screen.findByRole('button', { name: /^Memory/ });
}

it('requests the breakdown only while the memory row is expanded, every 3 s and on runtime events', async () => {
  const memoryRow = await openMonitor();
  expect(fetchHarnessProcesses).not.toHaveBeenCalled();
  expect(memoryRow).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(memoryRow);
  expect(await screen.findByText('Supervisor 680 MiB')).toBeVisible();
  expect(screen.getByText(/harnesses 600 MiB/)).toBeVisible();
  expect(screen.getByRole('img', { name: /^PSS/ })).toBeInTheDocument();
  expect(fetchHarnessProcesses).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(3_000));
  expect(fetchHarnessProcesses).toHaveBeenCalledTimes(2);
  act(() => announceHarnessRuntimeEvent({ type: 'thread.runtime.released', threadId: 'x', timestamp: '', payload: { reason: 'idle' } } as never));
  await waitFor(() => expect(fetchHarnessProcesses).toHaveBeenCalledTimes(3));
  // Other socket events do not refresh the list.
  act(() => announceHarnessRuntimeEvent({ type: 'thread.updated', threadId: 'x', timestamp: '', payload: {} } as never));
  fireEvent.click(memoryRow);
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(fetchHarnessProcesses).toHaveBeenCalledTimes(3);
});

it('shows each loaded thread with its state and per-process memory', async () => {
  fireEvent.click(await openMonitor());
  const idle = (await screen.findByText('Idle thread')).closest('li')!;
  expect(within(idle).getByText('Idle 12 min')).toBeVisible();
  expect(within(idle).getByText('300 MiB')).toBeVisible();
  expect(within(idle).queryByText('bitget-agent-mcp')).toBeNull();
  fireEvent.click(within(idle).getByRole('button', { name: 'Processes of Idle thread' }));
  expect(within(idle).getByText('bitget-agent-mcp')).toBeVisible();
  expect(within(idle).getByText('MCP')).toBeVisible();
  const busy = screen.getByText('Busy thread').closest('li')!;
  expect(within(busy).getByText('Running')).toBeVisible();
});

it('ends an idle thread directly and asks before interrupting a busy one', async () => {
  fireEvent.click(await openMonitor());
  fireEvent.click(await screen.findByRole('button', { name: 'End Idle thread' }));
  await waitFor(() => expect(releaseThread).toHaveBeenCalledWith('idle-id', {}));
  expect(screen.queryByText('Interrupt and end?')).toBeNull();

  fireEvent.click(screen.getByRole('button', { name: 'End Busy thread' }));
  const dialog = await screen.findByRole('dialog', { name: 'Interrupt and end?' });
  expect(releaseThread).toHaveBeenCalledTimes(1);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(await screen.findByText('Busy thread')).toBeVisible();
  expect(releaseThread).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole('button', { name: 'End Busy thread' }));
  fireEvent.click(within(await screen.findByRole('dialog', { name: 'Interrupt and end?' })).getByRole('button', { name: 'End' }));
  await waitFor(() => expect(releaseThread).toHaveBeenLastCalledWith('busy-id', { interrupt: true }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Interrupt and end?' })).toBeNull());
});

it('asks for confirmation when an idle-looking thread started a turn meanwhile', async () => {
  vi.mocked(releaseThread).mockRejectedValueOnce(new ApiError(409, { code: 'turn_running', message: 'A turn is running' } as never));
  fireEvent.click(await openMonitor());
  fireEvent.click(await screen.findByRole('button', { name: 'End Idle thread' }));
  expect(await screen.findByRole('dialog', { name: 'Interrupt and end?' })).toBeVisible();
});

it('keeps the memory row a plain reading on a Supervisor without the breakdown', async () => {
  vi.mocked(fetchHarnessProcesses).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' } as never));
  fireEvent.click(await openMonitor());
  await waitFor(() => expect(screen.queryByRole('button', { name: /^Memory/ })).toBeNull());
  expect(screen.getByText('Memory')).toBeVisible();
  expect(screen.queryByRole('alert')).toBeNull();
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(fetchHarnessProcesses).toHaveBeenCalledTimes(1);
});

it('opens the resources settings from the gear', async () => {
  const openSettingsSection = vi.fn();
  fireEvent.click(await openMonitor({ openSettingsSection }));
  fireEvent.click(await screen.findByRole('button', { name: 'Resource settings' }));
  expect(openSettingsSection).toHaveBeenCalledWith('resources');
  await waitFor(() => expect(screen.queryByText('Supervisor 680 MiB')).toBeNull());
});

it('labels busy, viewed and recently active threads', () => {
  const now = Date.now();
  expect(harnessState(thread({ busy: ['pendingRequest'] }), now)).toBe('Awaiting reply');
  expect(harnessState(thread({ busy: ['goal', 'turn'] }), now)).toBe('Running');
  expect(harnessState(thread({ displayed: true }), now)).toBe('Viewing');
  expect(harnessState(thread({ lastActivityAt: new Date(now - 30_000).toISOString() }), now)).toBe('Idle');
});
