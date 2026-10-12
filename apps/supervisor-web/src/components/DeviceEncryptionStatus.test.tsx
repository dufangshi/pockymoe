import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DeviceEncryptionStatus } from './DeviceEncryptionStatus';
import { probeDeviceEncryption } from '../lib/relayTransport';
import { notifyOwnerUnlocked } from '../lib/ownerUnlockRecovery';

vi.mock('../lib/relayTransport', () => ({
  getTransportStatus: () => undefined,
  probeDeviceEncryption: vi.fn().mockResolvedValue(undefined),
  trustDeviceIdentity: vi.fn(),
}));

function report(state: string, fingerprint?: string) {
  act(() => {
    window.dispatchEvent(new CustomEvent('remote-codex-transport', {
      detail: { deviceId: 'device-1', state, fingerprint, identityKey: 'replacement-key' },
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
  it('hides the healthy thread indicator but retains identity-change warnings', () => {
    render(<DeviceEncryptionStatus hideHealthy deviceId="device-1" connection={{
      loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: vi.fn(),
    }} />);
    report('encrypted', 'verified-fingerprint');
    expect(screen.queryByRole('button')).toBeNull();
    report('identity-changed', 'replacement-fingerprint');
    fireEvent.click(screen.getByRole('button', { name: /Device identity changed/ }));
    expect(screen.getByRole('button', { name: 'Trust replacement identity…' })).toBeVisible();
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

  it('connects a detached thread, but lets a blocked identity be inspected first', () => {
    const onConnect = vi.fn();
    render(<DeviceEncryptionStatus deviceId="device-1" connection={{
      loaded: false, busy: false, state: 'offline', label: 'Offline', onConnect,
    }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect thread' }));
    expect(onConnect).toHaveBeenCalledTimes(1);
    report('identity-changed', 'replacement-fingerprint');
    fireEvent.click(screen.getByRole('button', { name: /Device identity changed/ }));
    expect(screen.getByLabelText('Device fingerprint')).toHaveTextContent('replacement-fingerprint');
    expect(screen.getByRole('button', { name: 'Trust replacement identity…' })).toBeVisible();
    expect(onConnect).toHaveBeenCalledTimes(1);
  });
  it('keeps recipient trust visible and labels TOFU or invite pins without exposing owner management', () => {
    render(<DeviceEncryptionStatus hideHealthy deviceId="device-1" connection={{ loaded: true, busy: false, state: 'connected', label: 'Connected', onConnect: vi.fn() }} />);
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
      deviceId: 'device-1', state: 'encrypted', paired: true, role: 'grantee', identityTrust: 'tofu', certificateVerified: false,
    } })));
    fireEvent.click(screen.getByRole('button', { name: /identity trusted on first use/ }));
    expect(screen.getByText(/Device identity trusted on first use/)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Manage device trust' })).not.toBeInTheDocument();
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: {
      deviceId: 'device-1', state: 'encrypted', paired: true, role: 'grantee', identityTrust: 'invite', certificateVerified: false,
    } })));
    expect(screen.getByRole('button', { name: /invite identity pinned/ })).toBeVisible();
    expect(screen.getByText('Device identity pinned by the invite link')).toBeVisible();
  });

});
