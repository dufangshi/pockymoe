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
  approve: vi.fn(),
  paired: vi.fn(),
  grants: vi.fn(),
  account: vi.fn(),
  verified: vi.fn(),
  ownerKey: vi.fn(),
  announce: vi.fn(),
  reconciliation: 'missing',
  requireCode: true,
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
vi.mock('../lib/sharingPreferences', () => ({ useRequireSafetyCode: () => state.requireCode }));
vi.mock('../lib/sharePending', async (actual) => ({
  ...(await actual<typeof import('../lib/sharePending')>()),
  announceSharingChanged: state.announce,
}));
vi.mock('../lib/signedSharing', () => ({
  RecipientAccountMissingError: class extends Error {},
  approveRelayShare: state.approve,
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
  deviceName: 'Laptop',
  targetUserId: 'recipient',
  targetUsername: 'Recipient',
  threadId: 'thread',
  threadAccess: 'control',
  workspaceAccess: 'none',
  deviceConnected: true,
} as RelaySessionShareDto;
const CODE = vectors.safetyCode.display;
beforeEach(() => {
  for (const mock of [
    state.sync,
    state.revoke,
    state.confirm,
    state.approve,
    state.paired,
    state.grants,
    state.account,
    state.verified,
    state.ownerKey,
    state.announce,
  ])
    mock.mockReset();
  state.paired.mockResolvedValue(true);
  state.account.mockResolvedValue(account);
  state.verified.mockResolvedValue(false);
  state.grants.mockResolvedValue([]);
  state.sync.mockResolvedValue({});
  state.confirm.mockResolvedValue(undefined);
  state.approve.mockResolvedValue({ id: 'share', pending: false, verifiedAt: 'now' });
  state.revoke.mockResolvedValue(undefined);
  state.ownerKey.mockResolvedValue(account.accountKeyId);
  state.reconciliation = 'missing';
  state.requireCode = true;
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});

function typeCode(value: string, action: string) {
  fireEvent.change(screen.getByLabelText('Safety code from Recipient'), { target: { value } });
  fireEvent.click(screen.getByRole('button', { name: action }));
}

it('waits for the recipient’s code, explains a mismatch, and approves only a match', async () => {
  render(<SignedShareDetails kind="share" row={{ ...row, pending: true }} />);
  expect(screen.getByText('Waiting for Recipient\'s safety code')).toBeVisible();
  expect(screen.getByText(/Recipient can't open this yet/)).toBeVisible();
  // The expected code is never shown: the owner types what the recipient sent.
  await screen.findByLabelText('Safety code from Recipient');
  expect(screen.queryByText(CODE)).not.toBeInTheDocument();
  typeCode('VDNZ DB8Z', 'Check and approve');
  expect(await screen.findByRole('alert')).toHaveTextContent('A safety code has 12 letters and digits.');
  typeCode('VDNZ DB8Z 3HKD', 'Check and approve');
  expect(await screen.findByRole('alert')).toHaveTextContent(/does not match Recipient's account key.*revoke the share/);
  expect(state.confirm).not.toHaveBeenCalled();
  expect(state.approve).not.toHaveBeenCalled();
  // Case, dashes and look-alike letters do not matter.
  typeCode('vdnz-db8z-3hkc', 'Check and approve');
  await waitFor(() => expect(state.approve).toHaveBeenCalledWith('share', 'share', 'vdnz-db8z-3hkc'));
  expect(state.confirm).toHaveBeenCalledWith(account, 'recipient');
  // A protected device gets its signed grant before the relay starts the share.
  expect(state.sync).toHaveBeenCalledWith('share', expect.objectContaining({ id: 'share' }));
  expect(state.sync.mock.invocationCallOrder[0]).toBeLessThan(state.approve.mock.invocationCallOrder[0]!);
  await waitFor(() => expect(state.announce).toHaveBeenCalled());
});

it('approves a pending share on an unprotected device at the relay alone', async () => {
  state.paired.mockResolvedValue(false);
  render(<SignedShareDetails kind="grant" row={{ ...row, pending: true }} />);
  await screen.findByLabelText('Safety code from Recipient');
  typeCode(CODE, 'Check and approve');
  await waitFor(() => expect(state.approve).toHaveBeenCalledWith('grant', 'share', CODE));
  expect(state.sync).not.toHaveBeenCalled();
  expect(state.grants).not.toHaveBeenCalled();
});

it('says once why the recipient cannot open a share the device has not signed, and fixes it with one action', async () => {
  render(<SignedShareDetails kind="share" row={row} />);
  expect(await screen.findByText('Recipient can\'t open this now')).toBeVisible();
  expect(screen.getByText(/Laptop is protected and only accepts shares you signed for it/)).toBeVisible();
  // Not checked in this browser and the check is on: the code signs it.
  typeCode(CODE, 'Sign onto device');
  await waitFor(() => expect(state.sync).toHaveBeenCalledWith('share', row));
  expect(state.approve).toHaveBeenCalledWith('share', 'share', CODE);
});

it.each([
  ['missing', true, true, 'Sign onto device'],
  ['missing', false, false, 'Sign onto device'],
  ['changed', false, false, 'Sign onto device'],
  ['different', false, true, 'Sign current permissions'],
])('signs a %s grant with a button when no code is needed', async (reconciliation, verified, requireCode, action) => {
  state.reconciliation = reconciliation;
  state.verified.mockResolvedValue(verified);
  state.requireCode = requireCode;
  render(<SignedShareDetails kind="share" row={row} />);
  const button = await screen.findByRole('button', { name: action });
  expect(screen.queryByLabelText('Safety code from Recipient')).not.toBeInTheDocument();
  fireEvent.click(button);
  await waitFor(() => expect(state.sync).toHaveBeenCalledWith('share', row));
  expect(state.approve).not.toHaveBeenCalled();
});

it('asks for the new code when the recipient’s key changed', async () => {
  state.reconciliation = 'changed';
  state.verified.mockResolvedValue(true);
  render(<SignedShareDetails kind="share" row={row} />);
  expect(await screen.findByText('Recipient\'s account key changed')).toBeVisible();
  expect(await screen.findByLabelText('Safety code from Recipient')).toBeVisible();
});

it('shows a checked share as one quiet line with its code', async () => {
  state.reconciliation = 'active';
  render(<SignedShareDetails kind="share" row={{ ...row, verifiedAt: '2026-10-20T00:00:00Z' }} />);
  expect(await screen.findByText('Safety code checked')).toBeVisible();
  expect(screen.getByText(CODE)).toBeVisible();
});

it('keeps the grant action on an offline protected device, disabled, with the reason behind a mark', async () => {
  render(
    <SignedShareDetails
      kind="share"
      row={{ ...row, deviceConnected: false, pending: true }}
    />,
  );
  await screen.findByLabelText('Safety code from Recipient');
  fireEvent.change(screen.getByLabelText('Safety code from Recipient'), { target: { value: CODE } });
  expect(screen.getByRole('button', { name: 'Check and approve' })).toBeDisabled();
  fireEvent.click(await screen.findByRole('button', { name: 'Why it is unavailable' }));
  expect(screen.getByText(/protected device must be online/)).toBeVisible();
  expect(state.grants).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Copy invite link' }),
  ).not.toBeInTheDocument();
});

it('does not clear or reload the details when a refresh brings an equal share', async () => {
  const rendered = render(<SignedShareDetails kind="share" row={row} />);
  expect(await screen.findByText('Recipient can\'t open this now')).toBeVisible();
  await screen.findByRole('button', { name: 'Copy invite link' });
  const loads = [state.grants.mock.calls.length, state.account.mock.calls.length, state.paired.mock.calls.length, state.ownerKey.mock.calls.length];
  // A portal refresh builds new row objects with the same content.
  for (let poll = 0; poll < 3; poll += 1) {
    rendered.rerender(<SignedShareDetails kind="share" row={{ ...row, lastAccessedAt: `2026-10-2${poll}T00:00:00Z` } as RelaySessionShareDto} />);
    expect(screen.getByText('Recipient can\'t open this now')).toBeVisible();
    expect(screen.getByLabelText('Safety code from Recipient')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy invite link' })).toBeVisible();
  }
  await act(async () => { await new Promise((done) => setTimeout(done, 20)); });
  expect([state.grants.mock.calls.length, state.account.mock.calls.length, state.paired.mock.calls.length, state.ownerKey.mock.calls.length]).toEqual(loads);
  // A real change checks again, and the last result stays until it answers.
  let answer!: (value: unknown[]) => void;
  state.grants.mockReturnValueOnce(new Promise((done) => { answer = done; }));
  rendered.rerender(<SignedShareDetails kind="share" row={{ ...row, threadAccess: 'read' }} />);
  await waitFor(() => expect(state.grants.mock.calls.length).toBe(loads[0]! + 1));
  expect(screen.getByText('Recipient can\'t open this now')).toBeVisible();
  await act(async () => answer([]));
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
  const rendered = render(<OrphanDeviceGrants portal={portal} kind="share" />);
  expect(await screen.findByText('The share with Recipient has ended, but device still holds their signed grant.')).toBeVisible();
  // An equal portal from the next refresh reads no device again.
  const loads = state.grants.mock.calls.length;
  rendered.rerender(<OrphanDeviceGrants portal={{ ...portal, devices: [] }} kind="share" />);
  await act(async () => { await new Promise((done) => setTimeout(done, 20)); });
  expect(state.grants.mock.calls.length).toBe(loads);
  fireEvent.click(screen.getByRole('button', { name: 'Revoke' }));
  await waitFor(() =>
    expect(state.revoke).toHaveBeenCalledWith('device', 'orphan'),
  );
});

it('refreshes the account safety code when the security panel changes account key', async () => {
  const rendered = render(<AccountSafetyCode revision={1} />);
  expect(await screen.findByText(CODE)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Copy safety code' })).toBeVisible();
  state.account.mockResolvedValue(null);
  rendered.rerender(<AccountSafetyCode revision={2} />);
  expect(await screen.findByText(/Secure your account/)).toBeVisible();
});

it('hides copy invite when the handshake has no certificate-verified fingerprint', async () => {
  state.ownerKey.mockRejectedValue(new Error('Certificate required'));
  render(<SignedShareDetails kind="share" row={row} />);
  await screen.findByText('Recipient can\'t open this now');
  expect(screen.queryByRole('button', { name: 'Copy invite link' })).not.toBeInTheDocument();
});

it('refreshes reconciliation and orphan inventory after account browser unlock completes', async () => {
  const portal = { devices: [], sharedByMe: [], grantsByMe: [] } as unknown as RelayPortalSummaryDto;
  render(<><SignedShareDetails kind="share" row={row} /><OrphanDeviceGrants portal={portal} kind="share" /></>);
  await screen.findByText('Recipient can\'t open this now');
  const loaded = state.grants.mock.calls.length;
  state.reconciliation = 'active';
  state.verified.mockResolvedValue(true);
  act(() => window.dispatchEvent(new CustomEvent('remote-codex-owner-unlocked', { detail: { deviceIds: ['device'] } })));
  await waitFor(() => expect(screen.queryByText('Recipient can\'t open this now')).not.toBeInTheDocument());
  expect(screen.getByText('Safety code checked')).toBeVisible();
  await waitFor(() => expect(state.grants.mock.calls.length).toBeGreaterThanOrEqual(loaded + 2));
});
