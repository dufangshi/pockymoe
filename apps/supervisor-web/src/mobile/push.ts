import { request } from '../lib/api';
import { relayUrl } from '../lib/relayOrigin';
import { mobileHost } from '../desktop/surface';
import type { PushSettings } from '../lib/relayPush';

let accountId: string | null = null;
let registration: string | null = null;
let liveActivityToken: string | undefined;
const key = () => `pockymoe.mobile-push:${new URL(relayUrl('relay/auth/session'), location.origin).origin}:${accountId}`;
export function restoreMobilePush(userId: string, settings: PushSettings) {
  if (accountId && accountId !== userId) liveActivityToken = undefined;
  accountId = userId;
  const saved = localStorage.getItem(key());
  registration = saved && settings.subscriptionIds.includes(saved) ? saved : null;
  return !!registration;
}
export function clearMobilePush() { accountId = null; registration = null; liveActivityToken = undefined; }
export async function registerMobilePush() {
  const user = accountId; if (!user) return null;
  const result = await mobileHost()?.push?.register();
  if (!result || accountId !== user) return null;
  const sentLiveToken = liveActivityToken;
  const saved = await request<{ id: string }>('relay/account/notifications/native', {
    method: 'POST', body: JSON.stringify({ nativePlatform: 'apns', appKind: result.kind, deviceToken: result.token, sandbox: result.sandbox, relayOrigin: new URL(relayUrl('relay/auth/session'), location.origin).origin, liveActivityToken: sentLiveToken }),
  });
  if (accountId !== user) return null;
  registration = saved.id; localStorage.setItem(key(), saved.id);
  // ActivityKit may rotate while the native permission request is pending.
  if (liveActivityToken && liveActivityToken !== sentLiveToken) await setLiveActivityToken(liveActivityToken);
  return saved.id;
}
export async function setLiveActivityToken(token: string) {
  liveActivityToken = token;
  // Never ask for notification permission merely because ActivityKit issued a token.
  if (registration) await request('relay/account/notifications/native/live-activity', { method: 'PUT', body: JSON.stringify({ id: registration, token }) });
}
export function mobilePushEnabled(settings: PushSettings) { return !!registration && settings.subscriptionIds.includes(registration); }
export async function disableMobilePush() {
  if (registration) await request('relay/account/notifications/native', { method: 'DELETE', body: JSON.stringify({ id: registration }) });
  localStorage.removeItem(key()); registration = null;
}
