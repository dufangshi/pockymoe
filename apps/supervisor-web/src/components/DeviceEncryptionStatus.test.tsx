import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DeviceEncryptionStatus } from './DeviceEncryptionStatus';
import { probeDeviceEncryption } from '../lib/relayTransport';
import { currentAccount, boundDevices } from '../lib/accountVault';
import { listPendingEnrollments } from '../lib/deviceEnrollment';
import { notifyOwnerUnlocked } from '../lib/ownerUnlockRecovery';

const pins = vi.hoisted(() => ({ get: vi.fn(), forget: vi.fn(), reset: vi.fn() }));
vi.mock('../lib/sharedDeviceTrust', () => ({ inviteOwnerPin: pins.get, forgetInviteOwner: pins.forget }));
vi.mock('../lib/relayTransportCrypto', async (original) => ({ ...await original<object>(), resetPinnedDevice: pins.reset }));
beforeEach(() => {
  pins.get.mockReset().mockResolvedValue(undefined);
  pins.forget.mockReset().mockResolvedValue(undefined);
  pins.reset.mockReset().mockImplementation(() => new Promise(() => {}));
});
vi.mock('../lib/relayTransport', () => ({
  getTransportStatus: () => undefined,
  probeDeviceEncryption: vi.fn().mockResolvedValue(undefined),
  trustDeviceIdentity: vi.fn(),
}));

vi.mock('../lib/accountVault', () => ({ currentAccount: vi.fn(), boundDevices: vi.fn(), verifyDeviceCertificate: vi.fn() }));
vi.mock('../lib/deviceEnrollment', async original => ({ ...await original<typeof import('../lib/deviceEnrollment')>(), listPendingEnrollments: vi.fn() }));
beforeEach(() => {
  vi.mocked(currentAccount).mockResolvedValue({ userId: 'user', accountKeyId: 'account', accountPublicKey: 'public' });
  vi.mocked(boundDevices).mockResolvedValue([]);
  vi.mocked(listPendingEnrollments).mockResolvedValue([]);
});

function report(state: string, fingerprint?: string) {
  act(() => {
    window.dispatchEvent(new CustomEvent('remote-codex-transport', {
      detail: { deviceId: 'device-1', state, paired: false, fingerprint, identityKey: 'replacement-key' },
    }));
  });
}

