import { afterEach, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, ThreadDetailDto, ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { DesktopIndexStore, type DesktopDataAdapter } from './store';
import { buildActivitySnapshot } from './activity';
import { syncActivity } from '../integration';
import type { PockymoeHost } from '../host';
vi.mock('../../lib/api', () => ({ connectSupervisorEvents: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn(), request: vi.fn() }));
afterEach(() => vi.useRealTimers());

function realAccountShape() {
  const now = Date.parse('2026-11-01T12:00:00Z');
  const devices = Array.from({ length: 11 }, (_, i) => ({ id: `device-${i}`, name: `Device ${i}`, ownerUserId: 'account', connected: true, lastHeartbeatAt: new Date(now).toISOString() }));
  const threads: ThreadDto[] = Array.from({ length: 250 }, (_, i) => ({ id: `history-${i}`, workspaceId: `w-${i % 21}`, title: `Historical task ${i}`, provider: 'acp', model: 'fixture', status: i % 3 === 0 ? 'failed' : 'idle', isPinned: false, createdAt: '2026-01-01T00:00:00Z', updatedAt: new Date(now - 3_600_000).toISOString(), lastTurnStartedAt: new Date(now - 3_700_000).toISOString(), lastTurnCompletedAt: new Date(now - 3_600_000).toISOString(), activeTurnId: null, summaryText: null, lastError: i % 3 === 0 ? 'Old failure' : null } as ThreadDto));
  const active = (device: string): ThreadDto => ({ ...threads[0]!, id: `active-${device}`, title: 'Current activity', status: 'running', lastTurnCompletedAt: null, activeTurnId: 'turn', updatedAt: new Date(now).toISOString() });
  let omit = false, fail = false;
  const get = vi.fn(async (device: string, path: string) => {
    if (fail && device === 'device-1') throw new Error('Temporary device read failure');
    if (device === 'device-0' && (path.startsWith('api/thread-activity') || path.startsWith('api/stats/') || path.startsWith('api/device/') || path.startsWith('api/management/'))) throw Object.assign(new Error('0.12.73 endpoint unavailable'), { statusCode: 404 });
    if (path === 'api/workspaces') return Array.from({ length: 21 }, (_, i) => ({ id: `w-${i}`, label: `Workspace ${i}`, absPath: `/fixture/workspace-${i}`, hostId: 'fixture', isFavorite: false, createdAt: '', lastOpenedAt: null } satisfies WorkspaceDto));
    if (path === 'api/threads?includeAgentThreads=true' && device === 'device-0') throw Object.assign(new Error('Legacy list endpoint'), { statusCode: 404 });
    if (path.startsWith('api/threads?') || path === 'api/threads') return device === 'device-10' ? [] : [active(device), ...threads.filter((_, i) => i % 10 === Number(device.split('-')[1]))];
    if (path.startsWith('api/threads/')) {
      const id = decodeURIComponent(path.split('/')[2]!.split('?')[0]!);
      return { thread: id.startsWith('active-') ? active(device) : threads.find(thread => thread.id === id), turns: [], pendingRequests: device === 'device-2' && id.startsWith('active-') ? [{ id: 'cold-approval', kind: 'permissionRequest', title: 'Run checks?', createdAt: new Date(now - 60_000).toISOString(), questions: [] }] : [] } as unknown as ThreadDetailDto;
    }
    if (path.startsWith('api/thread-activity')) return { since: '', hours: 24, threads: [] };
    return {};
  });
  const adapter = { portal: async () => ({ devices: devices.filter(device => !(omit && device.id === 'device-1')) }) as RelayPortalSummaryDto,
    get, presence: async (id: string) => ({ connected: !(fail && id === 'device-1') }),
    socket: () => Object.assign(new EventTarget(), { close: vi.fn(), readyState: WebSocket.CONNECTING }),
  } as unknown as DesktopDataAdapter;
  return { store: new DesktopIndexStore('account', adapter), get, flap: (value: boolean) => { omit = value; fail = value; } };
}

it('does not flap native snapshot membership when portal presence and a device read fail briefly', async () => {
  vi.useFakeTimers(); vi.setSystemTime('2026-11-01T12:00:00Z');
  const { store, flap } = realAccountShape(); await store.refresh();
  const membership = () => buildActivitySnapshot(store.getSnapshot(), null, false).items.map(item => `${item.key}:${item.state}`).join(',');
  const baseline = membership(); const changes = new Set([baseline]);
  const host = { activity: { sync: vi.fn() }, badge: { set: vi.fn() } } as unknown as PockymoeHost;
  const stopNative = syncActivity(store, host, () => null, true); await vi.advanceTimersByTimeAsync(0);
  const stop = store.subscribe(() => changes.add(membership()));
  for (let i = 0; i < 4; i++) { flap(true); await store.refresh(); await vi.advanceTimersByTimeAsync(1000); flap(false); await store.refresh(); }
  expect(changes.size).toBe(1);
  expect(membership()).toBe(baseline); expect(host.activity.sync).toHaveBeenCalledTimes(1); stopNative(); stop(); store.stop();
});

it('finishes a baseline for every device, including an old runtime and a device with no threads', async () => {
  vi.useFakeTimers(); vi.setSystemTime('2026-11-01T12:00:00Z');
  const { store, get } = realAccountShape(); await store.refresh();
  const snapshot = buildActivitySnapshot(store.getSnapshot(), null, false);
  expect(snapshot).toHaveProperty('syncedDeviceIds', Array.from({ length: 11 }, (_, i) => `device-${i}`).sort());
  expect(snapshot.items.some(item => item.key === 'device-0:active-device-0')).toBe(true);
  expect(get.mock.calls.some(([device, path]) => device === 'device-0' && path === 'api/threads')).toBe(true); store.stop();
});


it('retains missing-device activity for 60s then removes it once', async () => {
  vi.useFakeTimers(); vi.setSystemTime('2026-11-01T12:00:00Z');
  const { store, flap } = realAccountShape(); await store.refresh();
  const present = () => buildActivitySnapshot(store.getSnapshot(), null, false).items.some(item => item.deviceId === 'device-1');
  expect(present()).toBe(true); flap(true); await store.refresh();
  await vi.advanceTimersByTimeAsync(60_000); expect(present()).toBe(true);
  await vi.advanceTimersByTimeAsync(2); expect(present()).toBe(false);
  expect(store.getSnapshot().devices.some(device => device.id === 'device-1')).toBe(false); store.stop();
});

it('keeps old-runtime optional endpoint failures out of activity and remembers five opened conversations', async () => {
  vi.useFakeTimers(); vi.setSystemTime('2026-11-01T12:00:00Z');
  const { store } = realAccountShape(); await store.refresh();
  const before = JSON.stringify(buildActivitySnapshot(store.getSnapshot(), null, false).items);
  store.setMapVisible(true); await store.refreshActivity('device-0'); await store.refreshExtras('device-0');
  expect(JSON.stringify(buildActivitySnapshot(store.getSnapshot(), null, false).items)).toBe(before);
  for (let i = 0; i < 7; i++) store.rememberThread(`device-${i}:active-device-${i}`);
  const recent = buildActivitySnapshot(store.getSnapshot(), null, false).recent!;
  expect(recent.map(item => item.key)).toEqual([6, 5, 4, 3, 2].map(i => `device-${i}:active-device-${i}`));
  expect(recent[0]?.workspaceLabel).toBe('Workspace 0'); store.stop();
});
