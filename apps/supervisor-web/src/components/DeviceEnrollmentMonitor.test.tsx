import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { currentAccount } from '../lib/accountVault';
import { fetchRelayPortal } from '../lib/api';
import {
  completeDeviceEnrollment,
  listPendingEnrollments,
} from '../lib/deviceEnrollment';
import { DeviceTrustError } from '../lib/deviceTrust';
import { DeviceEnrollmentMonitor } from './DeviceEnrollmentMonitor';
vi.mock('../lib/accountVault', () => ({ currentAccount: vi.fn() }));
vi.mock('../lib/api', () => ({ fetchRelayPortal: vi.fn() }));
vi.mock('../lib/deviceEnrollment', () => ({
  ENROLLMENT_CHANGED_EVENT: 'pockymoe-enrollment-changed',
  completeDeviceEnrollment: vi.fn(),
  listPendingEnrollments: vi.fn(),
}));
vi.mock('../lib/deviceTrust', async (original) => ({
  ...(await original<typeof import('../lib/deviceTrust')>()),
}));
vi.mock('../lib/relayTransportCrypto', () => ({ exchange: vi.fn() }));
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
  ({ devices: [{ id: 'device', connected }] }) as never;
beforeEach(() => {
  vi.clearAllMocks();
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
