import { act, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, ThreadDetailDto, ThreadDto } from '@remote-codex/shared';
import { DesktopStoreContext, useDesktopSelector } from './index';
import { DesktopIndexStore, type DesktopDataAdapter } from './store';

it('notifies only the row selected from the changed device', async () => {
  const thread = { id: 't', title: 'Thread', workspaceId: 'w', provider: 'codex', status: 'idle', updatedAt: '2026-10-30T00:00:00Z', createdAt: '', isPinned: false, lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null } as ThreadDto;
  const io: DesktopDataAdapter = {
    portal: async () => ({ devices: ['a', 'b'].map(id => ({ id, name: id, connected: true, ownerUserId: 'owner', lastHeartbeatAt: null })) }) as RelayPortalSummaryDto,
    get: (async (_id, path) => path === '/api/workspaces' ? [] : path.includes('/api/threads?') ? [thread] : { thread, turns: [], pendingRequests: [] } as unknown as ThreadDetailDto) as DesktopDataAdapter['get'],
    socket: () => Object.assign(new EventTarget(), { close: vi.fn() }) as unknown as WebSocket,
    presence: async () => ({ connected: true }),
  };
  const store = new DesktopIndexStore('owner', io); await store.refresh();
  const renders = { a: 0, b: 0 };
  function Row({ device }: { device: 'a' | 'b' }) {
    const thread = useDesktopSelector(index => index.threads.find(thread => thread.key === `${device}:t`));
    renders[device]++; return <span>{thread?.status}</span>;
  }
  const view = render(<DesktopStoreContext.Provider value={store}><Row device="a" /><Row device="b" /></DesktopStoreContext.Provider>);
  const before = { ...renders };
  act(() => store.applyEvent('a', { type: 'thread.turn.started', threadId: 't', timestamp: '2026-11-01T00:00:00Z', payload: { turnId: 'turn' } }));
  expect(renders.a).toBe(before.a + 1); expect(renders.b).toBe(before.b);
  view.unmount(); store.stop();
});
