// Owner trust protocol v1. This module is shared with the service worker.
export interface OwnerCredential {
  credentialId: string;
  publicKey: string;
  algorithm: -7 | -257;
  rpId: string;
  origin: string;
  name?: string;
}
export interface OwnerAssertion {
  credentialId: string;
  authenticatorData: string;
  clientDataJSON: string;
  signature: string;
}
export interface OwnerRegistration {
  version: 1;
  sessionPublicKey: string;
  issuedAt: number;
  expiresAt: number;
  nonce: string;
  assertion: OwnerAssertion;
}
export interface DeviceCertificate {
  version: 1;
  deviceId: string;
  identityKey: string;
  pairingId: string;
  assertion: OwnerAssertion;
}
export interface DeviceCertificateBundle {
  version: 1;
  deviceId: string;
  certificates: DeviceCertificate[];
}
export type CertificateDocument = DeviceCertificate | DeviceCertificateBundle;
export function certificateEntries(
  document: CertificateDocument,
): DeviceCertificate[] {
  return 'certificates' in document ? document.certificates : [document];
}
export const utf8 = (value: string) => new TextEncoder().encode(value);
export const bytes = (value: Uint8Array): ArrayBuffer => value.slice().buffer;
export function encode64(value: ArrayBuffer | Uint8Array) {
  const data = value instanceof Uint8Array ? value : new Uint8Array(value);
  let text = '';
  for (let i = 0; i < data.length; i += 8192)
    text += String.fromCharCode(...data.subarray(i, i + 8192));
  return btoa(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
export function decode64(value: string) {
  if (!/^[A-Za-z0-9_-]*$/.test(value)) throw new Error('Invalid base64url');
  const decoded = Uint8Array.from(
    atob(value.replace(/-/g, '+').replace(/_/g, '/')),
    (c) => c.charCodeAt(0),
  );
  if (encode64(decoded) !== value) throw new Error('Invalid base64url');
  return decoded;
}
export const hash = async (value: Uint8Array) =>
  new Uint8Array(await crypto.subtle.digest('SHA-256', bytes(value)));
export const hash64 = async (value: Uint8Array) => encode64(await hash(value));
const linesHash = (parts: (string | number)[]) => hash(utf8(parts.join('\n')));
export function sessionChallenge(
  session: Pick<
    OwnerRegistration,
    'sessionPublicKey' | 'issuedAt' | 'expiresAt' | 'nonce'
  >,
) {
  return linesHash([
    'pockymoe-owner-session-v1',
    session.sessionPublicKey,
    session.issuedAt,
    session.expiresAt,
    session.nonce,
  ]);
}
export function certificateChallenge(
  deviceId: string,
  identityKey: string,
  pairingId: string,
) {
  return linesHash([
    'pockymoe-device-cert-v1',
    deviceId,
    identityKey,
    pairingId,
  ]);
}
export function addCredentialChallenge(
  deviceId: string,
  credential: OwnerCredential,
) {
  return linesHash([
    'pockymoe-add-credential-v1',
    deviceId,
    credential.credentialId,
    credential.publicKey,
  ]);
}
// WebAuthn encodes ES256 as DER; WebCrypto expects IEEE P1363 r||s.
export function es256DerToRaw(der: Uint8Array): Uint8Array {
  let offset = 0;
  const read = () => {
    if (offset >= der.length) throw new Error('Truncated DER');
    return der[offset++]!;
  };
  if (read() !== 0x30 || read() !== der.length - 2)
    throw new Error('Invalid DER sequence');
  const raw = new Uint8Array(64);
  for (let part = 0; part < 2; part++) {
    if (read() !== 0x02) throw new Error('Invalid DER integer');
    const size = read();
    if (!size || size > 33 || offset + size > der.length)
      throw new Error('Invalid DER integer length');
    let integer = der.subarray(offset, offset + size);
    offset += size;
    if (integer[0]! & 0x80) throw new Error('Negative DER integer');
    if (integer[0] === 0 && integer.length > 1) {
      if (!(integer[1]! & 0x80)) throw new Error('Nonminimal DER integer');
      integer = integer.subarray(1);
    }
    if (integer.length > 32) throw new Error('Oversized DER integer');
    raw.set(integer, part * 32 + 32 - integer.length);
  }
  if (offset !== der.length) throw new Error('Trailing DER data');
  return raw;
}
export async function verifyAssertion(
  record: OwnerCredential,
  assertion: OwnerAssertion,
  challenge: Uint8Array,
): Promise<boolean> {
  try {
    if (assertion.credentialId !== record.credentialId) return false;
    const clientBytes = decode64(assertion.clientDataJSON);
    const client = JSON.parse(
      new TextDecoder('utf-8', { fatal: true }).decode(clientBytes),
    );
    if (
      !client ||
      client.type !== 'webauthn.get' ||
      client.challenge !== encode64(challenge) ||
      client.origin !== record.origin ||
      (client.crossOrigin !== undefined && client.crossOrigin !== false)
    )
      return false;
    const authenticator = decode64(assertion.authenticatorData);
    const rpHash = await hash(utf8(record.rpId));
    if (
      authenticator.length < 37 ||
      (authenticator[32]! & 0x05) !== 0x05 ||
      rpHash.some((v, i) => v !== authenticator[i])
    )
      return false;
    const signed = new Uint8Array(authenticator.length + 32);
    signed.set(authenticator);
    signed.set(await hash(clientBytes), authenticator.length);
    const algorithm =
      record.algorithm === -7
        ? { name: 'ECDSA', namedCurve: 'P-256' }
        : record.algorithm === -257
          ? { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }
          : null;
    if (!algorithm) return false;
    const key = await crypto.subtle.importKey(
      'spki',
      bytes(decode64(record.publicKey)),
      algorithm,
      false,
      ['verify'],
    );
    const signature =
      record.algorithm === -7
        ? es256DerToRaw(decode64(assertion.signature))
        : decode64(assertion.signature);
    return await crypto.subtle.verify(
      { name: algorithm.name, hash: 'SHA-256' },
      key,
      bytes(signature),
      bytes(signed),
    );
  } catch {
    return false;
  }
}
export async function verifyCertificate(
  certificate: DeviceCertificate,
  credential: OwnerCredential,
) {
  return (
    certificate.version === 1 &&
    verifyAssertion(
      credential,
      certificate.assertion,
      await certificateChallenge(
        certificate.deviceId,
        certificate.identityKey,
        certificate.pairingId,
      ),
    )
  );
}
export async function requestSigningInput(aad: string, sealed: Uint8Array) {
  return utf8(`pockymoe-request-v1\n${aad}\n${await hash64(sealed)}`);
}
export async function signOwnerRequest(
  privateKey: CryptoKey,
  aad: string,
  sealed: Uint8Array,
) {
  return encode64(
    await crypto.subtle.sign(
      { name: 'ECDSA', hash: 'SHA-256' },
      privateKey,
      bytes(await requestSigningInput(aad, sealed)),
    ),
  );
}
export async function pairingMac(
  secret: string,
  deviceId: string,
  pairingId: string,
  credential: OwnerCredential,
  assertion: OwnerAssertion,
) {
  const input = [
    'pockymoe-pair-v1',
    deviceId,
    pairingId,
    credential.credentialId,
    credential.publicKey,
    credential.algorithm,
    credential.rpId,
    credential.origin,
    await hash64(decode64(assertion.clientDataJSON)),
  ].join('\n');
  const key = await crypto.subtle.importKey(
    'raw',
    bytes(decode64(secret)),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
  return encode64(await crypto.subtle.sign('HMAC', key, utf8(input)));
}
