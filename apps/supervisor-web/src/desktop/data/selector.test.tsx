import { act, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ThreadDto, RelayPortalSummaryDto } from '@pockymoe/shared';
import { DesktopStoreContext, useDesktopSelector } from './index';
import { DesktopIndexStore, type DesktopDataAdapter } from './store';
vi.mock('../../lib/api', () => ({ connectSupervisorEvents: vi.fn(), deviceRequest: vi.fn(), fetchRelayPortal: vi.fn(), request: vi.fn() }));
it('re-renders only a selector whose thread identity changes', async () => {
  const thread = { id: 't', title: 'Thread', workspaceId: 'w', provider: 'acp', status: 'idle', updatedAt: '2026-10-01T00:00:00Z', createdAt: '', isPinned: false, lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null } as ThreadDto;
  const io = { portal: async () => ({ devices: ['a', 'b'].map(id => ({ id, name: id, connected: true, ownerUserId: 'owner', lastHeartbeatAt: null })) }) as RelayPortalSummaryDto,
    get: async (_id: string, path: string) => path === 'api/workspaces' ? [] : path.startsWith('api/threads?') ? [thread] : { thread, turns: [], pendingRequests: [] },
    socket: () => Object.assign(new EventTarget(), { close: vi.fn() }) as unknown as WebSocket, presence: async () => ({ connected: true }),
  } as unknown as DesktopDataAdapter;
  const store = new DesktopIndexStore('owner', io); await store.refresh();
  const counts = { a: 0, b: 0 };
  function Row({ id }: { id: 'a' | 'b' }) { const thread = useDesktopSelector(index => index.threads.find(thread => thread.key === `${id}:t`)); counts[id]++; return <span>{thread?.status}</span>; }
  const view = render(<DesktopStoreContext.Provider value={store}><Row id="a" /><Row id="b" /></DesktopStoreContext.Provider>);
  const before = { ...counts };
  act(() => store.applyEvent('a', { type: 'thread.turn.started', threadId: 't', timestamp: '2026-11-01T00:00:00Z', payload: { turnId: 'turn' } }));
  expect(counts.a).toBe(before.a + 1); expect(counts.b).toBe(before.b);
  view.unmount(); store.stop();
});
