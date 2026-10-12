import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type {
  RelaySessionShareDto,
  RelayPortalSummaryDto,
} from '@pockymoe/shared';
const state = vi.hoisted(() => ({
  sync: vi.fn(),
  revoke: vi.fn(),
  confirm: vi.fn(),
  unlock: vi.fn(),
  paired: vi.fn(),
  grants: vi.fn(),
  records: vi.fn(),
  verified: vi.fn(),
  fingerprint: vi.fn(),
  reconciliation: 'missing',
}));
const credential = {
  credentialId: 'AQID',
  publicKey: 'key',
  algorithm: -7,
  rpId: 'localhost',
  origin: 'http://localhost',
};
vi.mock('../lib/ownerDeviceTrust', () => ({
  hasDeviceCertificate: state.paired,
  locallyCertifiedDevices: async () => ['device'],
}));
vi.mock('../lib/ownerSession', () => ({
  prepareOwnerUnlock: async () => state.unlock,
  fetchOwnerCredentials: state.records,
}));
vi.mock('../lib/relayTransportCrypto', () => ({
  sharingFingerprint: state.fingerprint,
  clearTransportKeyCache: vi.fn(),
}));
vi.mock('../lib/signedSharing', () => ({
  confirmRecipient: state.confirm,
  recipientVerified: state.verified,
  safetyCode: async () => 'AAAA BBBB CCCC',
  recipientPasskeys: state.records,
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
    state.unlock,
    state.paired,
    state.grants,
    state.records,
    state.verified,
    state.fingerprint,
  ])
    mock.mockReset();
  state.paired.mockResolvedValue(true);
  state.records.mockResolvedValue([credential]);
  state.verified.mockResolvedValue(false);
  state.unlock.mockResolvedValue({});
  state.grants.mockResolvedValue([]);
  state.sync.mockResolvedValue({});
  state.confirm.mockResolvedValue(undefined);
  state.revoke.mockResolvedValue(undefined);
  state.fingerprint.mockResolvedValue('g'.repeat(43));
  state.reconciliation = 'missing';
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});
it('shows unverified recipient safety codes and confirms only after the owner compares them', async () => {
  render(<SignedShareDetails kind="share" row={row} />);
  expect(await screen.findByText(/AAAA BBBB CCCC/)).toBeVisible();
  expect(screen.getByText(/Unverified/)).toBeVisible();
  fireEvent.click(
    screen.getByRole('button', { name: 'I compared the safety code' }),
  );
  await waitFor(() => expect(state.confirm).toHaveBeenCalledWith([credential]));
  expect(await screen.findByText(/Verified/)).toBeVisible();
});
it.each([
  ['missing', 'Not active on device', 'Confirm with passkey'],
  ['changed', 'Recipient has new passkeys', 'Update'],
  ['different', 'Device permissions differ from this share', 'Update'],
])(
  'offers %s reconciliation with a direct passkey gesture',
  async (reconciliation, label, action) => {
    state.reconciliation = reconciliation;
    render(<SignedShareDetails kind="share" row={row} />);
    expect(await screen.findByText(label)).toBeVisible();
    const button = screen.getByRole('button', { name: action });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    expect(state.unlock).toHaveBeenCalledOnce();
    await waitFor(() => expect(state.sync).toHaveBeenCalledWith('share', row));
    expect(state.unlock.mock.invocationCallOrder[0]).toBeLessThan(
      state.sync.mock.invocationCallOrder[0]!,
    );
  },
);
it('explains offline paired sharing and missing recipient passkeys without offering an enabled grant action', async () => {
  render(
    <SignedShareDetails
      kind="share"
      row={{ ...row, deviceConnected: false }}
    />,
  );
  expect(await screen.findByText(/paired device must be online/)).toBeVisible();
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
      `http://localhost:3000/shared#v=1&device=device&identity=${'g'.repeat(43)}&thread=thread`,
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
it('refreshes the account safety code when the security panel changes passkeys', async () => {
  const rendered = render(<AccountSafetyCode revision={1} />);
  expect(await screen.findByText('AAAA BBBB CCCC')).toBeVisible();
  state.records.mockResolvedValue([]);
  rendered.rerender(<AccountSafetyCode revision={2} />);
  expect(await screen.findByText(/recipient must add a passkey/)).toBeVisible();
});

it('hides copy invite when the handshake has no certificate-verified fingerprint', async () => {
  state.fingerprint.mockRejectedValue(new Error('Certificate required'));
  render(<SignedShareDetails kind="share" row={row} />);
  await screen.findByText(/AAAA BBBB CCCC/);
  expect(screen.queryByRole('button', { name: 'Copy invite link' })).not.toBeInTheDocument();
});
