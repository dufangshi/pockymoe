// Test-only WebAuthn authenticator using the published interop key.
import {
  bytes,
  decode64,
  encode64,
  hash,
  utf8,
  type OwnerCredential,
} from './ownerTrustCrypto';
export function rawToDer(raw: Uint8Array) {
  const integers = [raw.slice(0, 32), raw.slice(32)].map((part) => {
    while (part.length > 1 && part[0] === 0) part = part.slice(1);
    if (part[0]! & 0x80) part = new Uint8Array([0, ...part]);
    return new Uint8Array([2, part.length, ...part]);
  });
  return new Uint8Array([
    48,
    integers[0]!.length + integers[1]!.length,
    ...integers[0]!,
    ...integers[1]!,
  ]);
}
export async function authenticatorResult(
  record: OwnerCredential,
  challenge: Uint8Array,
  key: CryptoKey,
) {
  const authenticatorData = new Uint8Array(37);
  authenticatorData.set(await hash(utf8(record.rpId)));
  authenticatorData[32] = 5;
  const clientDataJSON = utf8(
    JSON.stringify({
      type: 'webauthn.get',
      challenge: encode64(challenge),
      origin: record.origin,
    }),
  );
  const signed = new Uint8Array(69);
  signed.set(authenticatorData);
  signed.set(await hash(clientDataJSON), 37);
  const signature = rawToDer(
    new Uint8Array(
      await crypto.subtle.sign(
        { name: 'ECDSA', hash: 'SHA-256' },
        key,
        bytes(signed),
      ),
    ),
  );
  return {
    rawId: bytes(decode64(record.credentialId)),
    response: {
      authenticatorData: bytes(authenticatorData),
      clientDataJSON: bytes(clientDataJSON),
      signature: bytes(signature),
    },
  };
}
