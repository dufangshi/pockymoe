import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { RelayPortalPage } from './RelayPortalPage';
const state = vi.hoisted(() => ({ session: vi.fn(), login: vi.fn(), upgrade: vi.fn(), opened: vi.fn(), hint: vi.fn() }));
vi.mock('../lib/api', () => ({ ApiError: class extends Error {}, fetchRelaySession: state.session, relayLogin: state.login, enableRelayMode: vi.fn(), relayLogout: vi.fn() }));
vi.mock('../lib/accountAuth', () => ({ finishAccountLogin: vi.fn(), startLegacyUpgrade: state.upgrade,
  legacyUpgradeEnabled: () => true, legacyUpgradeMayApply: state.hint, hasOpenedAccountVault: state.opened,
  registerAccount: vi.fn(), startAccountRecovery: vi.fn(), startTemporaryResetLogin: vi.fn(), cancelAccountLogin: vi.fn(), isOAuthAccount: vi.fn() }));
vi.mock('../components/RelaySecurity', () => ({ usePendingLogin: () => useState(null), LoginVerification: () => null }));
vi.mock('../components/AccountUnlockPrompt', () => ({ ensureBrowserAccess: vi.fn(), RecoveryConfirmation: () => null }));
vi.mock('../lib/accountVaultState', () => ({ createVault: vi.fn(), openVault: vi.fn(), registerBrowser: vi.fn(), changeAccountPassword: vi.fn(), AccountTrustError: class extends Error {} }));
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en');
  state.session.mockResolvedValue({ authenticated: false, registrationEnabled: false });
  state.opened.mockResolvedValue(true); state.hint.mockResolvedValue(false);
  state.upgrade.mockRejectedValue(new Error('stop after explicit upgrade'));
});
it.each(['en', 'zh-CN'] as const)('requires the explicit plaintext upgrade warning acknowledgement in %s', async locale => {
  setLocale(locale); render(<MemoryRouter><RelayPortalPage /></MemoryRouter>);
  fireEvent.click(await screen.findByRole('button', { name: translate('auth.accountLegacyUpgrade') }));
  const confirm = await screen.findByRole('checkbox', { name: translate('auth.accountUpgradeExistingVaultConfirm') });
  expect(screen.getByRole('alert')).toHaveTextContent(translate('auth.accountUpgradeExistingVaultWarning'));
  const submit = screen.getByRole('button', { name: translate('auth.signIn') });
  expect(submit).toBeDisabled(); expect(state.upgrade).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'another-account' } });
  fireEvent.change(screen.getByLabelText(translate('auth.password'), { exact: true }), { target: { value: 'explicit legacy password' } });
  fireEvent.click(confirm); expect(submit).toBeEnabled(); fireEvent.click(submit);
  await waitFor(() => expect(state.upgrade).toHaveBeenCalledWith('another-account', 'explicit legacy password', true));
});
