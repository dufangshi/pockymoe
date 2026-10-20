import { currentBrowser } from './accountVaultState';
export const OWNER_UNLOCKED_EVENT = 'remote-codex-owner-unlocked';
export const OWNER_READ_BLOCKED_EVENT = 'remote-codex-owner-read-blocked';
export interface OwnerReadBlockedDetail {
  deviceId: string;
  revoked: boolean;
  /** The device's or transport's answer that blocked the read. */
  code?: string | undefined;
}
export interface OwnerUnlockedDetail {
  deviceIds: string[];
}
export function notifyOwnerUnlocked(deviceIds: string[]) {
  window.dispatchEvent(new CustomEvent<OwnerUnlockedDetail>(OWNER_UNLOCKED_EVENT, {
    detail: { deviceIds },
  }));
}
export function ownerUnlockedDevice(event: Event, deviceId: string) {
  const detail = (event as CustomEvent<OwnerUnlockedDetail>).detail;
  return Array.isArray(detail?.deviceIds) && detail.deviceIds.includes(deviceId);
}
/**
 * A device asking for this browser's registration, or for a signed request, does not
 * need the password while this browser still has its key: the next request registers
 * it again. Only a missing key or a removed browser needs an unlock.
 */
async function passwordWouldHelp(code: string) {
  if (code === 'browser_revoked' || code === 'accountUnlockRequired') return true;
  return !(await currentBrowser().catch(() => null));
}
function needsOwnerUnlock(error: unknown) {
  const code = (error as { code?: string } | null)?.code;
  return code === 'accountUnlockRequired' || code === 'browser_registration_required' || code === 'browser_revoked' ||
    code === 'device_strict_mode';
}

// Keep a blocked read attached to its original consumer. After an explicit
// unlock it completes the same load, including consumers inside the shared UI
// (history, documents, file trees, settings). Never use this for mutations.
export async function readAfterOwnerUnlock<T extends { response: Response }>(
  deviceId: string,
  read: () => Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  let generation = 0;
  let resume: (() => void) | undefined;
  let abort: (() => void) | undefined;
  const unlocked = (event: Event) => {
    if (!ownerUnlockedDevice(event, deviceId)) return;
    generation++;
    resume?.();
  };
  window.addEventListener(OWNER_UNLOCKED_EVENT, unlocked);
  let retried = false;
  try {
    for (;;) {
      signal?.throwIfAborted();
      const attempt = generation;
      let revoked = false;
      let outcome: { result: T } | { error: unknown };
      let code: string;
      try {
        const result = await read();
        if (result.response.ok || ![401, 403].includes(result.response.status))
          return result;
        const failure: unknown = await result.response.clone().json().catch(() => null);
        if (!needsOwnerUnlock(failure)) return result;
        code = (failure as { code: string }).code;
        outcome = { result };
      } catch (error) {
        if (!needsOwnerUnlock(error)) throw error;
        code = (error as { code: string }).code;
        outcome = { error };
      }
      revoked = code === 'browser_revoked';
      if (!await passwordWouldHelp(code)) {
        // Registering again happens on the retry; a device that still refuses
        // this browser is that device's problem, not a reason to ask for a password.
        if (!retried) { retried = true; continue; }
        if ('result' in outcome) return outcome.result;
        throw outcome.error;
      }
      // The gesture can finish while the encrypted failure is still in flight.
      if (generation !== attempt) continue;
      window.dispatchEvent(new CustomEvent<OwnerReadBlockedDetail>(OWNER_READ_BLOCKED_EVENT, {
        detail: { deviceId, revoked, code },
      }));
      if (generation !== attempt) continue;
      await new Promise<void>((resolve, reject) => {
        resume = resolve;
        abort = () => reject(signal!.reason);
        signal?.addEventListener('abort', abort, { once: true });
        if (signal?.aborted) abort();
      });
      if (abort) signal?.removeEventListener('abort', abort);
      resume = undefined;
      abort = undefined;
    }
  } finally {
    window.removeEventListener(OWNER_UNLOCKED_EVENT, unlocked);
    if (abort) signal?.removeEventListener('abort', abort);
  }
}
