import { describe, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, ThreadDetailDto, ThreadDto, ThreadEventEnvelope, WorkspaceDto } from '@remote-codex/shared';
import { DesktopIndexStore, isIndexEvent, mergeRows, type DesktopDataAdapter } from './store';
import { deviceApiPath, deviceRequest } from '../../lib/api';
import { unread } from './types';

vi.mock('../../lib/relayTransport', () => ({ encryptedBrowserFetch: vi.fn(async (url: string) => new Response(JSON.stringify({ url }))), encryptedRelaySocket: vi.fn() }));
const thread = (id: string): ThreadDto => ({ id, workspaceId: 'ws', title: id, provider: 'codex', status: 'idle', isPinned: false, createdAt: '2026-10-30T10:00:00Z', updatedAt: '2026-10-30T10:00:00Z', lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null, summaryText: null, lastError: null } as ThreadDto);
function setup() {
  const get = vi.fn(async (deviceId: string, path: string) => {
    if (path.includes('/api/threads?')) return [thread('same')];
    if (path === '/api/workspaces') return [{ id: 'ws', label: deviceId, isFavorite: false, createdAt: '', lastOpenedAt: null } as WorkspaceDto];
    return { thread: thread('same'), pendingRequests: [{ id: 'approval', kind: 'permissionRequest', title: 'Run tests', description: null }] } as ThreadDetailDto;
  });
  const io = { portal: vi.fn(async () => ({ user: { id: 'account' }, devices: ['a', 'b'].map(id => ({ id, name: id, connected: true, ownerUserId: 'account', lastHeartbeatAt: null })) } as RelayPortalSummaryDto)), get, socket: () => Object.assign(new EventTarget(), { close: vi.fn() }) as unknown as WebSocket } as unknown as DesktopDataAdapter;
  const store = new DesktopIndexStore('account', io);
  return { store, get };
}
describe('desktop index', () => {
  it('keeps unchanged references including nested request summaries', () => {
    const old = [{ id: 'a', requests: [{ id: 'r' }] }, { id: 'b', requests: [] }];
    expect(mergeRows(old, structuredClone(old), row => row.id)).toBe(old);
    const next = mergeRows(old, [{ ...old[0]!, requests: [] }, old[1]!], row => row.id);
    expect(next[1]).toBe(old[1]); expect(next[0]).not.toBe(old[0]);
  });
  it('indexes same IDs independently across devices and restores cold-start pending requests', async () => {
    const { store, get } = setup();
    await store.refresh();
    expect(store.getSnapshot().threads.map(row => row.key)).toEqual(['a:same', 'b:same']);
    expect(store.getSnapshot().threads.every(row => row.requests[0]?.id === 'approval')).toBe(true);
    expect(get).toHaveBeenCalledWith('b', '/api/threads?includeAgentThreads=true', expect.anything());
    const previous = store.getSnapshot().threads[0]; await store.refreshDevice('a');
    expect(store.getSnapshot().threads.find(row => row.key === 'a:same')).toBe(previous);
    store.stop();
  });
  it('drops output deltas without notifying subscribers and updates only the target row', async () => {
    const { store } = setup(); await store.refresh();
    const subscriber = vi.fn(); store.subscribe(subscriber);
    const previous = store.getSnapshot().threads;
    store.applyEvent('a', { type: 'thread.output.delta', threadId: 'same' } as ThreadEventEnvelope);
    expect(subscriber).not.toHaveBeenCalled(); expect(isIndexEvent('thread.item.started')).toBe(false);
    store.applyEvent('a', { type: 'thread.turn.completed', threadId: 'same', timestamp: '2026-11-01T00:00:00Z', payload: { turnId: 'turn', status: 'completed', error: null } });
    expect(store.getSnapshot().threads[1]).toBe(previous[1]);
    expect(unread(store.getSnapshot().threads[0]!)).toBe(true);
    store.markRead('a:same'); expect(unread(store.getSnapshot().threads[0]!)).toBe(false);
    store.stop();
  });
  it('never clears a warm tree while revalidating', async () => {
    const { store } = setup(); await store.refresh();
    const count: number[] = []; store.subscribe(() => count.push(store.getSnapshot().threads.length));
    await store.refresh(); expect(count.length).toBeGreaterThan(0); expect(count.every(value => value === 2)).toBe(true); store.stop();
  });
  it('routes concurrent requests explicitly without changing browser selection', async () => {
    localStorage.setItem('remote-codex-relay-device-id', 'current');
    const [a, b] = await Promise.all(['a', 'b'].map(id => deviceRequest<{ url: string }>(id, '/api/threads')));
    expect(a!.url).toBe('/relay/devices/a/api/threads'); expect(b!.url).toBe('/relay/devices/b/api/threads');
    expect(localStorage.getItem('remote-codex-relay-device-id')).toBe('current');
    expect(deviceApiPath('a/b', '/api/threads')).toBe('/relay/devices/a%2Fb/api/threads');
  });
});
