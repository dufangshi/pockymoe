import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import type { RelayAccessGrantDto, RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';

const state = vi.hoisted(() => ({ portal: vi.fn(), account: vi.fn() }));
vi.mock('../lib/api', () => ({
  enableRelayMode: vi.fn(),
  fetchRelayPortal: state.portal,
  setSelectedRelayDeviceId: vi.fn(),
  setSelectedRelayThreadId: vi.fn(),
  relayModeActive: () => true,
}));
vi.mock('../lib/accountVault', () => ({
  currentAccount: state.account,
  safetyCode: async () => 'VDNZ DB8Z 3HKC',
}));
vi.mock('../components/ProductHeader', () => ({ ProductHeader: ({ title }: { title: string }) => <header>{title}</header> }));
import { SharePendingPage } from './SharePendingPage';

const grant = {
  id: 'g1', ownerUserId: 'u-owner', ownerUsername: 'alwyn', targetUserId: 'me', targetUsername: 'me', deviceId: 'dev', deviceName: 'Laptop',
  deviceConnected: true, scope: 'workspace', threadId: null, threadTitle: null, workspaceId: 'web', workspaceLabel: 'Web app',
  workspaceScope: 'all', workspaceIds: [], label: null, threadAccess: 'control', workspaceAccess: 'read', canCreateThreads: false,
  createdAt: 'now', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [], pending: true,
} as RelayAccessGrantDto;
// A thread share is repeated in sharedThreadsWithMe without a grant scope.
const share = { ...grant, id: 's1', threadId: 't1', threadTitle: 'Fix login', scope: undefined } as unknown as RelaySessionShareDto;
const portal = (rows: { grant?: Partial<RelayAccessGrantDto> | null }) => ({
  user: { id: 'me' }, devices: [], sharedByMe: [], sharedWithMe: [share], sharedDevicesWithMe: [],
  sharedThreadsWithMe: [share, ...(rows.grant === null ? [] : [{ ...grant, ...rows.grant }])],
}) as unknown as RelayPortalSummaryDto;

function Where() {
  const location = useLocation();
  return <p data-testid="where">{location.pathname + location.search}</p>;
}
function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes><Route path="/shared-pending" element={<SharePendingPage />} /><Route path="*" element={<Where />} /></Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  state.portal.mockReset();
  state.account.mockResolvedValue({ accountKeyId: 'key' });
});
afterEach(() => vi.useRealTimers());

it('shows who shared what and the recipient’s own code, then opens the share once confirmed', async () => {
  state.portal.mockResolvedValue(portal({}));
  renderAt('/shared-pending?kind=grant&id=g1');
  expect(await screen.findByRole('heading', { name: 'Waiting for alwyn to confirm' })).toBeVisible();
  expect(screen.getByText('alwyn shared the workspace “Web app” on Laptop with you.')).toBeVisible();
  expect(screen.getByText('VDNZ DB8Z 3HKC')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Copy safety code' })).toBeVisible();
  expect(screen.getByText(/Send it to alwyn another way/)).toBeVisible();
  expect(screen.getByText('This page opens the share as soon as alwyn confirms.')).toHaveAttribute('role', 'status');
  state.portal.mockResolvedValue(portal({ grant: { pending: false } }));
  await act(async () => { await vi.advanceTimersByTimeAsync(4100); });
  expect(await screen.findByRole('heading', { name: 'Confirmed. Opening…' })).toBeVisible();
  await act(async () => { await vi.advanceTimersByTimeAsync(800); });
  expect(screen.getByTestId('where')).toHaveTextContent('/devices/dev/threads?workspaceId=web');
});

it('names a thread share from the relay’s repeated rows, and says when a share is gone', async () => {
  state.portal.mockResolvedValue(portal({}));
  const view = renderAt('/shared-pending?kind=share&id=s1');
  expect(await screen.findByText('alwyn shared the thread “Fix login” on Laptop with you.')).toBeVisible();
  view.unmount();
  state.portal.mockResolvedValue(portal({ grant: null }));
  renderAt('/shared-pending?kind=grant&id=g1');
  expect(await screen.findByText(/This share is no longer available/)).toBeVisible();
});

it('sends a recipient without an account key to secure it first', async () => {
  state.account.mockResolvedValue(null);
  state.portal.mockResolvedValue(portal({}));
  renderAt('/shared-pending?kind=grant&id=g1');
  expect(await screen.findByText(/You don't have a safety code yet/)).toBeVisible();
  expect(screen.getByRole('link', { name: 'Account settings' })).toHaveAttribute('href', '/relay-account');
});
