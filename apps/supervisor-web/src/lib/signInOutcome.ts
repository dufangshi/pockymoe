import { translate } from '@pockymoe/thread-ui/i18n';

// Why a sign-in ended without a session, worded for the sign-in card. The
// relay sends a code: after Google or GitHub as `relay-portal?oauthCode=…
// &oauthProvider=…` (with the English `oauthError` for older Webs), after a
// password as the 403 `code`. See docs/auth-and-connectivity-modes.md.

export type OAuthProviderName = 'google' | 'github';

export interface SignInNotice {
  /** `info` for a state the reader can only wait on or chose; `warning` for a refusal. */
  tone: 'info' | 'warning';
  text: string;
}

/** What the sign-in page does with a Google/GitHub return. */
export interface OAuthReturn {
  notice: SignInNotice;
  /** Continue on Create account with this provider: it needs an invite code. */
  register?: { provider: OAuthProviderName; inviteInvalid: boolean };
}

export const OAUTH_RETURN_PARAMS = ['oauthCode', 'oauthProvider', 'oauthError', 'oauthPending'] as const;

export function providerLabel(provider: string | null | undefined) {
  if (provider === 'google') return 'Google';
  if (provider === 'github') return 'GitHub';
  return translate('auth.ui.oauthProviderFallback');
}

/** Account states the relay names at sign-in, for password and OAuth alike. */
export function accountRefusal(code: string | null | undefined): SignInNotice | null {
  switch (code) {
    case 'registration_pending': return { tone: 'info', text: translate('auth.ui.accountPendingApproval') };
    case 'registration_rejected': return { tone: 'warning', text: translate('auth.ui.registrationRejected') };
    case 'account_disabled': return { tone: 'warning', text: translate('auth.ui.accountDisabled') };
    default: return null;
  }
}

/** Reads a Google/GitHub return from the sign-in page's query; null when there is none. */
export function readOAuthReturn(search: string): OAuthReturn | null {
  const params = new URLSearchParams(search);
  if (params.has('oauthPending')) return { notice: { tone: 'info', text: translate('auth.oAuthRegistrationReceivedAnAdminMustApprove') } };
  const code = params.get('oauthCode');
  const message = params.get('oauthError');
  if (!code && !message) return null;
  const raw = params.get('oauthProvider');
  const provider: OAuthProviderName | null = raw === 'google' || raw === 'github' ? raw : null;
  const name = providerLabel(provider);
  const warning = (text: string): OAuthReturn => ({ notice: { tone: 'warning', text } });
  const refusal = accountRefusal(code);
  if (refusal) return { notice: refusal };
  switch (code) {
    case 'invite_required':
    case 'invalid_invite_code': {
      const inviteInvalid = code === 'invalid_invite_code';
      const text = translate(inviteInvalid ? 'auth.ui.oauthInviteInvalid' : 'auth.ui.oauthInviteRequired', { provider: name });
      return provider ? { notice: { tone: inviteInvalid ? 'warning' : 'info', text }, register: { provider, inviteInvalid } } : warning(text);
    }
    case 'registration_closed': return warning(translate('auth.ui.oauthRegistrationClosed', { provider: name }));
    case 'email_in_use': return warning(translate('auth.ui.oauthEmailInUse', { provider: name }));
    case 'oauth_cancelled': return { notice: { tone: 'info', text: translate('auth.ui.oauthCancelled', { provider: name }) } };
    case 'oauth_expired': return warning(translate('auth.ui.oauthExpired', { provider: name }));
    case 'oauth_failed': return warning(translate('auth.ui.oauthFailed', { provider: name }));
    // An older relay, or a code this Web does not know yet: its own words.
    default: return warning(message || translate('auth.ui.oauthFailed', { provider: name }));
  }
}

/** The query without the OAuth return, so a reload does not show it again. */
export function withoutOAuthReturn(search: string) {
  const params = new URLSearchParams(search);
  for (const key of OAUTH_RETURN_PARAMS) params.delete(key);
  const text = params.toString();
  return text ? `?${text}` : '';
}
