import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  accountRevocations,
  boundDeviceIdentities,
  currentAccount,
  requireUnlock,
  type AccountKeyInfo,
} from './accountVault';
import { request } from './api';
import {
  completeDeviceEnrollment,
  createPendingEnrollment,
  inspectEnrollmentDevice,
  EnrollmentIdentityChangeError,
  type PendingEnrollment,
} from './deviceEnrollment';
import {
  deletePendingEnrollment,
  listPendingEnrollments,
  pendingIsCurrent,
  savePending,
  type RemoteProtectionIntent,
} from './pendingEnrollmentStore';
import { deviceTrustRequest, DeviceTrustError } from './deviceTrust';
import { clearTransportKeyCache } from './relayTransportCrypto';

export const REMOTE_PROTECTION_CHANGED_EVENT =
  'pockymoe-remote-protection-changed';
export type ProtectionProgress =
  | RemoteProtectionIntent
  | { phase: 'complete' | 'already-bound' };
const terminal = new Map<string, ProtectionProgress>();
const active = new Map<string, Promise<boolean>>();
const key = (
  account: Pick<AccountKeyInfo, 'userId' | 'accountKeyId'>,
  deviceId: string,
) => `${account.userId}:${account.accountKeyId}:${deviceId}`;
const notify = () =>
  window.dispatchEvent(new Event(REMOTE_PROTECTION_CHANGED_EVENT));
const apiRoot = (id: string) =>
  `/relay/devices/${encodeURIComponent(id)}/api/management/supervisor`;
const errorCode = (error: unknown) =>
  (error as { code?: string; payload?: { code?: string } } | null)?.code ??
  (error as { payload?: { code?: string } } | null)?.payload?.code;
const offlineError = (error: unknown) =>
  error instanceof TypeError ||
  [
    'device_offline',
    'device_unresponsive',
    'browser_registration_required',
  ].includes(errorCode(error) ?? '') ||
  ((error as { status?: number; statusCode?: number } | null)?.status ??
    (error as { statusCode?: number } | null)?.statusCode ??
    0) >= 500;
const retryable = (error: unknown) =>
  offlineError(error) ||
  ['already_bound', 'enrollment_missing'].includes(errorCode(error) ?? '');

async function liveAccount(pending: PendingEnrollment) {
  const account = await currentAccount();
  if (
    !account ||
    account.userId !== pending.userId ||
    account.accountKeyId !== pending.accountKeyId ||
    !(await pendingIsCurrent(pending))
  )
    return null;
  if (
    (await accountRevocations()).some(
      (r) =>
        r.accountKeyId === account.accountKeyId &&
        r.kind === 'device' &&
        r.subjectId === pending.deviceId,
    )
  ) {
    await deletePendingEnrollment(pending.deviceId, pending);
    return null;
  }
  return account;
}
async function progress(
  pending: PendingEnrollment,
  intent: RemoteProtectionIntent,
) {
  const current = (await listPendingEnrollments()).find(
    (p) => p.deviceId === pending.deviceId,
  );
  if (!current || !(await liveAccount(pending))) return false;
  return savePending({ ...current, remoteProtection: intent }, pending);
}
function finish(
  account: AccountKeyInfo,
  deviceId: string,
  phase: 'complete' | 'already-bound',
) {
  terminal.set(key(account, deviceId), { phase });
  notify();
}
async function certify(pending: PendingEnrollment, account: AccountKeyInfo) {
  if (!(await liveAccount(pending))) return false;
  const latest = (await listPendingEnrollments(account)).find(
    (p) => p.deviceId === pending.deviceId,
  );
  if (!latest || latest.enrollSecret !== pending.enrollSecret) return false;
  // Enrollment persists its certificate before installation. Give it the live
  // remote intent so an interrupted upload cannot overwrite bindStarted with
  // the pre-bind snapshot and incorrectly skip certification on reconnect.
  const completed = await completeDeviceEnrollment(latest, true);
  if (completed) finish(account, pending.deviceId, 'complete');
  return completed;
}
export async function deviceProtectionProgress(
  deviceId: string,
): Promise<ProtectionProgress | undefined> {
  const account = await currentAccount();
  if (!account) return undefined;
  const pending = (await listPendingEnrollments(account)).find(
    (p) => p.deviceId === deviceId,
  );
  return pending?.remoteProtection ?? terminal.get(key(account, deviceId));
}

