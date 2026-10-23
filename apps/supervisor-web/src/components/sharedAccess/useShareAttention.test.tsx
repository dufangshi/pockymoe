import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';
import { sharedItems } from './model';

const state = vi.hoisted(() => ({ paired: vi.fn(), grants: vi.fn(), account: vi.fn(), verified: vi.fn(), reconciliation: 'active' }));
vi.mock('../../lib/sharedDeviceTrust', () => ({ isProtectedSharingDevice: state.paired }));
vi.mock('../../lib/sharingPreferences', () => ({ useRequireSafetyCode: () => true }));
vi.mock('../../lib/signedSharing', () => ({
  deviceGrants: state.grants,
  recipientAccount: state.account,
  recipientVerified: state.verified,
  reconcileGrants: (rows: unknown[]) => [{ ...(rows[0] as object), state: state.reconciliation }],
}));
import { useShareAttention } from './useShareAttention';

const share = (id: string, extra: Partial<RelaySessionShareDto> = {}) => ({
  id, ownerUserId: 'me', ownerUsername: 'me', targetUserId: `u-${id}`, targetUsername: id, deviceId: 'dev', deviceName: 'Laptop', deviceConnected: true,
  threadId: `t-${id}`, threadTitle: id, workspaceId: null, workspaceLabel: null, label: null, threadAccess: 'control', workspaceAccess: 'none',
  createdAt: 'now', revokedAt: null, expiresAt: null, lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [], ...extra,
}) as RelaySessionShareDto;
const items = (rows: RelaySessionShareDto[]) => sharedItems({ user: { id: 'me' }, devices: [], sharedWithMe: [], sharedByMe: rows } as unknown as RelayPortalSummaryDto, 'out');

beforeEach(() => {
  state.paired.mockReset().mockResolvedValue(true);
  state.grants.mockReset().mockResolvedValue([]);
  state.account.mockReset().mockResolvedValue({ accountKeyId: 'k', accountPublicKey: 'p' });
  state.verified.mockReset().mockResolvedValue(false);
  state.reconciliation = 'active';
});

it('flags a share waiting for the code, an unchecked one, and rechecks only when a share changes', async () => {
  const rows = [share('waiting', { pending: true, deviceConnected: false }), share('unchecked'), share('checked', { verifiedAt: 'then' })];
  const { result, rerender } = renderHook(({ list }) => useShareAttention(list), { initialProps: { list: items(rows) } });
  await waitFor(() => expect(result.current.attention.size).toBe(2));
  expect(result.current.attention.get('share:waiting')).toBe('pending');
  expect(result.current.attention.get('share:unchecked')).toBe('unverified');
  // An offline device's waiting share needs no device to be flagged; checked ones are quiet.
  expect(result.current.attention.has('share:checked')).toBe(false);
  const reads = state.grants.mock.calls.length;
  const before = result.current.attention;
  // A refresh with equal content: no new checks, the same map.
  rerender({ list: items(rows.map(row => ({ ...row, lastAccessedAt: 'later' }))) });
  await new Promise(done => setTimeout(done, 20));
  expect(state.grants.mock.calls.length).toBe(reads);
  expect(result.current.attention).toBe(before);
  // Approved at the relay: the flag goes.
  rerender({ list: items([share('waiting', { deviceConnected: false, verifiedAt: 'now' }), rows[1]!, rows[2]!]) });
  await waitFor(() => expect(result.current.attention.has('share:waiting')).toBe(false));
});
