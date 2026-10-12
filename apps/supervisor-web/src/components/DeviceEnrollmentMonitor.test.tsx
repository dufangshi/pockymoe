import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import vectors from '../lib/fixtures/deviceEnrollmentVectors.json';
import { currentAccount } from '../lib/accountVault';
import { fetchRelayPortal } from '../lib/api';
import {
  completeDeviceEnrollment,
  listPendingEnrollments,
  EnrollmentIdentityChangeError,
} from '../lib/deviceEnrollment';
import { DeviceTrustError } from '../lib/deviceTrust';
import { clearTransportKeyCache } from '../lib/relayTransportCrypto';
import { DeviceEnrollmentMonitor } from './DeviceEnrollmentMonitor';
import { runRemoteProtection } from '../lib/remoteDeviceProtection';
vi.mock('../lib/remoteDeviceProtection', () => ({ runRemoteProtection: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentAccount: vi.fn() }));
vi.mock('../lib/api', () => ({ fetchRelayPortal: vi.fn() }));
vi.mock('../lib/deviceEnrollment', async (original) => ({
  ...(await original<typeof import('../lib/deviceEnrollment')>()),
  ENROLLMENT_CHANGED_EVENT: 'pockymoe-enrollment-changed',
  completeDeviceEnrollment: vi.fn(),
  listPendingEnrollments: vi.fn(),
  deviceIdentityFingerprint: vi.fn(async (key: string) =>
    key === oldIdentityKey ? oldFingerprint : newFingerprint,
  ),
}));
vi.mock('../lib/deviceTrust', async (original) => ({
  ...(await original<typeof import('../lib/deviceTrust')>()),
}));
vi.mock('../lib/relayTransportCrypto', () => ({ exchange: vi.fn(), clearTransportKeyCache: vi.fn() }));
const oldIdentityKey = vectors.negative.deviceCertWrongIdentity.identityKey;
const newIdentityKey = vectors.deviceCertificate.identityKey;
const oldFingerprint = 'SHA-256 IXGVb6Gh9VKTIP5kn0YmMh5gw1c1sVKEambZWuXBISQ';
const newFingerprint = 'SHA-256 GPl4rTU9tmxjC0ytioq85DBp75cqabZeAdUhx67JhXQ';
const account = {
  userId: 'user',
  accountKeyId: 'account',
  accountPublicKey: 'public',
};
const pending = {
  ...account,
  deviceId: 'device',
  createdAt: Date.now(),
  enrollSecret: 'secret',
};
const portal = (connected: boolean) =>
  ({ devices: [{ id: 'device', name: 'Build workstation', connected }] }) as never;
