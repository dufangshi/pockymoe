// Sign-up with Google or GitHub on a relay that has an invite code, and the way
// an administrator who signs in at the portal reaches the admin console.
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { RelayPortalPage } from './RelayPortalPage';

const state = vi.hoisted(() => ({ session: vi.fn(), start: vi.fn(), upgrade: vi.fn(), logout: vi.fn(), login: vi.fn() }));
vi.mock('../lib/api', () => ({
  ApiError: class ApiError extends Error {
    constructor(public statusCode: number, public payload: { code: string; message: string }) { super(payload.message); }
  },
  fetchRelaySession: state.session,
  relayLogin: state.login,
  enableRelayMode: vi.fn(),
  relayLogout: state.logout,
  startOAuthSignUp: state.start,
}));
vi.mock('../lib/accountAuth', () => ({ finishAccountLogin: vi.fn(), startLegacyUpgrade: state.upgrade,
  legacyUpgradeOffer: async () => ({ otherVault: false }),
  registerAccount: vi.fn(), startAccountRecovery: vi.fn(), startTemporaryResetLogin: vi.fn(), cancelAccountLogin: vi.fn(), isOAuthAccount: vi.fn().mockResolvedValue(false) }));
vi.mock('../components/RelaySecurity', () => ({ usePendingLogin: () => useState(null), LoginVerification: () => null }));
vi.mock('../components/auth/AccountSetupFlow', () => ({ AccountSetupFlow: () => null }));
vi.mock('../lib/accountVaultState', () => ({ createVault: vi.fn(), openVault: vi.fn(), registerBrowser: vi.fn(), changeAccountPassword: vi.fn(),
  AccountTrustError: class extends Error { constructor(public code: string) { super(code); } } }));

const settings = {
  enabled: true,
  registrationPassword: null,
  registrationPasswordConfigured: true,
  approvalRequired: false,
  googleAuthEnabled: true,
  githubAuthEnabled: true,
  emailVerificationEnabled: false,
  googleAuthAvailable: true,
  githubAuthAvailable: true,
  emailVerificationAvailable: false,
};

const assign = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  setLocale('en');
  state.session.mockResolvedValue({ authenticated: false, registrationEnabled: true, registrationSettings: settings });
  vi.stubGlobal('location', { ...window.location, assign });
});
afterEach(() => vi.unstubAllGlobals());

function renderPortal() {
  return render(
    <MemoryRouter initialEntries={['/relay-portal']}>
      <Routes>
        <Route element={<RelayPortalPage />} path="/relay-portal" />
        <Route element={<p>admin console</p>} path="/relay-admin" />
      </Routes>
    </MemoryRouter>,
  );
}

it('asks for the invite code before Google sign-up and sends it with the start', async () => {
  renderPortal();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.createAccount') }));
  // The code comes first: every way to create an account needs it.
  const code = screen.getByLabelText(translate('auth.registrationCode'));
  expect(screen.getByText(translate('auth.ui.inviteCodeHint'))).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continueWithGoogle') }));
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.ui.inviteCodeFirst'));
  expect(state.start).not.toHaveBeenCalled();

  state.start.mockResolvedValue('https://accounts.example/authorize?state=x');
  fireEvent.change(code, { target: { value: ' invite-2026 ' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continueWithGoogle') }));
  await waitFor(() => expect(state.start).toHaveBeenCalledWith('google', 'invite-2026'));
  await waitFor(() => expect(assign).toHaveBeenCalledWith('https://accounts.example/authorize?state=x'));
});

it('shows a wrong invite code beside the field and stays on the page', async () => {
  const { ApiError } = await import('../lib/api');
  state.start.mockRejectedValue(new (ApiError as unknown as new (status: number, payload: { code: string; message: string }) => Error)(403, { code: 'invalid_invite_code', message: 'The invite code is not valid.' }));
  renderPortal();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.createAccount') }));
  fireEvent.change(screen.getByLabelText(translate('auth.registrationCode')), { target: { value: 'wrong' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continueWithGitHub') }));
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.invalid_invite_code'));
  expect(assign).not.toHaveBeenCalled();
});

it('keeps plain sign-in links when no invite code is set', async () => {
  state.session.mockResolvedValue({ authenticated: false, registrationEnabled: true, registrationSettings: { ...settings, registrationPasswordConfigured: false } });
  renderPortal();
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.createAccount') }));
  expect(screen.getByRole('link', { name: translate('auth.continueWithGoogle') })).toHaveAttribute('href', expect.stringContaining('relay/auth/oauth/google/start'));
  expect(screen.queryByLabelText(translate('auth.registrationCode'))).toBeNull();
});

it('links the admin console from the sign-in and offers it to a signed-in administrator', async () => {
  renderPortal();
  expect(await screen.findByRole('link', { name: translate('auth.ui.adminConsole') })).toHaveAttribute('href', '/relay-admin');
  state.session.mockResolvedValue({ authenticated: true, user: { id: 'a', username: 'opsadmin', role: 'admin' }, registrationSettings: settings });
  const { unmount } = renderPortal();
  expect(await screen.findByText(translate('auth.ui.adminSessionTitle'))).toBeVisible();
  // The session is kept: the administrator is not signed out behind its back.
  expect(state.logout).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('link', { name: translate('auth.ui.openAdminConsole') }));
  expect(await screen.findByText('admin console')).toBeVisible();
  unmount();
});

it('takes an administrator who signs in at the portal to the admin console', async () => {
  const { AccountTrustError } = await import('../lib/accountVaultState');
  state.login.mockRejectedValue(new AccountTrustError('unauthorized'));
  state.upgrade.mockResolvedValue({ session: { authenticated: true, user: { id: 'a', username: 'opsadmin', role: 'admin' } } });
  renderPortal();
  fireEvent.change(await screen.findByLabelText(translate('auth.emailOrUsername')), { target: { value: 'opsadmin' } });
  fireEvent.change(screen.getByLabelText(translate('auth.password'), { exact: true }), { target: { value: 'admin password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.signIn') }));
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.ui.legacyAction') }));
  expect(await screen.findByText('admin console')).toBeVisible();
});
