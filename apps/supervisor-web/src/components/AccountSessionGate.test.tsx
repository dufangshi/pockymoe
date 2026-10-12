import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { AccountSessionGate } from './AccountSessionGate';

const state = vi.hoisted(() => ({
  session: vi.fn(), logout: vi.fn(), current: vi.fn(), register: vi.fn(), revocations: vi.fn(), clear: vi.fn(),
  unlock: vi.fn(), change: vi.fn(), startOver: vi.fn(), setSession: vi.fn(), oauth: vi.fn(),
  create: vi.fn(), setup: vi.fn(), open: vi.fn(),
}));
vi.mock('../lib/api', () => ({ fetchRelaySession: state.session, relayLogout: state.logout }));
vi.mock('../lib/accountVault', () => ({ currentBrowser: state.current }));
vi.mock('../lib/accountAuth', () => ({ isOAuthAccount: state.oauth, setAccountSession: state.setSession, setupAccount: state.setup }));
vi.mock('../lib/accountStore', () => ({ accountStore: async () => 'user' }));
vi.mock('../lib/accountVaultState', () => ({
  ACCOUNT_CHANGED: 'pockymoe-account-changed',
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } },
  accountRevocations: state.revocations, changeAccountPassword: state.change, clearBrowser: state.clear,
  registerBrowser: state.register, startAccountOver: state.startOver, unlockAccount: state.unlock,
  createVault: state.create, openVault: state.open,
}));

const CODE = '0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ';
function account(fields: Record<string, unknown>) {
  return { authenticated: true, user: { id: 'user', username: 'alice' }, account: { mode: 'account', setupRequired: false, recovery: false, hasVault: true, accountKeyId: 'k', recentAuth: true, ...fields } };
}
function renderGate() {
  return render(
    <MemoryRouter initialEntries={['/relay-devices']}>
      <Routes>
        <Route element={<AccountSessionGate><p>Device content</p></AccountSessionGate>} path="/relay-devices" />
        <Route element={<p>Sign-in page</p>} path="/relay-portal" />
      </Routes>
    </MemoryRouter>,
  );
}
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en');
  state.session.mockResolvedValue(account({})); state.current.mockResolvedValue({ browserId: 'b1' });
  state.revocations.mockResolvedValue([]); state.register.mockResolvedValue({}); state.oauth.mockResolvedValue(false);
  state.unlock.mockResolvedValue({ userId: 'user', accountKeyId: 'k' }); state.logout.mockResolvedValue({});
  state.create.mockResolvedValue({ vault: { userId: 'user' }, recoveryCode: CODE }); state.setup.mockResolvedValue(undefined); state.open.mockResolvedValue({});
});

it('opens the app for a certified browser after re-registering it', async () => {
  renderGate();
  expect(await screen.findByText('Device content')).toBeVisible();
  expect(state.register).toHaveBeenCalledOnce();
});

it('asks a pre-upgrade session to sign in again before any setup, and signs out', async () => {
  state.session.mockResolvedValue(account({ mode: 'legacy', hasVault: false, setupRequired: true, recentAuth: false }));
  renderGate();
  expect(await screen.findByRole('heading', { name: translate('auth.ui.reauthTitle') })).toBeVisible();
  expect(screen.queryByLabelText(translate('auth.password'))).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.reauthAction') }));
  expect(await screen.findByText('Sign-in page')).toBeVisible();
  expect(state.logout).toHaveBeenCalledOnce();
  expect(state.create).not.toHaveBeenCalled();
});

it('shows setup when an older relay does not report recent authentication', async () => {
  state.session.mockResolvedValue(account({ mode: 'legacy', hasVault: false, setupRequired: true, recentAuth: undefined }));
  renderGate();
  expect(await screen.findByRole('heading', { name: translate('auth.ui.secureTitle') })).toBeVisible();
});

