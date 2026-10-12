import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { RelayPortalPage } from './RelayPortalPage';
const state = vi.hoisted(() => ({ session: vi.fn(), login: vi.fn(), upgrade: vi.fn(), opened: vi.fn(), finish: vi.fn(), create: vi.fn(), register: vi.fn() }));
vi.mock('../lib/api', () => ({ ApiError: class extends Error {}, fetchRelaySession: state.session, relayLogin: state.login, enableRelayMode: vi.fn(), relayLogout: vi.fn() }));
vi.mock('../lib/accountAuth', () => ({ finishAccountLogin: state.finish, startLegacyUpgrade: state.upgrade,
  legacyUpgradeOffer: async () => ({ otherVault: await state.opened() }),
  registerAccount: state.register, startAccountRecovery: vi.fn(), startTemporaryResetLogin: vi.fn(), cancelAccountLogin: vi.fn(), isOAuthAccount: vi.fn().mockResolvedValue(false) }));
vi.mock('../components/RelaySecurity', () => ({ usePendingLogin: () => useState(null), LoginVerification: () => null }));
vi.mock('../components/auth/AccountSetupFlow', () => ({ AccountSetupFlow: ({ initialPassword }: { initialPassword?: string }) => <p>setup with {initialPassword}</p> }));
vi.mock('../lib/accountVaultState', () => ({ createVault: state.create, openVault: vi.fn(), registerBrowser: vi.fn(), changeAccountPassword: vi.fn(),
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } } }));
async function signInFails() {
  const { AccountTrustError } = await import('../lib/accountVaultState');
  state.login.mockRejectedValue(new AccountTrustError('unauthorized'));
  fireEvent.change(await screen.findByLabelText(translate('auth.emailOrUsername')), { target: { value: 'old-account' } });
  fireEvent.change(screen.getByLabelText(translate('auth.password'), { exact: true }), { target: { value: 'explicit legacy password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.signIn') }));
}
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en');
  state.session.mockResolvedValue({ authenticated: false, registrationEnabled: true });
  state.opened.mockResolvedValue(false);
  state.upgrade.mockResolvedValue({});
  state.finish.mockResolvedValue({ setup: true, recovery: false });
});

it('shows a clear error for a wrong password and offers the one-time upgrade inline', async () => {
  render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  await signInFails();
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.invalid_credentials'));
  expect(screen.getByText(translate('auth.ui.legacyTitle'))).toBeVisible();
  state.session.mockResolvedValue({ authenticated: true, user: { id: 'u', username: 'old-account' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.legacyAction') }));
  await waitFor(() => expect(state.upgrade).toHaveBeenCalledWith('old-account', 'explicit legacy password', false));
  // The account is secured right away with the password just used.
  expect(await screen.findByText('setup with explicit legacy password')).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('requires an explicit acknowledgement when this browser used an upgraded account (%s)', async locale => {
  setLocale(locale); state.opened.mockResolvedValue(true);
  render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  await signInFails();
  const action = await screen.findByRole('button', { name: translate('auth.ui.legacyAction') });
  expect(screen.getByText(translate('auth.ui.legacyOtherAccount'))).toBeVisible();
  expect(action).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox', { name: translate('auth.accountUpgradeExistingVaultConfirm') }));
  expect(action).toBeEnabled(); fireEvent.click(action);
  await waitFor(() => expect(state.upgrade).toHaveBeenCalledWith('old-account', 'explicit legacy password', true));
});

it('stores a new account with its vault before showing the recovery code', async () => {
  const code = '0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ';
  state.create.mockResolvedValue({ vault: { userId: 'new' }, recoveryCode: code });
  state.register.mockResolvedValue({ pendingApproval: true });
  render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.createAccount') }));
  fireEvent.change(screen.getByLabelText(translate('auth.email')), { target: { value: 'new@example.test' } });
  fireEvent.change(screen.getByLabelText(translate('auth.username')), { target: { value: 'newbie' } });
  fireEvent.change(screen.getByLabelText(translate('auth.password'), { exact: true }), { target: { value: 'a long enough password' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a long enough password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.createAccount') }));
  const confirm = await screen.findByLabelText(translate('auth.ui.recoveryConfirm'));
  expect(state.register).toHaveBeenCalledOnce();
  fireEvent.change(confirm, { target: { value: 'WXYZ' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.ui.recoverySaved') }));
  expect(await screen.findByText(translate('auth.ui.pendingApproval'))).toBeVisible();
});

it('offers the upgrade only after a wrong password', async () => {
  const { AccountTrustError } = await import('../lib/accountVaultState');
  render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  await signInFails();
  await screen.findByText(translate('auth.ui.legacyTitle'));
  state.login.mockRejectedValue(new AccountTrustError('rate_limited'));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.signIn') }));
  await waitFor(() => expect(screen.queryByText(translate('auth.ui.legacyTitle'))).toBeNull());
});

it('rejects mismatched passwords before creating anything', async () => {
  render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.createAccount') }));
  fireEvent.change(screen.getByLabelText(translate('auth.email')), { target: { value: 'new@example.test' } });
  fireEvent.change(screen.getByLabelText(translate('auth.username')), { target: { value: 'newbie' } });
  fireEvent.change(screen.getByLabelText(translate('auth.password'), { exact: true }), { target: { value: 'a long enough password' } });
  fireEvent.change(screen.getByLabelText(translate('auth.ui.confirmPassword')), { target: { value: 'a different password!' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.createAccount') }));
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.ui.passwordMismatch'));
  expect(state.create).not.toHaveBeenCalled();
});