/** The user approves remote binding once. Offline intent is durable and account-scoped. */
export async function protectDevices(devices: readonly RelayDeviceDto[]) {
  const account = await requireUnlock('bind-device');
  const pins = await boundDeviceIdentities();
  const revoked = await accountRevocations();
  const pending = await listPendingEnrollments(account);
  for (const device of devices) {
    if (device.ownerUserId !== account.userId) continue;
    if (
      revoked.some(
        (r) =>
          r.accountKeyId === account.accountKeyId &&
          r.kind === 'device' &&
          r.subjectId === device.id,
      )
    )
      continue;
    if (pins.some((pin) => pin.deviceId === device.id)) {
      if (device.connected) {
        try {
          const descriptor = await inspectEnrollmentDevice(device.id);
          if (
            (descriptor.trust?.bound &&
              descriptor.trust.accountKeyId !== account.accountKeyId) ||
            (descriptor.certificate &&
              descriptor.certificate.accountKeyId !== account.accountKeyId)
          ) {
            terminal.set(key(account, device.id), {
              phase: 'error',
              error: 'account-mismatch',
            });
            notify();
            continue;
          }
        } catch (error) {
          if (!offlineError(error)) {
            terminal.set(key(account, device.id), {
              phase: 'error',
              error:
                errorCode(error) === 'account_mismatch'
                  ? 'account-mismatch'
                  : 'failed',
            });
            notify();
            continue;
          }
        }
      }
      finish(account, device.id, 'already-bound');
      continue;
    }
    terminal.delete(key(account, device.id));
    const existing = pending.find((p) => p.deviceId === device.id);
    let enrollment: PendingEnrollment;
    if (existing) {
      enrollment = {
        ...existing,
        remoteProtection: { phase: 'queued', resumeEnrollment: true },
      };
      if (!(await savePending(enrollment, existing))) continue;
    } else
      enrollment = await createPendingEnrollment(
        device.id,
        account,
        Date.now(),
        { phase: 'queued' },
      );
    try {
      await runRemoteProtection(enrollment, device.connected);
    } catch (error) {
      // The global monitor presents the existing MAC-gated identity prompt.
      if (
        !(error instanceof EnrollmentIdentityChangeError) &&
        !retryable(error)
      )
        throw error;
    }
  }
}

export function runRemoteProtection(
  pending: PendingEnrollment,
  online: boolean,
): Promise<boolean> {
  if (!online || !pending.remoteProtection) return Promise.resolve(false);
  const id = `${key(pending, pending.deviceId)}:${pending.createdAt}:${pending.enrollSecret}`;
  const running = active.get(id);
  if (running) return running;
  const task = run(pending);
  active.set(id, task);
  void task.then(
    () => active.delete(id),
    () => active.delete(id),
  );
  return task;
}

