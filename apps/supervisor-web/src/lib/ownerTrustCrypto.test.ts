// @vitest-environment node
import { describe, expect, it } from 'vitest';
import vectors from './fixtures/ownerTrustVectors.json';
import {
  addCredentialChallenge,
  bytes,
  certificateChallenge,
  decode64,
  encode64,
  es256DerToRaw,
  hash,
  hash64,
  pairingMac,
  requestSigningInput,
  sessionChallenge,
  signOwnerRequest,
  utf8,
  verifyAssertion,
  verifyCertificate,
  type OwnerCredential,
} from './ownerTrustCrypto';
const record = vectors.credentialRecord as OwnerCredential;
const registration = vectors.session.registration;
const certificate = vectors.deviceCertificate.certificate;
const assertion = registration.assertion;

describe('owner trust interop vectors', () => {
  it('matches session challenge, id, certificate challenge, MAC and signing input', async () => {
    expect(encode64(await sessionChallenge(registration))).toBe(
      vectors.session.challenge,
    );
    expect(await hash64(decode64(registration.sessionPublicKey))).toBe(
      vectors.session.sessionId,
    );
    expect(
      await verifyAssertion(
        record,
        assertion,
        await sessionChallenge(registration),
      ),
    ).toBe(true);
    expect(
      await verifyCertificate({ ...certificate, version: 1 }, record),
    ).toBe(true);
    expect(
      await certificateChallenge(
        certificate.deviceId,
        certificate.identityKey,
        certificate.pairingId,
      ),
    ).toEqual(await hash(utf8(vectors.deviceCertificate.challengeInput)));
    expect(
      await pairingMac(
        vectors.pairing.secret,
        certificate.deviceId,
        certificate.pairingId,
        record,
        certificate.assertion,
      ),
    ).toBe(vectors.pairing.mac);
    expect(
      new TextDecoder().decode(
        await requestSigningInput(
          vectors.request.aad,
          decode64(vectors.request.sealed),
        ),
      ),
    ).toBe(vectors.request.signingInput);
    expect(await addCredentialChallenge(certificate.deviceId, record)).toEqual(
      await hash(
        utf8(
          `pockymoe-add-credential-v1\n${certificate.deviceId}\n${record.credentialId}\n${record.publicKey}`,
        ),
      ),
    );
  });
  for (const [name, negative] of Object.entries(vectors.negative)) {
    it(`rejects negative vector: ${name}`, async () => {
      expect(
        await verifyAssertion(
          record,
          negative.assertion,
          await sessionChallenge(negative),
        ),
      ).toBe(false);
    });
  }
  it('verifies the supplied raw request signature and binds every input byte', async () => {
    const key = await crypto.subtle.importKey(
      'spki',
      bytes(decode64(vectors.request.sessionPublicKey)),
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['verify'],
    );
    const verify = (
      aad: string,
      sealed: Uint8Array,
      signature = vectors.request.signature,
    ) =>
      requestSigningInput(aad, sealed).then((input) =>
        crypto.subtle.verify(
          { name: 'ECDSA', hash: 'SHA-256' },
          key,
          bytes(decode64(signature)),
          bytes(input),
        ),
      );
    expect(
      await verify(vectors.request.aad, decode64(vectors.request.sealed)),
    ).toBe(true);
    expect(
      await verify(vectors.request.aad + 'x', decode64(vectors.request.sealed)),
    ).toBe(false);
    expect(await verify(vectors.request.aad, new Uint8Array([1]))).toBe(false);
    const privateKey = await crypto.subtle.importKey(
      'jwk',
      vectors.session.privateJwk,
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['sign'],
    );
    const signed = await signOwnerRequest(
      privateKey,
      vectors.request.aad,
      decode64(vectors.request.sealed),
    );
    expect(decode64(signed)).toHaveLength(64);
    expect(
      await verify(
        vectors.request.aad,
        decode64(vectors.request.sealed),
        signed,
      ),
    ).toBe(true);
  });
});
describe('assertion verification boundaries', () => {
  it('rejects a substituted credential/key, wrong challenge/RP, missing UP, short authenticator data and malformed JSON', async () => {
    const challenge = await sessionChallenge(registration);
    const other = await crypto.subtle.generateKey(
      { name: 'ECDSA', namedCurve: 'P-256' },
      true,
      ['sign', 'verify'],
    );
    expect(
      await verifyAssertion(
        {
          ...record,
          publicKey: encode64(
            await crypto.subtle.exportKey('spki', other.publicKey),
          ),
        },
        assertion,
        challenge,
      ),
    ).toBe(false);
    expect(
      await verifyAssertion(
        { ...record, credentialId: 'other' },
        assertion,
        challenge,
      ),
    ).toBe(false);
    expect(
      await verifyAssertion(
        { ...record, rpId: 'other.test' },
        assertion,
        challenge,
      ),
    ).toBe(false);
    expect(await verifyAssertion(record, assertion, new Uint8Array([1]))).toBe(
      false,
    );
    const noUp = decode64(assertion.authenticatorData);
    noUp[32] = 4;
    for (const change of [
      { authenticatorData: encode64(noUp) },
      { authenticatorData: encode64(noUp.slice(0, 36)) },
      { clientDataJSON: encode64(utf8('{')) },
      { signature: encode64(new Uint8Array(64)) },
    ]) {
      expect(
        await verifyAssertion(record, { ...assertion, ...change }, challenge),
      ).toBe(false);
    }
  });
  it('accepts RS256 with reordered JSON and absent crossOrigin, rejects crossOrigin=true', async () => {
    const pair = await crypto.subtle.generateKey(
      {
        name: 'RSASSA-PKCS1-v1_5',
        modulusLength: 2048,
        publicExponent: new Uint8Array([1, 0, 1]),
        hash: 'SHA-256',
      },
      true,
      ['sign', 'verify'],
    );
    const rsa: OwnerCredential = {
      ...record,
      algorithm: -257,
      publicKey: encode64(
        await crypto.subtle.exportKey('spki', pair.publicKey),
      ),
    };
    const challenge = await sessionChallenge(registration);
    for (const crossOrigin of [undefined, false, true, 'false']) {
      const client = utf8(
        JSON.stringify({
          origin: rsa.origin,
          challenge: encode64(challenge),
          type: 'webauthn.get',
          crossOrigin,
        }),
      );
      const auth = decode64(assertion.authenticatorData);
      const signed = new Uint8Array(auth.length + 32);
      signed.set(auth);
      signed.set(await hash(client), auth.length);
      const sig = await crypto.subtle.sign(
        'RSASSA-PKCS1-v1_5',
        pair.privateKey,
        bytes(signed),
      );
      expect(
        await verifyAssertion(
          rsa,
          {
            ...assertion,
            clientDataJSON: encode64(client),
            signature: encode64(sig),
          },
          challenge,
        ),
      ).toBe(crossOrigin === undefined || crossOrigin === false);
    }
  });
  it('rejects malformed DER rather than accepting raw or ambiguous integers', () => {
    expect(es256DerToRaw(decode64(assertion.signature))).toHaveLength(64);
    for (const invalid of [
      new Uint8Array(64),
      new Uint8Array([48, 6, 2, 1, 128, 2, 1, 1]),
      new Uint8Array([48, 7, 2, 2, 0, 1, 2, 1, 1]),
      new Uint8Array([48, 5, 2, 0, 2, 1, 1]),
      decode64(assertion.signature).slice(0, -1),
    ])
      expect(() => es256DerToRaw(invalid)).toThrow();
  });
});