it('stores the vault before showing the recovery code and retries only the browser step', async () => {
  state.session.mockResolvedValue(account({ mode: 'oauth', hasVault: false, setupRequired: true }));
  state.oauth.mockResolvedValue(true);
  state.register.mockRejectedValueOnce(new Error('offline'));
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.unlockPasswordTitle') });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.unlockPasswordLabel')), { target: { value: 'a long unlock password' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a long unlock password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  expect(await screen.findByRole('alert')).toHaveTextContent('offline');
  expect(state.setup).toHaveBeenCalledWith({ vault: { userId: 'user' }, recoveryCode: CODE }, true);
  fireEvent.click(screen.getByRole('button', { name: translate('auth.retryConnection') }));
  const confirm = await screen.findByLabelText(translate('auth.ui.recoveryConfirm'));
  expect(state.create).toHaveBeenCalledOnce(); expect(state.setup).toHaveBeenCalledOnce();
  fireEvent.change(confirm, { target: { value: 'wxyz' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.recoverySaved') }));
  expect(await screen.findByText('Device content')).toBeVisible();
});

it('turns a stale setup session into a sign-in-again prompt instead of failing after the code', async () => {
  const { AccountTrustError } = await import('../lib/accountVaultState');
  state.session.mockResolvedValue(account({ mode: 'legacy', hasVault: false, setupRequired: true }));
  state.setup.mockRejectedValue(new AccountTrustError('reauthentication_required'));
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.secureTitle') });
  fireEvent.change(screen.getByLabelText(translate('auth.password')), { target: { value: 'a sufficiently long one' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a sufficiently long one' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  expect(await screen.findByRole('heading', { name: translate('auth.ui.reauthTitle') })).toBeVisible();
  expect(screen.queryByLabelText(translate('auth.ui.recoveryConfirm'))).toBeNull();
});

it('unlocks a browser without a key and always offers sign out', async () => {
  state.current.mockResolvedValue(null);
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.unlockTitle') });
  expect(screen.getByText(translate('auth.ui.signedInAs', { value1: 'alice' }))).toBeVisible();
  expect(screen.getByRole('button', { name: translate('auth.ui.signOut') })).toBeVisible();
  fireEvent.change(screen.getByLabelText(translate('auth.password')), { target: { value: 'my password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  expect(await screen.findByText('Device content')).toBeVisible();
  expect(state.unlock).toHaveBeenCalledWith('my password', false);
});

it('explains a removed browser from the account-signed list without registering it', async () => {
  state.revocations.mockResolvedValue([{ kind: 'browser', subjectId: 'b1' }]);
  renderGate();
  expect(await screen.findByRole('heading', { name: translate('auth.ui.removedTitle') })).toBeVisible();
  expect(state.clear).toHaveBeenCalledWith('b1'); expect(state.register).not.toHaveBeenCalled();
});

it('asks for a new password after a recovery-code unlock', async () => {
  state.current.mockResolvedValue(null);
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.unlockTitle') });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.useRecoveryCode') }));
  fireEvent.change(screen.getByLabelText(translate('auth.ui.resetWithCode')), { target: { value: CODE } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.unlock') }));
  await screen.findByRole('heading', { name: translate('auth.ui.newPasswordTitle') });
  expect(state.unlock).toHaveBeenCalledWith(CODE, true);
  fireEvent.change(screen.getByLabelText(translate('auth.newPassword')), { target: { value: 'a brand new password' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a brand new password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  expect(await screen.findByText('Device content')).toBeVisible();
  expect(state.change).toHaveBeenCalledWith('a brand new password', false);
});

it('finishes an administrator reset with the recovery code, or starts over', async () => {
  state.session.mockResolvedValue(account({ recovery: true }));
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.newPasswordTitle') });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.noRecoveryCode') }));
  expect(screen.getByRole('heading', { name: translate('auth.ui.startOverTitle') })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.cancel') }));
  fireEvent.change(screen.getByLabelText(translate('auth.ui.resetWithCode')), { target: { value: CODE } });
  fireEvent.change(screen.getByLabelText(translate('auth.newPassword')), { target: { value: 'a brand new password' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a brand new password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.resetAction') }));
  expect(await screen.findByText('Device content')).toBeVisible();
  expect(state.unlock).toHaveBeenCalledWith(CODE, true);
  expect(state.setSession).toHaveBeenCalledWith('user', expect.objectContaining({ recovery: false }));
});

it('re-evaluates after starting over', async () => {
  state.session.mockResolvedValueOnce(account({ recovery: true })).mockResolvedValue(account({ hasVault: false, setupRequired: true, mode: 'legacy' }));
  state.startOver.mockResolvedValue(undefined);
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.newPasswordTitle') });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.noRecoveryCode') }));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.startOverAction') }));
  expect(await screen.findByRole('heading', { name: translate('auth.ui.secureTitle') })).toBeVisible();
  expect(state.startOver).toHaveBeenCalledOnce();
});

it('picks up an unlock finished by the in-app prompt', async () => {
  state.current.mockResolvedValue(null);
  renderGate();
  await screen.findByRole('heading', { name: translate('auth.ui.unlockTitle') });
  state.current.mockResolvedValue({ browserId: 'b2' });
  await act(async () => { window.dispatchEvent(new Event('pockymoe-account-changed')); });
  await waitFor(() => expect(screen.getByText('Device content')).toBeVisible());
});