async function run(pending: PendingEnrollment): Promise<boolean> {
  const account = await liveAccount(pending);
  if (!account) return false;
  const intent = pending.remoteProtection!;
  if (intent.phase === 'error' || intent.phase === 'update-required')
    return false;
  try {
    let descriptor;
    try {
      descriptor = await inspectEnrollmentDevice(pending.deviceId);
    } catch (error) {
      // The inspector refuses a runtime without an encryption descriptor. Such
      // runtimes can be updated with the existing management API, but must never
      // receive the enrollment secret over the plaintext fallback.
      if (errorCode(error) !== 'accountIdentityMismatch') throw error;
      await needUpdate(pending, intent);
      return false;
    }
    if (!(await liveAccount(pending))) return false;
    if (
      descriptor.certificate &&
      descriptor.certificate.accountKeyId !== account.accountKeyId
    ) {
      await progress(pending, { phase: 'error', error: 'account-mismatch' });
      return false;
    }
    if (descriptor.trust?.bound) {
      if (descriptor.trust.accountKeyId !== account.accountKeyId) {
        await progress(pending, { phase: 'error', error: 'account-mismatch' });
        return false;
      }
      if (!intent.bindStarted && !intent.resumeEnrollment) {
        if (await deletePendingEnrollment(pending.deviceId, pending))
          finish(account, pending.deviceId, 'already-bound');
        return true;
      }
      // A bind may have succeeded just before the device disconnected. Resume
      // the existing MAC/certificate path, never remotely reset a bound device.
      if (!(await progress(pending, { ...intent, phase: 'certifying' })))
        return false;
      return await certify(pending, account);
    }
    const supportsBind =
      descriptor.trust?.capabilities?.includes('remote-bind') === true;
    if (!supportsBind) {
      await needUpdate(pending, intent);
      return false;
    }
    const binding: RemoteProtectionIntent = {
      ...intent,
      phase: 'binding',
      bindStarted: true,
    };
    if (!(await progress(pending, binding))) return false;
    const result = await deviceTrustRequest<{
      bound: boolean;
      accountKeyId: string;
      deviceId: string;
    }>(pending.deviceId, 'bind', 'POST', {
      accountPublicKey: account.accountPublicKey,
      enrollSecret: pending.enrollSecret,
      webOrigin: location.origin,
    });
    if (
      result.bound !== true ||
      result.accountKeyId !== account.accountKeyId ||
      result.deviceId !== pending.deviceId
    ) {
      await progress(pending, { phase: 'error', error: 'account-mismatch' });
      return false;
    }
    clearTransportKeyCache(pending.deviceId);
    if (!(await progress(pending, { ...binding, phase: 'certifying' })))
      return false;
    // Inspect again after binding. A compromised relay can race the initial
    // bind; make its resulting account mismatch explicit before certification.
    const bound = await inspectEnrollmentDevice(pending.deviceId);
    if (
      bound.trust?.bound &&
      bound.trust.accountKeyId !== account.accountKeyId
    ) {
      await progress(pending, { phase: 'error', error: 'account-mismatch' });
      return false;
    }
    return await certify(pending, account);
  } catch (error) {
    clearTransportKeyCache(pending.deviceId);
    if (error instanceof EnrollmentIdentityChangeError) throw error;
    if (retryable(error)) {
      // Let the enrollment monitor apply its backoff even when progress events
      // wake it while this attempt is in flight. A false result would allow a
      // queued wake to immediately repeat a failed bind forever.
      if ((error as { statusCode?: number } | null)?.statusCode !== undefined)
        throw new DeviceTrustError(
          errorCode(error) ?? 'device_unresponsive',
          (error as { statusCode: number }).statusCode,
        );
      throw error;
    }
    await progress(pending, {
      phase: 'error',
      error:
        errorCode(error) === 'account_mismatch' ? 'account-mismatch' : 'failed',
    });
    return false;
  }
}

async function needUpdate(
  pending: PendingEnrollment,
  intent: RemoteProtectionIntent,
) {
  if (intent.phase !== 'updating') {
    await progress(pending, { phase: 'update-required' });
    return;
  }
  // The update request can still be in flight; do not mistake a previous
  // completed job for the new operation's outcome.
  if (Date.now() - (intent.updateRequestedAt ?? 0) < 15_000) return;
  const status = await request<{
    job?: { phase?: string; state?: string; error?: string };
  }>(apiRoot(pending.deviceId), { cache: 'no-store' });
  if (
    ['failed', 'rolled-back', 'rollback-failed', 'completed'].includes(
      status.job?.phase ?? status.job?.state ?? '',
    ) ||
    Date.now() - (intent.updateRequestedAt ?? 0) > 5 * 60_000
  )
    await progress(pending, { phase: 'error', error: 'update-failed' });
}

/** Updates require a separate explicit click; binding resumes on reconnect. */
export async function updateAndProtectDevice(deviceId: string) {
  const account = await currentAccount();
  const pending =
    account &&
    (await listPendingEnrollments(account)).find(
      (p) => p.deviceId === deviceId,
    );
  if (!pending?.remoteProtection || !(await liveAccount(pending))) return;
  if (
    pending.remoteProtection.phase !== 'update-required' &&
    !(
      pending.remoteProtection.phase === 'error' &&
      pending.remoteProtection.error === 'update-failed'
    )
  )
    return;
  const intent: RemoteProtectionIntent = {
    phase: 'updating',
    updateRequestedAt: Date.now(),
  };
  // Claim the update inside the IndexedDB transaction so two open views/tabs
  // cannot both submit it after reading the same approval state.
  if (
    !(await savePending(
      { ...pending, remoteProtection: intent },
      pending,
      pending.remoteProtection.phase,
    ))
  )
    return;
  try {
    const result = await request<{
      canUpdate?: boolean;
      job?: { phase?: string };
    }>(`${apiRoot(deviceId)}/update`, { method: 'POST', body: '{}' });
    if (result.canUpdate === false && !result.job)
      await progress(pending, { phase: 'error', error: 'update-failed' });
    clearTransportKeyCache(deviceId);
  } catch (error) {
    if (!offlineError(error))
      await progress(pending, { phase: 'error', error: 'update-failed' });
  }
}
