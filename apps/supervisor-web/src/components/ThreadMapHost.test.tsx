import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { relayModeActive, request } from '../lib/api';
import { mapDevice, useThreadMapDevices } from './ThreadMapHost';

vi.mock('../lib/api', () => ({ request: vi.fn(), relayModeActive: vi.fn(() => true) }));

const thread = (id: string, extra: Partial<ThreadDto> = {}): ThreadDto => ({
  id, workspaceId: 'w1', provider: 'codex', title: id, status: 'idle', ...extra,
}) as ThreadDto;
const workspaces = [{ id: 'w1', label: 'app', absPath: '/src/app' }] as WorkspaceDto[];

describe('mapDevice', () => {
  it('folds agent threads into their root and sums their activity', () => {
    const device = mapDevice('laptop', 'Laptop', workspaces, [
      thread('root', { agentId: 'claude-code', model: 'opus', createdAt: '2026-10-01T00:00:00Z', lastTurnCompletedAt: '2026-10-15T08:00:00Z' }),
      thread('child', { parentThreadId: 'root', rootThreadId: 'root', status: 'running', lastTurnStartedAt: '2026-10-16T09:00:00Z' }),
      thread('grandchild', { parentThreadId: 'child', rootThreadId: 'root' }),
      thread('orphan', { parentThreadId: 'gone', rootThreadId: 'gone', workspaceId: 'w2' }),
    ], { since: '', hours: 24, threads: [
      { threadId: 'root', turns: 10, activeMs: 60_000, totalMs: 600_000, hourlyMs: [0, 60_000], lastPrompt: 'ship it',
        tokenUsage: { totalTokens: 1000, inputTokens: 900, cachedInputTokens: 500, outputTokens: 100, reasoningOutputTokens: 0 },
        priceEstimate: { currency: 'USD', totalUsd: 1, inputUsd: 0.5, cachedInputUsd: 0.25, outputUsd: 0.25 } },
      { threadId: 'child', turns: 3, activeMs: 120_000, totalMs: 120_000, hourlyMs: [120_000, 0], lastPrompt: 'agent prompt',
        tokenUsage: { totalTokens: 200, inputTokens: 150, cachedInputTokens: 0, outputTokens: 50, reasoningOutputTokens: 10 },
        priceEstimate: { currency: 'USD', totalUsd: 0.5, inputUsd: 0.2, cachedInputUsd: 0, outputUsd: 0.3 }, pricePartial: true },
    ] });
    expect(device.workspaces.map(workspace => workspace.label)).toEqual(['app', 'w2']);
    expect(device.workspaces[0]!.threads).toEqual([expect.objectContaining({
      key: 'laptop:root', href: '/devices/laptop/threads/root', agent: 'claude', agentName: 'Claude',
      running: true, turns: 13, activeMs: 180_000, agentThreads: 2,
      // Totals and the hourly strip add up the family; the prompt and dates are the person's thread.
      totalMs: 720_000, hourlyMs: [120_000, 60_000], model: 'opus', lastPrompt: 'ship it',
      createdAt: '2026-10-01T00:00:00Z', lastActiveAt: '2026-10-16T09:00:00Z',
      tokenUsage: { totalTokens: 1200, inputTokens: 1050, cachedInputTokens: 500, cacheWriteInputTokens: 0, outputTokens: 150, reasoningOutputTokens: 10 },
      priceEstimate: { totalUsd: 1.5, inputUsd: 0.7, cachedInputUsd: 0.25, cacheWriteInputUsd: 0, outputUsd: 0.55 },
      pricePartial: true,
    })]);
    // Opening the tile up shows each agent thread on its own.
    expect(device.workspaces[0]!.threads[0]!.children!.map(child => [child.key, child.turns, child.priceEstimate?.totalUsd])).toEqual([
      ['laptop:child', 3, 0.5], ['laptop:grandchild', 0, undefined],
    ]);
    // An agent thread whose root is gone still shows.
    expect(device.workspaces[1]!.threads[0]).toMatchObject({ key: 'laptop:orphan', turns: 0, activeMs: 0 });
  });

  it('gives an agent thread working in another workspace a tile of its own there', () => {
    const device = mapDevice(null, 'This device', [...workspaces, { id: 'w2', label: 'docs', absPath: '/src/docs' }] as WorkspaceDto[], [
      thread('root', { title: 'Rebuild the home page' }),
      thread('beside', { parentThreadId: 'root', rootThreadId: 'root' }),
      thread('docs', { parentThreadId: 'root', rootThreadId: 'root', workspaceId: 'w2', title: 'Update docs' }),
      // Its own agent thread in the same workspace folds into it; one back home folds into the root.
      thread('docs-helper', { parentThreadId: 'docs', rootThreadId: 'root', workspaceId: 'w2' }),
      thread('back-home', { parentThreadId: 'docs', rootThreadId: 'root' }),
    ], { since: '', hours: 24, threads: [
      { threadId: 'root', turns: 2, activeMs: 1000, totalMs: 1000 },
      { threadId: 'docs', turns: 5, activeMs: 50_000, totalMs: 50_000 },
      { threadId: 'docs-helper', turns: 1, activeMs: 7000, totalMs: 7000 },
      { threadId: 'back-home', turns: 1, activeMs: 3000, totalMs: 3000 },
    ] });
    const [app, docs] = device.workspaces;
    expect(app!.threads.map(tile => [tile.key, tile.turns, tile.activeMs, tile.agentThreads, tile.from])).toEqual([
      ['local:root', 3, 4000, 2, undefined],
    ]);
    expect(app!.threads[0]!.children!.map(child => child.key)).toEqual(['local:beside', 'local:back-home']);
    expect(docs).toMatchObject({ id: 'w2', label: 'docs' });
    expect(docs!.threads.map(tile => [tile.key, tile.turns, tile.activeMs, tile.agentThreads, tile.from])).toEqual([
      ['local:docs', 6, 57_000, 1, 'Rebuild the home page'],
    ]);
    expect(docs!.threads[0]!.children!.map(child => child.key)).toEqual(['local:docs-helper']);
  });

  it('leaves turns and run time unknown without activity', () => {
    const device = mapDevice(null, 'This device', workspaces, [thread('a')], null);
    expect(device.workspaces[0]!.threads[0]).toMatchObject({ key: 'local:a', href: '/threads/a', turns: null, activeMs: null });
  });
});

