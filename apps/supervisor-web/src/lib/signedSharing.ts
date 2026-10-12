import { translate } from '@pockymoe/thread-ui/i18n';
import type {
  RelayAccessGrantDto,
  RelaySessionShareDto,
  RelayPortalSummaryDto,
} from '@pockymoe/shared';
import { request } from './api';
import { hasDeviceCertificate } from './ownerDeviceTrust';
import { encode64, hash64, type OwnerCredential } from './ownerTrustCrypto';
import { trustStore } from './ownerTrustStore';

export type ShareKind = 'share' | 'grant';
export type ShareRow = RelaySessionShareDto | RelayAccessGrantDto;
export interface DeviceGrant {
  version: 1;
  grantId: string;
  relayRef: { kind: ShareKind; id: string };
  grantee: { userId: string; label: string; credentials: OwnerCredential[] };
  scope: 'thread' | 'workspace' | 'device';
  threadId: string | null;
  workspaceId: string | null;
  workspaceScope: 'all' | 'selected';
  workspaceIds: string[];
  threadAccess: 'read' | 'control';
  workspaceAccess: 'none' | 'read' | 'write';
  canCreateThreads: boolean;
  expiresAt: number | null;
}
const collection = (kind: ShareKind) =>
  kind === 'share' ? 'shares' : 'grants';
export const recipientPasskeys = (kind: ShareKind, id: string) =>
  request<{ passkeys: OwnerCredential[] }>(
    `/relay/${collection(kind)}/${encodeURIComponent(id)}/recipient-passkeys`,
  ).then((r) => r.passkeys);
export const deviceGrants = (deviceId: string) =>
  deviceRequest<{ grants: DeviceGrant[] }>(
    deviceId,
    '/api/transport/grants',
  ).then((r) => r.grants);
export const deleteDeviceGrant = (deviceId: string, grantId: string) =>
  deviceRequest(
    deviceId,
    '/api/transport/grants/' + encodeURIComponent(grantId),
    { method: 'DELETE' },
  );
