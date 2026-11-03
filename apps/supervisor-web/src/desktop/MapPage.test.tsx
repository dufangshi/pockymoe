import { render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ThreadMapProps } from '@pockymoe/thread-ui';
import type { DesktopIndex } from './data/types';
const fixture = vi.hoisted(() => ({ props: null as ThreadMapProps | null, index: null as DesktopIndex | null, navigate: vi.fn() }));
vi.mock('@pockymoe/thread-ui', async original => ({ ...await original<typeof import('@pockymoe/thread-ui')>(), MatterWorkbench: ({ children }: { children: React.ReactNode }) => <>{children}</>, ThreadMap: (props: ThreadMapProps) => { fixture.props = props; return null; } }));
vi.mock('react-router-dom', async original => ({ ...await original<typeof import('react-router-dom')>(), useNavigate: () => fixture.navigate }));
vi.mock('./data', async original => ({ ...await original<typeof import('./data')>(), useDesktopSelector: (select: (index: DesktopIndex) => unknown) => select(fixture.index!), useDesktopStore: () => ({}) }));
vi.mock('../pages/useWorkbenchNavigation', async original => ({ ...await original<typeof import('../pages/useWorkbenchNavigation')>(), useWorkbenchNavigation: () => ({}) }));
vi.mock('./context', () => ({ useDesktopWorkbench: () => ({ threads: [], notifications: [] }), useDesktopSurface: () => null }));
vi.mock('../components/ConversationSearch', () => ({ ConversationSearch: () => null }));
vi.mock('../components/RelayUserMenu', () => ({ RelayUserMenu: () => null }));
import { DesktopMapPage } from './MapPage';
it('keeps waiting, failed and unread tiles and opens the selected thread without map decisions', () => {
  fixture.index = { accountId: 'test', devices: [{ id: 'd', name: 'Mac', connected: true, lastHeartbeatAt: null, cachedAt: '2026-11-01' }], workspaces: [{ id: 'w', key: 'd:w', deviceId: 'd', label: 'Project', absPath: '/project' }],
    threads: ['waiting', 'failed', 'unread'].map((state, i) => ({ id: String(i), key: `d:${i}`, deviceId: 'd', workspaceId: 'w', title: state, status: state === 'failed' ? 'failed' : 'idle', requests: state === 'waiting' ? [{ id: 'r', kind: 'approval', summary: 'Review', createdAt: '2026-11-01' }] : [], lastViewedAt: null, lastTurnCompletedAt: '2026-11-01', updatedAt: '2026-11-01' })), activity: {}, extras: {}, pulseKeys: [], hydrated: true, refreshing: false, error: null } as unknown as DesktopIndex;
  performance.mark('pockymoe-desktop-shell-mount');
  const open = vi.fn();
  render(<DesktopMapPage visible mapVisible revealKey={null} onOpen={open} />);
  const props = fixture.props!;
  expect(props).not.toHaveProperty('attention'); expect(props).not.toHaveProperty('onRequestDecision');
  const tiles = props.devices[0]!.workspaces[0]!.threads;
  expect(tiles.find(tile => tile.key === 'd:0')?.needsInput).toMatchObject({ kind: 'approval', requestId: 'r' });
  expect(tiles.find(tile => tile.key === 'd:1')).toMatchObject({ failed: true, unread: false });
  expect(tiles.find(tile => tile.key === 'd:2')?.unread).toBe(true);
  props.onOpen(tiles.find(tile => tile.key === 'd:0')!); expect(open).toHaveBeenCalledWith('d:0');
});
