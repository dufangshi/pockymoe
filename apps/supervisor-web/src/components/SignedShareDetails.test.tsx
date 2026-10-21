import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import vectors from '../lib/fixtures/accountKeyVectors.json';
import type {
  RelaySessionShareDto,
  RelayPortalSummaryDto,
} from '@pockymoe/shared';
const state = vi.hoisted(() => ({
  sync: vi.fn(),
  revoke: vi.fn(),
  confirm: vi.fn(),
  paired: vi.fn(),
  grants: vi.fn(),
  account: vi.fn(),
  verified: vi.fn(),
  ownerKey: vi.fn(),
  reconciliation: 'missing',
}));
const account = { accountKeyId: vectors.account.accountKeyId, accountPublicKey: vectors.account.accountPublicKey };
vi.mock('../lib/sharedDeviceTrust', () => ({
  isProtectedSharingDevice: state.paired,
  sharingGrantDevices: async () => ['device'],
  sharingOwnerAccountKeyId: state.ownerKey,
}));
vi.mock('../lib/accountVault', () => ({ currentAccount: state.account }));
vi.mock('../lib/relayTransportCrypto', () => ({
  clearTransportKeyCache: vi.fn(),
}));
vi.mock('../lib/signedSharing', () => ({
  confirmRecipient: state.confirm,
  recipientVerified: state.verified,
  safetyCode: async () => vectors.safetyCode.display,
  recipientAccount: state.account,
  deviceGrants: state.grants,
  reconcileGrants: (rows: any[]) => [
    { ...rows[0], state: state.reconciliation },
  ],
  syncDeviceGrant: state.sync,
  deleteDeviceGrant: state.revoke,
}));
import {
  SignedShareDetails,
  OrphanDeviceGrants,
  AccountSafetyCode,
} from './SignedShareDetails';
const row = {
  id: 'share',
  deviceId: 'device',
  targetUserId: 'recipient',
  targetUsername: 'Recipient',
  threadId: 'thread',
  deviceConnected: true,
} as RelaySessionShareDto;
beforeEach(() => {
  for (const mock of [
    state.sync,
    state.revoke,
    state.confirm,
    state.paired,
    state.grants,
    state.account,
    state.verified,
    state.ownerKey,
  ])
    mock.mockReset();
  state.paired.mockResolvedValue(true);
  state.account.mockResolvedValue(account);
  state.verified.mockResolvedValue(false);
  state.grants.mockResolvedValue([]);
  state.sync.mockResolvedValue({});
  state.confirm.mockResolvedValue(undefined);
  state.revoke.mockResolvedValue(undefined);
  state.ownerKey.mockResolvedValue(account.accountKeyId);
  state.reconciliation = 'missing';
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});
it('shows unverified recipient safety codes and confirms only after the owner compares them', async () => {
  render(<SignedShareDetails kind="share" row={row} />);
  expect(await screen.findByText(/VDNZ DB8Z 3HKC/)).toBeVisible();
  expect(screen.getByText(/Unverified/)).toBeVisible();
  fireEvent.click(
    screen.getByRole('button', { name: 'I compared the safety code' }),
  );
  await waitFor(() => expect(state.confirm).toHaveBeenCalledWith(account, row.targetUserId));
  expect(await screen.findByText(/Verified/)).toBeVisible();
});
it.each([
  ['missing', 'Not active on device', 'Review and confirm'],
  ['changed', 'Recipient account key has changed', 'Update'],
  ['different', 'Device permissions differ from this share', 'Update'],
])(
  'offers %s reconciliation through the reviewed grant workflow',
  async (reconciliation, label, action) => {
    state.reconciliation = reconciliation;
    render(<SignedShareDetails kind="share" row={row} />);
    expect(await screen.findByText(label)).toBeVisible();
    const button = screen.getByRole('button', { name: action });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    await waitFor(() => expect(state.sync).toHaveBeenCalledWith('share', row));
  },
);
it('explains offline protected sharing and missing recipient account key without offering an enabled grant action', async () => {
  render(
    <SignedShareDetails
      kind="share"
      row={{ ...row, deviceConnected: false }}
    />,
  );
  // The grant button stays, disabled; its mark explains why on a tap.
  fireEvent.click(await screen.findByRole('button', { name: 'Why it is unavailable' }));
  expect(screen.getByText(/protected device must be online/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review and confirm' })).toBeDisabled();
  expect(state.grants).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Copy invite link' }),
  ).not.toBeInTheDocument();
});
it('copies a fragment-only identity-pinned thread invite', async () => {
  render(<SignedShareDetails kind="share" row={row} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Copy invite link' }));
  await waitFor(() =>
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
      `http://localhost:3000/shared#v=2&device=device&owner=${account.accountKeyId}&thread=thread`,
    ),
  );
  expect(
    await screen.findByRole('button', { name: 'Invite link copied' }),
  ).toBeVisible();
});
it('shows orphan device authorization and revokes it even when its relay row has disappeared', async () => {
  state.grants.mockResolvedValue([
    {
      grantId: 'orphan',
      relayRef: { kind: 'share', id: 'gone' },
      grantee: { label: 'Recipient' },
      expiresAt: null,
    },
  ]);
  const portal = {
    devices: [], // hidden by a compromised relay
    sharedByMe: [],
    grantsByMe: [],
  } as unknown as RelayPortalSummaryDto;
  render(<OrphanDeviceGrants portal={portal} kind="share" />);
  expect(await screen.findByText(/Still active on device/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Revoke' }));
  await waitFor(() =>
    expect(state.revoke).toHaveBeenCalledWith('device', 'orphan'),
  );
});
it('refreshes the account safety code when the security panel changes account key', async () => {
  const rendered = render(<AccountSafetyCode revision={1} />);
  expect(await screen.findByText('VDNZ DB8Z 3HKC')).toBeVisible();
  state.account.mockResolvedValue(null);
  rendered.rerender(<AccountSafetyCode revision={2} />);
  expect(await screen.findByText(/Secure your account/)).toBeVisible();
});

it('hides copy invite when the handshake has no certificate-verified fingerprint', async () => {
  state.ownerKey.mockRejectedValue(new Error('Certificate required'));
  render(<SignedShareDetails kind="share" row={row} />);
  await screen.findByText(/VDNZ DB8Z 3HKC/);
  expect(screen.queryByRole('button', { name: 'Copy invite link' })).not.toBeInTheDocument();
});

it('refreshes reconciliation and orphan inventory after account browser unlock completes', async () => {
  const portal = { devices: [], sharedByMe: [], grantsByMe: [] } as unknown as RelayPortalSummaryDto;
  render(<><SignedShareDetails kind="share" row={row} /><OrphanDeviceGrants portal={portal} kind="share" /></>);
  await screen.findByText('Not active on device');
  const loaded = state.grants.mock.calls.length;
  state.reconciliation = 'active';
  act(() => window.dispatchEvent(new CustomEvent('remote-codex-owner-unlocked', { detail: { deviceIds: ['device'] } })));
  await waitFor(() => expect(screen.queryByText('Not active on device')).not.toBeInTheDocument());
  await waitFor(() => expect(state.grants.mock.calls.length).toBeGreaterThanOrEqual(loaded + 2));
});
