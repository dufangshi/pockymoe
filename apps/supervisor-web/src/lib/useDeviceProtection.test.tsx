import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useDeviceProtection } from './useDeviceProtection';
import { boundDevices, currentAccount } from './accountVault';
import { listPendingEnrollments } from './deviceEnrollment';
vi.mock('./accountVault', () => ({
  boundDevices: vi.fn(),
  currentAccount: vi.fn(),
}));
vi.mock('./deviceEnrollment', async (original) => ({
  ...(await original<typeof import('./deviceEnrollment')>()),
  listPendingEnrollments: vi.fn(),
}));
const account = { userId: 'u', accountKeyId: 'a', accountPublicKey: 'public' };
beforeEach(() => {
  vi.mocked(currentAccount).mockResolvedValue(account);
  vi.mocked(boundDevices).mockResolvedValue([]);
  vi.mocked(listPendingEnrollments).mockResolvedValue([]);
});
describe('device protection display', () => {
  it('requires transport certificate verification to label Protected', async () => {
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'encrypted',
        certificateVerified: true,
      }),
    );
    await waitFor(() => expect(result.current).toBe('protected'));
  });
  it('never displays changed identity as protected even with a previously verified certificate', async () => {
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'identity-changed',
        certificateVerified: true,
      }),
    );
    await waitFor(() => expect(result.current).toBe('certificate-missing'));
  });
  it('never lets legacy or unbound hints override the opened vault', async () => {
    vi.mocked(boundDevices).mockResolvedValue(['d']);
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'legacy',
        paired: false,
      }),
    );
    await waitFor(() => expect(result.current).toBe('certificate-missing'));
  });
  it('shows pending enrollment and refreshes when its record changes', async () => {
    vi.mocked(listPendingEnrollments).mockResolvedValue([
      {
        deviceId: 'd',
        ...account,
        enrollSecret: 'secret',
        createdAt: Date.now(),
      },
    ]);
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'encrypted',
        paired: false,
      }),
    );
    await waitFor(() => expect(result.current).toBe('pending'));
    vi.mocked(listPendingEnrollments).mockResolvedValue([]);
    act(() => window.dispatchEvent(new Event('pockymoe-enrollment-changed')));
    await waitFor(() => expect(result.current).toBe('not-protected'));
  });
});
