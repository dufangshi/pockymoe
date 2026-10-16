import { translate } from '@pockymoe/thread-ui/i18n';
import type {
  RelayAccessGrantDto,
  RelaySessionShareDto,
  RelayPortalSummaryDto,
  CreateRelayAccessGrantInput,
  CreateRelaySessionShareInput,
} from '@pockymoe/shared';
import { request } from './api';
import { requestGrantReview, type GrantChange, type GrantSummary } from './sharingApproval';
import {
  isProtectedSharingDevice,
  sharingGrantDevices,
  validatePublicAccount,
  encode64,
  type PublicAccount,
} from './sharedDeviceTrust';
import { safetyCode } from './accountVault';
import { sharingTrustStore } from './sharingTrustStore';
export { safetyCode } from './accountVault';

export type ShareKind = 'share' | 'grant';
export type ShareRow = RelaySessionShareDto | RelayAccessGrantDto;
export interface DeviceGrant {
  version: 1;
  grantId: string;
  relayRef: { kind: ShareKind; id: string };
  grantee: { userId: string; label: string; accountKeyId: string; accountPublicKey: string };
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
export const recipientAccount = (kind: ShareKind, id: string) =>
  request<PublicAccount>(
    `relay/${collection(kind)}/${encodeURIComponent(id)}/recipient-account`,
  ).then(validatePublicAccount).catch((error: unknown) => {
    if (error && typeof error === 'object' && 'code' in error && error.code === 'recipient_account_missing')
      throw new Error(translate('sharing.recipientNeedsAccount'));
    throw error;
  });
export const deviceGrants = (deviceId: string) =>
  deviceRequest<{ grants: DeviceGrant[] }>(
    deviceId,
    'api/transport/grants',
  ).then((r) => r.grants);
export const deleteDeviceGrant = (deviceId: string, grantId: string) =>
  deviceRequest(
    deviceId,
    'api/transport/grants/' + encodeURIComponent(grantId),
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
      throw new Error(translate('sharing.protectedOnline'));
    throw error;
  }
}
export async function recipientVerified(account: PublicAccount, userId: string) {
  const checked = await validatePublicAccount(account);
  return !!(await sharingTrustStore<boolean>('recipient-confirmed:' + JSON.stringify([userId, checked.accountKeyId])));
}
export async function confirmRecipient(account: PublicAccount, userId: string) {
  const checked = await validatePublicAccount(account);
  if (!userId) throw new Error(translate('sharing.rowAltered'));
  await sharingTrustStore('recipient-confirmed:' + JSON.stringify([userId, checked.accountKeyId]), true);
}
export async function findRelayRow(kind: ShareKind, id: string) {
  const portal = await request<RelayPortalSummaryDto>('relay/portal');
  const row = (
    kind === 'share' ? portal.sharedByMe : (portal.grantsByMe ?? [])
  ).find((r) => r.id === id);
  if (!row) throw new Error(translate('sharing.rowUnavailable'));
  return row;
}
async function protectedForSharing(row: ShareRow) {
  // Certificates remain available while the device is offline. A saved certificate
  // prevents a relay omission from downgrading an owner to legacy sharing.
  if (await isProtectedSharingDevice(row.deviceId)) {
    if (row.deviceConnected === false)
      throw new Error(translate('sharing.protectedOnline'));
    return true;
  }
  return false;
}
export function grantForRow(
  kind: ShareKind,
  row: ShareRow,
  account: PublicAccount,
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
      accountKeyId: account.accountKeyId,
      accountPublicKey: account.accountPublicKey,
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
// All inputs are snapshotted before any async relay operation. Relay fields are
// suggestions until explicitly reviewed; a create can never expand dialog intent.
type SharingFields = Omit<
  CreateRelayAccessGrantInput & CreateRelaySessionShareInput,
  'threadId'
> & { threadId: string | null };
export type SharingInput = {
  [K in keyof SharingFields]?: SharingFields[K] | undefined;
} & Partial<Pick<ShareRow, 'targetUserId' | 'targetUsername' | 'deviceName'>>;
// Snapshot the values presented by the dialog. Callers combine this local
// context with form values; no subsequent relay read supplies signed fields.
export function sharingEditInput(kind: ShareKind, row: ShareRow): SharingInput {
  const grant = kind === 'grant' ? row as RelayAccessGrantDto : undefined;
  return {
    deviceId: row.deviceId, deviceName: row.deviceName,
    targetUserId: row.targetUserId, targetUsername: row.targetUsername,
    scope: grant?.scope ?? 'thread', threadId: row.threadId ?? null,
    workspaceId: row.workspaceId ?? null, workspaceScope: grant?.workspaceScope ?? 'all',
    workspaceIds: [...(grant?.workspaceIds ?? [])], canCreateThreads: grant?.canCreateThreads ?? false,
    threadAccess: row.threadAccess, workspaceAccess: row.workspaceAccess, expiresAt: row.expiresAt ?? null,
  };
}
async function approveDeviceGrant(
  deviceId: string,
  deviceName: string,
  grant: DeviceGrant,
  previous?: DeviceGrant,
) {
  const verified = await recipientVerified(grant.grantee, grant.grantee.userId);
  const code = await safetyCode(grant.grantee.accountKeyId);
  // An unlocked session and the reconciliation button never authorize a PUT.
  // Wait for explicit review of this exact snapshot and recipient account key.
  const fields = translate('sharing.reviewGrant', {
    value1: JSON.stringify({ name: deviceName, id: deviceId }),
    value2: JSON.stringify({
      scope: grant.scope,
      threadId: grant.threadId,
      workspaceId: grant.workspaceId,
      workspaceScope: grant.workspaceScope,
      workspaceIds: grant.workspaceIds,
    }),
    value3: JSON.stringify({
      threadAccess: grant.threadAccess,
      workspaceAccess: grant.workspaceAccess,
      canCreateThreads: grant.canCreateThreads,
    }),
    value4: grant.expiresAt === null
      ? translate('sharing.noExpiry')
      : new Date(grant.expiresAt).toISOString(),
    value5: JSON.stringify({ name: grant.grantee.label, userId: grant.grantee.userId }),
    value6: code,
  });
  const difference = translate('sharing.reviewDifference', {
    value1: previous ? JSON.stringify(previous) : translate('sharing.notActive'),
    value2: JSON.stringify(grant),
  });
  const changes = previous ? translate('sharing.reviewChanges', {
    value1: JSON.stringify(Object.fromEntries(
      (Object.keys(grant) as (keyof DeviceGrant)[])
        .filter((key) => JSON.stringify(previous[key]) !== JSON.stringify(grant[key]))
        .map((key) => [key, { before: previous[key], after: grant[key] }]),
    )),
  }) : '';
  const summarize = (value: DeviceGrant): GrantSummary => ({
    device: { name: deviceName, id: deviceId },
    recipient: { name: value.grantee.label, userId: value.grantee.userId },
    safetyCode: code,
    scope: value.scope,
    workspaceScope: value.workspaceScope,
    workspaceCount: value.workspaceIds.length,
    threadAccess: value.threadAccess,
    workspaceAccess: value.workspaceAccess,
    canCreateThreads: value.canCreateThreads,
    expiresAt: value.expiresAt,
  });
  const after = summarize(grant);
  if (previous) {
    const before = summarize(previous);
    const differs = (keys: (keyof DeviceGrant)[]) => keys.some(key => JSON.stringify(previous[key]) !== JSON.stringify(grant[key]));
    after.changes = ([
      ['scope', ['scope', 'threadId', 'workspaceId', 'workspaceScope', 'workspaceIds']],
      ['threadAccess', ['threadAccess']],
      ['workspaceAccess', ['workspaceAccess']],
      ['canCreateThreads', ['canCreateThreads']],
      ['expiresAt', ['expiresAt']],
      ['recipient', ['grantee']],
    ] as const).filter(([, keys]) => differs([...keys])).map(([field]) => ({ field, before, after: summarize(grant) }) as GrantChange);
  }
  if (!(await requestGrantReview({
    description: fields + '\n\n' + difference + (changes ? '\n' + changes : ''),
    recipientVerified: verified,
    summary: after,
  })))
    throw new Error(translate('sharing.signCancelled'));
}
export async function syncDeviceGrant(kind: ShareKind, source: ShareRow) {
  const row = structuredClone(source);
  if (!(await protectedForSharing(row))) return;
  const account = await recipientAccount(kind, row.id);
  const grants = await deviceGrants(row.deviceId);
  const existing = grants.find(
    (g) => g.relayRef.kind === kind && g.relayRef.id === row.id,
  );
  const grant = grantForRow(kind, row, account, existing?.grantId);
  if (grant.expiresAt !== null && !Number.isFinite(grant.expiresAt))
    throw new Error(translate('sharing.rowAltered'));
  await approveDeviceGrant(row.deviceId, row.deviceName, grant, existing);
  await deviceRequest(
    row.deviceId,
    'api/transport/grants/' + encodeURIComponent(grant.grantId),
    { method: 'PUT', body: JSON.stringify(grant) },
  );
  return grant;
}
function sameIntent(kind: ShareKind, expected: ShareRow, returned: ShareRow) {
  return (
    expected.deviceId === returned.deviceId &&
    grantPermissions(grantForRow(kind, expected, { accountKeyId: '', accountPublicKey: '' }, 'compare')) ===
      grantPermissions(grantForRow(kind, returned, { accountKeyId: '', accountPublicKey: '' }, 'compare'))
  );
}
export async function sharingMutation<T>(
  kind: ShareKind,
  operation: 'create' | 'edit' | 'revoke',
  id: string | undefined,
  mutate: () => Promise<T>,
  dialogInput?: SharingInput,
): Promise<T> {
  const input = dialogInput
    ? structuredClone(Object.fromEntries(
      Object.entries(dialogInput).filter(([, value]) => value !== undefined),
    )) as SharingInput
    : undefined;
  if (operation === 'revoke') {
    // Load every bound or locally certified device before deleting anything. A malicious
    // portal cannot hide/reassign the grant's device or claim it is unpaired.
    const inventory = [];
    for (const deviceId of await sharingGrantDevices())
      inventory.push({ deviceId, grants: await deviceGrants(deviceId) });
    for (const { deviceId, grants } of inventory)
      for (const grant of grants)
        if (grant.relayRef.kind === kind && grant.relayRef.id === id)
          await deleteDeviceGrant(deviceId, grant.grantId);
    return mutate();
  }
  if (!input) throw new Error(translate('sharing.rowAltered'));
  // Metadata refreshes must never re-authorize access from a portal row.
  const permissionFields = [
    'workspaceId', 'workspaceScope', 'workspaceIds', 'threadAccess',
    'workspaceAccess', 'canCreateThreads', 'expiresAt',
  ];
  if (operation === 'edit' && !permissionFields.some((key) => key in input))
    return mutate();
  if (operation === 'edit' && [
    'deviceId', 'targetUserId', 'targetUsername', 'scope', 'threadId', 'workspaceId',
    'workspaceScope', 'workspaceIds', 'threadAccess', 'workspaceAccess', 'canCreateThreads', 'expiresAt',
  ].some((key) => !(key in input)))
    throw new Error(translate('sharing.dialogIncomplete'));
  const previous = id ? await findRelayRow(kind, id) : undefined;
  const chosen = structuredClone(input);
  const result = await mutate();
  const response = result as ShareRow;
  const rowId = id ?? response.id;
  try {
    if (!rowId || (response.id !== undefined && response.id !== rowId))
      throw new Error(translate('sharing.rowAltered'));
    // Some grant endpoints return only id. The portal may validate that intent,
    // but must never supply the permissions that the browser signs.
    const returned = response.deviceId
      ? response
      : await findRelayRow(kind, rowId);
    if (!returned.targetUserId ||
      (response.targetUserId && response.targetUserId !== returned.targetUserId))
      throw new Error(translate('sharing.rowAltered'));
    const expected = {
      ...chosen,
      id: rowId,
      targetUserId: input.targetUserId ?? returned.targetUserId,
      targetUsername: input.targetIdentifier ?? input.targetUsername,
      // Names are display hints only; the reviewed device ID is authoritative.
      deviceName: input.deviceName ?? returned.deviceName,
    } as ShareRow;
    const metadataChanged = (['label', 'threadTitle', 'workspaceLabel'] as const)
      .some((key) => key in input &&
        (input[key] ?? null) !== (returned[key] ?? null));
    if (!sameIntent(kind, expected, returned) || metadataChanged)
      throw new Error(translate('sharing.rowAltered'));
    await syncDeviceGrant(kind, expected);
  } catch (error) {
    try {
      await request(
        `relay/${collection(kind)}/${encodeURIComponent(rowId)}`,
        previous
          ? { method: 'PATCH', body: JSON.stringify(previous) }
          : { method: 'DELETE' },
      );
    } catch {
      throw new Error(translate('sharing.rollbackFailed'));
    }
    throw new Error(translate('sharing.rolledBack', {
      value1: error instanceof Error ? error.message : translate('sharing.protectedOnline'),
    }));
  }
  return result;
}
export interface Reconciliation {
  row?: ShareRow;
  grant?: DeviceGrant | undefined;
  state: 'active' | 'missing' | 'orphan' | 'changed' | 'different';
  account: PublicAccount;
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
  rows: { kind: ShareKind; row: ShareRow; account: PublicAccount }[],
  grants: DeviceGrant[],
): Reconciliation[] {
  grants = grants.filter(
    (g) => g.expiresAt === null || g.expiresAt > Date.now(),
  );
  const entries: Reconciliation[] = rows.map(({ kind, row, account }) => {
    const grant = grants.find(
      (g) => g.relayRef.kind === kind && g.relayRef.id === row.id,
    );
    return {
      row,
      grant,
      account,
      state: !grant
        ? 'missing'
        : account.accountKeyId !== grant.grantee.accountKeyId ||
            account.accountPublicKey !== grant.grantee.accountPublicKey
          ? 'changed'
          : grantPermissions(grant) !==
              grantPermissions(
                grantForRow(kind, row, account, grant.grantId),
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
        account: { accountKeyId: grant.grantee.accountKeyId, accountPublicKey: grant.grantee.accountPublicKey },
      });
  }
  return entries;
}
