import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { request } from './api';

export interface SecurityStatus {
  authenticatorEnabled: boolean;
  passkeyAvailable: boolean;
  recoveryCodesRemaining: number;
  recentlyVerified: boolean;
  passkeys: Array<{
    id: string;
    name: string;
    createdAt: number;
    lastUsedAt: number | null;
  }>;
  sessions: Array<{
    id: string;
    name: string;
    current: boolean;
    createdAt: number;
    expiresAt: number;
  }>;
  trustedBrowsers: Array<{
    id: string;
    name: string;
    createdAt: number;
    expiresAt: number;
    lastUsedAt: number;
  }>;
}
export interface LoginChallenge {
  challengeRequired: true;
  authenticator: boolean;
  passkey: boolean;
}
export interface Enrollment {
  secret: string;
  uri: string;
  qrSvg: string;
}
export type SecurityRealm = 'default' | 'relay-admin';
export const securityRequest = <T>(
  path: string,
  method = 'GET',
  body?: unknown,
  realm: SecurityRealm = 'default',
) =>
  request<T>(
    `relay/account/security${path}`,
    { method, ...(body === undefined ? {} : { body: JSON.stringify(body) }) },
    { auth: realm },
  );
export const fetchChallenge = () =>
  request<LoginChallenge>(
    'relay/auth/challenge',
    { cache: 'no-store' },
    { auth: 'none' },
  );
export const verifyLoginCode = (code: string, rememberBrowser: boolean) =>
  request(
    'relay/auth/challenge',
    { method: 'POST', body: JSON.stringify({ code, rememberBrowser }) },
    { auth: 'none' },
  );

function bytes(value: string): ArrayBuffer {
  const decoded = atob(value.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(decoded, (c) => c.charCodeAt(0)).buffer;
}
function encoded(value: ArrayBuffer | null) {
  if (!value) return null;
  return btoa(String.fromCharCode(...new Uint8Array(value)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}
function credentialJson(credential: PublicKeyCredential) {
  const response = credential.response;
  const result: Record<string, unknown> = {
    clientDataJSON: encoded(response.clientDataJSON),
  };
  if (response instanceof AuthenticatorAttestationResponse) {
    result.attestationObject = encoded(response.attestationObject);
    result.transports = response.getTransports?.() ?? [];
  } else {
    const assertion = response as AuthenticatorAssertionResponse;
    result.authenticatorData = encoded(assertion.authenticatorData);
    result.signature = encoded(assertion.signature);
    result.userHandle = encoded(assertion.userHandle);
  }
  return {
    id: credential.id,
    rawId: encoded(credential.rawId),
    type: credential.type,
    response: result,
    extensions: credential.getClientExtensionResults(),
  };
}
interface CredentialDescriptor {
  type: PublicKeyCredentialType;
  id: string;
  transports?: AuthenticatorTransport[];
}
interface CreationOptions
  extends Omit<
    PublicKeyCredentialCreationOptions,
    'challenge' | 'user' | 'excludeCredentials'
  > {
  challenge: string;
  user: Omit<PublicKeyCredentialUserEntity, 'id'> & { id: string };
  excludeCredentials?: CredentialDescriptor[];
}
interface AuthOptions
  extends Omit<
    PublicKeyCredentialRequestOptions,
    'challenge' | 'allowCredentials'
  > {
  challenge: string;
  allowCredentials?: CredentialDescriptor[];
}
export async function registerPasskey(
  name: string,
  realm: SecurityRealm = 'default',
) {
  const { challengeId, options } = await securityRequest<{
    challengeId: string;
    options: { publicKey: CreationOptions };
  }>('/passkeys/register/start', 'POST', { name }, realm);
  const key = options.publicKey;
  const credential = (await navigator.credentials.create({
    publicKey: {
      ...key,
      challenge: bytes(key.challenge),
      user: { ...key.user, id: bytes(key.user.id) },
      excludeCredentials: (key.excludeCredentials ?? []).map((c) => ({
        ...c,
        id: bytes(c.id),
      })),
    },
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error(translate("workbench.passkeySetupWasCancelled"));
  return securityRequest<{ recoveryCodes?: string[] }>(
    '/passkeys/register/finish',
    'POST',
    { challengeId, credential: credentialJson(credential) },
    realm,
  );
}
export async function authenticatePasskey(
  purpose: 'login' | 'reauth',
  rememberBrowser = false,
  realm: SecurityRealm = 'default',
) {
  const { challengeId, options } = await request<{
    challengeId: string;
    options: { publicKey: AuthOptions };
  }>(
    'relay/auth/passkey/start',
    { method: 'POST', body: JSON.stringify({ purpose }) },
    { auth: realm },
  );
  const key = options.publicKey;
  const credential = (await navigator.credentials.get({
    publicKey: {
      ...key,
      challenge: bytes(key.challenge),
      allowCredentials: (key.allowCredentials ?? []).map((c) => ({
        ...c,
        id: bytes(c.id),
      })),
    },
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error(translate("workbench.passkeyVerificationWasCancelled"));
  return request<{ verificationToken?: string }>(
    'relay/auth/passkey/finish',
    {
      method: 'POST',
      body: JSON.stringify({
        challengeId,
        purpose,
        rememberBrowser,
        credential: credentialJson(credential),
      }),
    },
    { auth: realm },
  );
}
export function securityError(error: unknown) {
  if (error instanceof DOMException && error.name === 'NotAllowedError')
    return translate("workbench.passkeyVerificationWasCancelledYouCanTry");
  return error instanceof Error
    ? error.message
    : translate("workbench.unableToUpdateSecuritySettings");
}
