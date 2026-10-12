import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { AccountUnlockPrompt } from './AccountUnlockPrompt';
import { readAfterOwnerUnlock } from '../lib/ownerUnlockRecovery';
const state = vi.hoisted(() => ({ unlock: vi.fn(), register: vi.fn(), current: vi.fn(), fetch: vi.fn(), session: vi.fn(), change: vi.fn(), setSession: vi.fn(), account: vi.fn(), create: vi.fn(), open: vi.fn(), setup: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentAccount: state.account, currentBrowser: state.current }));
vi.mock('../lib/accountVaultState', () => ({ ACCOUNT_UNLOCK_REQUEST: 'pockymoe-account-unlock-request', accountRequest: state.fetch,
  unlockAccount: state.unlock, registerBrowser: state.register, createVault: state.create, openVault: state.open, changeAccountPassword: state.change, startAccountOver: vi.fn(),
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } },
}));
vi.mock('../lib/accountAuth', () => ({ isOAuthAccount: vi.fn(), accountSessionState: state.session, setAccountSession: state.setSession, setupAccount: state.setup }));
vi.mock('../lib/accountStore', () => ({ accountStore: async () => 'user' }));
vi.mock('../lib/relayTransportCrypto', () => ({ clearTransportKeyCache: vi.fn() }));
vi.mock('../lib/relayTransport', () => ({ probeDeviceEncryption: vi.fn().mockResolvedValue(undefined), getTransportStatus: () => ({ state: 'encrypted' }) }));
beforeEach(() => { vi.clearAllMocks(); setLocale('en'); state.fetch.mockResolvedValue({}); state.unlock.mockResolvedValue({}); state.current.mockResolvedValue({}); state.account.mockResolvedValue(null); state.session.mockResolvedValue(undefined); state.change.mockResolvedValue(undefined); state.setup.mockResolvedValue(undefined); state.register.mockResolvedValue({}); state.open.mockResolvedValue({ userId: 'user' }); });
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

it('forces recovery-code unlock and a new password for a restricted reset session', async () => {
  state.session.mockResolvedValue({ mode: 'account', recovery: true, hasVault: true });
  state.unlock.mockResolvedValue({ userId: 'user' });
  render(<AccountUnlockPrompt />); block();
  const recovery = await screen.findByLabelText(translate('auth.accountRecoveryCode'));
  fireEvent.change(recovery, { target: { value: 'ABCD-EFGH' } });
  fireEvent.change(screen.getByLabelText(translate('auth.newPassword')), { target: { value: 'a sufficiently long new password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  await waitFor(() => expect(state.change).toHaveBeenCalledWith('a sufficiently long new password', undefined));
  expect(state.unlock).toHaveBeenCalledWith('ABCD-EFGH', true);
  expect(state.setSession).toHaveBeenCalledWith('user', expect.objectContaining({ recovery: false }));
});
it('never offers a new vault when a verified cached account vault disappears', async () => {
  state.account.mockResolvedValue({ accountKeyId: 'known' });
  state.fetch.mockRejectedValue(Object.assign(new Error('missing'), { code: 'vault_missing' }));
  // Use the exported error class so the UI recognizes the protocol code.
  const { AccountTrustError } = await import('../lib/accountVaultState');
  state.fetch.mockRejectedValue(new AccountTrustError('vault_missing'));
  render(<AccountUnlockPrompt />); block();
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.accountVaultMissing'));
  expect(screen.queryByText(translate('auth.accountSecure'))).toBeNull();
});

it('creates a legacy/OAuth vault only after recovery-code confirmation and retries registration without recreating it', async () => {
  const { AccountTrustError } = await import('../lib/accountVaultState');
  state.fetch.mockRejectedValue(new AccountTrustError('vault_missing'));
  state.current.mockResolvedValue(null);
  state.create.mockResolvedValue({ vault: { userId: 'user' }, recoveryCode: '0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ' });
  state.register.mockRejectedValueOnce(new Error('offline')).mockResolvedValue({});
  render(<AccountUnlockPrompt />); block();
  await screen.findByRole('heading', { name: translate('auth.accountSecure') });
  fireEvent.change(screen.getByLabelText(translate('auth.password')), { target: { value: 'a sufficiently long password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountSecure') }));
  const confirm = await screen.findByLabelText(translate('auth.accountConfirmRecovery'));
  expect(state.setup).not.toHaveBeenCalled();
  fireEvent.change(confirm, { target: { value: 'WXYZ' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  expect(await screen.findByRole('alert')).toHaveTextContent('offline'); expect(state.setup).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountUnlock') }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(state.setup).toHaveBeenCalledTimes(1); expect(state.create).toHaveBeenCalledTimes(1);
});
