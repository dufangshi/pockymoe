import { translate } from '@pockymoe/thread-ui/i18n';
import type { DeviceCertificate } from './accountVault';
import { relayUrl } from './relayOrigin';
import { readAfterOwnerUnlock } from './ownerUnlockRecovery';
import { exchange } from './relayTransportCrypto';

export interface DeviceTrust {
  deviceId: string;
  bound: boolean;
  strict: boolean;
  accountKeyId: string | null;
  certificate: DeviceCertificate | null;
  browsers: {
    browserId: string;
    role: 'owner' | 'grantee';
    userId: string | null;
    registeredAt: number;
    lastSeenAt: number;
    current: boolean;
  }[];
  revocations: number;
}
export class DeviceTrustError extends Error {
  constructor(
    public code: string,
    public status: number,
  ) {
    super(translate('devices.protectionRequestFailed', { value1: code }));
  }
}
export async function deviceTrustRequest<T>(
  deviceId: string,
  path = '',
  method = 'GET',
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const send = () =>
    exchange(
      new Request(
        new URL(
          relayUrl(
            `/relay/devices/${encodeURIComponent(deviceId)}/api/transport/${path || 'trust'}`,
          ),
          location.origin,
        ),
        {
          method,
          credentials: 'include',
          signal: signal ?? null,
          headers: { 'content-type': 'application/json' },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        },
      ),
    );
  const { response } = await (method === 'GET' && path !== 'enrollment'
    ? readAfterOwnerUnlock(deviceId, send, signal)
    : send());
  if (!response.ok) {
    const data = (await response.json().catch(() => null)) as {
      code?: string;
    } | null;
    throw new DeviceTrustError(
      data?.code ?? 'device_unresponsive',
      response.status,
    );
  }
  return response.status === 204
    ? (undefined as T)
    : ((await response.json()) as T);
}
