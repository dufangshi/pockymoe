import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { AccountSecurityPanel } from './AccountSecurityPanel';
import vectors from '../lib/fixtures/accountKeyVectors.json';
const state = vi.hoisted(() => ({ request: vi.fn(), revoked: vi.fn(), change: vi.fn(), recovery: vi.fn(), verify: vi.fn(), authStatus: vi.fn() }));
vi.mock('../lib/accountVault', () => ({ currentAccount: async () => vectors.account, currentBrowser: async () => ({ browserId: vectors.browserCertificate.certificate.browserId }), safetyCode: async () => vectors.safetyCode.display }));
vi.mock('../lib/accountVaultState', () => ({ accountRequest: state.request, accountRevocations: async () => [], revokeAccountSubject: state.revoked,
  changeAccountPassword: state.change, regenerateRecoveryCode: state.recovery, startAccountOver: vi.fn() }));
vi.mock('../lib/relaySecurity', () => ({ securityRequest: state.authStatus }));
vi.mock('./RelaySecurity', () => ({ SecurityVerification: ({ onVerified }: { onVerified: () => void }) => <button onClick={onVerified}>Verify factor</button> }));
vi.mock('../lib/accountAuth', () => ({ isOAuthAccount: async () => true }));
vi.mock('../lib/accountCrypto', () => ({ verifyBrowserCertificate: state.verify }));
vi.mock('./AccountUnlockPrompt', () => ({ RecoveryConfirmation: ({ code, onConfirm }: { code: string; onConfirm: () => void }) => <div>{code}<button onClick={onConfirm}>Saved code</button></div> }));
beforeEach(() => {
  vi.clearAllMocks(); setLocale('en'); state.verify.mockResolvedValue(true); state.recovery.mockResolvedValue(vectors.derivation.recoveryCodeDisplay);
  state.request.mockResolvedValue({ browsers: [{ certificate: vectors.browserCertificate.certificate, label: 'My laptop', createdAt: 1760000000000, lastSeenAt: 1760000001000 }] });
});
it.each(['en', 'zh-CN'] as const)('shows verified named browsers, own badge, times and safety code in %s', async locale => {
  setLocale(locale); render(<AccountSecurityPanel />);
  expect(await screen.findByText('My laptop')).toBeVisible(); expect(screen.getByText(translate('auth.ownerCurrent'))).toBeVisible();
  expect(screen.getByText(vectors.safetyCode.display)).toBeVisible();
  expect(state.verify).toHaveBeenCalledWith(vectors.browserCertificate.certificate, vectors.account);
});
it('rejects an invalid relay-supplied browser certificate before listing it', async () => {
  state.verify.mockResolvedValue(false); render(<AccountSecurityPanel />);
  expect(await screen.findByRole('alert')).toHaveTextContent(translate('auth.accountCertificateInvalid')); expect(screen.queryByText('My laptop')).toBeNull();
});
it('uses the shared unlock-backed password change for an OAuth-only vault', async () => {
  render(<AccountSecurityPanel />); await screen.findByText('My laptop');
  fireEvent.click(screen.getByRole('button', { name: translate('auth.changePassword') }));
  fireEvent.change(screen.getByLabelText(translate('auth.newPassword')), { target: { value: 'a sufficiently long new password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  await waitFor(() => expect(state.change).toHaveBeenCalledWith('a sufficiently long new password', true));
});
it('requires explicit confirmation to remove a browser and shows a regenerated recovery code once', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true); render(<AccountSecurityPanel />); await screen.findByText('My laptop');
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountRemoveBrowser') }));
  await waitFor(() => expect(state.revoked).toHaveBeenCalledWith('browser', vectors.browserCertificate.certificate.browserId));
  await waitFor(() => expect(screen.getByRole('button', { name: translate('auth.accountRegenerateRecovery') })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: translate('auth.accountRegenerateRecovery') }));
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  expect(await screen.findByText(vectors.derivation.recoveryCodeDisplay)).toBeVisible(); fireEvent.click(screen.getByText('Saved code'));
  expect(screen.queryByText(vectors.derivation.recoveryCodeDisplay)).toBeNull(); vi.restoreAllMocks();
});

it('retries an OAuth-sensitive change after a real second-factor confirmation', async () => {
  state.authStatus.mockResolvedValue({ authenticatorEnabled: true, passkeys: [] });
  state.change.mockRejectedValueOnce(Object.assign(new Error('Reauthenticate'), { code: 'reauthentication_required' })).mockResolvedValue(undefined);
  render(<AccountSecurityPanel />); await screen.findByText('My laptop');
  fireEvent.click(screen.getByRole('button', { name: translate('auth.changePassword') }));
  fireEvent.change(screen.getByLabelText(translate('auth.newPassword')), { target: { value: 'a sufficiently long new password' } });
  fireEvent.click(screen.getByRole('button', { name: translate('auth.continue') }));
  fireEvent.click(await screen.findByText('Verify factor'));
  await waitFor(() => expect(state.change).toHaveBeenCalledTimes(2));
  expect(state.change).toHaveBeenLastCalledWith('a sufficiently long new password', true);
});
