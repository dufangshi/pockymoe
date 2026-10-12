import {
  OwnerTrustError,
  assertOwner,
  fetchOwnerCredentials,
} from './ownerSession';
import {
  certificateChallenge,
  decode64,
  pairingMac,
  type DeviceCertificate,
} from './ownerTrustCrypto';
import { saveDeviceCertificate } from './ownerDeviceTrust';
import {
  exchange,
  finishPairingInspection,
  inspectPairingDevice,
  type Descriptor,
} from './relayTransportCrypto';
export interface PairingOffer {
  deviceId: string;
  pairingId: string;
  secret: string;
  identity: string;
}
let pending: PairingOffer | undefined;
export function parsePairingFragment(fragment: string): PairingOffer {
  try {
    const params = new URLSearchParams(fragment.replace(/^#/, ''));
    const names = ['v', 'device', 'pairing', 'secret', 'identity'];
    if (
      names.some((key) => params.getAll(key).length !== 1) ||
      [...params.keys()].some((key) => !names.includes(key)) ||
      params.get('v') !== '1'
    )
      throw new Error();
    const offer = {
      deviceId: params.get('device')!,
      pairingId: params.get('pairing')!,
      secret: params.get('secret')!,
      identity: params.get('identity')!,
    };
    if (
      !offer.deviceId ||
      offer.deviceId.length > 200 ||
      decode64(offer.pairingId).length !== 16 ||
      decode64(offer.secret).length !== 32 ||
      decode64(offer.identity).length !== 32
    )
      throw new Error();
    return offer;
  } catch {
    throw new OwnerTrustError('ownerPairInvalid');
  }
}
// Only memory retains the secret across same-tab sign-in/account navigation. It
// is never copied into a returnTo query, storage, network request, or log.
export function takePairingOffer() {
  const fragment = location.hash;
  if (fragment) {
    history.replaceState(
      history.state,
      '',
      location.pathname + location.search,
    );
    pending = undefined;
    pending = parsePairingFragment(fragment);
  }
  if (!pending) throw new OwnerTrustError('ownerPairInvalid');
  return pending;
}
export function forgetPairingOffer() {
  pending = undefined;
}
export async function preparePairing(offer: PairingOffer) {
  const descriptor = await inspectPairingDevice(offer.deviceId, offer.identity);
  const records = await fetchOwnerCredentials();
  if (!records.length) throw new OwnerTrustError('ownerNoPasskey');
  const challenge = await certificateChallenge(
    offer.deviceId,
    descriptor.identityKey,
    offer.pairingId,
  );
  return { descriptor, start: () => assertOwner(records, challenge) };
}
export async function completePairing(
  offer: PairingOffer,
  descriptor: Descriptor,
  proof: Awaited<ReturnType<typeof assertOwner>>,
) {
  const { credential, assertion } = proof;
  try {
    const { response } = await exchange(
      new Request(
        new URL(
          `/relay/devices/${encodeURIComponent(offer.deviceId)}/api/transport/pair`,
          location.origin,
        ),
        {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            version: 1,
            pairingId: offer.pairingId,
            credential,
            assertion,
            mac: await pairingMac(
              offer.secret,
              offer.deviceId,
              offer.pairingId,
              credential,
              assertion,
            ),
          }),
        },
      ),
    );
    if (!response.ok) {
      if ([404, 409, 410, 429].includes(response.status))
        throw new OwnerTrustError('ownerPairExpired');
      throw new OwnerTrustError('ownerPairFailed');
    }
    const result = (await response.json()) as {
      paired: boolean;
      certificate: DeviceCertificate;
    };
    const certificate = result.certificate;
    if (
      !result.paired ||
      certificate?.deviceId !== offer.deviceId ||
      certificate.identityKey !== descriptor.identityKey ||
      certificate.pairingId !== offer.pairingId ||
      certificate.assertion.credentialId !== credential.credentialId
    )
      throw new OwnerTrustError('ownerIdentityMismatch');
    await saveDeviceCertificate(certificate);
    forgetPairingOffer();
    return certificate;
  } finally {
    finishPairingInspection(offer.deviceId);
  }
}
