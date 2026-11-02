import { translate } from '@pockymoe/thread-ui/i18n';
import { enableRelayMode, request, startOAuthSignUp } from '../lib/api';
import { relayUrl } from '../lib/relayOrigin';
import { connectedMobileHost, initializeMobileConnection } from './connection';
import { appUrl } from '../lib/webBase';
import { providerLabel, type OAuthProviderName } from '../lib/signInOutcome';

/** System-browser cookies never cross into the WebView. Redeem the one-use
 * return with a proof kept here, so intercepting a deep link grants no session. */
export async function openOAuth(provider: OAuthProviderName, inviteCode?: string) {
  const label = providerLabel(provider);
  await initializeMobileConnection();
  const host = connectedMobileHost();
  if (!host) {
    window.location.assign(inviteCode === undefined ? relayUrl(`relay/auth/oauth/${provider}/start`) : await startOAuthSignUp(provider, inviteCode));
    return;
  }
  const auth = host.auth;
  if (!auth) throw new Error(translate('auth.ui.oauthFailed', { provider: label }));
  enableRelayMode();
  const proof = Array.from(crypto.getRandomValues(new Uint8Array(32)), b => b.toString(16).padStart(2, '0')).join('');
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(proof));
  const proofHash = btoa(String.fromCharCode(...new Uint8Array(digest))).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
  const prepared = await request<{ path: string; callbackUrl: string }>('relay/auth/mobile/prepare', { method: 'POST', body: JSON.stringify({ provider, proofHash, inviteCode }) }, { auth: 'none' });
  const start = new URL(relayUrl(prepared.path), location.origin);
  const id = start.searchParams.get('mobileId');
  const expectedStart = new URL(relayUrl(`relay/auth/oauth/${provider}/start`), location.origin);
  if (!id || start.origin !== expectedStart.origin || start.pathname !== expectedStart.pathname) throw new Error(translate('auth.ui.oauthExpired', { provider: label }));
  const result = await auth.openExternal(start.toString());
  if (!result) throw new Error(translate('auth.ui.oauthCancelled', { provider: label }));
  const callback = new URL(result.callbackUrl);
  const expected = new URL(prepared.callbackUrl);
  if (callback.protocol !== expected.protocol || callback.host !== expected.host || callback.pathname !== expected.pathname || !callback.searchParams.get('code') || callback.searchParams.get('code')?.length !== 43) throw new Error(translate('auth.ui.oauthExpired', { provider: label }));
  const outcome = await request<{ path?: string }>('relay/auth/mobile/exchange', { method: 'POST', body: JSON.stringify({ code: callback.searchParams.get('code'), proof }) }, { auth: 'none' });
  const path = outcome.path ?? '/relay-portal';
  if (new URL(path, location.origin).pathname !== '/relay-portal' || !path.startsWith('/relay-portal')) throw new Error(translate('auth.ui.oauthFailed', { provider: label }));
  // The existing portal restores the destination and resumes MFA/setup from
  // its HttpOnly challenge/session cookie, without trusting URL credentials.
  window.location.assign(appUrl(path));
}
