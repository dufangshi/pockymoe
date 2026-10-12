import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useDeviceProtection } from './useDeviceProtection';
import {
  accountRevocations,
  boundDevices,
  currentAccount,
} from './accountVault';
import { listPendingEnrollments } from './deviceEnrollment';
vi.mock('./accountVault', () => ({
  boundDevices: vi.fn(),
  currentAccount: vi.fn(),
  accountRevocations: vi.fn(),
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
  vi.mocked(accountRevocations).mockResolvedValue([]);
});
describe('device protection display', () => {
  it('invalidates a previously Protected label after local removal even with a stale verified transport status', async () => {
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'encrypted',
        certificateVerified: true,
        accountKeyId: account.accountKeyId,
        role: 'owner',
      }),
    );
    await waitFor(() => expect(result.current).toBe('protected'));
    vi.mocked(accountRevocations).mockResolvedValue([
      {
        version: 1,
        accountKeyId: account.accountKeyId,
        kind: 'device',
        subjectId: 'd',
        revokedAt: Date.now(),
        signature: 'signature',
      },
    ]);
    act(() =>
      window.dispatchEvent(new Event('pockymoe-account-revocations-changed')),
    );
    await waitFor(() => expect(result.current).toBe('certificate-missing'));
  });
  it('requires transport certificate verification to label Protected', async () => {
    const { result } = renderHook(() =>
      useDeviceProtection('d', {
        deviceId: 'd',
        state: 'encrypted',
        certificateVerified: true,
        accountKeyId: account.accountKeyId,
        role: 'owner',
      }),
    );
    await waitFor(() => expect(result.current).toBe('protected'));
  });
  it.each([
    {
      role: 'grantee' as const,
      accountKeyId: 'other-account',
      state: 'encrypted' as const,
    },
    {
      role: 'owner' as const,
      accountKeyId: 'other-account',
      state: 'encrypted' as const,
    },
    {
      role: undefined,
      accountKeyId: account.accountKeyId,
      state: 'encrypted' as const,
    },
    {
      role: 'owner' as const,
      accountKeyId: account.accountKeyId,
      state: 'error' as const,
    },
  ])(
    'never labels a foreign, unregistered or failed connection Protected (%j)',
    async (fields) => {
      vi.mocked(boundDevices).mockResolvedValue(['d']);
      const { result } = renderHook(() =>
        useDeviceProtection('d', {
          deviceId: 'd',
          certificateVerified: true,
          ...fields,
        } as import('./relayTransportCrypto').TransportStatus),
      );
      await waitFor(() => expect(result.current).toBe('certificate-missing'));
    },
  );
  it('leaves an enrollment pending when a relay-supplied shared account certificate verifies', async () => {
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
        certificateVerified: true,
        role: 'grantee',
        accountKeyId: 'relay-account',
      }),
    );
    await waitFor(() => expect(result.current).toBe('pending'));
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
