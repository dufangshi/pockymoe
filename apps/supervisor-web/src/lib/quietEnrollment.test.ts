import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { currentAccount } from './accountVault';
import { isAccountUnlocked } from './accountVaultState';
import { request } from './api';
import { completeDeviceEnrollment, ENROLLMENT_CHANGED_EVENT, listPendingEnrollments, type PendingEnrollment } from './deviceEnrollment';
import { startQuietEnrollment } from './quietEnrollment';

vi.mock('./accountVault', () => ({ currentAccount: vi.fn() }));
vi.mock('./accountVaultState', () => ({ isAccountUnlocked: vi.fn() }));
vi.mock('./api', () => ({ request: vi.fn() }));
vi.mock('./deviceEnrollment', () => ({
  completeDeviceEnrollment: vi.fn(),
  listPendingEnrollments: vi.fn(),
  ENROLLMENT_CHANGED_EVENT: 'pockymoe-enrollment-changed',
}));

const account = { userId: 'u', accountKeyId: 'a', accountPublicKey: 'p' };
const pending = (deviceId: string, extra: Partial<PendingEnrollment> = {}): PendingEnrollment =>
  ({ deviceId, userId: 'u', accountKeyId: 'a', createdAt: 1, enrollSecret: 's', ...extra });
let stop: () => void;
beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(isAccountUnlocked).mockReturnValue(true);
  vi.mocked(listPendingEnrollments).mockResolvedValue([pending('new')]);
  vi.mocked(request).mockResolvedValue({ connected: true });
  vi.mocked(completeDeviceEnrollment).mockResolvedValue(true);
  stop = startQuietEnrollment();
});
afterEach(() => { stop(); vi.useRealTimers(); });

it('finishes a connected device while the account is unlocked, and only in quiet mode', async () => {
  await vi.advanceTimersByTimeAsync(5000);
  expect(request).toHaveBeenCalledWith('relay/devices/new/presence', { cache: 'no-store' });
  expect(completeDeviceEnrollment).toHaveBeenCalledExactlyOnceWith(pending('new'), true, undefined, { quiet: true });
});

it('does nothing at all while the account is locked: no read, no request, no prompt', async () => {
  vi.mocked(isAccountUnlocked).mockReturnValue(false);
  await vi.advanceTimersByTimeAsync(30_000);
  window.dispatchEvent(new Event(ENROLLMENT_CHANGED_EVENT));
  await vi.advanceTimersByTimeAsync(0);
  expect(listPendingEnrollments).not.toHaveBeenCalled();
  expect(request).not.toHaveBeenCalled();
  expect(completeDeviceEnrollment).not.toHaveBeenCalled();
});

it('waits for the device to connect and leaves remote protection to its own flow', async () => {
  vi.mocked(listPendingEnrollments).mockResolvedValue([pending('offline'), pending('remote', { remoteProtection: { phase: 'queued' } as never })]);
  vi.mocked(request).mockResolvedValue({ connected: false });
  await vi.advanceTimersByTimeAsync(5000);
  expect(request).toHaveBeenCalledExactlyOnceWith('relay/devices/offline/presence', { cache: 'no-store' });
  expect(completeDeviceEnrollment).not.toHaveBeenCalled();
});

it('survives a device that needs the reader and stops when asked', async () => {
  vi.mocked(completeDeviceEnrollment).mockRejectedValue(new Error('identity changed'));
  await vi.advanceTimersByTimeAsync(5000);
  expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
  stop();
  await vi.advanceTimersByTimeAsync(20_000);
  expect(completeDeviceEnrollment).toHaveBeenCalledTimes(1);
});
