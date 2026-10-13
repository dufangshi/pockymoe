import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  RelayDeviceDto,
  ThreadDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import { request } from '../lib/api';
import {
  rankSplitDevices,
  rankSplitThreads,
  rankSplitWorkspaces,
  useWorkbenchSplitCatalog,
} from './useWorkbenchSplitCatalog';
vi.mock('../lib/api', () => ({ request: vi.fn() }));
const thread = (id: string, workspaceId = 'workspace'): ThreadDto =>
  ({
    id,
    workspaceId,
    title: id,
    updatedAt: '2026-10-08',
    isPinned: false,
  }) as ThreadDto;
beforeEach(() => vi.mocked(request).mockReset());

describe('split catalog transport', () => {
  it('waits until opening, then loads remote workspaces and threads only as their branches expand', async () => {
    vi.mocked(request).mockImplementation(async (path) =>
      String(path) === 'relay/portal'
        ? {
            devices: [
              { id: 'host', name: 'Host' },
              { id: 'remote', name: 'Remote' },
            ],
          }
        : String(path).includes('/workspaces')
          ? [{ id: 'workspace', label: 'Workspace' }]
          : [thread('peer')],
    );
    const { result, rerender } = renderHook(
      ({ open }) => useWorkbenchSplitCatalog(open, 'host', 'workspace'),
      { initialProps: { open: false } },
    );
    expect(request).not.toHaveBeenCalled();
    rerender({ open: true });
    await waitFor(() => expect(result.current.devices.loaded).toBe(true));
    expect(result.current.devices.items.map((device) => device.id)).toEqual([
      'remote',
    ]);
    expect(vi.mocked(request).mock.calls.map(([path]) => path)).toEqual(
      expect.arrayContaining([
        'relay/portal',
        'relay/devices/host/api/workspaces',
        'relay/devices/host/api/threads?includeAgentThreads=true',
      ]),
    );
    expect(
      vi
        .mocked(request)
        .mock.calls.some(([path]) => String(path).includes('/remote/')),
    ).toBe(false);
    await act(() => result.current.loadWorkspaces('remote'));
    expect(request).toHaveBeenCalledWith(
      'relay/devices/remote/api/workspaces',
      expect.any(Object),
    );
    expect(
      vi
        .mocked(request)
        .mock.calls.some(([path]) =>
          String(path).includes('/remote/api/threads'),
        ),
    ).toBe(false);
    await act(() => result.current.loadThreads('remote'));
    expect(request).toHaveBeenCalledWith(
      'relay/devices/remote/api/threads?includeAgentThreads=true',
      expect.any(Object),
    );
    const count = vi.mocked(request).mock.calls.length;
    await act(() => result.current.loadThreads('remote'));
    expect(request).toHaveBeenCalledTimes(count);
  });
  it('uses local paths, exposes permission errors with retry and ignores a late previous-scope response', async () => {
    let finish!: (value: unknown) => void;
    vi.mocked(request).mockImplementation(async (path) => {
      if (String(path).startsWith('relay/devices/old'))
        return await new Promise((resolve) => {
          finish = resolve;
        });
      if (String(path).endsWith('/workspaces')) return [];
      throw new Error('Workspace access denied');
    });
    const { result, rerender } = renderHook(
      ({ device }) => useWorkbenchSplitCatalog(true, device, 'workspace'),
      { initialProps: { device: 'old' as string | null } },
    );
    rerender({ device: null });
    await waitFor(() =>
      expect(result.current.threads(null).error).toBe(
        'Workspace access denied',
      ),
    );
    expect(request).toHaveBeenCalledWith(
      'api/threads?includeAgentThreads=true',
      expect.any(Object),
    );
    await act(async () => finish([thread('private-old-device')]));
    expect(result.current.threads(null).items).toEqual([]);
    vi.mocked(request).mockResolvedValue([thread('local-peer')]);
    await act(() => result.current.loadThreads(null, true));
    expect(result.current.threads(null).items[0]?.id).toBe('local-peer');
    expect(result.current.threads(null).error).toBeNull();
  });
});
describe('hierarchical priority', () => {
  it('ranks favorites and visits only in the addressed device and orders favorite/recent workspaces', () => {
    const threads = [
      thread('new'),
      thread('recent'),
      thread('favorite'),
      thread('other-device-favorite'),
    ];
    expect(
      rankSplitThreads(threads, 'host', [
        { key: 'host:recent', favorite: false },
        { key: 'host:favorite', favorite: true },
        { key: 'remote:other-device-favorite', favorite: true },
      ]).map((value) => value.id),
    ).toEqual(['favorite', 'recent', 'new', 'other-device-favorite']);
    expect(
      rankSplitDevices(
        [
          { id: 'new', name: 'A new device' },
          { id: 'recent', name: 'Recent device' },
          { id: 'fav', name: 'Favorite device' },
        ] as RelayDeviceDto[],
        [
          { key: 'recent:peer', favorite: false },
          { key: 'fav:peer', favorite: true },
        ],
      ).map((device) => device.id),
    ).toEqual(['fav', 'recent', 'new']);
    expect(
      rankSplitWorkspaces(
        [
          { id: 'old', label: 'Old' },
          { id: 'recent', label: 'Recent', lastOpenedAt: '2026-10-08' },
          { id: 'fav', label: 'Favorite', isFavorite: true },
        ].map((value) => ({
          hostId: 'host',
          absPath: '/code',
          createdAt: '2026-10-08',
          lastOpenedAt: null,
          isFavorite: false,
          ...value,
        })) as WorkspaceDto[],
      ).map((value) => value.id),
    ).toEqual(['fav', 'recent', 'old']);
  });
});