async function deviceRequest<T>(
  deviceId: string,
  path: string,
  init?: RequestInit,
) {
  try {
    return await request<T>(path, init, { deviceId });
  } catch (error) {
    if (
      error &&
      typeof error === 'object' &&
      (('statusCode' in error &&
        [503, 504].includes(Number(error.statusCode))) ||
        ('code' in error &&
          ['device_offline', 'device_unresponsive'].includes(
            String(error.code),
          )))
    )
      throw new Error(translate('sharing.pairedOnline'));
    throw error;
  }
}
export async function safetyCode(
  credentials: Pick<OwnerCredential, 'credentialId'>[],
) {
  const digest = await hash64(
    new TextEncoder().encode(
      'pockymoe-grantee-v1\n' +
        credentials
          .map((c) => c.credentialId)
          .sort()
          .join('\n'),
    ),
  );
  return digest.slice(0, 12).match(/.{4}/g)!.join(' ');
}
export function credentialSet(credentials: OwnerCredential[]) {
  return JSON.stringify(
    credentials
      .map((c) => [c.credentialId, c.publicKey, c.algorithm, c.rpId, c.origin])
      .sort((a, b) =>
        String(a[0]) < String(b[0]) ? -1 : String(a[0]) > String(b[0]) ? 1 : 0,
      ),
  );
}
async function confirmationKey(credentials: OwnerCredential[]) {
  return (
    'grantee-confirmed:' +
    (await hash64(new TextEncoder().encode(credentialSet(credentials))))
  );
}
export async function recipientVerified(credentials: OwnerCredential[]) {
  return !!(await trustStore<boolean>(await confirmationKey(credentials)));
}
export async function confirmRecipient(credentials: OwnerCredential[]) {
  await trustStore(await confirmationKey(credentials), true);
}
export async function findRelayRow(kind: ShareKind, id: string) {
  const portal = await request<RelayPortalSummaryDto>('/relay/portal');
  const row = (
    kind === 'share' ? portal.sharedByMe : (portal.grantsByMe ?? [])
  ).find((r) => r.id === id);
  if (!row) throw new Error(translate('sharing.rowUnavailable'));
  return row;
}
async function pairedForSharing(row: ShareRow) {
  // Certificates remain available while the device is offline. A saved certificate
  // prevents a relay omission from downgrading an owner to legacy sharing.
  if (await hasDeviceCertificate(row.deviceId)) {
    if (row.deviceConnected === false)
      throw new Error(translate('sharing.pairedOnline'));
    return true;
  }
  return false;
}
export function grantForRow(
  kind: ShareKind,
  row: ShareRow,
  credentials: OwnerCredential[],
  grantId = encode64(crypto.getRandomValues(new Uint8Array(16))),
): DeviceGrant {
  const grant = kind === 'grant' ? (row as RelayAccessGrantDto) : undefined;
  return {
    version: 1,
    grantId,
    relayRef: { kind, id: row.id },
    grantee: {
      userId: row.targetUserId,
      label: row.targetUsername,
      credentials,
    },
    scope: grant?.scope ?? 'thread',
    threadId: row.threadId ?? null,
    workspaceId: row.workspaceId ?? null,
    workspaceScope: grant?.workspaceScope ?? 'all',
    workspaceIds: grant?.workspaceIds ?? [],
    threadAccess: row.threadAccess,
    workspaceAccess: row.workspaceAccess,
    canCreateThreads: grant?.canCreateThreads ?? false,
    expiresAt: row.expiresAt ? Date.parse(row.expiresAt) : null,
  };
}
export async function syncDeviceGrant(kind: ShareKind, row: ShareRow) {
  if (!(await pairedForSharing(row))) return;
  const credentials = await recipientPasskeys(kind, row.id);
  if (!credentials.length)
    throw new Error(translate('sharing.recipientNeedsPasskey'));
  const grants = await deviceGrants(row.deviceId);
  const existing = grants.find(
    (g) => g.relayRef.kind === kind && g.relayRef.id === row.id,
  );
  const grant = grantForRow(kind, row, credentials, existing?.grantId);
  await deviceRequest(
    row.deviceId,
    '/api/transport/grants/' + encodeURIComponent(grant.grantId),
    { method: 'PUT', body: JSON.stringify(grant) },
  );
  return grant;
}
export async function sharingMutation<T>(
  kind: ShareKind,
  operation: 'create' | 'edit' | 'revoke',
  id: string | undefined,
  mutate: () => Promise<T>,
): Promise<T> {
  const previous = id ? await findRelayRow(kind, id) : undefined;
  if (operation === 'revoke') {
    if (previous && (await pairedForSharing(previous))) {
      for (const grant of await deviceGrants(previous.deviceId)) {
        if (grant.relayRef.kind === kind && grant.relayRef.id === id)
          await deleteDeviceGrant(previous.deviceId, grant.grantId);
      }
    }
    return mutate();
  }
  const result = await mutate();
  const rowId = id ?? (result as { id: string }).id;
  try {
    const row = await findRelayRow(kind, rowId);
    await syncDeviceGrant(kind, row);
  } catch (error) {
    try {
      await request(
        `/relay/${collection(kind)}/${encodeURIComponent(rowId)}`,
        previous
          ? { method: 'PATCH', body: JSON.stringify(previous) }
          : { method: 'DELETE' },
      );
    } catch {
      throw new Error(translate('sharing.rollbackFailed'));
    }
    throw new Error(
      translate('sharing.rolledBack', {
        value1:
          error instanceof Error
            ? error.message
            : translate('sharing.pairedOnline'),
      }),
    );
  }
  return result;
}
export interface Reconciliation {
  row?: ShareRow;
  grant?: DeviceGrant | undefined;
  state: 'active' | 'missing' | 'orphan' | 'changed' | 'different';
  credentials: OwnerCredential[];
}
function grantPermissions(grant: DeviceGrant) {
  return JSON.stringify([
    grant.grantee.userId,
    grant.scope,
    grant.threadId,
    grant.workspaceId,
    grant.workspaceScope,
    [...grant.workspaceIds].sort(),
    grant.threadAccess,
    grant.workspaceAccess,
    grant.canCreateThreads,
    grant.expiresAt,
  ]);
}
export function reconcileGrants(
  rows: { kind: ShareKind; row: ShareRow; credentials: OwnerCredential[] }[],
  grants: DeviceGrant[],
): Reconciliation[] {
  grants = grants.filter(
    (g) => g.expiresAt === null || g.expiresAt > Date.now(),
  );
  const entries: Reconciliation[] = rows.map(({ kind, row, credentials }) => {
    const grant = grants.find(
      (g) => g.relayRef.kind === kind && g.relayRef.id === row.id,
    );
    return {
      row,
      grant,
      credentials,
      state: !grant
        ? 'missing'
        : credentialSet(credentials) !==
            credentialSet(grant.grantee.credentials)
          ? 'changed'
          : grantPermissions(grant) !==
              grantPermissions(
                grantForRow(kind, row, credentials, grant.grantId),
              )
            ? 'different'
            : 'active',
    };
  });
  for (const grant of grants) {
    if (
      !rows.some(
        (r) => r.kind === grant.relayRef.kind && r.row.id === grant.relayRef.id,
      )
    )
      entries.push({
        grant,
        state: 'orphan',
        credentials: grant.grantee.credentials,
      });
  }
  return entries;
}
