import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RelayAccessGrantDto, RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';
import { SharedAccessPanel } from './SharedAccessPanel';

vi.mock('../SignedShareDetails', () => ({ SignedShareDetails: () => <p>signed details</p> }));

const share: RelaySessionShareDto = {
  id: 's1', ownerUserId: 'me', ownerUsername: 'du', targetUserId: 'u-joe', targetUsername: 'joe',
  deviceId: 'dev-1', deviceName: 'Supervisor', deviceConnected: true, threadId: 't1', threadTitle: 'Video pipeline',
  workspaceId: 'ws-1', workspaceLabel: 'creativocli', label: null, threadAccess: 'read', workspaceAccess: 'read',
  createdAt: '2026-10-01T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null,
  accessEvents: [{ id: 'e1', shareId: 's1', userId: 'u-joe', username: 'joe', kind: 'open_thread', accessedAt: '2026-10-20T10:00:00Z' }],
};
const grant: RelayAccessGrantDto = {
  id: 'g1', ownerUserId: 'u-al', ownerUsername: 'alwyn', targetUserId: 'me', targetUsername: 'du',
  deviceId: 'dev-2', deviceName: 'dev', deviceConnected: false, scope: 'workspace', threadId: null, threadTitle: null,
  workspaceId: 'ws-9', workspaceLabel: 'infra', workspaceScope: 'all', workspaceIds: [], label: null,
  threadAccess: 'control', workspaceAccess: 'write', canCreateThreads: true,
  createdAt: '2026-10-02T00:00:00Z', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [],
};
const portal = { user: { id: 'me' }, devices: [], sharedWithMe: [], sharedByMe: [share], sharedThreadsWithMe: [grant] } as unknown as RelayPortalSummaryDto;

function renderPanel(direction: 'in' | 'out', attention = new Map()) {
  const actions = { onOpen: vi.fn(), onEdit: vi.fn(), onRevoke: vi.fn() };
  const onDirectionChange = vi.fn();
  render(<SharedAccessPanel portal={portal} loading={false} direction={direction} onDirectionChange={onDirectionChange}
    attention={attention} onAttentionRefresh={vi.fn()} busyKey={null} actions={actions} />);
  return { actions, onDirectionChange };
}

beforeEach(() => localStorage.clear());

describe('SharedAccessPanel', () => {
  it('lists my shares as compact rows whose details and actions open in place', () => {
    const { actions } = renderPanel('out', new Map([['share:s1', 'unverified']]));
    const toggle = screen.getByRole('button', { name: /Video pipeline/ });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(screen.getAllByText('Safety code not compared').length).toBeGreaterThan(0);
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('signed details')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Access history' }));
    expect(screen.getByText('open thread')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Edit permissions' }));
    fireEvent.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(actions.onEdit).toHaveBeenCalledWith(expect.objectContaining({ key: 'share:s1' }));
    expect(actions.onRevoke).toHaveBeenCalledWith(expect.objectContaining({ key: 'share:s1' }));
    fireEvent.click(screen.getByRole('button', { name: /To review/ }));
    expect(screen.getByRole('button', { name: /Video pipeline/ })).toBeTruthy();
  });

  it('opens what was shared with me, and not while its device is offline', () => {
    const { actions, onDirectionChange } = renderPanel('in');
    const open = screen.getByRole('button', { name: 'Open infra' }) as HTMLButtonElement;
    expect(open.disabled).toBe(true);
    expect(screen.getByText(/from alwyn/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Shared by me/ }));
    expect(onDirectionChange).toHaveBeenCalledWith('out');
    expect(actions.onOpen).not.toHaveBeenCalled();
  });

  it('switches to by-device and by-person views and remembers the choice', () => {
    renderPanel('out');
    fireEvent.click(screen.getByRole('button', { name: 'By device' }));
    const group = screen.getByRole('region', { name: 'Supervisor' });
    expect(within(group).getByText('creativocli')).toBeTruthy();
    expect(localStorage.getItem('pockymoe.sharedAccess.view')).toBe('devices');
    fireEvent.click(screen.getByRole('button', { name: 'By person' }));
    const people = screen.getByRole('navigation', { name: 'People you share with' });
    expect(within(people).getByText('joe')).toBeTruthy();
    expect(within(people).getByText('1 threads')).toBeTruthy();
  });

  it('renders what was shared with me in the shape the relay sends it, in every view', () => {
    // The relay repeats thread shares inside sharedThreadsWithMe, without a grant scope.
    const incoming = { ...share, id: 's2', ownerUserId: 'u-al', ownerUsername: 'alwyn', targetUserId: 'me', targetUsername: 'du', threadTitle: 'prod bootstrap' };
    const relayPortal = { user: { id: 'me' }, devices: [], sharedWithMe: [incoming], sharedByMe: [], sharedDevicesWithMe: [], sharedThreadsWithMe: [incoming, grant] } as unknown as RelayPortalSummaryDto;
    for (const view of ['list', 'devices', 'people']) {
      localStorage.setItem('pockymoe.sharedAccess.view', view);
      const { unmount } = render(<SharedAccessPanel portal={relayPortal} loading={false} direction="in" onDirectionChange={vi.fn()}
        attention={new Map()} onAttentionRefresh={vi.fn()} busyKey={null} actions={{ onOpen: vi.fn(), onEdit: vi.fn(), onRevoke: vi.fn() }} />);
      // A narrow screen lists the people first.
      if (view === 'people' && !screen.queryAllByText('prod bootstrap').length) fireEvent.click(screen.getByRole('button', { name: /alwyn/ }));
      expect(screen.getAllByText('prod bootstrap')).toHaveLength(1);
      expect(screen.getAllByText('infra').length).toBeGreaterThan(0);
      unmount();
    }
  });

  it('says when nothing matches the search', () => {
    renderPanel('out');
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search names, devices or people' }), { target: { value: 'nothing-like-this' } });
    expect(screen.getByText('No share matches.')).toBeTruthy();
  });
});
