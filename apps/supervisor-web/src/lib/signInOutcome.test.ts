import { beforeAll, describe, expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { accountRefusal, readOAuthReturn, withoutOAuthReturn } from './signInOutcome';

beforeAll(() => setLocale('en', false));

describe('sign-in outcomes', () => {
  it('words every OAuth return code with its provider', () => {
    const cases: Array<[string, 'info' | 'warning', string]> = [
      ['registration_pending', 'info', "Your account is waiting for an administrator's approval."],
      ['registration_rejected', 'warning', 'Your registration was not approved. Contact the relay administrator.'],
      ['account_disabled', 'warning', 'This account is disabled. Contact the relay administrator.'],
      ['registration_closed', 'warning', 'No account for this GitHub account, and this relay is not accepting new accounts.'],
      ['email_in_use', 'warning', 'An account with this GitHub email already exists. Sign in the way you did before.'],
      ['oauth_cancelled', 'info', 'GitHub sign-in was cancelled.'],
      ['oauth_expired', 'warning', 'The GitHub sign-in expired or was started in another browser. Try again.'],
      ['oauth_failed', 'warning', 'GitHub sign-in did not finish. Try again.'],
    ];
    for (const [code, tone, text] of cases) {
      const result = readOAuthReturn(`?oauthCode=${code}&oauthProvider=github&oauthError=ignored`);
      expect(result, code).toEqual({ notice: { tone, text } });
    }
  });

  it('sends a missing invite code to Create account with the provider', () => {
    expect(readOAuthReturn('?oauthCode=invite_required&oauthProvider=google')).toEqual({
      notice: { tone: 'info', text: 'No account for this Google account yet. Enter an invite code and continue.' },
      register: { provider: 'google', inviteInvalid: false },
    });
    expect(readOAuthReturn('?oauthCode=invalid_invite_code&oauthProvider=github')?.register).toEqual({ provider: 'github', inviteInvalid: true });
    // Without a known provider there is nothing to continue with.
    expect(readOAuthReturn('?oauthCode=invite_required')?.register).toBeUndefined();
  });

  it('keeps an older relay message, a fresh request notice and no return at all', () => {
    expect(readOAuthReturn('?oauthError=Something+else.')).toEqual({ notice: { tone: 'warning', text: 'Something else.' } });
    expect(readOAuthReturn('?oauthCode=new_code')?.notice.text).toBe('Google or GitHub sign-in did not finish. Try again.');
    expect(readOAuthReturn('?oauthPending=1')?.notice.tone).toBe('info');
    expect(readOAuthReturn('?returnTo=%2Fx')).toBeNull();
    expect(withoutOAuthReturn('?returnTo=%2Fx&oauthCode=a&oauthProvider=google&oauthError=b&oauthPending=1')).toBe('?returnTo=%2Fx');
    expect(withoutOAuthReturn('?oauthCode=a')).toBe('');
  });

  it('names only the account states the relay tells an owner', () => {
    expect(accountRefusal('registration_pending')?.tone).toBe('info');
    expect(accountRefusal('account_disabled')?.tone).toBe('warning');
    expect(accountRefusal('unauthorized')).toBeNull();
    expect(accountRefusal(undefined)).toBeNull();
  });
});
