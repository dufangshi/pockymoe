import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, ThreadDetailDto, ThreadDto, ThreadEventEnvelope, WorkspaceDto } from '@pockymoe/shared';
import { mergeRows } from './merge';
import { DesktopIndexStore, type DesktopDataAdapter } from './store';
import { plainSummary, summarizeThread, unread } from './types';
import { attentionFromActivity, buildActivityItems, buildActivitySnapshot } from './activity';
vi.mock('../../lib/api', () => ({ connectSupervisorEvents: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn(), request: vi.fn() }));
const thread = (id: string, patch: Partial<ThreadDto> = {}): ThreadDto => ({ id, workspaceId: 'w', title: id, provider: 'codex', model: 'gpt', status: 'idle', isPinned: false, createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-30T00:00:00Z', lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null, summaryText: null, lastError: null, ...patch } as ThreadDto);
function setup() {
  const sockets: WebSocket[] = [];
  const get = vi.fn(async (_device: string, path: string) => {
    if (path === 'api/workspaces') return [{ id: 'w', label: 'Project', createdAt: '', lastOpenedAt: null, isFavorite: false, hostId: 'h', absPath: '/project' } satisfies WorkspaceDto];
    if (path.startsWith('api/threads?')) return [thread('same')];
    if (path.startsWith('api/thread-activity')) return { since: '', hours: 24, threads: [] };
    if (path.startsWith('api/threads/')) return { thread: thread('same'), turns: [], pendingRequests: [{ id: 'input', kind: 'requestUserInput', title: 'Choose a path', createdAt: '2026-11-01T00:00:00Z' }] } as unknown as ThreadDetailDto;
    return {};
  });
  const io = {
    portal: async () => ({ devices: ['a', 'b'].map(id => ({ id, ownerUserId: 'account', name: id, connected: true, lastHeartbeatAt: null })) }) as RelayPortalSummaryDto,
    get, presence: async () => ({ connected: true }), socket: () => { const socket = Object.assign(new EventTarget(), { close: vi.fn(), readyState: WebSocket.CONNECTING }) as unknown as WebSocket; sockets.push(socket); return socket; },
  } as unknown as DesktopDataAdapter;
  return { store: new DesktopIndexStore('account', io), get, sockets };
}
afterEach(() => vi.useRealTimers());
describe('desktop cached index', () => {
  it('keeps native summaries plain, bounded and free of recognizable credentials', () => {
    expect(plainSummary('<b>Done</b>\nnext line')).toBe('Done');
    expect(plainSummary('Bearer ' + 'x'.repeat(32))).toBe('Bearer [redacted]');
    expect(plainSummary('api_key=' + 'x'.repeat(32))).toBe('api_key=[redacted]');
    expect(plainSummary('x'.repeat(200))).toHaveLength(140);
  });
  it('does not mark imported history unread on first observation, but observes a later completion', () => {
    const row = summarizeThread('a', thread('history', { lastTurnCompletedAt: '2026-10-01T00:00:00Z' }));
    expect(row.lastViewedAt).toBeNull(); expect(unread(row)).toBe(false);
    expect(unread({ ...row, lastTurnCompletedAt: '2026-11-01T00:00:00Z' })).toBe(true);
  });
  it('retains unchanged identities even when response property order differs', () => {
    const previous = [{ id: 'a', value: { x: 1, y: 2 } }];
    expect(mergeRows(previous, [{ value: { y: 2, x: 1 }, id: 'a' }], row => row.id)).toBe(previous);
    expect(mergeRows(previous, [{ id: 'a', value: { x: 2, y: 2 } }], row => row.id)[0]).not.toBe(previous[0]);
  });
  it('isolates equal thread IDs across devices and recovers cold input requests', async () => {
    const { store, get } = setup(); await store.refresh();
    expect(store.getSnapshot().threads.map(row => row.key)).toEqual(['a:same', 'b:same']);
    expect(store.getSnapshot().threads.every(row => row.requests[0]?.kind === 'input')).toBe(true);
    const previous = store.getSnapshot().threads[0]; get.mockClear(); await store.refreshDevice('a');
    expect(store.getSnapshot().threads.find(row => row.key === 'a:same')).toBe(previous);
    expect(get.mock.calls.filter(([, path]) => path.startsWith('api/threads/'))).toHaveLength(0);
    store.stop();
  });
  it('drops high-volume events and updates only the addressed row', async () => {
    const { store } = setup(); await store.refresh();
    const previous = store.getSnapshot().threads, notify = vi.fn(); store.subscribe(notify);
    store.applyEvent('a', { type: 'thread.output.delta', threadId: 'same' } as ThreadEventEnvelope);
    expect(notify).not.toHaveBeenCalled();
    store.applyEvent('a', { type: 'thread.turn.completed', threadId: 'same', timestamp: '2026-11-01T00:00:00Z', payload: { turnId: 'turn', status: 'completed', error: null } });
    expect(store.getSnapshot().threads[1]).toBe(previous[1]); expect(unread(store.getSnapshot().threads[0]!)).toBe(true);
    store.markRead('a:same'); expect(unread(store.getSnapshot().threads[0]!)).toBe(false); store.stop();
  });
  it('preserves a warm tree during revalidation', async () => {
    const { store } = setup(); await store.refresh();
    const sizes: number[] = []; store.subscribe(() => sizes.push(store.getSnapshot().threads.length)); await store.refresh();
    expect(sizes.every(size => size === 2)).toBe(true); store.stop();
  });
  it('refreshes activity only while the map is visible and debounces completion by 2s', async () => {
    vi.useFakeTimers();
    const { store, get } = setup(); await store.refresh(); get.mockClear();
    store.applyEvent('a', { type: 'thread.turn.completed', threadId: 'same', timestamp: '2026-11-01T00:00:00Z', payload: { turnId: 'turn', status: 'completed', error: null } });
    await vi.advanceTimersByTimeAsync(2500);
    expect(get.mock.calls.some(([, path]) => path.startsWith('api/thread-activity'))).toBe(false);
    store.setMapVisible(true); await vi.advanceTimersByTimeAsync(0); get.mockClear();
    store.applyEvent('a', { type: 'thread.turn.completed', threadId: 'same', timestamp: '2026-11-01T00:01:00Z', payload: { turnId: 'turn2', status: 'completed', error: null } });
    await vi.advanceTimersByTimeAsync(1999); expect(get.mock.calls.some(([, path]) => path.startsWith('api/thread-activity'))).toBe(false);
    await vi.advanceTimersByTimeAsync(1); expect(get.mock.calls.filter(([, path]) => path.startsWith('api/thread-activity'))).toHaveLength(1);
    store.setMapVisible(false); get.mockClear(); await vi.advanceTimersByTimeAsync(60_000);
    expect(get.mock.calls.some(([, path]) => path.startsWith('api/thread-activity'))).toBe(false); store.stop();
  });
  it('uses the same needs-you/failed/unread order for host and map attention', async () => {
    const { store } = setup(); await store.refresh();
    store.applyEvent('a', { type: 'thread.request.resolved', threadId: 'same', timestamp: '2026-11-01T01:00:00Z', payload: { requestId: 'input' } });
    store.applyEvent('a', { type: 'thread.turn.failed', threadId: 'same', timestamp: '2026-11-01T01:01:00Z', payload: { turnId: 'failed', error: '<b>Failure</b>\nsecret second line' } });
    const items = buildActivityItems(store.getSnapshot(), Date.parse('2026-11-01T02:00:00Z'));
    expect(items.map(item => item.state)).toEqual(['needsInput', 'failed']);
    expect(attentionFromActivity(items).map(item => item.key)).toEqual(items.map(item => item.key));
    expect(items[1]?.resultSummary).toBe('Failure');
    expect(buildActivitySnapshot(store.getSnapshot(), 'a:same', false).focusedThreadKey).toBeNull(); store.stop();
  });
});

it('clears a pulse edge before another reminder on the same thread', async () => {
  vi.useFakeTimers();
  const { store } = setup(); await store.refresh(); store.setMapVisible(true);
  const pulses: string[][] = []; store.subscribe(() => pulses.push([...store.getSnapshot().pulseKeys]));
  for (const [index, timestamp] of ['2026-11-01T01:00:00Z', '2026-11-01T01:01:00Z'].entries()) store.applyEvent('a', { type: 'thread.turn.completed', threadId: 'same', timestamp, payload: { turnId: `turn${index}`, status: 'completed', error: null } });
  expect(pulses.some(keys => keys.length === 0)).toBe(true);
  await vi.advanceTimersByTimeAsync(20); expect(store.getSnapshot().pulseKeys).toContain('a:same');
  store.stop(); vi.useRealTimers();
});