describe('useThreadMapDevices', () => {
  beforeEach(() => {
    vi.mocked(request).mockReset();
    vi.mocked(relayModeActive).mockReturnValue(true);
  });

  it('reads every own device once opened, shows offline ones and tolerates old Supervisors', async () => {
    vi.mocked(request).mockImplementation(async (path) => {
      const url = String(path);
      if (url === 'relay/portal') return { devices: [
        { id: 'laptop', name: 'Laptop', connected: true },
        { id: 'old', name: 'Old', connected: true },
        { id: 'server', name: 'Server', connected: false },
      ] };
      if (url.endsWith('/api/workspaces')) return workspaces;
      if (url.includes('/api/threads?')) return [thread(url.includes('/old/') ? 'b' : 'a')];
      if (url.includes('/old/')) throw new Error('404');
      return { since: '', hours: 24, threads: [{ threadId: 'a', turns: 5, activeMs: 1000 }] };
    });
    const { result, rerender } = renderHook(({ open }) => useThreadMapDevices(open), { initialProps: { open: false } });
    expect(request).not.toHaveBeenCalled();
    rerender({ open: true });
    await waitFor(() => expect(result.current.devices.filter(device => device.state === 'ready')).toHaveLength(2));
    expect(result.current.devices.map(device => [device.id, device.state])).toEqual([['laptop', 'ready'], ['old', 'ready'], ['server', 'offline']]);
    expect(result.current.devices[0]!.workspaces[0]!.threads[0]).toMatchObject({ turns: 5, activeMs: 1000 });
    expect(result.current.devices[1]!.workspaces[0]!.threads[0]).toMatchObject({ turns: null, activeMs: null });
    expect(vi.mocked(request).mock.calls.map(([path]) => path)).toContain('relay/devices/laptop/api/thread-activity?hours=24');
    expect(vi.mocked(request).mock.calls.some(([path]) => String(path).includes('/server/'))).toBe(false);
  });

  it('marks a device it cannot read and retries it on request', async () => {
    let fail = true;
    vi.mocked(request).mockImplementation(async (path) => {
      const url = String(path);
      if (url === 'relay/portal') return { devices: [{ id: 'laptop', name: 'Laptop', connected: true }] };
      if (fail) throw new Error('locked');
      if (url.endsWith('/api/workspaces')) return workspaces;
      if (url.includes('/api/threads?')) return [thread('a')];
      return { since: '', hours: 24, threads: [] };
    });
    const { result } = renderHook(() => useThreadMapDevices(true));
    await waitFor(() => expect(result.current.devices[0]?.state).toBe('error'));
    fail = false;
    act(() => result.current.retry('laptop'));
    await waitFor(() => expect(result.current.devices[0]?.state).toBe('ready'));
  });

  it('reads the local Supervisor outside relay mode', async () => {
    vi.mocked(relayModeActive).mockReturnValue(false);
    vi.mocked(request).mockImplementation(async (path) => String(path).includes('activity') ? { since: '', hours: 24, threads: [] } : String(path).includes('threads') ? [thread('a')] : workspaces);
    const { result } = renderHook(() => useThreadMapDevices(true));
    await waitFor(() => expect(result.current.devices[0]?.state).toBe('ready'));
    expect(result.current.devices[0]).toMatchObject({ id: 'local', name: 'This device' });
    expect(vi.mocked(request).mock.calls.map(([path]) => path)).toEqual(expect.arrayContaining(['api/workspaces', 'api/threads?includeAgentThreads=true', 'api/thread-activity?hours=24']));
  });
});