describe('combined device connection indicator', () => {
  it('resumes a detached connection only after its own device unlocks and leaves loaded connections alone', () => {
    const detached = vi.fn(), loaded = vi.fn();
    render(<>
      <DeviceEncryptionStatus deviceId="device-1" connection={{ loaded: false, busy: false, state: 'offline', label: 'Offline', onConnect: detached }} />
      <DeviceEncryptionStatus deviceId="device-1" connection={{ loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: loaded }} />
    </>);
    act(() => notifyOwnerUnlocked(['other-device']));
    expect(detached).not.toHaveBeenCalled();
    expect(loaded).not.toHaveBeenCalled();
    act(() => notifyOwnerUnlocked(['device-1']));
    expect(detached).toHaveBeenCalledTimes(1);
    expect(loaded).not.toHaveBeenCalled();
  });
  it('hides the healthy thread indicator but retains identity-change warnings', async () => {
    render(<DeviceEncryptionStatus hideHealthy deviceId="device-1" connection={{
      loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: vi.fn(),
    }} />);
    report('encrypted', 'verified-fingerprint');
    expect(screen.queryByRole('button')).toBeNull();
    report('identity-changed', 'replacement-fingerprint');
    fireEvent.click(screen.getByRole('button', { name: /Device identity changed/ }));
    expect(await screen.findByRole('button', { name: 'Trust replacement identity…' })).toBeVisible();
  });
  it('checks each online device without needing a previous thread visit', () => {
    vi.mocked(probeDeviceEncryption).mockClear();
    const { rerender } = render(<>
      <DeviceEncryptionStatus deviceId="mac" online />
      <DeviceEncryptionStatus deviceId="wsl" online />
      <DeviceEncryptionStatus deviceId="sleeping" online={false} />
    </>);
    expect(screen.getAllByRole('button', { name: 'Device encryption not verified' })).toHaveLength(3);
    expect(vi.mocked(probeDeviceEncryption).mock.calls).toEqual([['mac'], ['wsl']]);
    rerender(<DeviceEncryptionStatus deviceId="sleeping" online />);
    expect(probeDeviceEncryption).toHaveBeenLastCalledWith('sleeping');
  });
  it('shows encrypted identity details and warns about plaintext transport', () => {
    render(<DeviceEncryptionStatus deviceId="device-1" connection={{
      loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: vi.fn(),
    }} />);
    report('encrypted', 'verified-fingerprint');
    const button = screen.getByRole('button', { name: /Device connection encrypted/ });
    expect(button).toHaveAttribute('data-encryption', 'encrypted');
    fireEvent.click(button);
    expect(screen.getByLabelText('Device fingerprint')).toHaveTextContent('verified-fingerprint');
    fireEvent.click(screen.getByRole('button', { name: 'Close connection information' }));
    report('legacy');
    expect(button).toHaveAttribute('data-encryption', 'legacy');
    expect(button).toHaveAccessibleName(/not end-to-end encrypted/);
  });

  it('connects a detached thread, but lets a blocked identity be inspected first', async () => {
    const onConnect = vi.fn();
    render(<DeviceEncryptionStatus deviceId="device-1" connection={{
      loaded: false, busy: false, state: 'offline', label: 'Offline', onConnect,
    }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect thread' }));
    expect(onConnect).toHaveBeenCalledTimes(1);
    report('identity-changed', 'replacement-fingerprint');
    fireEvent.click(screen.getByRole('button', { name: /Device identity changed/ }));
    expect(screen.getByLabelText('Device fingerprint')).toHaveTextContent('replacement-fingerprint');
    expect(await screen.findByRole('button', { name: 'Trust replacement identity…' })).toBeVisible();
    expect(onConnect).toHaveBeenCalledTimes(1);
  });
  it('keeps recipient trust visible and labels TOFU or invite pins without exposing owner management', () => {
    render(<DeviceEncryptionStatus hideHealthy deviceId="device-1" connection={{ loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: vi.fn() }} />);
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
      deviceId: 'device-1', state: 'encrypted', paired: true, role: 'grantee', identityTrust: 'tofu', certificateVerified: false,
    } })));
    fireEvent.click(screen.getByRole('button', { name: /owner account trusted on first use/ }));
    expect(screen.getByText(/Owner account key trusted on first use/)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Manage device trust' })).not.toBeInTheDocument();
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
      deviceId: 'device-1', state: 'encrypted', paired: true, role: 'grantee', identityTrust: 'invite', certificateVerified: false,
    } })));
    expect(screen.getByRole('button', { name: /invited owner account verified/ })).toBeVisible();
    expect(screen.getByText("Device certificate verified with the invited owner's account key")).toBeVisible();
  });

});

it('offers forget pinned identity for a blocked invite and clears the pin before resetting transport identity', async () => {
  pins.get.mockResolvedValue('bogus-pin');
  render(<DeviceEncryptionStatus deviceId="device-1" />);
  report('identity-changed', 'actual-fingerprint');
  fireEvent.click(screen.getByRole('button', { name: 'Device identity changed' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Forget pinned owner account' }));
  await waitFor(() => expect(pins.reset).toHaveBeenCalledWith('device-1'));
  expect(pins.forget).toHaveBeenCalledWith('device-1');
  expect(pins.forget.mock.invocationCallOrder[0]).toBeLessThan(pins.reset.mock.invocationCallOrder[0]!);
});
it('does not offer to forget certificate-verified identity because of an obsolete invite pin', async () => {
  pins.get.mockResolvedValue('obsolete-pin');
  render(<DeviceEncryptionStatus deviceId="device-1" />);
  act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: { deviceId: 'device-1', state: 'encrypted', certificateVerified: true, paired: true } })));
  fireEvent.click(screen.getByRole('button', { name: 'Device connection encrypted' }));
  await waitFor(() => expect(pins.get).toHaveBeenCalled());
  expect(screen.queryByRole('button', { name: 'Forget pinned owner account' })).not.toBeInTheDocument();
});
it('lets a grantee forget the invite account pin even after the foreign device certificate verifies', async () => {
  pins.get.mockResolvedValue('confirmed-owner-pin');
  render(<DeviceEncryptionStatus deviceId="device-1" />);
  act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
    deviceId: 'device-1', state: 'encrypted', certificateVerified: true, role: 'grantee', identityTrust: 'invite',
  } })));
  fireEvent.click(screen.getByRole('button', { name: /invited owner account verified/ }));
  fireEvent.click(await screen.findByRole('button', { name: 'Forget pinned owner account' }));
  await waitFor(() => expect(pins.forget).toHaveBeenCalledWith('device-1'));
  expect(pins.reset).toHaveBeenCalledWith('device-1');
});
