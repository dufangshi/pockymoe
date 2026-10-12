import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { AccountUnlockPrompt } from './AccountUnlockPrompt';
import { readAfterOwnerUnlock } from '../lib/ownerUnlockRecovery';
const state = vi.hoisted(() => ({ unlock: vi.fn(), register: vi.fn(), current: vi.fn(), fetch: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentBrowser: state.current }));
vi.mock('../lib/accountVaultState', () => ({ ACCOUNT_UNLOCK_REQUEST: 'pockymoe-account-unlock-request', accountRequest: state.fetch,
  unlockAccount: state.unlock, registerBrowser: state.register, createVault: vi.fn(), openVault: vi.fn(),
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } },
}));
vi.mock('../lib/accountAuth', () => ({ isOAuthAccount: vi.fn(), setupAccount: vi.fn() }));
vi.mock('../lib/relayTransportCrypto', () => ({ clearTransportKeyCache: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({ probeDeviceEncryption: vi.fn().mockResolvedValue(undefined), getTransportStatus: () => ({ state: 'encrypted' }) }));
beforeEach(() => { vi.clearAllMocks(); setLocale('en'); state.fetch.mockResolvedValue({}); state.unlock.mockResolvedValue({}); state.current.mockResolvedValue({}); });
function block(revoked = false) { act(() => window.dispatchEvent(new CustomEvent('remote-codex-transport', { detail: { deviceId: 'a', state: 'unlock-required', ...(revoked ? { unlockReason: 'browser_revoked' } : {}) } }))); }
it.each(['en', 'zh-CN'] as const)('shows a resumable locked notice after Later and unlocks with a real submit in %s', async locale => {
  setLocale(locale); render(<AccountUnlockPrompt />); block();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ownerLater') }));
  expect(screen.queryByRole('dialog')).toBeNull(); expect(screen.getByRole('status')).toHaveTextContent(translate('auth.accountLocked'));
  expect(state.unlock).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  expect(screen.getByRole('dialog')).toBeVisible();
  fireEvent.change(screen.getByLabelText(translate('auth.password')), { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(screen.queryByRole('status')).toBeNull(); expect(state.unlock).toHaveBeenCalledWith('local password', false);
});
it('retains revoked wording after dismissal and lets a waiting read resolve via the notice', async () => {
  render(<AccountUnlockPrompt />);
  const read = vi.fn().mockRejectedValueOnce({ code: 'browser_revoked' }).mockResolvedValue({ response: Response.json({ ok: true }) });
  let waiting!: Promise<{ response: Response }>;
  await act(async () => { waiting = readAfterOwnerUnlock('a', read); });
  block(true); fireEvent.click(screen.getByRole('button', { name: translate('auth.ownerLater') }));
  expect(screen.getByRole('status')).toHaveTextContent(translate('auth.browser_revoked'));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  fireEvent.change(screen.getByLabelText(translate('auth.password')), { target: { value: 'local password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  await act(async () => { expect((await waiting).response.ok).toBe(true); });
  expect(read).toHaveBeenCalledTimes(2); expect(screen.queryByRole('status')).toBeNull();
});
it('rejects cancelled sensitive requests and retains the connection notice', async () => {
  render(<AccountUnlockPrompt />); const reject = vi.fn();
  act(() => window.dispatchEvent(new CustomEvent('pockymoe-account-unlock-request', { detail: { reason: 'connect', resolve: vi.fn(), reject } })));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ownerLater') }));
  expect(reject).toHaveBeenCalledWith(expect.objectContaining({ code: 'accountUnlockCancelled' })); expect(screen.getByRole('status')).toBeVisible();
});
