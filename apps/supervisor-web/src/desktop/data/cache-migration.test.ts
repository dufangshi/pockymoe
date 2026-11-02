import { expect, it, vi } from 'vitest';
import { readIndex } from './cache';
import { DesktopIndexStore } from './store';
import { buildActivitySnapshot } from './activity';
import type { DesktopIndex } from './types';
vi.mock('./cache', () => ({ readIndex: vi.fn(), writeIndex: vi.fn(), clearIndex: vi.fn() }));
vi.mock('../../lib/api', () => ({ connectSupervisorEvents: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn(), request: vi.fn() }));

it('migrates opened threads from older caches without treating cached device reads as this connection baseline', async () => {
  const cached = { accountId: 'account', devices: [{ id: 'device', name: 'Device', connected: true }],
    workspaces: [{ key: 'device:workspace', id: 'workspace', deviceId: 'device', label: 'Project' }],
    threads: [{ key: 'device:thread', id: 'thread', deviceId: 'device', workspaceId: 'workspace', title: 'Last opened', status: 'running', requests: [], lastViewedAt: '2026-11-01T12:00:00Z', updatedAt: '2026-11-01T12:00:00Z' }],
    syncedDeviceIds: ['device'], activity: {}, extras: {}, hydrated: true, refreshing: false, error: null, pulseKeys: [] } as unknown as DesktopIndex;
  vi.mocked(readIndex).mockResolvedValue(cached);
  const store = new DesktopIndexStore('account'); await store.hydrate();
  const snapshot = buildActivitySnapshot(store.getSnapshot(), null, false);
  expect(snapshot.syncedDeviceIds).toEqual([]); expect(snapshot.items).toEqual([]);
  expect(snapshot.recent).toEqual([{ key: 'device:thread', deviceId: 'device', deviceName: 'Device', workspaceLabel: 'Project', threadId: 'thread', title: 'Last opened' }]);
  store.stop();
});
