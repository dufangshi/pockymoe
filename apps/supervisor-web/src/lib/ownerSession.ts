import { relayUrl } from './relayOrigin';
import { translate } from '@pockymoe/thread-ui/i18n';
import { clearStoredOwnerSession, trustStore } from './ownerTrustStore';
import {
  bytes,
  decode64,
  encode64,
  hash64,
  sessionChallenge,
  signOwnerRequest,
  verifyAssertion,
  type OwnerAssertion,
  type OwnerCredential,
  type OwnerRegistration,
} from './ownerTrustCrypto';
const nativeFetch = globalThis.fetch.bind(globalThis);
export const OWNER_LIFETIME = 3 * 24 * 60 * 60 * 1000;
export interface OwnerSession {
  privateKey: CryptoKey;
  registration: OwnerRegistration;
  sessionId: string;
}
export type OwnerErrorCode =
  | 'ownerVerificationFailed'
  | 'ownerCredentialsUnavailable'
  | 'ownerCertificateInvalid'
  | 'ownerCertificateUnavailable'
  | 'ownerIdentityMismatch'
  | 'ownerNoPasskey'
  | 'ownerCancelled'
  | 'ownerUnlockExpired'
  | 'owner_session_required'
  | 'owner_session_revoked'
  | 'owner_origin_mismatch'
  | 'ownerPairInvalid'
  | 'ownerPairExpired'
  | 'ownerPairFailed'
  | 'ownerPairUpload'
  | 'ownerTrustFailed'
  | 'ownerLastCredential'
  | 'ownerCertificateSigner';
export class OwnerTrustError extends Error {
  constructor(
    public code: OwnerErrorCode,
    public credentialId?: string | string[],
  ) {
    super(translate(`auth.${code}`));
  }
}
export async function fetchOwnerCredentials(): Promise<OwnerCredential[]> {
  const response = await nativeFetch(relayUrl('/relay/account/passkeys/public'), {
    credentials: 'include',
    cache: 'no-store',
  });
  if (!response.ok) throw new OwnerTrustError('ownerCredentialsUnavailable');
  const result = (await response.json()) as { passkeys: OwnerCredential[] };
  const records = result.passkeys;
  if (!Array.isArray(records))
    throw new OwnerTrustError('ownerCredentialsUnavailable');
  // The relay cannot choose the RP or origin for a browser assertion.
  if (
    records.some(
      (r) =>
        r.rpId !== location.hostname ||
        r.origin !== location.origin ||
        ![-7, -257].includes(r.algorithm),
    )
  )
    throw new OwnerTrustError('ownerVerificationFailed');
  return records;
}
export async function verifiedCredential(id: string) {
  return trustStore<OwnerCredential>(`credential:${id}`);
}
export async function assertOwner(
  records: OwnerCredential[],
  challenge: Uint8Array,
) {
  if (!records.length) throw new OwnerTrustError('ownerNoPasskey');
  if (
    records.some(
      (r) => r.rpId !== location.hostname || r.origin !== location.origin,
    )
  )
    throw new OwnerTrustError('ownerVerificationFailed');
  const result = (await navigator.credentials.get({
    publicKey: {
      challenge: bytes(challenge),
      rpId: location.hostname,
      userVerification: 'required',
      allowCredentials: records.map((r) => ({
        type: 'public-key',
        id: bytes(decode64(r.credentialId)),
      })),
    },
  })) as PublicKeyCredential | null;
  if (!result) throw new OwnerTrustError('ownerCancelled');
  const response = result.response as AuthenticatorAssertionResponse;
  const assertion: OwnerAssertion = {
    credentialId: encode64(result.rawId),
    authenticatorData: encode64(response.authenticatorData),
    clientDataJSON: encode64(response.clientDataJSON),
    signature: encode64(response.signature),
  };
  const credential = records.find(
    (r) => r.credentialId === assertion.credentialId,
  );
  if (!credential || !(await verifyAssertion(credential, assertion, challenge)))
    throw new OwnerTrustError('ownerVerificationFailed');
  await trustStore(`credential:${credential.credentialId}`, credential);
  return { credential, assertion };
}
export async function currentOwnerSession(): Promise<OwnerSession | undefined> {
  const session = await trustStore<OwnerSession>('session');
  if (!session || session.registration.expiresAt <= Date.now())
    return undefined;
  return session;
}
// Preparation happens before the click, so Safari sees credentials.get directly
// in the click handler (without a network/IDB/crypto await before it).
export async function prepareOwnerUnlock(credentialId?: string | string[]) {
  const all = await fetchOwnerCredentials();
  const records = credentialId
    ? all.filter((r) =>
        (Array.isArray(credentialId) ? credentialId : [credentialId]).includes(
          r.credentialId,
        ),
      )
    : all;
  if (!records.length)
    throw new OwnerTrustError(
      credentialId && all.length ? 'ownerCertificateSigner' : 'ownerNoPasskey',
    );
  const pair = await crypto.subtle.generateKey(
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign', 'verify'],
  );
  const issuedAt = Date.now();
  const fields = {
    version: 1 as const,
    sessionPublicKey: encode64(
      await crypto.subtle.exportKey('spki', pair.publicKey),
    ),
    issuedAt,
    expiresAt: issuedAt + OWNER_LIFETIME,
    nonce: encode64(crypto.getRandomValues(new Uint8Array(16))),
  };
  const challenge = await sessionChallenge(fields);
  return async () => {
    if (Math.abs(Date.now() - issuedAt) > 4 * 60 * 1000)
      throw new OwnerTrustError('ownerUnlockExpired');
    const { assertion } = await assertOwner(records, challenge);
    const session: OwnerSession = {
      privateKey: pair.privateKey,
      registration: { ...fields, assertion },
      sessionId: await hash64(decode64(fields.sessionPublicKey)),
    };
    await trustStore('session', session);
    return session;
  };
}
export async function clearOwnerSession(sessionId?: string) {
  await clearStoredOwnerSession(sessionId);
}
export async function ownerRequestHeaders(
  session: OwnerSession | undefined,
  aad: string,
  sealed: Uint8Array,
) {
  if (!session || session.registration.expiresAt <= Date.now()) return {};
  return {
    'x-rcd-session': session.sessionId,
    'x-rcd-signature': await signOwnerRequest(session.privateKey, aad, sealed),
  };
}
export async function markSessionRegistered(
  session: OwnerSession,
  deviceId: string,
) {
  // Re-read so concurrent device registrations cannot overwrite each other's key.
  const stored = await currentOwnerSession();
  if (stored?.sessionId !== session.sessionId)
    throw new OwnerTrustError('ownerUnlockExpired');
  await trustStore(`registration:${session.sessionId}:${deviceId}`, true);
}
export async function isSessionRegistered(
  session: OwnerSession,
  deviceId: string,
) {
  return !!(await trustStore<boolean>(
    `registration:${session.sessionId}:${deviceId}`,
  ));
}

export async function forgetSessionRegistration(
  session: OwnerSession,
  deviceId: string,
) {
  await trustStore(`registration:${session.sessionId}:${deviceId}`, null);
}