beforeEach(() => {
  vi.clearAllMocks();
  setLocale('en');
  vi.useFakeTimers();
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(listPendingEnrollments).mockResolvedValue([pending]);
  vi.mocked(fetchRelayPortal).mockResolvedValue(portal(true));
  vi.mocked(completeDeviceEnrollment).mockResolvedValue(true);
});
afterEach(() => vi.useRealTimers());
async function tick(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
describe('automatic enrollment while the browser is open', () => {
  it('backs off remote-bind races and offline failures even when progress events wake the in-flight monitor', async () => {
    const remote = { ...pending, remoteProtection: { phase: 'binding' as const, bindStarted: true } };
    vi.mocked(listPendingEnrollments).mockResolvedValue([remote]);
    let attempts = 0;
    vi.mocked(runRemoteProtection).mockImplementation(async () => {
      if (++attempts >= 3) return true;
      window.dispatchEvent(new Event('pockymoe-enrollment-changed'));
      throw new DeviceTrustError(attempts === 1 ? 'already_bound' : 'device_offline', attempts === 1 ? 409 : 503);
    });
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(runRemoteProtection).toHaveBeenCalledTimes(1);
    await tick(15000);
    expect(runRemoteProtection).toHaveBeenCalledTimes(2);
    await tick(15000);
    expect(runRemoteProtection).toHaveBeenCalledTimes(2);
    await tick(15000);
    expect(runRemoteProtection).toHaveBeenCalledTimes(3);
    expect(screen.queryByRole('alert')).toBeNull();
  });
  it('resumes persisted remote-protection intent after an offline device reconnects, using the remote runner', async () => {
    const remote = { ...pending, remoteProtection: { phase: 'queued' as const } };
    vi.mocked(listPendingEnrollments).mockResolvedValue([remote]);
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(false));
    vi.mocked(runRemoteProtection).mockResolvedValue(true);
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(runRemoteProtection).not.toHaveBeenCalled();
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(true));
    await tick(15000);
    expect(runRemoteProtection).toHaveBeenCalledExactlyOnceWith(remote, true);
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
    await tick(60000);
    expect(runRemoteProtection).toHaveBeenCalledTimes(1);
  });
  it('backs off and retries registration-required failures without a manual Retry', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new DeviceTrustError('browser_registration_required', 401),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(clearTransportKeyCache).toHaveBeenCalledExactlyOnceWith(pending.deviceId);
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
    await tick(60000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
  });
  it('keeps repeated registration rejection status events from defeating backoff', async () => {
    let attempts = 0;
    vi.mocked(completeDeviceEnrollment).mockImplementation(async () => {
      if (++attempts > 5) return true;
      for (const detail of [
        { deviceId: pending.deviceId, state: 'encrypted', bound: true, certificateVerified: false, role: 'owner' },
        { deviceId: pending.deviceId, state: 'unlock-required', bound: true },
      ]) window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail }));
      throw new DeviceTrustError('browser_registration_required', 401);
    });
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).toBeNull();
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(3);
  });
  it('does not drop cached descriptors for devices without an active pending enrollment', async () => {
    vi.mocked(listPendingEnrollments).mockResolvedValue([]);
    render(<DeviceEnrollmentMonitor />);
    await tick();
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', {
      detail: { deviceId: pending.deviceId, state: 'encrypted', bound: true },
    })));
    await tick();
    expect(clearTransportKeyCache).not.toHaveBeenCalled();
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
  });
  it('queues a trust change received during an in-flight enrollment instead of losing the wake-up', async () => {
    let reject!: (error: Error) => void;
    vi.mocked(completeDeviceEnrollment).mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; }));
    render(<DeviceEnrollmentMonitor />);
    await tick();
    act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', {
      detail: { deviceId: pending.deviceId, state: 'encrypted', bound: true },
    })));
    await act(async () => reject(new DeviceTrustError('enrollment_missing', 404)));
    await tick();
    expect(clearTransportKeyCache).toHaveBeenCalledWith(pending.deviceId);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
  });

  it('waits for the bind command with bounded exponential backoff instead of showing an error', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValue(
      new DeviceTrustError('enrollment_missing', 404),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(3);
    await tick(60000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(4);
    await tick(120000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(5);
    vi.mocked(completeDeviceEnrollment).mockResolvedValue(true);
    await tick(120000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(6);
    await tick(240000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(6);
  });
  it('retries before the backoff deadline after an offline device reconnects', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValue(
      new DeviceTrustError('enrollment_missing', 404),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    await tick(15000);
    await tick(30000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(3);
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(false));
    await tick(15000);
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(true));
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(4);
  });
  it('retries when trust hints change, without repeated hints defeating backoff', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValue(
      new DeviceTrustError('enrollment_missing', 404),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    await tick(15000);
    const hint = {
      deviceId: pending.deviceId,
      state: 'encrypted',
      bound: true,
      identityKey: newIdentityKey,
    };
    act(() =>
      window.dispatchEvent(
        new CustomEvent('remote-codex-transport', { detail: hint }),
      ),
    );
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(3);
    expect(clearTransportKeyCache).toHaveBeenCalledExactlyOnceWith(pending.deviceId);
    act(() =>
      window.dispatchEvent(
        new CustomEvent('remote-codex-transport', { detail: hint }),
      ),
    );
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(3);
    expect(clearTransportKeyCache).toHaveBeenCalledTimes(1);
  });
  it('stops retries when the pending record disappears through expiry or removal', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValue(
      new DeviceTrustError('enrollment_missing', 404),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    vi.mocked(listPendingEnrollments).mockResolvedValue([]);
    await tick(240000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('alert')).toBeNull();
  });
  it('restarts a previously paused enrollment when a fresh repair command replaces it', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new Error('Enrollment refused'),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    const repair = { ...pending, enrollSecret: 'fresh-secret' };
    vi.mocked(listPendingEnrollments).mockResolvedValue([repair]);
    act(() => window.dispatchEvent(new Event('pockymoe-enrollment-changed')));
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenLastCalledWith(repair, true);
  });
  it('keeps authentication failures paused when transport hints change', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new Error('Enrollment refused'),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    act(() =>
      window.dispatchEvent(
        new CustomEvent('remote-codex-transport', {
          detail: {
            deviceId: pending.deviceId,
            state: 'encrypted',
            bound: true,
          },
        }),
      ),
    );
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('alert')).toHaveTextContent('Enrollment refused');
  });
  it('does not confirm a cached replacement dialog after the pending enrollment was removed', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new EnrollmentIdentityChangeError(pending.deviceId, oldIdentityKey, newIdentityKey),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    vi.mocked(listPendingEnrollments).mockResolvedValue([]);
    fireEvent.click(
      screen.getByRole('button', { name: 'Confirm replacement and unlock' }),
    );
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it.each(['en', 'zh-CN'] as const)('shows the device name and exact CLI fingerprints with a device comparison hint in %s', async (locale) => {
    setLocale(locale);
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(new EnrollmentIdentityChangeError(pending.deviceId, oldIdentityKey, newIdentityKey));
    render(<DeviceEnrollmentMonitor />);
    await tick();
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent('Build workstation');
    expect(dialog).toHaveTextContent(oldFingerprint);
    expect(dialog).toHaveTextContent(newFingerprint);
    expect(dialog).not.toHaveTextContent(oldIdentityKey);
    expect(dialog).not.toHaveTextContent(newIdentityKey);
    expect(dialog).toHaveTextContent('pockymoe bind --status');
    expect(dialog).toHaveTextContent('pockymoe relay-fingerprint');
    expect(dialog).toHaveTextContent(locale === 'en' ? 'Compare the new fingerprint' : '核对新指纹');
    fireEvent.click(screen.getByRole('button', { name: translate('devices.replaceIdentity') }));
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledWith(pending, true, { previousIdentityKey: oldIdentityKey, identityKey: newIdentityKey });
  });
  it('requires explicit old/new identity confirmation and passes only the shown replacement', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new EnrollmentIdentityChangeError(
        pending.deviceId,
        oldIdentityKey,
        newIdentityKey,
      ),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(screen.getByRole('dialog')).toHaveTextContent(oldFingerprint);
    expect(screen.getByRole('dialog')).toHaveTextContent(newFingerprint);
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    fireEvent.click(
      screen.getByRole('button', { name: 'Confirm replacement and unlock' }),
    );
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledWith(pending, true, {
      previousIdentityKey: oldIdentityKey,
      identityKey: newIdentityKey,
    });
  });
  it('does not replace an identity when confirmation is dismissed', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new EnrollmentIdentityChangeError(
        pending.deviceId,
        oldIdentityKey,
        newIdentityKey,
      ),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('finishes online pending enrollments without visiting the Devices page', async () => {
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(listPendingEnrollments).toHaveBeenCalledWith(account);
    expect(completeDeviceEnrollment).toHaveBeenCalledWith(pending, true);
  });
  it('waits for an offline device and retries when it connects', async () => {
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(false));
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(completeDeviceEnrollment).not.toHaveBeenCalled();
    vi.mocked(fetchRelayPortal).mockResolvedValue(portal(true));
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledWith(pending, true);
  });
  it('retries device connection failures without discarding pending enrollment', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new DeviceTrustError('device_offline', 502),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(screen.queryByRole('alert')).toBeNull();
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
  });
  it('pauses identity failures or canceled password prompts until explicit Retry', async () => {
    vi.mocked(completeDeviceEnrollment).mockRejectedValueOnce(
      new Error('Enrollment refused'),
    );
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(screen.getByRole('alert')).toHaveTextContent('Enrollment refused');
    await tick(15000);
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Retry enrollment' }));
    await tick();
    expect(completeDeviceEnrollment).toHaveBeenCalledTimes(2);
  });
  it('does not query the relay without a signed-in account and pending enrollments', async () => {
    vi.mocked(currentAccount).mockResolvedValue(null);
    render(<DeviceEnrollmentMonitor />);
    await tick();
    expect(fetchRelayPortal).not.toHaveBeenCalled();
  });
});
