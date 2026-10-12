import { exchange } from './relayTransportCrypto';
import { OwnerTrustError } from './ownerSession';
import { readAfterOwnerUnlock } from './ownerUnlockRecovery';
import type { DeviceCertificate, OwnerCredential } from './ownerTrustCrypto';
export interface DeviceTrust {
  deviceId: string;
  paired: boolean;
  strict: boolean;
  credentials: OwnerCredential[];
  sessions: {
    sessionId: string;
    credentialId: string;
    expiresAt: number;
    current: boolean;
  }[];
  certificate: DeviceCertificate | null;
}
export async function deviceTrustRequest<T>(
  deviceId: string,
  path = '',
  method = 'GET',
  body?: unknown,
): Promise<T> {
  const send = () => exchange(
    new Request(
      new URL(
        `/relay/devices/${encodeURIComponent(deviceId)}/api/transport/${path || 'trust'}`,
        location.origin,
      ),
      {
        method,
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      },
    ),
  );
  const { response } = await (method === 'GET'
    ? readAfterOwnerUnlock(deviceId, send)
    : send());
  if (!response.ok) {
    const data = (await response.json().catch(() => null)) as {
      code?: string;
    } | null;
    if (data?.code === 'last_owner_credential')
      throw new OwnerTrustError('ownerLastCredential');
    if (
      data?.code === 'owner_origin_mismatch' ||
      data?.code === 'owner_session_revoked'
    )
      throw new OwnerTrustError(data.code);
    if (
      data?.code === 'owner_session_required' ||
      data?.code === 'device_strict_mode'
    )
      throw new OwnerTrustError('owner_session_required');
    throw new OwnerTrustError('ownerTrustFailed');
  }
  return response.status === 204
    ? (undefined as T)
    : ((await response.json()) as T);
}
